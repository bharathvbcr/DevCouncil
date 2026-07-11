"""Unit tests for the advisory subsystem-boundary (architecture-drift) gate."""

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.verification.checks.subsystem_boundary import (
    detect_subsystem_boundary_gaps,
)

_REPO_MAP = {
    "subsystems": [
        {"area": "src/ui", "neighbors": ["src/api"]},
        {"area": "src/api", "neighbors": ["src/ui", "src/storage"]},
        {"area": "src/storage", "neighbors": ["src/api"]},
    ],
}


def _task(paths, changes=None):
    changes = changes or {}
    return Task(
        id="TASK-1", title="t", description="d",
        planned_files=[
            PlannedFile(path=p, reason="r", allowed_change=changes.get(p, "modify"))
            for p in paths
        ],
    )


def _ids():
    counter = {"n": 0}

    def next_id(task_id, suffix):
        counter["n"] += 1
        return f"GAP-{task_id}-{suffix}-{counter['n']}"

    return next_id


def test_no_map_is_noop():
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/x.py"]), changed_files=["src/ui/x.py"],
        repo_map=None, next_gap_id=_ids(),
    )
    assert gaps == []


def test_single_area_change_is_clean():
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/ui/b.py"],
        repo_map=_REPO_MAP, next_gap_id=_ids(),
    )
    assert gaps == []


def test_neighbor_crossing_is_not_flagged():
    # ui <-> api are declared neighbors, so crossing them is allowed.
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/api/b.py"],
        repo_map=_REPO_MAP, next_gap_id=_ids(),
    )
    assert gaps == []


def test_non_neighbor_undeclared_crossing_is_flagged_advisory():
    # ui and storage are NOT neighbors; the plan only declared ui.
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/storage/b.py"],
        repo_map=_REPO_MAP, next_gap_id=_ids(),
    )
    assert len(gaps) == 1
    gap = gaps[0]
    assert gap.gap_type == "architecture_drift"
    assert gap.blocking is False
    assert "src/ui" in gap.description and "src/storage" in gap.description


def test_crossing_declared_in_plan_is_covered():
    # Plan declares files in BOTH ui and storage -> crossing is intended, no gap.
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py", "src/storage/b.py"]),
        changed_files=["src/ui/a.py", "src/storage/b.py"],
        repo_map=_REPO_MAP, next_gap_id=_ids(),
    )
    assert gaps == []


def test_blocking_flag_makes_gap_block():
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/storage/b.py"],
        repo_map=_REPO_MAP, next_gap_id=_ids(), blocking=True,
    )
    assert len(gaps) == 1
    assert gaps[0].blocking is True
