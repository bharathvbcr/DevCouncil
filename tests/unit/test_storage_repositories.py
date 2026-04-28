from devcouncil.domain.evidence import CommandResult, TestEvidence
from devcouncil.domain.gap import Gap
from devcouncil.storage.db import Database, SCHEMA_VERSION
from devcouncil.storage.models import ProjectStateModel, SchemaVersionModel
from devcouncil.storage.repositories import EvidenceRepository, GapRepository, StateRepository


def test_gap_repository_deletes_task_scoped_gaps(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()

    with db.get_session() as session:
        repo = GapRepository(session)
        repo.save(Gap(
            id="GAP-TASK-001",
            severity="high",
            gap_type="orphan_diff",
            task_id="TASK-001",
            description="orphan",
            recommended_fix="revert",
            blocking=True,
        ))
        repo.save(Gap(
            id="GAP-TASK-002",
            severity="high",
            gap_type="orphan_diff",
            task_id="TASK-002",
            description="orphan",
            recommended_fix="revert",
            blocking=True,
        ))

        repo.delete_for_task("TASK-001")

        remaining = repo.get_all()
        assert [gap.id for gap in remaining] == ["GAP-TASK-002"]


def test_evidence_repository_deletes_task_scoped_command_and_test_evidence(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()

    with db.get_session() as session:
        repo = EvidenceRepository(session)
        repo.save_command_result("TASK-001", CommandResult(
            command="pytest",
            exit_code=0,
            stdout_path="",
            stderr_path="",
            summary="passed",
        ))
        repo.save_test_evidence(TestEvidence(
            requirement_id="REQ-001",
            acceptance_criterion_id="AC-001",
            command="pytest",
            status="passed",
            evidence_summary="passed",
        ), task_id="TASK-001")
        repo.save_command_result("TASK-002", CommandResult(
            command="pytest",
            exit_code=0,
            stdout_path="",
            stderr_path="",
            summary="passed",
        ))

        repo.delete_for_task("TASK-001")

        remaining = repo.get_all()
        assert len(remaining) == 1
        assert isinstance(remaining[0], CommandResult)


def test_database_records_schema_version(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()

    with db.get_session() as session:
        version = session.get(SchemaVersionModel, "singleton")
        assert version is not None
        assert version.version == SCHEMA_VERSION


def test_state_repository_uses_singleton_string_id(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()

    with db.get_session() as session:
        repo = StateRepository(session)
        repo.save_state(ProjectStateModel(current_phase="NEW"))
        state = repo.get_state()

        assert state is not None
        assert state.id == "singleton"
        assert state.current_phase == "NEW"


def test_state_repository_records_phase_history_without_duplicate_consecutive_entries(tmp_path):
    db = Database(tmp_path / "state.sqlite")
    db.create_db_and_tables()

    with db.get_session() as session:
        repo = StateRepository(session)
        repo.record_phase("TASK_VERIFYING")
        repo.record_phase("TASK_VERIFYING")
        repo.record_phase("TASK_BLOCKED")
        state = repo.get_state()

        assert state is not None
        assert state.current_phase == "TASK_BLOCKED"
        assert state.history_json == '["TASK_VERIFYING", "TASK_BLOCKED"]'
