"""Tests for Claude Code compaction survival hooks."""

from __future__ import annotations

import json
import subprocess
from datetime import datetime, timezone

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import PlannedFile, Task
from devcouncil.execution.stop_gate import (
    build_compact_snapshot,
    compact_briefing,
    compact_snapshot_path,
    compact_snapshot_recent,
    read_compact_snapshot,
    session_briefing,
    write_json,
)
from devcouncil.integrations.clients.hooks import SESSION_START_MATCHER
from devcouncil.integrations import claude_assets
from devcouncil.storage.db import get_db
from devcouncil.storage.repositories import GapRepository, StateRepository, TaskRepository
from devcouncil.storage.models import ProjectStateModel
from devcouncil.telemetry.traces import TraceLogger

runner = CliRunner()


def _init_repo(tmp_path):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "config.yaml").write_text(
        "project:\n  name: test\nexecution:\n  compact_snapshot_toast: true\n",
        encoding="utf-8",
    )
    return get_db(tmp_path)


def _task(task_id: str, *, status: str = "running") -> Task:
    return Task(
        id=task_id,
        title="Implement feature",
        description="Do the thing",
        status=status,
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )


def _blocking_gap(task_id: str) -> Gap:
    return Gap(
        id="GAP-001",
        severity="high",
        gap_type="missing_test",
        task_id=task_id,
        description="No tests cover the changed module",
        recommended_fix="Add unit tests",
        blocking=True,
    )


def test_session_start_matcher_includes_compact_sources():
    assert "compact" in SESSION_START_MATCHER
    assert "clear" in SESSION_START_MATCHER


def test_plugin_hooks_use_session_start_matcher_and_post_compact(tmp_path):
    bundle = claude_assets.build_plugin_bundle(tmp_path, version="1.0.0")
    hooks = json.loads(next(a for a in bundle if a.path.name == "hooks.json").content)
    session_start = hooks["hooks"]["SessionStart"][0]
    assert session_start["matcher"] == SESSION_START_MATCHER
    assert "PostCompact" in hooks["hooks"]
    assert "PreCompact" in hooks["hooks"]


def test_write_json_and_read_compact_snapshot(tmp_path):
    _init_repo(tmp_path)
    snapshot = {"written_at": datetime.now(timezone.utc).isoformat(), "phase": "PLAN_APPROVED"}
    path = compact_snapshot_path(tmp_path)
    write_json(path, snapshot)
    assert path.exists()
    assert read_compact_snapshot(tmp_path) == snapshot


def test_pre_compact_writes_snapshot_and_optional_toast(tmp_path):
    db = _init_repo(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(_task("TASK-001"))
        StateRepository(session).save_state(ProjectStateModel(current_phase="TASK_EXECUTING"))
        GapRepository(session).save(_blocking_gap("TASK-001"))

    result = runner.invoke(
        app,
        ["hook", "pre-compact", json.dumps({"session_id": "sess-1"}), "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    snapshot = read_compact_snapshot(tmp_path)
    assert snapshot is not None
    assert snapshot["session_id"] == "sess-1"
    assert snapshot["task_id"] == "TASK-001"
    assert snapshot["phase"] == "TASK_EXECUTING"
    assert "blocking gap" in (snapshot.get("blocking_gaps_summary") or "").lower()
    payload = json.loads(result.stdout)
    assert payload["hookSpecificOutput"]["hookEventName"] == "PreCompact"
    assert "compact snapshot" in payload["hookSpecificOutput"]["systemMessage"].lower()
    assert "additionalContext" not in payload["hookSpecificOutput"]


def test_pre_compact_never_blocks_when_db_missing(tmp_path):
    result = runner.invoke(app, ["hook", "pre-compact", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_session_start_compact_source_emits_slim_briefing(tmp_path):
    db = _init_repo(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(_task("TASK-001"))
        GapRepository(session).save(_blocking_gap("TASK-001"))

    write_json(
        compact_snapshot_path(tmp_path),
        build_compact_snapshot(tmp_path, {"session_id": "sess-1"}),
    )

    result = runner.invoke(
        app,
        ["hook", "session-start", json.dumps({"source": "compact", "session_id": "sess-1"}), "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    context = payload["hookSpecificOutput"]["additionalContext"]
    assert "post-compaction recovery" in context.lower()
    assert "TASK-001" in context
    assert "blocking gap" in context.lower()
    assert "Use the devcouncil_* MCP tools" not in context


def test_session_start_normal_source_emits_full_briefing(tmp_path):
    db = _init_repo(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(_task("TASK-001"))
        GapRepository(session).save(_blocking_gap("TASK-001"))

    result = runner.invoke(
        app,
        ["hook", "session-start", json.dumps({"source": "startup"}), "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0, result.output
    context = json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]
    assert "Use the devcouncil_* MCP tools" in context
    assert "Active task: TASK-001" in context


def test_user_prompt_submit_skips_status_after_recent_compact(tmp_path):
    _init_repo(tmp_path)
    write_json(
        compact_snapshot_path(tmp_path),
        {"written_at": datetime.now(timezone.utc).isoformat(), "phase": "PLAN_APPROVED"},
    )

    result = runner.invoke(
        app,
        ["hook", "user-prompt-submit", '{"prompt":"hi"}', "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    assert result.stdout.strip() == ""


def test_user_prompt_submit_emits_status_when_snapshot_stale(tmp_path):
    _init_repo(tmp_path)
    write_json(
        compact_snapshot_path(tmp_path),
        {"written_at": "2020-01-01T00:00:00+00:00", "phase": "PLAN_APPROVED"},
    )

    result = runner.invoke(
        app,
        ["hook", "user-prompt-submit", '{"prompt":"hi"}', "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["hookSpecificOutput"]["hookEventName"] == "UserPromptSubmit"
    assert "DevCouncil" in payload["hookSpecificOutput"]["additionalContext"]


def test_post_compact_is_trace_only(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["hook", "post-compact", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert result.stdout.strip() == ""
    trace = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "post_compact" in trace


def test_integrate_claude_hooks_include_post_compact_and_compact_matcher(tmp_path):
    _init_repo(tmp_path)
    result = runner.invoke(app, ["integrate", "hooks", "--tool", "claude", "--apply", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    settings = json.loads((tmp_path / ".claude" / "settings.local.json").read_text(encoding="utf-8"))
    assert "PostCompact" in settings["hooks"]
    session_start = settings["hooks"]["SessionStart"][0]
    assert session_start["matcher"] == SESSION_START_MATCHER


def test_compact_briefing_prefers_snapshot_over_db(tmp_path):
    db = _init_repo(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(_task("TASK-DB"))

    write_json(
        compact_snapshot_path(tmp_path),
        {
            "written_at": datetime.now(timezone.utc).isoformat(),
            "task_id": "TASK-SNAPSHOT",
            "phase": "TASK_EXECUTING",
            "blocking_gaps_summary": "1 blocking gap(s): from snapshot",
        },
    )
    brief = compact_briefing(tmp_path, {"source": "compact"})
    assert "TASK-SNAPSHOT" in (brief or "")
    assert "from snapshot" in (brief or "")


def test_compact_snapshot_recent_respects_window(tmp_path):
    _init_repo(tmp_path)
    write_json(
        compact_snapshot_path(tmp_path),
        {"written_at": datetime.now(timezone.utc).isoformat()},
    )
    assert compact_snapshot_recent(tmp_path, 60) is True
    assert compact_snapshot_recent(tmp_path, 0) is False


def test_session_briefing_includes_active_task_and_blocking_gaps(tmp_path):
    db = _init_repo(tmp_path)
    with db.get_session() as session:
        TaskRepository(session).save(_task("TASK-001"))
        GapRepository(session).save(_blocking_gap("TASK-001"))

    brief = session_briefing(tmp_path)
    assert brief is not None
    assert "Active task: TASK-001" in brief
    assert "blocking gap" in brief.lower()


def test_pre_compact_records_last_stop_gate_event(tmp_path):
    _init_repo(tmp_path)
    TraceLogger(tmp_path).log_event(
        "agent_response_ready",
        {"client": "claude"},
        task_id="TASK-001",
        summary="Claude response ready for critique-card review.",
    )
    snapshot = build_compact_snapshot(tmp_path, {"session_id": "sess-1"})
    stop_event = snapshot.get("last_stop_gate_event")
    assert stop_event is not None
    assert stop_event["type"] == "agent_response_ready"
    assert "critique-card" in stop_event["summary"]
