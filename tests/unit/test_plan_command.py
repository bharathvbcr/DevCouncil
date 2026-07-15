"""Coverage for `dev plan`/`dev approve` helpers and the approve command.

The full dry-run council flow is exercised in test_cli_commands.py; this file targets
the approve command, planning-error rendering, and the small reconciliation/role helpers.
"""

from __future__ import annotations

import asyncio
from types import SimpleNamespace

from typer.testing import CliRunner

import devcouncil.cli.commands.plan as plan_cmd
from devcouncil.cli.main import app
from devcouncil.domain.critique import CritiqueFinding
from devcouncil.llm.provider import ProviderRequestError
from devcouncil.llm.router import StructuredOutputError
from devcouncil.storage.db import get_db, reset_db_cache
from devcouncil.storage.models import ProjectStateModel
from devcouncil.storage.repositories import StateRepository

runner = CliRunner()


def _finding(fid: str, status: str = "open") -> CritiqueFinding:
    return CritiqueFinding(
        id=fid,
        source_agent="critic_a",
        target_plan_id="PLAN-A",
        severity="high",
        finding_type="missing_test",
        claim="needs a test",
        falsifiable_check="run pytest",
        status=status,
    )


# --- _decision_ids ----------------------------------------------------------------


def test_decision_ids_handles_strings_and_dicts():
    ids = plan_cmd._decision_ids(["A", {"id": "B"}, {"finding_id": "C"}, {"nope": "x"}, 123])
    assert ids == {"A", "B", "C"}


# --- _reconcile_findings ----------------------------------------------------------


def test_reconcile_findings_marks_accepted_and_rejected():
    findings = [_finding("F1"), _finding("F2"), _finding("F3")]
    decision = SimpleNamespace(
        accepted_finding_ids=["F1"],
        rejected_finding_ids=[{"id": "F2", "reason": "no"}],
    )
    reconciled = plan_cmd._reconcile_findings(findings, decision)
    by_id = {f.id: f.status for f in reconciled}
    assert by_id["F1"] == "converted"
    assert by_id["F2"] == "rejected"
    assert by_id["F3"] == "open"


# --- _ensure_planning_roles -------------------------------------------------------


def test_ensure_planning_roles_backfills_from_spec_writer():
    from devcouncil.app.config import ModelRoleConfig

    config = SimpleNamespace(
        models=SimpleNamespace(
            provider="openrouter",
            roles={"spec_writer": ModelRoleConfig(model="x/y")},
        )
    )
    plan_cmd._ensure_planning_roles(config)
    for role in plan_cmd.REQUIRED_PLANNING_ROLES:
        assert role in config.models.roles


def test_ensure_planning_roles_falls_back_to_provider_defaults():
    config = SimpleNamespace(
        models=SimpleNamespace(provider="openrouter", roles={})
    )
    plan_cmd._ensure_planning_roles(config)
    assert "planner_a" in config.models.roles


# --- _should_auto_convert_blocking_questions --------------------------------------


def test_should_auto_convert_respects_config_flag():
    config = SimpleNamespace(
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=False)
    )
    assert plan_cmd._should_auto_convert_blocking_questions(config) is False


def test_should_auto_convert_true_when_noninteractive(monkeypatch):
    config = SimpleNamespace(
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=True)
    )
    monkeypatch.setattr(plan_cmd.sys.stdin, "isatty", lambda: False)
    assert plan_cmd._should_auto_convert_blocking_questions(config) is True


# --- _maybe_convert_blocking_questions --------------------------------------------


def test_maybe_convert_no_op_when_disabled():
    config = SimpleNamespace(
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=False)
    )
    spec = SimpleNamespace(blocking_questions=["q"], assumptions=[])
    assert plan_cmd._maybe_convert_blocking_questions(spec, config, plan_cmd.console) is spec


def test_maybe_convert_writes_artifact(tmp_path, monkeypatch):
    config = SimpleNamespace(
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=True)
    )
    monkeypatch.setattr(plan_cmd.sys.stdin, "isatty", lambda: False)
    monkeypatch.setattr(
        plan_cmd, "convert_blocking_questions_to_assumptions",
        lambda assumptions, questions: (["converted-assumption"], []),
    )

    class _Spec:
        def __init__(self):
            self.blocking_questions = ["q1"]
            self.assumptions = []

        def model_copy(self, update):
            new = _Spec()
            new.assumptions = update["assumptions"]
            new.blocking_questions = update["blocking_questions"]
            return new

        def model_dump_json(self, indent=2):
            return "{}"

    artifact = tmp_path / "requirements.json"
    result = plan_cmd._maybe_convert_blocking_questions(
        _Spec(), config, plan_cmd.console, artifact_path=artifact
    )
    assert result.blocking_questions == []
    assert artifact.exists()


# --- run_plan_flow: state unavailable ---------------------------------------------


def test_run_plan_flow_db_unavailable(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    monkeypatch.setattr(plan_cmd, "get_db", lambda root: None)
    out = asyncio.run(plan_cmd.run_plan_flow("goal", project_root=tmp_path))
    assert out == []


# --- print_planning_error ---------------------------------------------------------


def test_print_planning_error_structured(capsys):
    exc = StructuredOutputError("bad json", role="planner_a", model="tiny/model")
    plan_cmd.print_planning_error(exc)
    out = capsys.readouterr().out
    assert "Planning could not complete" in out
    assert "planner_a" in out


def test_print_planning_error_payment_required(capsys):
    exc = ProviderRequestError("payment", status_code=402)
    plan_cmd.print_planning_error(exc)
    assert "credits" in capsys.readouterr().out


# --- _latest_run_with_decision ----------------------------------------------------


def test_latest_run_with_decision_variants(tmp_path):
    runs = tmp_path / ".devcouncil" / "runs"
    runs.mkdir(parents=True)
    # No runs → None
    assert plan_cmd._latest_run_with_decision(tmp_path, None) is None

    r1 = runs / "run1"
    r1.mkdir()
    (r1 / "decision.json").write_text("{}", encoding="utf-8")
    assert plan_cmd._latest_run_with_decision(tmp_path, None) == r1
    # Explicit run id that exists
    assert plan_cmd._latest_run_with_decision(tmp_path, "run1") == r1
    # Explicit run id without a decision → None
    (runs / "run2").mkdir()
    assert plan_cmd._latest_run_with_decision(tmp_path, "run2") is None


# --- approve command --------------------------------------------------------------


def test_approve_no_run_found(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    result = runner.invoke(app, ["approve"])
    assert result.exit_code == 1
    assert "No planning run" in result.output


def test_approve_db_unavailable(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(plan_cmd, "get_db", lambda root: None)
    result = runner.invoke(app, ["approve"])
    assert result.exit_code == 1
    assert "state is unavailable" in result.output


def test_approve_success_after_dry_run(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    # Produce a persisted planning run (decision.json etc.).
    assert runner.invoke(app, ["plan", "Add feature", "--dry-run", "--persist"]).exit_code == 0

    # Move phase back to AWAITING_USER_DECISIONS so approve can transition to PLAN_APPROVED.
    db = get_db(tmp_path)
    with db.get_session() as session:
        StateRepository(session).save_state(ProjectStateModel(current_phase="AWAITING_USER_DECISIONS"))

    result = runner.invoke(app, ["approve"])
    assert result.exit_code == 0
    assert "approved" in result.output.lower()


def test_approve_fails_gates_without_force(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    assert runner.invoke(app, ["plan", "Add feature", "--dry-run", "--persist"]).exit_code == 0

    from devcouncil.domain.gap import Gap
    from devcouncil.gating.policy import GateResult

    blocking = Gap(
        id="PG1", severity="high", gap_type="requirement_not_planned",
        description="unplanned requirement", recommended_fix="plan it", blocking=True,
    )
    monkeypatch.setattr(
        plan_cmd.GatePolicy, "check_plan_approval",
        lambda self, reqs, tasks, **k: GateResult(passed=False, gaps=[blocking]),
    )
    result = runner.invoke(app, ["approve"])
    assert result.exit_code == 1
    assert "still fails approval gates" in result.output


def test_approve_force_overrides_failing_gates(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    assert runner.invoke(app, ["plan", "Add feature", "--dry-run", "--persist"]).exit_code == 0

    db = get_db(tmp_path)
    with db.get_session() as session:
        StateRepository(session).save_state(ProjectStateModel(current_phase="AWAITING_USER_DECISIONS"))

    from devcouncil.domain.gap import Gap
    from devcouncil.gating.policy import GateResult

    blocking = Gap(
        id="PG1", severity="high", gap_type="requirement_not_planned",
        description="unplanned requirement", recommended_fix="plan it", blocking=True,
    )
    monkeypatch.setattr(
        plan_cmd.GatePolicy, "check_plan_approval",
        lambda self, reqs, tasks, **k: GateResult(passed=False, gaps=[blocking]),
    )
    result = runner.invoke(app, ["approve", "--force"])
    assert result.exit_code == 0
    assert "approved" in result.output.lower()


# --- plan command error rendering -------------------------------------------------


def test_plan_command_renders_provider_error(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0

    async def boom(*a, **k):
        raise StructuredOutputError("bad", role="planner_a", model="m")

    monkeypatch.setattr(plan_cmd, "run_plan_flow", boom)
    result = runner.invoke(app, ["plan", "goal"])
    assert result.exit_code == 1
    assert "Planning could not complete" in result.output
