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


def _tasks(count: int = 1):
    return [Task(id=f"TASK-{i:03d}", title=f"Task {i}" if i > 1 else "First task", description="d")
            for i in range(1, count + 1)]


def _result(count: int = 1, **kw):
    outcomes = kw.pop(
        "outcomes",
        [TaskOutcome(
            task_id=f"TASK-{i:03d}", title=f"Task {i}" if i > 1 else "First task", owner="worker1",
            bloom="apply", executed=True, verified=True, status="verified",
        ) for i in range(1, count + 1)],
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
    # The banner is a diagnostic and belongs on stderr; the rendered result is the stdout payload.
    assert "Dry run" in result.stderr
    assert "Dry run" not in result.stdout
    assert "First task" in result.stdout
    assert "verified" in result.stdout


def test_campaign_run_dry_run_json(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: (_tasks(), []))
    monkeypatch.setattr(campaign_cmd, "Campaign", _fake_campaign(_result()))

    result = runner.invoke(app, ["campaign", "run", "Ship it", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    # `--json` contract: stdout is exactly one JSON object; the banner goes to stderr.
    data = json.loads(result.stdout)
    assert data["goal"] == "Ship it"
    assert data["outcomes"][0]["task_id"] == "TASK-001"
    assert data["dry_run"] is True
    assert data["error"] is None
    assert "Dry run" in result.stderr


def test_campaign_run_json_no_plan_still_emits_one_object(tmp_path, monkeypatch):
    """No plan is a diagnostic, not a reason to emit zero JSON objects."""
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: ([], []))

    result = runner.invoke(app, ["campaign", "run", "Ship it", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["goal"] == "Ship it"
    assert data["outcomes"] == []
    assert data["success"] is False
    assert "No plan found" in data["error"]
    # The human-readable banner is on stderr; stdout is pure JSON (proved by the parse above,
    # which is why we do not also assert on the phrase — it legitimately appears in "error").
    assert "No plan found" in result.stderr


def test_campaign_run_json_large_plan_hint_goes_to_stderr(tmp_path, monkeypatch):
    """The >=5-task hint is the other banner that used to land on stdout."""
    monkeypatch.chdir(tmp_path)
    monkeypatch.setattr(campaign_cmd, "_load_plan", lambda root: (_tasks(6), []))
    monkeypatch.setattr(campaign_cmd, "Campaign", _fake_campaign(_result(6)))
    monkeypatch.setattr(campaign_cmd, "build_coding_executor_factory", lambda *a, **k: object())
    monkeypatch.setattr(campaign_cmd, "_persist_statuses", lambda root, tasks: None)

    result = runner.invoke(
        app,
        ["campaign", "run", "Ship it", "--json", "--executor", "claude", "--no-verify",
         "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["dry_run"] is False
    assert len(data["outcomes"]) == 6
    assert "Large plan" in result.stderr
    assert "Large plan" not in result.stdout


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
