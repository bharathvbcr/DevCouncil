"""Cursor integration adapter."""
from __future__ import annotations

import json
import os
import shutil
from pathlib import Path

from devcouncil.executors.agent_registry import resolve_cursor_agent_executable
from devcouncil.integrations.clients import common as _common

_project_root = _common._project_root
_warn_if_verify_only = _common._warn_if_verify_only
_server_args = _common._server_args
_format_command = _common._format_command
_quote_powershell_arg = _common._quote_powershell_arg
_run = _common._run
_run_capture = _common._run_capture
_config_path = _common._config_path
_load_raw_config = _common._load_raw_config
_save_raw_config = _common._save_raw_config
_load_json = _common._load_json
_save_json = _common._save_json
_load_json_strict = _common._load_json_strict
_mutate_raw_config = _common._mutate_raw_config
_batched_raw_config = _common._batched_raw_config
_probe_mcp_tools = _common._probe_mcp_tools
_print_command = _common._print_command
_configure = _common._configure

console = _common.console


def _cursor_executable() -> str | None:
    return resolve_cursor_agent_executable()


def _cursor_config_path(project_root: Path) -> Path:
    return project_root / ".cursor" / "mcp.json"


# Cursor MCP stdio resolution lives in ``common`` so every generator that emits an MCP
# ``command`` (Cursor's mcp.json, the Claude plugin's .mcp.json) shares one rule.
_cursor_mcp_command = _common.resolve_devcouncil_executable


def _cursor_mcp_config(project_root: Path) -> dict:
    root = project_root.expanduser().resolve()
    env_path = _common.venv_augmented_path(root)
    return {
        "mcpServers": {
            "devcouncil": {
                "type": "stdio",
                "command": _cursor_mcp_command(root),
                "args": ["mcp-server"],
                "env": {
                    "DEVCOUNCIL_PROJECT_ROOT": str(root),
                    "PATH": env_path,
                },
            }
        }
    }


def _write_cursor_config(project_root: Path) -> Path:
    path = _cursor_config_path(project_root)
    data = _load_json_strict(path, "Cursor")
    mcp_servers = data.setdefault("mcpServers", {})
    mcp_servers["devcouncil"] = _cursor_mcp_config(project_root)["mcpServers"]["devcouncil"]
    _save_json(path, data)
    return path


_CURSOR_RULE_CONTENT = """\
---
description: DevCouncil task loop and navigation
alwaysApply: true
---

# DevCouncil

Use DevCouncil MCP tools for status, checkout, scope, and verify — do not guess task state.

Navigate via `.devcouncil/repo_map.json` (subsystems, entry_points, critical_files). Prefer `dev map query|trace|dead` for symbol callers.

Interactive Cursor Shell/Write do **not** require a task lease under assist defaults
(`integrations.cursor.write_gate: false`, `execution.hook_gate.mode: off`). Do not claim
"Shell is gated" or checkout just to run shell. Only checkout when write-gates / contain
mode are active (`dev integrate … --write-gate` or `execution.hook_gate.mode: contain`).

Follow engineering skills under `.cursor/skills/` and `.claude/skills/` (`dev skills scaffold` / `dev integrate cursor --apply`).
"""


def _cursor_rules_path(project_root: Path) -> Path:
    return project_root / ".cursor" / "rules" / "devcouncil.mdc"


def _install_cursor_assets(project_root: Path) -> list[Path]:
    """Scaffold applicable skills and write the always-on DevCouncil Cursor rule.

    Idempotent: skills land under both ``.claude/skills/`` and ``.cursor/skills/``;
    the rule is ``.cursor/rules/devcouncil.mdc`` with ``alwaysApply: true``.
    """
    from devcouncil.skills.registry import scaffold_skills, skills_for_scaffold

    written: list[Path] = []
    skills = skills_for_scaffold("", project_root)
    written.extend(scaffold_skills(project_root, skills))

    rules_path = _cursor_rules_path(project_root)
    if not (rules_path.exists() and rules_path.read_text(encoding="utf-8") == _CURSOR_RULE_CONTENT):
        rules_path.parent.mkdir(parents=True, exist_ok=True)
        rules_path.write_text(_CURSOR_RULE_CONTENT, encoding="utf-8")
        written.append(rules_path)
    return written


def _record_cursor_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        cursor = config.setdefault("integrations", {}).setdefault("cursor", {})
        cursor.update({
            "enabled": True,
            "config_path": str(_cursor_config_path(project_root).relative_to(project_root)),
            "skills_path": ".cursor/skills",
            "rules_path": ".cursor/rules/devcouncil.mdc",
        })

    _mutate_raw_config(project_root, mutate)


def probe_cursor_auth() -> tuple[bool, str]:
    """Probe Cursor CLI auth via agent status/about."""
    executable = _cursor_executable()
    if not executable:
        return False, "agent/cursor-agent not found on PATH"
    if os.environ.get("CURSOR_API_KEY"):
        return True, "CURSOR_API_KEY set (CI headless auth)"
    for subcommand in ("status", "about"):
        code, output = _run_capture([executable, subcommand])
        if code == 0 and output.strip():
            first = output.strip().splitlines()[0]
            return True, first
    return False, "Run agent login or set CURSOR_API_KEY for CI headless auth"


def probe_cursor_mcp_list() -> tuple[bool, str]:
    executable = _cursor_executable()
    if not executable:
        return False, "agent/cursor-agent not found on PATH"
    code, output = _run_capture([executable, "mcp", "list"])
    if code != 0:
        return False, output.strip() or "agent mcp list failed"
    return True, output.strip().splitlines()[0] if output.strip() else "MCP servers listed"


def _configure_cursor(project_root: Path, apply: bool) -> bool:
    path = _cursor_config_path(project_root)
    config = _cursor_mcp_config(project_root)
    executable = _cursor_executable() or "agent"
    if not apply:
        console.print("[bold]Cursor[/bold]")
        console.print(f"Project MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(f"Verify in Cursor CLI with: [dim]{executable} mcp list[/dim]")
        auth_ok, auth_details = probe_cursor_auth()
        if auth_ok:
            console.print(f"Auth: [green]{auth_details}[/green]")
        else:
            console.print(f"Auth: [yellow]{auth_details}[/yellow]")
        if not os.environ.get("CURSOR_API_KEY"):
            console.print("CI headless: export [dim]CURSOR_API_KEY[/dim] (see Cursor headless docs)")
        return True

    if not shutil.which("cursor") and not _cursor_executable():
        console.print("[yellow]Cursor CLI not found on PATH. Project MCP config will still be available to Cursor.[/yellow]")
    try:
        written = _write_cursor_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_cursor_config(project_root)
    console.print(f"[green]Cursor MCP config written:[/green] {written}")
    mcp_ok, mcp_details = probe_cursor_mcp_list()
    if mcp_ok:
        console.print(f"[dim]MCP probe: {mcp_details}[/dim]")
    auth_ok, auth_details = probe_cursor_auth()
    if not auth_ok:
        console.print(f"[yellow]Auth check: {auth_details}[/yellow]")
    return True


def _uninstall_cursor(project_root: Path) -> list[str]:
    """Surgically remove DevCouncil Cursor MCP, hooks, rule, and unmodified skills."""
    from devcouncil.integrations.clients import hooks as _hooks

    removed: list[str] = []
    root = project_root.expanduser().resolve()

    mcp_path = _cursor_config_path(root)
    if mcp_path.exists():
        data = _load_json(mcp_path)
        servers = data.get("mcpServers")
        if isinstance(servers, dict) and "devcouncil" in servers:
            servers.pop("devcouncil")
            if not servers:
                data.pop("mcpServers", None)
            if data:
                _save_json(mcp_path, data)
            else:
                mcp_path.unlink()
            removed.append(f"mcpServers.devcouncil in {mcp_path.relative_to(root)}")

    removed.extend(_hooks._uninstall_cursor_hooks(root))

    rules_path = _cursor_rules_path(root)
    if rules_path.exists():
        try:
            on_disk = rules_path.read_text(encoding="utf-8")
        except OSError:
            on_disk = ""
        if on_disk == _CURSOR_RULE_CONTENT:
            rules_path.unlink()
            removed.append(str(rules_path.relative_to(root)))

    removed.extend(_common._remove_unmodified_library_skills(root, destinations=(".cursor/skills",)))
    removed.extend(_common._clear_client_integration_config(root, "cursor"))
    return removed
