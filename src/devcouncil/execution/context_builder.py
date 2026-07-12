from pathlib import Path
from typing import List

from devcouncil.domain.task import Task


class ContextBuilder:
    """Gathers repo-level context for agent prompts."""

    def __init__(self, project_root: Path):
        self.project_root = project_root

    def get_structure_summary(self, task: Task | None = None) -> List[str]:
        """Simple list of files in the project for context."""
        try:
            from devcouncil.utils.proc import git_output

            output = git_output(
                ["ls-files", "--cached", "--others", "--exclude-standard"],
                cwd=self.project_root,
            ).splitlines()

            if task and task.planned_files:
                planned_paths = {pf.path for pf in task.planned_files}
                output = [p for p in output if p in planned_paths] + [
                    p for p in output if p not in planned_paths
                ]

            return output[:100]  # Limit to avoid context overflow
        except Exception:
            return []
