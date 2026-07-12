"""CLI coverage for `dev campaign` — run (dry-run/executor/halt/fail-on-blocking),
status, inbox, and roster. The multi-agent orchestrator is replaced with a fake
Campaign that returns a prebuilt result so command wiring and rendering are tested."""

import json

import devcouncil.cli.commands.campaign as campaign_cmd
from devcouncil.campaign import Mailbox
from devcouncil.campaign.orchestrator import CampaignResult, TaskOutcome
from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from typer.testing import CliRunner

runner = CliRunner()


def _tasks():
    return [Task(id="TASK-001", title="First task", description="d")]


def _result(**kw):
    outcomes = kw.pop(
        "outcomes",
        [TaskOutcome(
            task_id="TASK-001", title="First task", owner="worker1", bloom="apply",
            executed=True, verified=True, status="verified",
        )],
    )
    defaults = dict(goal="Ship it", outcomes=outcomes, dashboard_path=None)
    defaults.update(kw)
    return CampaignResult(**defaults)


def _fake_campaign(result):
    class _FakeCampaign:
        def __init__(self, *args, **kwargs):
            pass

        def run(self):
            return result

    return _FakeCampaign


def test_campaign_run_no_plan(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: ([], []))

    result = runner.invoke(app, ["campaign", "run", "Ship it", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No plan found" in result.output


def test_campaign_run_dry_run_human(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: (_tasks(), []))
    monkeypatch.setattr(campaign_cmd, "Campaign", _fake_campaign(_result()))

    result = runner.invoke(app, ["campaign", "run", "Ship it", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Dry run" in result.output
    assert "First task" in result.output
    assert "verified" in result.output


def test_campaign_run_dry_run_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: (_tasks(), []))
    monkeypatch.setattr(campaign_cmd, "Campaign", _fake_campaign(_result()))

    result = runner.invoke(app, ["campaign", "run", "Ship it", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    # The dry-run banner is printed before the JSON payload; parse from the first brace.
    data = json.loads(result.stdout[result.stdout.index("{"):])
    assert data["goal"] == "Ship it"
    assert data["outcomes"][0]["task_id"] == "TASK-001"


def test_campaign_run_with_executor_persists(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: (_tasks(), []))
    monkeypatch.setattr(campaign_cmd, "Campaign", _fake_campaign(_result()))
    monkeypatch.setattr(campaign_cmd, "build_coding_executor_factory", lambda *a, **k: object())
    monkeypatch.setattr(campaign_cmd, "build_verifier_fn", lambda *a, **k: object())
    persisted = {}
    monkeypatch.setattr(campaign_cmd, "_persist_statuses", lambda root, tasks: persisted.update(n=len(tasks)))

    result = runner.invoke(
        app, ["campaign", "run", "Ship it", "--executor", "claude", "--no-verify", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert persisted["n"] == 1


def test_campaign_run_halted_exits_nonzero(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: (_tasks(), []))
    monkeypatch.setattr(
        campaign_cmd, "Campaign",
        _fake_campaign(_result(halted=True, halt_reason="cost budget exceeded")),
    )

    result = runner.invoke(app, ["campaign", "run", "Ship it", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_campaign_run_fail_on_blocking(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: (_tasks(), []))
    blocked = [TaskOutcome(
        task_id="TASK-001", title="First task", owner="worker1", bloom="apply",
        executed=True, verified=False, status="blocked", blocking_gaps=["missing test"],
    )]
    monkeypatch.setattr(campaign_cmd, "Campaign", _fake_campaign(_result(outcomes=blocked)))

    result = runner.invoke(
        app, ["campaign", "run", "Ship it", "--fail-on-blocking", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1


def test_campaign_status_no_dashboard(tmp_path):
    result = runner.invoke(app, ["campaign", "status", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No campaign has been run yet" in result.output


def test_campaign_status_prints_dashboard(tmp_path):
    dash = tmp_path / ".devcouncil" / "campaign"
    dash.mkdir(parents=True)
    (dash / "dashboard.md").write_text("# Campaign dashboard\nAll good.", encoding="utf-8")

    result = runner.invoke(app, ["campaign", "status", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Campaign dashboard" in result.output


def test_campaign_inbox_empty(tmp_path):
    result = runner.invoke(app, ["campaign", "inbox", "coordinator", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "mailbox is empty" in result.output


def test_campaign_inbox_with_message(tmp_path):
    mailbox = Mailbox(tmp_path)
    mailbox.send("coordinator", "Execute the plan.", type="order", from_agent="director")

    result = runner.invoke(app, ["campaign", "inbox", "coordinator", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "director" in result.output
    assert "Execute the plan" in result.output


def test_campaign_roster():
    result = runner.invoke(app, ["campaign", "roster"])
    assert result.exit_code == 0
    assert "Chain of Command" in result.output
    assert "Reports to" in result.output


def test_render_result_direct(capsys):
    campaign_cmd._render_result(_result())
    out = capsys.readouterr().out
    assert "Director Campaign" in out
    assert "First task" in out
