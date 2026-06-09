from __future__ import annotations

import shutil
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from devcouncil.app.config import CliAgentProfileConfig, load_config


AGENT_ALIASES = {
    "agy": "antigravity",
    "agy-cli": "antigravity",
    "antigravity-cli": "antigravity",
    "google-antigravity": "antigravity",
    "codex-cli": "codex",
    "gemini-cli": "gemini",
    "claude-cli": "claude",
    "claude-code": "claude",
    "opencode-cli": "opencode",
    "open-code": "opencode",
    "warp-cli": "warp",
    "oz": "warp",
    "oz-cli": "warp",
    "cursor-agent": "cursor",
    "cursor-cli": "cursor",
    "copilot-cli": "copilot",
    "github-copilot": "copilot",
    "gh-copilot": "copilot",
    "goose-cli": "goose",
    "block-goose": "goose",
    "amp-cli": "amp",
    "sourcegraph-amp": "amp",
    "qwen-code": "qwen",
    "qwen-cli": "qwen",
    "crush-cli": "crush",
    "charm-crush": "crush",
}

BUILTIN_AGENT_NAMES = {
    "aider",
    "amp",
    "antigravity",
    "claude",
    "codex",
    "copilot",
    "crush",
    "cursor",
    "gemini",
    "goose",
    "opencode",
    "qwen",
    "warp",
}

BUILTIN_CODING_EXECUTOR_NAMES = tuple(sorted(BUILTIN_AGENT_NAMES))
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
    capabilities: list[str] = field(default_factory=list)
    supports_handoff: bool = False
    preferred_artifact_format: str = "json"

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


def resolve_cursor_agent_executable() -> str | None:
    for candidate in ("cursor-agent", "agent"):
        if shutil.which(candidate):
            return candidate
    return None


CODING_CLI_PROBE_ORDER: tuple[str, ...] = (
    "codex",
    "gemini",
    "claude",
    "cursor",
    "opencode",
    "antigravity",
    "warp",
    "aider",
    "copilot",
    "goose",
    "amp",
    "qwen",
    "crush",
)

@dataclass(frozen=True)
class CodingCliIntegrationInfo:
    name: str
    label: str
    tier: int
    headless: bool
    mcp: bool
    hooks: bool
    launcher_shim: bool
    notes: str


# Tier 1 = headless executor + verify; tier 2 = MCP companion; hooks = native pre-tool policy.
CODING_CLI_INTEGRATION_INFO: dict[str, CodingCliIntegrationInfo] = {
    "codex": CodingCliIntegrationInfo(
        "codex", "Codex CLI", 1, True, True, True, True, "dev integrate codex --apply"
    ),
    "gemini": CodingCliIntegrationInfo(
        "gemini", "Gemini CLI", 1, True, True, True, True, "dev integrate gemini --apply"
    ),
    "claude": CodingCliIntegrationInfo(
        "claude", "Claude Code", 1, True, True, True, True, "dev integrate claude --apply"
    ),
    "cursor": CodingCliIntegrationInfo(
        "cursor",
        "Cursor",
        1,
        True,
        True,
        True,
        True,
        "dev integrate cursor --apply; cursor-agent for headless",
    ),
    "opencode": CodingCliIntegrationInfo(
        "opencode", "OpenCode", 1, True, True, True, True, "dev integrate opencode --apply"
    ),
    "antigravity": CodingCliIntegrationInfo(
        "antigravity",
        "Google Antigravity CLI",
        1,
        True,
        True,
        False,
        True,
        "dev integrate antigravity --apply",
    ),
    "warp": CodingCliIntegrationInfo(
        "warp", "Warp / Oz", 1, True, True, False, True, "dev integrate warp --apply"
    ),
    "aider": CodingCliIntegrationInfo(
        "aider", "Aider", 1, True, False, False, True, "dev integrate aider --apply"
    ),
    "copilot": CodingCliIntegrationInfo(
        "copilot", "GitHub Copilot CLI", 1, True, True, False, True, "dev run TASK-ID --executor copilot"
    ),
    "goose": CodingCliIntegrationInfo(
        "goose", "Goose", 1, True, True, False, True, "dev run TASK-ID --executor goose"
    ),
    "amp": CodingCliIntegrationInfo(
        "amp", "Amp (Sourcegraph)", 1, True, True, False, True, "dev run TASK-ID --executor amp"
    ),
    "qwen": CodingCliIntegrationInfo(
        "qwen", "Qwen Code", 1, True, True, False, True, "dev run TASK-ID --executor qwen"
    ),
    "crush": CodingCliIntegrationInfo(
        "crush", "Crush (Charm)", 1, True, True, False, True, "dev run TASK-ID --executor crush"
    ),
}

# First successful probe wins (cursor tries cursor-agent then cursor).
CODING_CLI_VERSION_COMMANDS: dict[str, tuple[tuple[str, ...], ...]] = {
    "codex": (("codex", "--version"),),
    "gemini": (("gemini", "--version"),),
    "claude": (("claude", "--version"),),
    "cursor": (("cursor-agent", "--version"), ("cursor", "--version")),
    "opencode": (("opencode", "--version"),),
    "antigravity": (("agy", "--version"),),
    "warp": (("oz", "--version"),),
    "aider": (("aider", "--version"),),
    "copilot": (("copilot", "--version"),),
    "goose": (("goose", "--version"),),
    "amp": (("amp", "--version"),),
    "qwen": (("qwen", "--version"),),
    "crush": (("crush", "--version"),),
}


def integration_tier_label(client: str) -> str:
    info = CODING_CLI_INTEGRATION_INFO.get(normalize_agent_name(client))
    tier = info.tier if info is not None else 3
    if tier == 1:
        return "Tier 1 (headless executor)"
    if tier == 2:
        return "Tier 2 (MCP companion)"
    return "Tier 3 (sidecar)"


def resolve_coding_cli_executable(project_root: Path, client: str) -> str | None:
    normalized = normalize_agent_name(client)
    if normalized == "cursor":
        return resolve_cursor_agent_executable()
    spec = get_cli_agent_spec(project_root, normalized)
    if not spec:
        return None
    return spec.executable if shutil.which(spec.executable) else None


def resolve_coding_cli_probe_order(project_root: Path) -> tuple[str, ...]:
    try:
        configured = load_config(project_root).execution.coding_cli_probe_order
        if configured:
            return tuple(normalize_agent_name(name) for name in configured)
    except Exception:
        pass
    return CODING_CLI_PROBE_ORDER


def detect_available_coding_cli(
    project_root: Path,
    probe_order: tuple[str, ...] | None = None,
) -> str | None:
    for client in probe_order or resolve_coding_cli_probe_order(project_root):
        if resolve_coding_cli_executable(project_root, client):
            return client
    return None


def resolve_automated_executor(
    project_root: Path,
    explicit: str | None = None,
    *,
    probe_order: tuple[str, ...] | None = None,
) -> str:
    if explicit:
        return normalize_agent_name(explicit)
    try:
        config = load_config(project_root).execution
        configured = normalize_agent_name(config.default_executor)
        if probe_order is None:
            probe_order = resolve_coding_cli_probe_order(project_root)
    except Exception:
        configured = "manual"
        if probe_order is None:
            probe_order = CODING_CLI_PROBE_ORDER
    if configured != "manual":
        return configured
    detected = detect_available_coding_cli(project_root, probe_order=probe_order)
    return detected or configured


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
            supports_handoff=True,
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
            supports_handoff=True,
            built_in=True,
        ),
        "opencode": CliAgentSpec(
            name="opencode",
            command="opencode",
            args=[
                "run",
                "--file",
                "{prompt_file}",
                "Execute the DevCouncil task described in the attached prompt file.",
            ],
            input_mode="prompt-file",
            display_name="OpenCode",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
            built_in=True,
        ),
        "antigravity": CliAgentSpec(
            name="antigravity",
            command="agy",
            args=[
                "--print",
                "--print-timeout",
                "30m",
                "Read and execute the DevCouncil task prompt at {prompt_file}.",
            ],
            input_mode="prompt-file",
            display_name="Google Antigravity CLI",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
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
            supports_handoff=True,
            built_in=True,
        ),
        "cursor": CliAgentSpec(
            name="cursor",
            command=resolve_cursor_agent_executable() or "cursor-agent",
            args=[
                "--print",
                "--trust",
                "--workspace",
                str(project_root),
                "Read and execute the DevCouncil task prompt at {prompt_file}.",
            ],
            input_mode="prompt-file",
            display_name="Cursor Agent",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
            built_in=True,
        ),
        "aider": CliAgentSpec(
            name="aider",
            command="aider",
            args=["--yes", "--no-show-model-warnings", "--message"],
            input_mode="argument",
            display_name="Aider",
            kind="coding-cli",
            supports_mcp=False,
            supports_diff_review=True,
            supports_handoff=True,
            built_in=True,
        ),
        "copilot": CliAgentSpec(
            name="copilot",
            command="copilot",
            args=["--allow-all-tools", "-p"],
            input_mode="argument",
            display_name="GitHub Copilot CLI",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
            built_in=True,
        ),
        "goose": CliAgentSpec(
            name="goose",
            command="goose",
            args=["run", "-i", "{prompt_file}"],
            input_mode="prompt-file",
            display_name="Goose",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
            built_in=True,
        ),
        "amp": CliAgentSpec(
            name="amp",
            command="amp",
            args=["-x"],
            input_mode="argument",
            display_name="Amp (Sourcegraph)",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
            built_in=True,
        ),
        "qwen": CliAgentSpec(
            name="qwen",
            command="qwen",
            display_name="Qwen Code",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
            built_in=True,
        ),
        "crush": CliAgentSpec(
            name="crush",
            command="crush",
            args=["run"],
            input_mode="argument",
            display_name="Crush (Charm)",
            kind="coding-cli",
            supports_mcp=True,
            supports_diff_review=True,
            supports_handoff=True,
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
