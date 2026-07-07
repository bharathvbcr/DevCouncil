"""Integration doctor and optional companion setup helpers."""

from __future__ import annotations

import shutil
from pathlib import Path

from rich.console import Console
from rich.table import Table

from devcouncil.executors.agent_registry import (
    VALID_INPUT_MODES,
    load_agent_profiles,
    load_cli_agent_specs,
)
from devcouncil.integrations.clients.common import _config_path, _load_raw_config, _save_raw_config
from devcouncil.utils.fsio import atomic_write_text

console = Console()


def build_integrations_doctor_table(root: Path) -> Table:
    """Build the integration doctor table (PATH probes + config presence)."""
    table = Table(title="DevCouncil Integration Doctor")
    table.add_column("Integration", style="cyan", no_wrap=True)
    table.add_column("Status")
    table.add_column("Notes", overflow="fold")

    checks = [
        ("Agent Flow", "agent-flow-app", "Optional live/replay visualizer for trace JSONL."),
        ("code-review-graph", "code-review-graph", "Optional structural graph context adapter."),
        ("Claude Code", "claude", "Optional MCP client and native hook runtime for pre-tool-use enforcement."),
        ("Codex CLI", "codex", "Optional MCP client, headless executor companion, and native hook runtime."),
        ("Gemini CLI", "gemini", "Optional MCP client companion and native hook runtime."),
        ("Cursor", "cursor-agent", "Optional MCP client, cursor-agent executor, and native hooks."),
        ("OpenCode", "opencode", "Optional MCP client and headless coding-agent executor."),
        ("Google Antigravity CLI", "agy", "Optional Antigravity CLI companion and headless coding-agent executor."),
        ("Warp / Oz", "oz", "Optional Warp/Oz CLI companion and agent executor."),
        ("Aider", "aider", "Optional headless executor via `dev run --executor aider` (no MCP)."),
    ]
    for label, executable, notes in checks:
        found = shutil.which(executable)
        table.add_row(label, "[green]OK[/green]" if found else "[yellow]Missing[/yellow]", found or notes)

    profiles = load_agent_profiles(root)
    for name, spec in load_cli_agent_specs(root).items():
        if spec.built_in:
            continue
        found = shutil.which(spec.executable)
        mode_ok = spec.input_mode in VALID_INPUT_MODES
        profile_ok = spec.default_profile in profiles
        status = "[green]OK[/green]" if found and mode_ok and profile_ok else "[red]Invalid[/red]"
        if not found:
            status = "[yellow]Missing[/yellow]"
        details = found or f"{spec.executable} not found on PATH"
        if not mode_ok:
            details = f"invalid input_mode={spec.input_mode}"
        if not profile_ok:
            details = f"{details}; missing profile={spec.default_profile}"
        table.add_row(f"CLI agent: {name}", status, details)

    config = _config_path(root)
    table.add_row(
        "DevCouncil config",
        "[green]OK[/green]" if config.exists() else "[red]Missing[/red]",
        str(config) if config.exists() else "Run dev init first.",
    )
    return table


def apply_agent_flow_setup(root: Path) -> Path:
    """Record Agent Flow trace integration in config and write docs stub."""
    trace_path = root / ".devcouncil" / "logs" / "traces.jsonl"
    config = _load_raw_config(root)
    integrations = config.setdefault("integrations", {})
    integrations["agent_flow"] = {
        "enabled": True,
        "trace_path": str(trace_path),
        "mode": "jsonl",
    }
    _save_raw_config(root, config)
    docs_dir = root / ".devcouncil" / "integrations"
    docs_dir.mkdir(parents=True, exist_ok=True)
    doc_path = docs_dir / "agent-flow.md"
    doc_path.write_text(
        "\n".join([
            "# Agent Flow",
            "",
            f"DevCouncil writes trace events to `{trace_path}`.",
            "",
            "Local replay:",
            "",
            "```bash",
            "dev trace tail --follow",
            "```",
            "",
            "External visualizers can watch the JSONL file directly. DevCouncil does not modify global editor or Claude Code settings from this setup command.",
            "",
        ]),
        encoding="utf-8",
    )
    return trace_path


def apply_code_review_graph_setup(root: Path) -> tuple[str | None, Path, bool]:
    """Record code-review-graph integration, ensure ignore file, and write docs stub."""
    executable = shutil.which("code-review-graph")
    ignore_path = root / ".code-review-graphignore"
    created = False
    if not ignore_path.exists():
        # Guarded by exists(): a torn write here would never be repaired, so write atomically.
        atomic_write_text(
            ignore_path,
            "\n".join([
                ".devcouncil/**",
                ".git/**",
                ".venv/**",
                "dist/**",
                "node_modules/**",
                "",
            ]),
        )
        created = True

    config = _load_raw_config(root)
    integrations = config.setdefault("integrations", {})
    integrations["code_review_graph"] = {
        "enabled": True,
        "command": "code-review-graph",
        "optional": True,
    }
    _save_raw_config(root, config)
    docs_dir = root / ".devcouncil" / "integrations"
    docs_dir.mkdir(parents=True, exist_ok=True)
    (docs_dir / "code-review-graph.md").write_text(
        "\n".join([
            "# code-review-graph",
            "",
            "Install and build the graph outside DevCouncil:",
            "",
            "```bash",
            "pipx install code-review-graph",
            "code-review-graph build",
            "```",
            "",
            "DevCouncil uses this as an optional context adapter for mapping, prompts, verification traces, and MCP graph context.",
            "",
        ]),
        encoding="utf-8",
    )
    return executable, ignore_path, created
