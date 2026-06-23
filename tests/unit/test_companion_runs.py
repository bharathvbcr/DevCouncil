"""Tests for `dev runs` inspection and the mirrored MCP run tools."""

import asyncio
import json
import os
import time
from pathlib import Path

from typer.testing import CliRunner

from devcouncil.cli.commands.runs import app as runs_app
from devcouncil.integrations.mcp import server


runner = CliRunner()


def _write_manifest(root: Path, run_id: str, **overrides) -> Path:
    run_dir = root / ".devcouncil" / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    manifest = {
        "run_id": run_id,
        "task_id": "TASK-001",
        "agent": "claude",
        "profile": "prod",
        "status": "finished",
        "started_at": "2026-06-20T10:00:00+00:00",
        "finished_at": "2026-06-20T10:05:00+00:00",
        "returncode": 0,
        "command": ["claude", "-p", "--permission-mode", "default"],
    }
    manifest.update(overrides)
    path = run_dir / "agent-run.json"
    path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return path


def test_runs_list_parses_manifests_newest_first(tmp_path):
    older = _write_manifest(tmp_path, "run-old", task_id="TASK-001")
    time.sleep(0.01)
    newer = _write_manifest(tmp_path, "run-new", task_id="TASK-002")
    # Ensure mtimes are ordered (old < new).
    os.utime(older, (time.time() - 100, time.time() - 100))

    result = runner.invoke(runs_app, ["list", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    data = json.loads(result.output)
    ids = [run["run_id"] for run in data["runs"]]
    assert ids == ["run-new", "run-old"]
    assert data["count"] == 2
    assert newer.exists()


def test_runs_list_status_filter(tmp_path):
    _write_manifest(tmp_path, "run-ok", status="finished")
    _write_manifest(tmp_path, "run-bad", status="failed")

    result = runner.invoke(
        runs_app, ["list", "--json", "--status", "failed", "--project-root", str(tmp_path)]
    )
    data = json.loads(result.output)
    assert [r["run_id"] for r in data["runs"]] == ["run-bad"]


def test_runs_list_flags_orphaned_running(tmp_path):
    path = _write_manifest(tmp_path, "run-stuck", status="running")
    # Backdate the manifest well beyond the orphan threshold.
    old = time.time() - 24 * 3600
    os.utime(path, (old, old))

    result = runner.invoke(runs_app, ["list", "--json", "--project-root", str(tmp_path)])
    data = json.loads(result.output)
    run = data["runs"][0]
    assert run["status"] == "running"
    assert run["orphaned"] is True


def test_runs_list_fresh_running_not_orphaned(tmp_path):
    _write_manifest(tmp_path, "run-live", status="running")
    result = runner.invoke(runs_app, ["list", "--json", "--project-root", str(tmp_path)])
    data = json.loads(result.output)
    assert data["runs"][0]["orphaned"] is False


def test_runs_show_includes_redacted_transcript_tail(tmp_path):
    _write_manifest(tmp_path, "run-1")
    transcript = tmp_path / ".devcouncil" / "runs" / "run-1" / "transcript.txt"
    transcript.write_text(
        "line one\nAPI_KEY: sk-1234567890abcdefghijklmnop\nline three\n", encoding="utf-8"
    )

    result = runner.invoke(
        runs_app, ["show", "run-1", "--json", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0, result.output
    data = json.loads(result.output)
    assert data["ok"] is True
    assert data["manifest"]["run_id"] == "run-1"
    assert "REDACTED" in data["transcript_tail"]
    assert "sk-1234567890abcdefghijklmnop" not in data["transcript_tail"]


def test_runs_show_missing_run_errors(tmp_path):
    result = runner.invoke(
        runs_app, ["show", "nope", "--json", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1
    data = json.loads(result.output)
    assert data["ok"] is False


def _call_tool(name: str, arguments: dict, root: Path) -> dict:
    os.environ["DEVCOUNCIL_PROJECT_ROOT"] = str(root)
    try:
        result = asyncio.run(server.call_tool(name, arguments))
    finally:
        os.environ.pop("DEVCOUNCIL_PROJECT_ROOT", None)
    assert len(result) == 1
    return json.loads(result[0].text)


def test_mcp_list_agent_runs_shape(tmp_path):
    _write_manifest(tmp_path, "run-a", status="finished")
    path = _write_manifest(tmp_path, "run-b", status="running")
    old = time.time() - 24 * 3600
    os.utime(path, (old, old))

    payload = _call_tool("devcouncil_list_agent_runs", {}, tmp_path)
    assert payload["ok"] is True
    assert payload["total"] == 2
    run_ids = {run["run_id"] for run in payload["runs"]}
    assert run_ids == {"run-a", "run-b"}
    orphaned = {run["run_id"]: run["orphaned"] for run in payload["runs"]}
    assert orphaned["run-b"] is True


def test_mcp_get_run_returns_manifest_and_tail(tmp_path):
    _write_manifest(tmp_path, "run-x")
    transcript = tmp_path / ".devcouncil" / "runs" / "run-x" / "transcript.txt"
    transcript.write_text("hello\nsecret token=abcd\nbye\n", encoding="utf-8")

    payload = _call_tool("devcouncil_get_run", {"run_id": "run-x"}, tmp_path)
    assert payload["ok"] is True
    assert payload["manifest"]["run_id"] == "run-x"
    assert payload["transcript_path"]
    assert "bye" in payload["transcript_tail"]


def test_mcp_get_run_missing(tmp_path):
    payload = _call_tool("devcouncil_get_run", {"run_id": "ghost"}, tmp_path)
    assert payload["ok"] is False
    assert payload["code"] == "not_found"


def test_mcp_get_run_requires_run_id(tmp_path):
    payload = _call_tool("devcouncil_get_run", {}, tmp_path)
    assert payload["ok"] is False
    assert payload["code"] == "missing_argument"
