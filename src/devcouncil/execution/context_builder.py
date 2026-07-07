from pathlib import Path
from typing import List
from devcouncil.domain.task import Task
from devcouncil.domain.requirement import Requirement
from devcouncil.utils.redaction import redact_string
import json

class ContextBuilder:
    """Gathers repo-level and task-level context for agent prompts."""
    
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def build_task_context(self, task: Task, requirements: List[Requirement]) -> str:
        """Collects all relevant information for implementing a task."""
        
        # 1. Map requirements relevant to this task
        req_map = {r.id: r for r in requirements}
        task_reqs = [req_map[rid] for rid in task.requirement_ids if rid in req_map]
        
        # 2. Gather content of planned files (if they exist)
        file_contents = {}
        for pf in task.planned_files:
            file_path = self.project_root / pf.path
            if file_path.exists() and file_path.is_file():
                try:
                    raw = file_path.read_text(encoding="utf-8")
                    file_contents[pf.path] = redact_string(raw)
                except Exception:
                    file_contents[pf.path] = "[Error reading file]"
            else:
                file_contents[pf.path] = "[New file - does not exist yet]"

        # 3. Assemble the context string
        context = {
            "task": task.model_dump(),
            "relevant_requirements": [r.model_dump() for r in task_reqs],
            "file_contents": file_contents,
            "project_structure": self.get_structure_summary(task)
        }
        
        return json.dumps(context, indent=2)

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
                output = [p for p in output if p in planned_paths] + [p for p in output if p not in planned_paths]
                
            return output[:100] # Limit to avoid context overflow
        except Exception:
            return []
