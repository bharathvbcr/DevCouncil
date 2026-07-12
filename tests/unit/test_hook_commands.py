"""Handler-branch coverage for the coding-CLI hook commands.

Focuses on ``devcouncil.cli.commands.hook`` decision emission, the Claude Code
lifecycle hooks (session start/end, notifications, statusline), and the small
lock/queue metadata helpers — with the policy engine, DB, and signal writer
mocked so we exercise branch logic without a real project.
"""

from __future__ import annotations

import json
from types import SimpleNamespace

import pytest
import typer
from typer.testing import CliRunner

import devcouncil.cli.commands.hook as hook
from devcouncil.cli.commands.hook import (
    _emit_decision,
    _emit_unevaluable,
    _parse_queue_file,
    _read_lock_meta,
    _read_stdin_payload,
    _status_line,
    app as hook_app,
)

runner = CliRunner()


def _decision(action: str, reason: str = "because"):
    return SimpleNamespace(action=action, reason=reason)


class _FakePolicy:
    """Stand-in for HookPolicy whose evaluate() returns a preset decision."""

    _decision = _decision("allow")

    def __init__(self, *, project_root):
        self.project_root = project_root

    def evaluate(self, call_data, active_task):
        return type(self)._decision


@pytest.fixture
def patch_policy(monkeypatch):
    monkeypatch.setattr(hook, "_active_task", lambda root: None)

    def _set(action, reason="because"):
        _FakePolicy._decision = _decision(action, reason)
        monkeypatch.setattr(hook, "HookPolicy", _FakePolicy)

    return _set


# ---- _emit_decision / _emit_unevaluable ---------------------------------------

def test_emit_decision_deny_raises_exit_2(capsys):
    with pytest.raises(typer.Exit) as exc:
        _emit_decision("claude", "deny", "blocked write")
    assert exc.value.exit_code == 2
    assert "blocked write" in capsys.readouterr().err


def test_emit_decision_codex_allow_emits_json(capsys):
    _emit_decision("codex", "allow", "fine")
    payload = json.loads(capsys.readouterr().out)
    assert payload["decision"] == "allow"
    assert payload["suppressOutput"] is True


def test_emit_decision_codex_warn_includes_system_message(capsys):
    _emit_decision("gemini", "warn", "heads up")
    payload = json.loads(capsys.readouterr().out)
    assert payload["systemMessage"] == "DevCouncil Warning: heads up"


def test_emit_decision_claude_warn_prints_console(capsys):
    _emit_decision("claude", "warn", "careful")
    assert "DevCouncil Warning" in capsys.readouterr().out


def test_emit_unevaluable_strict_denies():
    with pytest.raises(typer.Exit) as exc:
        _emit_unevaluable("claude", "cannot parse", strict=True)
    assert exc.value.exit_code == 2


def test_emit_unevaluable_non_strict_allows(capsys):
    # action defaults to warn -> allowed for claude (no exit)
    _emit_unevaluable("claude", "cannot parse", strict=False)
    assert "cannot parse" in capsys.readouterr().out


# ---- pre_tool_use -------------------------------------------------------------

def test_pre_tool_use_empty_payload_allows(tmp_path):
    result = runner.invoke(hook_app, ["pre-tool-use", "", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_pre_tool_use_empty_payload_strict_blocks(tmp_path):
    result = runner.invoke(hook_app, ["pre-tool-use", "", "--strict", "--project-root", str(tmp_path)])
    assert result.exit_code == 2


def test_pre_tool_use_invalid_json_warns_allow(tmp_path):
    result = runner.invoke(hook_app, ["pre-tool-use", "{not json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_pre_tool_use_invalid_json_strict_blocks(tmp_path):
    result = runner.invoke(hook_app, ["pre-tool-use", "{not json", "--strict", "--project-root", str(tmp_path)])
    assert result.exit_code == 2


def test_pre_tool_use_policy_allow(tmp_path, patch_policy):
    patch_policy("allow")
    payload = json.dumps({"tool_name": "Write", "tool_input": {"file_path": "a.py"}})
    result = runner.invoke(hook_app, ["pre-tool-use", payload, "--project-root", str(tmp_path)])
    assert result.exit_code == 0


def test_pre_tool_use_policy_deny_blocks(tmp_path, patch_policy):
    patch_policy("deny", "not in lease")
    payload = json.dumps({"tool_name": "Write", "tool_input": {"file_path": "a.py"}})
    result = runner.invoke(hook_app, ["pre-tool-use", payload, "--project-root", str(tmp_path)])
    assert result.exit_code == 2
    assert "not in lease" in result.output


def test_pre_tool_use_codex_allow_emits_json(tmp_path, patch_policy):
    patch_policy("allow", "ok")
    payload = json.dumps({"tool_name": "Read"})
    result = runner.invoke(
        hook_app, ["pre-tool-use", payload, "--client", "codex", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert '"decision":"allow"' in result.output


def test_pre_tool_use_policy_crash_is_contained(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "_active_task", lambda root: None)

    class Boom:
        def __init__(self, *, project_root):
            pass

        def evaluate(self, *a):
            raise RuntimeError("engine down")

    monkeypatch.setattr(hook, "HookPolicy", Boom)
    payload = json.dumps({"tool_name": "Write"})
    # Non-strict: a crashing hook must not emit an undefined exit code -> allow (0).
    result = runner.invoke(hook_app, ["pre-tool-use", payload, "--project-root", str(tmp_path)])
    assert result.exit_code == 0


# ---- post_tool_use ------------------------------------------------------------

def test_post_tool_use_refresh_is_best_effort(tmp_path, monkeypatch):
    called = {}
    monkeypatch.setattr(hook, "_maybe_refresh_map", lambda root, text: called.setdefault("text", text))
    payload = json.dumps({"tool_name": "Edit", "tool_input": {"file_path": "a.py"}})
    result = runner.invoke(hook_app, ["post-tool-use", payload, "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert called["text"] == payload


def test_post_tool_use_refresh_error_is_swallowed(tmp_path, monkeypatch):
    def boom(root, text):
        raise RuntimeError("refresh failed")

    monkeypatch.setattr(hook, "_maybe_refresh_map", boom)
    result = runner.invoke(hook_app, ["post-tool-use", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "map refresh error" in result.output


def test_post_tool_use_codex_emits_allow_json(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "_maybe_refresh_map", lambda root, text: None)
    result = runner.invoke(
        hook_app, ["post-tool-use", "{}", "--client", "codex", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert '"decision":"allow"' in result.output


# ---- agent_response -----------------------------------------------------------

def test_agent_response_writes_signal(tmp_path, monkeypatch):
    seen = {}
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-9")
    monkeypatch.setattr(hook, "write_signal", lambda root, client, payload: seen.setdefault("payload", payload) or (tmp_path / "sig.json"))
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))
    result = runner.invoke(hook_app, ["agent-response", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert seen["payload"]["task_id"] == "TASK-9"


def test_agent_response_codex_emits_allow_json(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: None)
    monkeypatch.setattr(hook, "write_signal", lambda *a, **k: tmp_path / "sig.json")
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))
    result = runner.invoke(
        hook_app, ["agent-response", "{}", "--client", "codex", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert '"decision":"allow"' in result.output


def test_agent_response_never_raises_on_signal_error(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "active_task_id", lambda root: None)

    def boom(*a, **k):
        raise OSError("cannot write")

    monkeypatch.setattr(hook, "write_signal", boom)
    result = runner.invoke(hook_app, ["agent-response", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "agent-response hook error" in result.output


# ---- session lifecycle hooks --------------------------------------------------

def test_session_start_injects_status_context(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "_status_line", lambda root: "DevCouncil status snapshot")
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))
    result = runner.invoke(
        hook_app, ["session-start", json.dumps({"session_id": "abc"}), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    payload = json.loads(result.output)
    assert payload["hookSpecificOutput"]["hookEventName"] == "SessionStart"
    assert "snapshot" in payload["hookSpecificOutput"]["additionalContext"]


def test_session_start_no_status_no_output(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "_status_line", lambda root: None)
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))
    result = runner.invoke(hook_app, ["session-start", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert result.output.strip() == ""


def test_user_prompt_submit_emits_context(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "_status_line", lambda root: "ctx line")
    result = runner.invoke(hook_app, ["user-prompt-submit", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "UserPromptSubmit" in result.output


def test_session_end_records_trace(tmp_path, monkeypatch):
    logged = {}
    monkeypatch.setattr(
        hook, "TraceLogger",
        lambda root: SimpleNamespace(log_event=lambda name, details, **k: logged.update({"name": name, "details": details})),
    )
    result = runner.invoke(
        hook_app, ["session-end", json.dumps({"session_id": "s1", "reason": "done"}), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert logged["name"] == "session_end"
    assert logged["details"]["reason"] == "done"


def test_pre_compact_records_trace(tmp_path, monkeypatch):
    logged = {}
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda name, *a, **k: logged.setdefault("name", name)))
    result = runner.invoke(hook_app, ["pre-compact", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert logged["name"] == "pre_compact"


def test_subagent_stop_writes_signal_with_active_task(tmp_path, monkeypatch):
    seen = {}
    monkeypatch.setattr(hook, "active_task_id", lambda root: "TASK-5")
    monkeypatch.setattr(hook, "write_signal", lambda root, client, payload: seen.setdefault("payload", payload) or (tmp_path / "s.json"))
    monkeypatch.setattr(hook, "TraceLogger", lambda root: SimpleNamespace(log_event=lambda *a, **k: None))
    result = runner.invoke(hook_app, ["subagent-stop", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert seen["payload"]["task_id"] == "TASK-5"


def test_notification_records_message(tmp_path, monkeypatch):
    logged = {}
    monkeypatch.setattr(
        hook, "TraceLogger",
        lambda root: SimpleNamespace(log_event=lambda name, details, summary=None, **k: logged.update({"summary": summary})),
    )
    result = runner.invoke(
        hook_app, ["notification", json.dumps({"message": "build finished"}), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "build finished" in logged["summary"]


def test_claude_statusline_uninitialized(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "_status_line", lambda root: None)
    result = runner.invoke(hook_app, ["claude-statusline", "{}", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "not initialized" in result.output


def test_claude_statusline_trims_guidance(tmp_path, monkeypatch):
    monkeypatch.setattr(hook, "_status_line", lambda root: "phase: X. Use the tools to do stuff.")
    result = runner.invoke(hook_app, ["claude-statusline", json.dumps({"cwd": str(tmp_path)}), "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "phase: X" in result.output
    assert "Use the" not in result.output


# ---- post_task ----------------------------------------------------------------

def test_post_task_verify_disabled_prints_reminder(tmp_path, monkeypatch):
    # load_config is imported lazily inside post_task; patch at source module.
    import devcouncil.app.config as config_mod
    monkeypatch.setattr(
        config_mod, "load_config",
        lambda root: SimpleNamespace(execution=SimpleNamespace(verify_on_post_task=False)),
    )
    result = runner.invoke(hook_app, ["post-task", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "dev verify" in result.output


def test_post_task_verify_enabled_runs_verification(tmp_path, monkeypatch):
    import devcouncil.app.config as config_mod
    monkeypatch.setattr(
        config_mod, "load_config",
        lambda root: SimpleNamespace(execution=SimpleNamespace(verify_on_post_task=True)),
    )
    monkeypatch.setattr(hook, "_verify_active_task", lambda root: "[green]verified summary[/green]")
    result = runner.invoke(hook_app, ["post-task", "--client", "codex", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "verified summary" in result.output
    assert '"decision":"allow"' in result.output


# ---- metadata helpers ---------------------------------------------------------

def test_read_stdin_payload_variants():
    assert _read_stdin_payload("") == {}
    assert _read_stdin_payload(json.dumps({"a": 1})) == {"a": 1}
    # Non-dict JSON and invalid JSON both fall back to a raw wrapper.
    assert _read_stdin_payload("[1,2]") == {"raw": "[1,2]"}
    assert _read_stdin_payload("nonjson") == {"raw": "nonjson"}


def test_read_lock_meta_json_and_legacy(tmp_path):
    j = tmp_path / "lock.json"
    j.write_text(json.dumps({"pid": 42, "started_at": 1000.5}), encoding="utf-8")
    assert _read_lock_meta(j) == (42, 1000.5)

    legacy = tmp_path / "lock.txt"
    legacy.write_text("77\n2000.0\n", encoding="utf-8")
    assert _read_lock_meta(legacy) == (77, 2000.0)

    empty = tmp_path / "empty"
    empty.write_text("", encoding="utf-8")
    assert _read_lock_meta(empty) == (None, None)


def test_parse_queue_file_dict_and_list(tmp_path):
    d = tmp_path / "q1.json"
    d.write_text(json.dumps({"paths": ["a.py", "b.py"]}), encoding="utf-8")
    assert _parse_queue_file(d) == ["a.py", "b.py"]

    lst = tmp_path / "q2.json"
    lst.write_text(json.dumps(["c.py"]), encoding="utf-8")
    assert _parse_queue_file(lst) == ["c.py"]

    missing = tmp_path / "nope.json"
    assert _parse_queue_file(missing) == []


def test_status_line_none_when_uninitialized(tmp_path):
    # No DB in an empty tmp dir -> best-effort None, never raises.
    assert _status_line(tmp_path) is None
