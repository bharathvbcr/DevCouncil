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


def test_failed_evidence_is_scoped_to_task(tmp_path):
    from devcouncil.domain.evidence import CommandResult
    from devcouncil.storage.repositories import EvidenceRepository

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "project:\n  name: test\n", encoding="utf-8"
    )
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    task = Task(id="TASK-001", title="T", description="D",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
                expected_tests=["pytest tests/a"])
    with db.get_session() as session:
        TaskRepository(session).save(task)
        repo = EvidenceRepository(session)
        repo.save_command_result("TASK-001", CommandResult(
            command="pytest tests/a", exit_code=1, stdout_path="", stderr_path="", summary="mine failed"))
        repo.save_command_result("TASK-002", CommandResult(
            command="pytest tests/other", exit_code=1, stdout_path="", stderr_path="", summary="unrelated"))

    manifest = build_correction_manifest(tmp_path, task, [Gap(
        id="G", severity="high", gap_type="test_failed", task_id="TASK-001",
        description="failed", recommended_fix="fix", blocking=True)])

    joined = " ".join(manifest.failed_evidence)
    assert "pytest tests/a" in joined
    assert "pytest tests/other" not in joined  # another task's failure must not leak in


def test_repair_plan_scope_merged_into_manifest(tmp_path):
    from devcouncil.planning.repair_service import RepairOutput

    (tmp_path / ".devcouncil").mkdir()
    (tmp_path / ".devcouncil" / "config.yaml").write_text("project:\n  name: test\n", encoding="utf-8")
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    db.create_db_and_tables()
    task = Task(id="TASK-001", title="T", description="D",
                planned_files=[PlannedFile(path="src/a.py", reason="x", allowed_change="modify")],
                expected_tests=["pytest tests/a"])

    class _FakeRepair:
        async def generate_repair_plan(self, gaps, context):
            return RepairOutput(suggested_tasks=[Task(
                id="R1", title="repair", description="fix the regex",
                planned_files=[PlannedFile(path="src/b.py", reason="root cause", allowed_change="modify")],
                expected_tests=["pytest tests/b"])])

    manifest = build_correction_manifest(
        tmp_path, task,
        [Gap(id="G", severity="high", gap_type="test_failed", task_id="TASK-001",
             description="failed", recommended_fix="fix", blocking=True)],
        repair_service=_FakeRepair(),
    )

    assert manifest.root_cause == "fix the regex"
    assert "src/a.py" in manifest.allowed_repair_files  # task scope kept
    assert "src/b.py" in manifest.allowed_repair_files  # repair-plan scope added
    assert "pytest tests/a" in manifest.commands_to_rerun
    assert "pytest tests/b" in manifest.commands_to_rerun
