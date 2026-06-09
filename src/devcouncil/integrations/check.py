"""Shared integration readiness checks for `dev integrate check`."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from devcouncil.executors.agent_registry import (
    CODING_CLI_INTEGRATION_INFO,
    CODING_CLI_VERSION_COMMANDS,
    detect_available_coding_cli,
    resolve_automated_executor,
    resolve_coding_cli_executable,
    resolve_coding_cli_probe_order,
)


@dataclass(frozen=True)
class IntegrationCheckRow:
    name: str
    status: str
    details: str

    def as_dict(self) -> dict[str, str]:
        return {"name": self.name, "status": self.status, "details": self.details}


@dataclass(frozen=True)
class IntegrationCheckReport:
    checks: tuple[IntegrationCheckRow, ...]
    recommended_executor: str | None
    failures: int

    @property
    def ok(self) -> bool:
        return self.failures == 0

    def as_dict(self) -> dict[str, Any]:
        return {
            "ok": self.ok,
            "failures": self.failures,
            "recommended_executor": self.recommended_executor,
            "checks": [row.as_dict() for row in self.checks],
        }

    def to_json(self, *, indent: int | None = 2) -> str:
        return json.dumps(self.as_dict(), indent=indent)


CODING_CLI_CHECK_LABELS: dict[str, str] = {
    "codex": "Codex CLI",
    "gemini": "Gemini CLI",
    "claude": "Claude Code",
    "cursor": "Cursor",
    "opencode": "OpenCode",
    "antigravity": "Google Antigravity CLI",
    "warp": "Warp / Oz",
    "aider": "Aider",
    "copilot": "GitHub Copilot CLI",
    "goose": "Goose",
    "amp": "Amp (Sourcegraph)",
    "qwen": "Qwen Code",
    "crush": "Crush (Charm)",
}


def probe_cli_version(command: list[str], *, timeout: int = 10) -> tuple[int, str]:
    executable = shutil.which(command[0])
    if not executable:
        return 127, f"{command[0]} not found on PATH"

    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    invocation = subprocess.list2cmdline(resolved) if use_shell else resolved
    try:
        result = subprocess.run(
            invocation,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            shell=use_shell,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return 124, "timed out"
    except (FileNotFoundError, OSError) as exc:
        return 127, f"{command[0]} could not be executed: {exc}"
    return result.returncode, (result.stdout + result.stderr).strip()


def probe_coding_cli_version(client: str) -> tuple[bool, str]:
    label = CODING_CLI_CHECK_LABELS.get(client, client)
    commands = CODING_CLI_VERSION_COMMANDS.get(client, ())
    for command in commands:
        code, output = probe_cli_version(list(command))
        if code == 0:
            first_line = output.splitlines()[0] if output else "installed"
            return True, first_line
    return False, f"Optional; install {label} to use this integration."


def recommended_executor_status(project_root: Path) -> tuple[bool, str]:
    detected = detect_available_coding_cli(project_root)
    if not detected:
        return False, "No built-in coding CLI on PATH. Run dev integrate recommend after installing one."
    resolved = resolve_automated_executor(project_root, None)
    return True, f"Use --executor {resolved} for dev go / dev run (detected: {detected})."


def coding_clis_on_path(project_root: Path) -> list[str]:
    order = resolve_coding_cli_probe_order(project_root)
    return [client for client in order if resolve_coding_cli_executable(project_root, client)]


def _load_json_file(path: Path) -> dict[str, Any]:
    if not path.exists():
        return {}
    try:
        loaded = json.loads(path.read_text(encoding="utf-8")) or {}
    except json.JSONDecodeError:
        return {}
    return loaded if isinstance(loaded, dict) else {}


def _cursor_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    path = project_root / ".cursor" / "mcp.json"
    data = _load_json_file(path)
    server = ((data.get("mcpServers") or {}).get("devcouncil") or {}) if data else {}
    ok = (
        server.get("type") == "stdio"
        and server.get("command") == "devcouncil"
        and server.get("args") == ["mcp-server"]
        and (server.get("env") or {}).get("DEVCOUNCIL_PROJECT_ROOT") == str(project_root)
    )
    if ok:
        return "ok", False, [str(path)]
    return ("missing" if not path.exists() else "drifted"), True, [str(path)]


def _opencode_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    path = project_root / "opencode.json"
    data = _load_json_file(path)
    server = ((data.get("mcp") or {}).get("devcouncil") or {}) if data else {}
    plugin = data.get("plugin") or []
    plugin_ok = "./.devcouncil/integrations/opencode_devcouncil_plugin.mjs" in plugin
    ok = (
        server.get("type") == "local"
        and server.get("command") == ["devcouncil", "mcp-server"]
        and (server.get("environment") or {}).get("DEVCOUNCIL_PROJECT_ROOT") == str(project_root)
    )
    status = "ok" if ok else ("missing" if not path.exists() else "drifted")
    return status, not ok or not plugin_ok, [str(path), str(project_root / ".devcouncil" / "integrations" / "opencode_devcouncil_plugin.mjs")]


def _antigravity_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    path = project_root / ".agents" / "mcp_config.json"
    data = _load_json_file(path)
    server = ((data.get("mcpServers") or {}).get("devcouncil") or {}) if data else {}
    ok = (
        server.get("command") == "devcouncil"
        and server.get("args") == ["mcp-server"]
        and server.get("cwd") == str(project_root)
        and (server.get("env") or {}).get("DEVCOUNCIL_PROJECT_ROOT") == str(project_root)
    )
    if ok:
        return "ok", False, [str(path)]
    return ("missing" if not path.exists() else "drifted"), True, [str(path)]


def _warp_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    path = project_root / ".devcouncil" / "integrations" / "warp-mcp.json"
    data = _load_json_file(path)
    server = data.get("devcouncil") or {}
    ok = (
        "mcpServers" not in data
        and server.get("command") == "devcouncil"
        and server.get("args") == ["mcp-server"]
        and (server.get("env") or {}).get("DEVCOUNCIL_PROJECT_ROOT") == str(project_root)
    )
    if ok:
        return "ok", False, [str(path)]
    return ("missing" if not path.exists() else "drifted"), True, [str(path)]


def integration_capability_rows(project_root: Path) -> list[dict[str, object]]:
    order = resolve_coding_cli_probe_order(project_root)
    rows: list[dict[str, object]] = []
    for client in order:
        info = CODING_CLI_INTEGRATION_INFO.get(client)
        if info is None:
            continue
        config_status = "not_applicable"
        fixable = bool(info.mcp or info.hooks or info.launcher_shim)
        paths: list[str] = []
        if client == "cursor":
            config_status, fixable, paths = _cursor_config_status(project_root)
        elif client == "opencode":
            config_status, fixable, paths = _opencode_config_status(project_root)
        elif client == "antigravity":
            config_status, fixable, paths = _antigravity_config_status(project_root)
        elif client == "warp":
            config_status, fixable, paths = _warp_config_status(project_root)

        rows.append({
            "name": info.name,
            "label": info.label,
            "on_path": resolve_coding_cli_executable(project_root, client) is not None,
            "tier": info.tier,
            "headless": info.headless,
            "mcp": info.mcp,
            "hooks": info.hooks,
            "launcher_shim": info.launcher_shim,
            "notes": info.notes,
            "configured": config_status == "ok",
            "config_status": config_status,
            "fixable": fixable,
            "paths": paths,
            "apply_target": client,
        })
    return rows


def build_integration_check_report(project_root: Path, *, strict: bool = False) -> IntegrationCheckReport:
    from devcouncil.cli.commands import integrate

    rows: list[IntegrationCheckRow] = []
    failures = 0

    def add(ok: bool, name: str, details: str) -> None:
        nonlocal failures
        rows.append(IntegrationCheckRow(name=name, status="ok" if ok else "fail", details=details))
        if not ok:
            failures += 1

    def add_optional(ok: bool, name: str, details: str) -> None:
        if strict and not ok:
            add(False, name, details)
            return
        rows.append(IntegrationCheckRow(name=name, status="ok" if ok else "missing", details=details))

    def add_skip(name: str, details: str) -> None:
        rows.append(IntegrationCheckRow(name=name, status="skip", details=details))

    root = project_root.expanduser().resolve()
    add((root / ".devcouncil").exists(), "Project state", str(root / ".devcouncil"))
    devcouncil_path = shutil.which("devcouncil")
    add(
        devcouncil_path is not None or Path(sys.executable).exists(),
        "devcouncil CLI",
        devcouncil_path or f"{sys.executable} -m devcouncil",
    )

    devcouncil_launch = [devcouncil_path] if devcouncil_path else [sys.executable, "-m", "devcouncil"]
    code, output = integrate._run_capture([*devcouncil_launch, "--help"])
    add(code == 0, "devcouncil command", output.splitlines()[0] if output else "No output")

    for client in resolve_coding_cli_probe_order(root):
        cli_ok, cli_details = probe_coding_cli_version(client)
        add_optional(cli_ok, CODING_CLI_CHECK_LABELS.get(client, client), cli_details)

    rec_ok, rec_details = recommended_executor_status(root)
    add_optional(rec_ok, "Recommended coding CLI", rec_details)

    for row in integration_capability_rows(root):
        name = str(row["name"])
        status = str(row["config_status"])
        if status == "ok":
            paths = row.get("paths")
            path_list = paths if isinstance(paths, list) else []
            add(True, f"{row['label']} config", ", ".join(str(path) for path in path_list))
        elif status in {"missing", "drifted"}:
            add_skip(f"{row['label']} config", f"Run dev integrate {name} --apply to repair.")

    raw_config = integrate._load_raw_config(root) if (root / ".devcouncil").exists() else {}

    cursor_hooks = root / ".cursor" / "hooks.json"
    cursor_enabled = bool(raw_config.get("integrations", {}).get("cursor", {}).get("enabled"))
    if cursor_enabled or cursor_hooks.exists():
        hooks_ok = False
        if cursor_hooks.exists():
            try:
                hooks_data = json.loads(cursor_hooks.read_text(encoding="utf-8")) or {}
                hooks_ok = "preToolUse" in hooks_data.get("hooks", {})
            except json.JSONDecodeError:
                hooks_ok = False
        add(
            hooks_ok,
            "Cursor hooks",
            str(cursor_hooks) if hooks_ok else "Run dev integrate hooks --apply --tool cursor.",
        )

    opencode_config = integrate._opencode_config_path(root)
    opencode_enabled = bool(raw_config.get("integrations", {}).get("opencode", {}).get("enabled"))
    opencode_plugin = integrate._opencode_plugin_path(root)
    if opencode_enabled or opencode_plugin.exists():
        plugin_ok = opencode_plugin.exists()
        plugin_registered = False
        if plugin_ok and opencode_config.exists():
            try:
                opencode_data = json.loads(opencode_config.read_text(encoding="utf-8")) or {}
                plugin_registered = (
                    f"./.devcouncil/integrations/{integrate.OPENCODE_HOOK_PLUGIN_NAME}"
                    in (opencode_data.get("plugin") or [])
                )
            except json.JSONDecodeError:
                plugin_registered = False
        add(
            plugin_ok and plugin_registered,
            "OpenCode hook plugin",
            str(opencode_plugin) if plugin_ok and plugin_registered else "Run dev integrate hooks --apply --tool opencode.",
        )

    bundled_plugin = integrate._opencode_plugin_source()
    add(
        bundled_plugin.exists(),
        "Bundled OpenCode hook plugin",
        str(bundled_plugin) if bundled_plugin.exists() else "Reinstall DevCouncil; package asset is missing.",
    )

    custom_agents = raw_config.get("integrations", {}).get("cli_agents", {}).get("agents", {})
    if custom_agents:
        for name, agent in sorted(custom_agents.items()):
            command = str(agent.get("command", "")).strip()
            found = shutil.which(command) if command else None
            add(found is not None, f"CLI agent: {name}", found or f"{command or 'command'} not found on PATH")
    else:
        add_skip("Custom CLI agents", "No agents registered.")

    try:
        tools = integrate._probe_mcp_tools(root)
        expected = {"devcouncil_status", "devcouncil_report", "devcouncil_get_task"}
        add(expected.issubset(set(tools)), "MCP server", ", ".join(tools))
    except Exception as exc:
        add(False, "MCP server", str(exc))

    detected = detect_available_coding_cli(root)
    recommended = resolve_automated_executor(root, None) if detected else None
    return IntegrationCheckReport(tuple(rows), recommended, failures)


def integration_status_summary(project_root: Path) -> dict[str, Any]:
    from devcouncil.app.config import load_config

    probe_order = resolve_coding_cli_probe_order(project_root)
    on_path = coding_clis_on_path(project_root)
    detected = on_path[0] if on_path else None
    try:
        execution = load_config(project_root).execution
        default_executor = execution.default_executor
        stream_cli_output = execution.stream_cli_output
        cursor_resume_mode = execution.cursor_resume_mode
        custom_probe_order = list(execution.coding_cli_probe_order)
    except Exception:
        default_executor = "manual"
        stream_cli_output = False
        cursor_resume_mode = "off"
        custom_probe_order = []

    resolved = resolve_automated_executor(project_root, None) if detected or default_executor != "manual" else "manual"
    config_path = project_root / ".devcouncil" / "config.yaml"
    return {
        "project_initialized": (project_root / ".devcouncil").is_dir(),
        "config_path": str(config_path) if config_path.exists() else None,
        "default_executor": default_executor,
        "resolved_executor": resolved,
        "detected_executor": detected,
        "coding_clis_on_path": on_path,
        "probe_order": list(probe_order),
        "custom_probe_order": custom_probe_order,
        "stream_cli_output": stream_cli_output,
        "cursor_resume_mode": cursor_resume_mode,
        "capabilities": integration_capability_rows(project_root),
    }
