import fnmatch
from pathlib import Path
from typing import List, Literal, Optional
from pydantic import BaseModel, Field
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.app.errors import GatingError

class PermissionPolicy(BaseModel):
    """Defines the security boundaries for task execution."""
    allow_file_create: bool = False
    allow_file_delete: bool = False
    allowed_shell_commands: List[str] = Field(default_factory=list)
    restricted_paths: List[str] = Field(default_factory=lambda: [".git/*", ".devcouncil/*", ".env*"])

class PermissionManager:
    def __init__(self, policy: PermissionPolicy, project_root: Path = Path(".")):
        self.policy = policy
        self.project_root = project_root
        self.dynamic_ignores = self._load_devcouncilignore()

    def _load_devcouncilignore(self) -> List[str]:
        """Load additional restricted paths from .devcouncilignore."""
        ignore_file = self.project_root / ".devcouncilignore"
        if ignore_file.exists():
            try:
                lines = ignore_file.read_text().splitlines()
                return [line.strip() for line in lines if line.strip() and not line.startswith("#")]
            except Exception:
                pass
        return []

    def is_file_change_allowed(
        self,
        path: str,
        task: Task,
        operation: Literal["create", "modify", "delete", "write"] = "write",
    ) -> bool:
        """Check if a file change is authorized by the task or policy."""
        # 1. Check restricted paths (e.g. .git) and dynamic ignores
        all_restricted = self.policy.restricted_paths + self.dynamic_ignores
        for restricted in all_restricted:
            if fnmatch.fnmatch(path, restricted) or path.startswith(restricted.strip("*")):
                return False

        # 2. Check if path is in task's planned files with a compatible operation.
        planned = self._planned_file_for(path, task)
        if not planned:
            return False

        if planned.allowed_change == "read_only":
            return False
        if operation == "write":
            return planned.allowed_change in {"create", "modify"}
        return planned.allowed_change == operation

    def _planned_file_for(self, path: str, task: Task) -> Optional[PlannedFile]:
        normalized = path.replace("\\", "/")
        for planned in task.planned_files:
            planned_path = planned.path.replace("\\", "/")
            if normalized == planned_path or fnmatch.fnmatch(normalized, planned_path):
                return planned
        return None

    def is_command_allowed(self, command: str, task: Task) -> bool:
        """Check if a shell command is authorized by the task or global allowlist."""
        # 1. Check task-specific allowlist
        if any(fnmatch.fnmatch(command, allowed) for allowed in task.allowed_commands):
            return True
            
        # 2. Check global policy allowlist
        if any(fnmatch.fnmatch(command, allowed) for allowed in self.policy.allowed_shell_commands):
            return True
            
        return False

    def validate_action(
        self,
        action_type: str,
        target: str,
        task: Task,
        operation: Literal["create", "modify", "delete", "write"] = "write",
    ):
        """Raise GatingError if an execution action violates permissions."""
        if action_type == "file_write":
            if not self.is_file_change_allowed(target, task, operation):
                raise GatingError(
                    f"Unauthorized file {operation}: {target}. "
                    "File and operation must match task planned_files."
                )
        elif action_type == "shell":
            if not self.is_command_allowed(target, task):
                raise GatingError(f"Unauthorized shell command: {target}. Command must be in task's allowed_commands.")
