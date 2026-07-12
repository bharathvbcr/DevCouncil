"""Helper-level coverage for hook.py — path extraction, lock/queue mechanics,
the queue+drain map refresh, status line, and post-task verification."""

from __future__ import annotations

import json
import os
import time
from pathlib import Path

from typer.testing import CliRunner

import devcouncil.cli.commands.hook as hook
from devcouncil.cli.main import app

runner = CliRunner()


# --- _extract_written_paths -------------------------------------------------------


def test_extract_written_paths_non_dict():
    assert hook._extract_written_paths("nope") == []


def test_extract_written_paths_direct_keys_and_list():
    payload = {"file_path": ["a.py", " b.py "]}
    assert hook._extract_written_paths(payload) == ["a.py", "b.py"]


def test_extract_written_paths_tool_input_dict():
    payload = {"tool_name": "Write", "tool_input": {"file_path": "./src/a.py"}}
    assert hook._extract_written_paths(payload) == ["src/a.py"]


def test_extract_written_paths_tool_input_json_string():
    payload = {"tool_input": json.dumps({"path": "x.py"})}
    assert "x.py" in hook._extract_written_paths(payload)


def test_extract_written_paths_edits_list():
    payload = {"edits": [{"file_path": "e1.py"}, {"file_path": "e2.py"}]}
    assert hook._extract_written_paths(payload) == ["e1.py", "e2.py"]


def test_extract_written_paths_bad_tool_input_json():
    payload = {"tool_input": "{not json"}
    assert hook._extract_written_paths(payload) == []


# --- _pid_alive -------------------------------------------------------------------


def test_pid_alive_variants():
    assert hook._pid_alive(0) is False
    assert hook._pid_alive(os.getpid()) is True
    # A very high pid is almost certainly not running.
    assert hook._pid_alive(2_000_000_000) is False


# --- _lock_is_reclaimable ---------------------------------------------------------


def test_lock_reclaimable_when_missing(tmp_path):
    assert hook._lock_is_reclaimable(tmp_path / "nope.lock") is True


def test_lock_reclaimable_when_pid_dead(tmp_path):
    lock = tmp_path / "l.lock"
    lock.write_text(json.dumps({"pid": 2_000_000_000, "started_at": time.time()}), encoding="utf-8")
    assert hook._lock_is_reclaimable(lock) is True


def test_lock_not_reclaimable_when_fresh_and_alive(tmp_path):
    lock = tmp_path / "l.lock"
    lock.write_text(json.dumps({"pid": os.getpid(), "started_at": time.time()}), encoding="utf-8")
    assert hook._lock_is_reclaimable(lock) is False


def test_lock_reclaimable_when_stale(tmp_path):
    lock = tmp_path / "l.lock"
    lock.write_text(json.dumps({"pid": os.getpid(), "started_at": 1.0}), encoding="utf-8")
    assert hook._lock_is_reclaimable(lock, now=time.time()) is True


# --- _try_acquire_refresh_lock ----------------------------------------------------


def test_acquire_lock_fresh(tmp_path):
    lock = tmp_path / "cache" / "m.lock"
    assert hook._try_acquire_refresh_lock(lock) is True
    assert lock.exists()


def test_acquire_lock_blocked_when_held_by_alive(tmp_path):
    lock = tmp_path / "m.lock"
    lock.write_text(json.dumps({"pid": os.getpid(), "started_at": time.time()}), encoding="utf-8")
    assert hook._try_acquire_refresh_lock(lock) is False


def test_acquire_lock_reclaims_stale(tmp_path):
    lock = tmp_path / "m.lock"
    lock.write_text(json.dumps({"pid": os.getpid(), "started_at": 1.0}), encoding="utf-8")
    assert hook._try_acquire_refresh_lock(lock) is True


# --- _enqueue_refresh_paths / _take_queued_paths ----------------------------------


def test_enqueue_and_take_queue(tmp_path):
    queue = tmp_path / "q.json"
    hook._enqueue_refresh_paths(queue, ["a.py", "b.py"])
    hook._enqueue_refresh_paths(queue, ["b.py", "c.py"])  # dedupe b.py
    assert hook._parse_queue_file(queue) == ["a.py", "b.py", "c.py"]
    taken = hook._take_queued_paths(queue)
    assert set(taken) == {"a.py", "b.py", "c.py"}
    assert not queue.exists()


def test_enqueue_over_existing_list_payload(tmp_path):
    queue = tmp_path / "q.json"
    queue.write_text(json.dumps(["x.py"]), encoding="utf-8")
    hook._enqueue_refresh_paths(queue, ["y.py"])
    assert set(hook._parse_queue_file(queue)) == {"x.py", "y.py"}


# --- _maybe_refresh_map -----------------------------------------------------------


def test_maybe_refresh_map_disabled(tmp_path, monkeypatch):
    import devcouncil.app.config as config_mod
    from types import SimpleNamespace
    monkeypatch.setattr(
        config_mod, "load_config",
        lambda root: SimpleNamespace(indexing=SimpleNamespace(auto_refresh=False)),
    )
    # Returns without doing anything (no exception).
    hook._maybe_refresh_map(tmp_path, json.dumps({"file_path": "a.py"}))


def test_maybe_refresh_map_invalid_json(tmp_path):
    hook._maybe_refresh_map(tmp_path, "{not json")


def test_maybe_refresh_map_no_code_paths(tmp_path):
    hook._maybe_refresh_map(tmp_path, json.dumps({"file_path": "README.txt"}))


def test_maybe_refresh_map_too_many_paths(tmp_path, monkeypatch):
    import devcouncil.app.config as config_mod
    from types import SimpleNamespace
    monkeypatch.setattr(
        config_mod, "load_config",
        lambda root: SimpleNamespace(indexing=SimpleNamespace(auto_refresh=True, auto_refresh_max_files=1)),
    )
    payload = {"edits": [{"file_path": "a.py"}, {"file_path": "b.py"}]}
    hook._maybe_refresh_map(tmp_path, json.dumps(payload))


def test_maybe_refresh_map_success(tmp_path, monkeypatch):
    import devcouncil.indexing.graph.build as graph_build
    monkeypatch.setattr(time, "sleep", lambda s: None)
    refreshed = {}
    monkeypatch.setattr(
        graph_build, "refresh_map_for_paths",
        lambda root, batch: refreshed.setdefault("batch", list(batch)),
    )
    hook._maybe_refresh_map(tmp_path, json.dumps({"tool_name": "Write", "file_path": "src/a.py"}))
    assert refreshed["batch"] == ["src/a.py"]


def test_maybe_refresh_map_queues_when_locked(tmp_path, monkeypatch):
    # Pre-hold the lock so acquisition fails and paths are enqueued.
    lock = tmp_path / hook._MAP_REFRESH_LOCK_REL
    lock.parent.mkdir(parents=True, exist_ok=True)
    lock.write_text(json.dumps({"pid": os.getpid(), "started_at": time.time()}), encoding="utf-8")
    hook._maybe_refresh_map(tmp_path, json.dumps({"file_path": "src/a.py"}))
    queue = tmp_path / hook._MAP_REFRESH_QUEUE_REL
    assert "src/a.py" in hook._parse_queue_file(queue)


# --- _status_line -----------------------------------------------------------------


def test_status_line_after_init(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    line = hook._status_line(tmp_path)
    assert line is not None
    assert "DevCouncil" in line


# --- _verify_active_task ----------------------------------------------------------


def test_verify_active_task_no_active(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(hook, "active_task_id", lambda root: None)
    assert "dev verify" in hook._verify_active_task(tmp_path)


def test_verify_active_task_verified(tmp_path, monkeypatch):
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository
    from devcouncil.verification.verifier import Verifier

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="t", description="d", status="running"))
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")

    async def fake_verify(self, task, reqs):
        return [], []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify)
    result = hook._verify_active_task(tmp_path)
    assert "verified" in result.lower()


def test_verify_active_task_blocked(tmp_path, monkeypatch):
    from devcouncil.domain.gap import Gap
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository
    from devcouncil.verification.verifier import Verifier

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="t", description="d", status="running"))
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")

    async def fake_verify(self, task, reqs):
        gap = Gap(
            id="G1", severity="high", gap_type="missing_test", description="no test",
            blocking=True, recommended_fix="test", task_id="TASK-1",
        )
        return [gap], []

    monkeypatch.setattr(Verifier, "verify_task", fake_verify)
    result = hook._verify_active_task(tmp_path)
    assert "blocked" in result.lower()


def test_verify_active_task_swallows_errors(tmp_path, monkeypatch):
    def boom(root):
        raise RuntimeError("db exploded")

    monkeypatch.setattr(hook, "active_task_id", boom)
    result = hook._verify_active_task(tmp_path)
    assert "skipped" in result.lower()


# --- session hooks emitting context (status line present) -------------------------


def test_session_start_and_user_prompt_inject_context(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    ss = runner.invoke(app, ["hook", "session-start", "{}", "--project-root", str(tmp_path)])
    assert ss.exit_code == 0
    assert "SessionStart" in ss.output
    ups = runner.invoke(app, ["hook", "user-prompt-submit", "{}", "--project-root", str(tmp_path)])
    assert ups.exit_code == 0
    assert "UserPromptSubmit" in ups.output


def test_claude_statusline_prints_status(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    result = runner.invoke(
        app, ["hook", "claude-statusline", json.dumps({"cwd": str(tmp_path)}), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "DevCouncil" in result.output


# --- _active_task -----------------------------------------------------------------


def test_active_task_none_when_no_active(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: None)
    assert hook._active_task(tmp_path) is None


def test_active_task_none_when_db_missing(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")
    monkeypatch.setattr(hook, "get_db", lambda root: None)
    assert hook._active_task(tmp_path) is None


def test_active_task_returns_task(tmp_path, monkeypatch):
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="t", description="d", status="running"))
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")
    task = hook._active_task(tmp_path)
    assert task.id == "TASK-1"


# --- _read_lock_meta: legacy parse failures ---------------------------------------


def test_read_lock_meta_legacy_bad_pid(tmp_path):
    lock = tmp_path / "l"
    lock.write_text("notanint\n", encoding="utf-8")
    assert hook._read_lock_meta(lock) == (None, None)


def test_read_lock_meta_legacy_bad_timestamp(tmp_path):
    lock = tmp_path / "l"
    lock.write_text("42\nnotafloat\n", encoding="utf-8")
    assert hook._read_lock_meta(lock) == (42, None)


# --- _maybe_refresh_map: absolute path + refresh exception + leftover drain --------


def test_maybe_refresh_map_absolute_path(tmp_path, monkeypatch):
    import devcouncil.indexing.graph.build as graph_build
    monkeypatch.setattr(time, "sleep", lambda s: None)
    refreshed = {}
    monkeypatch.setattr(
        graph_build, "refresh_map_for_paths",
        lambda root, batch: refreshed.setdefault("batch", list(batch)),
    )
    abs_path = str((tmp_path / "src" / "a.py"))
    hook._maybe_refresh_map(tmp_path, json.dumps({"file_path": abs_path}))
    assert refreshed["batch"] == ["src/a.py"]


def test_maybe_refresh_map_refresh_exception_is_swallowed(tmp_path, monkeypatch):
    import devcouncil.indexing.graph.build as graph_build
    monkeypatch.setattr(time, "sleep", lambda s: None)

    def boom(root, batch):
        raise RuntimeError("refresh failed")

    monkeypatch.setattr(graph_build, "refresh_map_for_paths", boom)
    # Must not raise — the finally-block cleans up the lock.
    hook._maybe_refresh_map(tmp_path, json.dumps({"file_path": "src/a.py"}))
    assert not (tmp_path / hook._MAP_REFRESH_LOCK_REL).exists()


# --- agent_response: invalid-json payload injects active task ---------------------


def test_agent_response_invalid_json_injects_task(tmp_path, monkeypatch):
    from types import SimpleNamespace
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-7")
    seen = {}
    monkeypatch.setattr(
        hook, "write_signal",
        lambda root, client, payload: seen.setdefault("payload", payload) or (tmp_path / "s.json"),
    )
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))
    result = runner.invoke(app, ["hook", "agent-response", "{not json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert seen["payload"]["task_id"] == "TASK-7"
    assert seen["payload"]["raw"] == "{not json"


# --- subagent_stop: signal write failure is contained -----------------------------


def test_subagent_stop_signal_error_is_logged(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: None)

    def boom(*a, **k):
        raise OSError("cannot write")

    monkeypatch.setattr(hook, "write_signal", boom)
    result = runner.invoke(app, ["hook", "subagent-stop", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


# --- trace-logging failures never break a hook ------------------------------------


def test_session_end_trace_failure_is_swallowed(tmp_path, monkeypatch):
    def boom(root):
        raise RuntimeError("trace down")

    monkeypatch.setattr(hook, "TraceLogger", boom)
    result = runner.invoke(
        app, ["hook", "session-end", json.dumps({"session_id": "s", "reason": "done"}), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0


def test_pre_compact_trace_failure_is_swallowed(tmp_path, monkeypatch):
    def boom(root):
        raise RuntimeError("trace down")

    monkeypatch.setattr(hook, "TraceLogger", boom)
    result = runner.invoke(app, ["hook", "pre-compact", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_notification_trace_failure_is_swallowed(tmp_path, monkeypatch):
    def boom(root):
        raise RuntimeError("trace down")

    monkeypatch.setattr(hook, "TraceLogger", boom)
    result = runner.invoke(
        app, ["hook", "notification", json.dumps({"message": "hi"}), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0


# --- _verify_active_task: task missing + evidence persistence ---------------------


def test_verify_active_task_missing_task(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    monkeypatch.setattr(hook, "active_task_id", lambda root: "MISSING")
    assert "dev verify" in hook._verify_active_task(tmp_path)


def test_verify_active_task_persists_all_evidence(tmp_path, monkeypatch):
    from devcouncil.domain.evidence import (
        CommandResult,
        DiffCoverageEvidence,
        DiffEvidence,
        TestEvidence,
    )
    from devcouncil.domain.task import Task
    from devcouncil.storage.db import get_db
    from devcouncil.storage.repositories import TaskRepository
    from devcouncil.verification.verifier import Verifier

    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    db = get_db(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(Task(id="TASK-1", title="t", description="d", status="running"))
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-1")

    async def fake_verify(self, task, reqs):
        evidence = [
            CommandResult(command="pytest", exit_code=0, stdout_path="", stderr_path="", summary="ok"),
            DiffCoverageEvidence(task_id="TASK-1", tool="coverage", measured=True),
            DiffEvidence(task_id="TASK-1", changed_files=["a.py"], added_files=[], deleted_files=[], diff_summary="s"),
            TestEvidence(requirement_id="R1", acceptance_criterion_id="AC1", command="pytest", status="passed", evidence_summary="ok"),
        ]
        return [], evidence

    monkeypatch.setattr(Verifier, "verify_task", fake_verify)
    result = hook._verify_active_task(tmp_path)
    assert "verified" in result.lower()
