"""Round-trip persistence tests for the SQLite/SQLModel store.

Covers save -> read-back fidelity for the three core artifacts (Requirement,
Task, Gap), upsert-style task status transitions, and durability across a
fresh ``Database`` handle opened on the same file.

Note: ``Task.depends_on`` and ``Gap.stdout_path``/``Gap.stderr_path`` are
intentionally left at their defaults here — the repository layer does not
persist them, so whole-model equality only holds when they are unset.
"""

from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.storage.db import SCHEMA_VERSION, Database
from devcouncil.storage.models import SchemaVersionModel
from devcouncil.storage.repositories import (
    GapRepository,
    RequirementRepository,
    TaskRepository,
)


def _make_db(tmp_path) -> Database:
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()
    return db


def _requirement() -> Requirement:
    return Requirement(
        id="REQ-001",
        title="Median",
        description="median() computes the statistical median",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(
                id="AC-001",
                description="median([]) raises ValueError",
                verification_method="unit_test",
                required=True,
            ),
            AcceptanceCriterion(
                id="AC-002",
                description="median([1, 3, 2]) == 2",
                verification_method="unit_test",
                required=False,
            ),
        ],
    )


def _task() -> Task:
    return Task(
        id="TASK-001",
        title="Implement median",
        description="Implement median() in stats.py",
        requirement_ids=["REQ-001"],
        acceptance_criterion_ids=["AC-001", "AC-002"],
        planned_files=[
            PlannedFile(path="stats.py", reason="median logic", allowed_change="modify"),
            PlannedFile(path="tests/test_stats.py", reason="unit tests", allowed_change="create"),
        ],
        expected_tests=["python -m pytest tests/test_stats.py -q"],
        agent_appended_expected_tests=["python -m pytest tests/test_stats_extra.py -q"],
        allowed_commands=["python -m pytest -q"],
        agent_appended_allowed_commands=["python -m compileall stats.py"],
        forbidden_changes=["setup.py"],
        difficulty="hard",
        priority="high",
        status="planned",
    )


def _blocking_gap() -> Gap:
    return Gap(
        id="GAP-TASK-001-AC-001",
        severity="high",
        gap_type="acceptance_criteria_unproven",
        requirement_id="REQ-001",
        task_id="TASK-001",
        description="AC-001 has no passing evidence.",
        evidence=["pytest exited 1"],
        recommended_fix="Fix the failing test and re-run verification.",
        blocking=True,
        file="stats.py",
        line=42,
        suggested_command="python -m pytest tests/test_stats.py -q",
        acceptance_criterion_id="AC-001",
        expected_verification_method="unit_test",
    )


def _advisory_gap() -> Gap:
    return Gap(
        id="GAP-TASK-001-ADVISORY",
        severity="low",
        gap_type="missing_test",
        task_id="TASK-001",
        description="Task TASK-001 has no expected verification evidence.",
        recommended_fix="Add expected_tests that prove the acceptance criteria.",
        blocking=False,
    )


def test_requirement_roundtrip_preserves_acceptance_criteria(tmp_path):
    db = _make_db(tmp_path)
    requirement = _requirement()

    with db.get_session() as session:
        RequirementRepository(session).save(requirement)

    with db.get_session() as session:
        loaded = RequirementRepository(session).get_all()

    assert loaded == [requirement]


def test_task_roundtrip_preserves_all_persisted_fields(tmp_path):
    db = _make_db(tmp_path)
    task = _task()

    with db.get_session() as session:
        TaskRepository(session).save(task)

    with db.get_session() as session:
        loaded = TaskRepository(session).get_by_id("TASK-001")
        missing = TaskRepository(session).get_by_id("TASK-404")

    assert loaded == task
    assert missing is None


def test_gap_roundtrip_preserves_machine_routing_fields(tmp_path):
    db = _make_db(tmp_path)
    blocking_gap = _blocking_gap()
    advisory_gap = _advisory_gap()

    with db.get_session() as session:
        repo = GapRepository(session)
        repo.save(blocking_gap)
        repo.save(advisory_gap)

    with db.get_session() as session:
        repo = GapRepository(session)
        for_task = sorted(repo.get_for_task("TASK-001"), key=lambda g: g.id)
        blocking_only = repo.get_blocking_for_task("TASK-001")
        for_other_task = repo.get_for_task("TASK-404")

    assert for_task == sorted([blocking_gap, advisory_gap], key=lambda g: g.id)
    assert blocking_only == [blocking_gap]
    assert for_other_task == []


def test_task_status_transitions_update_in_place(tmp_path):
    db = _make_db(tmp_path)
    task = _task()

    with db.get_session() as session:
        repo = TaskRepository(session)
        repo.save(task)
        repo.save(task.model_copy(update={"status": "running"}))
        updated = repo.get_by_id("TASK-001")
        assert updated is not None
        assert updated.status == "running"
        repo.save(task.model_copy(update={"status": "verified"}))

    with db.get_session() as session:
        all_tasks = TaskRepository(session).get_all()

    # merge() upserts on the primary key: one row, latest status.
    assert len(all_tasks) == 1
    assert all_tasks[0].status == "verified"


def test_reopening_database_file_preserves_persisted_state(tmp_path):
    db_path = tmp_path / "state.sqlite"
    db = Database(db_path)
    db.create_db_and_tables()
    requirement, task, gap = _requirement(), _task(), _blocking_gap()

    with db.get_session() as session:
        RequirementRepository(session).save(requirement)
        TaskRepository(session).save(task)
        GapRepository(session).save(gap)
    db.engine.dispose()

    reopened = Database(db_path)
    reopened.ensure_schema_version()

    with reopened.get_session() as session:
        assert RequirementRepository(session).get_all() == [requirement]
        assert TaskRepository(session).get_by_id("TASK-001") == task
        assert GapRepository(session).get_for_task("TASK-001") == [gap]
        version = session.get(SchemaVersionModel, "singleton")
        assert version is not None
        assert version.version == SCHEMA_VERSION
