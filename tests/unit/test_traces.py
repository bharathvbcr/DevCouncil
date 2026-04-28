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
    lines = [line for line in result.output.splitlines() if line.strip()]
    assert len(lines) == 1
    assert json.loads(lines[0])["task_id"] == "TASK-001"
