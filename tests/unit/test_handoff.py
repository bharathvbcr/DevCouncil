from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.handoff import HandoffService
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import GapRepository, TaskRepository


def test_manifest_saved(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="T",
                description="D",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
            )
        )
        GapRepository(session).save(
            Gap(
                id="GAP-1",
                severity="high",
                gap_type="test_failed",
                task_id="TASK-001",
                description="failed",
                recommended_fix="fix",
                blocking=True,
            )
        )
    manifest, path, _run = HandoffService(tmp_path).create("TASK-001", "codex", "aider")
    assert path.exists()
    assert manifest.task["id"] == "TASK-001"
    assert manifest.from_agent == "codex"
    assert manifest.to_agent == "aider"
