"""Companion improvements: stateless incremental trace polling and per-task
cost attribution."""

import json

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.llm.provider import _log_model_call
from devcouncil.telemetry.cost import UNATTRIBUTED, _model_calls_file, group_cost, read_cost_records
from devcouncil.telemetry.traces import TraceLogger, read_trace_events_since

runner = CliRunner()


@pytest.fixture(autouse=True)
def _isolated_cost_ledger(tmp_path, monkeypatch):
    """Each test gets its own model_calls ledger (session DEVCOUNCIL_LOG_DIR is shared)."""
    log_dir = tmp_path / ".devcouncil" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("DEVCOUNCIL_LOG_DIR", str(log_dir))


# --------------------------------------------------------------------------- #
# (1) Incremental trace polling
# --------------------------------------------------------------------------- #


def test_incremental_polling_returns_only_new_events(tmp_path):
    logger = TraceLogger(tmp_path)
    logger.log_event("e1", {}, task_id="T1", summary="first")
    logger.log_event("e2", {}, task_id="T2", summary="second")

    # First poll from the start returns everything plus a cursor.
    events, cursor = read_trace_events_since(tmp_path, None)
    assert [e.type for e in events] == ["e1", "e2"]
    assert cursor > 0

    # Polling again at the same cursor with no new appends yields nothing and a
    # stable cursor (O(new) — nothing re-parsed).
    events, cursor2 = read_trace_events_since(tmp_path, cursor)
    assert events == []
    assert cursor2 == cursor

    # Append a new event; the next poll returns ONLY the new one.
    logger.log_event("e3", {}, task_id="T3", summary="third")
    events, cursor3 = read_trace_events_since(tmp_path, cursor2)
    assert [e.type for e in events] == ["e3"]
    assert cursor3 > cursor2


def test_incremental_polling_resets_on_truncation(tmp_path):
    logger = TraceLogger(tmp_path)
    # A few events so the cursor is comfortably past a freshly-rotated short file.
    for i in range(5):
        logger.log_event(f"e{i}", {"padding": "x" * 200})
    _, cursor = read_trace_events_since(tmp_path, None)
    assert cursor > 0

    # Simulate rotation/truncation, then a single new short line; the stale
    # cursor now points past EOF and reading must reset to the start.
    trace_file = tmp_path / ".devcouncil" / "logs" / "traces.jsonl"
    trace_file.write_text("", encoding="utf-8")
    logger.log_event("after_reset", {})
    assert cursor > trace_file.stat().st_size

    events, _ = read_trace_events_since(tmp_path, cursor)
    assert [e.type for e in events] == ["after_reset"]


def test_incremental_polling_missing_file_is_safe(tmp_path):
    events, cursor = read_trace_events_since(tmp_path, 123)
    assert events == []
    assert cursor == 123


def test_trace_tail_json_emits_events_and_next_cursor(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    TraceLogger(tmp_path).log_event("e1", {}, task_id="T1", summary="s1")

    result = runner.invoke(app, ["trace", "tail", "--json"])
    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert "events" in payload and "next_cursor" in payload
    assert payload["events"][0]["type"] == "e1"
    assert payload["events"][0]["task_id"] == "T1"
    cursor = payload["next_cursor"]

    # Re-poll with --since <cursor>: only-new (none yet).
    result = runner.invoke(app, ["trace", "tail", "--json", "--since", str(cursor)])
    assert result.exit_code == 0
    payload2 = json.loads(result.output)
    assert payload2["events"] == []
    assert payload2["next_cursor"] == cursor

    # Append and poll again — only the new event.
    TraceLogger(tmp_path).log_event("e2", {}, task_id="T2")
    result = runner.invoke(app, ["trace", "tail", "--json", "--since", str(cursor)])
    payload3 = json.loads(result.output)
    assert [e["type"] for e in payload3["events"]] == ["e2"]


# --------------------------------------------------------------------------- #
# (2) Per-task cost attribution
# --------------------------------------------------------------------------- #


def _log(project_root, *, model, prompt, completion, task_id=None, run_id=None):
    _log_model_call(
        {"model": model},
        {"model": model, "choices": []},
        {"prompt_tokens": prompt, "completion_tokens": completion},
        project_root,
        task_id=task_id,
        run_id=run_id,
    )


def test_cost_record_carries_task_and_run_id(tmp_path):
    _log(tmp_path, model="m", prompt=1000, completion=500, task_id="T1", run_id="R1")

    records = read_cost_records(tmp_path)
    assert len(records) == 1
    assert records[0]["task_id"] == "T1"
    assert records[0]["run_id"] == "R1"
    assert records[0]["timestamp"] is not None
    assert records[0]["cost"] > 0.0


def test_cost_record_defaults_to_none_when_unattributed(tmp_path):
    # Backward-compatible: omitting task_id/run_id leaves them None.
    _log(tmp_path, model="m", prompt=10, completion=10)
    records = read_cost_records(tmp_path)
    assert records[0]["task_id"] is None
    assert records[0]["run_id"] is None


def test_group_cost_aggregates_by_task_and_run(tmp_path):
    _log(tmp_path, model="m", prompt=1000, completion=0, task_id="T1", run_id="R1")
    _log(tmp_path, model="m", prompt=1000, completion=0, task_id="T1", run_id="R2")
    _log(tmp_path, model="m", prompt=1000, completion=0, task_id="T2", run_id="R1")
    _log(tmp_path, model="m", prompt=1000, completion=0)  # unattributed

    summary = group_cost(tmp_path)
    assert summary["total_calls"] == 4

    by_task = summary["by_task"]
    assert by_task["T1"]["calls"] == 2
    assert by_task["T2"]["calls"] == 1
    assert by_task[UNATTRIBUTED]["calls"] == 1
    # T1 had two calls so its cost is double a single-call bucket.
    assert by_task["T1"]["cost"] == 2 * by_task["T2"]["cost"]

    by_run = summary["by_run"]
    assert by_run["R1"]["calls"] == 2
    assert by_run["R2"]["calls"] == 1
    assert by_run[UNATTRIBUTED]["calls"] == 1

    # Grand total equals the sum of the per-task buckets.
    assert abs(summary["total_cost"] - sum(g["cost"] for g in by_task.values())) < 1e-9


def test_group_cost_handles_legacy_records_without_fields(tmp_path):
    log_file = _model_calls_file(tmp_path)
    log_file.parent.mkdir(parents=True, exist_ok=True)
    # Old-style record with no task_id/run_id/timestamp keys at all.
    log_file.write_text(
        json.dumps({"request": {}, "response": {"model": "m"}, "usage": {"prompt_tokens": 1000}}) + "\n",
        encoding="utf-8",
    )
    summary = group_cost(tmp_path)
    assert summary["total_calls"] == 1
    assert UNATTRIBUTED in summary["by_task"]
    assert summary["by_task"][UNATTRIBUTED]["calls"] == 1


def test_cost_show_command_json(tmp_path):
    # The `cost` Typer app is wired into main.py by the caller; invoke its
    # command app directly so this test is independent of that wiring.
    from devcouncil.cli.commands.cost import app as cost_app

    _log(tmp_path, model="m", prompt=1000, completion=0, task_id="T1", run_id="R1")

    # A single-command Typer app exposes that command directly (no subcommand name).
    result = runner.invoke(cost_app, ["show", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    payload = json.loads(result.stdout)
    assert payload["by_task"]["T1"]["calls"] == 1
    assert payload["by_run"]["R1"]["calls"] == 1


def test_status_includes_cost_by_task(tmp_path):
    from devcouncil.cli.commands.status import _status_payload

    _log(tmp_path, model="m", prompt=1000, completion=0, task_id="T1", run_id="R1")
    payload = _status_payload(tmp_path)
    assert "cost_by_task" in payload
    assert payload["cost_by_task"]["T1"]["calls"] == 1
