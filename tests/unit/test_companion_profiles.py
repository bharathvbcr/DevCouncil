"""Tests that profiles actually constrain the spawned CLI invocation."""

from pathlib import Path

from devcouncil.app.config import CliAgentProfileConfig
from devcouncil.executors.coding_cli import CodingCliExecutor


def _executor(tmp_path: Path, client: str, profile: CliAgentProfileConfig) -> CodingCliExecutor:
    executor = CodingCliExecutor(tmp_path, client, profile="custom")
    # Inject the profile directly so we don't depend on a config file.
    executor.profile = profile
    executor.profile_name = "custom"
    return executor


def test_empty_profile_reproduces_baseline(tmp_path):
    """An all-default profile must not change today's invocation (no regression)."""
    baseline = CodingCliExecutor(tmp_path, "claude", profile="default")
    base_command = baseline.spec.base_command()

    empty = _executor(tmp_path, "claude", CliAgentProfileConfig())
    assert empty._command() == base_command


def test_no_profile_reproduces_baseline(tmp_path):
    executor = CodingCliExecutor(tmp_path, "claude", profile="default")
    executor.profile = None
    assert executor._apply_profile_args(["claude", "-p"]) == ["claude", "-p"]


def test_extra_args_appear_in_stdin_invocation(tmp_path):
    # extra_args are placed by _invocation (not _command), so the prompt flag of an
    # argument-mode CLI can't be split from its value. For a stdin CLI they land at the tail.
    from pathlib import Path as _Path

    profile = CliAgentProfileConfig(extra_args=["--max-turns", "5"])
    executor = _executor(tmp_path, "claude", profile)
    invocation, stdin = executor._invocation(executor._command(), "PROMPT", _Path("instr.md"))
    assert "--max-turns" in invocation and "5" in invocation
    assert stdin == "PROMPT"  # claude reads the prompt from stdin


def test_extra_args_precede_trailing_prompt_flag(tmp_path):
    # The bug this guards: for an argument-mode CLI whose last base token IS the prompt
    # flag (e.g. warp --prompt / aider --message), extra_args must go BEFORE that flag so
    # it still binds to the prompt — not between the flag and its value.
    import dataclasses
    from pathlib import Path as _Path

    profile = CliAgentProfileConfig(extra_args=["--max-turns", "5"])
    executor = _executor(tmp_path, "claude", profile)
    executor.spec = dataclasses.replace(executor.spec, input_mode="argument", prompt_arg=None)
    invocation, stdin = executor._invocation(["tool", "--prompt"], "PROMPT", _Path("instr.md"))
    assert invocation == ["tool", "--max-turns", "5", "--prompt", "PROMPT"]
    assert stdin is None


def test_model_override_replaces_existing_flag(tmp_path):
    profile = CliAgentProfileConfig(model="claude-opus-4")
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    # Claude base has no --model, so it is appended.
    idx = command.index("--model")
    assert command[idx + 1] == "claude-opus-4"
    assert command.count("--model") == 1


def test_model_override_rewrites_in_place_for_codex(tmp_path):
    profile = CliAgentProfileConfig(model="gpt-5")
    executor = _executor(tmp_path, "codex", profile)
    command = executor._command()
    assert "--model" in command
    assert command[command.index("--model") + 1] == "gpt-5"


def test_permission_mode_gated_drops_accept_edits(tmp_path):
    baseline = CodingCliExecutor(tmp_path, "claude", profile="default")
    assert "acceptEdits" in baseline.spec.base_command()

    profile = CliAgentProfileConfig(permission_mode="gated")
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    assert "acceptEdits" not in command
    assert command[command.index("--permission-mode") + 1] == "default"


def test_permission_mode_auto_keeps_accept_edits(tmp_path):
    profile = CliAgentProfileConfig(permission_mode="auto")
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    assert command[command.index("--permission-mode") + 1] == "acceptEdits"


def test_native_permission_value_passes_through(tmp_path):
    profile = CliAgentProfileConfig(permission_mode="bypassPermissions")
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    assert command[command.index("--permission-mode") + 1] == "bypassPermissions"


def test_prod_and_yolo_produce_different_invocations(tmp_path):
    """The core fix: prod and yolo must no longer be byte-for-byte identical."""
    yolo = _executor(tmp_path, "claude", CliAgentProfileConfig(permission_mode="auto"))
    prod = _executor(tmp_path, "claude", CliAgentProfileConfig(permission_mode="gated"))
    assert yolo._command() != prod._command()


def test_permission_mode_ignored_for_unknown_client(tmp_path):
    profile = CliAgentProfileConfig(permission_mode="gated")
    executor = _executor(tmp_path, "gemini", profile)
    # Gemini has no permission-mode translation; baseline preserved.
    baseline = CodingCliExecutor(tmp_path, "gemini", profile="default").spec.base_command()
    assert executor._command() == baseline


def test_profile_override_summary_in_manifest(tmp_path):
    profile = CliAgentProfileConfig(
        extra_args=["--foo"], permission_mode="gated", model="claude-opus-4"
    )
    executor = _executor(tmp_path, "claude", profile)
    summary = executor._profile_override_summary()
    assert summary == {
        "extra_args": ["--foo"],
        "permission_mode": "gated",
        "model": "claude-opus-4",
    }


def test_default_profiles_distinguish_yolo_and_prod():
    from devcouncil.executors.agent_registry import default_agent_profiles

    profiles = default_agent_profiles()
    assert profiles["yolo"]["permission_mode"] == "auto"
    assert profiles["prod"]["permission_mode"] == "gated"
