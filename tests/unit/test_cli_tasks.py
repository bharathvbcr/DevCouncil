import json
from pathlib import Path
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.task import Task
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.repositories import TaskRepository

runner = CliRunner()


def _setup_tasks_db(tmp_path: Path, monkeypatch) -> tuple[Path, str]:
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    task = Task(
        id="TASK-1",
        title="Original Title",
        description="Original Desc",
        status="planned",
        priority="low",
    )
    with db.get_session() as session:
        TaskRepository(session).save(task)
    return tmp_path, "TASK-1"


def test_cli_tasks_list(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    
    # Text list
    res = runner.invoke(app, ["tasks"])
    assert res.exit_code == 0
    assert "TASK-1" in res.output
    assert "Original Title" in res.output
    assert "Lease" in res.output
    
    # JSON list
    res_json = runner.invoke(app, ["tasks", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["total"] == 1
    assert data["tasks"][0]["id"] == "TASK-1"
    assert data["tasks"][0]["lease"] is None


def test_cli_tasks_list_includes_active_lease(tmp_path, monkeypatch):
    tmp_path, task_id = _setup_tasks_db(tmp_path, monkeypatch)
    from devcouncil.storage.db import Database
    from devcouncil.storage.native import TaskLeaseRepository

    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        TaskLeaseRepository(session).acquire(task_id, owner="agent:codex", agent="codex", ttl_seconds=600)

    res_json = runner.invoke(app, ["tasks", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    lease = data["tasks"][0]["lease"]
    assert lease is not None
    assert lease["owner"] == "agent:codex"
    assert lease["agent"] == "codex"
    assert lease["expired"] is False
    assert "lease_token" not in lease

    res = runner.invoke(app, ["tasks"])
    assert res.exit_code == 0
    assert "agent:codex" in res.output


def test_cli_tasks_cancel(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    
    # Cancel task
    res = runner.invoke(app, ["tasks", "cancel", "TASK-1", "--json"])
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True
    assert data["status"] == "cancelled"
    
    # Try cancelling again (should fail)
    res2 = runner.invoke(app, ["tasks", "cancel", "TASK-1", "--json"])
    assert res2.exit_code != 0
    data2 = json.loads(res2.output)
    assert data2["ok"] is False


def test_cli_tasks_reprioritize(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    
    # Set to high
    res = runner.invoke(app, ["tasks", "reprioritize", "TASK-1", "--priority", "high", "--json"])
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True
    assert data["priority"] == "high"
    
    # Invalid priority
    res2 = runner.invoke(app, ["tasks", "reprioritize", "TASK-1", "--priority", "invalid"])
    assert res2.exit_code != 0


def test_cli_tasks_edit(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    
    # Edit title & desc
    res = runner.invoke(app, ["tasks", "edit", "TASK-1", "--title", "New Title", "--description", "New Desc", "--json"])
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["ok"] is True
    assert data["changes"]["title"]["to"] == "New Title"
    
    # Edit nothing (should fail)
    res2 = runner.invoke(app, ["tasks", "edit", "TASK-1"])
    assert res2.exit_code != 0


def test_cli_tasks_no_db_errors(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    import devcouncil.cli.commands.tasks as tasks_cmd

    monkeypatch.setattr(tasks_cmd, "get_db", lambda root: None)

    res = runner.invoke(app, ["tasks"])
    assert res.exit_code == 1
    assert "state is unavailable" in res.output


def test_cli_tasks_empty_list_text_mode(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    import devcouncil.cli.commands.tasks as tasks_cmd

    class _Repo:
        def get_all(self):
            return []

    class _Session:
        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

    class _Db:
        def get_session(self):
            return _Session()

    monkeypatch.setattr(tasks_cmd, "get_db", lambda root: _Db())
    monkeypatch.setattr(tasks_cmd, "TaskRepository", lambda session: _Repo())
    monkeypatch.setattr(tasks_cmd, "_active_leases_by_task", lambda session: {})

    res = runner.invoke(app, ["tasks"])
    assert res.exit_code == 0
    assert "No tasks found" in res.output


def test_cli_tasks_status_filter_and_pagination(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.chdir(tmp_path)
    runner.invoke(app, ["init"])
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        repo = TaskRepository(session)
        repo.save(Task(id="TASK-A", title="A", description="d", status="planned"))
        repo.save(Task(id="TASK-B", title="B", description="d", status="done"))

    res = runner.invoke(app, ["tasks", "--status", "done", "--json", "--limit", "1", "--offset", "0"])
    assert res.exit_code == 0
    data = json.loads(res.output)
    assert data["total"] == 1
    assert data["tasks"][0]["id"] == "TASK-B"


def test_cli_tasks_cancel_not_found_text_mode(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    res = runner.invoke(app, ["tasks", "cancel", "MISSING"])
    assert res.exit_code == 1
    assert "not found" in res.output


def test_cli_tasks_cancel_already_done_text_mode(tmp_path, monkeypatch):
    tmp_path, task_id = _setup_tasks_db(tmp_path, monkeypatch)
    db = Database(tmp_path / ".devcouncil" / "state.sqlite")
    with db.get_session() as session:
        task = TaskRepository(session).get_by_id(task_id)
        task.status = "done"
        TaskRepository(session).save(task)

    res = runner.invoke(app, ["tasks", "cancel", task_id])
    assert res.exit_code == 2
    assert "cannot be cancelled" in res.output


def test_cli_tasks_cancel_no_db_json(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    import devcouncil.cli.commands.tasks as tasks_cmd

    monkeypatch.setattr(tasks_cmd, "get_db", lambda root: None)
    res = runner.invoke(app, ["tasks", "cancel", "TASK-1", "--json"])
    assert res.exit_code == 1
    assert json.loads(res.output)["ok"] is False


def test_cli_tasks_reprioritize_invalid_json(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    res = runner.invoke(app, ["tasks", "reprioritize", "TASK-1", "--priority", "urgent", "--json"])
    assert res.exit_code == 2
    assert json.loads(res.output)["ok"] is False


def test_cli_tasks_reprioritize_not_found(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    res = runner.invoke(app, ["tasks", "reprioritize", "MISSING", "--priority", "high", "--json"])
    assert res.exit_code == 1


def test_cli_tasks_edit_text_mode_success(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    res = runner.invoke(app, ["tasks", "edit", "TASK-1", "--title", "Edited"])
    assert res.exit_code == 0
    assert "Updated TASK-1" in res.output


def test_cli_tasks_edit_no_db(tmp_path, monkeypatch):
    _setup_tasks_db(tmp_path, monkeypatch)
    import devcouncil.cli.commands.tasks as tasks_cmd

    monkeypatch.setattr(tasks_cmd, "get_db", lambda root: None)
    res = runner.invoke(app, ["tasks", "edit", "TASK-1", "--title", "X", "--json"])
    assert res.exit_code == 1
