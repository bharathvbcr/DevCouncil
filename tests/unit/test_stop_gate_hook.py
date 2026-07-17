"""Hook integration tests for unified Stop / post_task emission."""

from __future__ import annotations

import json
import subprocess

from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _init_repo(tmp_path, *, mode: str = "block"):
    subprocess.run(["git", "init"], cwd=tmp_path, capture_output=True)
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "config.yaml").write_text(
        (
            "project:\n  name: test\n"
            "commands:\n  test:\n    - python -c \"import sys; sys.exit(1)\"\n"
            f"execution:\n  stop_gate:\n    mode: {mode}\n    verify_active_task: false\n"
        ),
        encoding="utf-8",
    )


def test_agent_response_emits_block_json(tmp_path, monkeypatch):
    _init_repo(tmp_path, mode="block")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    payload = json.dumps(
        {"session_id": "hook-1", "claim_text": "All tests pass."},
    )
    result = runner.invoke(
        app,
        ["hook", "agent-response", payload, "--project-root", str(tmp_path), "--client", "claude"],
    )
    assert result.exit_code == 0
    out = result.stdout.strip()
    assert out
    data = json.loads(out)
    assert data["decision"] == "block"
    assert "reason" in data


def test_agent_response_assist_emits_system_message(tmp_path, monkeypatch):
    _init_repo(tmp_path, mode="assist")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    payload = json.dumps({"session_id": "hook-2", "claim_text": "All tests pass."})
    result = runner.invoke(
        app,
        ["hook", "agent-response", payload, "--project-root", str(tmp_path), "--client", "claude"],
    )
    assert result.exit_code == 0
    if result.stdout.strip():
        data = json.loads(result.stdout.strip())
        assert data.get("decision") != "block"
        assert "systemMessage" in data or "reason" not in data


def test_post_task_is_thin_alias(tmp_path, monkeypatch):
    _init_repo(tmp_path, mode="block")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    payload = json.dumps({"session_id": "hook-3", "claim_text": "All tests pass."})
    result = runner.invoke(
        app,
        ["hook", "post-task", payload, "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout.strip())
    assert data["decision"] == "block"


def test_emit_stop_result_codex_allow(capsys):
    from devcouncil.cli.commands.hook import _emit_stop_result
    from devcouncil.execution.stop_gate import StopGateResult

    _emit_stop_result(
        "codex",
        StopGateResult(decision="pass", system_message="ok"),
    )
    payload = json.loads(capsys.readouterr().out)
    assert payload == {"systemMessage": "ok"}
