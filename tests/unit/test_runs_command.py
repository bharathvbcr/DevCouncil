"""CLI coverage for `dev runs` — list/show operate on on-disk manifests, while
timeline/diff/revert/supervise are exercised against a stubbed run-trace layer."""

import json
import time

from typer.testing import CliRunner

import devcouncil.cli.commands.runs as runs_cmd
import devcouncil.execution.run_trace as run_trace
from devcouncil.cli.main import app
from devcouncil.execution.run_trace import RunCheckpoint, RunTimeline, SupervisorVerdict

runner = CliRunner()


def _write_manifest(root, run_id, **fields):
    run_dir = root / ".devcouncil" / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    manifest = {"run_id": run_id, **fields}
    (run_dir / "agent-run.json").write_text(json.dumps(manifest), encoding="utf-8")
    return run_dir


# --- list -------------------------------------------------------------------------


def test_runs_list_empty(tmp_path):
    result = runner.invoke(app, ["runs", "list", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No agent runs found" in result.output


def test_runs_list_table(tmp_path):
    _write_manifest(tmp_path, "run-1", task_id="TASK-001", agent="claude", status="finished")
    result = runner.invoke(app, ["runs", "list", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "run-1" in result.output
    assert "TASK-001" in result.output


def test_runs_list_json_and_status_filter(tmp_path):
    _write_manifest(tmp_path, "run-1", task_id="T1", agent="claude", status="finished")
    _write_manifest(tmp_path, "run-2", task_id="T2", agent="codex", status="running")

    result = runner.invoke(
        app, ["runs", "list", "--json", "--status", "running", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["count"] == 1
    assert data["runs"][0]["run_id"] == "run-2"


def test_runs_list_marks_orphaned_running_manifest(tmp_path):
    run_dir = _write_manifest(tmp_path, "old", task_id="T1", agent="claude", status="running")
    old = time.time() - 100000
    import os

    os.utime(run_dir / "agent-run.json", (old, old))

    result = runner.invoke(app, ["runs", "list", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["runs"][0]["orphaned"] is True


# --- show -------------------------------------------------------------------------


def test_runs_show_missing_run_errors(tmp_path):
    result = runner.invoke(app, ["runs", "show", "nope", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "not found" in result.output


def test_runs_show_missing_run_json(tmp_path):
    result = runner.invoke(app, ["runs", "show", "nope", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert json.loads(result.stdout)["ok"] is False


def test_runs_show_with_transcript_tail(tmp_path):
    run_dir = _write_manifest(tmp_path, "run-1", task_id="T1", agent="claude", status="finished")
    (run_dir / "transcript.txt").write_text("line one\nline two\n", encoding="utf-8")

    result = runner.invoke(app, ["runs", "show", "run-1", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["ok"] is True
    assert "line two" in data["transcript_tail"]


def test_runs_show_human_no_transcript(tmp_path):
    _write_manifest(tmp_path, "run-1", task_id="T1", agent="claude", status="finished")
    result = runner.invoke(app, ["runs", "show", "run-1", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No transcript file found" in result.output


# --- timeline / diff / revert / supervise (stubbed run-trace) ---------------------


def _timeline(**kw):
    defaults = dict(
        run_id="run-1",
        task_id="TASK-001",
        manifest={"status": "finished", "returncode": 0},
        events=[],
        checkpoints=[RunCheckpoint(stage="before", ref="ref/before", sha="abc123def456")],
        diff_stat=" file.py | 2 +-",
        reversible=True,
    )
    defaults.update(kw)
    return RunTimeline(**defaults)


def test_runs_timeline_human(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())
    result = runner.invoke(app, ["runs", "timeline", "TASK-001", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Checkpoints" in result.output
    assert "Reversible" in result.output


def test_runs_timeline_json(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())
    result = runner.invoke(app, ["runs", "timeline", "TASK-001", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["run_id"] == "run-1"


def test_runs_timeline_load_error_exits(tmp_path, monkeypatch):
    def boom(root, ref, **k):
        raise ValueError("no such run")

    monkeypatch.setattr(run_trace, "load_timeline", boom)
    result = runner.invoke(app, ["runs", "timeline", "bad", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "no such run" in result.output


def test_runs_diff_outputs_diff(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())
    monkeypatch.setattr(run_trace, "diff_run", lambda root, task_id, stat_only=False: "diff --git a b")
    result = runner.invoke(app, ["runs", "diff", "TASK-001", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "diff --git" in result.output


def test_runs_diff_empty_exits_nonzero(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())
    monkeypatch.setattr(run_trace, "diff_run", lambda root, task_id, stat_only=False: "")
    result = runner.invoke(app, ["runs", "diff", "TASK-001", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No recorded diff" in result.output


def test_runs_revert_confirmed(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())

    class _Result:
        message = "Rolled back cleanly."

    monkeypatch.setattr(run_trace, "revert_run", lambda root, ref: _Result())
    result = runner.invoke(app, ["runs", "revert", "TASK-001", "--yes", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Reverted run" in result.output


def test_runs_revert_not_reversible(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline(reversible=False))
    result = runner.invoke(app, ["runs", "revert", "TASK-001", "--yes", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "no before/after checkpoints" in result.output


def test_runs_revert_declined(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())
    result = runner.invoke(
        app, ["runs", "revert", "TASK-001", "--project-root", str(tmp_path)], input="n\n"
    )
    assert result.exit_code == 1


def test_runs_supervise_keep_json(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())

    async def fake_supervise(root, tl, router):
        return SupervisorVerdict(verdict="keep", confidence=0.9, rationale="looks good", source="heuristic")

    monkeypatch.setattr(run_trace, "supervise_run", fake_supervise)
    result = runner.invoke(
        app, ["runs", "supervise", "TASK-001", "--no-llm", "--json", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert json.loads(result.stdout)["verdict"] == "keep"


def test_runs_supervise_revert_applies(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())

    async def fake_supervise(root, tl, router):
        return SupervisorVerdict(verdict="revert", confidence=0.8, rationale="failed", findings=["bad"])

    class _Result:
        message = "Rolled back."

    monkeypatch.setattr(run_trace, "supervise_run", fake_supervise)
    monkeypatch.setattr(run_trace, "revert_run", lambda root, ref: _Result())
    result = runner.invoke(
        app, ["runs", "supervise", "TASK-001", "--no-llm", "--apply", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "Applied revert" in result.output


def test_runs_supervise_revert_without_apply_hints(tmp_path, monkeypatch):
    monkeypatch.setattr(run_trace, "load_timeline", lambda root, ref, **k: _timeline())

    async def fake_supervise(root, tl, router):
        return SupervisorVerdict(verdict="revert", confidence=0.8, rationale="failed")

    monkeypatch.setattr(run_trace, "supervise_run", fake_supervise)
    result = runner.invoke(
        app, ["runs", "supervise", "TASK-001", "--no-llm", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "dev runs revert" in result.output
