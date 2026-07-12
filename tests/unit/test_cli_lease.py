import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _setup_lease_env(tmp_path: Path, monkeypatch) -> tuple[Path, str]:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    # Create task in database
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(
        id="TASK-1",
        title="Settings Task",
        description="d",
        status="planned",
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
    return tmp_path, "TASK-1"


def test_cli_lease_checkout_and_release(tmp_path, monkeypatch):
    root, task_id = _setup_lease_env(tmp_path, monkeypatch)
    
    # 1. Checkout lease
    res = runner.invoke(app, ["checkout", task_id, "--client-id", "test-agent", "--agent", "test-runner", "--json"])
    assert res.exit_code == 0, f"checkout failed: {res.output}\nexception: {res.exception}"
    data = json.loads(res.output)
    assert data["ok"] is True
    lease_tok = data["lease_token"]
    assert lease_tok is not None
    
    # 2. Try checkout again (should fail/already leased)
    res2 = runner.invoke(app, ["checkout", task_id, "--client-id", "test-agent", "--agent", "test-runner", "--json"])
    assert res2.exit_code != 0
    data2 = json.loads(res2.output)
    assert data2["ok"] is False
    assert "active lease already exists" in data2["error"].lower()
    
    # 3. Release lease
    res3 = runner.invoke(app, ["release", task_id, "--lease-token", lease_tok, "--json"])
    assert res3.exit_code == 0
    data3 = json.loads(res3.output)
    assert data3["ok"] is True
    
    # 4. Checkout again (should succeed since released)
    res4 = runner.invoke(app, ["checkout", task_id, "--client-id", "test-agent", "--agent", "test-runner"])
    assert res4.exit_code == 0
    assert "checked out" in res4.output.lower()
