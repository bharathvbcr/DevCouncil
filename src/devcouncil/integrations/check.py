"""Shared integration readiness checks for `dev integrate check`."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from devcouncil.executors.agent_registry import (
    CODING_CLI_INTEGRATION_INFO,
    CODING_CLI_VERSION_COMMANDS,
    DEPRECATED_CODING_CLIS,
    GEMINI_DEPRECATION_MESSAGE,
    detect_available_coding_cli,
    resolve_automated_executor,
    resolve_coding_cli_executable,
    resolve_coding_cli_probe_order,
)
from devcouncil.utils.subprocess_env import clean_subprocess_env
from devcouncil.utils.json_persist import read_json


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
    "grok": "Grok Build",
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
            env=clean_subprocess_env(),
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


def recommended_executor_status(
    project_root: Path, detected: str | None = None
) -> tuple[bool, str]:
    if detected is None:
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
        loaded = read_json(path) or {}
    except json.JSONDecodeError:
        return {}
    return loaded if isinstance(loaded, dict) else {}


def _claude_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    settings_path = project_root / ".claude" / "settings.local.json"
    assets_dir = project_root / ".claude" / "commands" / "devcouncil"
    data = _load_json_file(settings_path)
    status_line = data.get("statusLine") if data else {}
    status_line_ok = (
        isinstance(status_line, dict)
        and "hook claude-statusline" in str(status_line.get("command", ""))
    )
    mcp_ok = "devcouncil" in (data.get("enabledMcpjsonServers") or [])
    assets_ok = assets_dir.is_dir() and (assets_dir / "status.md").exists()
    ok = (status_line_ok or mcp_ok) and assets_ok
    paths = [str(settings_path)]
    if assets_ok:
        paths.append(str(assets_dir))
    if ok:
        return "ok", False, paths
    if not settings_path.exists() and not assets_ok:
        return "missing", True, paths
    return "drifted", True, paths


def _codex_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    """Check the client-persisted MCP registration, not just repo hook files."""
    executable = resolve_coding_cli_executable(project_root, "codex")
    logical_path = "codex mcp:devcouncil"
    if not executable:
        return "missing", True, [logical_path]
    try:
        result = subprocess.run(
            [executable, "mcp", "get", "devcouncil", "--json"],
            cwd=project_root,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=10,
            env=clean_subprocess_env(),
        )
        data = json.loads(result.stdout) if result.returncode == 0 else {}
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
        data = {}

    from devcouncil.integrations.clients.common import _server_args

    expected = _server_args(project_root)
    transport = data.get("transport") if isinstance(data, dict) else {}
    transport = transport if isinstance(transport, dict) else {}
    env = transport.get("env") or {}
    ok = (
        data.get("enabled") is True
        and transport.get("type") == "stdio"
        and transport.get("command") == expected[0]
        and transport.get("args") == expected[1:]
        and isinstance(env, dict)
        and env.get("DEVCOUNCIL_PROJECT_ROOT") == str(project_root)
    )
    return ("ok" if ok else "drifted"), not ok, [logical_path, str(project_root / ".codex" / "hooks.json")]


def _codex_hook_schema_status(project_root: Path, *, write_gate: bool = False) -> tuple[bool, str]:
    """Validate the repository Codex hook schema and canonical feature flag.

    Assist (``write_gate=False``): PostToolUse + lifecycle, no PreToolUse.
    Contain (``write_gate=True``): PreToolUse required (exit-2 deny is intentional).
    """
    hooks_path = project_root / ".codex" / "hooks.json"
    config_path = project_root / ".codex" / "config.toml"
    try:
        data = read_json(hooks_path)
        config = tomllib.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError, tomllib.TOMLDecodeError) as exc:
        return False, f"Invalid Codex hook configuration: {exc}"
    hooks = data.get("hooks") if isinstance(data, dict) else None
    required = {"PostToolUse", "SessionStart", "Stop", "SubagentStop"}
    if write_gate:
        required.add("PreToolUse")
    if not isinstance(hooks, dict) or not required.issubset(hooks):
        missing = sorted(required - set(hooks or {}))
        return False, f"Missing Codex hook events: {', '.join(missing)}"
    has_pre = "PreToolUse" in hooks
    if write_gate and not has_pre:
        return False, "Contain mode expected PreToolUse. Run dev integrate hooks --apply --tool codex --write-gate."
    if not write_gate and has_pre:
        return False, (
            "Assist mode expected (no PreToolUse). "
            "Re-run dev integrate hooks --apply --tool codex (or pass --write-gate)."
        )
    for event, groups in hooks.items():
        if not isinstance(groups, list):
            return False, f"Codex hook event {event} must contain a list"
        for group in groups:
            for hook in group.get("hooks", []) if isinstance(group, dict) else []:
                if not isinstance(hook, dict) or not str(hook.get("name", "")).startswith("devcouncil-"):
                    continue
                timeout = hook.get("timeout")
                if not isinstance(timeout, int) or not 1 <= timeout <= 600:
                    return False, f"Codex hook {hook.get('name')} timeout must be 1..600 seconds"
                if "--client codex" not in str(hook.get("command", "")):
                    return False, f"Codex hook {hook.get('name')} does not select the Codex schema"
    features = config.get("features") if isinstance(config, dict) else {}
    if not isinstance(features, dict) or features.get("hooks") is not True:
        return False, "[features].hooks must be true"
    if "codex_hooks" in features:
        return False, "Deprecated [features].codex_hooks is still present"
    posture = "contain" if write_gate else "assist"
    return True, f"{hooks_path} ({posture}); hook trust must be reviewed in Codex with /hooks"


def _cursor_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    path = project_root / ".cursor" / "mcp.json"
    data = _load_json_file(path)
    server = ((data.get("mcpServers") or {}).get("devcouncil") or {}) if data else {}
    command = server.get("command")
    # Accept bare ``devcouncil`` or an absolute/relative path whose basename is
    # ``devcouncil`` (project venv). Both are valid; absolute paths are preferred
    # when Cursor's shell PATH does not include the project venv.
    from pathlib import Path as _Path

    name_ok = False
    if isinstance(command, str) and command.strip():
        name = _Path(command.replace("\\", "/")).name.lower()
        name_ok = name in {"devcouncil", "devcouncil.exe"}
    ok = (
        server.get("type") == "stdio"
        and name_ok
        and server.get("args") == ["mcp-server"]
        and (server.get("env") or {}).get("DEVCOUNCIL_PROJECT_ROOT") == str(project_root)
    )
    if ok:
        return "ok", False, [str(path)]
    return ("missing" if not path.exists() else "drifted"), True, [str(path)]


def _grok_config_status(project_root: Path) -> tuple[str, bool, list[str]]:
    path = project_root / ".grok" / "config.toml"
    if not path.exists():
        return "missing", True, [str(path)]
    text = path.read_text(encoding="utf-8")
    ok = (
        "[mcp_servers.devcouncil]" in text
        and "devcouncil" in text
        and "mcp-server" in text
        and str(project_root) in text
    )
    if ok:
        return "ok", False, [str(path)]
    return "drifted", True, [str(path)]


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
    # Clients with opt-in PreToolUse: report installed posture, not capability ceiling.
    write_gate_clients = {"claude", "cursor", "grok", "opencode", "codex", "gemini"}
    raw_integrations: dict = {}
    try:
        from devcouncil.integrations.clients import common as _common

        raw = _common._load_raw_config(project_root)
        integrations = raw.get("integrations")
        if isinstance(integrations, dict):
            raw_integrations = integrations
    except Exception:
        raw_integrations = {}

    for client in order:
        info = CODING_CLI_INTEGRATION_INFO.get(client)
        if info is None:
            continue
        config_status = "not_applicable"
        fixable = bool(info.mcp or info.hooks or info.launcher_shim)
        paths: list[str] = []
        if client == "codex":
            config_status, fixable, paths = _codex_config_status(project_root)
        elif client == "cursor":
            config_status, fixable, paths = _cursor_config_status(project_root)
        elif client == "grok":
            config_status, fixable, paths = _grok_config_status(project_root)
        elif client == "claude":
            config_status, fixable, paths = _claude_config_status(project_root)
        elif client == "opencode":
            config_status, fixable, paths = _opencode_config_status(project_root)
        elif client == "antigravity":
            config_status, fixable, paths = _antigravity_config_status(project_root)
        elif client == "warp":
            config_status, fixable, paths = _warp_config_status(project_root)

        enforcement = info.enforcement
        if client in write_gate_clients and info.hooks:
            cfg = raw_integrations.get(client)
            write_gate = bool(cfg.get("write_gate", False)) if isinstance(cfg, dict) else False
            # Assist (default) is advisory+verify; pre-action only when write_gate installed.
            enforcement = "pre-action" if write_gate else "advisory+verify"

        rows.append({
            "name": info.name,
            "label": info.label,
            "on_path": resolve_coding_cli_executable(project_root, client) is not None,
            "tier": info.tier,
            "headless": info.headless,
            "mcp": info.mcp,
            "hooks": info.hooks,
            "enforcement": enforcement,
            "launcher_shim": info.launcher_shim,
            "notes": info.notes,
            "configured": config_status == "ok",
            "config_status": config_status,
            "fixable": fixable,
            "paths": paths,
            "apply_target": client,
        })
    return rows


# Gate / tool-hook event names DevCouncil installs across clients.
_DEVCOUNCIL_GATE_EVENTS = frozenset({
    "PreToolUse",
    "PostToolUse",
    "BeforeTool",
    "preToolUse",
    "postToolUse",
})

# Integrity label → integrations.<client> config key (for enabled:true checks).
_HOOK_INTEGRITY_CLIENT_KEYS: dict[str, str] = {
    "Claude": "claude",
    "Codex": "codex",
    "Gemini": "gemini",
    "Cursor": "cursor",
    "Grok": "grok",
}


def _text_references_devcouncil_hooks(text: str) -> bool:
    """True when raw config text still wires DevCouncil hook commands."""
    lower = text.lower()
    if "devcouncil" in lower:
        return True
    # Cursor flat hooks: ``…/bin/dev hook pre-tool-use --client cursor …``
    return "hook pre-tool-use" in lower or "hook post-tool-use" in lower


def _hook_config_references_devcouncil(path: Path) -> bool | None:
    """Return whether a client hook config still wires DevCouncil's gate.

    ``True``  -> the file exists and still invokes the DevCouncil pre/post-tool gate.
    ``False`` -> the file exists but no longer references DevCouncil (may be clean
    uninstall or a disarmed shell — callers must distinguish via
    :func:`_hook_config_integrity_status`).
    ``None``  -> the file does not exist (client was never integrated here).

    Reads the raw text rather than parsing each client's bespoke schema so it works
    uniformly across JSON hook files and is resilient to format drift; the goal is a
    tamper tripwire, not full schema validation.

    Accepts either an explicit ``devcouncil`` marker (named hooks / ``devcouncil`` CLI)
    or the project-venv ``dev hook …`` command Cursor installs (no named hook id).
    """
    if not path.exists():
        return None
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return False
    return _text_references_devcouncil_hooks(text)


def _hook_entry_command(entry: dict[str, Any]) -> str:
    return str(entry.get("command") or "").strip()


def _hook_entry_is_devcouncil(entry: dict[str, Any]) -> bool:
    name = str(entry.get("name") or "")
    if name.startswith("devcouncil-"):
        return True
    return _text_references_devcouncil_hooks(_hook_entry_command(entry))


def _gate_event_is_disarmed(groups: Any) -> bool:
    """True when a gate event remains as an emptied / disarmed shell.

    Examples: ``PreToolUse: []``, matcher groups with ``hooks: []``, named
    ``devcouncil-*`` handlers with blank commands, or Cursor flat entries with
    an empty ``command``.
    """
    if not isinstance(groups, list):
        return False
    if not groups:
        return True
    saw_live = False
    saw_disarmed = False
    for group in groups:
        if not isinstance(group, dict):
            continue
        # Cursor flat: command lives on the event entry itself.
        if "command" in group and "hooks" not in group:
            cmd = _hook_entry_command(group)
            if not cmd:
                saw_disarmed = True
            elif _hook_entry_is_devcouncil(group) or _text_references_devcouncil_hooks(cmd):
                saw_live = True
            continue
        handlers = group.get("hooks")
        if not isinstance(handlers, list):
            continue
        if not handlers:
            saw_disarmed = True
            continue
        for hook in handlers:
            if not isinstance(hook, dict):
                continue
            if not _hook_entry_is_devcouncil(hook):
                continue
            if _hook_entry_command(hook):
                saw_live = True
            else:
                saw_disarmed = True
    return saw_disarmed and not saw_live


def _hook_config_has_disarmed_gate_events(path: Path) -> bool:
    """True when DevCouncil gate event keys remain but were emptied/disarmed."""
    data = _load_json_file(path)
    hooks = data.get("hooks") if isinstance(data, dict) else None
    if not isinstance(hooks, dict):
        return False
    for event, groups in hooks.items():
        if event not in _DEVCOUNCIL_GATE_EVENTS:
            continue
        if _gate_event_is_disarmed(groups):
            return True
    return False


def _hook_config_integrity_status(path: Path, *, client_enabled: bool) -> str | None:
    """Classify hook-file integrity for ``dev integrate check``.

    Returns:
      ``"ok"`` — file still references live DevCouncil hooks.
      ``"tampered"`` — gate events emptied/disarmed, or ``enabled: true`` with
      hooks stripped while the settings file remains.
      ``None`` — skip (missing file, or clean uninstall that preserved user
      settings / an empty orphaned hooks file).
    """
    references = _hook_config_references_devcouncil(path)
    if references is None:
        return None
    if references:
        # Named hooks with blank commands still contain "devcouncil" in text;
        # treat emptied DevCouncil handlers as tamper even when the marker remains.
        if _hook_config_has_disarmed_gate_events(path):
            return "tampered"
        return "ok"
    if _hook_config_has_disarmed_gate_events(path):
        return "tampered"
    if client_enabled:
        return "tampered"
    return None


def _hook_config_tamper_targets(project_root: Path) -> list[tuple[str, Path]]:
    return [
        ("Claude", project_root / ".claude" / "settings.local.json"),
        ("Codex", project_root / ".codex" / "hooks.json"),
        ("Gemini", project_root / ".gemini" / "settings.json"),
        ("Cursor", project_root / ".cursor" / "hooks.json"),
        ("Grok", project_root / ".grok" / "hooks" / "devcouncil.json"),
    ]


def _client_integration_enabled(integrations_cfg: dict[str, Any], client: str) -> bool:
    cfg = integrations_cfg.get(client)
    return bool(cfg.get("enabled")) if isinstance(cfg, dict) else False


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
        # Coding CLIs (and similar optional probes) stay informational even under
        # --strict. Absence is never a failure; --strict still elevates real
        # project/integration defects via add() / add_soft().
        rows.append(IntegrationCheckRow(name=name, status="ok" if ok else "missing", details=details))

    def add_skip(name: str, details: str) -> None:
        rows.append(IntegrationCheckRow(name=name, status="skip", details=details))

    root = project_root.expanduser().resolve()
    detected = detect_available_coding_cli(root)
    add((root / ".devcouncil").exists(), "Project state", str(root / ".devcouncil"))
    from devcouncil.integrations.clients.common import resolve_dev_executable

    devcouncil_path = resolve_dev_executable(root)
    resolved_dev = Path(devcouncil_path).exists() or shutil.which(devcouncil_path) is not None
    devcouncil_launch = [devcouncil_path] if resolved_dev else [sys.executable, "-m", "devcouncil"]
    add(
        resolved_dev or Path(sys.executable).exists(),
        "devcouncil CLI",
        " ".join(devcouncil_launch),
    )

    code, output = integrate._run_capture([*devcouncil_launch, "--help"])
    add(code == 0, "devcouncil command", output.splitlines()[0] if output else "No output")

    probe_order = resolve_coding_cli_probe_order(root)
    for client in probe_order:
        cli_ok, cli_details = probe_coding_cli_version(client)
        add_optional(cli_ok, CODING_CLI_CHECK_LABELS.get(client, client), cli_details)
    for client in sorted(DEPRECATED_CODING_CLIS):
        if client in probe_order:
            continue
        label = CODING_CLI_CHECK_LABELS.get(client, client)
        add_skip(label, GEMINI_DEPRECATION_MESSAGE)

    rec_ok, rec_details = recommended_executor_status(root, detected)
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
    integrations_cfg = raw_config.get("integrations", {}) if isinstance(raw_config.get("integrations"), dict) else {}

    claude_hooks_path = root / ".claude" / "settings.local.json"
    claude_cfg = integrations_cfg.get("claude", {}) if isinstance(integrations_cfg.get("claude"), dict) else {}
    claude_enabled = bool(claude_cfg.get("enabled"))
    claude_write_gate = bool(claude_cfg.get("write_gate", False))
    # Skip when disabled and no DevCouncil hooks remain (clean uninstall that
    # preserved user settings). Event-key presence alone is not enough — foreign
    # PreToolUse/PostToolUse must not force a re-apply failure.
    claude_has_dc_hooks = _hook_config_references_devcouncil(claude_hooks_path) is True
    if claude_enabled or claude_has_dc_hooks:
        hooks_ok = False
        details = "Run dev integrate claude --apply."
        if claude_hooks_path.exists():
            try:
                hooks_data = read_json(claude_hooks_path) or {}
                hook_events = hooks_data.get("hooks", {}) if isinstance(hooks_data, dict) else {}
                has_post = "PostToolUse" in hook_events
                has_pre = "PreToolUse" in hook_events
                if claude_write_gate:
                    hooks_ok = has_pre and has_post
                    details = str(claude_hooks_path) if hooks_ok else (
                        "Run dev integrate claude --apply --write-gate."
                    )
                else:
                    hooks_ok = has_post and not has_pre
                    if has_post and has_pre:
                        details = (
                            "Assist mode expected (no PreToolUse). "
                            "Re-run dev integrate claude --apply (or pass --write-gate)."
                        )
                    elif hooks_ok:
                        details = f"{claude_hooks_path} (assist)"
                    else:
                        details = "Run dev integrate claude --apply."
            except json.JSONDecodeError:
                hooks_ok = False
        add(hooks_ok, "Claude hooks", details)

    cursor_hooks = root / ".cursor" / "hooks.json"
    cursor_cfg = integrations_cfg.get("cursor", {}) if isinstance(integrations_cfg.get("cursor"), dict) else {}
    cursor_enabled = bool(cursor_cfg.get("enabled"))
    cursor_write_gate = bool(cursor_cfg.get("write_gate", False))
    cursor_has_dc_hooks = _hook_config_references_devcouncil(cursor_hooks) is True
    if cursor_enabled or cursor_has_dc_hooks:
        hooks_ok = False
        details = "Run dev integrate hooks --apply --tool cursor."
        if cursor_hooks.exists():
            try:
                hooks_data = read_json(cursor_hooks) or {}
                hook_events = hooks_data.get("hooks", {}) if isinstance(hooks_data, dict) else {}
                has_post = "postToolUse" in hook_events
                has_pre = "preToolUse" in hook_events
                # Assist (default): PostToolUse only. Contain: PreToolUse present.
                if cursor_write_gate:
                    hooks_ok = has_pre and has_post
                    details = str(cursor_hooks) if hooks_ok else (
                        "Run dev integrate cursor --apply --write-gate."
                    )
                else:
                    hooks_ok = has_post and not has_pre
                    if has_post and has_pre:
                        details = (
                            "Assist mode expected (no preToolUse). "
                            "Re-run dev integrate cursor --apply (or pass --write-gate)."
                        )
                    elif hooks_ok:
                        details = f"{cursor_hooks} (assist)"
                    else:
                        details = "Run dev integrate cursor --apply."
            except json.JSONDecodeError:
                hooks_ok = False
        add(hooks_ok, "Cursor hooks", details)
        from devcouncil.integrations.clients.cursor import probe_cursor_auth

        auth_ok, auth_details = probe_cursor_auth()
        # Authentication for an optional, non-selected client is machine/user
        # state, not a broken repository integration.  Keep it informational even
        # under --strict, matching missing optional CLI handling.
        add_optional(auth_ok, "Cursor auth", auth_details)

    grok_hooks = root / ".grok" / "hooks" / "devcouncil.json"
    grok_cfg = integrations_cfg.get("grok", {}) if isinstance(integrations_cfg.get("grok"), dict) else {}
    grok_enabled = bool(grok_cfg.get("enabled"))
    grok_write_gate = bool(grok_cfg.get("write_gate", False))
    grok_has_dc_hooks = _hook_config_references_devcouncil(grok_hooks) is True
    if grok_enabled or grok_has_dc_hooks:
        hooks_ok = False
        details = "Run dev integrate hooks --apply --tool grok (then /hooks-trust in Grok)."
        if grok_hooks.exists():
            try:
                hooks_data = read_json(grok_hooks) or {}
                hook_events = hooks_data.get("hooks", {}) if isinstance(hooks_data, dict) else {}
                has_post = "PostToolUse" in hook_events
                has_pre = "PreToolUse" in hook_events
                if grok_write_gate:
                    hooks_ok = has_pre and has_post
                    details = str(grok_hooks) if hooks_ok else (
                        "Run dev integrate hooks --apply --tool grok --write-gate."
                    )
                else:
                    hooks_ok = has_post and not has_pre
                    if has_post and has_pre:
                        details = (
                            "Assist mode expected (no PreToolUse). "
                            "Re-run dev integrate hooks --apply --tool grok."
                        )
                    elif hooks_ok:
                        details = f"{grok_hooks} (assist)"
                    else:
                        details = "Run dev integrate hooks --apply --tool grok (then /hooks-trust in Grok)."
            except json.JSONDecodeError:
                hooks_ok = False
        add(hooks_ok, "Grok hooks", details)

    opencode_config = integrate._opencode_config_path(root)
    opencode_cfg = integrations_cfg.get("opencode", {}) if isinstance(integrations_cfg.get("opencode"), dict) else {}
    opencode_enabled = bool(opencode_cfg.get("enabled"))
    opencode_write_gate = bool(opencode_cfg.get("write_gate", False))
    opencode_plugin = integrate._opencode_plugin_path(root)
    # Disabled + plugin removed = clean uninstall; do not demand re-apply.
    if opencode_enabled or opencode_plugin.exists():
        plugin_ok = opencode_plugin.exists()
        plugin_registered = False
        posture_ok = False
        details = "Run dev integrate hooks --apply --tool opencode."
        if plugin_ok and opencode_config.exists():
            try:
                from devcouncil.integrations.clients.opencode import _opencode_plugin_registered

                opencode_data = read_json(opencode_config) or {}
                plugin_registered = (
                    _opencode_plugin_registered(opencode_data)
                    if isinstance(opencode_data, dict)
                    else False
                )
            except json.JSONDecodeError:
                plugin_registered = False
        if plugin_ok:
            try:
                plugin_body = opencode_plugin.read_text(encoding="utf-8")
            except OSError:
                plugin_body = ""
            has_before = '"tool.execute.before"' in plugin_body
            has_after = '"tool.execute.after"' in plugin_body
            if opencode_write_gate:
                posture_ok = has_before and has_after
                if not posture_ok:
                    details = "Run dev integrate hooks --apply --tool opencode --write-gate."
            else:
                posture_ok = has_after and not has_before
                if has_after and has_before:
                    details = (
                        "Assist mode expected (no tool.execute.before). "
                        "Re-run dev integrate hooks --apply --tool opencode."
                    )
                elif posture_ok:
                    details = f"{opencode_plugin} (assist)"
        add(
            plugin_ok and plugin_registered and posture_ok,
            "OpenCode hook plugin",
            details if not (plugin_ok and plugin_registered and posture_ok) else (
                f"{opencode_plugin} (assist)" if not opencode_write_gate else str(opencode_plugin)
            ),
        )

    bundled_plugin = integrate._opencode_plugin_source()
    bundled_ok = bundled_plugin.exists()
    bundled_details = str(bundled_plugin) if bundled_ok else "Reinstall DevCouncil; package asset is missing."
    if bundled_ok:
        try:
            bundled_body = bundled_plugin.read_text(encoding="utf-8")
        except OSError:
            bundled_body = ""
        # Packaged source is the assist template; pre-tool handler must not ship by default.
        if '"tool.execute.before"' in bundled_body:
            bundled_ok = False
            bundled_details = (
                f"{bundled_plugin} must not register tool.execute.before "
                "(assist default; generator adds it only with write_gate)."
            )
    add(bundled_ok, "Bundled OpenCode hook plugin", bundled_details)

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

    # Tamper tripwire: emptied/disarmed DevCouncil gate events, or enabled:true
    # with hooks stripped mid-file. Clean uninstall that preserves user settings
    # (file exists, no DevCouncil hooks remain) is not a failure.
    for label, hook_path in _hook_config_tamper_targets(root):
        client_key = _HOOK_INTEGRITY_CLIENT_KEYS.get(label, label.lower())
        client_enabled = _client_integration_enabled(integrations_cfg, client_key)
        integrity_status = _hook_config_integrity_status(hook_path, client_enabled=client_enabled)
        if integrity_status is None:
            continue
        if integrity_status == "ok":
            add(True, f"{label} hook integrity", str(hook_path))
            continue
        if client_enabled and not _hook_config_has_disarmed_gate_events(hook_path):
            details = (
                f"{hook_path} no longer references DevCouncil while "
                f"integrations.{client_key}.enabled is true (hooks stripped)."
            )
        else:
            details = f"{hook_path} no longer references DevCouncil (tampered/disarmed)."
        add(False, f"{label} hook integrity", details)

    codex_hooks_path = root / ".codex" / "hooks.json"
    # Skip schema check after clean uninstall that left an empty/foreign hooks file.
    if codex_hooks_path.exists() and _hook_config_references_devcouncil(codex_hooks_path) is True:
        codex_cfg = integrations_cfg.get("codex", {}) if isinstance(integrations_cfg.get("codex"), dict) else {}
        codex_write_gate = bool(codex_cfg.get("write_gate", False))
        codex_schema_ok, codex_schema_details = _codex_hook_schema_status(root, write_gate=codex_write_gate)
        add(codex_schema_ok, "Codex hook schema", codex_schema_details)
        add_skip("Codex hook trust", "Open Codex in this project and run /hooks after any hook change.")

    from devcouncil.integrations.clients.common import check_hook_dev_executable

    hook_exe_ok, hook_exe_details = check_hook_dev_executable(root)
    if common_recorded_hook_exe(root) is not None:
        add(hook_exe_ok, "Hook `dev` executable", hook_exe_details)
    else:
        add_skip("Hook `dev` executable", hook_exe_details)

    recommended = resolve_automated_executor(root, None) if detected else None
    return IntegrationCheckReport(tuple(rows), recommended, failures)


def common_recorded_hook_exe(project_root: Path) -> str | None:
    from devcouncil.integrations.clients.common import recorded_hook_dev_executable

    return recorded_hook_dev_executable(project_root)


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
        grok_resume_mode = execution.grok_resume_mode
        custom_probe_order = list(execution.coding_cli_probe_order)
    except Exception:
        default_executor = "manual"
        stream_cli_output = False
        cursor_resume_mode = "off"
        grok_resume_mode = "off"
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
        "grok_resume_mode": grok_resume_mode,
        "capabilities": integration_capability_rows(project_root),
    }
