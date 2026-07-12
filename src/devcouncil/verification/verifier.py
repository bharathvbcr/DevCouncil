import logging
import uuid
from dataclasses import dataclass, asdict, field
from pathlib import Path
from typing import List, Dict, Any, Optional, Tuple

from devcouncil.app.config import load_config

from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.gap import Gap
from devcouncil.domain.evidence import CommandResult
from devcouncil.verification import diff_coverage as dc
from devcouncil.verification.checks.orphan_diff import classify_change_paths
from devcouncil.verification.checks.semantic_diff import (
    detect_semantic_diff_gaps,
    import_top_level,
    is_new_third_party_import,
    load_project_dependencies,
    task_intent_text,
)
from devcouncil.verification.verify_setup import (
    cleanup_verify_futures,
    prime_verify_memos,
    resolve_verify_context,
    start_verify_futures,
)
from devcouncil.verification.verify_orchestration import run_verify_orchestration
from devcouncil.verification.command_evidence import (
    command_has_acceptance_evidence,
    command_is_trivial_evidence,
)
from devcouncil.gating.checks.secret_scan_check import SecretScanner
from devcouncil.verification.implementation_reviewer import ImplementationReviewer
from devcouncil.verification.acceptance_compiler import AcceptanceTestCompiler
from devcouncil.llm.router import ModelRouter
from devcouncil.utils.json_persist import read_json
from devcouncil.utils.subprocess_env import clean_subprocess_env
from devcouncil.verification import command_malformation as cmd_malf
from devcouncil.verification import command_runner as cmd_runner
from devcouncil.verification.coverage_measurement import DiffCoverageMeasurer
from devcouncil.verification.git_diff_fallback import GitDiffFallback

logger = logging.getLogger(__name__)


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
    # Rigor metadata: the task's estimated (or manually set) difficulty and which
    # anti-laziness escalations actually took effect on this run, so "passed" can
    # be distinguished from "passed under strict gates".
    difficulty: Optional[str] = None
    rigor_applied: List[str] = field(default_factory=list)
    wiki_refresh: Optional[Dict[str, Any]] = None
    # Liveness ratchet baseline status for this verify run: "ok" | "missing".
    liveness_baseline: Optional[str] = None

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
        self._git_fallback = GitDiffFallback(project_root)
        self._command_timeout_cache: Optional[int] = None
        # Project dependency names (lower-cased), loaded once per verify_task and cleared
        # in its finally so a reused Verifier re-reads them for a later task.
        self._project_deps_cache: Optional[set] = None
        self._coverage_measurer: Optional[DiffCoverageMeasurer] = None

    def _get_coverage_measurer(self) -> DiffCoverageMeasurer:
        if self._coverage_measurer is None:
            self._coverage_measurer = DiffCoverageMeasurer.from_verifier(self)
        return self._coverage_measurer

    def _next_gap_id(self, task_id: str, suffix: str) -> str:
        """Generate unique gap IDs to prevent SQLite overwrites."""
        self._gap_counter += 1
        return f"GAP-{task_id}-{suffix}-{uuid.uuid4().hex[:6]}-{self._gap_counter:03d}"

    def get_diff(self) -> str:
        return self._git_fallback.get_diff()

    def get_changed_files(self) -> List[str]:
        return self._git_fallback.get_changed_files()

    def get_task_changed_files(self, task_id: str) -> List[str]:
        changed = set(self.get_changed_files())
        changed.difference_update(self._load_baseline_files())
        changed.difference_update(self._load_task_snapshot_files(task_id))
        return sorted(changed)

    def _committed_task_diff(self, task_id: str) -> str:
        return self._git_fallback.committed_task_diff(task_id)

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

    def _get_untracked_files(self) -> List[str]:
        return self._git_fallback.get_untracked_files()

    def _filter_change_paths(self, paths: List[str]) -> List[str]:
        return self._git_fallback.filter_change_paths(paths)

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
            data = read_json(path)
            return {
                item.replace("\\", "/")
                for item in data.get("changed_files", [])
                if isinstance(item, str)
            }
        except Exception as e:
            logger.warning("Failed to load verification snapshot %s: %s", path, e)
            return set()

    def _load_repo_map(self) -> Optional[dict]:
        """Parse ``.devcouncil/repo_map.json`` (None if absent/unreadable).

        Used by structural gates (subsystem-boundary drift) and the wiki post-step.
        Best-effort: any failure degrades to ``None`` so a missing/corrupt map never
        breaks verification."""
        map_path = self.project_root / ".devcouncil" / "repo_map.json"
        if not map_path.exists():
            return None
        try:
            data = read_json(map_path)
            return data if isinstance(data, dict) else None
        except Exception as e:
            logger.debug("Failed to load repo map for verification: %s", e)
            return None

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
        return cmd_runner.save_command_log(self.project_root, label, command, stream, content)

    def _verification_env(self) -> Dict[str, str]:
        return clean_subprocess_env()

    @staticmethod
    def _summarize_stream(content: str, budget: int = 360) -> str:
        return cmd_runner.summarize_stream(content, budget)

    def _run_command(self, command: str, task_id: str = "verify") -> CommandResult:
        if self._command_timeout_cache is not None:
            timeout = self._command_timeout_cache
        else:
            try:
                config = load_config(self.project_root)
                timeout = config.execution.command_timeout
            except Exception:
                timeout = 300
        return cmd_runner.run_verification_command(
            self.project_root,
            command,
            task_id=task_id,
            timeout=timeout,
        )

    def _split_command(self, command: str) -> List[str]:
        return cmd_runner.split_command(command)

    def _classify_change_paths(self, changed_files: List[str]) -> Tuple[List[str], List[str]]:
        return classify_change_paths(self.project_root, changed_files, self._get_untracked_files)

    def _diff_coverage_settings(self) -> Tuple[bool, bool, float]:
        """Return (measure, enforce, min_ratio) with safe defaults when unconfigured."""
        if self._diff_coverage_override is not None:
            return self._diff_coverage_override
        try:
            cfg = load_config(self.project_root).verification.diff_coverage
            return bool(cfg.measure), bool(cfg.enforce), float(cfg.min_ratio)
        except Exception:
            return True, False, 0.0

    def _coverage_target_commands(self, task: Task) -> List[str]:
        return self._get_coverage_measurer().coverage_target_commands(task)

    def measure_diff_coverage(self, task: Task, diff_content: str) -> dc.DiffCoverageResult:
        """Run the task's test command(s) under coverage and intersect with the diff."""
        return self._get_coverage_measurer().measure(task, diff_content)

    async def verify_task(self, task: Task, requirements: List[Requirement]) -> Tuple[List[Gap], List[Any]]:
        logger.info("verify_task: task=%s requirements=%d", task.id, len(requirements))
        self._gap_counter = 0
        prime_verify_memos(self)
        ctx = resolve_verify_context(self, task, requirements)
        compile_future, review_future = start_verify_futures(
            self,
            task=task,
            requirements=requirements,
            diff_content=ctx.diff_content,
            ac_samples=ctx.ac_samples,
            ac_per_criterion=ctx.ac_per_criterion,
        )
        try:
            return await run_verify_orchestration(
                self, task, requirements, ctx, compile_future, review_future,
            )
        finally:
            await cleanup_verify_futures(self, compile_future, review_future)

    def _task_intent_text(self, task: Task, requirements: Optional[List[Requirement]]) -> str:
        return task_intent_text(task, requirements)

    def _check_semantic_diff(self, task: Task, requirements: Optional[List[Requirement]] = None) -> List[Gap]:
        cached = getattr(self, "_project_deps_cache", None)
        if cached is None:
            cached = load_project_dependencies(self.project_root)
            self._project_deps_cache = cached
        return detect_semantic_diff_gaps(
            project_root=self.project_root,
            task=task,
            requirements=requirements,
            next_gap_id=self._next_gap_id,
            project_deps=cached,
        )

    @staticmethod
    def _import_top_level(statement: str) -> Optional[str]:
        return import_top_level(statement)

    def _is_new_third_party_import(self, top: Optional[str]) -> bool:
        return is_new_third_party_import(top, project_deps=self._project_dependencies())

    def _project_dependencies(self) -> set[str]:
        """Lower-cased distribution names declared by the project."""
        cached: set[str] | None = getattr(self, "_project_deps_cache", None)
        if cached is not None:
            return cached
        deps = load_project_dependencies(self.project_root)
        self._project_deps_cache = deps
        return deps

    def _malformed_signature_precedes_traceback(self, text: str) -> bool:
        return cmd_malf.malformed_signature_precedes_traceback(text)

    def _launcher_text(self, result: CommandResult) -> str:
        return cmd_malf.launcher_text(result)

    def _failure_location(self, result: CommandResult) -> Tuple[Optional[str], Optional[int]]:
        return cmd_malf.failure_location(self.project_root, result)

    def _command_is_malformed(self, result: CommandResult) -> bool:
        return cmd_malf.command_is_malformed(result)

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

    @staticmethod
    def _is_test_path(path: str) -> bool:
        from devcouncil.verification.checks.orphan_diff import is_test_path
        return is_test_path(path)

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
        """Whether a run of ``command`` may count as acceptance evidence.

        Declared TEST commands (planner expected_tests / config commands.test) are
        trusted unless trivially incapable of proving behavior (``python --version``,
        ``echo ok``, ``git status``) — the deny-list keeps legitimate keyword-less
        behavioral commands (``make check``, ``./run_smoke.sh``) evidential. Agent-
        appended expected_tests are additionally excluded at coarse-proof time.
        Other command types (allowed_commands) keep the strict keyword allowlist:
        an incidental ``make build`` succeeding must not prove a behavioral AC."""
        if cmd_type == "test":
            return not command_is_trivial_evidence(command)
        return command_has_acceptance_evidence(command)

    def _requirement_id_for_ac(self, requirements: List[Requirement], ac_id: str) -> Optional[str]:
        for req in requirements:
            if any(ac.id == ac_id for ac in req.acceptance_criteria):
                return req.id
        return None
