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


# --- a gate that could not evaluate must never look like a gate that passed -------


def _invoke_stop(tmp_path, client: str = "claude"):
    return runner.invoke(
        app,
        ["hook", "agent-response", json.dumps({"session_id": "unevaluable"}),
         "--project-root", str(tmp_path), "--client", client],
    )


def test_clean_pass_is_silent(tmp_path, monkeypatch):
    """The control case: an evaluated, passing gate emits nothing on stdout.

    ``mode`` is quoted deliberately. Unquoted ``off`` is a YAML 1.1 *boolean*, which
    fails the ``str`` field and sends ``evaluate_stop`` down its fail-open path — so
    an unquoted fixture is not a passing gate, it is a gate that never ran. That
    distinction was invisible before this change, which is the whole point of it.
    """
    _init_repo(tmp_path, mode='"off"')
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    from devcouncil.execution.stop_gate import evaluate_stop

    assert evaluate_stop(tmp_path, {"session_id": "unevaluable"}).fail_open is False
    result = _invoke_stop(tmp_path)
    assert result.exit_code == 0
    assert result.stdout.strip() == ""


def test_stop_gate_crash_is_distinguishable_from_a_pass(tmp_path, monkeypatch):
    """An exception out of ``evaluate_stop`` must not be byte-identical to a pass.

    The handler printed the reason to stderr and exited 0 with empty stdout —
    and stderr on an exit-0 hook reaches Claude Code's debug log only, so a gate
    that could not run (locked DB, unreadable config, import failure) was
    indistinguishable from one that ran and approved. Fails against the pre-fix
    code with ``stdout == ''``, exactly like the control case above.
    """
    import devcouncil.execution.stop_gate as stop_gate

    _init_repo(tmp_path, mode="off")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    def _boom(root, payload):
        raise RuntimeError("db is locked")

    monkeypatch.setattr(stop_gate, "evaluate_stop", _boom)
    result = _invoke_stop(tmp_path)
    assert result.exit_code == 0
    out = result.stdout.strip()
    assert out, "an un-evaluable stop gate emitted nothing, exactly like a pass"
    data = json.loads(out)
    assert data.get("decision") != "block"
    message = data["systemMessage"]
    assert "did not evaluate" in message
    assert "db is locked" in message


def test_stop_gate_internal_fail_open_is_distinguishable_from_a_pass(tmp_path, monkeypatch):
    """``evaluate_stop``'s own fail-open returns a pass-shaped result; say so.

    ``StopGateResult.fail_open`` already carried the fact — the emitter threw it
    away, so an internal fail-open was silent on every Claude-facing channel.
    """
    import devcouncil.execution.stop_gate as stop_gate

    _init_repo(tmp_path, mode="off")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))
    monkeypatch.setattr(
        stop_gate,
        "evaluate_stop",
        lambda root, payload: stop_gate.StopGateResult(decision="pass", fail_open=True, mode="off"),
    )
    result = _invoke_stop(tmp_path)
    assert result.exit_code == 0
    out = result.stdout.strip()
    assert out, "an internal fail-open emitted nothing, exactly like a pass"
    assert "did not evaluate" in json.loads(out)["systemMessage"]


def test_stop_gate_crash_still_lets_gemini_through_with_its_own_schema(tmp_path, monkeypatch):
    """Gemini keeps its allow/suppressOutput envelope and gains the notice."""
    import devcouncil.execution.stop_gate as stop_gate

    _init_repo(tmp_path, mode="off")
    monkeypatch.setenv("DEVCOUNCIL_PROJECT_ROOT", str(tmp_path))

    def _boom(root, payload):
        raise RuntimeError("db is locked")

    monkeypatch.setattr(stop_gate, "evaluate_stop", _boom)
    result = _invoke_stop(tmp_path, client="gemini")
    assert result.exit_code == 0
    data = json.loads(result.stdout.strip())
    assert data["decision"] == "allow"
    assert data["suppressOutput"] is True
    assert "did not evaluate" in data["systemMessage"]


class TestNotAProjectIsNotAFailureToEvaluate:
    """`fail_open` means "tried and could not", never "nothing applies here".

    The stop gate's `fail_open` flag drives a user-visible notice — "did not
    evaluate this stop: nothing was checked. This is not a pass." — which exists
    because an unexamined stop must not look like an approved one.

    `load_config` raises `FileNotFoundError` in any directory that has never been
    `dev init`-ed, which is most directories a hook ever runs in. That landed in
    the blanket `except Exception` and was marked `fail_open`, so the notice
    fired on **every stop outside an initialized repository**. A warning that is
    always on is a warning nobody reads, and it would have drowned the real one.

    Both directions are asserted here. Testing only the quiet case would pass
    against a gate that had simply stopped reporting.
    """

    def test_an_uninitialized_directory_is_answered_quietly(self, tmp_path):
        from devcouncil.execution.stop_gate import evaluate_stop

        result = evaluate_stop(tmp_path, {})

        assert result.decision == "pass"
        assert result.fail_open is False, (
            "a directory with no DevCouncil project is 'nothing to gate', which is a "
            "complete answer; marking it un-evaluable fires the notice on every stop"
        )

    def test_a_genuine_internal_error_is_still_reported_as_unevaluated(
        self, tmp_path, monkeypatch
    ):
        """The honesty this whole flag exists for must survive the fix above."""
        import devcouncil.execution.stop_gate as stop_gate

        def _boom(*_a, **_k):
            raise RuntimeError("simulated internal fault")

        monkeypatch.setattr("devcouncil.app.config.load_config", _boom)

        result = stop_gate.evaluate_stop(tmp_path, {})

        assert result.decision == "pass", "the gate must still fail open, not block"
        assert result.fail_open is True, (
            "an internal fault means the gate did not evaluate; reporting it as an "
            "ordinary pass is the defect this flag was added to close"
        )
