import json
import subprocess
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.main import app


runner = CliRunner()


def test_cli_check_reports_scope_and_secrets(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    (tmp_path / "app.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "-c", "user.email=t@t.com", "-c", "user.name=t", "commit", "-m", "init"],
                   cwd=tmp_path, capture_output=True)
    assert runner.invoke(app, ["init"]).exit_code == 0
    # make a change that includes a secret-looking value
    (tmp_path / "app.py").write_text(
        'x = 1\nAPI_KEY = "sk-1234567890abcdefghijklmnopqrstuv"\n', encoding="utf-8"
    )

    result = runner.invoke(app, ["check", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.output)
    assert "app.py" in data["changed_files"]
    assert data["secret_findings"]  # the fake API key is flagged


def test_cli_check_clean_tree_reports_nothing(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    (tmp_path / "app.py").write_text("x = 1\n", encoding="utf-8")
    subprocess.run(["git", "add", "-A"], cwd=tmp_path, capture_output=True)
    subprocess.run(["git", "-c", "user.email=t@t.com", "-c", "user.name=t", "commit", "-m", "init"],
                   cwd=tmp_path, capture_output=True)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["check", "--json"])

    assert result.exit_code == 0
    assert json.loads(result.output)["ok"] is True


def test_cli_check_expands_github_reference_goal(tmp_path, monkeypatch):
    import devcouncil.cli.commands.check as check_cmd
    from devcouncil.verification.ad_hoc_check import AdHocCheckResult

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    captured = {}
    monkeypatch.setattr(
        check_cmd, "resolve_goal_intent",
        lambda goal, root: ("Implement issue #142: Add reset\n\nbody", "Pulled intent from issue #142 via gh."),
    )

    def fake_gate(root, requirement, **kwargs):
        captured["requirement"] = requirement
        return AdHocCheckResult(requirement=requirement, reason="no_changes", changed_files=[], gaps=[], next_actions=[])

    monkeypatch.setattr(check_cmd, "run_working_tree_check", fake_gate)

    result = runner.invoke(app, ["check", "--goal", "#142", "--verify"])

    # The evidence gate received the expanded issue text, not the bare "#142".
    assert captured["requirement"].startswith("Implement issue #142: Add reset")
    assert "Pulled intent from issue #142" in result.output


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


def test_cli_map_auto_initializes_when_missing(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "pyproject.toml").write_text("[project]\nname='sample'\n", encoding="utf-8")
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "test_sample.py").write_text("def test_sample(): pass\n", encoding="utf-8")

    result = runner.invoke(app, ["map", "sample", "--output", ".devcouncil/repo_map.json"])
    assert result.exit_code == 0

    data = json.loads((tmp_path / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert "python" in data["languages"]
    assert "uv" not in data["package_managers"]
    assert (tmp_path / ".devcouncil" / "state.sqlite").exists()


def test_cli_map_auto_initializes_with_project_root(tmp_path, monkeypatch):
    project = tmp_path / "project"
    project.mkdir()
    (project / "pyproject.toml").write_text("[project]\nname='sample'\n", encoding="utf-8")
    (project / "sample.py").write_text("print('sample')\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["map", "sample", "--project-root", str(project), "--output", ".devcouncil/repo_map.json"])
    assert result.exit_code == 0

    data = json.loads((project / ".devcouncil" / "repo_map.json").read_text(encoding="utf-8"))
    assert "python" in data["languages"]
    assert (project / ".devcouncil" / "state.sqlite").exists()


def test_cli_map_stdout_is_machine_readable_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "pyproject.toml").write_text("[project]\nname='sample'\n", encoding="utf-8")
    (tmp_path / "sample.py").write_text("print('sample')\n", encoding="utf-8")
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["map", "sample"])

    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert "python" in data["languages"]


def test_cli_init_accepts_model_selection_options(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)

    result = runner.invoke(
        app,
        [
            "init",
            "--provider",
            "vertexai",
            "--model",
            "google/gemini-custom",
            "--role-model",
            "critic_a=google/gemini-critic",
        ],
    )

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "vertexai"
    assert raw_config["models"]["roles"]["spec_writer"]["model"] == "google/gemini-custom"
    assert raw_config["models"]["roles"]["critic_a"]["model"] == "google/gemini-critic"


def test_cli_init_defaults_to_manual_executor(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["init"])

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["execution"]["default_executor"] == "manual"


def test_cli_init_creates_gitignore(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    # Pre-create gitignore with existing contents to test appending
    gitignore_path = tmp_path / ".gitignore"
    gitignore_path.write_text("my_ignored_file\n", encoding="utf-8")

    result = runner.invoke(app, ["init"])
    assert result.exit_code == 0
    assert gitignore_path.exists()
    content = gitignore_path.read_text(encoding="utf-8")
    assert "my_ignored_file" in content
    assert ".devcouncil/*" in content
    assert "!.devcouncil/config.yaml" in content
    assert ".aider*" in content
    assert ".gemini/" in content
    assert ".claude*" in content
    assert ".cursor/" in content
    assert ".agents/" in content
    assert ".codex/" in content
    assert ".openhands/" in content
    assert ".opencode/" in content
    assert "*.tmp" in content
    assert "*.log" in content
    assert "logs/" in content
    assert "tmp/" in content
    assert "scratch/" in content
    assert "dumps/" in content
    assert ".env" in content
    assert "__pycache__/" in content
    assert ".conductor/" in content
    assert ".conducor/" in content


def test_cli_init_rejects_invalid_role_model_override(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["init", "--role-model", "critic_a"])

    assert result.exit_code == 2
    assert "Use ROLE=MODEL" in result.output
    assert not (tmp_path / ".devcouncil").exists()


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


def test_cli_artifacts_validate_honors_project_root(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    project = tmp_path / "project"
    project.mkdir()
    monkeypatch.chdir(tmp_path)

    initialize_project(project)

    result = runner.invoke(app, ["artifacts", "validate", "--project-root", str(project)])

    assert result.exit_code == 0
    assert "Artifacts are valid" in result.output
    assert not (tmp_path / ".devcouncil").exists()


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


def test_cli_plan_reports_unsupported_provider_before_missing_key(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("ACME_API_KEY", raising=False)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    raw_config["models"]["provider"] = "acme"
    config_path.write_text(yaml.dump(raw_config), encoding="utf-8")

    result = runner.invoke(app, ["plan", "Add password reset"])

    assert result.exit_code == 0
    assert "Unsupported model provider 'acme'" in result.output
    assert "ACME_API_KEY" not in result.output


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


def test_cli_plan_quick_mode_skips_the_council(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["plan", "Add password reset", "--dry-run", "--persist", "--quick"])

    assert result.exit_code == 0
    assert "Final Tasks: 1" in result.output
    run_dirs = list((tmp_path / ".devcouncil" / "runs").glob("*"))
    assert len(run_dirs) == 1
    run_dir = run_dirs[0]
    # Single plan + synthesized decision; no A/B debate, critique, or rebuttal calls.
    assert (run_dir / "plan_a.json").exists()
    assert (run_dir / "decision.json").exists()
    assert not (run_dir / "plan_b.json").exists()
    assert not (run_dir / "critique_a.json").exists()
    assert not (run_dir / "rebuttal_a.json").exists()
    # The spec's requirements pass through verbatim as the final requirements.
    decision = json.loads((run_dir / "decision.json").read_text(encoding="utf-8"))
    assert [r["id"] for r in decision["final_requirements"]] == ["REQ-001"]
    assert [t["id"] for t in decision["final_tasks"]] == ["TASK-001"]


def test_cli_plan_dry_run_saves_codebase_prompt_enhancement(tmp_path, monkeypatch):
    from devcouncil.telemetry.traces import read_trace_events

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["plan", "Add password reset", "--dry-run"])

    assert result.exit_code == 0
    artifacts = list((tmp_path / ".devcouncil" / "runs").glob("*/prompt_enhancement.json"))
    assert len(artifacts) == 1
    enhancement = json.loads(artifacts[0].read_text(encoding="utf-8"))
    assert enhancement["original_goal"] == "Add password reset"
    assert "existing patterns" in enhancement["enhanced_goal"]
    assert enhancement["debate_focus"]
    assert "Prompt enhancement:" in result.output

    events = list(read_trace_events(tmp_path))
    prompt_events = [event for event in events if event.type == "prompt_enhanced"]
    assert len(prompt_events) == 1
    assert prompt_events[0].details["original_goal"] == "Add password reset"
    assert prompt_events[0].details["debate_focus_count"] == 1
    assert prompt_events[0].details["artifact"].endswith("/prompt_enhancement.json")


def test_cli_plan_sends_enhanced_prompt_to_debate_services(tmp_path, monkeypatch):
    from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.planning.arbiter_service import ArbiterDecision
    from devcouncil.planning.critique_service import CritiqueOutput, RebuttalOutput
    from devcouncil.planning.plan_service import PlanOutput
    from devcouncil.planning.prompt_enhancer_service import PromptEnhancement
    from devcouncil.planning.spec_service import SpecOutput

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    seen = {"spec": [], "plan": [], "arbiter": []}

    async def fake_enhance(self, goal, repo_map_json, graph_context_json=None, project_root=None):
        _ = self, repo_map_json, graph_context_json, project_root
        return PromptEnhancement(
            original_goal="wrong model echo",
            enhanced_goal=f"Enhanced for this codebase: {goal}",
            codebase_context=[" src/app.py "],
            debate_focus=["compare minimal and robust plans"],
            constraints=["preserve user intent"],
        ).normalized(goal)

    async def fake_spec(self, goal, repo_map_json):
        _ = self, repo_map_json
        seen["spec"].append(goal)
        return SpecOutput(
            requirements=[
                Requirement(
                    id="REQ-001",
                    title="Requirement",
                    description="desc",
                    priority="high",
                    source="user",
                    acceptance_criteria=[
                        AcceptanceCriterion(
                            id="AC-001",
                            description="works",
                            verification_method="unit_test",
                        )
                    ],
                )
            ],
            assumptions=[],
            blocking_questions=[],
        )

    async def fake_plan(self, role, goal, requirements_json, repo_map_json):
        _ = self, role, requirements_json, repo_map_json
        seen["plan"].append(goal)
        return PlanOutput(
            id="PLAN",
            rationale="rationale",
            tasks=[
                Task(
                    id="TASK-001",
                    title="Task",
                    description="desc",
                    requirement_ids=["REQ-001"],
                    acceptance_criterion_ids=["AC-001"],
                    planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                )
            ],
        )

    async def fake_critique(self, role, target_plan_json, requirements_json):
        _ = self, role, target_plan_json, requirements_json
        return CritiqueOutput(findings=[])

    async def fake_rebuttal(self, role, original_plan_json, findings_json):
        _ = self, role, original_plan_json, findings_json
        return RebuttalOutput(rebuttals=[])

    async def fake_arbitrate(
        self,
        goal,
        requirements_json,
        plan_a_json,
        plan_b_json,
        critique_a_json,
        critique_b_json,
        rebuttal_a_json,
        rebuttal_b_json,
    ):
        _ = self, requirements_json, plan_a_json, plan_b_json, critique_a_json, critique_b_json, rebuttal_a_json, rebuttal_b_json
        seen["arbiter"].append(goal)
        return ArbiterDecision(
            accepted_finding_ids=[],
            rejected_finding_ids=[],
            final_requirements=[
                Requirement(
                    id="REQ-001",
                    title="Requirement",
                    description="desc",
                    priority="high",
                    source="user",
                    acceptance_criteria=[
                        AcceptanceCriterion(
                            id="AC-001",
                            description="works",
                            verification_method="unit_test",
                        )
                    ],
                )
            ],
            final_tasks=[
                Task(
                    id="TASK-001",
                    title="Task",
                    description="desc",
                    requirement_ids=["REQ-001"],
                    acceptance_criterion_ids=["AC-001"],
                    planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                )
            ],
        )

    monkeypatch.setattr("devcouncil.planning.prompt_enhancer_service.PromptEnhancerService.enhance_prompt", fake_enhance)
    monkeypatch.setattr("devcouncil.planning.spec_service.SpecService.generate_spec", fake_spec)
    monkeypatch.setattr("devcouncil.planning.plan_service.PlanService.generate_plan", fake_plan)
    monkeypatch.setattr("devcouncil.planning.critique_service.CritiqueService.generate_critique", fake_critique)
    monkeypatch.setattr("devcouncil.planning.critique_service.CritiqueService.generate_rebuttal", fake_rebuttal)
    monkeypatch.setattr("devcouncil.planning.arbiter_service.ArbiterService.arbitrate", fake_arbitrate)

    result = runner.invoke(app, ["plan", "Add login", "--dry-run"])

    assert result.exit_code == 0
    assert "Enhanced Planning Prompt" in seen["spec"][0]
    assert "Enhanced for this codebase: Add login" in seen["spec"][0]
    assert seen["plan"] == [seen["spec"][0], seen["spec"][0]]
    assert seen["arbiter"] == [seen["spec"][0]]
    artifacts = list((tmp_path / ".devcouncil" / "runs").glob("*/prompt_enhancement.json"))
    enhancement = json.loads(artifacts[0].read_text(encoding="utf-8"))
    assert enhancement["original_goal"] == "Add login"
    assert enhancement["codebase_context"] == ["src/app.py"]


def test_cli_plan_dry_run_tolerates_legacy_config_missing_planning_roles(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    (tmp_path / ".devcouncil" / "config.yaml").write_text(
        "\n".join([
            "models:",
            "  provider: openrouter",
            "  roles: {}",
            "",
        ]),
        encoding="utf-8",
    )

    result = runner.invoke(app, ["plan", "Add audit log", "--dry-run"])

    assert result.exit_code == 0
    assert "Planning complete" in result.output
    artifacts = list((tmp_path / ".devcouncil" / "runs").glob("*/prompt_enhancement.json"))
    assert len(artifacts) == 1
    enhancement = json.loads(artifacts[0].read_text(encoding="utf-8"))
    assert enhancement["original_goal"] == "Add audit log"


def test_cli_plan_resets_reused_task_status_to_planned(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Old task",
            description="Previously verified task",
            planned_files=[PlannedFile(path="old.py", reason="old", allowed_change="modify")],
            status="verified",
        ))

    result = runner.invoke(app, ["plan", "Add password reset", "--dry-run", "--persist"])

    assert result.exit_code == 0
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id("TASK-001")
        assert task is not None
        assert task.status == "planned"


def test_cli_plan_replaces_stale_active_plan_artifacts(tmp_path, monkeypatch):
    from devcouncil.domain.evidence import CommandResult
    from devcouncil.domain.gap import Gap
    from devcouncil.domain.requirement import Requirement
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import (
        EvidenceRepository,
        GapRepository,
        RequirementRepository,
        TaskRepository,
    )

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-OLD",
            title="Old requirement",
            description="old",
            priority="low",
            source="user",
        ))
        TaskRepository(session).save(Task(
            id="TASK-OLD",
            title="Old task",
            description="old",
            planned_files=[PlannedFile(path="old.py", reason="old", allowed_change="modify")],
            status="verified",
        ))
        GapRepository(session).save(Gap(
            id="GAP-OLD",
            severity="high",
            gap_type="missing_test",
            task_id="TASK-OLD",
            description="old gap",
            recommended_fix="old fix",
            blocking=True,
        ))
        EvidenceRepository(session).save_command_result(
            "TASK-OLD",
            CommandResult(
                command="pytest",
                exit_code=0,
                stdout_path=".devcouncil/logs/old.out",
                stderr_path=".devcouncil/logs/old.err",
                summary="old",
            ),
        )

    result = runner.invoke(app, ["plan", "Add password reset", "--dry-run", "--persist"])

    assert result.exit_code == 0
    with db.get_session() as session:
        assert [req.id for req in RequirementRepository(session).get_all()] == ["REQ-001"]
        assert [task.id for task in TaskRepository(session).get_all()] == ["TASK-001"]
        assert GapRepository(session).get_all() == []
        assert EvidenceRepository(session).get_all() == []


def test_cli_plan_does_not_persist_failed_gate_tasks(tmp_path, monkeypatch):
    from devcouncil.domain.gap import Gap
    from devcouncil.gating.policy import GateResult
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import GapRepository, RequirementRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    def fail_plan_gate(self, requirements, tasks, assumptions=None, findings=None, blocking_questions=None):
        _ = self, requirements, tasks, assumptions, findings, blocking_questions
        return GateResult(
            passed=False,
            gaps=[
                Gap(
                    id="GAP-PLAN-TEST",
                    severity="high",
                    gap_type="requirement_not_planned",
                    description="forced failure",
                    recommended_fix="fix the plan",
                    blocking=True,
                )
            ],
        )

    monkeypatch.setattr("devcouncil.cli.commands.plan.GatePolicy.check_plan_approval", fail_plan_gate)

    result = runner.invoke(app, ["plan", "Add password reset", "--dry-run", "--persist"])

    assert result.exit_code == 0
    assert "failed gates" in result.output
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        assert RequirementRepository(session).get_all() == []
        assert TaskRepository(session).get_all() == []
        assert [gap.id for gap in GapRepository(session).get_all()] == ["GAP-PLAN-TEST"]


def test_cli_go_plans_runs_tasks_and_reports(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(tmp_path)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001"]

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        db = get_db(tmp_path)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "codex", "--json-report"])

    assert result.exit_code == 0
    assert "Planning goal" in result.output
    assert "TASK-001" in result.output
    assert '"verdict": "passed"' in result.output


def test_cli_go_expands_github_reference_goal_before_planning(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    captured = {}

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        captured["goal"] = goal
        return []

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr(
        "devcouncil.cli.commands.go.resolve_goal_intent",
        lambda goal, root: ("Implement issue #142: Add password reset\n\nfull body", "Pulled intent from issue #142 via gh."),
    )

    result = runner.invoke(app, ["go", "#142", "--executor", "codex"])

    # Planning ran against the expanded issue text, not the bare "#142".
    assert captured["goal"].startswith("Implement issue #142: Add password reset")
    assert "Pulled intent from issue #142" in result.output


def test_cli_go_quick_flag_is_forwarded_to_planning(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    captured = {}

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **kwargs):
        captured["quick"] = kwargs.get("quick")
        return []

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)

    runner.invoke(app, ["go", "Add a feature", "--executor", "codex", "--quick"])
    assert captured["quick"] is True

    runner.invoke(app, ["go", "Add a feature", "--executor", "codex"])
    assert captured["quick"] is False


def test_cli_e2e_alias_plans_runs_tasks_and_reports(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001"]

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["e2e", "Add a feature", "--executor", "codex", "--json-report"])

    assert result.exit_code == 0
    assert "Planning goal" in result.output
    assert "TASK-001" in result.output
    assert '"verdict": "passed"' in result.output


def test_cli_e2e_uses_configured_default_executor_when_omitted(tmp_path, monkeypatch):
    import yaml

    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    raw_config["execution"]["default_executor"] = "gemini-cli"
    config_path.write_text(yaml.safe_dump(raw_config), encoding="utf-8")

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001"]

    seen = {}

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        seen["executor"] = executor
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["e2e", "Add a feature", "--json-report"])

    assert result.exit_code == 0
    assert seen["executor"] == "gemini"


def test_cli_e2e_without_executor_rejects_manual_default(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr("shutil.which", lambda _command: None)

    result = runner.invoke(app, ["e2e", "Add a feature", "--agent"])

    assert result.exit_code == 2
    assert "requires an automated executor" in result.output
    assert "execution.default_executor" in result.output
    assert "coding CLI on PATH" in result.output


def test_cli_e2e_writes_machine_readable_report_file(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001"]

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, [
        "e2e",
        "Add a feature",
        "--executor",
        "codex",
        "--json",
        "--report-file",
        ".devcouncil/reports/latest.json",
    ])

    assert result.exit_code == 0
    report_path = tmp_path / ".devcouncil" / "reports" / "latest.json"
    assert "Final report written to" in result.output
    payload = json.loads(report_path.read_text(encoding="utf-8"))
    assert payload["verdict"] == "passed"


def test_cli_e2e_agent_mode_writes_default_json_report(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001"]

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["e2e", "Add a feature", "--executor", "codex", "--agent"])

    assert result.exit_code == 0
    report_path = tmp_path / ".devcouncil" / "reports" / "latest.json"
    assert report_path.exists()
    payload = json.loads(report_path.read_text(encoding="utf-8"))
    assert payload["verdict"] == "passed"
    assert '"verdict": "passed"' in result.output


def test_cli_go_supports_project_root_from_other_directory(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    project = tmp_path / "project"
    other = tmp_path / "other"
    project.mkdir()
    other.mkdir()
    monkeypatch.chdir(other)

    seen = {}

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=Path("."), **_kwargs):
        seen["plan_root"] = project_root
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001"]

    def fake_run(task_id, executor="manual", project_root=Path("."), **kwargs):
        seen["run_root"] = project_root
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "codex", "--project-root", str(project), "--json-report"])

    assert result.exit_code == 0
    assert seen["plan_root"] == project.resolve()
    assert seen["run_root"] == project.resolve()
    assert (project / ".devcouncil" / "state.sqlite").exists()


def test_cli_go_only_runs_tasks_from_current_plan(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-OLD",
            title="Old task",
            description="Must not run",
            planned_files=[PlannedFile(path="src/old.py", reason="old", allowed_change="modify")],
            allowed_commands=["python --version"],
            expected_tests=["python --version"],
        ))

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-NEW",
                title=goal,
                description="Current plan task",
                planned_files=[PlannedFile(path="src/new.py", reason="new", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-NEW"]

    ran = []

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        ran.append(task_id)
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Add new feature", "--executor", "codex", "--json-report"])

    assert result.exit_code == 0
    assert ran == ["TASK-NEW"]


def test_cli_go_fails_when_plan_returns_no_task_ids(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        return []

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)

    result = runner.invoke(app, ["go", "No runnable tasks", "--executor", "codex"])

    assert result.exit_code == 1
    assert "Planning did not produce any approved tasks" in result.output


def test_cli_go_fails_when_planned_task_id_is_not_persisted(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        _ = goal, requirements_only, dry_run, persist, project_root
        return ["TASK-MISSING"]

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)

    result = runner.invoke(app, ["go", "Missing task", "--executor", "codex"])

    assert result.exit_code == 1
    assert "not persisted: TASK-MISSING" in result.output


def test_cli_go_deduplicates_planned_task_ids(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001", "TASK-001"]

    ran = []

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        ran.append(task_id)
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Duplicate task", "--executor", "codex", "--json-report"])

    assert result.exit_code == 0
    assert ran == ["TASK-001"]


def test_cli_go_fails_when_executor_leaves_task_unverified(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Executor will not verify this",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python -c \"import sys; sys.exit(1)\""],
                expected_tests=["python -c \"import sys; sys.exit(1)\""],
            ))
        return ["TASK-001"]

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        _ = task_id, executor, project_root

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "codex", "--json-report"])

    assert result.exit_code == 1
    # After the final reconciliation pass the task's real verification (a failing
    # command) keeps it unfinished; the exact status is blocked rather than planned.
    assert "Unfinished task(s): TASK-001" in result.output


def test_cli_go_fails_when_all_planned_tasks_were_precompleted(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import StateRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Already verified",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
                status="verified",
            ))
        return ["TASK-001"]

    ran = []

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        ran.append(task_id)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "codex", "--json-report"])

    assert result.exit_code == 1
    assert ran == []
    assert "all planned tasks were already completed before execution" in result.output
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        assert StateRepository(session).get_state().current_phase != "PROJECT_DONE"


def test_cli_go_continue_on_blocked_runs_later_tasks_but_fails_project(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import StateRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            for task_id in ("TASK-001", "TASK-002"):
                # TASK-001 has a genuinely failing check so it stays blocked through
                # the final reconciliation pass; TASK-002's check passes.
                check = (
                    'python -c "import sys; sys.exit(1)"'
                    if task_id == "TASK-001"
                    else "python --version"
                )
                repo.save(Task(
                    id=task_id,
                    title=f"{goal} {task_id}",
                    description="Generated task",
                    planned_files=[PlannedFile(path=f"src/{task_id}.py", reason="logic", allowed_change="modify")],
                    allowed_commands=[check],
                    expected_tests=[check],
                ))
        return ["TASK-001", "TASK-002"]

    ran = []

    def fake_run(task_id, executor="manual", project_root=tmp_path, **kwargs):
        ran.append(task_id)
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "blocked" if task_id == "TASK-001" else "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "codex", "--continue-on-blocked", "--json-report"])

    assert result.exit_code == 1
    assert ran == ["TASK-001", "TASK-002"]
    assert "Unfinished task(s): TASK-001 (blocked)" in result.output
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        assert StateRepository(session).get_state().current_phase != "PROJECT_DONE"


def test_cli_go_rejects_manual_executor(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "manual"])

    assert result.exit_code == 2
    assert "`dev go` requires an automated executor" in result.output
    assert "requires an automated executor" in result.output


def test_cli_e2e_rejects_manual_executor_with_e2e_command_name(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["e2e", "Add a feature", "--executor", "manual"])

    assert result.exit_code == 2
    assert "`dev e2e` requires an automated executor" in result.output


def test_cli_go_rejects_unknown_executor_before_planning(tmp_path, monkeypatch):
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    called = False

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        nonlocal called
        called = True
        return []

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "experimental"])

    assert result.exit_code == 2
    assert called is False
    assert "Unsupported executor" in result.output
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        assert TaskRepository(session).get_all() == []


def test_cli_e2e_rejects_unknown_executor_with_e2e_command_name(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["e2e", "Add a feature", "--executor", "experimental"])

    assert result.exit_code == 2
    assert "Unsupported executor for `dev e2e`" in result.output


def test_cli_go_supports_custom_agent_registry_and_profile(tmp_path, monkeypatch):
    import yaml

    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "input_mode": "stdin",
    }
    config_path.write_text(yaml.safe_dump(raw), encoding="utf-8")
    seen = {}

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        _ = requirements_only, dry_run, persist
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title=goal,
                description="Generated task",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python --version"],
                expected_tests=["python --version"],
            ))
        return ["TASK-001"]

    def fake_run(task_id, executor="manual", profile=None, project_root=tmp_path, **kwargs):
        seen["executor"] = executor
        seen["profile"] = profile
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Add a feature", "--executor", "custombot", "--profile", "prod"])

    assert result.exit_code == 0
    assert seen["executor"] == "custombot"
    assert seen["profile"] == "prod"


def test_cli_integrate_prints_coding_cli_setup_commands(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["integrate", "all"])

    assert result.exit_code == 0
    assert "codex mcp add devcouncil" in result.output
    assert "gemini mcp add --scope project" in result.output
    assert "claude mcp add --scope local" in result.output
    assert ".cursor" in result.output
    assert '"mcpServers":{"devcouncil"' in result.output
    assert "OpenCode" in result.output
    assert '"mcp":{"devcouncil"' in result.output
    assert "Google Antigravity CLI" in result.output
    assert ".agents" in result.output
    assert "Warp / Oz" in result.output
    assert "Native hook config preview" in result.output
    assert "DEVCOUNCIL_PROJECT_ROOT=" in result.output


def test_cli_integrate_cursor_preview_uses_project_mcp_json(tmp_path):
    from devcouncil.cli.commands import integrate

    config = integrate._cursor_mcp_config(tmp_path)

    server = config["mcpServers"]["devcouncil"]
    assert server["type"] == "stdio"
    assert server["command"] == "devcouncil"
    assert server["args"] == ["mcp-server"]
    assert server["env"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)


def test_cli_integrate_cursor_writes_project_mcp_json(tmp_path, monkeypatch):
    import yaml

    monkeypatch.setattr("shutil.which", lambda command: None)

    result = runner.invoke(app, ["integrate", "cursor", "--apply", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    config_path = tmp_path / ".cursor" / "mcp.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))
    server = data["mcpServers"]["devcouncil"]
    assert server["type"] == "stdio"
    assert server["command"] == "devcouncil"
    assert server["args"] == ["mcp-server"]
    assert server["env"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    cursor = raw_config["integrations"]["cursor"]
    assert cursor["enabled"] is True
    assert cursor["config_path"].replace("\\", "/") == ".cursor/mcp.json"


def test_cli_integrate_hooks_previews_native_hook_files(tmp_path):
    result = runner.invoke(app, ["integrate", "hooks", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert ".codex" in result.output
    assert ".gemini" in result.output
    assert ".claude" in result.output
    assert ".cursor" in result.output
    assert "opencode_devcouncil_plugin.mjs" in result.output
    assert "Preview only" in result.output


def test_cli_integrate_hooks_apply_writes_native_hook_files(tmp_path):
    result = runner.invoke(app, ["integrate", "hooks", "--apply", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    codex_hooks = json.loads((tmp_path / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    gemini_settings = json.loads((tmp_path / ".gemini" / "settings.json").read_text(encoding="utf-8"))
    claude_settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    cursor_hooks = json.loads((tmp_path / ".cursor" / "hooks.json").read_text(encoding="utf-8"))
    assert "PreToolUse" in codex_hooks["hooks"]
    assert "BeforeTool" in gemini_settings["hooks"]
    assert "PreToolUse" in claude_settings["hooks"]
    assert "Stop" in claude_settings["hooks"]
    assert "agent-response" in json.dumps(claude_settings)
    assert "preToolUse" in cursor_hooks["hooks"]
    assert "postToolUse" in cursor_hooks["hooks"]
    assert "pre-tool-use" in json.dumps(cursor_hooks)
    assert (tmp_path / ".devcouncil" / "integrations" / "opencode_devcouncil_plugin.mjs").exists()
    opencode_config = json.loads((tmp_path / "opencode.json").read_text(encoding="utf-8"))
    assert "./.devcouncil/integrations/opencode_devcouncil_plugin.mjs" in opencode_config["plugin"]
    assert "codex_hooks = true" in (tmp_path / ".codex" / "config.toml").read_text(encoding="utf-8")


def test_cli_integrate_check_strict_fails_when_optional_cli_missing(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    result = runner.invoke(app, ["integrate", "check", "--strict", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "Missing" not in result.output or "FAIL" in result.output


def test_cli_go_auto_detects_coding_cli_when_default_is_manual(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    def fake_which(command):
        return "/usr/bin/codex" if command == "codex" else None

    monkeypatch.setattr("shutil.which", fake_which)

    async def fake_plan_flow(goal, requirements_only=False, dry_run=False, persist=True, project_root=tmp_path, **_kwargs):
        _ = goal, requirements_only, dry_run, persist
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            TaskRepository(session).save(Task(
                id="TASK-001",
                title="Auto executor",
                description="Detect codex",
                planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
                allowed_commands=["python -m pytest"],
                expected_tests=["python -m pytest"],
            ))
        return ["TASK-001"]

    seen = {}

    def fake_run(task_id, executor="manual", profile=None, stream=False, project_root=tmp_path, **kwargs):
        seen["executor"] = executor
        seen["stream"] = stream
        db = get_db(project_root)
        assert db is not None
        with db.get_session() as session:
            repo = TaskRepository(session)
            task = repo.get_by_id(task_id)
            assert task is not None
            task.status = "verified"
            repo.save(task)

    monkeypatch.setattr("devcouncil.cli.commands.go.plan_command.run_plan_flow", fake_plan_flow)
    monkeypatch.setattr("devcouncil.cli.commands.go.run_command.run", fake_run)

    result = runner.invoke(app, ["go", "Ship feature", "--json-report"])

    assert result.exit_code == 0
    assert seen.get("executor") == "codex"
    assert "Using automated executor" in result.output


def test_cli_integrate_check_reports_hook_wiring(tmp_path):
    (tmp_path / ".devcouncil").mkdir()
    result = runner.invoke(
        app,
        ["integrate", "hooks", "--apply", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0

    check = runner.invoke(app, ["integrate", "check", "--project-root", str(tmp_path)])
    assert check.exit_code == 0
    assert "Cursor hooks" in check.output
    assert "OpenCode hook plugin" in check.output
    assert "Bundled OpenCode hook plugin" in check.output
    assert "Ready." in check.output


def test_cli_integrate_aider_apply_records_executor(tmp_path):
    import yaml

    result = runner.invoke(app, ["integrate", "aider", "--apply", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["integrations"]["aider"]["enabled"] is True


def test_cli_integrate_all_apply_writes_native_hook_files_when_clients_missing(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda command: None)

    result = runner.invoke(app, ["integrate", "all", "--apply", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "Skipping optional integration" in result.output
    assert (tmp_path / ".codex" / "hooks.json").exists()
    assert (tmp_path / ".gemini" / "settings.json").exists()
    assert (tmp_path / ".claude" / "settings.local.json").exists()


def test_cli_integrations_doctor_reports_optional_tools(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["integrations", "doctor"])

    assert result.exit_code == 0
    assert "code-review-graph" in result.output
    assert "Agent Flow" in result.output
    assert "Claude Code" in result.output
    assert "Cursor" in result.output
    assert "OpenCode" in result.output
    assert "Google Antigravity CLI" in result.output
    assert "Aider" in result.output


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


def test_cli_watch_review_persists_critique_card(tmp_path, monkeypatch):
    from devcouncil.telemetry.traces import read_trace_events

    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["verdict"] == "Concerns"
    assert payload["duplicate"] is False
    assert (tmp_path / ".devcouncil" / "live" / "cards").exists()
    events = list(read_trace_events(tmp_path))
    assert events[-1].type == "live_review_card_saved"
    assert events[-1].details["card_id"] == payload["id"]


def test_cli_watch_review_can_scope_card_to_task(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(app, [
        "watch",
        "review",
        "--transcript",
        str(transcript),
        "--task-id",
        "TASK-001",
        "--json",
    ])

    assert result.exit_code == 0
    assert json.loads(result.output)["task_id"] == "TASK-001"


def test_cli_watch_review_defaults_to_single_running_task(tmp_path, monkeypatch):
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Running",
            description="Active task",
            status="running",
        ))
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--json"])

    assert result.exit_code == 0
    assert json.loads(result.output)["task_id"] == "TASK-001"


def test_cli_watch_review_is_idempotent_for_same_turn(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )

    first = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--json"])
    second = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--json"])

    assert first.exit_code == 0
    assert second.exit_code == 0
    first_payload = json.loads(first.output)
    second_payload = json.loads(second.output)
    assert first_payload["id"] == second_payload["id"]
    assert second_payload["duplicate"] is True
    assert len(list((tmp_path / ".devcouncil" / "live" / "cards").glob("*.json"))) == 1


def test_cli_watch_review_force_preserves_resolved_status(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    review_result = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--json"])
    card_id = json.loads(review_result.output)["id"]
    assert runner.invoke(app, ["watch", "resolve", card_id, "--status", "resolved"]).exit_code == 0

    force_result = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--force", "--json"])

    assert force_result.exit_code == 0
    payload = json.loads(force_result.output)
    assert payload["duplicate"] is False
    saved = json.loads((tmp_path / ".devcouncil" / "live" / "cards" / f"{card_id}.json").read_text(encoding="utf-8"))
    assert saved["status"] == "resolved"


def test_cli_watch_review_latest_discovered_session(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    session_dir = tmp_path / ".devcouncil" / "live" / "generic"
    session_dir.mkdir(parents=True)
    older = session_dir / "older.jsonl"
    newer = session_dir / "newer.jsonl"
    older.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )
    newer.write_text(
        json.dumps({"role": "assistant", "id": "A-2", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(app, ["watch", "review", "--client", "generic", "--latest", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["turn_id"] == "A-2"
    assert payload["verdict"] == "Concerns"


def test_cli_watch_review_named_discovered_session(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    session_dir = tmp_path / ".devcouncil" / "live" / "generic"
    session_dir.mkdir(parents=True)
    transcript = session_dir / "chosen.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(app, ["watch", "review", "--client", "generic", "--session", "chosen", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["session_id"] == "chosen"
    assert payload["verdict"] == "Approved"


def test_cli_watch_review_requires_transcript_or_session(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["watch", "review", "--json"])

    assert result.exit_code == 2
    payload = json.loads(result.output)
    assert "No transcript selected" in payload["error"]


def test_cli_watch_follow_once_reviews_latest_turn(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(app, ["watch", "follow", "--transcript", str(transcript), "--once"])

    assert result.exit_code == 0
    assert "Critique Card" in result.output
    assert "Approved" in result.output


def test_cli_watch_follow_once_latest_session(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    session_dir = tmp_path / ".devcouncil" / "live" / "generic"
    session_dir.mkdir(parents=True)
    transcript = session_dir / "latest.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )

    result = runner.invoke(app, ["watch", "follow", "--client", "generic", "--latest", "--once"])

    assert result.exit_code == 0
    assert "latest.jsonl" in result.output
    assert "Approved" in result.output


def test_cli_hook_agent_response_writes_signal(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input='{"transcript_path":"session.jsonl"}',
    )

    assert result.exit_code == 0
    signals = list((tmp_path / ".devcouncil" / "live" / "signals").glob("claude-*.json"))
    assert signals


def test_cli_hook_agent_response_defaults_to_single_running_task(tmp_path, monkeypatch):
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Running",
            description="Active task",
            status="running",
        ))

    result = runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input='{"transcript_path":"session.jsonl"}',
    )

    assert result.exit_code == 0
    signal = json.loads(next((tmp_path / ".devcouncil" / "live" / "signals").glob("claude-*.json")).read_text())
    assert signal["task_id"] == "TASK-001"
    assert signal["review_command"] == "dev watch review --client claude --transcript session.jsonl --task-id TASK-001"


def test_cli_watch_pending_reviews_signal_and_moves_it(tmp_path, monkeypatch):
    from devcouncil.telemetry.traces import read_trace_events

    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )
    result = runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input=json.dumps({"transcript_path": str(transcript)}),
    )
    assert result.exit_code == 0

    pending_result = runner.invoke(app, ["watch", "pending", "--json"])

    assert pending_result.exit_code == 0
    payload = json.loads(pending_result.output)
    assert payload["reviewed"][0]["card"]["verdict"] == "Concerns"
    assert list((tmp_path / ".devcouncil" / "live" / "signals" / "processed").glob("claude-*.json"))
    assert not list((tmp_path / ".devcouncil" / "live" / "signals").glob("claude-*.json"))
    event_types = [event.type for event in read_trace_events(tmp_path)]
    assert "live_review_card_saved" in event_types
    assert "live_review_signal_processed" in event_types


def test_cli_watch_pending_defaults_to_single_running_task(tmp_path, monkeypatch):
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Running",
            description="Active task",
            status="running",
        ))
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input=json.dumps({"transcript_path": str(transcript), "task_id": ""}),
    ).exit_code == 0

    pending_result = runner.invoke(app, ["watch", "pending", "--json"])

    assert pending_result.exit_code == 0
    payload = json.loads(pending_result.output)
    assert payload["reviewed"][0]["card"]["task_id"] == "TASK-001"


def test_cli_watch_pending_task_id_overrides_signal_scope(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Done, implemented it."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input=json.dumps({"transcript_path": str(transcript), "task_id": "TASK-OLD"}),
    ).exit_code == 0

    pending_result = runner.invoke(app, ["watch", "pending", "--task-id", "TASK-NEW", "--json"])

    assert pending_result.exit_code == 0
    payload = json.loads(pending_result.output)
    assert payload["reviewed"][0]["card"]["task_id"] == "TASK-NEW"


def test_cli_watch_resolve_updates_card_status(tmp_path, monkeypatch):
    from devcouncil.telemetry.traces import read_trace_events

    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    review_result = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--json"])
    assert review_result.exit_code == 0
    card_id = json.loads(review_result.output)["id"]

    resolve_result = runner.invoke(app, ["watch", "resolve", card_id, "--status", "ignored", "--json"])

    assert resolve_result.exit_code == 0
    payload = json.loads(resolve_result.output)
    assert payload["card"]["status"] == "ignored"
    events = list(read_trace_events(tmp_path))
    assert events[-1].type == "live_review_card_status_updated"
    assert events[-1].details["card_id"] == card_id
    assert events[-1].details["status"] == "ignored"


def test_cli_watch_cards_filters_by_task_status_verdict_and_client(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    first = tmp_path / "first.jsonl"
    first.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    second = tmp_path / "second.jsonl"
    second.write_text(
        json.dumps({"role": "assistant", "id": "A-2", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )
    other = tmp_path / "other.jsonl"
    other.write_text(
        json.dumps({"role": "assistant", "id": "A-3", "content": "Force push to main."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(app, [
        "watch",
        "review",
        "--client",
        "claude",
        "--transcript",
        str(first),
        "--task-id",
        "TASK-001",
    ]).exit_code == 0
    assert runner.invoke(app, [
        "watch",
        "review",
        "--client",
        "claude",
        "--transcript",
        str(second),
        "--task-id",
        "TASK-001",
    ]).exit_code == 0
    other_result = runner.invoke(app, [
        "watch",
        "review",
        "--client",
        "gemini",
        "--transcript",
        str(other),
        "--task-id",
        "TASK-OTHER",
        "--json",
    ])
    assert other_result.exit_code == 0
    assert runner.invoke(
        app,
        ["watch", "resolve", json.loads(other_result.output)["id"], "--status", "ignored"],
    ).exit_code == 0

    result = runner.invoke(app, [
        "watch",
        "cards",
        "--task-id",
        "TASK-001",
        "--status",
        "open",
        "--verdict",
        "critical",
        "--client",
        "claude",
        "--json",
    ])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert len(payload["cards"]) == 1
    assert payload["cards"][0]["task_id"] == "TASK-001"
    assert payload["cards"][0]["status"] == "open"
    assert payload["cards"][0]["verdict"] == "Critical Issues"
    assert payload["cards"][0]["client"] == "claude"
    assert payload["filters"] == {
        "task_id": "TASK-001",
        "status": "open",
        "verdict": "critical",
        "client": "claude",
    }
    assert payload["limit"] == 10
    assert payload["total"] == 1


def test_cli_watch_cards_rejects_invalid_filters(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    bad_status = runner.invoke(app, ["watch", "cards", "--status", "closed", "--json"])
    bad_verdict = runner.invoke(app, ["watch", "cards", "--verdict", "danger", "--json"])
    bad_limit = runner.invoke(app, ["watch", "cards", "--limit", "0", "--json"])

    assert bad_status.exit_code == 2
    assert json.loads(bad_status.output)["error"] == "--status must be open, resolved, or ignored."
    assert bad_verdict.exit_code == 2
    assert json.loads(bad_verdict.output)["error"] == "--verdict must be approved, concerns, or critical."
    assert bad_limit.exit_code == 2
    assert json.loads(bad_limit.output)["error"] == "--limit must be greater than 0."


def test_cli_watch_repair_outputs_prompt_for_card(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    review_result = runner.invoke(app, ["watch", "review", "--transcript", str(transcript), "--json"])
    assert review_result.exit_code == 0
    card_id = json.loads(review_result.output)["id"]

    repair_result = runner.invoke(app, ["watch", "repair", card_id, "--json"])

    assert repair_result.exit_code == 0
    payload = json.loads(repair_result.output)
    assert payload["card"]["id"] == card_id
    assert "Repair Live Review Card" in payload["prompt"]
    assert f"dev watch resolve {card_id} --status resolved" in payload["prompt"]


def test_cli_watch_repair_all_outputs_blocking_cards_in_scope(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    first = tmp_path / "first.jsonl"
    first.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    second = tmp_path / "second.jsonl"
    second.write_text(
        json.dumps({"role": "assistant", "id": "A-2", "content": "Ignore failing tests."}) + "\n",
        encoding="utf-8",
    )
    other = tmp_path / "other.jsonl"
    other.write_text(
        json.dumps({"role": "assistant", "id": "A-3", "content": "Force push to main."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(app, ["watch", "review", "--transcript", str(first), "--task-id", "TASK-001"]).exit_code == 0
    assert runner.invoke(app, ["watch", "review", "--transcript", str(second)]).exit_code == 0
    assert runner.invoke(app, ["watch", "review", "--transcript", str(other), "--task-id", "TASK-OTHER"]).exit_code == 0

    result = runner.invoke(app, ["watch", "repair-all", "--task-id", "TASK-001", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["scope_task_id"] == "TASK-001"
    assert len(payload["cards"]) == 2
    assert "Repair Blocking Live Review Cards" in payload["prompt"]
    assert all(card["task_id"] in {"TASK-001", None} for card in payload["cards"])


def test_cli_watch_status_summarizes_cards_signals_and_scope(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    critical = tmp_path / "critical.jsonl"
    critical.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    approved = tmp_path / "approved.jsonl"
    approved.write_text(
        json.dumps({"role": "assistant", "id": "A-2", "content": "Implemented and verified with pytest."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(app, [
        "watch",
        "review",
        "--transcript",
        str(critical),
        "--task-id",
        "TASK-001",
    ]).exit_code == 0
    assert runner.invoke(app, ["watch", "review", "--transcript", str(approved)]).exit_code == 0
    assert runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input='{"transcript_path":"session.jsonl"}',
    ).exit_code == 0

    status_result = runner.invoke(app, ["watch", "status", "--task-id", "TASK-001", "--json"])

    assert status_result.exit_code == 0
    payload = json.loads(status_result.output)
    assert payload["pending_signals"] == 1
    assert payload["pending_signal_items"][0]["transcript_path"] == "session.jsonl"
    assert payload["pending_signal_items"][0]["review_command"] == "dev watch review --client claude --transcript session.jsonl"
    assert payload["cards"]["total"] == 2
    assert payload["cards"]["critical_open"] == 1
    assert len(payload["blocking_cards"]) == 1
    assert payload["blocking_cards"][0]["task_id"] == "TASK-001"


def test_cli_watch_status_prints_pending_signal_commands(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    result = runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input='{"transcript_path":"session.jsonl","task_id":"TASK-001"}',
    )
    assert result.exit_code == 0

    status_result = runner.invoke(app, ["watch", "status"])

    assert status_result.exit_code == 0
    assert "Pending Agent Responses" in status_result.output
    assert "dev watch review --client claude --transcript session.jsonl --task-id TASK-001" in status_result.output


def test_cli_watch_status_excludes_other_task_blockers(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    critical = tmp_path / "critical.jsonl"
    critical.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(app, [
        "watch",
        "review",
        "--transcript",
        str(critical),
        "--task-id",
        "TASK-OTHER",
    ]).exit_code == 0

    status_result = runner.invoke(app, ["watch", "status", "--task-id", "TASK-001", "--json"])

    assert status_result.exit_code == 0
    payload = json.loads(status_result.output)
    assert payload["cards"]["critical_open"] == 1
    assert payload["blocking_cards"] == []


def test_cli_watch_signals_lists_pending_signal(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    result = runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input='{"transcript_path":"session.jsonl"}',
    )
    assert result.exit_code == 0

    signals_result = runner.invoke(app, ["watch", "signals", "--json"])

    assert signals_result.exit_code == 0
    payload = json.loads(signals_result.output)
    assert payload["signals"][0]["transcript_path"] == "session.jsonl"


def test_cli_hook_pre_tool_use_accepts_stdin_payload(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(
        app,
        ["hook", "pre-tool-use"],
        input='{"name":"Bash","arguments":{"command":"git commit --no-verify -m test"}}',
    )

    assert result.exit_code == 2
    assert "Verification bypass" in result.output


def test_cli_hook_pre_tool_use_denies_write_without_running_task(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(
        app,
        ["hook", "pre-tool-use"],
        input='{"name":"Write","arguments":{"path":"src/app.py"}}',
    )

    assert result.exit_code == 2
    assert "No running DevCouncil task" in result.output


def test_cli_hook_pre_tool_use_uses_project_root_env(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    project = tmp_path / "project"
    other = tmp_path / "other"
    project.mkdir()
    other.mkdir()
    monkeypatch.chdir(project)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(project)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Task",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            status="running",
        ))

    monkeypatch.chdir(other)
    result = runner.invoke(
        app,
        ["hook", "pre-tool-use"],
        input='{"name":"Write","arguments":{"path":"src/app.py"}}',
        env={"DEVCOUNCIL_PROJECT_ROOT": str(project)},
    )

    assert result.exit_code == 0


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


def test_cli_integrate_supports_claude_scope(tmp_path):
    result = runner.invoke(
        app,
        [
            "integrate",
            "claude",
            "--scope",
            "project",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 0
    assert "claude mcp add --scope project" in result.output
    assert f"DEVCOUNCIL_PROJECT_ROOT={tmp_path}" in result.output


def test_cli_integrate_supports_cursor_mcp_json(tmp_path):
    result = runner.invoke(
        app,
        [
            "integrate",
            "cursor",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 0
    assert ".cursor" in result.output
    assert '"mcpServers":{"devcouncil"' in result.output
    assert '"type":"stdio"' in result.output
    assert '"DEVCOUNCIL_PROJECT_ROOT":"' in result.output


def test_cli_integrate_warp_writes_mcp_config(tmp_path):
    import yaml

    result = runner.invoke(app, ["integrate", "warp", "--apply", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    config_path = tmp_path / ".devcouncil" / "integrations" / "warp-mcp.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))
    server = data["devcouncil"]
    assert server["command"] == "devcouncil"
    assert server["args"] == ["mcp-server"]
    assert server["env"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)
    assert "working_directory" not in server
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    warp = raw_config["integrations"]["warp"]
    assert warp["enabled"] is True
    assert warp["command"] == "oz"
    assert warp["run_mode"] == "local"
    assert warp["mcp_config_path"].replace("\\", "/") == ".devcouncil/integrations/warp-mcp.json"


def test_cli_integrate_opencode_writes_project_mcp_config(tmp_path, monkeypatch):
    import yaml

    monkeypatch.setattr("shutil.which", lambda command: f"/usr/bin/{command}")

    result = runner.invoke(app, ["integrate", "opencode", "--apply", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    config_path = tmp_path / "opencode.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))
    server = data["mcp"]["devcouncil"]
    assert server["type"] == "local"
    assert server["command"] == ["devcouncil", "mcp-server"]
    assert server["environment"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)
    assert server["enabled"] is True
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    opencode = raw_config["integrations"]["opencode"]
    assert opencode["enabled"] is True
    assert opencode["config_path"] == "opencode.json"


def test_cli_integrate_antigravity_writes_project_mcp_config(tmp_path, monkeypatch):
    import yaml

    monkeypatch.setattr("shutil.which", lambda command: f"/usr/bin/{command}")

    result = runner.invoke(app, ["integrate", "antigravity", "--apply", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    config_path = tmp_path / ".agents" / "mcp_config.json"
    data = json.loads(config_path.read_text(encoding="utf-8"))
    server = data["mcpServers"]["devcouncil"]
    assert server["command"] == "devcouncil"
    assert server["args"] == ["mcp-server"]
    assert server["env"]["DEVCOUNCIL_PROJECT_ROOT"] == str(tmp_path)
    assert server["cwd"] == str(tmp_path)
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    antigravity = raw_config["integrations"]["antigravity"]
    assert antigravity["enabled"] is True
    assert antigravity["mcp_config_path"].replace("\\", "/") == ".agents/mcp_config.json"


def test_cli_integrate_registers_bring_your_own_cli_executor(tmp_path):
    import yaml

    result = runner.invoke(
        app,
        [
            "integrate",
            "cli-agent",
            "custombot",
            "--command",
            "custombot",
            "--arg",
            "run",
            "--input-mode",
            "prompt-file",
            "--prompt-arg=--prompt-file",
            "--apply",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 0
    raw = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    agent = raw["integrations"]["cli_agents"]["agents"]["custombot"]
    assert agent["command"] == "custombot"
    assert agent["args"] == ["run"]
    assert agent["input_mode"] == "prompt-file"
    assert agent["prompt_arg"] == "--prompt-file"
    assert agent["kind"] == "custom"
    assert agent["default_profile"] == "default"


def test_cli_agents_add_writes_typed_agent_config(tmp_path):
    import yaml

    result = runner.invoke(
        app,
        [
            "agents",
            "add",
            "custombot",
            "--command",
            "custombot",
            "--arg",
            "run",
            "--input-mode",
            "prompt-file",
            "--prompt-arg=--prompt-file",
            "--display-name",
            "OpenCode",
            "--kind",
            "coding-cli",
            "--supports-mcp",
            "--supports-diff-review",
            "--default-profile",
            "prod",
            "--help-arg",
            "--help",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 0
    raw = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    agent = raw["integrations"]["cli_agents"]["agents"]["custombot"]
    assert agent["command"] == "custombot"
    assert agent["args"] == ["run"]
    assert agent["input_mode"] == "prompt-file"
    assert agent["display_name"] == "OpenCode"
    assert agent["kind"] == "coding-cli"
    assert agent["supports_mcp"] is True
    assert agent["supports_diff_review"] is True
    assert agent["default_profile"] == "prod"
    assert agent["help_command"] == ["custombot", "--help"]


def test_cli_agents_add_rejects_unknown_default_profile(tmp_path):
    result = runner.invoke(
        app,
        [
            "agents",
            "add",
            "custombot",
            "--command",
            "custombot",
            "--default-profile",
            "missing",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 2
    assert "Unknown --default-profile 'missing'" in result.output


def test_cli_agents_add_rejects_blank_command(tmp_path):
    result = runner.invoke(
        app,
        [
            "agents",
            "add",
            "opencode",
            "--command",
            " ",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 2
    assert "--command cannot be empty" in result.output


def test_cli_integrate_cli_agent_rejects_unknown_default_profile(tmp_path):
    result = runner.invoke(
        app,
        [
            "integrate",
            "cli-agent",
            "custombot",
            "--command",
            "custombot",
            "--default-profile",
            "missing",
            "--apply",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 2
    assert "Unknown --default-profile 'missing'" in result.output


def test_cli_integrate_cli_agent_rejects_blank_name(tmp_path):
    result = runner.invoke(
        app,
        [
            "integrate",
            "cli-agent",
            " ",
            "--command",
            "opencode",
            "--apply",
            "--project-root",
            str(tmp_path),
        ],
    )

    assert result.exit_code == 2
    assert "Agent name cannot be empty" in result.output


def test_cli_agents_add_rejects_reserved_builtin_name(tmp_path):
    for name in ("oz", "agy"):
        result = runner.invoke(
            app,
            [
                "agents",
                "add",
                name,
                "--command",
                f"custom-{name}",
                "--project-root",
                str(tmp_path),
            ],
        )

        assert result.exit_code == 2
        assert "reserved for a built-in DevCouncil agent" in result.output


def test_cli_integrate_cli_agent_rejects_reserved_builtin_name(tmp_path):
    for name in ("codex", "google-antigravity"):
        result = runner.invoke(
            app,
            [
                "integrate",
                "cli-agent",
                name,
                "--command",
                f"custom-{name}",
                "--apply",
                "--project-root",
                str(tmp_path),
            ],
        )

        assert result.exit_code == 2
        assert "reserved for a built-in DevCouncil agent" in result.output


def test_cli_agents_lists_builtins_and_custom_agents(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "args": ["run"],
        "input_mode": "stdin",
    }
    config_path.write_text(yaml.safe_dump(raw), encoding="utf-8")

    result = runner.invoke(app, ["agents"])

    assert result.exit_code == 0
    assert "codex" in result.output
    assert "warp" in result.output
    assert "opencode" in result.output
    assert "antigravity" in result.output
    assert "custombot" in result.output


def test_cli_agents_doctor_reports_custom_agent_status(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "args": ["run"],
        "input_mode": "bad-mode",
    }
    config_path.write_text(yaml.safe_dump(raw), encoding="utf-8")
    monkeypatch.setattr("devcouncil.cli.commands.agents._which", lambda command: None)

    result = runner.invoke(app, ["agents", "doctor"])

    assert result.exit_code == 0
    assert "custombot" in result.output
    assert "invalid input_mode=bad-mode" in result.output


def test_cli_integrate_doctor_uses_project_root_for_custom_agents(tmp_path, monkeypatch):
    import yaml

    project = tmp_path / "project"
    project.mkdir()
    assert runner.invoke(app, ["setup", "--project-root", str(project), "--skip-api-key", "--skip-integrations"]).exit_code == 0
    config_path = project / ".devcouncil" / "config.yaml"
    raw = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    raw.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})["custombot"] = {
        "command": "custombot",
        "input_mode": "bad-mode",
    }
    config_path.write_text(yaml.safe_dump(raw), encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr("shutil.which", lambda command: None)

    result = runner.invoke(app, ["integrate", "doctor", "--project-root", str(project)])

    assert result.exit_code == 0
    assert "CLI agent: custombot" in result.output
    assert "invalid input_mode=bad-mode" in result.output


def test_cli_agents_run_passes_agent_and_profile(tmp_path, monkeypatch):
    called = {}

    def fake_run(task_id, executor, profile, project_root, **kwargs):
        called["task_id"] = task_id
        called["executor"] = executor
        called["profile"] = profile
        called["project_root"] = project_root

    monkeypatch.setattr("devcouncil.cli.commands.agents.run_command.run", fake_run)

    result = runner.invoke(
        app,
        ["agents", "run", "TASK-001", "--agent", "opencode", "--profile", "yolo", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    assert called["task_id"] == "TASK-001"
    assert called["executor"] == "opencode"
    assert called["profile"] == "yolo"
    assert called["project_root"] == tmp_path


def test_cli_agents_run_omits_profile_to_use_agent_default(tmp_path, monkeypatch):
    called = {}

    def fake_run(task_id, executor, profile, project_root, **kwargs):
        called["task_id"] = task_id
        called["executor"] = executor
        called["profile"] = profile
        called["project_root"] = project_root

    monkeypatch.setattr("devcouncil.cli.commands.agents.run_command.run", fake_run)

    result = runner.invoke(
        app,
        ["agents", "run", "TASK-001", "--agent", "opencode", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    assert called["task_id"] == "TASK-001"
    assert called["executor"] == "opencode"
    assert called["profile"] is None
    assert called["project_root"] == tmp_path


def test_cli_dashboard_open_launches_browser_before_serving(tmp_path, monkeypatch):
    opened = {}
    served = {}

    monkeypatch.setattr("devcouncil.cli.commands.dashboard.webbrowser.open", lambda url: opened.setdefault("url", url))
    monkeypatch.setattr(
        "devcouncil.cli.commands.dashboard.run_dashboard",
        lambda root, host, port: served.update({"root": root, "host": host, "port": port}),
    )

    result = runner.invoke(
        app,
        ["dashboard", "--open", "--host", "127.0.0.1", "--port", "9999", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 0
    assert opened["url"] == "http://127.0.0.1:9999"
    assert served == {"root": tmp_path, "host": "127.0.0.1", "port": 9999}


def test_cli_setup_initializes_project_and_prints_next_commands(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)

    result = runner.invoke(app, ["setup"])

    assert result.exit_code == 0
    assert (tmp_path / ".devcouncil" / "config.yaml").exists()
    assert (tmp_path / ".devcouncil" / "state.sqlite").exists()
    assert "DevCouncil Doctor Check" in result.output
    assert "Keep running DevCouncil commands in this terminal" in result.output
    assert "Paste only the dev prompt output into your coding CLI." in result.output
    assert "OPENROUTER_API_KEY is not set" in result.output
    assert "dev plan" in result.output
    assert "dev verify TASK-001" in result.output


def test_cli_setup_api_key_option_writes_local_secret(tmp_path, monkeypatch):
    from devcouncil.app.config import get_api_key

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)

    result = runner.invoke(app, ["setup", "--api-key", "sk-test-local"])

    assert result.exit_code == 0
    secrets = tmp_path / ".devcouncil" / "secrets.env"
    assert secrets.exists()
    assert "OPENROUTER_API_KEY=sk-test-local" in secrets.read_text(encoding="utf-8")
    assert get_api_key("openrouter", tmp_path) == "sk-test-local"
    assert "Found in .devcouncil/secrets.env" in result.output


def test_cli_setup_provider_option_updates_config_before_key_setup(tmp_path, monkeypatch):
    import yaml

    from devcouncil.app.config import get_api_key

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)

    result = runner.invoke(app, ["setup", "--provider", "openrouter", "--api-key", "sk-openrouter"])

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "openrouter"
    assert "OPENROUTER_API_KEY=sk-openrouter" in (tmp_path / ".devcouncil" / "secrets.env").read_text(encoding="utf-8")
    assert get_api_key("openrouter", tmp_path) == "sk-openrouter"


def test_cli_setup_vertexai_provider_writes_access_token(tmp_path, monkeypatch):
    import yaml

    from devcouncil.app.config import get_api_key

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("VERTEXAI_ACCESS_TOKEN", raising=False)

    result = runner.invoke(
        app,
        [
            "setup",
            "--provider",
            "vertexai",
            "--api-key",
            "ya29.local",
            "--vertex-project",
            "vertex-project",
            "--vertex-location",
            "us-central1",
        ],
    )

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "vertexai"
    assert raw_config["models"]["roles"]["spec_writer"]["model"] == "google/gemini-2.5-flash"
    assert raw_config["models"]["roles"]["critic_a"]["model"] == "google/gemini-2.5-flash"
    secrets = (tmp_path / ".devcouncil" / "secrets.env").read_text(encoding="utf-8")
    assert "VERTEXAI_ACCESS_TOKEN=ya29.local" in secrets
    assert "VERTEXAI_PROJECT=vertex-project" in secrets
    assert "VERTEXAI_LOCATION=us-central1" in secrets
    assert get_api_key("vertexai", tmp_path) == "ya29.local"


def test_cli_doctor_reports_vertexai_project_from_local_secret(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("VERTEXAI_ACCESS_TOKEN", raising=False)
    monkeypatch.delenv("VERTEXAI_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)
    assert runner.invoke(
        app,
        [
            "setup",
            "--provider",
            "vertexai",
            "--api-key",
            "ya29.local",
            "--vertex-project",
            "vertex-project",
            "--skip-integrations",
        ],
    ).exit_code == 0

    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "vertexai"

    result = runner.invoke(app, ["doctor"])

    assert result.exit_code == 0
    assert "VERTEXAI_ACCESS_TOKEN" in result.output
    assert "VERTEXAI_PROJECT" in result.output
    assert "Found in .devcouncil/secrets.env" in result.output
    assert "VERTEXAI_LOCATION" in result.output
    assert "global" in result.output


def test_cli_doctor_reports_vertexai_gcloud_token(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("VERTEXAI_ACCESS_TOKEN", raising=False)
    monkeypatch.setattr("devcouncil.app.config.shutil.which", lambda command: "gcloud" if command == "gcloud" else None)
    monkeypatch.setattr("devcouncil.app.config.subprocess.check_output", lambda *args, **kwargs: "ya29.gcloud\n")
    assert runner.invoke(
        app,
        [
            "setup",
            "--provider",
            "vertexai",
            "--vertex-project",
            "vertex-project",
            "--skip-integrations",
        ],
    ).exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "vertexai"

    result = runner.invoke(app, ["doctor"])

    assert result.exit_code == 0
    assert "VERTEXAI_ACCESS_TOKEN" in result.output
    assert "gcloud" in result.output


def test_cli_setup_accepts_model_selection_options(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)

    result = runner.invoke(
        app,
        [
            "setup",
            "--skip-api-key",
            "--skip-integrations",
            "--model",
            "openai/custom-default",
            "--role-model",
            "planner_b=google/custom-planner",
        ],
    )

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["roles"]["spec_writer"]["model"] == "openai/custom-default"
    assert raw_config["models"]["roles"]["planner_b"]["model"] == "google/custom-planner"


def test_cli_setup_rejects_unsupported_model_provider(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["setup", "--provider", "acme", "--api-key", "sk-acme"])

    assert result.exit_code == 2
    assert "Unsupported model provider 'acme'" in result.output
    assert not (tmp_path / ".devcouncil" / "secrets.env").exists()


def test_cli_setup_prefers_existing_environment_key(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("OPENROUTER_API_KEY", "sk-env")

    result = runner.invoke(app, ["setup", "--api-key", "sk-local"])

    assert result.exit_code == 0
    assert "already set in the environment" in result.output
    assert not (tmp_path / ".devcouncil" / "secrets.env").exists()


def test_cli_doctor_supports_project_root_for_local_secret(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("OPENROUTER_API_KEY", raising=False)
    assert runner.invoke(app, ["setup", "--api-key", "sk-local"]).exit_code == 0

    outside = tmp_path / "outside"
    outside.mkdir()
    monkeypatch.chdir(outside)

    result = runner.invoke(app, ["doctor", "--project-root", str(tmp_path)])

    assert result.exit_code == 0
    assert "OPENROUTER_API_KEY" in result.output
    assert "Found in .devcouncil/secrets.env" in result.output


def test_cli_doctor_reports_unsupported_model_provider(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    config_path = tmp_path / ".devcouncil" / "config.yaml"
    raw_config = yaml.safe_load(config_path.read_text(encoding="utf-8"))
    raw_config["models"]["provider"] = "acme"
    config_path.write_text(yaml.dump(raw_config), encoding="utf-8")

    result = runner.invoke(app, ["doctor"])

    assert result.exit_code == 0
    assert "models.provider" in result.output
    assert "Unsupported" in result.output
    assert "acme is configured" in result.output


def test_cli_config_models_shows_provider(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "models"])

    assert result.exit_code == 0
    assert "provider" in result.output
    assert "openrouter" in result.output
    assert "supported" in result.output


def test_cli_config_models_can_update_provider(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "models", "--provider", "vertexai"])

    assert result.exit_code == 0
    assert "Updated model provider from 'openrouter' to 'vertexai'" in result.output
    assert "Updated default role models for 'vertexai'" in result.output
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "vertexai"
    assert raw_config["models"]["roles"]["arbiter"]["model"] == "google/gemini-2.5-flash"


def test_cli_config_models_honors_project_root(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project
    import yaml

    project = tmp_path / "project"
    project.mkdir()
    monkeypatch.chdir(tmp_path)
    initialize_project(project)

    result = runner.invoke(app, ["config", "models", "--provider", "vertexai", "--project-root", str(project)])

    assert result.exit_code == 0
    raw_config = yaml.safe_load((project / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "vertexai"
    assert not (tmp_path / ".devcouncil").exists()


def test_cli_reset_demo_state_honors_project_root(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    project = tmp_path / "project"
    project.mkdir()
    monkeypatch.chdir(tmp_path)
    initialize_project(project)

    result = runner.invoke(app, ["reset-demo-state", "--yes", "--project-root", str(project)])

    assert result.exit_code == 0
    assert "Cleared requirements" in result.output
    assert not (tmp_path / ".devcouncil").exists()


def test_cli_repair_honors_project_root_when_no_gaps(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    project = tmp_path / "project"
    project.mkdir()
    monkeypatch.chdir(tmp_path)
    initialize_project(project)

    result = runner.invoke(app, ["repair", "--project-root", str(project)])

    assert result.exit_code == 0
    assert "No blocking gaps" in result.output
    assert not (tmp_path / ".devcouncil").exists()


def test_cli_config_models_can_update_all_roles_with_overrides(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(
        app,
        [
            "config",
            "models",
            "--model",
            "openai/custom-default",
            "--role-model",
            "planner_b=google/custom-planner",
        ],
    )

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["roles"]["spec_writer"]["model"] == "openai/custom-default"
    assert raw_config["models"]["roles"]["planner_b"]["model"] == "google/custom-planner"


def test_cli_config_models_can_update_provider_and_model_in_one_command(tmp_path, monkeypatch):
    import yaml

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(
        app,
        [
            "config",
            "models",
            "--provider",
            "vertexai",
            "--model",
            "google/custom-default",
            "--role-model",
            "critic_a=google/custom-critic",
        ],
    )

    assert result.exit_code == 0
    raw_config = yaml.safe_load((tmp_path / ".devcouncil" / "config.yaml").read_text(encoding="utf-8"))
    assert raw_config["models"]["provider"] == "vertexai"
    assert raw_config["models"]["roles"]["spec_writer"]["model"] == "google/custom-default"
    assert raw_config["models"]["roles"]["critic_a"]["model"] == "google/custom-critic"


def test_cli_config_models_rejects_invalid_role_model_override(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "models", "--role-model", "critic_a"])

    assert result.exit_code == 2
    assert "Use ROLE=MODEL" in result.output


def test_cli_config_models_rejects_unsupported_provider(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["config", "models", "--provider", "acme"])

    assert result.exit_code == 2
    assert "Unsupported model provider 'acme'" in result.output


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
    assert "claude mcp add --scope local" in result.output
    assert ".cursor" in result.output
    assert '"mcpServers":{"devcouncil"' in result.output
    assert "Google Antigravity CLI" in result.output
    assert ".agents" in result.output
    assert "Native hook config preview" in result.output
    assert f"DEVCOUNCIL_PROJECT_ROOT={tmp_path}" in result.output


def test_cli_setup_first_run_prompts_to_apply_integrations(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr("devcouncil.cli.commands.setup._is_interactive_terminal", lambda: True)
    monkeypatch.setattr("shutil.which", lambda command: None)

    result = runner.invoke(app, ["setup", "--skip-api-key"], input="\n")

    assert result.exit_code == 0
    assert "Coding CLI Setup" in result.output
    assert "Set up coding CLI integrations now?" in result.output
    assert "Skipping optional integration" in result.output
    assert (tmp_path / ".codex" / "hooks.json").exists()


def test_cli_setup_first_run_can_skip_integration_prompt(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr("devcouncil.cli.commands.setup._is_interactive_terminal", lambda: True)

    result = runner.invoke(app, ["setup", "--skip-api-key"], input="n\n")

    assert result.exit_code == 0
    assert "Skipped coding CLI integration setup" in result.output
    assert not (tmp_path / ".codex" / "hooks.json").exists()


def test_cli_setup_skip_integrations_suppresses_first_run_prompt(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr("devcouncil.cli.commands.setup._is_interactive_terminal", lambda: True)

    result = runner.invoke(app, ["setup", "--skip-api-key", "--skip-integrations"])

    assert result.exit_code == 0
    assert "Coding CLI Setup" not in result.output
    assert not (tmp_path / ".codex" / "hooks.json").exists()


def test_cli_setup_apply_skips_missing_optional_integrations(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr("shutil.which", lambda command: None)

    result = runner.invoke(app, ["setup", "--integrate", "--apply"])

    assert result.exit_code == 0
    assert "Skipping optional integration" in result.output
    assert (tmp_path / ".devcouncil" / "config.yaml").exists()
    assert (tmp_path / ".codex" / "hooks.json").exists()


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


def test_cli_prompt_outputs_raw_markdown(tmp_path, monkeypatch):
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
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            expected_tests=["pytest tests/test_app.py"],
            allowed_commands=["pytest tests/test_app.py"],
        ))

    result = runner.invoke(app, ["prompt", "TASK-001"])

    assert result.exit_code == 0
    assert result.output.startswith("# Implement TASK-001")
    assert "`src/app.py`" in result.output


def test_cli_report_json_is_machine_readable(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    result = runner.invoke(app, ["report", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data["verdict"] == "passed"


def test_cli_report_json_includes_live_review_blockers(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    critical = tmp_path / "critical.jsonl"
    critical.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(app, ["watch", "review", "--transcript", str(critical)]).exit_code == 0

    result = runner.invoke(app, ["report", "--json"])

    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data["verdict"] == "blocked"
    assert data["live_review"]["cards"]["critical_open"] == 1
    assert data["live_review"]["blocking_cards"][0]["verdict"] == "Critical Issues"


def test_cli_report_markdown_includes_live_review_section(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    critical = tmp_path / "critical.jsonl"
    critical.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(app, ["watch", "review", "--transcript", str(critical)]).exit_code == 0

    result = runner.invoke(app, ["report"])

    assert result.exit_code == 0
    assert "Live Review" in result.output
    assert "Blocking Live-Review Cards" in result.output


def test_cli_status_tasks_show_json_outputs(tmp_path, monkeypatch):
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
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            expected_tests=["python --version"],
            allowed_commands=["python --version"],
        ))

    status = runner.invoke(app, ["status", "--json"])
    tasks = runner.invoke(app, ["tasks", "--json"])
    show = runner.invoke(app, ["show", "TASK-001", "--json"])

    assert status.exit_code == 0
    assert json.loads(status.output)["initialized"] is True
    assert tasks.exit_code == 0
    assert json.loads(tasks.output)["tasks"][0]["id"] == "TASK-001"
    assert show.exit_code == 0
    assert json.loads(show.output)["task"]["id"] == "TASK-001"


def test_cli_status_json_includes_live_review_summary(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    critical = tmp_path / "critical.jsonl"
    critical.write_text(
        json.dumps({"role": "assistant", "id": "A-1", "content": "Run git reset --hard."}) + "\n",
        encoding="utf-8",
    )
    assert runner.invoke(app, [
        "watch",
        "review",
        "--transcript",
        str(critical),
        "--task-id",
        "TASK-001",
    ]).exit_code == 0
    assert runner.invoke(
        app,
        ["hook", "agent-response", "--client", "claude"],
        input='{"transcript_path":"session.jsonl"}',
    ).exit_code == 0

    result = runner.invoke(app, ["status", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["live_review"]["pending_signals"] == 1
    assert payload["live_review"]["cards"]["critical_open"] == 1
    assert payload["live_review"]["blocking_cards"][0]["task_id"] == "TASK-001"


def test_cli_status_auto_initializes_when_missing(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "README.md").write_text("status\n", encoding="utf-8")

    result = runner.invoke(app, ["status", "--json"])
    assert result.exit_code == 0
    assert json.loads(result.output)["initialized"] is True
    assert json.loads(result.output)["phase"] == "NEW"


def test_cli_status_auto_initializes_with_project_root(tmp_path, monkeypatch):
    project = tmp_path / "project"
    project.mkdir()
    (project / "README.md").write_text("status\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)

    result = runner.invoke(app, ["status", "--json", "--project-root", str(project)])
    assert result.exit_code == 0
    data = json.loads(result.output)
    assert data["initialized"] is True
    assert data["phase"] == "NEW"
    assert (project / ".devcouncil" / "state.sqlite").exists()


def test_cli_verify_json_output(tmp_path, monkeypatch):
    from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
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
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            expected_tests=["python --version"],
            allowed_commands=["python --version"],
        ))

    # Produce the planned change so there is real work to verify. An empty diff is
    # now correctly blocked (the agent cannot "pass" having written nothing), so a
    # passing-verify shape test must actually implement the file.
    (tmp_path / "src").mkdir(exist_ok=True)
    (tmp_path / "src" / "app.py").write_text("VALUE = 1\n", encoding="utf-8")

    result = runner.invoke(app, ["verify", "TASK-001", "--json"])

    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["ok"] is True
    assert payload["tasks"][0]["task_id"] == "TASK-001"
    # The verify response now reports the rigor of the run.
    assert payload["tasks"][0]["verification_mode"] in {"coarse", "compiled"}
    assert payload["tasks"][0]["diff_empty"] is False


def test_cli_verify_blocks_empty_diff(tmp_path, monkeypatch):
    """A task that declares changes but produced none is blocked (exit 1)."""
    from devcouncil.domain.requirement import AcceptanceCriterion, Requirement
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import RequirementRepository, TaskRepository

    monkeypatch.chdir(tmp_path)
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        RequirementRepository(session).save(Requirement(
            id="REQ-001", title="R", description="d", priority="high", source="user",
            acceptance_criteria=[AcceptanceCriterion(id="AC-001", description="t", verification_method="unit_test")],
        ))
        TaskRepository(session).save(Task(
            id="TASK-001", title="T", description="d",
            requirement_ids=["REQ-001"], acceptance_criterion_ids=["AC-001"],
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            expected_tests=["python --version"],
        ))

    result = runner.invoke(app, ["verify", "TASK-001", "--json"])

    assert result.exit_code == 1
    payload = json.loads(result.output)
    assert payload["ok"] is False
    assert any(g["gap_type"] == "task_not_implemented" for g in payload["tasks"][0]["gaps"])


def test_cli_prompt_project_root_option(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    project = tmp_path / "project"
    other = tmp_path / "other"
    project.mkdir()
    other.mkdir()
    monkeypatch.chdir(project)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(project)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Task",
            description="desc",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
        ))

    monkeypatch.chdir(other)
    result = runner.invoke(app, ["prompt", "TASK-001", "--project-root", str(project)])

    assert result.exit_code == 0
    assert result.output.startswith("# Implement TASK-001")


def test_cli_rollback_accepts_after_patch_without_before_patch(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    target = tmp_path / "src" / "new_file.py"
    target.parent.mkdir()
    target.write_text("print('new')\n", encoding="utf-8")
    checkpoint_dir = tmp_path / ".devcouncil" / "checkpoints"
    checkpoint_dir.mkdir(parents=True)
    (checkpoint_dir / "TASK-001-after.patch").write_text(
        "\n".join([
            "diff --git a/src/new_file.py b/src/new_file.py",
            "new file mode 100644",
            "--- /dev/null",
            "+++ b/src/new_file.py",
            "@@ -0,0 +1,1 @@",
            "+print('new')",
            "",
        ]),
        encoding="utf-8",
    )

    result = runner.invoke(app, ["rollback", "TASK-001"])

    assert result.exit_code == 0
    assert not target.exists()


def test_cli_run_supports_coding_cli_executors(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.execution.executor import ExecutionResult
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    assert runner.invoke(app, ["init"]).exit_code == 0

    called = {}

    class FakeCodingExecutor:
        def __init__(self, project_root, client, **kwargs):
            called["client"] = client
            called["project_root"] = str(project_root)

        def run_task(self, task, reqs):
            called["task_id"] = task.id
            return ExecutionResult(success=True, message="ok")

    monkeypatch.setattr("devcouncil.cli.commands.run.CodingCliExecutor", FakeCodingExecutor)
    monkeypatch.setattr("devcouncil.cli.commands.run._verify_after_execution", lambda *_a, **_kw: True)

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Run with Claude",
            description="Implement with Claude",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            allowed_commands=["python -m pytest"],
            expected_tests=["python -m pytest"],
        ))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "claude-code"])

    assert result.exit_code == 0
    assert called["client"] == "claude"
    assert called["task_id"] == "TASK-001"


def test_cli_run_passes_project_root_to_task_ready_gate(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.gating.policy import GateResult
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    project = tmp_path / "project"
    other = tmp_path / "other"
    project.mkdir()
    other.mkdir()
    monkeypatch.chdir(project)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(project)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Run from outside",
            description="Uses target root",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            allowed_commands=["python --version"],
            expected_tests=["python --version"],
        ))

    seen = {}

    def fake_check_task_ready(self, task, project_root):
        _ = self, task
        seen["project_root"] = project_root
        return GateResult(passed=True, gaps=[])

    monkeypatch.setattr("devcouncil.gating.policy.GatePolicy.check_task_ready", fake_check_task_ready)
    monkeypatch.chdir(other)

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "manual", "--project-root", str(project)])

    assert result.exit_code == 0
    assert seen["project_root"] == project.resolve()


def test_cli_run_supports_coding_cli_alias_executors(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.execution.executor import ExecutionResult
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    assert runner.invoke(app, ["init"]).exit_code == 0

    called = {}

    class FakeCodingExecutor:
        def __init__(self, project_root, client, **kwargs):
            called["client"] = client

        def run_task(self, task, reqs):
            return ExecutionResult(success=True, message="ok")

    monkeypatch.setattr("devcouncil.cli.commands.run.CodingCliExecutor", FakeCodingExecutor)
    monkeypatch.setattr("devcouncil.cli.commands.run._verify_after_execution", lambda *_a, **_kw: True)

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Run with alias executors",
            description="Implement with aliases",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            allowed_commands=["python -m pytest"],
            expected_tests=["python -m pytest"],
        ))

    for alias, expected_client in [
        ("codex_cli", "codex"),
        ("codex-cli", "codex"),
        ("gemini-cli", "gemini"),
        ("claude-cli", "claude"),
        ("antigravity-cli", "antigravity"),
        ("agy", "antigravity"),
        ("warp-cli", "warp"),
        ("oz", "warp"),
        ("cursor-agent", "cursor"),
        ("cursor-cli", "cursor"),
        ("aider", "aider"),
    ]:
        called.clear()
        result = runner.invoke(app, ["run", "TASK-001", "--executor", alias])
        assert result.exit_code == 0
        assert called["client"] == expected_client


def test_cli_run_reports_unimplemented_executor(tmp_path, monkeypatch):
    from devcouncil.domain.task import PlannedFile, Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    subprocess.check_call(["git", "init"], cwd=tmp_path, stdout=subprocess.DEVNULL)
    assert runner.invoke(app, ["init"]).exit_code == 0

    db = get_db(tmp_path)
    assert db is not None
    with db.get_session() as session:
        TaskRepository(session).save(Task(
            id="TASK-001",
            title="Unknown executor",
            description="Fails",
            planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
            allowed_commands=["python -m pytest"],
            expected_tests=["python -m pytest"],
        ))

    result = runner.invoke(app, ["run", "TASK-001", "--executor", "experimental"])

    assert result.exit_code == 0
    assert "Executor experimental not yet implemented." in result.output
