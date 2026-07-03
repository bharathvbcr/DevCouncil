from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap

class PlannedFilesCheck:
    """Ensures a task has legitimate files planned for modification.

    Calibration note: weak planner models routinely emit auxiliary process tasks
    ("run linter checks", "verify the suite passes") that declare commands or
    expected tests but no planned files. Hard-blocking those stalls the whole run
    and surfaces as a false ``blocked`` verdict on correct code (seen directly in
    the benchmark). A task that declares NOTHING actionable (no files, no commands,
    no tests) still blocks — an executor genuinely cannot do anything with it —
    but a command/verification-only task proceeds with an advisory, and is judged
    on its actual output by the verify gate like any other task.
    """

    def check(self, task: Task) -> list[Gap]:
        gaps = []
        has_runnable_work = bool(task.allowed_commands or task.expected_tests)
        if not task.planned_files:
            gaps.append(Gap(
                id=f"GAP-{task.id}-NO-FILES",
                severity="high" if not has_runnable_work else "medium",
                gap_type="task_not_implemented",
                task_id=task.id,
                description=(
                    f"Task {task.id} has no planned files. Agents won't know where to write code."
                    if not has_runnable_work else
                    f"Task {task.id} has no planned files but declares commands/tests; "
                    "treating it as a command/verification-only task (advisory)."
                ),
                recommended_fix=(
                    "Update the task to include at least one planned file path."
                    if not has_runnable_work else
                    "Add planned files if this task is meant to change code; otherwise this is fine."
                ),
                blocking=not has_runnable_work,
            ))

        has_modify = any(pf.allowed_change in ["create", "modify", "delete"] for pf in task.planned_files)
        if task.planned_files and not has_modify:
            # Read-only-only tasks are analysis tasks: they can still execute (read code,
            # run commands) and are already surfaced as advisory at PLAN time by the
            # plan-approval gate. Blocking them at readiness contradicted that and
            # stalled runs whose planner emitted a legitimate analysis step.
            gaps.append(Gap(
                id=f"GAP-{task.id}-READ-ONLY",
                severity="medium",
                gap_type="task_not_implemented",
                task_id=task.id,
                description=(
                    f"Task {task.id} only has read-only files; it can analyze but not change code. "
                    "Expected only if this is an analysis-only task."
                ),
                recommended_fix="Grant 'modify' or 'create' permissions to at least one file if this task should write code.",
                blocking=False,
            ))

        return gaps
