"""Unit tests for Anthropic advisor tool wiring (config, CLI, SDK, settings, pairing)."""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace

import yaml

from devcouncil.app.config import CliAgentProfileConfig, load_config
from devcouncil.executors.advisor_tool import (
    ADVISOR_INFRA_FAILURE_MARKERS,
    ADVISOR_STEERING_NUDGE,
    advisor_pairing_ok,
    advisor_steering_text,
    decide_advisor_attach,
    parse_claude_version,
    strip_duplicate_advisor_args,
)
from devcouncil.executors.claude_sdk import ClaudeSdkExecutor
from devcouncil.executors.coding_cli import CodingCliExecutor
from devcouncil.integrations.clients import claude as claude_client


def _executor(tmp_path: Path, client: str, profile: CliAgentProfileConfig) -> CodingCliExecutor:
    executor = CodingCliExecutor(tmp_path, client, profile="custom")
    executor.profile = profile
    executor.profile_name = "custom"
    return executor


def test_config_loads_advisor_model(tmp_path):
    cfg_dir = tmp_path / ".devcouncil"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text(
        yaml.dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {
                            "default": {
                                "model": "sonnet",
                                "advisor_model": "opus",
                            }
                        }
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    config = load_config(tmp_path)
    profile = config.integrations.cli_agents.profiles["default"]
    assert profile.model == "sonnet"
    assert profile.advisor_model == "opus"


def test_claude_cli_argv_gets_advisor(tmp_path):
    profile = CliAgentProfileConfig(model="sonnet", advisor_model="opus")
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    assert "--advisor" in command
    assert command[command.index("--advisor") + 1] == "opus"
    assert executor._advisor_attached is True


def test_non_claude_ignores_advisor_model(tmp_path):
    profile = CliAgentProfileConfig(model="gpt-5", advisor_model="opus")
    executor = _executor(tmp_path, "codex", profile)
    command = executor._command()
    assert "--advisor" not in command
    assert executor._advisor_attached is False


def test_claude_cli_advisor_on_resume_rebuild(tmp_path):
    """Repair spawns also get --advisor (profile wiring on every _command)."""
    session_dir = tmp_path / ".devcouncil" / "sessions"
    session_dir.mkdir(parents=True)
    (session_dir / "TASK-1-claude.json").write_text(
        json.dumps({"session_id": "sess-repair-1"}),
        encoding="utf-8",
    )
    profile = CliAgentProfileConfig(model="sonnet", advisor_model="opus")
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command("TASK-1")
    assert "--resume" in command
    assert command[command.index("--resume") + 1] == "sess-repair-1"
    assert "--advisor" in command
    assert command[command.index("--advisor") + 1] == "opus"


def test_pairing_preflight_skips_bad_pairs(tmp_path):
    assert advisor_pairing_ok("opus", "sonnet")[0] is False
    assert advisor_pairing_ok("sonnet", "haiku")[0] is False
    assert advisor_pairing_ok("fable", "opus")[0] is False
    assert advisor_pairing_ok("sonnet", "opus")[0] is True

    profile = CliAgentProfileConfig(model="opus", advisor_model="sonnet")
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    assert "--advisor" not in command
    assert executor._advisor_attached is False


def test_soft_skip_does_not_inject_steering(tmp_path, monkeypatch):
    """P0: soft-skip must not prefix 'consult the advisor' steering."""
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.claude_supports_append_system_prompt",
        lambda: False,
    )
    profile = CliAgentProfileConfig(model="opus", advisor_model="sonnet")
    executor = _executor(tmp_path, "claude", profile)
    executor._command()  # triggers soft-skip → _advisor_attached=False
    assert executor._advisor_attached is False
    prompt, system = executor._apply_advisor_steering("BODY", repair=False)
    assert prompt == "BODY"
    assert system is None
    assert "advisor" not in prompt.lower()


def test_provider_env_soft_skips_attach(tmp_path, monkeypatch):
    profile = CliAgentProfileConfig(
        model="sonnet",
        advisor_model="opus",
        env={"CLAUDE_CODE_USE_BEDROCK": "1"},
    )
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    assert "--advisor" not in command
    assert executor._advisor_attached is False
    decision, _, reason = decide_advisor_attach(
        main_model="sonnet",
        advisor_model="opus",
        env={"CLAUDE_CODE_USE_VERTEX": "1"},
    )
    assert decision == "skip"
    assert reason is not None
    assert "CLAUDE_CODE_USE_VERTEX" in reason


def test_extra_args_advisor_deduped_when_attached(tmp_path):
    profile = CliAgentProfileConfig(
        model="sonnet",
        advisor_model="opus",
        extra_args=["--advisor", "haiku", "--max-turns", "3"],
    )
    executor = _executor(tmp_path, "claude", profile)
    command = executor._command()
    assert executor._advisor_attached is True
    assert command.count("--advisor") == 1
    assert command[command.index("--advisor") + 1] == "opus"
    invocation, _ = executor._invocation(command, "PROMPT", tmp_path / "p.md")
    assert invocation.count("--advisor") == 1
    assert "--max-turns" in invocation
    assert "haiku" not in invocation


def test_strip_duplicate_advisor_args():
    assert strip_duplicate_advisor_args(["--foo", "--advisor", "x", "--bar"]) == [
        "--foo",
        "--bar",
    ]
    assert strip_duplicate_advisor_args(["--advisor=opus", "--ok"]) == ["--ok"]


def test_profile_override_summary_includes_advisor(tmp_path):
    profile = CliAgentProfileConfig(model="sonnet", advisor_model="opus")
    executor = _executor(tmp_path, "claude", profile)
    summary = executor._profile_override_summary()
    assert summary["advisor_model"] == "opus"
    assert summary["model"] == "sonnet"


def test_repair_steering_includes_authority(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.claude_supports_append_system_prompt",
        lambda: False,
    )
    profile = CliAgentProfileConfig(model="sonnet", advisor_model="opus")
    executor = _executor(tmp_path, "claude", profile)
    executor._advisor_attached = True
    prompt, system = executor._apply_advisor_steering("BODY", repair=True)
    assert system is None
    assert "correction_manifest" in prompt
    assert "authoritative" in prompt.lower()
    assert "BEFORE substantive work" in prompt
    assert "Give the advice serious weight" in prompt
    assert "~80 words" in prompt
    assert prompt.endswith("BODY")


def test_append_system_prompt_when_supported(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.executors.coding_cli.claude_supports_append_system_prompt",
        lambda: True,
    )
    profile = CliAgentProfileConfig(model="sonnet", advisor_model="opus")
    executor = _executor(tmp_path, "claude", profile)
    executor._advisor_attached = True
    prompt, system = executor._apply_advisor_steering("BODY", repair=True)
    assert "~80 words" in prompt
    assert prompt.endswith("BODY")
    assert system is not None
    assert "BEFORE substantive work" in system
    assert "correction_manifest" in system


def test_advisor_steering_text_has_coding_blocks():
    text = advisor_steering_text(repair=False)
    assert "BEFORE substantive work" in text
    assert "Give the advice serious weight" in text
    assert "correction_manifest" not in text
    repair = advisor_steering_text(repair=True)
    assert "correction_manifest" in repair


def test_interactive_nudge_constant_stable():
    assert "When the advisor tool is available" in ADVISOR_STEERING_NUDGE


def test_render_claude_stream_advisor_event():
    render = CodingCliExecutor._render_claude_stream_event
    event = json.dumps(
        {
            "type": "assistant",
            "message": {
                "content": [
                    {
                        "type": "server_tool_use",
                        "name": "advisor",
                        "input": {"model": "opus"},
                    }
                ]
            },
        }
    )
    assert render(event) == "Advising (opus)\n"


def test_sdk_options_get_extra_args_advisor(tmp_path, monkeypatch):
    calls: dict = {}

    class Options:
        def __init__(self, **kw):
            self.__dict__.update(kw)

    sdk = SimpleNamespace(ClaudeAgentOptions=Options)
    ex = ClaudeSdkExecutor(tmp_path, model="sonnet", advisor_model="opus")
    options = ex._build_options(sdk, can_use_tool=lambda *a, **k: None, advisor_extra={"advisor": "opus"})
    assert options.extra_args == {"advisor": "opus"}
    assert options.permission_mode == "default"
    assert decide_advisor_attach(main_model="sonnet", advisor_model="opus")[0] == "attach"
    assert calls == {}


def test_sdk_options_have_advisor_false_when_dropped(tmp_path):
    class StrictOptions:
        can_use_tool = None

        def __init__(self, cwd=None, permission_mode=None, model=None, env=None):
            self.cwd = cwd
            self.permission_mode = permission_mode
            self.model = model
            self.env = env

    sdk = SimpleNamespace(ClaudeAgentOptions=StrictOptions)
    ex = ClaudeSdkExecutor(tmp_path, model="sonnet", advisor_model="opus")
    options = ex._build_options(
        sdk, can_use_tool=lambda *a, **k: None, advisor_extra={"advisor": "opus"}
    )
    assert ClaudeSdkExecutor._options_have_advisor(options, {"advisor": "opus"}) is False
    assert ClaudeSdkExecutor._options_have_advisor(
        SimpleNamespace(extra_args={"advisor": "opus"}), {"advisor": "opus"}
    )


def test_sdk_extra_args_degrade_warns(tmp_path, caplog):
    class StrictOptions:
        can_use_tool = None

        def __init__(self, cwd=None, permission_mode=None, model=None, env=None):
            self.cwd = cwd
            self.permission_mode = permission_mode
            self.model = model
            self.env = env

    sdk = SimpleNamespace(ClaudeAgentOptions=StrictOptions)
    ex = ClaudeSdkExecutor(tmp_path, model="sonnet", advisor_model="opus")
    with caplog.at_level("WARNING"):
        options = ex._build_options(
            sdk, can_use_tool=lambda *a, **k: None, advisor_extra={"advisor": "opus"}
        )
    assert options.model == "sonnet"
    assert not hasattr(options, "extra_args")
    assert any("extra_args" in record.message for record in caplog.records)


def test_settings_merge_advisor_only_when_configured(tmp_path):
    cfg_dir = tmp_path / ".devcouncil"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text(
        yaml.dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {"default": {"model": "sonnet", "advisor_model": "opus"}}
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    path, changed = claude_client._install_claude_settings(tmp_path)
    assert changed
    settings = json.loads(path.read_text(encoding="utf-8"))
    assert settings["advisorModel"] == "opus"

    # Unset profile: leave existing advisorModel alone.
    (cfg_dir / "config.yaml").write_text(
        yaml.dump({"integrations": {"cli_agents": {"profiles": {"default": {}}}}}),
        encoding="utf-8",
    )
    path2, changed2 = claude_client._install_claude_settings(tmp_path)
    settings2 = json.loads(path2.read_text(encoding="utf-8"))
    assert settings2["advisorModel"] == "opus"
    # Only status/permissions may change; advisor must remain.
    assert "advisorModel" in settings2


def test_settings_merge_skips_bad_pairing(tmp_path):
    cfg_dir = tmp_path / ".devcouncil"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text(
        yaml.dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {"default": {"model": "opus", "advisor_model": "sonnet"}}
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    path, _changed = claude_client._install_claude_settings(tmp_path)
    settings = json.loads(path.read_text(encoding="utf-8"))
    assert "advisorModel" not in settings


def test_settings_prefers_default_profile_not_other(tmp_path):
    cfg_dir = tmp_path / ".devcouncil"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text(
        yaml.dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {
                            "other": {"model": "sonnet", "advisor_model": "opus"},
                            "default": {"model": "sonnet"},
                        }
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    path, _ = claude_client._install_claude_settings(tmp_path)
    settings = json.loads(path.read_text(encoding="utf-8"))
    assert "advisorModel" not in settings


def test_uninstall_pops_devcouncil_advisor_model(tmp_path):
    cfg_dir = tmp_path / ".devcouncil"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text(
        yaml.dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {"default": {"model": "sonnet", "advisor_model": "opus"}}
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    settings_path = tmp_path / ".claude" / "settings.local.json"
    settings_path.parent.mkdir(parents=True)
    settings_path.write_text(
        json.dumps(
            {
                "advisorModel": "opus",
                "statusLine": {"type": "command", "command": "devcouncil hook claude-statusline"},
                "userKey": "keep-me",
            }
        ),
        encoding="utf-8",
    )
    removed = claude_client._uninstall_claude(tmp_path)
    assert "advisorModel" in removed
    remaining = json.loads(settings_path.read_text(encoding="utf-8"))
    assert "advisorModel" not in remaining
    assert remaining.get("userKey") == "keep-me"


def test_uninstall_preserves_foreign_advisor_model(tmp_path):
    cfg_dir = tmp_path / ".devcouncil"
    cfg_dir.mkdir(parents=True)
    (cfg_dir / "config.yaml").write_text(
        yaml.dump(
            {
                "integrations": {
                    "cli_agents": {
                        "profiles": {"default": {"model": "sonnet", "advisor_model": "opus"}}
                    }
                }
            }
        ),
        encoding="utf-8",
    )
    settings_path = tmp_path / ".claude" / "settings.local.json"
    settings_path.parent.mkdir(parents=True)
    settings_path.write_text(json.dumps({"advisorModel": "fable", "userKey": 1}), encoding="utf-8")
    claude_client._uninstall_claude(tmp_path)
    remaining = json.loads(settings_path.read_text(encoding="utf-8"))
    assert remaining["advisorModel"] == "fable"


def test_advisor_infra_markers_shared():
    assert "does not support the advisor" in ADVISOR_INFRA_FAILURE_MARKERS
    assert "--advisor" in ADVISOR_INFRA_FAILURE_MARKERS


def test_parse_claude_version():
    assert parse_claude_version("2.1.207 (Claude Code)") == (2, 1, 207)
    assert parse_claude_version("nope") is None


def test_fable_version_warn(monkeypatch):
    from devcouncil.executors import advisor_tool as at

    monkeypatch.setattr(at, "probe_claude_version", lambda: (2, 1, 100))
    warnings = at.warn_advisor_preflight(main_model="sonnet", advisor_model="fable")
    assert any("2.1.170" in w for w in warnings)
    monkeypatch.setattr(at, "probe_claude_version", lambda: (2, 1, 200))
    assert not any("2.1.170" in w for w in at.warn_advisor_preflight(advisor_model="fable"))
