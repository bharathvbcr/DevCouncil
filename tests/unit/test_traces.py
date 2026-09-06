import json

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.telemetry.traces import TRACE_SCHEMA_VERSION, TraceLogger, read_trace_events


runner = CliRunner()


def test_trace_logger_writes_stable_jsonl(tmp_path):
    event = TraceLogger(tmp_path).log_event(
        "task_verified",
        {"task_id": "TASK-001"},
        run_id="run-1",
        task_id="TASK-001",
        summary="verified",
    )

    raw = json.loads((tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8"))
    assert raw["schema"] == TRACE_SCHEMA_VERSION
    assert raw["type"] == "task_verified"
    assert raw["task_id"] == "TASK-001"
    assert event.summary == "verified"


def test_read_trace_events_accepts_legacy_lines(tmp_path):
    trace_file = tmp_path / ".devcouncil" / "logs" / "traces.jsonl"
    trace_file.parent.mkdir(parents=True)
    trace_file.write_text(
        json.dumps({"type": "gate_failed", "run_id": "run-1", "details": {"task_id": "TASK-001"}}),
        encoding="utf-8",
    )

    events = list(read_trace_events(tmp_path))

    assert len(events) == 1
    assert events[0].type == "gate_failed"
    assert events[0].task_id == "TASK-001"


def test_trace_tail_jsonl_remains_one_json_object_per_line(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    TraceLogger(tmp_path).log_event(
        "task_verified",
        {"message": "x" * 300},
        task_id="TASK-001",
        summary="x" * 300,
    )

    result = runner.invoke(app, ["trace", "tail", "--limit", "1"])

    assert result.exit_code == 0
    lines = [line for line in result.stdout.splitlines() if line.strip()]
    assert len(lines) == 1
    assert json.loads(lines[0])["task_id"] == "TASK-001"


def test_trace_tail_jsonl_cursor_line_stays_off_stdout(tmp_path, monkeypatch):
    """`--jsonl` makes stdout a newline-delimited JSON stream; the cursor line is not a record.

    `--json` returns before this point and was never affected — but the same defect is
    here in the sibling mode: a human line as the last thing a consumer reads.
    """
    monkeypatch.chdir(tmp_path)
    assert runner.invoke(app, ["init"]).exit_code == 0
    TraceLogger(tmp_path).log_event("task_verified", {"task_id": "TASK-001"}, summary="verified")

    result = runner.invoke(app, ["trace", "tail", "--since", "0", "--jsonl"])

    assert result.exit_code == 0
    lines = [line for line in result.stdout.splitlines() if line.strip()]
    assert lines, "expected at least one JSONL record on stdout"
    for line in lines:
        json.loads(line)
    assert "next_cursor" in result.stderr
    assert "next_cursor" not in result.stdout
