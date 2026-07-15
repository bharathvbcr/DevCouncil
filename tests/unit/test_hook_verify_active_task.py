from types import SimpleNamespace

from devcouncil.cli.commands import hook
from devcouncil.cli.commands.hook import _verify_active_task
from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap
from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence, DiffEvidence, TestEvidence
from devcouncil.storage.db import Database, reset_db_cache
from devcouncil.storage.repositories import TaskRepository
from devcouncil.verification.verifier import Verifier


def test_verify_active_task_no_active_id(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: None)
    res = _verify_active_task(tmp_path)
    assert "finalize implementation evidence" in res


def test_verify_active_task_db_missing(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")
    monkeypatch.setattr(hook, "get_db", lambda root: None)
    res = _verify_active_task(tmp_path)
    assert "finalize implementation evidence" in res


def test_verify_active_task_task_not_found(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")
    
    db_path = tmp_path / ".devcouncil" / "state.sqlite"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    db = Database(db_path)
    db.create_db_and_tables()
    monkeypatch.setattr(hook, "get_db", lambda root: db)
    
    # Task not added to repo
    res = _verify_active_task(tmp_path)
    assert "finalize implementation evidence" in res


def test_verify_active_task_success_no_gaps(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")
    
    db_path = tmp_path / ".devcouncil" / "state.sqlite"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    db = Database(db_path)
    db.create_db_and_tables()
    task = Task(id="TASK-1", title="T", description="D", status="running")
    with db.get_session() as session:
        TaskRepository(session).save(task)
        
    monkeypatch.setattr(hook, "get_db", lambda root: db)
    
    # Mock Verifier
    async def fake_verify_task(self, t, reqs):
        evidence = [
            CommandResult(command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok"),
            DiffCoverageEvidence(task_id="TASK-1", tool="pytest-cov", measured=True, changed_lines=10, covered_lines=10, summary="100% coverage"),
            DiffEvidence(task_id="TASK-1", changed_files=["src/a.py"], added_files=[], deleted_files=[], diff_summary="diff"),
            TestEvidence(requirement_id="REQ-1", acceptance_criterion_id="CRIT-1", command="pytest", status="passed", evidence_summary="passed"),
        ]
        return [], evidence
        
    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    
    # Mock TraceLogger
    monkeypatch.setattr(hook, "TraceLogger", lambda r: SimpleNamespace(log_event=lambda *a, **k: None))
    
    res = _verify_active_task(tmp_path)
    assert "TASK-1 verified" in res
    
    # Verify status in database is now verified
    with db.get_session() as session:
        t = TaskRepository(session).get_by_id("TASK-1")
        assert t.status == "verified"


def test_verify_active_task_blocked_with_gaps(tmp_path, monkeypatch):
    reset_db_cache()
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")
    
    db_path = tmp_path / ".devcouncil" / "state.sqlite"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    db = Database(db_path)
    db.create_db_and_tables()
    task = Task(id="TASK-1", title="T", description="D", status="running")
    with db.get_session() as session:
        TaskRepository(session).save(task)
        
    monkeypatch.setattr(hook, "get_db", lambda root: db)
    
    # Mock Verifier to return a blocking gap
    async def fake_verify_task(self, t, reqs):
        gap = Gap(
            id="GAP-1",
            severity="high",
            gap_type="missing_test",
            description="Missing tests",
            blocking=True,
            recommended_fix="Write tests",
            task_id="TASK-1",
        )
        return [gap], []
        
    monkeypatch.setattr(Verifier, "verify_task", fake_verify_task)
    monkeypatch.setattr(hook, "TraceLogger", lambda r: SimpleNamespace(log_event=lambda *a, **k: None))
    
    res = _verify_active_task(tmp_path)
    assert "blocked by 1 gap" in res
    
    # Verify status in database is now blocked
    with db.get_session() as session:
        t = TaskRepository(session).get_by_id("TASK-1")
        assert t.status == "blocked"


def test_verify_active_task_raises_exception(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")
    monkeypatch.setattr(hook, "get_db", lambda root: 123)  # raises AttributeError on get_session
    
    # Should catch exception and return a dim skipped message
    res = _verify_active_task(tmp_path)
    assert "post-task verification skipped" in res
