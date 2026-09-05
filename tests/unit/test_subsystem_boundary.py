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


_NEIGHBORS_NEVER_COMPUTED = {
    "subsystems": [
        {"area": "src/ui", "neighbors": []},
        {"area": "src/api", "neighbors": []},
        {"area": "src/storage", "neighbors": []},
    ],
}


def test_a_map_that_never_computed_neighbors_reports_that_it_could_not_check():
    """Class A: "could not check" must not read as "checked, and here is drift".

    Every subsystem's `neighbors` is the literal `[]` the kernel writes, so no
    pair in this map is known to be non-adjacent. Flagging `architecture_drift`
    for such a pair reports a finding about the change when the only fact
    available is a limitation of the producer.
    """
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/storage/b.py"],
        repo_map=_NEIGHBORS_NEVER_COMPUTED, next_gap_id=_ids(),
    )
    assert len(gaps) == 1
    gap = gaps[0]
    assert gap.gap_type == "architecture_check_unavailable"
    assert gap.blocking is False
    assert "neighbour" in gap.description or "neighbor" in gap.description


def test_an_unavailable_check_never_blocks_even_when_configured_to():
    """`blocking` is about an undeclared crossing, not about a producer gap.

    Blocking on this would halt the loop in every repository whose map writer
    does not compute the field — a gate failing closed on its own inability to
    run, which is not the same thing as failing closed on a finding.
    """
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/storage/b.py"],
        repo_map=_NEIGHBORS_NEVER_COMPUTED, next_gap_id=_ids(), blocking=True,
    )
    assert [g.gap_type for g in gaps] == ["architecture_check_unavailable"]
    assert gaps[0].blocking is False


def test_a_single_area_change_still_needs_no_check():
    """Nothing to check is not the same as a check that could not run."""
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/ui/b.py"],
        repo_map=_NEIGHBORS_NEVER_COMPUTED, next_gap_id=_ids(),
    )
    assert gaps == []


_NEIGHBORS_COMPUTED_BUT_CAPPED = {
    "meta": {"devmap_rust": {"neighbors_computed": True}},
    "liveness_meta": {
        "subsystems": {
            "neighbors_shown": 2,
            "neighbors_total": 11,
            "neighbors_truncated": True,
            "neighbors_endpoints_unresolved": 0,
        }
    },
    "subsystems": [
        {"area": "src/ui", "neighbors": ["src/api"]},
        {"area": "src/api", "neighbors": ["src/ui"]},
        {"area": "src/storage", "neighbors": []},
    ],
}


def test_a_capped_relation_is_also_a_check_that_could_not_run():
    """Computed is not the same as complete.

    This map derived neighbours and says so, but reports its per-area lists
    capped — so `src/storage` missing from `src/ui`'s list proves nothing, and a
    gate that only asked "were they computed" would report a clean check off a
    relation it holds two entries of.
    """
    gaps = detect_subsystem_boundary_gaps(
        task=_task(["src/ui/a.py"]),
        changed_files=["src/ui/a.py", "src/storage/b.py"],
        repo_map=_NEIGHBORS_COMPUTED_BUT_CAPPED, next_gap_id=_ids(),
    )
    assert [g.gap_type for g in gaps] == ["architecture_check_unavailable"]
    assert gaps[0].blocking is False
