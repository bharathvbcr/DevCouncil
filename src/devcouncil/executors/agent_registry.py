from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from devcouncil.app.config import CliAgentProfileConfig, load_config


AGENT_ALIASES = {
    "codex-cli": "codex",
    "gemini-cli": "gemini",
    "claude-cli": "claude",
    "claude-code": "claude",
    "warp-cli": "warp",
    "oz": "warp",
    "oz-cli": "warp",
}

BUILTIN_AGENT_NAMES = {"codex", "gemini", "claude", "warp"}
VALID_INPUT_MODES = {"stdin", "argument", "prompt-file"}


@dataclass(frozen=True)
class CliAgentSpec:
    name: str
    command: str
    args: list[str] = field(default_factory=list)
    input_mode: str = "stdin"
    prompt_arg: str | None = None
    timeout_seconds: int | None = None
    env: dict[str, str] = field(default_factory=dict)
    display_name: str | None = None
    kind: str = "custom"
    supports_mcp: bool = False
    supports_diff_review: bool = False
    default_profile: str = "default"
    help_command: list[str] = field(default_factory=list)
    built_in: bool = False

    @property
    def label(self) -> str:
        return self.display_name or self.name

    @property
    def executable(self) -> str:
        return self.command

    def base_command(self) -> list[str]:
        return [self.command, *self.args]


def normalize_agent_name(name: str) -> str:
    normalized = (name or "").strip().lower().replace("_", "-")
    return AGENT_ALIASES.get(normalized, normalized)


def is_reserved_agent_name(name: str) -> bool:
    return normalize_agent_name(name) in BUILTIN_AGENT_NAMES


def builtin_agent_specs(project_root: Path) -> dict[str, CliAgentSpec]:
    warp_mcp_path = project_root / ".devcouncil" / "integrations" / "warp-mcp.json"
    return {
        "codex": CliAgentSpec(
            name="codex",
            command="codex",
            args=["exec", "-"],
            display_name="Codex CLI",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            built_in=True,
        ),
        "gemini": CliAgentSpec(
            name="gemini",
            command="gemini",
            display_name="Gemini CLI",
            kind="coding-cli",
            supports_mcp=True,
            built_in=True,
        ),
        "claude": CliAgentSpec(
            name="claude",
            command="claude",
            args=["-p"],
            display_name="Claude Code",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            built_in=True,
        ),
        "warp": CliAgentSpec(
            name="warp",
            command="oz",
            args=["agent", "run", "--cwd", str(project_root), "--mcp", str(warp_mcp_path), "--prompt"],
            input_mode="argument",
            display_name="Warp / Oz",
            kind="agent-platform",
            supports_mcp=True,
            supports_diff_review=True,
            built_in=True,
        ),
    }


def load_cli_agent_specs(project_root: Path) -> dict[str, CliAgentSpec]:
    specs = builtin_agent_specs(project_root)
    try:
        configured = load_config(project_root).integrations.cli_agents.agents
    except Exception:
        configured = {}

    for raw_name, agent in configured.items():
        name = normalize_agent_name(raw_name)
        if name in BUILTIN_AGENT_NAMES:
            continue
        data = agent.model_dump()
        specs[name] = CliAgentSpec(
            name=name,
            command=data["command"],
            args=list(data.get("args") or []),
            input_mode=data.get("input_mode") or "stdin",
            prompt_arg=data.get("prompt_arg"),
            timeout_seconds=data.get("timeout_seconds"),
            env=dict(data.get("env") or {}),
            display_name=data.get("display_name"),
            kind=data.get("kind") or "custom",
            supports_mcp=bool(data.get("supports_mcp")),
            supports_diff_review=bool(data.get("supports_diff_review")),
            default_profile=data.get("default_profile") or "default",
            help_command=list(data.get("help_command") or []),
            built_in=False,
        )
    return specs


def get_cli_agent_spec(project_root: Path, name: str) -> CliAgentSpec | None:
    return load_cli_agent_specs(project_root).get(normalize_agent_name(name))


def default_agent_profiles() -> dict[str, dict[str, Any]]:
    return {
        "default": {
            "description": "Balanced local execution with DevCouncil verification.",
            "timeout_seconds": None,
            "prompt_preamble": "",
            "require_explicit_confirmation": False,
        },
        "yolo": {
            "description": "Faster local execution; DevCouncil still verifies the final diff.",
            "timeout_seconds": 3600,
            "prompt_preamble": (
                "Profile: yolo. Move efficiently within the task scope, but keep all changes "
                "inside the planned files and run the expected verification commands."
            ),
            "require_explicit_confirmation": False,
        },
        "prod": {
            "description": "Restrictive execution for high-risk repositories.",
            "timeout_seconds": 1800,
            "prompt_preamble": (
                "Profile: prod. Treat this as high-risk work. Do not broaden scope, do not "
                "change dependencies or schemas unless the task explicitly allows it, and "
                "prefer minimal, easily reviewed edits."
            ),
            "require_explicit_confirmation": True,
        },
    }


def load_agent_profiles(project_root: Path) -> dict[str, CliAgentProfileConfig]:
    raw_profiles = default_agent_profiles()
    try:
        configured = load_config(project_root).integrations.cli_agents.profiles
    except Exception:
        configured = {}
    profiles = {
        name: CliAgentProfileConfig.model_validate(data)
        for name, data in raw_profiles.items()
    }
    profiles.update(configured)
    return profiles


def agent_config_entry(
    *,
    command: str,
    args: list[str] | None = None,
    input_mode: str = "stdin",
    prompt_arg: str | None = None,
    timeout_seconds: int | None = None,
    env: dict[str, str] | None = None,
    display_name: str | None = None,
    kind: str = "custom",
    supports_mcp: bool = False,
    supports_diff_review: bool = False,
    default_profile: str = "default",
    help_command: list[str] | None = None,
) -> dict[str, Any]:
    entry: dict[str, Any] = {
        "command": command,
        "args": args or [],
        "input_mode": input_mode,
        "kind": kind,
        "supports_mcp": supports_mcp,
        "supports_diff_review": supports_diff_review,
        "default_profile": default_profile,
    }
    if prompt_arg:
        entry["prompt_arg"] = prompt_arg
    if timeout_seconds is not None:
        entry["timeout_seconds"] = timeout_seconds
    if env:
        entry["env"] = env
    if display_name:
        entry["display_name"] = display_name
    if help_command:
        entry["help_command"] = help_command
    return entry
