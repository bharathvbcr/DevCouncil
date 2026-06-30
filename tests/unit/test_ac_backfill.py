from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.planning.plan_service import backfill_acceptance_criteria


def _req(req_id, *ac_ids):
    return Requirement(
        id=req_id, title="t", description="d", priority="high", source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id=a, description=f"{a} behavior", verification_method="unit_test")
            for a in ac_ids
        ],
    )


def _task(task_id, req_id, *, acs=(), change="modify", path="src/a.py"):
    return Task(
        id=task_id, title="t", description="d", requirement_ids=[req_id],
        acceptance_criterion_ids=list(acs),
        planned_files=[PlannedFile(path=path, reason="x", allowed_change=change)],
    )


def test_backfill_attaches_uncovered_acs_to_owning_task():
    req = _req("REQ-1", "AC-1", "AC-2", "AC-3")
    task = _task("T1", "REQ-1", acs=["AC-1"])  # planner linked only AC-1; AC-2/AC-3 dropped
    tasks, backfilled = backfill_acceptance_criteria([task], [req])
    assert set(tasks[0].acceptance_criterion_ids) == {"AC-1", "AC-2", "AC-3"}
    assert {ac for _, ac in backfilled} == {"AC-2", "AC-3"}


def test_backfill_prefers_a_writable_task():
    req = _req("REQ-1", "AC-1")
    reader = _task("T-read", "REQ-1", change="read_only", path="src/a.py")
    writer = _task("T-write", "REQ-1", change="create", path="src/b.py")
    tasks, _ = backfill_acceptance_criteria([reader, writer], [req])
    by_id = {t.id: t for t in tasks}
    assert by_id["T-write"].acceptance_criterion_ids == ["AC-1"]
    assert by_id["T-read"].acceptance_criterion_ids == []


def test_backfill_targets_primary_implementer_over_list_order():
    # Two writable tasks on the requirement; the one already owning an AC (the primary
    # implementer) gets the backfill even though the other appears first in the list.
    req = _req("REQ-1", "AC-1", "AC-2")
    helper = _task("T-helper", "REQ-1", change="create", path="src/util.py")
    primary = _task("T-primary", "REQ-1", acs=["AC-1"], change="create", path="src/main.py")
    tasks, _ = backfill_acceptance_criteria([helper, primary], [req])  # helper first
    by_id = {t.id: t for t in tasks}
    assert set(by_id["T-primary"].acceptance_criterion_ids) == {"AC-1", "AC-2"}
    assert by_id["T-helper"].acceptance_criterion_ids == []


def test_backfill_is_noop_when_every_ac_is_covered():
    req = _req("REQ-1", "AC-1")
    task = _task("T1", "REQ-1", acs=["AC-1"])
    tasks, backfilled = backfill_acceptance_criteria([task], [req])
    assert backfilled == []
    assert tasks[0] is task  # unchanged when nothing to do


def test_backfill_leaves_ac_whose_requirement_no_task_owns():
    # A requirement with no task is a requirement-coverage gap (handled elsewhere); the
    # backfill must not crash or invent a link.
    req = _req("REQ-1", "AC-1")
    other = _task("T1", "REQ-OTHER")
    tasks, backfilled = backfill_acceptance_criteria([other], [req])
    assert backfilled == []
    assert tasks[0].acceptance_criterion_ids == []
