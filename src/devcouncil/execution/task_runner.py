import hashlib
import subprocess
import logging
import re
import shlex
from pathlib import Path
from typing import Optional
from devcouncil.domain.task import Task
from devcouncil.execution.permissions import PermissionManager
from devcouncil.domain.evidence import CommandResult
from devcouncil.app.errors import ExecutionError

from devcouncil.execution.patch import PatchEngine
from devcouncil.execution.paths import resolve_project_path

logger = logging.getLogger(__name__)

class TaskRunner:
    """Safely executes task actions while enforcing permission boundaries."""
    
    def __init__(self, project_root: Path, permission_manager: PermissionManager):
        self.project_root = project_root
        self.permissions = permission_manager
        self.patch_engine = PatchEngine(project_root)

    def _validate_path_within_root(self, path: str) -> None:
        """Ensure a path resolves to a location within the project root."""
        resolve_project_path(self.project_root, path)

    def apply_patch(self, patch: str, task: Task) -> bool:
        """Apply a patch if permissions allow (all affected files must be in planned_files)."""
        for path in self._extract_patch_paths(patch):
            self._validate_path_within_root(path)
            self.permissions.validate_action("file_write", path, task)
        return self.patch_engine.apply_patch(patch)

    def _extract_patch_paths(self, patch: str) -> set[str]:
        """Extract repository-relative file paths touched by a unified git patch."""
        paths: set[str] = set()
        for line in patch.splitlines():
            if line.startswith("diff --git "):
                parts = line.split()
                for raw in parts[2:4]:
                    cleaned = self._normalize_patch_path(raw)
                    if cleaned:
                        paths.add(cleaned)
                continue

            if line.startswith(("--- ", "+++ ")):
                raw = line[4:].split("\t", 1)[0].strip()
                cleaned = self._normalize_patch_path(raw)
                if cleaned:
                    paths.add(cleaned)

        if not paths:
            raise ExecutionError("Patch does not declare any affected files.")
        return paths

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
        log_path.write_text(content, encoding="utf-8")
        return str(log_path)

    def run_command(self, command: str, task: Task) -> CommandResult:
        """Execute a shell command if allowed by permissions."""
        self.permissions.validate_action("shell", command, task)
        
        logger.info(f"Executing authorized command: {command}")
        
        try:
            from devcouncil.app.config import load_config
            config = load_config(self.project_root)
            timeout = config.execution.command_timeout
        except Exception:
            timeout = 300

        try:
            result = subprocess.run(
                shlex.split(command, posix=False),
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
            return CommandResult(
                command=command,
                exit_code=result.returncode,
                stdout_path=stdout_path,
                stderr_path=stderr_path,
                summary=f"Exit code {result.returncode}. stdout: {stdout[-500:]}. stderr: {stderr[-500:]}"
            )
        except Exception as e:
            raise ExecutionError(f"Command execution failed: {e}")

    def write_file(self, path: str, content: str, task: Task):
        """Write content to a file if allowed by permissions."""
        self._validate_path_within_root(path)
        self.permissions.validate_action("file_write", path, task)
        
        full_path = resolve_project_path(self.project_root, path)
        logger.info(f"Writing authorized file: {path}")
        try:
            full_path.parent.mkdir(parents=True, exist_ok=True)
            full_path.write_text(content, encoding="utf-8")
        except Exception as e:
            raise ExecutionError(f"Failed to write file {path}: {e}")
