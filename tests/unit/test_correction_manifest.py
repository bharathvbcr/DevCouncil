from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.planning.correction_manifest import build_correction_manifest, write_correction_manifest
from devcouncil.storage.db import Database
from devcouncil.storage.repositories import GapRepository, TaskRepository
from typer.testing import CliRunner

from devcouncil.cli.main import app


def test_deterministic_fallback_manifest(tmp_path, monkeypatch):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\nexecution:\n  max_repair_attempts: 2\n",
        encoding="utf-8",
    )
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    task = Task(
        id="TASK-001",
        title="T",
        description="D",
        planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
        expected_tests=["pytest tests/"],
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
        GapRepository(session).save(
            Gap(
                id="GAP-1",
                severity="high",
                gap_type="test_failed",
                task_id="TASK-001",
                description="tests failed",
                recommended_fix="fix tests",
                blocking=True,
            )
        )
    path = write_correction_manifest(tmp_path, "TASK-001")
    assert path is not None
    manifest = build_correction_manifest(
        tmp_path,
        task,
        [Gap(
            id="GAP-1",
            severity="high",
            gap_type="test_failed",
            task_id="TASK-001",
            description="tests failed",
            recommended_fix="fix",
            blocking=True,
        )],
    )
    assert manifest.retry_budget == 2
    assert "src/a.py" in manifest.allowed_repair_files


def test_cli_repair_writes_fallback_manifest_without_model_credentials(tmp_path, monkeypatch):
    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\nmodels:\n  provider: openrouter\n",
        encoding="utf-8",
    )
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        TaskRepository(session).save(
            Task(
                id="TASK-001",
                title="T",
                description="D",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
                expected_tests=["pytest tests/"],
            )
        )
        GapRepository(session).save(
            Gap(
                id="GAP-1",
                severity="high",
                gap_type="test_failed",
                task_id="TASK-001",
                description="tests failed",
                recommended_fix="fix tests",
                blocking=True,
            )
        )

    result = CliRunner().invoke(app, ["repair", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Wrote correction manifest" in result.output
    assert list((tmp_path / ".devcouncil" / "runs").glob("*/correction-manifest.json"))
