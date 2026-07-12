import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.repositories import TaskRepository
from devcouncil.campaign.mailbox import Mailbox

runner = CliRunner()


def _setup_campaign_env(tmp_path: Path, monkeypatch) -> Path:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    return tmp_path


def test_cli_campaign_roster():
    res = runner.invoke(app, ["campaign", "roster"])
    assert res.exit_code == 0
    assert "Director Chain of Command" in res.output
    assert "director" in res.output.lower()
    assert "coordinator" in res.output.lower()


def test_cli_campaign_status_missing(tmp_path, monkeypatch):
    _setup_campaign_env(tmp_path, monkeypatch)
    res = runner.invoke(app, ["campaign", "status"])
    assert res.exit_code == 0
    assert "No campaign has been run yet" in res.output


def test_cli_campaign_status_present(tmp_path, monkeypatch):
    root = _setup_campaign_env(tmp_path, monkeypatch)
    dash = root / ".devcouncil" / "campaign" / "dashboard.md"
    dash.parent.mkdir(parents=True, exist_ok=True)
    dash.write_text("CAMPAIGN ACTIVE", encoding="utf-8")
    
    res = runner.invoke(app, ["campaign", "status"])
    assert res.exit_code == 0
    assert "CAMPAIGN ACTIVE" in res.output


def test_cli_campaign_inbox(tmp_path, monkeypatch):
    root = _setup_campaign_env(tmp_path, monkeypatch)
    
    # Empty inbox
    res = runner.invoke(app, ["campaign", "inbox", "coordinator"])
    assert res.exit_code == 0
    assert "mailbox is empty" in res.output
    
    # Send a message
    mailbox = Mailbox(root)
    mailbox.send(
        target="coordinator",
        content="implement authentication",
        type="goal",
        from_agent="director",
    )
    
    # Read inbox
    res2 = runner.invoke(app, ["campaign", "inbox", "coordinator"])
    assert res2.exit_code == 0
    assert "director" in res2.output
    assert "goal" in res2.output
    assert "implement authentication" in res2.output


def test_cli_campaign_run_no_plan(tmp_path, monkeypatch):
    _setup_campaign_env(tmp_path, monkeypatch)
    res = runner.invoke(app, ["campaign", "run", "do task"])
    assert res.exit_code == 0
    assert "No plan found" in res.output


def test_cli_campaign_run_dry_run(tmp_path, monkeypatch):
    root = _setup_campaign_env(tmp_path, monkeypatch)
    
    # Put a task in database
    db = Database(root / ".devcouncil" / "state.sqlite")
    task = Task(
        id="TASK-1",
        title="Implement settings",
        description="d",
        status="planned",
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
        
    res = runner.invoke(app, ["campaign", "run", "Implement settings", "--json"])
    assert res.exit_code == 0
    
    # Parse json portion only
    json_start = res.output.find("{")
    assert json_start != -1
    json_str = res.output[json_start:]
    data = json.loads(json_str)
    
    assert data["success"] is True
    assert any(o["task_id"] == "TASK-1" for o in data["outcomes"])
