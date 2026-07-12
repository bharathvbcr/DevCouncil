"""Tests for the in-process Claude Agent SDK executor and its lease-aware gate.

The SDK is an optional dependency and is not installed in CI, so the run loop is exercised
against an injected fake module that mimics the documented ``query`` / ``can_use_tool``
contract. The permission decision is tested directly against the real HookPolicy.
"""

from __future__ import annotations

import sys
import types
from types import SimpleNamespace

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


# ---- _allow / _deny permission result construction ----------------------------


def test_allow_prefers_typed_class_then_falls_back():
    class Allow:
        def __init__(self, updated_input=None):
            self.updated_input = updated_input

    sdk = SimpleNamespace(PermissionResultAllow=Allow)
    out = ClaudeSdkExecutor._allow(sdk, {"a": 1})
    assert isinstance(out, Allow)
    assert out.updated_input == {"a": 1}


def test_allow_typeerror_falls_back_to_no_arg_class():
    class Allow:
        def __init__(self):
            self.updated_input = None

    sdk = SimpleNamespace(PermissionResultAllow=Allow)
    out = ClaudeSdkExecutor._allow(sdk, {"a": 1})
    assert isinstance(out, Allow)


def test_allow_dict_fallback_without_class():
    out = ClaudeSdkExecutor._allow(SimpleNamespace(), {"a": 1})
    assert out == {"behavior": "allow", "updatedInput": {"a": 1}}


def test_deny_typed_then_positional_then_dict():
    class Deny:
        def __init__(self, message=None):
            self.message = message

    assert ClaudeSdkExecutor._deny(SimpleNamespace(PermissionResultDeny=Deny), "no").message == "no"

    class DenyPositional:
        def __init__(self, reason):  # rejects keyword 'message'
            self.reason = reason

    out = ClaudeSdkExecutor._deny(SimpleNamespace(PermissionResultDeny=DenyPositional), "why")
    assert out.reason == "why"

    assert ClaudeSdkExecutor._deny(SimpleNamespace(), "because") == {
        "behavior": "deny",
        "message": "because",
    }


# ---- _run_coroutine_from_sync nested-loop path --------------------------------

def test_run_coroutine_from_sync_nested_loop():
    import asyncio

    async def outer():
        async def inner():
            return 42

        # Called while a loop is already running -> worker-thread path.
        return ClaudeSdkExecutor._run_coroutine_from_sync(inner())

    assert asyncio.run(outer()) == 42


# ---- _build_options fallbacks -------------------------------------------------

def test_build_options_without_options_class(tmp_path):
    _init_repo(tmp_path)
    ex = ClaudeSdkExecutor(tmp_path, active_task=_task(), model="m", env={"K": "V"})
    kwargs = ex._build_options(SimpleNamespace(), can_use_tool=lambda *a: None)
    assert kwargs["cwd"] == str(tmp_path)
    assert kwargs["model"] == "m"
    assert kwargs["env"] == {"K": "V"}


def test_build_options_all_kwargs_rejected_falls_back_to_bare(tmp_path):
    _init_repo(tmp_path)

    class Bare:
        def __init__(self):  # rejects every kwarg combination
            self.constructed = True

    ex = ClaudeSdkExecutor(tmp_path, active_task=_task(), model="m", env={"K": "V"})
    obj = ex._build_options(SimpleNamespace(ClaudeAgentOptions=Bare), can_use_tool=lambda *a: None)
    assert isinstance(obj, Bare)
    # No can_use_tool attribute to fill -> returned as-is.
    assert not hasattr(obj, "can_use_tool")


# ---- _message_session_id / _message_result -----------------------------------

def test_message_session_id_variants():
    assert ClaudeSdkExecutor._message_session_id(SimpleNamespace(session_id="s1")) == "s1"
    assert ClaudeSdkExecutor._message_session_id({"session_id": "s2"}) == "s2"
    assert ClaudeSdkExecutor._message_session_id(SimpleNamespace(data={"session_id": "s3"})) == "s3"
    assert ClaudeSdkExecutor._message_session_id({}) is None


def test_message_result_non_result_message():
    assert ClaudeSdkExecutor._message_result(SimpleNamespace(type="assistant")) == (None, False)


def test_message_result_success_and_error():
    ok = SimpleNamespace(type="result", subtype="success", result="done", is_error=False)
    assert ClaudeSdkExecutor._message_result(ok) == ("done", False)

    err = SimpleNamespace(type="result", subtype="error_max_turns", result="bad", is_error=True)
    assert ClaudeSdkExecutor._message_result(err) == ("bad", True)


def test_message_result_dict_and_derived_error():
    # Dict form; is_error absent -> derived from a non-success subtype.
    msg = {"type": "result", "subtype": "error_during_execution", "result": "x"}
    text, is_error = ClaudeSdkExecutor._message_result(msg)
    assert text == "x" and is_error is True

    # ResultMessage-by-classname path with no explicit error and a success subtype.
    class ResultMessage:
        subtype = "success"
        result = "final"

    text2, err2 = ClaudeSdkExecutor._message_result(ResultMessage())
    assert text2 == "final" and err2 is False


def test_message_result_no_result_text():
    msg = SimpleNamespace(type="result", subtype="success", result=None, is_error=False)
    assert ClaudeSdkExecutor._message_result(msg) == (None, False)


# ---- run loop: no denials suffix and mid-run failures -------------------------

def _install_sdk_with_query(monkeypatch, query_fn):
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

    fake.ClaudeAgentOptions = ClaudeAgentOptions
    fake.PermissionResultAllow = PermissionResultAllow
    fake.PermissionResultDeny = PermissionResultDeny
    fake.query = query_fn
    monkeypatch.setitem(sys.modules, "claude_agent_sdk", fake)
    return fake


def test_run_task_success_without_denials(tmp_path, monkeypatch):
    _init_repo(tmp_path)

    class ResultMessage:
        type = "result"
        subtype = "success"

        def __init__(self, result):
            self.result = result
            self.is_error = False
            self.session_id = "sess-1"

    async def query(prompt=None, options=None):
        yield ResultMessage("All good.")

    _install_sdk_with_query(monkeypatch, query)
    result = ClaudeSdkExecutor(tmp_path, active_task=_task()).run_task(_task(), [])
    assert result.success
    assert result.message == "All good."  # no gate-denial suffix
    assert "Gate denied" not in result.message


def test_run_task_non_transient_failure(tmp_path, monkeypatch):
    _init_repo(tmp_path)

    async def query(prompt=None, options=None):
        raise ValueError("hard failure")
        yield  # pragma: no cover - marks this an async generator

    _install_sdk_with_query(monkeypatch, query)
    result = ClaudeSdkExecutor(tmp_path, active_task=_task()).run_task(_task(), [])
    assert not result.success
    assert "run failed" in result.message.lower()
    assert "hard failure" in result.message


def test_run_task_transient_failure_retries_then_succeeds(tmp_path, monkeypatch):
    _init_repo(tmp_path)
    state = {"n": 0}

    class ResultMessage:
        type = "result"
        subtype = "success"

        def __init__(self):
            self.result = "recovered"
            self.is_error = False
            self.session_id = "s"

    async def query(prompt=None, options=None):
        state["n"] += 1
        if state["n"] == 1:
            raise RuntimeError("connection reset by peer")
        yield ResultMessage()

    _install_sdk_with_query(monkeypatch, query)
    monkeypatch.setattr("time.sleep", lambda _s: None)
    result = ClaudeSdkExecutor(tmp_path, active_task=_task()).run_task(_task(), [])
    assert result.success
    assert "recovered" in result.message
    assert state["n"] == 2


def test_run_task_transient_failure_retry_also_fails(tmp_path, monkeypatch):
    _init_repo(tmp_path)

    async def query(prompt=None, options=None):
        raise RuntimeError("connection reset by peer")
        yield  # pragma: no cover

    _install_sdk_with_query(monkeypatch, query)
    monkeypatch.setattr("time.sleep", lambda _s: None)
    result = ClaudeSdkExecutor(tmp_path, active_task=_task()).run_task(_task(), [])
    assert not result.success
    assert "after retry" in result.message
