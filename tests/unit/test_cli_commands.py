import json

from typer.testing import CliRunner

from devcouncil.cli.main import app


runner = CliRunner()


def test_cli_map_writes_repo_map(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "pyproject.toml").write_text("[project]\nname='sample'\n", encoding="utf-8")
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "test_sample.py").write_text("def test_sample(): pass\n", encoding="utf-8")

    init_result = runner.invoke(app, ["init"])
    assert init_result.exit_code == 0

    map_result = runner.invoke(app, ["map", "sample", "--output", ".devcouncil/repo_map.json"])
    assert map_result.exit_code == 0

    data = json.loads((tmp_path / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert "python" in data["languages"]
    assert "uv" not in data["package_managers"]


def test_cli_baseline_writes_snapshot(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "README.md").write_text("baseline\n", encoding="utf-8")

    assert runner.invoke(app, ["init"]).exit_code == 0
    result = runner.invoke(app, ["baseline"])

    assert result.exit_code == 0
    baseline = json.loads((tmp_path / ".devcouncil" / "baseline.json").read_text(encoding="utf-8"))
    assert "README.md" in baseline["changed_files"]


def test_cli_artifacts_validate_fails_on_empty_task_commands(tmp_path, monkeypatch):
    from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-001",
            title="Requirement",
            description="desc",
            priority="high",
            source="user",
            acceptance_criteria=[
                AcceptanceCriterion(
                    id="AC-001",
                    description="testable",
                    verification_method="unit_test",
                )
            ],
        ))
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Task",
            description="desc",
            requirement_ids=["REQ-001"],
            acceptance_criterion_ids=["AC-001"],
            planned_files=[
                PlannedFile(path="src/app.py", reason="logic", allowed_change="modify"),
            ],
        ))

    result = runner.invoke(app, ["artifacts", "validate"])

    assert result.exit_code == 1
    assert "must define allowed commands or expected tests" in result.output


def test_cli_plan_dry_run_does_not_persist_mock_tasks(tmp_path, monkeypatch):
    from devcouncil.storage.db import get_db
    from devcouncil.storage.models import ProjectStateModel
    from devcouncil.storage.repositories import RequirementRepository, StateRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        StateRepository(session).save_state(ProjectStateModel(current_phase="PLAN_APPROVED"))

    result = runner.invoke(app, ["plan", "Add password reset", "--dry-run"])

    assert result.exit_code == 0
    assert "were not persisted" in result.output
    with db.get_session() as session:
        assert RequirementRepository(session).get_all() == []
        assert TaskRepository(session).get_all() == []
        assert StateRepository(session).get_state().current_phase == "PLAN_APPROVED"


def test_cli_plan_dry_run_can_persist_when_requested(tmp_path, monkeypatch):
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["plan", "Add password reset", "--dry-run", "--persist"])

    assert result.exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        assert [req.id for req in RequirementRepository(session).get_all()] == ["REQ-001"]
        assert [task.id for task in TaskRepository(session).get_all()] == ["TASK-001"]
