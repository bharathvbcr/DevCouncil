"""Branch coverage for CodingCliExecutor internals.

Complements ``test_executors.py`` by targeting the transient-retry loop, the Warp
command/config builder (env overrides + MCP map), per-CLI permission/model override
translation, Grok session capture, and the small preview/display helpers — all with
subprocess and config mocked.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from types import SimpleNamespace

import pytest

from devcouncil.domain.task import PlannedFile, Task
from devcouncil.executors.coding_cli import CodingCliExecutor


def _task(task_id="TASK-001"):
    return Task(
        id=task_id,
        title="t",
        description="Implement feature",
        planned_files=[PlannedFile(path="src/app.py", reason="logic", allowed_change="modify")],
    )


def _write_config(tmp_path, body: str):
    (tmp_path / ".devcouncil").mkdir(exist_ok=True)
    (tmp_path / ".devcouncil" / "config.yaml").write_text(body, encoding="utf-8")


# ---- transient retry ----------------------------------------------------------

def test_transient_failure_reason_matches_network_markers(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "codex")
    result = subprocess.CompletedProcess(["codex"], 1, stdout="", stderr="Connection reset by peer")
    assert ex._transient_failure_reason(result) == "connection reset"
    clean = subprocess.CompletedProcess(["codex"], 1, stdout="AssertionError", stderr="real bug")
    assert ex._transient_failure_reason(clean) is None


def test_transient_retry_limit_from_config(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    _write_config(tmp_path, "execution:\n  transient_retry_attempts: 4\n")
    ex = CodingCliExecutor(tmp_path, "codex")
    assert ex._transient_retry_limit() == 4


def test_run_task_retries_transient_then_succeeds(tmp_path, monkeypatch):
    _write_config(tmp_path, "execution:\n  transient_retry_attempts: 2\n")
    calls = {"n": 0}

    def fake_run(cmd, **kwargs):
        calls["n"] += 1
        if calls["n"] == 1:
            return subprocess.CompletedProcess(cmd, 1, stdout="", stderr="socket hang up")
        return subprocess.CompletedProcess(cmd, 0, stdout="ok", stderr="")

    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    monkeypatch.setattr("subprocess.run", fake_run)
    monkeypatch.setattr("devcouncil.executors.coding_cli.time.sleep", lambda _s: None)

    result = CodingCliExecutor(tmp_path, "codex").run_task(_task(), [])
    assert result.success
    assert calls["n"] == 2
    traces = (tmp_path / ".devcouncil" / "logs" / "traces.jsonl").read_text(encoding="utf-8")
    assert "agent_run_transient_retry" in traces


def test_run_task_non_transient_failure_no_retry(tmp_path, monkeypatch):
    _write_config(tmp_path, "execution:\n  transient_retry_attempts: 3\n")
    calls = {"n": 0}

    def fake_run(cmd, **kwargs):
        calls["n"] += 1
        return subprocess.CompletedProcess(cmd, 1, stdout="", stderr="SyntaxError: bad code")

    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    monkeypatch.setattr("subprocess.run", fake_run)
    monkeypatch.setattr("devcouncil.executors.coding_cli.time.sleep", lambda _s: None)

    result = CodingCliExecutor(tmp_path, "codex").run_task(_task(), [])
    assert not result.success
    assert calls["n"] == 1  # no retry for a genuine error


# ---- permission / model override translation ----------------------------------

def _codex_executor(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    return CodingCliExecutor(tmp_path, "codex")


def test_apply_claude_permission_modes():
    fn = CodingCliExecutor._apply_claude_permission_mode
    assert fn(["claude", "-p"], "auto")[-2:] == ["--permission-mode", "acceptEdits"]
    assert fn(["claude", "-p"], "gated")[-2:] == ["--permission-mode", "default"]
    assert fn(["claude", "-p"], "plan")[-2:] == ["--permission-mode", "plan"]
    # An explicit native value passes through.
    assert fn(["claude", "-p"], "bypassPermissions")[-1] == "bypassPermissions"
    # Existing flag is rewritten in place, not appended.
    rewritten = fn(["claude", "--permission-mode", "old", "-p"], "auto")
    assert rewritten.count("--permission-mode") == 1
    assert rewritten[rewritten.index("--permission-mode") + 1] == "acceptEdits"


def test_apply_grok_permission_modes():
    fn = CodingCliExecutor._apply_grok_permission_mode
    assert fn(["grok"], "yolo")[-1] == "acceptEdits"
    assert fn(["grok"], "prod")[-1] == "dontAsk"
    assert fn(["grok"], "plan")[-1] == "plan"


def test_apply_cursor_permission_mode(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    assert ex._apply_cursor_permission_mode(["cursor-agent", "--print"], "auto")[1] == "--force"
    assert "--mode=plan" in ex._apply_cursor_permission_mode(["cursor-agent", "--print"], "plan")
    # gated leaves the command untouched.
    assert ex._apply_cursor_permission_mode(["cursor-agent"], "gated") == ["cursor-agent"]


def test_apply_model_override(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.profile = SimpleNamespace(model="gpt-5", permission_mode="", extra_args=[], env={})
    # Codex accepts --model -> appended.
    assert ex._apply_model_override(["codex", "exec"])[-2:] == ["--model", "gpt-5"]
    # Existing flag rewritten.
    assert ex._apply_model_override(["codex", "--model", "old"])[-1] == "gpt-5"


def test_apply_model_override_noop_without_model(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.profile = SimpleNamespace(model="", permission_mode="", extra_args=[], env={})
    assert ex._apply_model_override(["codex", "exec"]) == ["codex", "exec"]


# ---- warp command / config ----------------------------------------------------

def test_load_warp_config_env_overrides(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    monkeypatch.setenv("DEVCOUNCIL_WARP_COMMAND", "oz")
    monkeypatch.setenv("DEVCOUNCIL_WARP_RUN_MODE", "cloud")
    monkeypatch.setenv("DEVCOUNCIL_WARP_PROFILE", "prod")
    monkeypatch.setenv("DEVCOUNCIL_WARP_MODEL", "sonnet")
    monkeypatch.setenv("DEVCOUNCIL_WARP_ENVIRONMENT", "staging")
    ex = CodingCliExecutor(tmp_path, "warp")
    cfg = ex._load_warp_config()
    assert cfg["command"] == "oz"
    assert cfg["run_mode"] == "cloud"
    assert cfg["profile"] == "prod"
    assert cfg["model"] == "sonnet"
    assert cfg["environment"] == "staging"


def test_warp_command_cloud_mode_shape(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    monkeypatch.setenv("DEVCOUNCIL_WARP_RUN_MODE", "cloud")
    monkeypatch.setenv("DEVCOUNCIL_WARP_PROFILE", "prod")
    monkeypatch.setenv("DEVCOUNCIL_WARP_MODEL", "sonnet")
    ex = CodingCliExecutor(tmp_path, "warp")
    cmd = ex._warp_command()
    assert cmd[:4] == ["oz", "agent", "run-cloud", "--name"]
    assert "--cwd" not in cmd  # cloud mode omits cwd
    assert "--profile" in cmd and "--model" in cmd
    assert cmd[-1] == "--prompt"


def test_warp_command_local_mode_includes_cwd(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "warp")
    cmd = ex._warp_command()
    assert "run" in cmd
    assert "--cwd" in cmd
    assert cmd[cmd.index("--cwd") + 1] == str(tmp_path)


def test_ensure_warp_mcp_config_upgrades_legacy_map(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "warp")
    path = tmp_path / ".devcouncil" / "integrations" / "warp-mcp.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"mcpServers": {"other": {}}}), encoding="utf-8")
    out = ex._ensure_warp_mcp_config({"mcp_config_path": str(path)})
    data = json.loads(out.read_text(encoding="utf-8"))
    assert data["devcouncil"]["command"] == "devcouncil"


# ---- grok session capture -----------------------------------------------------

def test_capture_grok_session_from_result_persists(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    _write_config(tmp_path, "execution:\n  grok_resume_mode: project\n")
    ex = CodingCliExecutor(tmp_path, "grok")
    result = subprocess.CompletedProcess(
        ["grok"], 0, stdout=json.dumps({"session_id": "grok-123", "result": "done"}), stderr=""
    )
    ex._capture_grok_session_from_result("TASK-001", result)
    assert ex.last_agent_session_id == "grok-123"
    session = json.loads(
        (tmp_path / ".devcouncil" / "integrations" / "grok-session.json").read_text(encoding="utf-8")
    )
    assert session["session_id"] == "grok-123"


def test_capture_grok_session_off_mode_noop(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "grok")  # default resume mode off
    result = subprocess.CompletedProcess(["grok"], 0, stdout=json.dumps({"session_id": "x"}), stderr="")
    ex._capture_grok_session_from_result("TASK-001", result)
    assert ex.last_agent_session_id is None


# ---- helpers ------------------------------------------------------------------

def test_display_invocation_masks_prompt(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    out = ex._display_invocation(["codex", "--prompt", "secret body"], "secret body")
    assert out == ["codex", "--prompt", "<task prompt>"]


def test_preview_lines_limits_and_redacts(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    text = "\n".join(f"line{i}" for i in range(50))
    assert ex._preview_lines(text, limit=5) == ["line0", "line1", "line2", "line3", "line4"]
    assert ex._preview_lines(None) == []


def test_effective_timeout_prefers_profile(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.profile = SimpleNamespace(timeout_seconds=99, model="", permission_mode="", extra_args=[], env={})
    assert ex._effective_timeout() == 99


def test_unsupported_client_raises(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    with pytest.raises(ValueError):
        CodingCliExecutor(tmp_path, "totally-unknown-cli-xyz")


# ---- stream output resolution -------------------------------------------------

def test_resolve_stream_output_explicit_and_exception(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    assert ex._resolve_stream_output(True) is True
    # A config object that raises on attribute access falls back to False.
    ex._config = object()
    assert ex._resolve_stream_output(None) is False
    ex._config = None
    assert ex._resolve_stream_output(None) is False


# ---- model / permission override edge cases -----------------------------------

def test_apply_model_override_client_without_flag(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "goose")  # goose absent from _MODEL_FLAGS
    ex.profile = SimpleNamespace(model="whatever", permission_mode="", extra_args=[], env={})
    assert ex._apply_model_override(["goose", "run"]) == ["goose", "run"]


def test_apply_grok_permission_mode_rewrites_existing_flag():
    fn = CodingCliExecutor._apply_grok_permission_mode
    out = fn(["grok", "--permission-mode", "old", "-p"], "yolo")
    assert out.count("--permission-mode") == 1
    assert out[out.index("--permission-mode") + 1] == "acceptEdits"


def test_apply_cursor_permission_mode_idempotent(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    # --force already present -> unchanged.
    assert ex._apply_cursor_permission_mode(["cursor-agent", "--force"], "auto") == ["cursor-agent", "--force"]
    # --mode=plan already present -> unchanged.
    assert ex._apply_cursor_permission_mode(["cursor-agent", "--mode=plan"], "plan") == [
        "cursor-agent",
        "--mode=plan",
    ]


# ---- cursor / grok command shaping --------------------------------------------

def test_cursor_command_missing_executable_raises(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "cursor")
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.resolve_cursor_agent_executable", lambda: None
    )
    with pytest.raises(ValueError, match="not installed"):
        ex._cursor_command("TASK-1")


def test_cursor_command_stream_and_resume(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.resolve_cursor_agent_executable", lambda: "cursor-agent"
    )
    ex = CodingCliExecutor(tmp_path, "cursor")
    ex.stream_output = True
    monkeypatch.setattr(ex, "_cursor_resume_chat_id", lambda tid: "chat-9")
    cmd = ex._cursor_command("TASK-1")
    assert "stream-json" in cmd
    assert "--resume" in cmd and "chat-9" in cmd


def test_grok_command_stream_and_resume(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "grok")
    ex.stream_output = True
    monkeypatch.setattr(ex, "_grok_resume_session_id", lambda tid: "grok-42")
    cmd = ex._grok_command("TASK-1")
    assert "stream-json" in cmd
    assert cmd[cmd.index("--resume") + 1] == "grok-42"


# ---- warp config edge cases ---------------------------------------------------

def test_warp_command_environment_and_share(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "warp")
    monkeypatch.setattr(
        ex,
        "_load_warp_config",
        lambda: {"command": "oz", "run_mode": "local", "environment": "prod", "share": ["a", "b"]},
    )
    cmd = ex._warp_command()
    assert cmd[cmd.index("--environment") + 1] == "prod"
    assert cmd.count("--share") == 2


def test_load_warp_config_handles_broken_config(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    for var in (
        "DEVCOUNCIL_WARP_COMMAND",
        "DEVCOUNCIL_WARP_RUN_MODE",
        "DEVCOUNCIL_WARP_PROFILE",
        "DEVCOUNCIL_WARP_MODEL",
        "DEVCOUNCIL_WARP_ENVIRONMENT",
    ):
        monkeypatch.delenv(var, raising=False)
    ex = CodingCliExecutor(tmp_path, "warp")
    ex._config = object()  # accessing .integrations raises -> data == {}
    assert ex._load_warp_config() == {}


def test_ensure_warp_mcp_config_invalid_json(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "warp")
    path = tmp_path / ".devcouncil" / "integrations" / "warp-mcp.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("not json", encoding="utf-8")
    # Invalid JSON is tolerated (treated as {}), so no rewrite happens.
    out = ex._ensure_warp_mcp_config({"mcp_config_path": str(path)})
    assert out == path
    assert path.read_text(encoding="utf-8") == "not json"


# ---- run_task failure branches ------------------------------------------------

def test_run_task_command_build_failure(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "cursor")
    # Executable resolves at construction but disappears when the command is built.
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.resolve_cursor_agent_executable", lambda: None
    )
    result = ex.run_task(_task(), [])
    assert not result.success
    assert "not installed" in result.message


def test_run_task_executable_not_on_path(tmp_path, monkeypatch):
    # which() succeeds at construction (spec resolution) but the executable itself
    # is not found when run_task probes PATH.
    seen = {"n": 0}

    def which(cmd, *a, **k):
        seen["n"] += 1
        # First calls (construction) succeed; deny the command lookup in run_task.
        return None if cmd == "codex" and seen["n"] > 3 else f"/usr/bin/{cmd}"

    monkeypatch.setattr("shutil.which", which)
    ex = CodingCliExecutor(tmp_path, "codex")
    monkeypatch.setattr("shutil.which", lambda c, *a, **k: None)
    result = ex.run_task(_task(), [])
    assert not result.success
    assert "not installed or not on PATH" in result.message


def test_run_task_scope_revert_fails_run(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    monkeypatch.setattr(
        "subprocess.run",
        lambda cmd, **k: subprocess.CompletedProcess(cmd, 0, stdout="ok", stderr=""),
    )
    monkeypatch.setattr(
        "devcouncil.planning.correction_manifest.repair_prompt_prefix",
        lambda root, tid: "REPAIR-PREFIX\n",
    )
    ex = CodingCliExecutor(tmp_path, "codex")
    monkeypatch.setattr(ex, "_scope_enforcement_enabled", lambda: True)
    monkeypatch.setattr(ex, "_enforce_file_scope", lambda task: [("src/x.py", "out of scope")])
    result = ex.run_task(_task(), [])
    assert not result.success
    assert "out-of-scope" in result.message
    # The repair prefix was prepended to the written instruction file.
    instruction = (tmp_path / ".devcouncil" / "TASK-001-codex-task.md").read_text(encoding="utf-8")
    assert instruction.startswith("REPAIR-PREFIX")


# ---- _revert_path (real git) --------------------------------------------------

def _init_git_repo(root):
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.email", "t@t.io"], cwd=root, check=True)
    subprocess.run(["git", "config", "user.name", "t"], cwd=root, check=True)


def test_revert_path_restores_tracked_file(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    _init_git_repo(tmp_path)
    tracked = tmp_path / "keep.py"
    tracked.write_text("original\n", encoding="utf-8")
    subprocess.run(["git", "add", "keep.py"], cwd=tmp_path, check=True)
    subprocess.run(["git", "commit", "-qm", "init"], cwd=tmp_path, check=True)
    tracked.write_text("tampered\n", encoding="utf-8")
    ex = CodingCliExecutor(tmp_path, "codex")
    assert ex._revert_path("keep.py") is True
    assert tracked.read_text(encoding="utf-8") == "original\n"


def test_revert_path_deletes_new_file(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    _init_git_repo(tmp_path)
    seed = tmp_path / "seed.py"
    seed.write_text("x\n", encoding="utf-8")
    subprocess.run(["git", "add", "seed.py"], cwd=tmp_path, check=True)
    subprocess.run(["git", "commit", "-qm", "init"], cwd=tmp_path, check=True)
    new_file = tmp_path / "extra.py"
    new_file.write_text("new\n", encoding="utf-8")
    ex = CodingCliExecutor(tmp_path, "codex")
    assert ex._revert_path("extra.py") is True
    assert not new_file.exists()


def test_revert_path_returns_false_on_exception(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "codex")

    def boom(*a, **k):
        raise RuntimeError("git unavailable")

    monkeypatch.setattr("subprocess.run", boom)
    assert ex._revert_path("anything.py") is False


# ---- windows shim resolution --------------------------------------------------

def test_resolve_invocation_windows_batch_shim(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    monkeypatch.setattr("devcouncil.executors.coding_cli.os.name", "nt")
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.shutil.which", lambda cmd, path=None: "C:\\bin\\codex.CMD"
    )
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.os.environ", {"COMSPEC": "C:\\Windows\\cmd.exe"}
    )
    out = ex._resolve_invocation(["codex", "exec"], {"PATH": "C:\\bin"})
    assert out[:2] == ["C:\\Windows\\cmd.exe", "/c"]
    assert out[-1] == "exec"


def test_resolve_invocation_noop_off_windows(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    assert ex._resolve_invocation(["codex"], {}) == ["codex"]


# ---- streaming subprocess -----------------------------------------------------

def test_run_subprocess_streaming_with_transcript(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.stream_output = True
    monkeypatch.setattr(ex, "_effective_timeout", lambda: 30)
    transcript = tmp_path / "runs" / "transcript.txt"
    script = "import sys; sys.stdin.read(); print('alpha'); print('beta')"

    # display_transform raises for one line (exercises the fallback), suppresses another.
    def transform(line):
        if "alpha" in line:
            raise RuntimeError("boom")
        return None

    result = ex._run_subprocess(
        [sys.executable, "-c", script],
        input_text="stdin-data",
        env=dict(**os.environ),
        transcript_path=transcript,
        display_transform=transform,
    )
    assert result.returncode == 0
    assert "alpha" in result.stdout and "beta" in result.stdout
    assert transcript.exists()
    assert "alpha" in transcript.read_text(encoding="utf-8")


# ---- resume mode resolution ---------------------------------------------------

def test_cursor_resume_mode_invalid_and_exception(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    _write_config(tmp_path, "execution:\n  cursor_resume_mode: bogus\n")
    ex = CodingCliExecutor(tmp_path, "cursor")
    assert ex._cursor_resume_mode() == "off"
    ex._config = object()
    assert ex._cursor_resume_mode() == "off"


def test_grok_resume_mode_invalid_and_exception(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    _write_config(tmp_path, "execution:\n  grok_resume_mode: bogus\n")
    ex = CodingCliExecutor(tmp_path, "grok")
    assert ex._grok_resume_mode() == "off"
    ex._config = object()
    assert ex._grok_resume_mode() == "off"


def test_cursor_session_path_task_mode(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "cursor")
    monkeypatch.setattr(ex, "_cursor_resume_mode", lambda: "task")
    path = ex._cursor_session_path("TASK-77")
    assert path.name == "TASK-77-cursor.json"


# ---- cursor chat id creation --------------------------------------------------

def test_ensure_cursor_chat_id_no_executable(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "cursor")
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.resolve_cursor_agent_executable", lambda: None
    )
    assert ex._ensure_cursor_chat_id() is None


def test_ensure_cursor_chat_id_timeout(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "cursor")
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.resolve_cursor_agent_executable", lambda: "cursor-agent"
    )

    def timeout(*a, **k):
        raise subprocess.TimeoutExpired(["cursor-agent"], 60)

    monkeypatch.setattr("subprocess.run", timeout)
    assert ex._ensure_cursor_chat_id() is None


def test_ensure_cursor_chat_id_success_and_failure(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "cursor")
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.resolve_cursor_agent_executable", lambda: "cursor-agent"
    )
    monkeypatch.setattr(
        "subprocess.run",
        lambda *a, **k: subprocess.CompletedProcess(a[0], 0, stdout="\nchat-xyz\n", stderr=""),
    )
    assert ex._ensure_cursor_chat_id() == "chat-xyz"
    monkeypatch.setattr(
        "subprocess.run",
        lambda *a, **k: subprocess.CompletedProcess(a[0], 1, stdout="", stderr="err"),
    )
    assert ex._ensure_cursor_chat_id() is None


# ---- grok session persistence -------------------------------------------------

def test_grok_resume_session_id_reads_existing_and_bad_json(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "grok")
    monkeypatch.setattr(ex, "_grok_resume_mode", lambda: "project")
    path = ex._grok_session_path(None)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"session_id": "s-1"}), encoding="utf-8")
    assert ex._grok_resume_session_id(None) == "s-1"
    path.write_text("broken", encoding="utf-8")
    assert ex._grok_resume_session_id(None) is None


def test_persist_grok_session_off_and_oserror(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "grok")
    monkeypatch.setattr(ex, "_grok_resume_mode", lambda: "off")
    ex._persist_grok_session("s", None)  # no-op, off mode
    monkeypatch.setattr(ex, "_grok_resume_mode", lambda: "project")

    def raise_oserror(*a, **k):
        raise OSError("disk full")

    monkeypatch.setattr("devcouncil.executors.coding_cli.write_json", raise_oserror)
    ex._persist_grok_session("s", None)  # swallowed


def test_capture_grok_session_line_by_line_and_camelcase(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "grok")
    monkeypatch.setattr(ex, "_grok_resume_mode", lambda: "project")
    monkeypatch.setattr(ex, "_persist_grok_session", lambda *a: None)
    stdout = "noise line\n" + json.dumps({"sessionId": "grok-cc"}) + "\n"
    ex._capture_grok_session_from_result("TASK-1", subprocess.CompletedProcess([], 0, stdout=stdout))
    assert ex.last_agent_session_id == "grok-cc"


def test_capture_grok_session_empty_and_no_session(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "grok")
    monkeypatch.setattr(ex, "_grok_resume_mode", lambda: "project")
    ex._capture_grok_session_from_result("T", subprocess.CompletedProcess([], 0, stdout="   "))
    assert ex.last_agent_session_id is None
    # Valid JSON dict but no session id -> nothing recorded.
    ex._capture_grok_session_from_result(
        "T", subprocess.CompletedProcess([], 0, stdout=json.dumps({"other": 1}))
    )
    assert ex.last_agent_session_id is None


# ---- claude session helpers ---------------------------------------------------

def test_read_claude_session_id_bad_json(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    path = tmp_path / "sess.json"
    path.write_text("nope", encoding="utf-8")
    assert CodingCliExecutor._read_claude_session_id(path) is None
    assert CodingCliExecutor._read_claude_session_id(tmp_path / "missing.json") is None


def test_persist_claude_session_none_and_oserror(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "claude")
    ex._pending_claude_session = None
    ex._persist_claude_session()  # no-op branch
    ex._pending_claude_session = (tmp_path / "s" / "sess.json", "sid-1")

    def raise_oserror(*a, **k):
        raise OSError("nope")

    monkeypatch.setattr("devcouncil.executors.coding_cli.write_json", raise_oserror)
    ex._persist_claude_session()
    assert ex.last_agent_session_id == "sid-1"


def test_mirror_claude_transcript_no_session_and_error(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "claude")
    ex.last_agent_session_id = None
    ex._mirror_claude_transcript()  # no session -> returns early
    ex.last_agent_session_id = "sid"
    import devcouncil.live.transcripts as transcripts

    def boom(*a, **k):
        raise RuntimeError("mirror failed")

    monkeypatch.setattr(transcripts, "mirror_claude_transcript", boom)
    ex._mirror_claude_transcript()  # exception swallowed


# ---- claude json capture ------------------------------------------------------

def test_capture_claude_json_non_dict_and_no_text(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "claude")
    # Whole payload is a JSON list (not a dict) -> returned unchanged.
    proc = subprocess.CompletedProcess([], 0, stdout="[1, 2, 3]")
    assert ex._capture_claude_json("r", proc) is proc
    # Valid dict but no usable result text -> original returned.
    proc2 = subprocess.CompletedProcess([], 0, stdout=json.dumps({"session_id": "s"}))
    out = ex._capture_claude_json("r", proc2)
    assert out is proc2


def test_record_claude_result_meta_session_mismatch(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "claude")
    sess_path = tmp_path / ".devcouncil" / "sessions" / "claude-session.json"
    ex._pending_claude_session = (sess_path, "assigned-id")
    payload = {
        "session_id": "real-id",
        "total_cost_usd": 0.12,
        "num_turns": 3,
        "usage": {"input_tokens": 10, "output_tokens": 5},
    }
    ex._record_claude_result_meta("run-x", payload)
    assert ex.last_agent_session_id == "real-id"
    saved = json.loads(sess_path.read_text(encoding="utf-8"))
    assert saved["session_id"] == "real-id"


def test_capture_claude_stream_json_finds_result_event(tmp_path, monkeypatch):
    monkeypatch.setattr("shutil.which", lambda c: f"/usr/bin/{c}")
    ex = CodingCliExecutor(tmp_path, "claude")
    recorded = {}
    monkeypatch.setattr(ex, "_record_claude_result_meta", lambda rid, p: recorded.update(p))
    lines = [
        "",
        "not json",
        json.dumps({"type": "assistant"}),
        json.dumps({"type": "result", "session_id": "s", "num_turns": 2}),
    ]
    ex._capture_claude_stream_json("r", subprocess.CompletedProcess([], 0, stdout="\n".join(lines)))
    assert recorded.get("session_id") == "s"


# ---- claude stream event rendering --------------------------------------------

def test_render_claude_stream_event_variants():
    fn = CodingCliExecutor._render_claude_stream_event
    assert fn("   ") is None
    assert fn("plain text line") == "plain text line"  # non-JSON passthrough
    assert fn("123") == "123"  # JSON but not a dict -> passthrough
    assistant = json.dumps(
        {
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "text", "text": "hello"},
                    {"type": "tool_use", "name": "Edit", "input": {"file_path": "a.py"}},
                    "ignored-non-dict",
                ]
            },
        }
    )
    out = fn(assistant)
    assert "hello" in out and "Edit" in out and "a.py" in out
    # assistant with no renderable content -> None
    assert fn(json.dumps({"type": "assistant", "message": {"content": []}})) is None
    result = fn(json.dumps({"type": "result", "num_turns": 4, "total_cost_usd": 0.5}))
    assert "4 turns" in result and "$0.5000" in result
    assert fn(json.dumps({"type": "result"})) is None  # no bits
    assert fn(json.dumps({"type": "system"})) is None


# ---- invocation shaping -------------------------------------------------------

def test_invocation_argument_with_prompt_arg(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.spec = SimpleNamespace(input_mode="argument", prompt_arg="--message")
    ex.profile = SimpleNamespace(extra_args=["--flag"], model="", permission_mode="", env={})
    inv, stdin = ex._invocation(["aider"], "PROMPT", tmp_path / "p.md")
    assert stdin is None
    assert inv == ["aider", "--flag", "--message", "PROMPT"]


def test_invocation_prompt_file_with_prompt_arg(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.spec = SimpleNamespace(input_mode="prompt-file", prompt_arg="--file")
    ex.profile = SimpleNamespace(extra_args=[], model="", permission_mode="", env={})
    instruction = tmp_path / "p.md"
    inv, stdin = ex._invocation(["goose", "run"], "PROMPT", instruction)
    assert stdin is None
    assert inv == ["goose", "run", "--file", str(instruction)]


def test_invocation_argument_with_prompt_placeholder(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.spec = SimpleNamespace(input_mode="argument", prompt_arg=None)
    ex.profile = SimpleNamespace(extra_args=["-x"], model="", permission_mode="", env={})
    inv, stdin = ex._invocation(["tool", "{prompt}"], "PROMPT", tmp_path / "p.md")
    assert stdin is None
    assert inv == ["tool", "PROMPT", "-x"]


# ---- profile prompt / summary helpers -----------------------------------------

def test_apply_profile_prompt_no_profile(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.profile = None
    assert ex._apply_profile_prompt("body") == "body"


def test_apply_profile_prompt_with_preamble_and_confirmation(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.profile = SimpleNamespace(prompt_preamble="PRE", require_explicit_confirmation=True)
    out = ex._apply_profile_prompt("body")
    assert "PRE" in out and "confirmation" in out and out.endswith("body")


def test_profile_override_summary_no_profile(tmp_path, monkeypatch):
    ex = _codex_executor(tmp_path, monkeypatch)
    ex.profile = None
    summary = ex._profile_override_summary()
    assert summary == {"extra_args": [], "permission_mode": None, "model": None, "env_keys": []}
