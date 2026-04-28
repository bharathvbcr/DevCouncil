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


def test_cli_integrate_prints_coding_cli_setup_commands(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["integrate", "all"])

    assert result.exit_code == 0
    assert "codex mcp add devcouncil" in result.output
    assert "gemini mcp add --scope project" in result.output
    assert "DEVCOUNCIL_PROJECT_ROOT=" in result.output


def test_cli_integrations_doctor_reports_optional_tools(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["integrations", "doctor"])

    assert result.exit_code == 0
    assert "code-review-graph" in result.output
    assert "Agent Flow" in result.output


def test_cli_setup_agent_flow_dry_run_does_not_write_config(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / ".devcouncil").mkdir()

    result = runner.invoke(app, ["integrations", "setup", "agent-flow"])

    assert result.exit_code == 0
    assert "Preview only" in result.output
    assert not (tmp_path / ".devcouncil" / "config.yaml").exists()


def test_cli_setup_code_review_graph_apply_writes_local_files(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / ".devcouncil").mkdir()

    result = runner.invoke(app, ["integrations", "setup", "code-review-graph", "--apply"])

    assert result.exit_code == 0
    assert (tmp_path / ".code-review-graphignore").exists()
    config = (tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8")
    assert "code_review_graph" in config


def test_cli_trace_tail_prints_jsonl(tmp_path, monkeypatch):
    from devcouncil.telemetry.traces import TraceLogger

    monkeypatch.chdir(tmp_path)
    TraceLogger(tmp_path).log_event("task_verified", {}, task_id="TASK-001")

    result = runner.invoke(app, ["trace", "tail", "--limit", "1"])

    assert result.exit_code == 0
    assert '"schema":"devcouncil.trace.v1"' in result.output
    assert '"task_id":"TASK-001"' in result.output


def test_cli_hook_pre_tool_use_accepts_stdin_payload(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(
        app,
        ["hook", "pre-tool-use"],
        input='{"name":"Bash","arguments":{"command":"git commit --no-verify -m test"}}',
    )

    assert result.exit_code == 2
    assert "Verification bypass" in result.output


def test_cli_integrate_supports_custom_project_root_and_gemini_scope(tmp_path):
    result = runner.invoke(
        app,
        [
            "integrate",
            "gemini",
            "--scope",
            "user",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 0
    assert "gemini mcp add --scope user" in result.output
    assert f"DEVCOUNCIL_PROJECT_ROOT={tmp_path}" in result.output


def test_cli_setup_initializes_project_and_prints_next_commands(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["setup"])

    assert result.exit_code == 0
    assert (tmp_path / ".devcouncil" / "config.yaml").exists()
    assert (tmp_path / ".devcouncil" / "state.sqlite").exists()
    assert "DevCouncil Doctor Check" in result.output
    assert "Keep running DevCouncil commands in this terminal" in result.output
    assert "Paste only the dev prompt output into your coding CLI." in result.output
    assert "dev plan" in result.output
    assert "dev verify TASK-001" in result.output


def test_cli_setup_help_explains_terminal_and_project_root():
    result = runner.invoke(app, ["setup", "--help"])

    assert result.exit_code == 0
    assert "normal terminal" in result.output
    assert "target repository root" in result.output
    assert "current directory" in result.output


def test_cli_setup_can_preview_integrations(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["setup", "--integrate", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "codex mcp add devcouncil" in result.output
    assert "gemini mcp add --scope project" in result.output
    assert f"DEVCOUNCIL_PROJECT_ROOT={tmp_path}" in result.output


def test_cli_setup_apply_skips_missing_optional_integrations(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr("shutil.which", lambda command: None)

    result = runner.invoke(app, ["setup", "--integrate", "--apply"])

    assert result.exit_code == 0
    assert "Skipping optional integration" in result.output
    assert (tmp_path / ".devcouncil" / "config.yaml").exists()


def test_prompt_builder_wraps_paths_and_commands_as_markdown_code():
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.execution.prompt_builder import PromptBuilder

    prompt = PromptBuilder().build_task_prompt(
        Task(
            id="TASK-001",
            title="Prompt render",
            description="Render paths exactly",
            planned_files=[
                PlannedFile(
                    path="src/package/__init__.py",
                    reason="export helper",
                    allowed_change="modify",
                )
            ],
            expected_tests=["uv run pytest"],
            allowed_commands=["uv run pytest"],
            forbidden_changes=["src/package/private_*"],
        ),
        [],
    )

    assert "`src/package/__init__.py`" in prompt
    assert "`uv run pytest`" in prompt
    assert "`src/package/private_*`" in prompt
