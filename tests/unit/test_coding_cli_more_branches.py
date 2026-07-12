import json
import queue
import subprocess
import sys
from pathlib import Path
import pytest
from rich.console import Console

from devcouncil.executors.coding_cli import CodingCliExecutor
from devcouncil.domain.task import Task
from devcouncil.app.config import DevCouncilConfig
from devcouncil.verification.verifier import Verifier
from devcouncil.execution.policy_engine import TaskPolicyEngine


def test_scope_enforcement_logic(tmp_path, monkeypatch):
    ex = CodingCliExecutor(tmp_path, "codex")
    
    # 1. Enable scope enforcement
    monkeypatch.setattr(ex, "_scope_enforcement_enabled", lambda: True)
    
    # Mock Verifier
    monkeypatch.setattr(Verifier, "get_task_changed_files", lambda self, tid: ["src/a.py", "src/b.py"])
    
    # Mock TaskPolicyEngine decision
    class MockDecision:
        def __init__(self, action, reason):
            self.action = action
            self.reason = reason
            
    def mock_eval(self, path, task, action):
        if path == "src/a.py":
            return MockDecision("deny", "out of scope")
        return MockDecision("allow", "")
        
    monkeypatch.setattr(TaskPolicyEngine, "evaluate_file_change", mock_eval)
    
    # Mock _revert_path
    reverted_paths = []
    monkeypatch.setattr(ex, "_revert_path", lambda path: reverted_paths.append(path) or True)
    
    task = Task(id="TASK-1", title="T", description="D")
    reverted = ex._enforce_file_scope(task)
    
    assert reverted == [("src/a.py", "out of scope")]
    assert reverted_paths == ["src/a.py"]
    
    # 2. Test get_task_changed_files raises exception
    def bad_get_changed_files(self, tid):
        raise ValueError("Git error")
    monkeypatch.setattr(Verifier, "get_task_changed_files", bad_get_changed_files)
    
    with pytest.raises(RuntimeError, match="could not determine changed files"):
        ex._enforce_file_scope(task)
        
    # 3. Test evaluate_file_change raises exception (should skip and continue)
    monkeypatch.setattr(Verifier, "get_task_changed_files", lambda self, tid: ["src/a.py"])
    def bad_eval(self, path, task, action):
        raise ValueError("Policy error")
    monkeypatch.setattr(TaskPolicyEngine, "evaluate_file_change", bad_eval)
    
    reverted_paths.clear()
    reverted = ex._enforce_file_scope(task)
    assert reverted == []


def test_emit_stream_line_unicode_encode_error(monkeypatch):
    # Mock console.print to raise UnicodeEncodeError on first call
    calls = []
    def mock_print(*args, **kwargs):
        # The first argument is self (Console instance) when mocked on the class
        val = args[1] if len(args) > 1 else ""
        if not isinstance(val, str):
            val = str(val)
        calls.append(val)
        if len(calls) == 1:
            raise UnicodeEncodeError("ascii", val, 0, 1, "mock error")
            
    monkeypatch.setattr(Console, "print", mock_print)
    
    CodingCliExecutor._emit_stream_line("hello ✓")
    assert len(calls) == 2
    assert "hello" in calls[1]


def test_run_subprocess_timeout(tmp_path, monkeypatch):
    ex = CodingCliExecutor(tmp_path, "codex")
    
    # Run a slow process (e.g. sleep 10) with timeout 0.01 inside self._effective_timeout
    monkeypatch.setattr(ex, "_effective_timeout", lambda: 0)
    
    invocation = ["sleep", "5"]
    env = {}
    
    with pytest.raises(subprocess.TimeoutExpired):
        ex._run_subprocess(invocation, input_text=None, env=env)


def test_cursor_chat_id_json_failures(tmp_path, monkeypatch):
    ex = CodingCliExecutor(tmp_path, "cursor")
    
    # Mock resume mode to task
    monkeypatch.setattr(ex, "_cursor_resume_mode", lambda: "task")
    
    # Mock session path
    session_file = tmp_path / "session.json"
    monkeypatch.setattr(ex, "_cursor_session_path", lambda task_id: session_file)
    
    # 1. File contains invalid JSON
    session_file.write_text("invalid json", encoding="utf-8")
    monkeypatch.setattr(ex, "_ensure_cursor_chat_id", lambda: "new-chat-id")
    
    chat_id = ex._cursor_resume_chat_id("TASK-1")
    assert chat_id == "new-chat-id"
    
    # 2. File contains valid JSON
    session_file.write_text('{"chat_id": "existing-chat-id"}', encoding="utf-8")
    chat_id2 = ex._cursor_resume_chat_id("TASK-1")
    assert chat_id2 == "existing-chat-id"


def test_parse_grok_session_id(tmp_path, monkeypatch):
    ex = CodingCliExecutor(tmp_path, "grok")
    
    # Make sure Grok resume mode is on
    monkeypatch.setattr(ex, "_grok_resume_mode", lambda: "task")
    
    # 1. Invalid JSON
    proc = subprocess.CompletedProcess(args=[], returncode=0, stdout="not json")
    ex._capture_grok_session_from_result("TASK-1", proc)
    assert ex.last_agent_session_id is None
    
    # 2. Valid JSON not line-by-line but overall
    proc2 = subprocess.CompletedProcess(args=[], returncode=0, stdout='{"session_id": "grok-123"}')
    ex._capture_grok_session_from_result("TASK-1", proc2)
    assert ex.last_agent_session_id == "grok-123"


def test_capture_claude_json(tmp_path):
    ex = CodingCliExecutor(tmp_path, "claude")
    
    # 1. Invalid JSON
    proc = subprocess.CompletedProcess(args=[], returncode=0, stdout="invalid json")
    res = ex._capture_claude_json("run-1", proc)
    assert res == proc
    
    # 2. Valid JSON
    proc2 = subprocess.CompletedProcess(args=[], returncode=0, stdout='{"result": "success text", "session_id": "c1"}')
    res2 = ex._capture_claude_json("run-1", proc2)
    assert res2.stdout == "success text"


def test_update_run_manifest_json_failure(tmp_path):
    ex = CodingCliExecutor(tmp_path, "codex")
    
    manifest_dir = tmp_path / ".devcouncil" / "runs" / "run-1"
    manifest_dir.mkdir(parents=True, exist_ok=True)
    manifest_file = manifest_dir / "agent-run.json"
    manifest_file.write_text("invalid json", encoding="utf-8")
    
    # Should not throw
    ex._update_run_manifest("run-1", key="value")
