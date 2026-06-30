import hashlib
import os
import subprocess
import logging
import re
import shlex
from pathlib import Path
from typing import Literal, Optional, TYPE_CHECKING
from devcouncil.domain.task import Task

if TYPE_CHECKING:
    from devcouncil.app.config import DevCouncilConfig
from devcouncil.execution.permissions import PermissionManager
from devcouncil.domain.evidence import CommandResult
from devcouncil.app.errors import ExecutionError

from devcouncil.execution.patch import PatchEngine
from devcouncil.execution.paths import resolve_project_path
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.utils.redaction import redact_string

logger = logging.getLogger(__name__)

class TaskRunner:
    """Safely executes task actions while enforcing permission boundaries."""
    
    def __init__(self, project_root: Path, permission_manager: PermissionManager):
        self.project_root = project_root
        self.permissions = permission_manager
        self.patch_engine = PatchEngine(project_root)
        # Load config and build the trace logger once: run_command previously
        # re-parsed config.yaml on every command, and each operation rebuilt a
        # TraceLogger. Both are reusable for the lifetime of the runner.
        self.config: "Optional[DevCouncilConfig]"
        try:
            from devcouncil.app.config import load_config
            self.config = load_config(project_root)
        except Exception:
            self.config = None
        self.tracer = TraceLogger(project_root)

    def _validate_path_within_root(self, path: str) -> None:
        """Ensure a path resolves to a location within the project root."""
        resolve_project_path(self.project_root, path)

    def apply_patch(self, patch: str, task: Task) -> bool:
        """Apply a patch if permissions allow (all affected files must be in planned_files)."""
        changes = self._extract_patch_changes(patch)
        for path, operation in changes.items():
            self._validate_path_within_root(path)
            self.permissions.validate_action("file_write", path, task, operation=operation)
        applied = self.patch_engine.apply_patch(patch)
        self.tracer.log_event(
            "tool_patch_applied",
            {"paths": sorted(changes), "success": applied},
            task_id=task.id,
            summary=f"Patch {'applied' if applied else 'failed'} for {task.id}",
        )
        return applied

    def _extract_patch_paths(self, patch: str) -> set[str]:
        return set(self._extract_patch_changes(patch))

    def _extract_patch_changes(self, patch: str) -> dict[str, Literal["create", "modify", "delete"]]:
        """Extract repository-relative file paths touched by a unified git patch."""
        changes: dict[str, Literal["create", "modify", "delete"]] = {}
        old_path: Optional[str] = None
        old_is_null = False
        for line in patch.splitlines():
            if line.startswith("diff --git "):
                parts = line.split()
                cleaned = self._normalize_patch_path(parts[3]) if len(parts) > 3 else None
                if cleaned:
                    changes.setdefault(cleaned, "modify")
                old_path = self._normalize_patch_path(parts[2]) if len(parts) > 2 else None
                old_is_null = False
                continue

            if line.startswith("--- "):
                raw = line[4:].split("\t", 1)[0].strip()
                old_is_null = raw == "/dev/null"
                old_path = self._normalize_patch_path(raw)
                continue

            if line.startswith("+++ "):
                raw = line[4:].split("\t", 1)[0].strip()
                if raw == "/dev/null":
                    if old_path:
                        changes[old_path] = "delete"
                    continue
                new_path = self._normalize_patch_path(raw)
                if new_path:
                    changes[new_path] = "create" if old_is_null else "modify"

        if not changes:
            raise ExecutionError("Patch does not declare any affected files.")
        return changes

    def _normalize_patch_path(self, raw_path: str) -> Optional[str]:
        raw_path = raw_path.strip().strip('"')
        if raw_path == "/dev/null":
            return None
        raw_path = re.sub(r"^[ab]/", "", raw_path)
        return raw_path.replace("\\", "/")

    def _save_command_log(self, task_id: str, command: str, stream: str, content: str) -> str:
        """Save command output to a log file and return the path."""
        log_dir = self.project_root / ".devcouncil" / "logs"
        log_dir.mkdir(parents=True, exist_ok=True)
        cmd_hash = hashlib.sha256(command.encode()).hexdigest()[:8]
        filename = f"{task_id}-{cmd_hash}-{stream}.log"
        log_path = log_dir / filename
        log_path.write_text(redact_string(content), encoding="utf-8")
        return str(log_path)

    def run_command(self, command: str, task: Task) -> CommandResult:
        """Execute a shell command if allowed by permissions."""
        self.permissions.validate_action("shell", command, task)
        
        logger.info(f"Executing authorized command: {command}")
        
        try:
            timeout = self.config.execution.command_timeout if self.config is not None else 300
        except Exception:
            timeout = 300

        try:
            result = subprocess.run(
                # POSIX-correct tokenization on POSIX hosts (so quoted args like
                # `pytest -k "a and b"` split right and match the allowlist), Windows
                # rules on Windows. Matches shell_session.py.
                shlex.split(command, posix=(os.name != "nt")),
                shell=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                cwd=self.project_root,
                timeout=timeout
            )
            stdout = result.stdout or ""
            stderr = result.stderr or ""
            stdout_path = self._save_command_log(task.id, command, "stdout", stdout)
            stderr_path = self._save_command_log(task.id, command, "stderr", stderr)
            stdout_summary = redact_string(stdout[-500:])
            stderr_summary = redact_string(stderr[-500:])
            self.tracer.log_event(
                "command_executed",
                {"command": command, "exit_code": result.returncode},
                task_id=task.id,
                summary=f"{command} exited {result.returncode}",
            )
            return CommandResult(
                command=command,
                exit_code=result.returncode,
                stdout_path=stdout_path,
                stderr_path=stderr_path,
                summary=f"Exit code {result.returncode}. stdout: {stdout_summary}. stderr: {stderr_summary}"
            )
        except Exception as e:
            raise ExecutionError(f"Command execution failed: {e}")

    def write_file(self, path: str, content: str, task: Task):
        """Write content to a file if allowed by permissions."""
        self._validate_path_within_root(path)
        full_path = resolve_project_path(self.project_root, path)
        operation: Literal["create", "modify"] = "modify" if full_path.exists() else "create"
        self.permissions.validate_action("file_write", path, task, operation=operation)

        logger.info(f"Writing authorized file: {path}")
        try:
            full_path.parent.mkdir(parents=True, exist_ok=True)
            full_path.write_text(content, encoding="utf-8")
            self.tracer.log_event(
                "file_written",
                {"path": path},
                task_id=task.id,
                summary=f"Wrote {path}",
            )
        except Exception as e:
            raise ExecutionError(f"Failed to write file {path}: {e}")
