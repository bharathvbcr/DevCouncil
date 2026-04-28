import hashlib
import subprocess
import logging
import uuid
import fnmatch
import json
import shlex
from pathlib import Path
from typing import List, Dict, Any, Optional, Tuple

from devcouncil.app.config import load_config

from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.gap import Gap
from devcouncil.domain.evidence import TestEvidence, DiffEvidence, CommandResult
from devcouncil.gating.checks.secret_scan_check import SecretScanner
from devcouncil.verification.implementation_reviewer import ImplementationReviewer
from devcouncil.llm.router import ModelRouter

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
)

class Verifier:
    def __init__(self, project_root: Path, router: Optional[ModelRouter] = None):
        self.project_root = project_root
        self._gap_counter = 0
        self.secret_scanner = SecretScanner()
        self.reviewer = ImplementationReviewer(router) if router else None

    def _next_gap_id(self, task_id: str, suffix: str) -> str:
        """Generate unique gap IDs to prevent SQLite overwrites."""
        self._gap_counter += 1
        return f"GAP-{task_id}-{suffix}-{uuid.uuid4().hex[:6]}-{self._gap_counter:03d}"

    def get_diff(self) -> str:
        try:
            if not self._has_head():
                return self._get_initial_repo_diff()
            return subprocess.check_output(
                ["git", "diff", "HEAD"], cwd=self.project_root
            ).decode("utf-8", errors="replace")
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
            return self._filter_change_paths(output)
        except Exception as e:
            logger.warning("Failed to get changed files: %s", e)
            return []

    def get_task_changed_files(self, task_id: str) -> List[str]:
        changed = set(self.get_changed_files())
        changed.difference_update(self._load_baseline_files())
        changed.difference_update(self._load_task_snapshot_files(task_id))
        return sorted(changed)

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
        log_path.write_text(content, encoding="utf-8")
        return str(log_path)

    def _run_command(self, command: str, task_id: str = "verify") -> CommandResult:
        try:
            config = load_config(self.project_root)
            timeout = config.execution.command_timeout
        except Exception:
            timeout = 300

        try:
            result = subprocess.run(
                self._split_command(command),
                shell=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                timeout=timeout,
            )
            stdout = result.stdout or ""
            stderr = result.stderr or ""
            stdout_path = self._save_log(task_id, command, "stdout", stdout)
            stderr_path = self._save_log(task_id, command, "stderr", stderr)
            return CommandResult(
                command=command,
                exit_code=result.returncode,
                stdout_path=stdout_path,
                stderr_path=stderr_path,
                summary=(
                    f"Exit code {result.returncode}. "
                    f"stdout: {stdout[-500:] if stdout else '(empty)'}. "
                    f"stderr: {stderr[-500:] if stderr else '(empty)'}"
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
        return shlex.split(command, posix=False)

    def _check_dependency_changes(self, changed_files: List[str]) -> List[str]:
        dep_files = {
            "package.json", "package-lock.json", "yarn.lock", "pnpm-lock.yaml",
            "requirements.txt", "pyproject.toml", "uv.lock", "Pipfile.lock",
            "go.mod", "go.sum", "Cargo.toml", "Cargo.lock",
        }
        return [f for f in changed_files if Path(f).name in dep_files]

    async def verify_task(self, task: Task, requirements: List[Requirement]) -> Tuple[List[Gap], List[Any]]:
        self._gap_counter = 0
        gaps: List[Gap] = []
        evidence_to_save: List[Any] = []
        changed_files = self.get_task_changed_files(task.id)
        diff_content = self.get_diff()

        if diff_content:
            diff_ev = DiffEvidence(
                task_id=task.id,
                changed_files=changed_files,
                added_files=[], 
                deleted_files=[], 
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
                ))

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
                ))

        # 4. Run allowed commands
        command_results: List[CommandResult] = []
        for cmd_type, cmds in self._commands_for_task(task).items():
            for cmd in cmds:
                result = self._run_command(cmd, task_id=task.id)
                command_results.append(result)
                evidence_to_save.append(result)
                if result.exit_code != 0:
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, cmd_type.upper()),
                        severity="high",
                        gap_type="test_failed",
                        task_id=task.id,
                        description=f"Command '{cmd}' failed with exit code {result.exit_code}.",
                        evidence=[result.summary[:500]],
                        recommended_fix=f"Fix the issues reported by '{cmd}'.",
                        blocking=True,
                    ))

        # 5. Acceptance-criteria evidence mapping
        successful_commands = [result for result in command_results if result.exit_code == 0]
        if task.acceptance_criterion_ids:
            if successful_commands:
                req_by_ac = {
                    ac.id: req.id
                    for req in requirements
                    for ac in req.acceptance_criteria
                }
                evidence_command = ", ".join(result.command for result in successful_commands)
                for ac_id in task.acceptance_criterion_ids:
                    evidence_to_save.append(TestEvidence(
                        requirement_id=req_by_ac.get(ac_id, task.requirement_ids[0] if task.requirement_ids else ""),
                        acceptance_criterion_id=ac_id,
                        command=evidence_command,
                        status="passed",
                        evidence_summary=(
                            "Acceptance criterion linked to successful verification command(s): "
                            f"{evidence_command}"
                        ),
                    ))
            else:
                for ac_id in task.acceptance_criterion_ids:
                    gaps.append(Gap(
                        id=self._next_gap_id(task.id, "AC"),
                        severity="high",
                        gap_type="acceptance_criteria_unproven",
                        requirement_id=self._requirement_id_for_ac(requirements, ac_id),
                        task_id=task.id,
                        description=(
                            f"Acceptance criterion {ac_id} has no passing verification evidence "
                            f"for task {task.id}."
                        ),
                        evidence=[result.summary[:500] for result in command_results] if command_results else [],
                        recommended_fix=(
                            "Run or add an allowed verification command that proves this acceptance criterion."
                        ),
                        blocking=True,
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

        # 6. Secret scan
        if diff_content:
            gaps.extend(self.secret_scanner.scan_diff(diff_content, task.id))

        # 7. LLM Implementation Review
        if self.reviewer and diff_content:
            try:
                review_result = await self.reviewer.review_changes(task, requirements, diff_content)
                for finding in review_result.findings:
                    finding.id = self._next_gap_id(task.id, "REVIEW")
                    gaps.append(finding)
            except Exception as e:
                logger.error("Implementation review failed: %s", e)

        return gaps, evidence_to_save

    def _commands_for_task(self, task: Task) -> Dict[str, List[str]]:
        if task.expected_tests:
            return {"test": task.expected_tests}
        if task.allowed_commands:
            return {"allowed": task.allowed_commands}
        return self._load_commands()

    def _requirement_id_for_ac(self, requirements: List[Requirement], ac_id: str) -> Optional[str]:
        for req in requirements:
            if any(ac.id == ac_id for ac in req.acceptance_criteria):
                return req.id
        return None
