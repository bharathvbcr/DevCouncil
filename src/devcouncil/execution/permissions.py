import fnmatch
import logging
from pathlib import Path
from typing import List, Literal, Optional
from pydantic import BaseModel, Field
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.app.errors import GatingError
from devcouncil.execution.policy_engine import TaskPolicyEngine

logger = logging.getLogger(__name__)

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
        self.policy_engine = TaskPolicyEngine(
            project_root,
            global_allowed_commands=policy.allowed_shell_commands,
        )

    def _load_devcouncilignore(self) -> List[str]:
        """Load additional restricted paths from .devcouncilignore."""
        ignore_file = self.project_root / ".devcouncilignore"
        if ignore_file.exists():
            try:
                lines = ignore_file.read_text().splitlines()
                return [line.strip() for line in lines if line.strip() and not line.startswith("#")]
            except Exception:
                logger.debug("Failed to read .devcouncilignore", exc_info=True)
                pass
        return []

    def is_file_change_allowed(
        self,
        path: str,
        task: Task,
        operation: Literal["create", "modify", "delete", "write"] = "write",
        *,
        internal: bool = False,
    ) -> bool:
        """Check if a file change is authorized by the task or policy."""
        for restricted in self.dynamic_ignores:
            if fnmatch.fnmatch(path, restricted) or path.startswith(restricted.strip("*")):
                return False
        decision = self.policy_engine.evaluate_file_change(
            path, task, operation, internal=internal
        )
        return decision.action in {"allow", "warn"}

    def _planned_file_for(self, path: str, task: Task) -> Optional[PlannedFile]:
        normalized = path.replace("\\", "/")
        for planned in task.planned_files:
            planned_path = planned.path.replace("\\", "/")
            if normalized == planned_path or fnmatch.fnmatch(normalized, planned_path):
                return planned
        return None

    def is_command_allowed(self, command: str, task: Task) -> bool:
        """Check if a shell command is authorized by the task or global allowlist."""
        decision = self.policy_engine.evaluate_command(command, task)
        return decision.action == "allow"

    def validate_action(
        self,
        action_type: str,
        target: str,
        task: Task,
        operation: Literal["create", "modify", "delete", "write"] = "write",
        *,
        internal: bool = False,
    ):
        """Raise GatingError if an execution action violates permissions."""
        if action_type == "file_write":
            if not self.is_file_change_allowed(target, task, operation, internal=internal):
                logger.warning("DENIED file %s for %s: %s (not in planned_files)", operation, task.id, target)
                raise GatingError(
                    f"Unauthorized file {operation}: {target}. "
                    "File and operation must match task planned_files."
                )
            logger.debug("Allowed file %s for %s: %s", operation, task.id, target)
        elif action_type == "shell":
            if not self.is_command_allowed(target, task):
                logger.warning("DENIED shell command for %s: %s (not in allowed_commands)", task.id, target)
                raise GatingError(f"Unauthorized shell command: {target}. Command must be in task's allowed_commands.")
            logger.debug("Allowed shell command for %s: %s", task.id, target)
