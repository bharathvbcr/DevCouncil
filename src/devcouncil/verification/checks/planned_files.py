"""Planned-file compliance checks extracted from Verifier."""

from __future__ import annotations

from pathlib import Path
from typing import Callable, List

from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task

_DEP_FILES = {
    "package.json", "package-lock.json", "yarn.lock", "pnpm-lock.yaml",
    "requirements.txt", "pyproject.toml", "uv.lock", "Pipfile.lock",
    "go.mod", "go.sum", "Cargo.toml", "Cargo.lock",
}


def dependency_files_changed(changed_files: List[str]) -> List[str]:
    """Return changed dependency manifest paths."""
    return [path for path in changed_files if Path(path).name in _DEP_FILES]


def detect_no_work_gap(
    *,
    task: Task,
    work_present: bool,
    next_gap_id: Callable[[str, str], str],
) -> Gap | None:
    """Block when a task expects file changes but produced none."""
    expects_change = any(pf.allowed_change != "read_only" for pf in task.planned_files)
    if work_present or not expects_change:
        return None
    return Gap(
        id=next_gap_id(task.id, "NODIFF"),
        severity="high",
        gap_type="task_not_implemented",
        task_id=task.id,
        description=(
            f"Task {task.id} declares files to create or modify, but produced no "
            "changes. Verification cannot prove work that does not exist."
        ),
        evidence=[f"planned files expecting change: {sorted(p.path for p in task.planned_files if p.allowed_change != 'read_only')}"],
        recommended_fix=(
            "Implement the planned changes so the diff is non-empty, then re-verify. "
            "If you did make changes, ensure they are saved and visible to git "
            "(not reverted, stashed, or written outside the project root)."
        ),
        blocking=True,
    )


def detect_planned_file_gaps(
    *,
    task: Task,
    changed_files: List[str],
    next_gap_id: Callable[[str, str], str],
) -> List[Gap]:
    """Advisory gaps for planned files that were not modified."""
    gaps: List[Gap] = []
    changed_set = set(changed_files)
    for pf in task.planned_files:
        if pf.path not in changed_set and pf.allowed_change != "read_only":
            gaps.append(Gap(
                id=next_gap_id(task.id, f"FILE-{pf.path}"),
                severity="medium",
                gap_type="planned_file_not_changed",
                task_id=task.id,
                description=f"Planned file {pf.path} was not modified.",
                recommended_fix=f"Modify {pf.path} as planned or update the task.",
                blocking=False,
                file=pf.path,
            ))
    return gaps


def detect_dependency_risk_gaps(
    *,
    task: Task,
    changed_files: List[str],
    planned_paths: set[str],
    next_gap_id: Callable[[str, str], str],
) -> List[Gap]:
    """Blocking gaps for dependency manifest edits outside the plan."""
    gaps: List[Gap] = []
    for dep_file in dependency_files_changed(changed_files):
        if dep_file not in planned_paths:
            gaps.append(Gap(
                id=next_gap_id(task.id, f"DEP-{dep_file}"),
                severity="high",
                gap_type="dependency_risk",
                task_id=task.id,
                description=f"Dependency file {dep_file} was modified without being in planned files.",
                evidence=[dep_file],
                recommended_fix=f"Justify the dependency change or revert {dep_file}.",
                blocking=True,
                file=dep_file,
            ))
    return gaps
