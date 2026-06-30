import asyncio
import hashlib
import os
import shutil
import subprocess
import sys
import logging
import uuid
import fnmatch
import json
import re
import shlex
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import List, Dict, Any, Literal, Optional, Tuple

from devcouncil.app.config import load_config

from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.gap import Gap
from devcouncil.domain.evidence import TestEvidence, DiffEvidence, DiffCoverageEvidence, CommandResult
from devcouncil.verification import diff_coverage as dc
from devcouncil.gating.checks.secret_scan_check import SecretScanner
from devcouncil.verification.implementation_reviewer import ImplementationReviewer
from devcouncil.verification.acceptance_compiler import AcceptanceTestCompiler
from devcouncil.llm.router import ModelRouter
from devcouncil.utils.redaction import redact_string
from devcouncil.live.cards import unresolved_blocking_cards

logger = logging.getLogger(__name__)

IGNORED_CHANGE_PATTERNS = (
    "__pycache__/*",
    "*/__pycache__/*",
    "*.pyc",
    "*.pyo",
    ".pytest_cache/*",
    ".mypy_cache/*",
    ".ruff_cache/*",
    ".devcouncil/*",
    # DevCouncil manages the root .gitignore itself (ensure_gitignore runs on
    # init and before every task), so its drift is not task work.
    ".gitignore",
)

MAX_UNTRACKED_DIFF_BYTES = 256_000


@dataclass
class VerificationOutcome:
    """Non-gap metadata about HOW a verification run executed.

    The pass/fail verdict lives in the gaps; this records the *rigor* of the run so
    an autonomous agent never mistakes ``passed`` for ``proven`` when the gate could
    not actually check. ``mode`` is ``"compiled"`` when DevCouncil's per-criterion
    acceptance checks were available (a model router was supplied) and ``"coarse"``
    on the keyless fallback path. ``diff_empty`` flags a run with nothing to verify,
    and the coverage fields say whether the diff↔coverage gate measured anything.
    """

    mode: str = "coarse"
    compiler_active: bool = False
    diff_empty: bool = True
    coverage_measured: bool = False
    coverage_skipped_reason: Optional[str] = None

    def as_dict(self) -> Dict[str, Any]:
        return asdict(self)


class Verifier:
    def __init__(self, project_root: Path, router: Optional[ModelRouter] = None):
        self.project_root = project_root
        self._gap_counter = 0
        self.secret_scanner = SecretScanner()
        self.reviewer = ImplementationReviewer(router) if router else None
        self.acceptance_compiler = AcceptanceTestCompiler(router) if router else None
        # Metadata about the most recent verify_task run (rigor mode, diff/coverage
        # status). Populated at the end of verify_task; read by the MCP/CLI surfaces
        # so the agent knows whether the strong checks actually ran.
        self.last_outcome: Optional[VerificationOutcome] = None
        # Interpreter used to run diff-coverage instrumentation. None -> resolve the
        # target repo's ``python`` from the cleaned PATH (falling back to the current
        # interpreter). Overridable as a seam for deterministic tests.
        self._coverage_python: Optional[str] = None
        # When set, overrides the (measure, enforce, min_ratio) diff-coverage settings
        # that would otherwise come from config. Used by ad-hoc checks and tests.
        self._diff_coverage_override: Optional[Tuple[bool, bool, float]] = None
        # Per-verify_task memos (primed at verify_task entry, cleared before it returns)
        # so the hot path does not re-run `git ls-files` or re-load config repeatedly.
        # None outside a verify_task call, so all other callers behave exactly as before.
        self._untracked_cache: Optional[List[str]] = None
        self._command_timeout_cache: Optional[int] = None
        # Project dependency names (lower-cased), loaded once per verify_task and cleared
        # in its finally so a reused Verifier re-reads them for a later task.
        self._project_deps_cache: Optional[set] = None

    def _next_gap_id(self, task_id: str, suffix: str) -> str:
        """Generate unique gap IDs to prevent SQLite overwrites."""
        self._gap_counter += 1
        return f"GAP-{task_id}-{suffix}-{uuid.uuid4().hex[:6]}-{self._gap_counter:03d}"

    def get_diff(self) -> str:
        try:
            if not self._has_head():
                return self._get_initial_repo_diff()
            tracked_diff = subprocess.check_output(
                ["git", "diff", "HEAD"], cwd=self.project_root
            ).decode("utf-8", errors="replace")
            untracked_diff = self._get_untracked_files_diff()
            return "\n".join(part for part in [tracked_diff, untracked_diff] if part)
        except Exception as e:
            logger.warning("Failed to get git diff: %s", e)
            return ""

    def get_changed_files(self) -> List[str]:
        try:
            if not self._has_head():
                return self._get_status_files()
            output = subprocess.check_output(
                ["git", "diff", "HEAD", "--name-only"], cwd=self.project_root
            ).decode("utf-8", errors="replace").splitlines()
            files = set(output)
            files.update(self._get_untracked_files())
            return self._filter_change_paths(sorted(files))
        except Exception as e:
            logger.warning("Failed to get changed files: %s", e)
            return []

    def get_task_changed_files(self, task_id: str) -> List[str]:
        changed = set(self.get_changed_files())
        changed.difference_update(self._load_baseline_files())
        changed.difference_update(self._load_task_snapshot_files(task_id))
        return sorted(changed)

    def _committed_task_diff(self, task_id: str) -> str:
        """Diff of work committed since the task's ``before`` checkpoint, or "".

        When ``dev go`` commits a task's work (e.g. between self-repair attempts, or
        before the reconciliation pass), the working-tree diff (``git diff HEAD``) is
        empty even though the task is fully implemented. This recovers that committed
        change so acceptance compilation/review still have something to reason about
        instead of seeing an empty diff and skipping — which would mark every criterion
        unproven and wrongly block correct, committed code.
        """
        # Literal of CheckpointService.REF_BEFORE (kept inline to avoid a circular
        # import: checkpoints.py imports Verifier).
        before_ref = f"refs/devcouncil/tasks/{task_id}/before"
        try:
            has_ref = subprocess.run(
                ["git", "rev-parse", "--verify", before_ref],
                cwd=self.project_root,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            ).returncode == 0
            if has_ref:
                return subprocess.check_output(
                    ["git", "diff", before_ref],
                    cwd=self.project_root,
                    stderr=subprocess.DEVNULL,
                ).decode("utf-8", errors="replace")
        except Exception:
            pass
        return ""

    def _task_produced_changes(self, task_id: str) -> bool:
        """True when the task has a footprint beyond the current working-tree diff.

        Used so the empty-diff guard does not misfire on already-committed work: in
        ``dev go`` each task is committed and then re-verified by the reconciliation
        pass, at which point ``git diff HEAD`` is empty even though the task was fully
        implemented. We detect that via the task's ``before`` checkpoint ref (work
        committed since the task started) and a non-empty ``after`` patch. A genuine
        no-op run has neither, so it is still correctly flagged as empty.
        """
        if self._committed_task_diff(task_id).strip():
            return True
        after_patch = self.project_root / ".devcouncil" / "checkpoints" / f"{task_id}-after.patch"
        try:
            return after_patch.exists() and bool(after_patch.read_text(encoding="utf-8", errors="replace").strip())
        except Exception:
            return False

    def _has_head(self) -> bool:
        return subprocess.run(
            ["git", "rev-parse", "--verify", "HEAD"],
            cwd=self.project_root,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode == 0

    def _get_initial_repo_diff(self) -> str:
        parts: List[str] = []
        for cmd in (["git", "diff", "--cached"], ["git", "diff"]):
            result = subprocess.run(
                cmd,
                cwd=self.project_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            if result.returncode == 0 and result.stdout:
                parts.append(result.stdout)
        untracked_diff = self._get_untracked_files_diff()
        if untracked_diff:
            parts.append(untracked_diff)
        return "\n".join(parts)

    def _get_status_files(self) -> List[str]:
        files: set[str] = set()
        commands = (
            ["git", "diff", "--cached", "--name-only"],
            ["git", "diff", "--name-only"],
            ["git", "ls-files", "--others", "--exclude-standard"],
        )
        for cmd in commands:
            try:
                output = subprocess.check_output(
                    cmd,
                    cwd=self.project_root,
                    stderr=subprocess.DEVNULL,
                ).decode("utf-8", errors="replace").splitlines()
                files.update(path.replace("\\", "/") for path in output if path.strip())
            except subprocess.CalledProcessError:
                continue
        if not files:
            files.update(self._walk_project_files())
        return self._filter_change_paths(sorted(files))

    def _get_untracked_files(self) -> List[str]:
        # Per-verify_task memo: git ls-files is otherwise re-run via get_changed_files,
        # _get_untracked_files_diff, and _classify_change_paths. verify_task primes this
        # once; it is None for every other caller, so they recompute fresh as before.
        if self._untracked_cache is not None:
            return self._untracked_cache
        try:
            output = subprocess.check_output(
                ["git", "ls-files", "--others", "--exclude-standard"],
                cwd=self.project_root,
                stderr=subprocess.DEVNULL,
            ).decode("utf-8", errors="replace").splitlines()
            return self._filter_change_paths(output)
        except Exception as e:
            logger.debug("Failed to list untracked files: %s", e)
            return []

    def _get_untracked_files_diff(self) -> str:
        parts: List[str] = []
        for rel_path in self._get_untracked_files():
            full_path = self.project_root / rel_path
            if not full_path.is_file():
                continue
            parts.append(self._format_new_file_diff(rel_path, full_path))
        return "\n".join(part for part in parts if part)

    def _format_new_file_diff(self, rel_path: str, full_path: Path) -> str:
        try:
            raw = full_path.read_bytes()
        except Exception as e:
            logger.debug("Failed to read untracked file %s: %s", rel_path, e)
            return ""

        header = [
            f"diff --git a/{rel_path} b/{rel_path}",
            "new file mode 100644",
            "--- /dev/null",
            f"+++ b/{rel_path}",
        ]
        if b"\0" in raw[:8192]:
            return "\n".join([*header, f"Binary files /dev/null and b/{rel_path} differ"])

        truncated = len(raw) > MAX_UNTRACKED_DIFF_BYTES
        if truncated:
            raw = raw[:MAX_UNTRACKED_DIFF_BYTES]
        text = raw.decode("utf-8", errors="replace")
        lines = text.splitlines()
        if text.endswith(("\n", "\r")):
            line_count = len(lines)
        else:
            line_count = max(len(lines), 1 if text else 0)

        diff_lines = [*header, f"@@ -0,0 +1,{line_count} @@"]
        if not text:
            return "\n".join(header) + "\n"

        diff_lines.extend(f"+{line}" for line in lines)
        if truncated:
            diff_lines.append("+[devcouncil: untracked file diff truncated]")
        return "\n".join(diff_lines)

    def _walk_project_files(self) -> List[str]:
        files: List[str] = []
        for path in self.project_root.rglob("*"):
            if not path.is_file():
                continue
            rel = path.relative_to(self.project_root).as_posix()
            if rel.startswith(".git/"):
                continue
            files.append(rel)
        return files

    def _filter_change_paths(self, paths: List[str]) -> List[str]:
        return [
            path
            for path in (p.strip().replace("\\", "/") for p in paths)
            if path and not self._is_ignored_change(path)
        ]

    def _is_ignored_change(self, path: str) -> bool:
        return any(fnmatch.fnmatch(path, pattern) for pattern in IGNORED_CHANGE_PATTERNS)

    def _load_baseline_files(self) -> set[str]:
        return self._load_snapshot_files(self.project_root / ".devcouncil" / "baseline.json")

    def _load_task_snapshot_files(self, task_id: str) -> set[str]:
        return self._load_snapshot_files(
            self.project_root / ".devcouncil" / "checkpoints" / f"{task_id}-before.json"
        )

    def _load_snapshot_files(self, path: Path) -> set[str]:
        if not path.exists():
            return set()
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            return {
                item.replace("\\", "/")
                for item in data.get("changed_files", [])
                if isinstance(item, str)
            }
        except Exception as e:
            logger.warning("Failed to load verification snapshot %s: %s", path, e)
            return set()

    def _load_commands(self) -> Dict[str, List[str]]:
        try:
            config = load_config(self.project_root)
            return {
                "test": config.commands.test,
                "lint": config.commands.lint,
                "typecheck": config.commands.typecheck,
            }
        except Exception as e:
            logger.warning("Failed to load config commands: %s", e)
            return {}

    def _save_log(self, label: str, command: str, stream: str, content: str) -> str:
        """Save command output to a log file and return the path."""
        log_dir = self.project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        cmd_hash = hashlib.sha256(command.encode()).hexdigest()[:8]
        filename = f"{label}-{cmd_hash}-{stream}.log"
        log_path = log_dir / filename
        log_path.write_text(redact_string(content), encoding="utf-8")
        return str(log_path)

    def _verification_env(self) -> Dict[str, str]:
        """Environment for verification commands that does not leak DevCouncil's
        own virtualenv into the target repository.

        When DevCouncil is installed/run from a venv (e.g. ``uv tool install`` or
        a project ``.venv``), a bare ``python``/``pytest`` in a task's evidence
        command would otherwise resolve to DevCouncil's interpreter, which lacks
        the target project's dependencies — producing false ``No module named
        pytest`` style failures. Strip DevCouncil's venv from ``PATH`` and unset
        the virtualenv markers so commands resolve the project/system interpreter,
        exactly as they would in a plain terminal at the repo root.
        """
        env = dict(os.environ)
        venv_prefix = Path(sys.prefix).resolve()
        base_prefix = Path(getattr(sys, "base_prefix", sys.prefix)).resolve()
        if venv_prefix == base_prefix:
            return env  # Not running inside a venv; nothing to strip.

        venv_dirs = {
            str(venv_prefix).lower(),
            str((venv_prefix / "Scripts").resolve()).lower(),
            str((venv_prefix / "bin").resolve()).lower(),
        }
        path = env.get("PATH", "")
        kept = []
        for entry in path.split(os.pathsep):
            if not entry:
                continue
            try:
                normalized = str(Path(entry).resolve()).lower()
            except Exception:
                normalized = entry.lower()
            if normalized in venv_dirs:
                continue
            kept.append(entry)
        env["PATH"] = os.pathsep.join(kept)

        # Drop the virtualenv-activation markers that would pin a freshly-resolved
        # child ``python`` back to DevCouncil's interpreter. VIRTUAL_ENV points at
        # the venv (sys.prefix); PYTHONHOME — set by uv-managed interpreters — points
        # at the base interpreter (sys.base_prefix) and forcibly overrides the stdlib
        # / site-packages location of ANY python the child invokes, which is what
        # makes ``python -m pytest`` fail with "No module named pytest" even when the
        # project's interpreter has pytest installed.
        own_prefixes = {str(venv_prefix), str(base_prefix)}
        for marker in ("VIRTUAL_ENV", "PYTHONHOME"):
            value = env.get(marker)
            if not value:
                continue
            try:
                resolved = str(Path(value).resolve())
            except Exception:
                resolved = value
            if resolved in own_prefixes:
                env.pop(marker, None)
        # uv stashes the same path here and re-applies it to child pythons.
        env.pop("UV_INTERNAL__PYTHONHOME", None)
        return env

    @staticmethod
    def _summarize_stream(content: str, budget: int = 360) -> str:
        """Condense a command's stdout/stderr for the evidence summary so the ACTUAL
        error survives downstream truncation.

        Plain ``content[-500:]`` kept the tail but the combined summary is later clipped
        to its first 500 chars at the gap-evidence sites, which dropped the exception
        line entirely. We hoist the salient error line (the last non-indented line, where
        Python prints the exception) to the front, then append bounded context."""
        if not content or not content.strip():
            return "(empty)"
        lines = [ln.rstrip() for ln in content.splitlines() if ln.strip()]
        markers = ("error", "exception", "assert", "traceback", "failed", "not found", "no module named")
        salient = ""
        for ln in reversed(lines):
            low = ln.lower()
            if any(m in low for m in markers):
                salient = ln.strip()
                break
        if not salient:
            salient = lines[-1].strip()
        salient = salient[:240]  # cap a single huge (e.g. minified) line
        tail = content.strip()[-budget:]
        summary = f"{salient} | {tail}" if salient not in tail[: len(salient) + 5] else tail
        return summary[: budget + len(salient) + 8]

    def _run_command(self, command: str, task_id: str = "verify") -> CommandResult:
        # Per-verify_task memo: avoid re-loading config for the timeout on every command
        # in the expected_tests / allowed_commands / compiled-check loops. Falls back to
        # loading config when called outside verify_task (cache is None).
        if self._command_timeout_cache is not None:
            timeout = self._command_timeout_cache
        else:
            try:
                config = load_config(self.project_root)
                timeout = config.execution.command_timeout
            except Exception:
                timeout = 300

        env = self._verification_env()
        argv = self._split_command(command)
        # Resolve the program to an absolute path against the (cleaned) PATH.
        # On Windows, CreateProcess searches the launching executable's own
        # directory before PATH, so a bare ``python`` would otherwise pick up
        # DevCouncil's bundled interpreter (in .venv\Scripts) regardless of PATH.
        # Resolving here pins the command to the project/system interpreter.
        if argv:
            resolved = shutil.which(argv[0], path=env.get("PATH"))
            if resolved:
                argv = [resolved, *argv[1:]]

        try:
            result = subprocess.run(
                argv,
                shell=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                timeout=timeout,
                env=env,
            )
            stdout = result.stdout or ""
            stderr = result.stderr or ""
            stdout_path = self._save_log(task_id, command, "stdout", stdout)
            stderr_path = self._save_log(task_id, command, "stderr", stderr)
            stdout_summary = redact_string(self._summarize_stream(stdout))
            stderr_summary = redact_string(self._summarize_stream(stderr))
            return CommandResult(
                command=command,
                exit_code=result.returncode,
                stdout_path=stdout_path,
                stderr_path=stderr_path,
                # stderr first: downstream evidence clips summary[:500], so the error
                # line must land in the first 500 chars to stay diagnosable.
                summary=(
                    f"Exit code {result.returncode}. "
                    f"stderr: {stderr_summary}. "
                    f"stdout: {stdout_summary}"
                ),
            )
        except Exception as e:
            return CommandResult(
                command=command,
                exit_code=-1,
                stdout_path="",
                stderr_path="",
                summary=f"Failed to run command: {e}",
            )

    def _split_command(self, command: str) -> List[str]:
        # Use POSIX splitting so quotes are interpreted, not preserved. With
        # posix=False, `python -c "assert x"` keeps the surrounding quotes, so the
        # interpreter receives the literal string `"assert x"` and treats it as a
        # no-op string expression that exits 0 — every quoted-argument evidence
        # command would then silently "pass" without running, producing false
        # verification. posix=True strips the quotes correctly; planner-generated
        # commands use forward-slash paths, which the interpreter accepts on Windows.
        return shlex.split(command, posix=True)

    def _check_dependency_changes(self, changed_files: List[str]) -> List[str]:
        dep_files = {
            "package.json", "package-lock.json", "yarn.lock", "pnpm-lock.yaml",
            "requirements.txt", "pyproject.toml", "uv.lock", "Pipfile.lock",
            "go.mod", "go.sum", "Cargo.toml", "Cargo.lock",
        }
        return [f for f in changed_files if Path(f).name in dep_files]

    def _classify_change_paths(self, changed_files: List[str]) -> Tuple[List[str], List[str]]:
        changed_set = set(changed_files)
        added = set(self._get_untracked_files())
        deleted: set[str] = set()
        try:
            output = subprocess.check_output(
                ["git", "diff", "HEAD", "--name-status"],
                cwd=self.project_root,
                stderr=subprocess.DEVNULL,
            ).decode("utf-8", errors="replace").splitlines()
            for line in output:
                parts = line.split("\t")
                if len(parts) < 2:
                    continue
                status = parts[0]
                path = parts[-1].replace("\\", "/")
                if status.startswith("A"):
                    added.add(path)
                elif status.startswith("D"):
                    deleted.add(path)
        except Exception as e:
            logger.debug("Failed to classify changed files: %s", e)
        return sorted(added & changed_set), sorted(deleted & changed_set)

    def _diff_coverage_settings(self) -> Tuple[bool, bool, float]:
        """Return (measure, enforce, min_ratio) with safe defaults when unconfigured."""
        if self._diff_coverage_override is not None:
            return self._diff_coverage_override
        try:
            cfg = load_config(self.project_root).verification.diff_coverage
            return bool(cfg.measure), bool(cfg.enforce), float(cfg.min_ratio)
        except Exception:
            return True, False, 0.0

    def _resolve_coverage_python(self, env: Dict[str, str]) -> str:
        if self._coverage_python:
            return self._coverage_python
        for name in ("python", "python3", "py"):
            found = shutil.which(name, path=env.get("PATH"))
            if found:
                return found
        return sys.executable

    def _coverage_available(self, python: str, env: Dict[str, str]) -> bool:
        try:
            result = subprocess.run(
                [python, "-m", "coverage", "--version"],
                cwd=self.project_root,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=30,
                env=env,
            )
            return result.returncode == 0
        except Exception:
            return False

    def _coverage_target_commands(self, task: Task) -> List[str]:
        """The test command(s) to instrument — the ones that purport to prove the ACs."""
        if task.expected_tests:
            return list(task.expected_tests)
        test_like = [c for c in task.allowed_commands if self._command_can_prove_acceptance("allowed", c)]
        if test_like:
            return test_like
        return list(self._load_commands().get("test", []))

    def measure_diff_coverage(self, task: Task, diff_content: str) -> dc.DiffCoverageResult:
        """Run the task's test command(s) under coverage and intersect with the diff.

        Returns an *unmeasured* result (never a false positive) whenever reliable
        data is unavailable: no measurable Python changes, no instrumentable test
        command, or no coverage tool in the target environment.
        """
        changed = dc.measurable_python_changes(dc.parse_changed_lines(diff_content))
        if not changed:
            return dc.DiffCoverageResult(measured=False, reason="no measurable Python changes in diff")
        commands = self._coverage_target_commands(task)
        if not commands:
            return dc.DiffCoverageResult(measured=False, reason="no test command to instrument")

        env = self._verification_env()
        python = self._resolve_coverage_python(env)
        if not self._coverage_available(python, env):
            return dc.DiffCoverageResult(measured=False, reason="coverage tool not available in target environment")

        try:
            timeout = load_config(self.project_root).execution.command_timeout
        except Exception:
            timeout = 300

        tmp_dir = self.project_root / ".devcouncil" / "tmp"
        tmp_dir.mkdir(parents=True, exist_ok=True)
        data_file = tmp_dir / f"diffcov-{task.id}.coverage"
        json_file = tmp_dir / f"diffcov-{task.id}.json"
        for stale in (data_file, json_file):
            try:
                stale.unlink()
            except FileNotFoundError:
                pass

        ran_any = False
        append = False
        inline_scripts: List[Path] = []
        try:
            for idx, cmd in enumerate(commands):
                argv = self._split_command(cmd)
                inline = dc.inline_python_code(argv)
                if inline is not None:
                    # Materialise `python -c "CODE"` as a temp script so coverage can
                    # instrument it (coverage cannot run a bare -c snippet).
                    script = tmp_dir / f"diffcov-inline-{task.id}-{idx}.py"
                    try:
                        script.write_text(dc.inline_script_content(inline, self.project_root), encoding="utf-8")
                    except Exception as exc:
                        logger.warning("Diff-coverage inline script write failed for %s: %s", task.id, exc)
                        continue
                    inline_scripts.append(script)
                    cov_argv: Optional[List[str]] = dc.coverage_run_script_argv(
                        str(script), python, append=append, data_file=str(data_file)
                    )
                else:
                    cov_argv = dc.coverage_run_argv(argv, python, append=append, data_file=str(data_file))
                if cov_argv is None:
                    continue
                try:
                    subprocess.run(
                        cov_argv,
                        cwd=self.project_root,
                        capture_output=True,
                        text=True,
                        encoding="utf-8",
                        errors="replace",
                        timeout=timeout,
                        env=env,
                    )
                except Exception as exc:
                    logger.warning("Diff-coverage run failed for %s: %s", task.id, exc)
                    continue
                ran_any = True
                append = True

            if not ran_any:
                return dc.DiffCoverageResult(measured=False, reason="no instrumentable test command")
            if not data_file.exists():
                return dc.DiffCoverageResult(measured=False, reason="coverage produced no data")

            try:
                subprocess.run(
                    [python, "-m", "coverage", "json", f"--data-file={data_file}", "-o", str(json_file)],
                    cwd=self.project_root,
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    timeout=120,
                    env=env,
                )
                data = json.loads(json_file.read_text(encoding="utf-8"))
            except Exception as exc:
                return dc.DiffCoverageResult(measured=False, reason=f"coverage report unreadable: {exc}")

            coverage = dc.parse_coverage_json(data, self.project_root)
            return dc.intersect(changed, coverage, tool="coverage.py")
        finally:
            for path in [data_file, json_file, *inline_scripts]:
                try:
                    path.unlink()
                except OSError:
                    pass

    async def verify_task(self, task: Task, requirements: List[Requirement]) -> Tuple[List[Gap], List[Any]]:
        logger.info("verify_task: task=%s requirements=%d", task.id, len(requirements))
        self._gap_counter = 0
        gaps: List[Gap] = []
        evidence_to_save: List[Any] = []
        # Prime the per-call memos: compute the untracked-file list once (otherwise
        # re-run by get_changed_files, get_diff, and _classify_change_paths) and load the
        # command timeout once (otherwise re-loaded by _run_command on every command).
        # Both are cleared before this method returns.
        self._untracked_cache = self._get_untracked_files()
        ac_samples, ac_repair_attempts = 1, 1
        try:
            _cfg = load_config(self.project_root)
            self._command_timeout_cache = _cfg.execution.command_timeout
            ac_samples = max(1, _cfg.verification.acceptance_checks.samples)
            ac_repair_attempts = max(0, _cfg.verification.acceptance_checks.repair_attempts)
            ac_per_criterion = bool(_cfg.verification.acceptance_checks.per_criterion)
        except Exception:
            self._command_timeout_cache = 300
            ac_per_criterion = False
        changed_files = self.get_task_changed_files(task.id)
        diff_content = self.get_diff()
        # When the working tree is clean but the task's work was committed (dev go commits
        # between repair attempts and before reconciliation), fall back to the committed
        # checkpoint diff. Otherwise acceptance compilation/review below — gated on a
        # non-empty diff_content — would be skipped, leaving every criterion unproven and
        # wrongly blocking correct, already-committed code.
        if not diff_content.strip():
            committed_diff = self._committed_task_diff(task.id)
            if committed_diff.strip():
                diff_content = committed_diff
        diff_empty = not bool(diff_content.strip())
        # Launch the two independent LLM passes — acceptance compilation and the advisory
        # implementation review — concurrently as soon as the diff is available, instead
        # of awaiting them sequentially later. Each depends only on (task, requirements,
        # diff_content), so there is no data hazard; each result is awaited (with its
        # existing try/except) at the point it is consumed below. The create-time guards
        # match the consume-time guards exactly, so every task created is always awaited.
        compile_future: Optional["asyncio.Task[Dict[str, List[str]]]"] = None
        if self.acceptance_compiler and diff_content and task.acceptance_criterion_ids:
            # Prefer the self-consistency interface; fall back to single-shot ``compile`` so
            # older compiler doubles/implementations keep working.
            if hasattr(self.acceptance_compiler, "compile_candidates"):
                _compile_coro = self.acceptance_compiler.compile_candidates(
                    task, requirements, diff_content, samples=ac_samples,
                    per_criterion=ac_per_criterion,
                )
            else:
                _compile_coro = self.acceptance_compiler.compile(task, requirements, diff_content)
            compile_future = asyncio.create_task(_compile_coro)
        review_future: Optional["asyncio.Task[Any]"] = None
        if self.reviewer and diff_content:
            review_future = asyncio.create_task(
                self.reviewer.review_changes(task, requirements, diff_content)
            )
        try:
            # "Work present" is broader than the current working-tree diff: a task whose
            # changes were already committed (e.g. `dev go`'s per-task commit, then the
            # final reconciliation pass where `git diff HEAD` is empty) still counts as
            # implemented. A genuine no-op run has neither a working diff nor committed
            # changes since the task's checkpoint.
            work_present = (not diff_empty) or self._task_produced_changes(task.id)

            # Empty-diff guard. If the task declares files to create or modify but produced
            # NO work at all, there is nothing to prove — an agent must not be able to
            # declare victory having written nothing (or after a transient git error that
            # degraded the diff to ""). This is the single most dangerous false-pass for
            # autonomy, so it blocks regardless of which commands ran.
            expects_change = any(pf.allowed_change != "read_only" for pf in task.planned_files)
            if not work_present and expects_change:
                gaps.append(Gap(
                    id=self._next_gap_id(task.id, "NODIFF"),
                    severity="high",
                    gap_type="task_not_implemented",
                    task_id=task.id,
                    description=(
                        f"Task {task.id} declares files to create or modify, but produced no "
                        "changes. Verification cannot prove work that does not exist."
                    ),
                    evidence=[f"planned files expecting change: {sorted(p.path for p in task.planned_files if p.allowed_change != 'read_only')}"],
                    recommended_fix=(
                        "Implement the planned changes so the diff is non-empty, then re-verify. "
                        "If you did make changes, ensure they are saved and visible to git "
                        "(not reverted, stashed, or written outside the project root)."
                    ),
                    blocking=True,
                ))

            if diff_content:
                added_files, deleted_files = self._classify_change_paths(changed_files)
                diff_ev = DiffEvidence(
                    task_id=task.id,
                    changed_files=changed_files,
                    added_files=added_files,
                    deleted_files=deleted_files,
                    diff_summary=f"Diff captured for {len(changed_files)} files."
                )
                evidence_to_save.append(diff_ev)

            # 1. Planned-file coverage check
            planned_paths = {pf.path for pf in task.planned_files}
            changed_set = set(changed_files)
            for pf in task.planned_files:
                if pf.path not in changed_set and pf.allowed_change != "read_only":
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, "FILE"),
                        severity="medium",
                        gap_type="planned_file_not_changed",
                        task_id=task.id,
                        description=f"Planned file {pf.path} was not modified.",
                        recommended_fix=f"Modify {pf.path} as planned or update the task.",
                        blocking=False,
                        file=pf.path,
                    ))

            # 2. Orphan-diff detection
            for cf in changed_files:
                if cf not in planned_paths:
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, "ORPHAN"),
                        severity="high",
                        gap_type="orphan_diff",
                        task_id=task.id,
                        description=f"File {cf} was modified but not planned for this task.",
                        evidence=[cf],
                        recommended_fix=f"Revert changes to {cf} or add it to the task's planned files.",
                        blocking=True,
                        file=cf,
                    ))

            gaps.extend(self._check_semantic_diff(task, requirements))

            # 3. Dependency change detection
            dep_changes = self._check_dependency_changes(changed_files)
            for dep_file in dep_changes:
                if dep_file not in planned_paths:
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, "DEP"),
                        severity="high",
                        gap_type="dependency_risk",
                        task_id=task.id,
                        description=f"Dependency file {dep_file} was modified without being in planned files.",
                        evidence=[dep_file],
                        recommended_fix=f"Justify the dependency change or revert {dep_file}.",
                        blocking=True,
                        file=dep_file,
                    ))

            # When DevCouncil can compile its own per-criterion checks, THOSE are the
            # authority and the planner's expected_tests are demoted to advisory — so a
            # bogus planner command (irrelevant linters, npm on a Python project, tests
            # that reference missing files) can no longer block correct work.
            compiler_active = bool(self.acceptance_compiler and diff_content and task.acceptance_criterion_ids)

            # 4. Run verification commands
            command_results: List[CommandResult] = []
            evidence_results: List[CommandResult] = []
            genuine_failure = False  # a command that actually ran and failed (real defect signal)
            had_unrunnable = False   # a command that could not run (missing tool / missing tests)
            # Genuine test failures demoted to non-blocking only because a compiler is active.
            # That demotion is legitimate ONLY if the compiler actually produces per-criterion
            # checks to take authority; re-promoted below if it produces none.
            demoted_failures: List[Gap] = []
            for cmd_type, cmds in self._commands_for_task(task).items():
                for cmd in cmds:
                    applicable, skip_reason = self._command_applicable(cmd)
                    if not applicable:
                        # Wrong-stack command (e.g. `npm test` on a Python repo): skip it
                        # entirely rather than running and failing for a stack reason — an
                        # advisory note so the skip is visible (no silent drop).
                        gaps.append(Gap(
                            id=self._next_gap_id(task.id, "SKIP"),
                            severity="low",
                            gap_type="skipped_verification_command",
                            task_id=task.id,
                            description=f"Skipped verification command '{cmd}': {skip_reason}.",
                            evidence=[skip_reason],
                            recommended_fix=(
                                "Replace it with a command for this repo's stack, or remove it "
                                "from .devcouncil/config.yaml / the task's expected_tests."
                            ),
                            blocking=False,
                            suggested_command=cmd,
                        ))
                        continue
                    result = self._run_command(cmd, task_id=task.id)
                    command_results.append(result)
                    evidence_to_save.append(result)
                    if self._command_can_prove_acceptance(cmd_type, cmd):
                        evidence_results.append(result)
                    if result.exit_code != 0:
                        if self._command_is_malformed(result):
                            had_unrunnable = True
                            # The verification command itself could not run (e.g. a
                            # SyntaxError in a `python -c` one-liner, or a missing test
                            # tool). This proves nothing about the implementation, so do
                            # not report it as a code failure — surface it as a plan/
                            # command defect the user can regenerate instead.
                            gaps.append(Gap(
                                id=self._next_gap_id(task.id, "BADCMD"),
                                severity="medium",
                                gap_type="invalid_verification_command",
                                task_id=task.id,
                                description=(
                                    f"Verification command could not run (not a code failure): '{cmd}'. "
                                    "It appears malformed or its tooling is unavailable, so this command "
                                    "proves nothing either way."
                                ),
                                evidence=[result.summary[:500]],
                                recommended_fix=(
                                    "Regenerate the task's verification commands with 'dev repair', or edit "
                                    "them to be a single runnable command (e.g. 'python -m pytest <file>')."
                                ),
                                # Non-blocking: a command that cannot run is not evidence of a
                                # defect. If it was the *only* check for an acceptance criterion,
                                # that criterion is independently caught as unproven (blocking).
                                blocking=False,
                                suggested_command=cmd,
                                stdout_path=result.stdout_path or None,
                                stderr_path=result.stderr_path or None,
                            ))
                        else:
                            # A verification command that genuinely failed. Lint/typecheck
                            # commands (from the config fallback) report style/type opinion,
                            # not a correctness defect, so they are ADVISORY — blocking a
                            # behaviorally-correct task on `flake8`/`mypy`/`ruff` is the
                            # false-block the benchmark surfaced. A real test failure still
                            # gates (unless compiled checks supersede it).
                            is_quality_gate = cmd_type in {"lint", "typecheck"} or self._is_quality_only_command(cmd)
                            blocking = (not compiler_active) and not is_quality_gate
                            if blocking:
                                genuine_failure = True
                            fail_file, fail_line = self._failure_location(result)
                            gap = Gap(
                                id=self._next_gap_id(task.id, cmd_type.upper()),
                                severity="high" if blocking else "medium",
                                gap_type="quality_gate_failed" if is_quality_gate else "test_failed",
                                task_id=task.id,
                                description=(
                                    f"{'Quality gate' if is_quality_gate else 'Command'} '{cmd}' "
                                    f"failed with exit code {result.exit_code}"
                                    + (" (advisory: style/type, not a correctness gate)." if is_quality_gate else ".")
                                ),
                                evidence=[result.summary[:500]],
                                recommended_fix=f"Fix the issues reported by '{cmd}'.",
                                blocking=blocking,
                                suggested_command=cmd,
                                file=fail_file,
                                line=fail_line,
                                stdout_path=result.stdout_path or None,
                                stderr_path=result.stderr_path or None,
                            )
                            gaps.append(gap)
                            # A real test failure demoted only because the compiler is active:
                            # remember it so we can re-promote if the compiler yields no checks.
                            if compiler_active and not is_quality_gate and not blocking:
                                demoted_failures.append(gap)

            # 4b. Compiled acceptance checks — precise, DevCouncil-owned per-criterion
            # evidence. Derive one runnable check per acceptance criterion from the
            # criterion text + the diff, instead of trusting planner-authored
            # expected_tests (which the benchmark showed often reference absent tools or
            # test files). Each check maps 1:1 to its criterion, replacing the coarse
            # "any command passed -> every criterion proven" mapping.
            compiled_pass: Dict[str, bool] = {}
            # Per-AC bookkeeping so the unproven-AC gap can attach ONLY the check(s) that
            # targeted that criterion (and the specific failing result), instead of dumping
            # every command summary. Keys are AC ids; values track the compiled command(s)
            # and any failing CommandResults for that AC.
            compiled_cmds_by_ac: Dict[str, List[str]] = {}
            failing_results_by_ac: Dict[str, List[CommandResult]] = {}
            # Per-AC vote tally for proven criteria: {ac_id: (passes, decisive, repaired)}.
            # Recorded into the stored TestEvidence so an audit can see HOW a criterion was
            # proven (single check vs. majority of independent checks; whether a check had to
            # be repaired to run) instead of just "passed".
            compiled_vote: Dict[str, Tuple[int, int, bool]] = {}
            # ACs whose independently-generated checks split (some pass, some fail) with no
            # majority. Per policy this is inconclusive — neither proof nor a defect — so the
            # AC is surfaced NON-blocking below instead of false-blocking on a lone bad check.
            inconclusive_acs: set[str] = set()
            if compile_future is not None:
                try:
                    compiled = await compile_future
                except Exception as exc:  # pragma: no cover - best effort
                    logger.warning("Acceptance compiler failed for %s: %s", task.id, exc)
                    compiled = {}
                ac_meta = {ac.id: ac for req in requirements for ac in req.acceptance_criteria}
                for ac_id, raw_cmds in compiled.items():
                    # Defensive: drop any wrong-stack candidate so it can't fail an AC for a
                    # stack reason (the compiler is told not to emit these).
                    candidates = [c for c in raw_cmds if self._command_applicable(c)[0]]
                    compiled_cmds_by_ac[ac_id] = list(candidates)
                    if not candidates:
                        compiled_pass[ac_id] = False
                        continue
                    # Run each INDEPENDENT candidate; a check that merely failed to RUN
                    # (malformed/unrunnable) is regenerated from the launcher error up to
                    # ``ac_repair_attempts`` times — safe, because a check that never ran
                    # proves nothing, so repairing it cannot weaken the gate.
                    passes = 0
                    genuine_fails = 0
                    repaired = False  # a check had to be regenerated before it ran
                    fail_results: List[Tuple[str, CommandResult]] = []
                    for cmd in candidates:
                        result = self._run_command(cmd, task_id=task.id)
                        command_results.append(result)
                        evidence_to_save.append(result)
                        attempts = 0
                        _repair = getattr(self.acceptance_compiler, "repair", None)
                        while (
                            result.exit_code != 0
                            and self._command_is_malformed(result)
                            and attempts < ac_repair_attempts
                            and _repair is not None
                        ):
                            attempts += 1
                            ac_desc = ac_meta[ac_id].description if ac_id in ac_meta else ac_id
                            try:
                                fixed = await _repair(
                                    ac_id, ac_desc, cmd, result.summary[:800], diff_content
                                )
                            except Exception:
                                fixed = None
                            if not fixed or not self._command_applicable(fixed)[0]:
                                break
                            cmd = fixed
                            compiled_cmds_by_ac[ac_id].append(cmd)
                            result = self._run_command(cmd, task_id=task.id)
                            command_results.append(result)
                            evidence_to_save.append(result)
                        if result.exit_code == 0:
                            passes += 1
                            if attempts > 0:
                                repaired = True
                        elif self._command_is_malformed(result):
                            # Still couldn't run after repair: proves nothing either way.
                            had_unrunnable = True
                            failing_results_by_ac.setdefault(ac_id, []).append(result)
                        else:
                            genuine_fails += 1
                            fail_results.append((cmd, result))
                            failing_results_by_ac.setdefault(ac_id, []).append(result)
                    decisive = passes + genuine_fails
                    # Majority vote over the checks that actually ran. Proven iff a strict
                    # majority pass; unanimous failure of independent checks is strong evidence
                    # of a real defect and blocks; a split is inconclusive (handled below).
                    ac_proven = decisive > 0 and passes > genuine_fails
                    compiled_pass[ac_id] = ac_proven
                    if ac_proven:
                        compiled_vote[ac_id] = (passes, decisive, repaired)
                        continue
                    if passes == 0 and genuine_fails > 0:
                        genuine_failure = True
                        cmd, result = fail_results[0]
                        fail_file, fail_line = self._failure_location(result)
                        agree = (
                            f" {genuine_fails}/{decisive} independent checks agreed it fails."
                            if decisive > 1 else ""
                        )
                        gaps.append(Gap(
                            id=self._next_gap_id(task.id, "ACCHK"),
                            severity="high",
                            gap_type="test_failed",
                            task_id=task.id,
                            description=f"Acceptance check for {ac_id} failed: '{cmd}' (exit {result.exit_code}).{agree}",
                            evidence=[result.summary[:500]],
                            recommended_fix=f"Fix the implementation so acceptance criterion {ac_id} holds.",
                            blocking=True,
                            acceptance_criterion_id=ac_id,
                            suggested_command=cmd,
                            file=fail_file,
                            line=fail_line,
                            stdout_path=result.stdout_path or None,
                            stderr_path=result.stderr_path or None,
                        ))
                    elif passes > 0 and genuine_fails > 0:
                        # Independent checks disagree with no majority: neither proof nor a
                        # defect. Mark inconclusive so the unproven-AC gap below is NON-blocking
                        # (never false-block on a lone bad check, never auto-pass a real bug).
                        inconclusive_acs.add(ac_id)

            # The compiler only earns the authority to demote a genuinely-failing planner
            # test if it produced a per-criterion check for EVERY targeted AC. A partial
            # compile is not enough: the uncovered ACs fall back to the coarse signal, so a
            # demoted real failure + coarse-proven remainder would otherwise slip past the
            # gate. If coverage is incomplete (or zero — empty compile / all-wrong-stack /
            # a compile exception swallowed to {}), re-promote the demoted failures.
            compiler_covered_all = bool(task.acceptance_criterion_ids) and all(
                compiled_cmds_by_ac.get(ac_id) for ac_id in task.acceptance_criterion_ids
            )
            if compiler_active and not compiler_covered_all and demoted_failures:
                for gap in demoted_failures:
                    gap.blocking = True
                    gap.severity = "high"
                    genuine_failure = True
                    logger.info(
                        "Re-promoted demoted test failure %s to blocking: acceptance compiler "
                        "did not produce a check for every criterion of task %s.",
                        gap.id, task.id,
                    )

            # 5. Acceptance-criteria evidence mapping (precise, per criterion).
            # Quality-only commands (lint/typecheck) are excluded: a passing `mypy`/`ruff
            # check`/`tsc` exercises no behavior, so it must not coarse-prove a behavioral AC
            # — the same false-confidence the per-criterion checks exist to prevent.
            successful_commands = [
                result for result in evidence_results
                if result.exit_code == 0 and not self._is_quality_only_command(result.command)
            ]
            # Coarse fallback (used only when no compiled per-criterion check exists for an
            # AC): a criterion may be marked proven by a passing acceptance-capable command
            # ONLY when the task actually produced work. Without this guard a no-op run
            # whose unrelated command happens to pass would "prove" every criterion against
            # zero changes.
            coarse_proof_available = work_present and bool(successful_commands)
            if task.acceptance_criterion_ids:
                req_by_ac = {ac.id: req.id for req in requirements for ac in req.acceptance_criteria}
                unproven_acs: List[str] = []
                coarse_proven_acs: List[str] = []
                for ac_id in task.acceptance_criterion_ids:
                    # An AC is proven if its compiled check passed; if no compiled check
                    # exists for it, fall back to the coarse signal (any expected_test passed).
                    proven: Optional[bool] = compiled_pass.get(ac_id)
                    coarse = False
                    if proven is None:
                        proven = coarse_proof_available
                        coarse = proven  # proven only by the coarse, not-AC-specific signal
                    if proven:
                        if coarse:
                            coarse_proven_acs.append(ac_id)
                        # Don't persist a "passed" record for a coarse-proven criterion during a
                        # run that also has a genuine blocking failure — the gate already fails,
                        # and a stored "passed" would mislead audits that read evidence directly.
                        if not (coarse and genuine_failure):
                            proof_mode: Literal["compiled", "vote", "coarse", ""]
                            if coarse:
                                proof_summary = (
                                    "Acceptance criterion proven only by a COARSE signal (a passing "
                                    "acceptance-capable command, not a per-criterion check); behavior "
                                    "not precisely verified."
                                )
                                proof_mode = "coarse"
                            else:
                                # Make the per-criterion proof auditable: single check vs. majority
                                # of independent checks, and whether a check had to be repaired to run.
                                passes_n, decisive_n, was_repaired = compiled_vote.get(ac_id, (1, 1, False))
                                proof_mode = "vote" if decisive_n > 1 else "compiled"
                                how = (
                                    f"a majority vote of independent compiled checks ({passes_n}/{decisive_n} passed)"
                                    if decisive_n > 1 else
                                    "a per-criterion compiled check"
                                )
                                repaired_note = " (one check was regenerated from its launcher error to run)" if was_repaired else ""
                                proof_summary = f"Acceptance criterion proven by {how}.{repaired_note}"
                            evidence_to_save.append(TestEvidence(
                                requirement_id=req_by_ac.get(ac_id, task.requirement_ids[0] if task.requirement_ids else ""),
                                acceptance_criterion_id=ac_id,
                                command="(devcouncil acceptance check)",
                                status="passed",
                                evidence_summary=proof_summary,
                                mode=proof_mode,
                            ))
                    else:
                        unproven_acs.append(ac_id)
                # Surface coarse proof as a first-class advisory: these criteria passed only
                # because some acceptance-capable command exited 0, not because a check tied
                # to the criterion passed. Non-blocking, but no longer invisible.
                if coarse_proven_acs:
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, "COARSE"),
                        severity="low",
                        gap_type="coarse_acceptance_proof",
                        task_id=task.id,
                        description=(
                            "Verification mode = COARSE for "
                            f"{', '.join(coarse_proven_acs)}: proven by a passing acceptance-capable "
                            "command, not a per-criterion check. Behavior is not precisely verified."
                        ),
                        evidence=[f"coarse-proven: {', '.join(coarse_proven_acs)}"],
                        recommended_fix=(
                            "Add a verification command (or test) that exercises each listed criterion "
                            "specifically, so DevCouncil can compile a per-criterion check instead of "
                            "relying on the coarse fallback."
                        ),
                        blocking=False,
                    ))
                if unproven_acs:
                    # Block only on positive evidence of a problem. If verification was
                    # attempted but every failure was unrunnable (missing tooling / tests)
                    # and nothing genuinely failed, that is a verification defect, not a
                    # code defect — surface it as a non-blocking "could not verify".
                    couldnt_verify = had_unrunnable and not genuine_failure and work_present
                    ac_by_id = {ac.id: ac for req in requirements for ac in req.acceptance_criteria}
                    # Methods whose criteria HARD-BLOCK the gate when unproven: only those
                    # that assert BEHAVIOR. Inherently-manual criteria (manual/llm_review),
                    # optional ones, and quality-only `static_check` criteria (PEP 8 /
                    # docstring / formatting) are surfaced for review instead of
                    # false-blocking the autonomous loop. static_check is a quality gate,
                    # not a correctness gate — mirroring how lint/type COMMAND failures are
                    # already demoted to advisory — and the compiler often cannot author a
                    # reliable style check (or the criterion lands on a no-diff process task),
                    # which otherwise blocks correct, style-conforming code.
                    automatable_methods = {"unit_test", "integration_test"}
                    for ac_id in unproven_acs:
                        ac = ac_by_id.get(ac_id)
                        method = ac.verification_method if ac else "unit_test"
                        is_automatable = (ac.required if ac else True) and method in automatable_methods
                        if not is_automatable:
                            blocks = False
                            optional = "" if (ac is None or ac.required) else " optional"
                            fix = (
                                f"This{optional} criterion's verification method is '{method}'; it cannot be "
                                "proven by running code. Review it manually (it does not block the gate)."
                            )
                            suffix = f" (non-blocking: {method})"
                        elif ac_id in inconclusive_acs:
                            # Independently-generated checks split with no majority — inconclusive,
                            # so this does not block (a lone bad check must not fail correct code).
                            blocks = False
                            fix = ("Auto-generated acceptance checks disagreed on this criterion (some "
                                   "passed, some failed). Add a precise verification command that "
                                   "unambiguously proves it so the result is decisive.")
                            suffix = " (auto-checks inconclusive)"
                        elif couldnt_verify:
                            blocks = False
                            fix = ("Could not verify this criterion: the verification commands did not run "
                                   "(missing tooling or tests). Regenerate them with 'dev repair' to confirm the work.")
                            suffix = " (verification commands could not run)"
                        else:
                            blocks = True
                            fix = "Add or fix a verification command that proves this acceptance criterion."
                            suffix = ""
                        # Concrete, AC-scoped evidence instead of "all command summaries":
                        #  * if a compiled check targeted this AC, attach its command(s) and
                        #    the specific failing result;
                        #  * otherwise an explicit "no check compiled" marker so the agent
                        #    knows it must author one, not hunt through unrelated output.
                        ac_compiled = compiled_cmds_by_ac.get(ac_id, [])
                        ac_failures = failing_results_by_ac.get(ac_id, [])
                        ac_evidence: List[str] = []
                        suggested_cmd: Optional[str] = None
                        if ac_compiled:
                            suggested_cmd = ac_compiled[0]
                            ac_evidence.extend(f"compiled check: {c}" for c in ac_compiled)
                            ac_evidence.extend(r.summary[:500] for r in ac_failures)
                        else:
                            ac_evidence.append(
                                f"no DevCouncil check compiled for {ac_id} "
                                f"(expected verification method: {method})"
                            )
                        gaps.append(Gap(
                            id=self._next_gap_id(task.id, "AC"),
                            severity="high" if blocks else "medium",
                            gap_type="acceptance_criteria_unproven",
                            requirement_id=self._requirement_id_for_ac(requirements, ac_id),
                            task_id=task.id,
                            description=(
                                f"Acceptance criterion {ac_id} has no passing verification evidence "
                                f"for task {task.id}.{suffix}"
                            ),
                            evidence=ac_evidence,
                            recommended_fix=fix,
                            blocking=blocks,
                            acceptance_criterion_id=ac_id,
                            expected_verification_method=method,
                            suggested_command=suggested_cmd,
                        ))
            elif task.requirement_ids:
                gaps.append(Gap(
                    id=self._next_gap_id(task.id, "NOAC"),
                    severity="high",
                    gap_type="acceptance_criteria_unproven",
                    requirement_id=task.requirement_ids[0],
                    task_id=task.id,
                    description=f"Task {task.id} is linked to requirements but no acceptance criteria.",
                    recommended_fix="Link the task to specific acceptance_criterion_ids before verification.",
                    blocking=True,
                ))

            # 5b. Diff↔coverage gate. A green suite is only acceptance evidence if it
            # exercised the lines the diff changed. This catches the failure the README
            # promises to stop: tests "pass" while the new logic is never run (unrelated
            # suite, code never imported, untouched branch). Measured only when the target
            # repo has coverage tooling and the diff has measurable Python changes; absent
            # that, it degrades silently rather than blocking correct work.
            measure_cov, enforce_cov, min_ratio = self._diff_coverage_settings()
            any_passing = bool(successful_commands) or any(compiled_pass.values())
            coverage_measured = False
            coverage_skipped_reason: Optional[str] = None
            if not measure_cov:
                coverage_skipped_reason = "diff coverage disabled in config"
            elif not diff_content:
                coverage_skipped_reason = "no diff to measure"
            elif not task.acceptance_criterion_ids:
                coverage_skipped_reason = "task has no acceptance criteria"
            elif not any_passing:
                coverage_skipped_reason = "no passing verification command to instrument"
            if measure_cov and diff_content and task.acceptance_criterion_ids and any_passing:
                cov = self.measure_diff_coverage(task, diff_content)
                if not cov.measured:
                    coverage_skipped_reason = cov.reason or "diff coverage could not be measured"
                if cov.measured:
                    coverage_measured = True
                    coverage_skipped_reason = None
                    evidence_to_save.append(DiffCoverageEvidence(
                        task_id=task.id,
                        tool=cov.tool,
                        measured=True,
                        changed_lines=cov.changed_executable_lines,
                        covered_lines=cov.covered_changed_lines,
                        coverage_ratio=cov.ratio,
                        uncovered_by_file=cov.uncovered_by_file,
                        absent_files=cov.absent_files,
                        summary=cov.summary(),
                    ))
                    failing = cov.covered_changed_lines == 0 if min_ratio <= 0 else cov.ratio < min_ratio
                    if failing:
                        first_file = next(iter(cov.uncovered_by_file), None)
                        first_lines = cov.uncovered_by_file.get(first_file or "", [])
                        target_cmds = self._coverage_target_commands(task)
                        gaps.append(Gap(
                            id=self._next_gap_id(task.id, "DIFFCOV"),
                            severity="high" if enforce_cov else "medium",
                            gap_type="diff_not_exercised",
                            task_id=task.id,
                            description=(
                                f"Verification commands passed but exercised "
                                f"{cov.covered_changed_lines}/{cov.changed_executable_lines} changed line(s): "
                                f"{cov.summary()}. The acceptance criteria are not proven because the new "
                                "logic was never executed by the tests."
                            ),
                            evidence=[cov.summary()] + [
                                f"{path}: lines {lines}" for path, lines in list(cov.uncovered_by_file.items())[:5]
                            ],
                            recommended_fix=(
                                "Add or extend a test that executes the changed lines, then re-verify. "
                                "A passing suite that does not run the new code is not acceptance evidence."
                            ),
                            # Off by default (signal first); teams opt into blocking via
                            # verification.diff_coverage.enforce.
                            blocking=enforce_cov,
                            file=first_file,
                            line=first_lines[0] if first_lines else None,
                            suggested_command=target_cmds[0] if target_cmds else None,
                        ))

            # 6. Secret scan
            if diff_content:
                gaps.extend(self.secret_scanner.scan_diff(diff_content, task.id))

            # 7. LLM Implementation Review (ADVISORY ONLY).
            # DevCouncil's authority is executable evidence, not model confidence — so
            # an LLM reviewer must never block on its own say-so. Subjective reviewers
            # over-flag correct code (false negatives that erode trust in "blocked"),
            # so review findings are surfaced as non-blocking signals. A genuine
            # requirement gap is caught by the acceptance-criteria evidence checks
            # above; the review just adds human-facing context.
            if review_future is not None:
                try:
                    review_result = await review_future
                    for finding in review_result.findings:
                        finding.id = self._next_gap_id(task.id, "REVIEW")
                        finding.blocking = False
                        gaps.append(finding)
                except Exception as e:
                    logger.error("Implementation review failed: %s", e)

            # 8. Open live-review cards
            for card in unresolved_blocking_cards(self.project_root, task_id=task.id):
                gaps.append(Gap(
                    id=self._next_gap_id(task.id, "LIVE"),
                    severity="critical",
                    gap_type="architecture_drift",
                    task_id=task.id,
                    description=f"Open critical live-review card remains: {card.summary}",
                    evidence=[card.id, card.message_for_agent],
                    recommended_fix=(
                        f"Address the critique card, then run `dev watch resolve {card.id}` "
                        "or mark it ignored with justification outside the verification gate."
                    ),
                    blocking=True,
                ))

            self.last_outcome = VerificationOutcome(
                mode="compiled" if self.acceptance_compiler else "coarse",
                compiler_active=compiler_active,
                diff_empty=diff_empty,
                coverage_measured=coverage_measured,
                coverage_skipped_reason=coverage_skipped_reason,
            )
            return gaps, evidence_to_save
        finally:
            # Always drain the two background LLM tasks (even if the body raised
            # before their await points) so neither is destroyed-while-pending nor
            # logs 'exception never retrieved', and clear the per-call memos so a
            # later non-verify_task call on this instance recomputes fresh.
            for _fut in (compile_future, review_future):
                if _fut is not None:
                    if not _fut.done():
                        _fut.cancel()
                    try:
                        await _fut
                    except (asyncio.CancelledError, Exception):
                        pass
            self._untracked_cache = None
            self._command_timeout_cache = None
            # Reload project dependencies next run: a reused Verifier may verify a later
            # task after pyproject/requirements changed on disk.
            self._project_deps_cache = None

    def _task_intent_text(self, task: Task, requirements: Optional[List[Requirement]]) -> str:
        """Lowercased text describing what the task is meant to do — its title,
        description, and the descriptions of its acceptance criteria. Used to tell an
        INTENDED public-API change ("remove deprecated foo") from silent drift."""
        parts = [task.title or "", task.description or ""]
        if requirements:
            ac_ids = set(task.acceptance_criterion_ids)
            for req in requirements:
                for ac in req.acceptance_criteria:
                    if ac.id in ac_ids:
                        parts.append(ac.description or "")
        return " ".join(parts).lower()

    def _check_semantic_diff(self, task: Task, requirements: Optional[List[Requirement]] = None) -> List[Gap]:
        gaps: List[Gap] = []
        semantic_path = self.project_root / ".devcouncil" / "semantic" / task.id
        after_path = semantic_path / "after.json"
        if not after_path.exists():
            return gaps
        try:
            from devcouncil.indexing.semantic_index import SemanticIndex

            result = SemanticIndex(self.project_root).diff(task.id)
        except Exception as e:
            logger.warning("Semantic diff check failed for %s; skipping semantic gaps: %s", task.id, e)
            return gaps

        planned_paths = {pf.path for pf in task.planned_files}
        classifications = result.get("classifications", [])
        # Drift signal inputs: a public symbol re-added elsewhere is a move/rename (a
        # legitimate refactor, not drift); and the task's own intent text lets a removal
        # the task actually asked for ("remove deprecated foo") pass without false-blocking.
        readded_public = {
            item.get("name") for item in classifications
            if item.get("type") == "exported_symbol_added" and item.get("name")
        }
        intent_text = self._task_intent_text(task, requirements)
        for item in classifications:
            change_type = item.get("type", "")
            path = item.get("path", "")
            if change_type == "exported_symbol_removed":
                # An executor deleting/renaming an existing PUBLIC symbol — even inside a
                # file it is allowed to touch — is scope drift / a regression the focused
                # task rarely intends. Block it UNLESS the symbol was re-added elsewhere
                # (a move/rename) or the task text explicitly calls for the removal.
                name = item.get("name", "")
                moved = name in readded_public
                intended = bool(name) and name.lower() in intent_text
                gaps.append(Gap(
                    id=self._next_gap_id(task.id, "DRIFT"),
                    severity="high",
                    gap_type="architecture_drift",
                    task_id=task.id,
                    description=(
                        f"Public symbol '{name}' was removed from {path} — possible scope "
                        "drift: the executor changed a public API the task did not call for."
                    ),
                    evidence=[f"{path}:{name}"],
                    recommended_fix=(
                        "Restore the removed public symbol. If its removal IS part of this "
                        "task, state that in the task description / acceptance criteria so the "
                        "change is an intended, reviewed decision rather than silent drift."
                    ),
                    blocking=(not moved and not intended),
                    file=path,
                ))
            elif change_type == "public_api_change" and path not in planned_paths:
                gaps.append(Gap(
                    id=self._next_gap_id(task.id, "SEM"),
                    severity="high",
                    gap_type="architecture_drift",
                    task_id=task.id,
                    description=f"Unplanned public API change detected in {path}.",
                    evidence=[path],
                    recommended_fix="Add file to planned_files and document acceptance criteria.",
                    blocking=not bool(task.acceptance_criterion_ids),
                ))
            elif change_type == "public_api_change" and path in planned_paths:
                # The file is in scope, but the executor changed the SIGNATURE of an
                # existing public symbol. Tasks legitimately change signatures of files
                # they own, so this is ADVISORY only — surfaced so an audit/agent can see
                # the public contract moved, not silently drifted.
                gaps.append(Gap(
                    id=self._next_gap_id(task.id, "SIGDRIFT"),
                    severity="medium",
                    gap_type="architecture_drift",
                    task_id=task.id,
                    description=(
                        f"Public API signature change in planned file {path}"
                        + (f" ({item.get('name')})" if item.get("name") else "")
                        + ". Confirm callers are updated and the change is intended."
                    ),
                    evidence=[f"{path}:{item.get('name', '')}"],
                    recommended_fix=(
                        "If the signature change is part of this task, note it in the task "
                        "description / acceptance criteria; otherwise revert it."
                    ),
                    blocking=False,
                ))
            elif change_type == "import_dependency_change":
                # A NEW third-party top-level package added to the diff is supply-chain
                # drift — block it. Everything else (stdlib, relative/local, or an
                # already-declared/available dependency) stays advisory, and only on an
                # unplanned file (an unplanned file is already orphan-blocked anyway).
                statement = item.get("statement", "")
                top = self._import_top_level(statement)
                new_third_party = self._is_new_third_party_import(top)
                if new_third_party:
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, "DEPADD"),
                        severity="high",
                        gap_type="dependency_risk",
                        task_id=task.id,
                        description=(
                            f"New undeclared third-party dependency '{top}' imported in {path} "
                            f"({statement.strip()}). Adding a dependency the task did not plan is "
                            "supply-chain drift."
                        ),
                        evidence=[path, statement.strip()],
                        recommended_fix=(
                            f"Declare '{top}' in the project's dependencies and plan the change, "
                            "or use an existing/standard-library alternative."
                        ),
                        blocking=True,
                        file=path,
                    ))
                elif path not in planned_paths:
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, "IMP"),
                        severity="medium",
                        gap_type="dependency_risk",
                        task_id=task.id,
                        description=f"Import dependency change in {path}.",
                        evidence=[path],
                        recommended_fix="Confirm dependency change is intentional.",
                        blocking=False,
                    ))
            elif change_type == "config_schema_dependency_change" and path not in planned_paths:
                gaps.append(Gap(
                    id=self._next_gap_id(task.id, "CFG"),
                    severity="high",
                    gap_type="dependency_risk",
                    task_id=task.id,
                    description=f"Config/schema change detected in {path}.",
                    evidence=[path],
                    recommended_fix="Plan the config change or revert it.",
                    blocking=True,
                ))
        return gaps

    @staticmethod
    def _import_top_level(statement: str) -> Optional[str]:
        """Top-level package of an import statement, or None for relative/local/unparseable.

        ``import requests`` / ``import os.path`` -> the first dotted component; ``from x.y
        import z`` -> ``x``; ``from . import z`` / ``from .mod import z`` -> None (relative).
        """
        s = (statement or "").strip()
        if s.startswith("import "):
            first = s[len("import "):].split(",")[0].strip()
            top = first.split(" as ")[0].strip().split(".")[0].strip()
            return top or None
        if s.startswith("from "):
            rest = s[len("from "):].lstrip()
            if rest.startswith("."):  # relative import -> local, never a new dependency
                return None
            mod = rest.split(" import ")[0].strip()
            return (mod.split(".")[0].strip() or None) if mod else None
        return None

    def _is_new_third_party_import(self, top: Optional[str]) -> bool:
        """True only when ``top`` is a genuinely new, undeclared third-party package.

        Conservative on purpose (this gates a BLOCK): a module is NOT flagged when it is
        the standard library, a declared project dependency, or already importable in the
        environment (so import-name vs distribution-name mismatches like ``yaml``/``pyyaml``
        never false-block). Only a package that is none of those — i.e. undeclared AND not
        present — counts as supply-chain drift."""
        if not top:
            return False
        if top in self._stdlib_modules():
            return False
        if top.lower() in self._project_dependencies():
            return False
        try:
            import importlib.util
            if importlib.util.find_spec(top) is not None:
                return False  # already available in the environment; not a new dependency
        except Exception:
            # A find_spec error (e.g. a partially-installed parent) is ambiguous; do not
            # block on ambiguity.
            return False
        return True

    @staticmethod
    def _stdlib_modules() -> frozenset:
        names = getattr(sys, "stdlib_module_names", None)
        return frozenset(names) if names else frozenset()

    def _project_dependencies(self) -> set:
        """Lower-cased distribution names declared by the project (pyproject/requirements/
        package.json). Cached per Verifier instance; best-effort (parse errors are ignored)."""
        cached = getattr(self, "_project_deps_cache", None)
        if cached is not None:
            return cached
        deps: set = set()
        split_re = r"[><=!~;\[\] ]"
        pyproject = self.project_root / "pyproject.toml"
        if pyproject.exists():
            try:
                import tomllib
                data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
                project = data.get("project", {}) or {}
                for dep in project.get("dependencies", []) or []:
                    pkg = re.split(split_re, dep.strip())[0].strip().lower()
                    if pkg:
                        deps.add(pkg)
                for group in (project.get("optional-dependencies", {}) or {}).values():
                    for dep in group or []:
                        pkg = re.split(split_re, dep.strip())[0].strip().lower()
                        if pkg:
                            deps.add(pkg)
            except Exception:
                pass
        requirements = self.project_root / "requirements.txt"
        if requirements.exists():
            try:
                for line in requirements.read_text(encoding="utf-8").splitlines():
                    line = line.strip()
                    if line and not line.startswith("#"):
                        pkg = re.split(split_re, line)[0].strip().lower()
                        if pkg:
                            deps.add(pkg)
            except Exception:
                pass
        package_json = self.project_root / "package.json"
        if package_json.exists():
            try:
                data = json.loads(package_json.read_text(encoding="utf-8"))
                for key in ("dependencies", "devDependencies", "optionalDependencies"):
                    deps.update(k.lower() for k in (data.get(key) or {}).keys())
            except Exception:
                pass
        self._project_deps_cache = deps
        return deps

    # Signatures that mean the verification command itself could not run (or had
    # nothing to run), so its non-zero exit says nothing about whether the
    # implementation is correct — a tooling/plan defect, not a code defect.
    _MALFORMED_COMMAND_SIGNATURES = (
        "syntaxerror",
        "invalid syntax",
        "indentationerror",
        "no module named",                 # any tool not installed (pytest, flake8, mypy, ...)
        "can't open file",
        "no such file or directory",
        "file or directory not found",     # pytest: target path missing
        "no tests ran",                    # pytest -k matched nothing / empty file
        "no tests collected",
        "error: not found",                # pytest: test node id does not exist
        "is not recognized as an internal or external command",
        "command not found",
        "executable file not found",
        "failed to run command",
        "importerror",                     # the verification harness itself failed to import
        "modulenotfounderror",
    )
    # Compile-/launch-time signatures that mean the code NEVER executed — these are
    # always authoritative regardless of any ``File "<string>", line N`` marker (a
    # SyntaxError prints that marker even though nothing ran). They must not be subject
    # to the "signature must precede a traceback frame" rule that distinguishes a real
    # in-test traceback from a launcher error.
    _UNCONDITIONAL_UNRUNNABLE_SIGNATURES = (
        "syntaxerror",
        "invalid syntax",
        "indentationerror",
        "can't open file",
        "is not recognized as an internal or external command",
        "command not found",
        "executable file not found",
        "failed to run command",
        "no tests ran",
        "no tests collected",
        "error: not found",
    )
    # pytest exit codes that mean "could not run / collect", not "tests failed":
    #   4 = usage/collection error, 5 = no tests collected.
    _PYTEST_NONRUN_EXIT_CODES = {4, 5}

    @staticmethod
    def _is_traceback_frame(line: str) -> bool:
        """True for a Python traceback frame line: ``  File "...", line N``."""
        stripped = line.strip()
        return stripped.startswith('File "') and ", line " in stripped

    def _malformed_signature_precedes_traceback(self, text: str) -> bool:
        """Decide whether an unrunnable-launcher signature is authoritative.

        A launcher/collection failure prints its error WITHOUT a Python traceback that
        executed the code under test (e.g. ``ModuleNotFoundError: No module named
        pytest`` straight from the interpreter, or pytest's collection error banner).
        A genuine in-test failure, by contrast, raises from inside a traceback whose
        frames point at the test/source files; the same signature words can appear
        there (``ImportError`` re-raised inside a test) but that is a real defect, not
        an unrunnable command.

        So a signature only proves "unrunnable" when it appears BEFORE the first
        traceback frame (or there is no traceback frame at all). If a traceback frame
        appears at or before the signature, the code under test ran and failed — keep
        it a blocking test failure."""
        if not text:
            return False
        low_all = text.lower()
        # Compile-/launch-time failures: the code never executed, so a ``File ...``
        # marker (printed by SyntaxError) is not a real frame. Authoritative outright.
        if any(sig in low_all for sig in self._UNCONDITIONAL_UNRUNNABLE_SIGNATURES):
            return True
        lines = text.splitlines()
        lowered_lines = [ln.lower() for ln in lines]
        first_frame_idx: Optional[int] = None
        for idx, line in enumerate(lines):
            if self._is_traceback_frame(line):
                first_frame_idx = idx
                break
        for idx, low in enumerate(lowered_lines):
            if any(sig in low for sig in self._MALFORMED_COMMAND_SIGNATURES):
                # Signature found; it is only authoritative if no traceback frame
                # precedes it (i.e. the failure is from the launcher, not from code
                # that actually executed under a traceback).
                if first_frame_idx is None or idx < first_frame_idx:
                    return True
                return False
        return False

    def _launcher_text(self, result: CommandResult) -> str:
        """Captured output for launcher-vs-test analysis, ordered stderr then stdout.

        The traceback-precedence discriminator
        (:meth:`_malformed_signature_precedes_traceback`) needs to see BOTH streams:
        an interpreter "cannot run" error lands on stderr (with no traceback frame),
        while a genuine in-test failure's traceback lands on stdout (frame first, then
        the exception). We therefore concatenate stderr+stdout so the relative ordering
        of any signature vs the first traceback frame is preserved.

        Reading the merged ``result.summary`` alone is unsafe: it hoists the salient
        error line to the FRONT, which would place an in-test ``ImportError`` before its
        own traceback frame and misclassify a real failure as unrunnable. So prefer the
        raw logs; only fall back to the summary when no log path is available (e.g. unit
        tests that stub ``_run_command``). Never raises."""
        parts: List[str] = []
        for path in (result.stderr_path, result.stdout_path):
            if not path:
                continue
            try:
                content = Path(path).read_text(encoding="utf-8", errors="replace")
                if content.strip():
                    parts.append(content)
            except Exception:
                pass
        if parts:
            return "\n".join(parts)
        return result.summary or ""

    # Matches a Python traceback frame: ``  File "path/to/x.py", line 42, in foo``.
    _TRACEBACK_FRAME_RE = re.compile(r'File "(?P<file>[^"]+)", line (?P<line>\d+)')

    def _failure_location(self, result: CommandResult) -> Tuple[Optional[str], Optional[int]]:
        """Best-effort (file, line) of a failing command's deepest traceback frame.

        The LAST frame in a Python traceback is the actual raise site, so we scan all
        frames and keep the last one that points at a real-looking source file (not the
        ``<string>`` of a ``python -c`` snippet). Returns repo-relative posix paths when
        the frame is inside the project root. Reads the captured logs (stdout has the
        test traceback; stderr has interpreter errors). Never raises."""
        sources = []
        for path in (result.stdout_path, result.stderr_path):
            if path:
                try:
                    content = Path(path).read_text(encoding="utf-8", errors="replace")
                    if content.strip():
                        sources.append(content)
                except Exception:
                    pass
        sources.append(result.summary or "")
        best_file: Optional[str] = None
        best_line: Optional[int] = None
        for text in sources:
            for match in self._TRACEBACK_FRAME_RE.finditer(text):
                raw_file = match.group("file")
                if not raw_file or raw_file.startswith("<"):
                    continue  # e.g. "<string>" from python -c
                best_file = self._relativize(raw_file)
                try:
                    best_line = int(match.group("line"))
                except ValueError:
                    best_line = None
            if best_file is not None:
                return best_file, best_line
        return best_file, best_line

    def _relativize(self, raw_path: str) -> str:
        """Normalize a traceback file path to a repo-relative posix path when possible."""
        normalized = raw_path.replace("\\", "/")
        try:
            candidate = Path(raw_path)
            if candidate.is_absolute():
                rel = candidate.resolve().relative_to(self.project_root.resolve())
                return rel.as_posix()
        except Exception:
            pass
        return normalized

    def _command_is_malformed(self, result: CommandResult) -> bool:
        """True when a non-zero exit reflects a broken/unrunnable command rather
        than a genuine assertion or test failure of the code under verification.

        Authoritative signals (in priority order):
          1. pytest exit 4/5 -> collection/usage error -> unrunnable.
          2. The launcher error text: an unrunnable signature only counts when it
             appears BEFORE any Python traceback frame. This stops a genuinely failing
             test whose traceback contains ``ImportError``/``ModuleNotFoundError`` from
             being downgraded to a non-blocking "invalid command" (which would let
             verification falsely PASS)."""
        is_pytest = "pytest" in (result.command or "")
        if is_pytest and result.exit_code in self._PYTEST_NONRUN_EXIT_CODES:
            return True
        # Otherwise the exit code alone is ambiguous: pytest exit 1 is "tests ran and
        # FAILED" (a real defect), but a missing pytest module also exits 1 from the
        # interpreter (``No module named pytest``). The launcher error text is the
        # authoritative discriminator — a signature only means "unrunnable" when it
        # appears BEFORE any Python traceback frame. A genuine test failure whose
        # traceback merely mentions ``ImportError`` keeps a traceback frame first and so
        # stays a blocking test failure (preventing a false PASS).
        text = self._launcher_text(result)
        return self._malformed_signature_precedes_traceback(text)

    def _commands_for_task(self, task: Task) -> Dict[str, List[str]]:
        if task.expected_tests:
            return {"test": task.expected_tests}
        if task.allowed_commands:
            return {"allowed": task.allowed_commands}
        return self._load_commands()

    def _command_applicable(self, command: str) -> tuple[bool, str]:
        """Stack-aware gate for a verification command.

        A planner- or config-supplied command must not BLOCK a task when it targets a
        language stack the repository does not have (e.g. ``npm test``/``eslint``/
        ``tsc`` on a Python-only repo). Those fail for stack reasons, not real defects —
        the false-block the benchmark surfaced. Returns ``(applicable, reason)``; an
        inapplicable command is skipped and recorded as advisory rather than run."""
        cmd = (command or "").strip()
        if not cmd:
            return True, ""
        try:
            from devcouncil.repo.ci_scaffold import _command_stack, detect_stacks

            stacks = detect_stacks(self.project_root)
            stack = _command_stack(cmd)
        except Exception:
            return True, ""
        if stack is not None and stacks and stack not in stacks:
            detected = ", ".join(sorted(stacks)) or "none"
            return False, f"command targets the '{stack}' stack not present in this repo (detected: {detected})"
        return True, ""

    # Linters / formatters / type checkers: a non-zero exit is a style/type OPINION,
    # not proof of a behavioral defect. Blocking a behaviorally-correct task on these is
    # the false-block the benchmark surfaced (the planner even spawns dedicated
    # "add flake8 check" / "run black --check" tasks). Their failures are advisory.
    _QUALITY_TOOLS = {
        "black", "flake8", "ruff", "isort", "pylint", "mypy", "pyright", "autopep8",
        "yapf", "pyflakes", "pycodestyle", "bandit", "eslint", "tsc", "prettier",
        "stylelint", "standard", "biome",
    }

    def _is_quality_only_command(self, command: str) -> bool:
        """True when the command's executable is purely a linter/formatter/type checker.

        Handles common wrappers (``python -m mypy``, ``npx eslint``, ``poetry run black``,
        ``npm run lint``). A behavioral check like ``pytest`` or ``python -c 'assert ...'``
        is NOT a quality-only command and still gates."""
        tokens = command.split()
        i = 0
        while i < len(tokens):
            tok = tokens[i]
            if tok in {"python", "python3", "py"} and i + 1 < len(tokens) and tokens[i + 1] == "-m":
                i += 2
                continue
            if tok in {"npx", "poetry", "uv", "pdm", "hatch", "rye"}:
                i += 1
                if i < len(tokens) and tokens[i] == "run":
                    i += 1
                continue
            if tok in {"npm", "pnpm", "yarn"}:
                return any(word in tokens for word in ("lint", "format", "eslint", "prettier", "stylelint", "biome"))
            break
        if i >= len(tokens):
            return False
        tool = tokens[i].replace("\\", "/").split("/")[-1].split("==")[0].lower()
        return tool in self._QUALITY_TOOLS

    def _command_can_prove_acceptance(self, cmd_type: str, command: str) -> bool:
        if cmd_type == "test":
            return True
        lowered = command.lower()
        evidence_keywords = (
            "test",
            "pytest",
            "vitest",
            "jest",
            "unittest",
            "cargo test",
            "go test",
            "mvn test",
            "gradle test",
            "ruff check",
            "mypy",
            "tsc",
            "typecheck",
            "type-check",
        )
        return any(keyword in lowered for keyword in evidence_keywords)

    def _requirement_id_for_ac(self, requirements: List[Requirement], ac_id: str) -> Optional[str]:
        for req in requirements:
            if any(ac.id == ac_id for ac in req.acceptance_criteria):
                return req.id
        return None
