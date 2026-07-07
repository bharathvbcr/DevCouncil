"""Diff-coverage instrumentation extracted from Verifier."""

from __future__ import annotations

import json
import logging
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable, Dict, List, Optional

from devcouncil.app.config import load_config
from devcouncil.domain.task import Task
from devcouncil.utils.subprocess_env import clean_subprocess_env
from devcouncil.verification import command_runner as cmd_runner
from devcouncil.verification import diff_coverage as dc

logger = logging.getLogger(__name__)

SplitCommand = Callable[[str], List[str]]
VerificationEnv = Callable[[], Dict[str, str]]
LoadCommands = Callable[[], Dict[str, List[str]]]
CommandCanProve = Callable[[str, str], bool]


class DiffCoverageMeasurer:
    """Run task test commands under coverage and intersect with a diff."""

    def __init__(
        self,
        project_root: Path,
        *,
        get_coverage_python: Callable[[], Optional[str]],
        split_command: SplitCommand,
        verification_env: VerificationEnv,
        load_commands: LoadCommands,
        command_can_prove_acceptance: CommandCanProve,
    ) -> None:
        self.project_root = project_root
        self._get_coverage_python = get_coverage_python
        self._split_command = split_command
        self._verification_env = verification_env
        self._load_commands = load_commands
        self._command_can_prove_acceptance = command_can_prove_acceptance

    @classmethod
    def from_verifier(cls, verifier: object) -> DiffCoverageMeasurer:
        """Build a measurer bound to a Verifier instance."""
        return cls(
            verifier.project_root,  # type: ignore[attr-defined]
            get_coverage_python=lambda: verifier._coverage_python,  # type: ignore[attr-defined]
            split_command=cmd_runner.split_command,
            verification_env=clean_subprocess_env,
            load_commands=verifier._load_commands,  # type: ignore[attr-defined]
            command_can_prove_acceptance=verifier._command_can_prove_acceptance,  # type: ignore[attr-defined]
        )

    def coverage_target_commands(self, task: Task) -> List[str]:
        """The test command(s) to instrument — the ones that purport to prove the ACs."""
        if task.expected_tests:
            return list(task.expected_tests)
        test_like = [
            c for c in task.allowed_commands
            if self._command_can_prove_acceptance("allowed", c)
        ]
        if test_like:
            return test_like
        return list(self._load_commands().get("test", []))

    def measure(self, task: Task, diff_content: str) -> dc.DiffCoverageResult:
        """Run instrumentation for Python, JS/TS, and Go changed paths."""
        parsed = dc.parse_changed_lines(diff_content)
        py_changed = dc.measurable_python_changes(parsed)
        js_changed = dc.measurable_js_changes(parsed)
        go_changed = dc.measurable_go_changes(parsed)
        if not py_changed and not js_changed and not go_changed:
            return dc.DiffCoverageResult(measured=False, reason="no measurable source changes in diff")

        commands = self.coverage_target_commands(task)
        if not commands:
            return dc.DiffCoverageResult(measured=False, reason="no test command to instrument")

        try:
            timeout = load_config(self.project_root).execution.command_timeout
        except Exception:
            timeout = 300

        env = self._verification_env()
        tmp_dir = self.project_root / ".devcouncil" / "tmp"
        tmp_dir.mkdir(parents=True, exist_ok=True)
        results: List[dc.DiffCoverageResult] = []

        if py_changed:
            results.append(self._measure_python(task, commands, py_changed, tmp_dir, env, timeout))
        if js_changed:
            results.append(self._measure_js(task, commands, js_changed, tmp_dir, env, timeout))
        if go_changed:
            results.append(self._measure_go(task, commands, go_changed, tmp_dir, env, timeout))

        return dc.merge_diff_coverage_results(results)

    def _resolve_coverage_python(self, env: Dict[str, str]) -> str:
        override = self._get_coverage_python()
        if override:
            return override
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

    def _measure_python(
        self,
        task: Task,
        commands: List[str],
        changed: Dict[str, Dict[int, str]],
        tmp_dir: Path,
        env: dict,
        timeout: int,
    ) -> dc.DiffCoverageResult:
        python = self._resolve_coverage_python(env)
        if not self._coverage_available(python, env):
            return dc.DiffCoverageResult(measured=False, reason="coverage.py not available")
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
                try:
                    argv = self._split_command(cmd)
                except ValueError as exc:
                    logger.warning("Diff-coverage skipping unparseable command %r: %s", cmd, exc)
                    continue
                inline = dc.inline_python_code(argv)
                if inline is not None:
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
                        cov_argv, cwd=self.project_root, capture_output=True,
                        text=True, encoding="utf-8", errors="replace", timeout=timeout, env=env,
                    )
                except Exception as exc:
                    logger.warning("Diff-coverage run failed for %s: %s", task.id, exc)
                    continue
                ran_any = True
                append = True
            if not ran_any or not data_file.exists():
                return dc.DiffCoverageResult(measured=False, reason="no instrumentable Python test command")
            subprocess.run(
                [python, "-m", "coverage", "json", f"--data-file={data_file}", "-o", str(json_file)],
                cwd=self.project_root, capture_output=True, text=True,
                encoding="utf-8", errors="replace", timeout=120, env=env,
            )
            data = json.loads(json_file.read_text(encoding="utf-8"))
            coverage = dc.parse_coverage_json(data, self.project_root)
            return dc.intersect(changed, coverage, tool="coverage.py")
        except Exception as exc:
            return dc.DiffCoverageResult(measured=False, reason=f"coverage.py failed: {exc}")
        finally:
            for path in [data_file, json_file, *inline_scripts]:
                try:
                    path.unlink()
                except OSError:
                    pass

    def _measure_js(
        self,
        task: Task,
        commands: List[str],
        changed: Dict[str, Dict[int, str]],
        tmp_dir: Path,
        env: dict,
        timeout: int,
    ) -> dc.DiffCoverageResult:
        reports_dir = tmp_dir / f"c8-{task.id}"
        reports_dir.mkdir(parents=True, exist_ok=True)
        try:
            for stale in reports_dir.glob("coverage-final.json"):
                stale.unlink()
        except OSError:
            pass
        ran_any = False
        for cmd in commands:
            try:
                argv = self._split_command(cmd)
            except ValueError as exc:
                logger.warning("Diff-coverage skipping unparseable command %r: %s", cmd, exc)
                continue
            cov_argv = dc.c8_run_argv(argv, reports_dir=str(reports_dir))
            if cov_argv is None:
                continue
            try:
                subprocess.run(
                    cov_argv, cwd=self.project_root, capture_output=True,
                    text=True, encoding="utf-8", errors="replace", timeout=timeout, env=env,
                )
            except Exception as exc:
                logger.warning("c8 diff-coverage run failed for %s: %s", task.id, exc)
                continue
            ran_any = True
        candidates = list(reports_dir.glob("**/coverage-final.json"))
        if not ran_any or not candidates:
            return dc.DiffCoverageResult(measured=False, reason="c8 not available or no JS test command")
        try:
            data = json.loads(candidates[0].read_text(encoding="utf-8"))
            coverage = dc.parse_istanbul_json(data, self.project_root)
            return dc.intersect(changed, coverage, tool="c8")
        except Exception as exc:
            return dc.DiffCoverageResult(measured=False, reason=f"c8 report unreadable: {exc}")

    def _measure_go(
        self,
        task: Task,
        commands: List[str],
        changed: Dict[str, Dict[int, str]],
        tmp_dir: Path,
        env: dict,
        timeout: int,
    ) -> dc.DiffCoverageResult:
        profile = tmp_dir / f"diffcov-{task.id}.out"
        try:
            profile.unlink()
        except FileNotFoundError:
            pass
        ran_any = False
        for cmd in commands:
            try:
                argv = self._split_command(cmd)
            except ValueError as exc:
                logger.warning("Diff-coverage skipping unparseable command %r: %s", cmd, exc)
                continue
            cov_argv = dc.go_cover_run_argv(argv, str(profile))
            if cov_argv is None:
                continue
            try:
                subprocess.run(
                    cov_argv, cwd=self.project_root, capture_output=True,
                    text=True, encoding="utf-8", errors="replace", timeout=timeout, env=env,
                )
            except Exception as exc:
                logger.warning("go cover run failed for %s: %s", task.id, exc)
                continue
            ran_any = True
        if not ran_any or not profile.exists():
            return dc.DiffCoverageResult(measured=False, reason="go test -coverprofile not available")
        try:
            coverage = dc.parse_go_coverprofile(profile.read_text(encoding="utf-8"), self.project_root)
            return dc.intersect(changed, coverage, tool="go-cover")
        except Exception as exc:
            return dc.DiffCoverageResult(measured=False, reason=f"go cover profile unreadable: {exc}")
        finally:
            try:
                profile.unlink()
            except OSError:
                pass
