import json
from pathlib import Path
import subprocess
from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _setup_runs_env(tmp_path: Path, monkeypatch) -> tuple[Path, str]:
    monkeypatch.chdir(tmp_path)
    
    # Initialize git and DevCouncil
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    runner.invoke(app, ["init"])
    
    run_id = "run-123"
    run_dir = tmp_path / ".devcouncil" / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    
    manifest = {
        "run_id": run_id,
        "task_id": "TASK-1",
        "agent": "worker1",
        "profile": "default",
        "status": "finished",
        "timestamp": "2026-07-12T12:00:00Z",
    }
    (run_dir / "agent-run.json").write_text(json.dumps(manifest), encoding="utf-8")
    (run_dir / "transcript.txt").write_text("Hello from coding agent transcript\n", encoding="utf-8")
    
    # Force log directory to be inside our tmp_path to avoid global test isolation interference
    log_dir = tmp_path / ".devcouncil" / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("DEVCOUNCIL_LOG_DIR", str(log_dir))
    
    # Mock traces file
    traces_file = log_dir / "traces.jsonl"
    trace = {
        "schema": "devcouncil.trace.v1",
        "run_id": run_id,
        "task_id": "TASK-1",
        "timestamp": "2026-07-12T12:00:00Z",
        "type": "task_started",
        "summary": "Started task"
    }
    traces_file.write_text(json.dumps(trace) + "\n", encoding="utf-8")
    
    # Mock checkpoint patches
    cp_dir = tmp_path / ".devcouncil" / "checkpoints"
    cp_dir.mkdir(parents=True, exist_ok=True)
    (cp_dir / "TASK-1-before.patch").write_text("diff before patch", encoding="utf-8")
    (cp_dir / "TASK-1-after.patch").write_text("diff --git a/src/a.py b/src/a.py\n--- a/src/a.py\n+++ b/src/a.py\n", encoding="utf-8")
    
    return tmp_path, run_id


def test_cli_runs_list(tmp_path, monkeypatch):
    _setup_runs_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["runs", "list"])
    assert res.exit_code == 0
    assert "run-123" in res.output
    assert "TASK-1" in res.output
    
    # JSON output
    res_json = runner.invoke(app, ["runs", "list", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["count"] == 1
    assert data["runs"][0]["run_id"] == "run-123"


def test_cli_runs_show(tmp_path, monkeypatch):
    root, run_id = _setup_runs_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["runs", "show", run_id])
    assert res.exit_code == 0
    assert "run-123" in res.output
    assert "Hello from coding agent transcript" in res.output
    
    # JSON output
    res_json = runner.invoke(app, ["runs", "show", run_id, "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["ok"] is True
    assert data["run_id"] == run_id


def test_cli_runs_timeline(tmp_path, monkeypatch):
    root, run_id = _setup_runs_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["runs", "timeline", run_id])
    assert res.exit_code == 0
    assert "run-123" in res.output
    assert "Started task" in res.output
    
    # JSON output
    res_json = runner.invoke(app, ["runs", "timeline", run_id, "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert data["run_id"] == run_id


def test_cli_runs_diff(tmp_path, monkeypatch):
    root, run_id = _setup_runs_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["runs", "diff", run_id])
    assert res.exit_code == 0
    assert "diff --git a/src/a.py b/src/a.py" in res.output
    
    res_stat = runner.invoke(app, ["runs", "diff", run_id, "--stat"])
    assert res_stat.exit_code == 0
    assert "src/a.py" in res_stat.output


def test_cli_runs_revert(tmp_path, monkeypatch):
    root, run_id = _setup_runs_env(tmp_path, monkeypatch)
    
    # Revert will look for checkpoints in git. Since we don't have git refs set up,
    # it won't be fully reversible unless we mock/fake git checkpoints.
    # But we can test that it evaluates reversibility.
    res = runner.invoke(app, ["runs", "revert", run_id, "-y"])
    # Should exit with error since refs don't exist in git repository
    assert res.exit_code != 0
    assert "failed" in res.output.lower()


def test_cli_runs_supervise(tmp_path, monkeypatch):
    root, run_id = _setup_runs_env(tmp_path, monkeypatch)
    
    res = runner.invoke(app, ["runs", "supervise", run_id, "--no-llm"])
    assert res.exit_code == 0
    assert "verdict" in res.output.lower()
    
    res_json = runner.invoke(app, ["runs", "supervise", run_id, "--no-llm", "--json"])
    assert res_json.exit_code == 0
    data = json.loads(res_json.output)
    assert "verdict" in data
