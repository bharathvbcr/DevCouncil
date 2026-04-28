from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap

class PlannedFilesCheck:
    """Ensures a task has legitimate files planned for modification."""
    
    def check(self, task: Task) -> list[Gap]:
        gaps = []
        if not task.planned_files:
            gaps.append(Gap(
                id=f"GAP-{task.id}-NO-FILES",
                severity="high",
                gap_type="task_not_implemented",
                task_id=task.id,
                description=f"Task {task.id} has no planned files. Agents won't know where to write code.",
                recommended_fix="Update the task to include at least one planned file path.",
                blocking=True
            ))
        
        has_modify = any(pf.allowed_change in ["create", "modify", "delete"] for pf in task.planned_files)
        if task.planned_files and not has_modify:
            gaps.append(Gap(
                id=f"GAP-{task.id}-READ-ONLY",
                severity="medium",
                gap_type="task_not_implemented",
                task_id=task.id,
                description=f"Task {task.id} only has read-only files. No changes can be made.",
                recommended_fix="Grant 'modify' or 'create' permissions to at least one file.",
                blocking=True
            ))
            
        return gaps
