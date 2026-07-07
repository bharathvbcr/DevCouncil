"""Tests for the in-process Claude Agent SDK executor and its lease-aware gate.

The SDK is an optional dependency and is not installed in CI, so the run loop is exercised
against an injected fake module that mimics the documented ``query`` / ``can_use_tool``
contract. The permission decision is tested directly against the real HookPolicy.
"""

from __future__ import annotations

import sys
import types

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.executors.claude_sdk import ClaudeSdkExecutor


def _init_repo(tmp_path):
    (tmp_path / ".devcouncil").mkdir(parents=True, exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text("models:\n  provider: anthropic\n", encoding="utf-8")


def _task():
    return Task(
        id="TASK-900",
        title="SDK",
        description="Implement the feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )


def test_permission_decision_allows_in_scope_write(tmp_path):
    _init_repo(tmp_path)
    ex = ClaudeSdkExecutor(tmp_path, active_task=_task())
    decision = ex.permission_decision("Write", {"file_path": "src/app.py", "content": "x = 1\n"})
    assert decision.allowed


def test_permission_decision_denies_out_of_scope_write(tmp_path):
    _init_repo(tmp_path)
    ex = ClaudeSdkExecutor(tmp_path, active_task=_task())
    decision = ex.permission_decision("Write", {"file_path": "secrets/prod.env", "content": "TOKEN=1"})
    assert not decision.allowed


def test_run_task_without_sdk_returns_clean_failure(tmp_path):
    _init_repo(tmp_path)
    # claude_agent_sdk is not installed in the test environment.
    result = ClaudeSdkExecutor(tmp_path, active_task=_task()).run_task(_task(), [])
    assert not result.success
    assert "claude-agent-sdk" in result.message.lower()


def _install_fake_sdk(monkeypatch, calls):
    fake = types.ModuleType("claude_agent_sdk")

    class ClaudeAgentOptions:
        def __init__(self, **kw):
            self.__dict__.update(kw)

    class PermissionResultAllow:
        def __init__(self, updated_input=None):
            self.updated_input = updated_input
            self.behavior = "allow"

    class PermissionResultDeny:
        def __init__(self, message=None):
            self.message = message
            self.behavior = "deny"

    class ResultMessage:
        type = "result"
        subtype = "success"

        def __init__(self, result, session_id):
            self.result = result
            self.session_id = session_id
            self.is_error = False

    async def query(prompt=None, options=None):
        calls["options"] = options
        calls["prompt"] = prompt
        # Drive the gate: an in-scope call is allowed, an out-of-scope call is denied.
        calls["allow"] = await options.can_use_tool("Write", {"file_path": "src/app.py", "content": "x = 1\n"})
        calls["deny"] = await options.can_use_tool("Write", {"file_path": "secrets/prod.env", "content": "T=1"})
        yield ResultMessage("Done implementing.", "sdk-session-777")

    fake.ClaudeAgentOptions = ClaudeAgentOptions
    fake.PermissionResultAllow = PermissionResultAllow
    fake.PermissionResultDeny = PermissionResultDeny
    fake.ResultMessage = ResultMessage
    fake.query = query
    monkeypatch.setitem(sys.modules, "claude_agent_sdk", fake)


def test_run_task_with_fake_sdk_gates_and_succeeds(tmp_path, monkeypatch):
    _init_repo(tmp_path)
    calls: dict = {}
    _install_fake_sdk(monkeypatch, calls)

    ex = ClaudeSdkExecutor(tmp_path, active_task=_task())
    result = ex.run_task(_task(), [])

    assert result.success
    assert "Done implementing." in result.message
    assert ex.last_agent_session_id == "sdk-session-777"

    # The gate allowed the in-scope write and denied the out-of-scope one, live.
    assert calls["allow"].behavior == "allow"
    assert calls["deny"].behavior == "deny"
    assert any(name == "Write" for name, _ in ex.denials)
    # The denial is surfaced in the run message.
    assert "Gate denied 1 out-of-scope call" in result.message
    # Guard the containment invariant: the gate must not be bypassed by acceptEdits.
    assert calls["options"].permission_mode == "default"


def test_env_overrides_reach_sdk_options(tmp_path, monkeypatch):
    """Provider redirection: env passed to the executor must land on the SDK options,
    so the underlying Claude Code process targets the configured endpoint."""
    _init_repo(tmp_path)
    calls: dict = {}
    _install_fake_sdk(monkeypatch, calls)

    ex = ClaudeSdkExecutor(
        tmp_path,
        active_task=_task(),
        env={"ANTHROPIC_BASE_URL": "http://127.0.0.1:4000", "ANTHROPIC_AUTH_TOKEN": "tok"},
    )
    result = ex.run_task(_task(), [])

    assert result.success
    assert calls["options"].env == {
        "ANTHROPIC_BASE_URL": "http://127.0.0.1:4000",
        "ANTHROPIC_AUTH_TOKEN": "tok",
    }


def test_no_env_keeps_options_env_free(tmp_path, monkeypatch):
    _init_repo(tmp_path)
    calls: dict = {}
    _install_fake_sdk(monkeypatch, calls)

    result = ClaudeSdkExecutor(tmp_path, active_task=_task()).run_task(_task(), [])

    assert result.success
    assert not hasattr(calls["options"], "env")


def test_env_degrades_gracefully_on_old_sdk(tmp_path, monkeypatch):
    """An SDK whose options class rejects ``env`` must still receive cwd/model/permission_mode."""
    _init_repo(tmp_path)
    calls: dict = {}
    _install_fake_sdk(monkeypatch, calls)
    import sys as _sys

    fake = _sys.modules["claude_agent_sdk"]

    class StrictOptions:
        can_use_tool = None  # slot the executor's fallback fills in

        def __init__(self, cwd=None, permission_mode=None, model=None):
            self.cwd = cwd
            self.permission_mode = permission_mode
            self.model = model

    fake.ClaudeAgentOptions = StrictOptions

    ex = ClaudeSdkExecutor(
        tmp_path,
        active_task=_task(),
        model="claude-opus-4",
        env={"ANTHROPIC_BASE_URL": "http://127.0.0.1:4000"},
    )
    result = ex.run_task(_task(), [])

    assert result.success
    options = calls["options"]
    assert options.model == "claude-opus-4"
    assert options.permission_mode == "default"
    # env silently dropped rather than crashing the run.
    assert not getattr(options, "env", None)


def test_run_task_prepends_correction_manifest(tmp_path, monkeypatch):
    _init_repo(tmp_path)
    calls: dict = {}
    _install_fake_sdk(monkeypatch, calls)

    class _FakeManifest:
        def model_dump_json(self, indent=None):
            return '{"gap": "missing regression test"}'

    # A repair run must carry the correction manifest into the prompt (parity with the CLI).
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.load_latest_correction_manifest",
        lambda project_root, task_id: _FakeManifest(),
    )

    result = ClaudeSdkExecutor(tmp_path, active_task=_task()).run_task(_task(), [])

    assert result.success
    assert "Correction Manifest" in calls["prompt"]
    assert "missing regression test" in calls["prompt"]
    assert "Repair rules (non-negotiable)" in calls["prompt"]
