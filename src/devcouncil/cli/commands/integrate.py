import json
import shlex
import shutil
import subprocess
import sys
from contextlib import contextmanager
from pathlib import Path

import typer
import yaml  # type: ignore[import-untyped]
from rich.console import Console
from rich.table import Table

from devcouncil.executors.agent_registry import (
    BUILTIN_CODING_EXECUTOR_NAMES,
    CODING_CLI_INTEGRATION_INFO,
    VALID_INPUT_MODES,
    agent_config_entry,
    detect_available_coding_cli,
    integration_tier_label,
    is_reserved_agent_name,
    load_agent_profiles,
    load_cli_agent_specs,
    normalize_agent_name,
    resolve_automated_executor,
    resolve_coding_cli_executable,
    resolve_coding_cli_probe_order,
)
from devcouncil.integrations.actions import apply_integration_target
from devcouncil.integrations.clients.hooks import SESSION_START_MATCHER
from devcouncil.utils.subprocess_env import clean_subprocess_env
from devcouncil.integrations.check import (
    build_integration_check_report,
    integration_status_summary,
)

app = typer.Typer(help="Set up DevCouncil integrations with coding CLIs.")
setup_app = typer.Typer(help="Set up optional external companion integrations.")
app.add_typer(setup_app, name="setup")
console = Console()

SUPPORTED_TOOLS = ("codex", "gemini", "claude", "cursor", "opencode", "antigravity", "warp", "aider")
SUPPORTED_HOOK_TOOLS = ("codex", "gemini", "claude", "cursor")
OPENCODE_HOOK_PLUGIN_NAME = "opencode_devcouncil_plugin.mjs"
PREFERRED_COMMAND = "dev integrate"
LEGACY_COMMAND = "dev setup --integrate"


def _project_root(path: str | Path | None) -> Path:
    return Path(path or ".").expanduser().resolve()


def _warn_if_verify_only(client: str) -> None:
    """Print a prominent containment warning when wiring a verify-only client.

    Verify-only clients have no native pre-tool-use hook, so DevCouncil cannot block a
    forbidden write or command before it happens — it is only caught post-hoc at verify
    time. Surface this loudly so users don't assume hard containment."""
    info = CODING_CLI_INTEGRATION_INFO.get(normalize_agent_name(client))
    if info is not None and not info.hooks:
        console.print(
            f"[bold yellow]Warning ({info.label}): No pre-action containment — "
            "forbidden writes/commands are caught only at verify time.[/bold yellow]"
        )


def _server_args(project_root: Path) -> list[str]:
    return ["devcouncil", "mcp-server"]


def _codex_command(project_root: Path) -> list[str]:
    return [
        "codex",
        "mcp",
        "add",
        "devcouncil",
        "--env",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "--",
        *_server_args(project_root),
    ]


def _gemini_command(project_root: Path, scope: str) -> list[str]:
    return [
        "gemini",
        "mcp",
        "add",
        "--scope",
        scope,
        "--env",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "devcouncil",
        *_server_args(project_root),
    ]


def _claude_command(project_root: Path, scope: str) -> list[str]:
    # The server name must come BEFORE --env: the current Claude CLI treats --env
    # as variadic, so `--env KEY=VALUE devcouncil` swallows the name `devcouncil`
    # as a second (invalid) env var. Putting the name first — matching the working
    # codex form — and terminating options with `--` avoids that.
    return [
        "claude",
        "mcp",
        "add",
        "--scope",
        scope,
        "devcouncil",
        "--env",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "--",
        *_server_args(project_root),
    ]


def _cursor_config_path(project_root: Path) -> Path:
    return project_root / ".cursor" / "mcp.json"


def _cursor_mcp_config(project_root: Path) -> dict:
    return {
        "mcpServers": {
            "devcouncil": {
                "type": "stdio",
                "command": "devcouncil",
                "args": ["mcp-server"],
                "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
            }
        }
    }


def _warp_mcp_config(project_root: Path) -> dict:
    return {
        "devcouncil": {
            "command": "devcouncil",
            "args": ["mcp-server"],
            "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
        }
    }


def _warp_mcp_path(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "integrations" / "warp-mcp.json"


def _opencode_config_path(project_root: Path) -> Path:
    return project_root / "opencode.json"


def _opencode_mcp_entry(project_root: Path) -> dict:
    return {
        "type": "local",
        "command": ["devcouncil", "mcp-server"],
        "environment": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
        "enabled": True,
        "timeout": 10000,
    }


def _antigravity_mcp_path(project_root: Path) -> Path:
    return project_root / ".agents" / "mcp_config.json"


def _antigravity_mcp_config(project_root: Path) -> dict:
    return {
        "mcpServers": {
            "devcouncil": {
                "command": "devcouncil",
                "args": ["mcp-server"],
                "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
                "cwd": str(project_root),
            }
        }
    }


def _write_warp_mcp_config(project_root: Path) -> Path:
    path = _warp_mcp_path(project_root)
    _save_json(path, _warp_mcp_config(project_root))
    return path


def _write_cursor_config(project_root: Path) -> Path:
    path = _cursor_config_path(project_root)
    data = _load_json_strict(path, "Cursor")
    mcp_servers = data.setdefault("mcpServers", {})
    mcp_servers["devcouncil"] = _cursor_mcp_config(project_root)["mcpServers"]["devcouncil"]
    _save_json(path, data)
    return path


# When set, _mutate_raw_config applies record mutations in memory and
# _batched_raw_config saves config.yaml once at the end (used by
# `dev integrate all --apply`, which otherwise re-parses YAML per tool).
_PENDING_RAW_CONFIG: dict | None = None


@contextmanager
def _batched_raw_config(project_root: Path):
    global _PENDING_RAW_CONFIG
    # Re-entrant: if a batch is already active, participate in it instead of
    # starting a nested load/save (which would otherwise reset the shared
    # buffer to None on inner exit and drop the outer batch's mutations).
    if _PENDING_RAW_CONFIG is not None:
        yield
        return
    _PENDING_RAW_CONFIG = _load_raw_config(project_root)
    try:
        yield
    finally:
        # Persist whatever mutations accumulated, even if an inner installer raised
        # partway through — matching the old per-installer save, which committed each
        # installer's change immediately rather than dropping the whole batch on a
        # mid-loop failure.
        pending = _PENDING_RAW_CONFIG
        _PENDING_RAW_CONFIG = None
        if pending is not None:
            _save_raw_config(project_root, pending)


def _mutate_raw_config(project_root: Path, mutate) -> None:
    if _PENDING_RAW_CONFIG is not None:
        mutate(_PENDING_RAW_CONFIG)
        return
    config = _load_raw_config(project_root)
    mutate(config)
    _save_raw_config(project_root, config)


def _record_cursor_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        cursor = config.setdefault("integrations", {}).setdefault("cursor", {})
        cursor.update({
            "enabled": True,
            "config_path": str(_cursor_config_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)


def _record_warp_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        warp = config.setdefault("integrations", {}).setdefault("warp", {})
        warp.update({
            "enabled": True,
            "command": warp.get("command", "oz"),
            "run_mode": warp.get("run_mode", "local"),
            "mcp_config_path": str(_warp_mcp_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)


def _record_opencode_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        opencode = config.setdefault("integrations", {}).setdefault("opencode", {})
        opencode.update({
            "enabled": True,
            "config_path": str(_opencode_config_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)


def _record_antigravity_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        antigravity = config.setdefault("integrations", {}).setdefault("antigravity", {})
        antigravity.update({
            "enabled": True,
            "mcp_config_path": str(_antigravity_mcp_path(project_root).relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)


def _load_json_strict(path: Path, label: str = "JSON") -> dict:
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8")) or {}
    except json.JSONDecodeError as exc:
        raise ValueError(f"{path} is not valid JSON. Fix the {label} config before rerunning integration setup.") from exc


def _write_opencode_config(project_root: Path) -> Path:
    path = _opencode_config_path(project_root)
    data = _load_json_strict(path, "OpenCode")
    data.setdefault("$schema", "https://opencode.ai/config.json")
    mcp = data.setdefault("mcp", {})
    mcp["devcouncil"] = _opencode_mcp_entry(project_root)
    _save_json(path, data)
    return path


def _write_antigravity_mcp_config(project_root: Path) -> Path:
    path = _antigravity_mcp_path(project_root)
    data = _load_json_strict(path, "Antigravity")
    mcp_servers = data.setdefault("mcpServers", {})
    mcp_servers["devcouncil"] = _antigravity_mcp_config(project_root)["mcpServers"]["devcouncil"]
    _save_json(path, data)
    return path


def _configure_cursor(project_root: Path, apply: bool) -> bool:
    path = _cursor_config_path(project_root)
    config = _cursor_mcp_config(project_root)
    if not apply:
        console.print("[bold]Cursor[/bold]")
        console.print(f"Project MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print("Verify in Cursor CLI with: [dim]cursor-agent mcp list[/dim]")
        return True

    if not shutil.which("cursor") and not shutil.which("cursor-agent"):
        console.print("[yellow]Cursor CLI not found on PATH. Project MCP config will still be available to Cursor.[/yellow]")
    try:
        written = _write_cursor_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_cursor_config(project_root)
    console.print(f"[green]Cursor MCP config written:[/green] {written}")
    return True


def _configure_opencode(project_root: Path, apply: bool) -> bool:
    path = _opencode_config_path(project_root)
    config = {
        "$schema": "https://opencode.ai/config.json",
        "mcp": {"devcouncil": _opencode_mcp_entry(project_root)},
    }
    if not apply:
        console.print("[bold]OpenCode[/bold]")
        console.print(f"Project config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(
            "Direct executor command: "
            "[dim]opencode run --file .devcouncil/TASK-001-opencode-task.md "
            '"Execute the DevCouncil task described in the attached prompt file."[/dim]'
        )
        return True

    if not shutil.which("opencode"):
        console.print("[yellow]OpenCode CLI not found on PATH. Install it before using `dev run --executor opencode`.[/yellow]")
    try:
        written = _write_opencode_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_opencode_config(project_root)
    console.print(f"[green]OpenCode MCP config written:[/green] {written}")
    return True


def _configure_antigravity(project_root: Path, apply: bool) -> bool:
    path = _antigravity_mcp_path(project_root)
    config = _antigravity_mcp_config(project_root)
    if not apply:
        console.print("[bold]Google Antigravity CLI[/bold]")
        console.print(f"Project MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(
            "Direct executor command: "
            "[dim]agy --print --print-timeout 30m "
            '"Read and execute the DevCouncil task prompt at .devcouncil/TASK-001-antigravity-task.md."[/dim]'
        )
        return True

    if not shutil.which("agy"):
        console.print("[yellow]Antigravity CLI (`agy`) not found on PATH. Install it before using `dev run --executor antigravity`.[/yellow]")
    try:
        written = _write_antigravity_mcp_config(project_root)
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        return False
    _record_antigravity_config(project_root)
    console.print(f"[green]Antigravity MCP config written:[/green] {written}")
    return True


def _configure_warp(project_root: Path, apply: bool) -> bool:
    path = _warp_mcp_path(project_root)
    config = _warp_mcp_config(project_root)
    if not apply:
        console.print("[bold]Warp / Oz[/bold]")
        console.print(f"MCP config file: [dim]{path}[/dim]")
        console.print(json.dumps(config, separators=(",", ":")), soft_wrap=True)
        console.print(f"Direct executor command: [dim]oz agent run --cwd {project_root} --mcp {path} --prompt <task prompt>[/dim]")
        return True

    written = _write_warp_mcp_config(project_root)
    _record_warp_config(project_root)
    console.print(f"[green]Warp MCP config written:[/green] {written}")
    if not shutil.which("oz"):
        console.print("[yellow]oz CLI not found on PATH. Install Warp/Oz before using `dev run --executor warp`.[/yellow]")
    return True


def _format_command(command: list[str]) -> str:
    if sys.platform == "win32":
        return " ".join(_quote_powershell_arg(arg) for arg in command)
    return shlex.join(command)


def _quote_powershell_arg(arg: str) -> str:
    if arg == "":
        return "''"
    special_chars = set(" \t\r\n'\"{}[](),;|&<>")
    if not any(char in special_chars for char in arg):
        return arg
    return "'" + arg.replace("'", "''") + "'"


def _opencode_plugin_source() -> Path:
    return Path(__file__).resolve().parents[2] / "integrations" / OPENCODE_HOOK_PLUGIN_NAME


def _opencode_plugin_path(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "integrations" / OPENCODE_HOOK_PLUGIN_NAME


def _hook_command(project_root: Path, client: str, event: str) -> str:
    return _format_command([
        "devcouncil",
        "hook",
        event,
        "--client",
        client,
        "--project-root",
        str(project_root),
    ])


def _probe_mcp_tools(root: Path, *, timeout_seconds: float = 30.0) -> list[str]:
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    import asyncio
    import os

    async def _list_tools() -> list[str]:
        env = os.environ.copy()
        env["DEVCOUNCIL_PROJECT_ROOT"] = str(root)
        params = StdioServerParameters(
            command=sys.executable,
            args=["-m", "devcouncil", "mcp-server"],
            cwd=str(root),
            env=env,
        )
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                await session.initialize()
                tools = await session.list_tools()
                return [tool.name for tool in tools.tools]

    async def _list_tools_with_deadline() -> list[str]:
        # A wedged server process would otherwise block `dev integrate check`
        # indefinitely; the caller treats TimeoutError as a failed probe.
        return await asyncio.wait_for(_list_tools(), timeout=timeout_seconds)

    return asyncio.run(_list_tools_with_deadline())


def _run(command: list[str]) -> int:
    executable = shutil.which(command[0])
    if not executable:
        return 127
    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    invocation = subprocess.list2cmdline(resolved) if use_shell else resolved
    try:
        result = subprocess.run(invocation, text=True, shell=use_shell)
    except (FileNotFoundError, OSError):
        return 127
    return result.returncode


def _run_capture(command: list[str], timeout: int = 10) -> tuple[int, str]:
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


def _config_path(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "config.yaml"


def _load_raw_config(project_root: Path) -> dict:
    path = _config_path(project_root)
    if not path.exists():
        return {}
    return yaml.safe_load(path.read_text(encoding="utf-8")) or {}


def _save_raw_config(project_root: Path, config: dict) -> None:
    path = _config_path(project_root)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(config, sort_keys=False), encoding="utf-8")


def _load_json(path: Path) -> dict:
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8")) or {}
    except json.JSONDecodeError:
        return {}


def _save_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def _upsert_hook(settings: dict, event: str, matcher: str, command: str, name: str) -> None:
    hooks = settings.setdefault("hooks", {})
    groups = hooks.setdefault(event, [])
    for group in groups:
        if group.get("matcher") == matcher:
            group_hooks = group.setdefault("hooks", [])
            if not any(hook.get("command") == command for hook in group_hooks):
                group_hooks.append({
                    "type": "command",
                    "name": name,
                    "command": command,
                    "timeout": 10000,
                })
            return
    groups.append({
        "matcher": matcher,
        "hooks": [{
            "type": "command",
            "name": name,
            "command": command,
            "timeout": 10000,
        }],
    })


def _ensure_codex_hooks_enabled(project_root: Path) -> Path:
    config_path = project_root / ".codex" / "config.toml"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    existing = config_path.read_text(encoding="utf-8") if config_path.exists() else ""
    if "codex_hooks" not in existing:
        if "[features]" in existing:
            lines = existing.splitlines()
            updated: list[str] = []
            in_features = False
            inserted = False
            for line in lines:
                stripped = line.strip()
                if stripped == "[features]":
                    in_features = True
                    updated.append(line)
                    continue
                if in_features and stripped.startswith("[") and stripped.endswith("]"):
                    updated.append("codex_hooks = true")
                    inserted = True
                    in_features = False
                updated.append(line)
            if in_features and not inserted:
                updated.append("codex_hooks = true")
            config_path.write_text("\n".join(updated) + "\n", encoding="utf-8")
        else:
            separator = "\n" if existing and not existing.endswith("\n") else ""
            config_path.write_text(f"{existing}{separator}\n[features]\ncodex_hooks = true\n", encoding="utf-8")
    return config_path


def _install_codex_hooks(project_root: Path) -> list[Path]:
    path = project_root / ".codex" / "hooks.json"
    settings = _load_json(path)
    matcher = "Bash|shell_command|exec_command|local_shell|Write|Edit|MultiEdit|write_file|edit_file|apply_patch"
    _upsert_hook(
        settings,
        "PreToolUse",
        matcher,
        _hook_command(project_root, "codex", "pre-tool-use"),
        "devcouncil-pre-tool-use",
    )
    _upsert_hook(
        settings,
        "PostToolUse",
        matcher,
        _hook_command(project_root, "codex", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    _save_json(path, settings)
    return [path, _ensure_codex_hooks_enabled(project_root)]


def _install_gemini_hooks(project_root: Path) -> list[Path]:
    path = project_root / ".gemini" / "settings.json"
    settings = _load_json(path)
    matcher = "run_shell_command|shell_command|write_file|edit_file|replace|apply_patch"
    _upsert_hook(
        settings,
        "BeforeTool",
        matcher,
        _hook_command(project_root, "gemini", "pre-tool-use"),
        "devcouncil-pre-tool-use",
    )
    _upsert_hook(
        settings,
        "AfterTool",
        matcher,
        _hook_command(project_root, "gemini", "post-tool-use"),
        "devcouncil-post-tool-use",
    )
    _save_json(path, settings)
    return [path]


def _upsert_cursor_hook(settings: dict, event: str, matcher: str, command: str) -> None:
    hooks = settings.setdefault("hooks", {})
    entries = hooks.setdefault(event, [])
    for entry in entries:
        if entry.get("command") == command:
            return
    payload: dict = {"command": command}
    if matcher:
        payload["matcher"] = matcher
    entries.append(payload)


def _install_cursor_hooks(project_root: Path) -> list[Path]:
    path = project_root / ".cursor" / "hooks.json"
    settings = _load_json(path)
    settings.setdefault("version", 1)
    matcher = "Shell|Write|Edit|MultiEdit|Read|Task"
    _upsert_cursor_hook(
        settings,
        "preToolUse",
        matcher,
        _hook_command(project_root, "cursor", "pre-tool-use"),
    )
    _upsert_cursor_hook(
        settings,
        "postToolUse",
        matcher,
        _hook_command(project_root, "cursor", "post-tool-use"),
    )
    _save_json(path, settings)

    def mutate(config: dict) -> None:
        cursor = config.setdefault("integrations", {}).setdefault("cursor", {})
        cursor.update({
            "hooks_path": str(path.relative_to(project_root)),
        })

    _mutate_raw_config(project_root, mutate)
    return [path]


def _install_opencode_hooks(project_root: Path) -> list[Path]:
    source = _opencode_plugin_source()
    if not source.exists():
        raise FileNotFoundError(f"Missing bundled OpenCode hook plugin: {source}")
    destination = _opencode_plugin_path(project_root)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(source.read_text(encoding="utf-8"), encoding="utf-8")

    path = _opencode_config_path(project_root)
    data = _load_json_strict(path, "OpenCode") if path.exists() else {"$schema": "https://opencode.ai/config.json"}
    data.setdefault("$schema", "https://opencode.ai/config.json")
    plugins_raw = data.setdefault("plugin", [])
    if not isinstance(plugins_raw, list):
        plugins_raw = []
        data["plugin"] = plugins_raw
    plugins: list[str] = [str(item) for item in plugins_raw]
    data["plugin"] = plugins
    plugin_ref = f"./.devcouncil/integrations/{OPENCODE_HOOK_PLUGIN_NAME}"
    if plugin_ref not in plugins:
        plugins.append(plugin_ref)
    _save_json(path, data)
    _record_opencode_config(project_root)
    return [destination, path]


def _install_claude_hooks(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Install DevCouncil's Claude Code hooks into .claude/settings.local.json.

    By default this installs only the *assistive* lifecycle hooks (status injection on
    SessionStart/UserPromptSubmit, the live-review Stop signal, and the SessionEnd/
    PreCompact/SubagentStop/Notification trace hooks). These never block a tool call.

    The blocking pre-action **write-gate** (PreToolUse/PostToolUse, which denies any
    Bash/Write/Edit not authorized by an active task lease) is installed ONLY when
    ``write_gate`` is True. It is meant for autonomous executor runs, not interactive
    human sessions — in an interactive session there is no task lease, so the gate would
    fail-closed and deny every command. (``dev run --executor claude`` does its own
    post-hoc scope enforcement and does not depend on this hook, so leaving it off by
    default loses no containment.)"""
    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    matcher = "Bash|Write|Edit|MultiEdit"
    if write_gate:
        _upsert_hook(
            settings,
            "PreToolUse",
            matcher,
            _hook_command(project_root, "claude", "pre-tool-use"),
            "devcouncil-pre-tool-use",
        )
        _upsert_hook(
            settings,
            "PostToolUse",
            matcher,
            _hook_command(project_root, "claude", "post-tool-use"),
            "devcouncil-post-tool-use",
        )
    _upsert_hook(
        settings,
        "Stop",
        "",
        _hook_command(project_root, "claude", "agent-response"),
        "devcouncil-agent-response-ready",
    )
    # Lifecycle events: status-on-start/prompt, teardown, compaction, subagent finish,
    # and notifications. These complete DevCouncil's coverage of the documented Claude
    # Code hook surface beyond the pre/post/stop gate.
    _upsert_hook(
        settings,
        "SessionStart",
        SESSION_START_MATCHER,
        _hook_command(project_root, "claude", "session-start"),
        "devcouncil-session-start",
    )
    _upsert_hook(
        settings,
        "UserPromptSubmit",
        "",
        _hook_command(project_root, "claude", "user-prompt-submit"),
        "devcouncil-user-prompt-submit",
    )
    _upsert_hook(
        settings,
        "SessionEnd",
        "",
        _hook_command(project_root, "claude", "session-end"),
        "devcouncil-session-end",
    )
    _upsert_hook(
        settings,
        "PreCompact",
        "",
        _hook_command(project_root, "claude", "pre-compact"),
        "devcouncil-pre-compact",
    )
    _upsert_hook(
        settings,
        "PostCompact",
        "",
        _hook_command(project_root, "claude", "post-compact"),
        "devcouncil-post-compact",
    )
    _upsert_hook(
        settings,
        "SubagentStop",
        "",
        _hook_command(project_root, "claude", "subagent-stop"),
        "devcouncil-subagent-stop",
    )
    _upsert_hook(
        settings,
        "Notification",
        "",
        _hook_command(project_root, "claude", "notification"),
        "devcouncil-notification",
    )
    _save_json(path, settings)
    return [path]


def _devcouncil_version() -> str:
    """Package version for plugin manifests, or a stable placeholder when uninstalled."""
    import importlib.metadata

    try:
        return importlib.metadata.version("devcouncil")
    except importlib.metadata.PackageNotFoundError:
        return "0.0.0"


# Read-only DevCouncil commands the generated slash commands / hooks shell out to. Adding
# them to the Claude permissions allow-list keeps the integration from prompting on every
# `dev status`/`dev report` the slash commands run.
_CLAUDE_PERMISSION_ALLOW = [
    "Bash(dev status:*)",
    "Bash(dev report:*)",
    "Bash(dev tasks:*)",
    "Bash(dev verify:*)",
    "Bash(dev repair:*)",
    "Bash(dev plan:*)",
    "Bash(dev watch:*)",
    "Bash(devcouncil mcp-server)",
]


def _install_claude_settings(project_root: Path) -> tuple[Path, bool]:
    """Write the statusLine, MCP enablement, and permission allow-list into Claude settings.

    Merges into .claude/settings.local.json without clobbering existing user entries.
    Returns (path, changed); only rewrites the file when the merge changes something so
    re-running integration is a true no-op."""
    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)

    settings["statusLine"] = {
        "type": "command",
        "command": "devcouncil hook claude-statusline",
    }
    # Auto-enable the project-scoped DevCouncil MCP server so a teammate cloning the repo
    # doesn't have to approve it interactively.
    enabled = settings.setdefault("enabledMcpjsonServers", [])
    if isinstance(enabled, list) and "devcouncil" not in enabled:
        enabled.append("devcouncil")

    permissions = settings.setdefault("permissions", {})
    if isinstance(permissions, dict):
        allow = permissions.setdefault("allow", [])
        if isinstance(allow, list):
            for rule in _CLAUDE_PERMISSION_ALLOW:
                if rule not in allow:
                    allow.append(rule)

    changed = json.dumps(settings, sort_keys=True) != before
    if changed:
        _save_json(path, settings)
    return path, changed


def _selected_skill_assets(project_root: Path):
    """Scaffold the applicable skills and return them as GeneratedAsset-like records.

    Returns (written_paths, skill_assets) where skill_assets carry (path, content) for the
    plugin bundler so the plugin ships the same skill bodies that land in .claude/skills/."""
    from devcouncil.integrations.claude_assets import GeneratedAsset
    from devcouncil.skills.registry import scaffold_skills, select_skills

    skills = select_skills("", project_root)
    written = scaffold_skills(project_root, skills)
    assets: list[GeneratedAsset] = []
    skills_root = project_root / ".claude" / "skills"
    for skill in skills:
        target = skills_root / skill.name / "SKILL.md"
        if target.exists():
            assets.append(GeneratedAsset(target, target.read_text(encoding="utf-8")))
    return written, assets


def _install_claude_assets(project_root: Path) -> list[Path]:
    """Generate the static Claude Code asset surface (commands, agents, output style,
    statusline, permissions) and scaffold the applicable skills. Idempotent."""
    from devcouncil.integrations import claude_assets

    written: list[Path] = []
    assets: list[claude_assets.GeneratedAsset] = []
    assets += claude_assets.build_slash_commands(project_root)
    assets += claude_assets.build_subagents(project_root)
    assets += claude_assets.build_output_style(project_root)
    for asset in assets:
        if asset.write_if_changed():
            written.append(asset.path)

    skills_written, _ = _selected_skill_assets(project_root)
    written.extend(skills_written)
    settings_path, settings_changed = _install_claude_settings(project_root)
    if settings_changed:
        written.append(settings_path)
    return written


def _install_claude_plugin(project_root: Path, *, write_gate: bool = False) -> list[Path]:
    """Build the self-contained Claude Code plugin + single-repo marketplace bundle.

    Bundles the commands, agents, applicable skills, hooks, and MCP config so the entire
    DevCouncil integration installs with one `/plugin install`. Assist-mode hooks by
    default; pass write_gate=True to bundle the blocking containment gate."""
    from devcouncil.integrations import claude_assets

    _, skill_assets = _selected_skill_assets(project_root)
    bundle = claude_assets.build_plugin_bundle(
        project_root, version=_devcouncil_version(), skill_assets=skill_assets, write_gate=write_gate
    )
    return [asset.path for asset in bundle if asset.write_if_changed()]


def _uninstall_claude(project_root: Path) -> list[str]:
    """Remove everything DevCouncil installed into a Claude Code project. Idempotent.

    Strips DevCouncil's hooks (every event), the DevCouncil statusLine, the MCP enablement
    and permission rules from .claude/settings.local.json (leaving any user-authored
    entries untouched), deletes the generated commands/subagents/output-style files, and
    best-effort de-registers the MCP server via `claude mcp remove`. Returns a list of the
    changes made. The recoverable, in-band counterpart to a fail-closed write-gate."""
    removed: list[str] = []
    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    before = json.dumps(settings, sort_keys=True)

    # Hooks: drop any entry whose command invokes `devcouncil hook`, then prune empties.
    hooks = settings.get("hooks")
    if isinstance(hooks, dict):
        for event in list(hooks):
            groups = hooks.get(event)
            if not isinstance(groups, list):
                continue
            kept_groups = []
            for group in groups:
                inner = group.get("hooks", []) if isinstance(group, dict) else []
                inner_kept = [
                    h for h in inner
                    if "devcouncil hook" not in str(h.get("command", ""))
                ]
                if inner_kept:
                    group["hooks"] = inner_kept
                    kept_groups.append(group)
            if kept_groups:
                hooks[event] = kept_groups
            else:
                hooks.pop(event)
        if not hooks:
            settings.pop("hooks")
        removed.append(f"hooks in {path.name}")

    # statusLine: only remove ours.
    status = settings.get("statusLine")
    if isinstance(status, dict) and "devcouncil" in str(status.get("command", "")):
        settings.pop("statusLine")
        removed.append("statusLine")

    enabled = settings.get("enabledMcpjsonServers")
    if isinstance(enabled, list) and "devcouncil" in enabled:
        enabled.remove("devcouncil")
        if not enabled:
            settings.pop("enabledMcpjsonServers")
        removed.append("enabledMcpjsonServers entry")

    permissions = settings.get("permissions")
    if isinstance(permissions, dict) and isinstance(permissions.get("allow"), list):
        kept = [r for r in permissions["allow"] if r not in _CLAUDE_PERMISSION_ALLOW]
        if len(kept) != len(permissions["allow"]):
            permissions["allow"] = kept
            removed.append("permission allow-rules")
        if not permissions.get("allow"):
            permissions.pop("allow", None)
        if not permissions:
            settings.pop("permissions")

    if json.dumps(settings, sort_keys=True) != before:
        if settings:
            _save_json(path, settings)
        elif path.exists():
            path.unlink()
            removed.append(f"deleted empty {path.name}")

    # Generated asset files.
    targets = [
        project_root / ".claude" / "commands" / "devcouncil",
        project_root / ".claude" / "output-styles" / "devcouncil.md",
    ]
    targets += [
        project_root / ".claude" / "agents" / f"{name}.md"
        for name in ("devcouncil-implementer", "devcouncil-verifier", "devcouncil-reviewer")
    ]
    for target in targets:
        if target.is_dir():
            shutil.rmtree(target)
            removed.append(str(target.relative_to(project_root)))
        elif target.exists():
            target.unlink()
            removed.append(str(target.relative_to(project_root)))

    # De-register the MCP server (best-effort; only if the claude CLI is present).
    if shutil.which("claude"):
        code = _run(["claude", "mcp", "remove", "devcouncil"])
        if code == 0:
            removed.append("claude mcp server registration")

    return removed


def _preview_hook_paths(project_root: Path, tool: str) -> list[tuple[str, Path]]:
    paths = {
        "codex": [project_root / ".codex" / "hooks.json", project_root / ".codex" / "config.toml"],
        "gemini": [project_root / ".gemini" / "settings.json"],
        "claude": [project_root / ".claude" / "settings.local.json"],
        "cursor": [project_root / ".cursor" / "hooks.json"],
        "opencode": [_opencode_plugin_path(project_root), _opencode_config_path(project_root)],
    }
    selected: tuple[str, ...]
    if tool == "all":
        selected = (*SUPPORTED_HOOK_TOOLS, "opencode")
    elif tool == "opencode":
        selected = ("opencode",)
    else:
        selected = (tool,)
    return [(client, path) for client in selected for path in paths.get(client, [])]


def _configure_native_hooks(
    project_root: Path, tool: str = "all", apply: bool = False, *, claude_write_gate: bool = False
) -> None:
    allowed = {"all", *SUPPORTED_HOOK_TOOLS, "opencode"}
    if tool not in allowed:
        console.print("[red]--tool must be one of: all, codex, gemini, claude, cursor, opencode.[/red]")
        raise typer.Exit(code=2)

    if not apply:
        console.print("[bold]Native hook config preview[/bold]")
        for client, path in _preview_hook_paths(project_root, tool):
            console.print(f"{client}: {path}", soft_wrap=True)
        console.print("[yellow]Preview only. Rerun with --apply to write hook config files.[/yellow]")
        return

    selected: tuple[str, ...]
    if tool == "all":
        selected = (*SUPPORTED_HOOK_TOOLS, "opencode")
    elif tool == "opencode":
        selected = ("opencode",)
    else:
        selected = (tool,)
    installers = {
        "codex": _install_codex_hooks,
        "gemini": _install_gemini_hooks,
        # Claude's blocking write-gate is opt-in (assist-mode default); the other clients
        # install their native pre/post hooks unconditionally as before.
        "claude": lambda root: _install_claude_hooks(root, write_gate=claude_write_gate),
        "cursor": _install_cursor_hooks,
        "opencode": _install_opencode_hooks,
    }
    # Batch the per-installer config.yaml record updates (cursor/opencode)
    # into one load/save instead of re-parsing YAML per tool.
    with _batched_raw_config(project_root):
        for client in selected:
            try:
                written = installers[client](project_root)
            except (ValueError, FileNotFoundError) as exc:
                console.print(f"[red]{client} hook setup failed: {exc}[/red]")
                raise typer.Exit(code=1) from exc
            console.print(f"[green]{client} native hooks configured:[/green] {', '.join(str(path) for path in written)}")


def _print_command(tool: str, command: list[str], apply: bool):
    if apply:
        console.print(f"[cyan]Configuring {tool} MCP integration...[/cyan]")
    else:
        console.print(f"[bold]{tool}[/bold]")
        console.print(_format_command(command), soft_wrap=True)


def _configure(tool: str, command: list[str], apply: bool) -> bool:
    executable = command[0]
    if not shutil.which(executable):
        console.print(f"[yellow]{tool} CLI not found on PATH. Install it first, then rerun this command.[/yellow]")
        console.print(_format_command(command), soft_wrap=True)
        return False

    _print_command(tool, command, apply)
    if not apply:
        return True

    code = _run(command)
    if code == 0:
        console.print(f"[green]{tool} integration configured.[/green]")
        return True

    console.print(f"[red]{tool} integration command failed with exit code {code}.[/red]")
    console.print("You can rerun it manually:")
    console.print(_format_command(command), soft_wrap=True)
    return False


@app.callback(invoke_without_command=True)
def overview(ctx: typer.Context):
    """
    Show integration options for supported coding CLIs.
    """
    if ctx.invoked_subcommand is not None:
        return

    table = Table(title="DevCouncil Coding CLI Integrations")
    table.add_column("Tool", style="cyan")
    table.add_column("Setup command", style="green")
    table.add_column("Notes")
    table.add_row("Codex CLI", f"{PREFERRED_COMMAND} codex --apply", "Adds DevCouncil as a stdio MCP server.")
    table.add_row("Gemini CLI", f"{PREFERRED_COMMAND} gemini --apply", "Adds DevCouncil as a project-scoped stdio MCP server.")
    table.add_row("Claude Code", f"{PREFERRED_COMMAND} claude --apply", "MCP + assistive hooks + slash commands, subagents, output style, skills, statusline. Add --write-gate for blocking containment.")
    table.add_row("Claude assets", f"{PREFERRED_COMMAND} claude-assets --apply", "Slash commands, subagents, output style, statusline, permissions, skills (no MCP/hooks).")
    table.add_row("Claude plugin", f"{PREFERRED_COMMAND} claude-plugin --apply", "Self-contained Claude Code plugin + marketplace bundling everything for /plugin install.")
    table.add_row("Claude uninstall", f"{PREFERRED_COMMAND} claude --uninstall", "Remove DevCouncil hooks, statusline, MCP enablement, and generated assets from .claude/.")
    table.add_row("Cursor", f"{PREFERRED_COMMAND} cursor --apply", "Writes project .cursor/mcp.json for Cursor editor and cursor-agent.")
    table.add_row("OpenCode", f"{PREFERRED_COMMAND} opencode --apply", "Adds DevCouncil as a project-scoped OpenCode MCP server and executor.")
    table.add_row("Google Antigravity CLI", f"{PREFERRED_COMMAND} antigravity --apply", "Writes project .agents/mcp_config.json and enables the agy executor.")
    table.add_row("Warp / Oz", f"{PREFERRED_COMMAND} warp --apply", "Writes a Warp-compatible MCP JSON file for local agents and Oz CLI.")
    table.add_row("Aider", f"{PREFERRED_COMMAND} aider --apply", "Enables the built-in Aider headless executor (no MCP).")
    table.add_row("Bring your own CLI", f"{PREFERRED_COMMAND} cli-agent NAME --command TOOL --apply", "Registers any prompt-taking CLI as a DevCouncil executor.")
    table.add_row("All", f"{PREFERRED_COMMAND} all --apply", "Runs MCP setup and installs native hooks.")
    table.add_row("Native hooks", f"{PREFERRED_COMMAND} hooks --apply", "Installs Codex, Gemini, Claude, Cursor, and OpenCode hook files.")
    table.add_row("Recommend", f"{PREFERRED_COMMAND} recommend", "Show the best executor for this machine and project.")
    table.add_row("Status", f"{PREFERRED_COMMAND} status", "Compact PATH + config summary (no MCP probe).")
    table.add_row("Matrix", f"{PREFERRED_COMMAND} matrix", "Print built-in coding CLI integration tiers.")
    table.add_row("Check", f"{PREFERRED_COMMAND} check", "Verify MCP, hooks, and optional CLIs (--strict, --json for CI).")
    console.print(table)
    console.print(f"\nIf your install exposes only the setup flow, use: {LEGACY_COMMAND} --apply")
    console.print("\nRun without [bold]--apply[/bold] to preview the exact commands first.")


@app.command("doctor")
def integrations_doctor(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Check optional integration tools and local client wiring prerequisites."""
    root = _project_root(project_root)
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
    console.print(table)


@app.command("codex")
def codex(
    apply: bool = typer.Option(False, "--apply", help="Run the setup command instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Set up DevCouncil MCP tools for Codex CLI.
    """
    root = _project_root(project_root)
    command = _codex_command(root)
    ok = _configure("Codex CLI", command, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("gemini")
def gemini(
    apply: bool = typer.Option(False, "--apply", help="Run the setup command instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    scope: str = typer.Option("project", "--scope", help="Gemini MCP config scope: project or user."),
):
    """
    Set up DevCouncil MCP tools for Gemini CLI.
    """
    if scope not in {"project", "user"}:
        console.print("[red]--scope must be 'project' or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = _project_root(project_root)
    command = _gemini_command(root, scope)
    ok = _configure("Gemini CLI", command, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("claude")
def claude(
    apply: bool = typer.Option(False, "--apply", help="Run the setup command instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    scope: str = typer.Option("local", "--scope", help="Claude MCP config scope: local, project, or user."),
    write_gate: bool = typer.Option(
        False,
        "--write-gate/--no-write-gate",
        "--contain/--no-contain",
        help="Also install the blocking PreToolUse/PostToolUse write-gate (containment). "
        "Off by default — it denies any tool call not authorized by an active task lease, "
        "which fail-closes an interactive session. Use it for autonomous executor runs.",
    ),
    uninstall: bool = typer.Option(
        False,
        "--uninstall",
        help="Remove DevCouncil's Claude hooks, statusline, MCP enablement, and generated assets.",
    ),
):
    """
    Set up DevCouncil for Claude Code: MCP server + assistive hooks + slash commands,
    subagents, output style, skills, and statusline. The blocking write-gate is opt-in
    via --write-gate. Use --uninstall to remove everything DevCouncil installed.
    """
    root = _project_root(project_root)
    if uninstall:
        removed = _uninstall_claude(root)
        if removed:
            console.print(f"[green]Removed DevCouncil Claude integration[/green] ({len(removed)} change(s)):")
            for item in removed:
                console.print(f"  {item}")
        else:
            console.print("[dim]Nothing to remove — DevCouncil Claude integration not found.[/dim]")
        return

    if scope not in {"local", "project", "user"}:
        console.print("[red]--scope must be 'local', 'project', or 'user'.[/red]")
        raise typer.Exit(code=2)

    command = _claude_command(root, scope)
    ok = _configure("Claude Code", command, apply)
    if apply:
        # One-shot: MCP server + assistive hooks (write-gate only with --write-gate) + the
        # static asset surface (slash commands, subagents, output style, skills, statusline).
        try:
            written = _install_claude_hooks(root, write_gate=write_gate)
            written += _install_claude_assets(root)
        except (ValueError, FileNotFoundError, OSError) as exc:
            console.print(f"[red]Claude asset setup failed: {exc}[/red]")
            raise typer.Exit(code=1) from exc
        mode = "with write-gate (containment)" if write_gate else "assist mode (no write-gate)"
        console.print(
            f"[green]Claude Code integration installed[/green] ({len(written)} file(s), {mode}): "
            "MCP, hooks, slash commands, subagents, output style, skills, statusline, permissions."
        )
        if not write_gate:
            console.print(
                "[dim]Add pre-action containment for autonomous runs with[/dim] "
                f"[dim]{PREFERRED_COMMAND} claude --apply --write-gate[/dim]"
            )
        console.print(
            "Bundle everything as an installable plugin with: "
            f"[dim]{PREFERRED_COMMAND} claude-plugin --apply[/dim]"
        )
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("claude-assets")
def claude_assets_cmd(
    apply: bool = typer.Option(False, "--apply", help="Write the Claude asset files instead of previewing them."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Generate the Claude Code asset surface: slash commands, subagents, output style,
    statusline, permissions, and scaffolded skills (no MCP/hook registration).
    """
    from devcouncil.integrations import claude_assets as _assets

    root = _project_root(project_root)
    if not apply:
        console.print("[bold]Claude Code assets (preview)[/bold]")
        preview = (
            _assets.build_slash_commands(root)
            + _assets.build_subagents(root)
            + _assets.build_output_style(root)
        )
        for asset in preview:
            console.print(f"  {asset.path}", soft_wrap=True)
        console.print("  .claude/settings.local.json (statusLine + permissions + enabledMcpjsonServers)")
        console.print("  .claude/skills/<applicable>/SKILL.md")
        console.print("[yellow]Preview only. Rerun with --apply to write the files.[/yellow]")
        return

    try:
        written = _install_claude_assets(root)
    except (ValueError, FileNotFoundError, OSError) as exc:
        console.print(f"[red]Claude asset setup failed: {exc}[/red]")
        raise typer.Exit(code=1) from exc
    console.print(f"[green]Wrote {len(written)} Claude asset file(s).[/green]")
    for path in written:
        try:
            console.print(f"  {path.relative_to(root).as_posix()}")
        except ValueError:
            console.print(f"  {path}")


@app.command("claude-plugin")
def claude_plugin_cmd(
    apply: bool = typer.Option(False, "--apply", help="Write the plugin bundle instead of previewing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    write_gate: bool = typer.Option(
        False,
        "--write-gate/--no-write-gate",
        "--contain/--no-contain",
        help="Bundle Claude's blocking write-gate in the plugin hooks (off by default).",
    ),
):
    """
    Build a self-contained Claude Code plugin + single-repo marketplace bundling the
    DevCouncil commands, subagents, skills, hooks, and MCP server under
    .devcouncil/claude-plugin/ for one-command `/plugin install`.
    """
    from devcouncil.integrations.claude_assets import PLUGIN_ROOT_REL

    root = _project_root(project_root)
    market_dir = root / PLUGIN_ROOT_REL
    if not apply:
        console.print("[bold]Claude Code plugin bundle (preview)[/bold]")
        console.print(f"Marketplace + plugin root: [dim]{market_dir}[/dim]")
        console.print("Install after --apply with:")
        console.print(f"  [dim]/plugin marketplace add {market_dir}[/dim]")
        console.print("  [dim]/plugin install devcouncil@devcouncil-local[/dim]")
        console.print("[yellow]Preview only. Rerun with --apply to write the bundle.[/yellow]")
        return

    try:
        written = _install_claude_plugin(root, write_gate=write_gate)
    except (ValueError, FileNotFoundError, OSError) as exc:
        console.print(f"[red]Claude plugin build failed: {exc}[/red]")
        raise typer.Exit(code=1) from exc
    console.print(f"[green]Built Claude plugin bundle[/green] ({len(written)} file(s)) at {market_dir}")
    console.print("Install it in Claude Code with:")
    console.print(f"  [dim]/plugin marketplace add {market_dir}[/dim]")
    console.print("  [dim]/plugin install devcouncil@devcouncil-local[/dim]")


@app.command("cursor")
def cursor(
    apply: bool = typer.Option(False, "--apply", help="Write project Cursor MCP config instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Set up DevCouncil MCP tools for Cursor.
    """
    root = _project_root(project_root)
    if apply:
        report = apply_integration_target(root, "cursor")
        if not report.ok:
            console.print(report.to_json())
            raise typer.Exit(code=1)
        console.print("[green]Cursor integration configured.[/green]")
        return
    ok = _configure_cursor(root, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("opencode")
def opencode(
    apply: bool = typer.Option(False, "--apply", help="Write project OpenCode config instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Set up DevCouncil MCP tools for OpenCode.
    """
    root = _project_root(project_root)
    if apply:
        report = apply_integration_target(root, "opencode")
        if not report.ok:
            console.print(report.to_json())
            raise typer.Exit(code=1)
        console.print("[green]OpenCode integration configured.[/green]")
        return
    ok = _configure_opencode(root, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("agy")
@app.command("antigravity")
def antigravity(
    apply: bool = typer.Option(False, "--apply", help="Write project Antigravity MCP config instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Set up DevCouncil MCP tools for Google Antigravity CLI.
    """
    root = _project_root(project_root)
    if apply:
        report = apply_integration_target(root, "antigravity")
        if not report.ok:
            console.print(report.to_json())
            raise typer.Exit(code=1)
        console.print("[green]Antigravity integration configured.[/green]")
        _warn_if_verify_only("antigravity")
        return
    ok = _configure_antigravity(root, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("warp")
def warp(
    apply: bool = typer.Option(False, "--apply", help="Write Warp MCP config instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Set up DevCouncil MCP tools for Warp local agents and the Oz CLI.
    """
    root = _project_root(project_root)
    if apply:
        report = apply_integration_target(root, "warp")
        if not report.ok:
            console.print(report.to_json())
            raise typer.Exit(code=1)
        console.print("[green]Warp integration configured.[/green]")
        _warn_if_verify_only("warp")
        return
    _configure_warp(root, apply)


def _record_aider_config(project_root: Path) -> None:
    def mutate(config: dict) -> None:
        config.setdefault("integrations", {}).setdefault("aider", {}).update({"enabled": True})

    _mutate_raw_config(project_root, mutate)


def _configure_aider(project_root: Path, apply: bool) -> bool:
    command = ["aider", "--yes", "--no-show-model-warnings", "--message", "<task prompt>"]
    if not apply:
        console.print("[bold]Aider[/bold]")
        console.print("Built-in executor: [dim]dev run TASK-001 --executor aider[/dim]")
        console.print("Launch command: [dim]" + _format_command(command) + "[/dim]")
        console.print("Aider does not expose a first-party DevCouncil MCP server.")
        return True

    if not shutil.which("aider"):
        console.print("[yellow]Aider CLI not found on PATH. Install it before using `dev run --executor aider`.[/yellow]")
    _record_aider_config(project_root)
    console.print("[green]Aider executor enabled in .devcouncil/config.yaml.[/green]")
    return True


@app.command("aider")
def aider(
    apply: bool = typer.Option(False, "--apply", help="Record the built-in Aider executor in DevCouncil config."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Enable the built-in Aider headless executor (no MCP integration).
    """
    root = _project_root(project_root)
    if apply:
        report = apply_integration_target(root, "aider")
        if not report.ok:
            console.print(report.to_json())
            raise typer.Exit(code=1)
        console.print("[green]Aider integration configured.[/green]")
        _warn_if_verify_only("aider")
        return
    ok = _configure_aider(root, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("cli-agent")
def cli_agent(
    name: str = typer.Argument(..., help="Executor name to register, for example opencode or aider."),
    command: str = typer.Option(..., "--command", help="Executable to launch."),
    arg: list[str] | None = typer.Option(None, "--arg", help="Argument to pass to the CLI. Repeat for multiple args."),
    input_mode: str = typer.Option("stdin", "--input-mode", help="Prompt input mode: stdin, argument, or prompt-file."),
    prompt_arg: str | None = typer.Option(None, "--prompt-arg", help="Flag used before the prompt or prompt file, for example --prompt."),
    timeout_seconds: int | None = typer.Option(None, "--timeout-seconds", help="Agent-specific timeout override."),
    display_name: str | None = typer.Option(None, "--display-name", help="Human-readable agent name."),
    kind: str = typer.Option("custom", "--kind", help="Agent kind, for example coding-cli or review-cli."),
    supports_mcp: bool = typer.Option(False, "--supports-mcp", help="Mark this agent as MCP-capable."),
    supports_diff_review: bool = typer.Option(False, "--supports-diff-review", help="Mark this agent as able to review diffs."),
    default_profile: str = typer.Option("default", "--default-profile", help="Default execution profile for this agent."),
    help_arg: list[str] | None = typer.Option(None, "--help-arg", help="Argument for the agent help command. Repeat for multiple args."),
    apply: bool = typer.Option(False, "--apply", help="Write .devcouncil/config.yaml instead of previewing."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Register an arbitrary prompt-taking CLI as a DevCouncil executor.
    """
    if input_mode not in VALID_INPUT_MODES:
        console.print("[red]--input-mode must be one of: stdin, argument, prompt-file.[/red]")
        raise typer.Exit(code=2)
    if not name.strip():
        console.print("[red]Agent name cannot be empty.[/red]")
        raise typer.Exit(code=2)
    if not command.strip():
        console.print("[red]--command cannot be empty.[/red]")
        raise typer.Exit(code=2)

    root = _project_root(project_root)
    if is_reserved_agent_name(name):
        console.print(f"[red]'{name}' is reserved for a built-in DevCouncil agent.[/red]")
        raise typer.Exit(code=2)
    if default_profile not in load_agent_profiles(root):
        console.print(f"[red]Unknown --default-profile '{default_profile}'.[/red]")
        raise typer.Exit(code=2)

    normalized = normalize_agent_name(name)
    entry = agent_config_entry(
        command=command,
        args=arg or [],
        input_mode=input_mode,
        prompt_arg=prompt_arg,
        timeout_seconds=timeout_seconds,
        display_name=display_name,
        kind=kind,
        supports_mcp=supports_mcp,
        supports_diff_review=supports_diff_review,
        default_profile=default_profile,
        help_command=[command, *(help_arg or [])] if help_arg else [],
    )

    if not apply:
        console.print("[bold]Bring your own CLI executor preview[/bold]")
        console.print(f"Executor: [cyan]{normalized}[/cyan]")
        console.print(json.dumps(entry, indent=2), soft_wrap=True)
        console.print(f"Run with: [dim]dev run TASK-001 --executor {normalized}[/dim]")
        console.print("[yellow]Preview only. Rerun with --apply to update .devcouncil/config.yaml.[/yellow]")
        return

    config = _load_raw_config(root)
    agents = config.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})
    agents[normalized] = entry
    _save_raw_config(root, config)
    console.print(f"[green]Registered CLI executor '{normalized}' in .devcouncil/config.yaml.[/green]")


@app.command("all")
def all_tools(
    apply: bool = typer.Option(False, "--apply", help="Run setup commands instead of printing them."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    gemini_scope: str = typer.Option("project", "--gemini-scope", help="Gemini MCP config scope: project or user."),
    claude_scope: str = typer.Option("local", "--claude-scope", help="Claude MCP config scope: local, project, or user."),
    hooks: bool = typer.Option(True, "--hooks/--no-hooks", help="Include native Codex, Gemini, and Claude hook setup."),
    write_gate: bool = typer.Option(
        False,
        "--write-gate/--no-write-gate",
        "--contain/--no-contain",
        help="Install Claude's blocking write-gate too (off by default; for autonomous executor runs).",
    ),
    strict: bool = typer.Option(
        False,
        "--strict",
        help="After --apply, run dev integrate check --strict and fail on missing optional CLIs.",
    ),
):
    """
    Set up DevCouncil MCP tools and native hooks for every supported coding CLI found on PATH.
    """
    if gemini_scope not in {"project", "user"}:
        console.print("[red]--gemini-scope must be 'project' or 'user'.[/red]")
        raise typer.Exit(code=2)
    if claude_scope not in {"local", "project", "user"}:
        console.print("[red]--claude-scope must be 'local', 'project', or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = _project_root(project_root)
    if apply:
        report = apply_integration_target(
            root,
            "all",
            include_hooks=hooks,
            strict=strict,
            gemini_scope=gemini_scope,
            claude_scope=claude_scope,
            claude_write_gate=write_gate,
        )
        if not report.ok:
            console.print(report.to_json())
            raise typer.Exit(code=1)
        console.print("[green]Coding CLI integrations configured.[/green]")
        return

    commands = [
        ("Codex CLI", _codex_command(root)),
        ("Gemini CLI", _gemini_command(root, gemini_scope)),
        ("Claude Code", _claude_command(root, claude_scope)),
    ]
    for tool, command in commands:
        _configure(tool, command, apply)
    _configure_cursor(root, apply)
    _configure_opencode(root, apply)
    _configure_antigravity(root, apply)
    _configure_warp(root, apply)
    _configure_aider(root, apply)
    if hooks:
        _configure_native_hooks(root, "all", apply)


@app.command("recommend")
def recommend(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Recommend a coding CLI executor for this machine and project."""
    root = _project_root(project_root)
    probe_order = resolve_coding_cli_probe_order(root)
    detected = detect_available_coding_cli(root, probe_order=probe_order)
    resolved = resolve_automated_executor(root, None)

    table = Table(title="DevCouncil Integration Recommendations")
    table.add_column("Client", style="cyan")
    table.add_column("PATH")
    table.add_column("Tier")
    table.add_column("MCP")
    table.add_column("Hooks")

    for client in probe_order:
        info = CODING_CLI_INTEGRATION_INFO.get(client)
        on_path = resolve_coding_cli_executable(root, client)
        table.add_row(
            client,
            "[green]yes[/green]" if on_path else "[dim]no[/dim]",
            integration_tier_label(client),
            "yes" if info and info.mcp else "no",
            "yes" if info and info.hooks else "no",
        )

    console.print(table)
    if summary := integration_status_summary(root):
        if summary.get("custom_probe_order"):
            console.print(
                f"\n[dim]Probe order:[/dim] {', '.join(summary['probe_order'])} "
                f"(from execution.coding_cli_probe_order)"
            )
        else:
            console.print(f"\n[dim]Probe order:[/dim] {', '.join(summary['probe_order'])} (default)")
    if detected:
        console.print(f"\n[bold]Recommended executor:[/bold] [cyan]{resolved}[/cyan]")
        console.print(f"Run: [dim]dev run TASK-001 --executor {resolved}[/dim]")
        console.print(f"Or:  [dim]dev go \"Your goal\" --executor {resolved}[/dim]")
        console.print(f"Setup: [dim]{PREFERRED_COMMAND} {resolved} --apply[/dim]")
    else:
        console.print("\n[yellow]No built-in coding CLI was found on PATH.[/yellow]")
        console.print("Install Codex, Gemini, Claude Code, Cursor Agent, OpenCode, or register a custom CLI:")
        console.print(f"[dim]{PREFERRED_COMMAND} cli-agent NAME --command TOOL --apply[/dim]")


@app.command("status")
def status(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    as_json: bool = typer.Option(False, "--json", help="Emit machine-readable JSON."),
):
    """Show a compact integration summary without running the MCP server probe."""
    root = _project_root(project_root)
    summary = integration_status_summary(root)
    raw_config = _load_raw_config(root) if (root / ".devcouncil").exists() else {}
    integrations = raw_config.get("integrations", {})

    if as_json:
        payload = {
            **summary,
            "integrations_enabled": {
                name: bool(integrations.get(name, {}).get("enabled"))
                for name in ("cursor", "opencode", "antigravity", "warp", "aider")
            },
        }
        typer.echo(json.dumps(payload, indent=2))
        return

    table = Table(title="DevCouncil Integration Status")
    table.add_column("Setting", style="cyan")
    table.add_column("Value")

    table.add_row("Project", "[green]initialized[/green]" if summary["project_initialized"] else "[yellow]not initialized[/yellow]")
    table.add_row("Default executor", summary["default_executor"])
    table.add_row("Resolved executor", summary["resolved_executor"])
    table.add_row("CLIs on PATH", ", ".join(summary["coding_clis_on_path"]) or "[dim]none[/dim]")
    table.add_row("Probe order", ", ".join(summary["probe_order"]))
    table.add_row("Stream CLI output", "yes" if summary["stream_cli_output"] else "no")
    table.add_row("Cursor resume mode", summary["cursor_resume_mode"])

    for name in ("cursor", "opencode", "antigravity", "warp", "aider"):
        enabled = bool(integrations.get(name, {}).get("enabled"))
        table.add_row(f"{name} integration", "[green]enabled[/green]" if enabled else "[dim]off[/dim]")

    console.print(table)
    if summary["resolved_executor"] not in {"", "manual"}:
        console.print(
            f"\n[dim]Next:[/dim] dev run TASK-001 --executor {summary['resolved_executor']} "
            f"| {PREFERRED_COMMAND} check for full readiness"
        )
    else:
        console.print(f"\n[dim]Next:[/dim] {PREFERRED_COMMAND} recommend | {PREFERRED_COMMAND} check")


@app.command("matrix")
def matrix(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Print built-in coding CLI integration tiers and capabilities."""
    root = _project_root(project_root)
    _ = root
    table = Table(title="DevCouncil Coding CLI Integration Matrix")
    table.add_column("Client", style="cyan")
    table.add_column("Tier")
    table.add_column("Headless")
    table.add_column("MCP setup")
    table.add_column("Native hooks")
    table.add_column("Enforcement")
    table.add_column("Notes")

    for client in sorted(BUILTIN_CODING_EXECUTOR_NAMES):
        info = CODING_CLI_INTEGRATION_INFO.get(client)
        posture = info.enforcement if info else "verify-only"
        posture_render = "[green]pre-action[/green]" if posture == "pre-action" else "[yellow]verify-only[/yellow]"
        table.add_row(
            client,
            integration_tier_label(client),
            "yes" if info and info.tier == 1 else "no",
            "yes" if info and info.mcp else "no",
            "yes" if info and info.hooks else "verify only",
            posture_render,
            info.notes if info else "",
        )
    console.print(table)
    console.print(
        "\n[dim]Enforcement:[/dim] [green]pre-action[/green] blocks forbidden writes/commands "
        "before they happen; [yellow]verify-only[/yellow] catches them only at verify time."
    )
    console.print("\nSee [dim]docs/integration-tiers.md[/dim] for workflow guidance.")


@app.command("hooks")
def hooks(
    apply: bool = typer.Option(False, "--apply", help="Write native hook config files instead of previewing paths."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    tool: str = typer.Option("all", "--tool", help="Hook target: all, codex, gemini, claude, cursor, or opencode."),
    write_gate: bool = typer.Option(
        False,
        "--write-gate/--no-write-gate",
        "--contain/--no-contain",
        help="Install Claude's blocking PreToolUse/PostToolUse write-gate too (off by default; "
        "fail-closes an interactive session without a task lease).",
    ),
):
    """
    Install DevCouncil hook configuration for Codex, Gemini, Claude, Cursor, and OpenCode.

    Claude installs only assistive hooks by default; add --write-gate for pre-action
    containment (intended for autonomous executor runs).
    """
    root = _project_root(project_root)
    if apply and tool == "all":
        report = apply_integration_target(root, "hooks", claude_write_gate=write_gate)
        if not report.ok:
            console.print(report.to_json())
            raise typer.Exit(code=1)
        console.print("[green]Native hooks configured.[/green]")
        return
    _configure_native_hooks(root, tool, apply, claude_write_gate=write_gate)


@app.command("uninstall")
def uninstall(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    target: str = typer.Option("claude", "--target", help="What to uninstall. Currently: claude."),
):
    """
    Remove a DevCouncil integration. Reverses `dev integrate claude` — hooks, statusline,
    MCP enablement, permission rules, and the generated commands/subagents/output style.
    """
    root = _project_root(project_root)
    if target != "claude":
        console.print("[red]--target must be 'claude'.[/red]")
        raise typer.Exit(code=2)
    removed = _uninstall_claude(root)
    if removed:
        console.print(f"[green]Removed DevCouncil Claude integration[/green] ({len(removed)} change(s)):")
        for item in removed:
            console.print(f"  {item}")
    else:
        console.print("[dim]Nothing to remove — DevCouncil Claude integration not found.[/dim]")


@app.command("check")
def check(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    strict: bool = typer.Option(
        False,
        "--strict",
        help="Treat missing optional coding CLIs as failures instead of warnings.",
    ),
    as_json: bool = typer.Option(False, "--json", help="Emit machine-readable JSON for CI."),
    report_file: Path | None = typer.Option(
        None,
        "--report-file",
        "--output",
        "-o",
        help="Write the JSON integration report to this file (implies structured output).",
    ),
):
    """
    Check whether DevCouncil is ready to integrate with coding CLIs.
    """
    root = _project_root(project_root)
    report = build_integration_check_report(root, strict=strict)
    table = Table(title="DevCouncil Integration Check")
    table.add_column("Check", style="cyan")
    table.add_column("Status", style="magenta")
    table.add_column("Details")

    for row in report.checks:
        if row.status == "ok":
            rendered = "[green]OK[/green]"
        elif row.status == "skip":
            rendered = "[dim]SKIP[/dim]"
        elif row.status == "missing":
            rendered = "[yellow]Missing[/yellow]"
        else:
            rendered = "[red]FAIL[/red]"
        table.add_row(row.name, rendered, row.details)

    write_json = as_json or report_file is not None
    if write_json:
        json_text = report.to_json()
        if report_file is not None:
            report_path = Path(report_file).expanduser().resolve()
            report_path.parent.mkdir(parents=True, exist_ok=True)
            report_path.write_text(json_text + "\n", encoding="utf-8")
            if not as_json:
                console.print(f"[dim]Wrote integration report to[/dim] {report_path}")
        if as_json:
            typer.echo(json_text)
    if not write_json or not as_json:
        console.print(table)

    if report.failures:
        if not as_json:
            console.print(
                f"\n[yellow]Fix failed checks, then run:[/yellow] {PREFERRED_COMMAND} all --apply "
                f"(or {LEGACY_COMMAND} --apply)."
            )
        raise typer.Exit(code=1)

    if not as_json:
        console.print(f"\n[green]Ready.[/green] Run: {PREFERRED_COMMAND} all --apply (or {LEGACY_COMMAND} --apply).")


@setup_app.command("agent-flow")
def setup_agent_flow(
    apply: bool = typer.Option(False, "--apply", help="Write DevCouncil config instead of previewing."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Configure DevCouncil trace output for Agent Flow-style JSONL replay."""
    root = _project_root(project_root)
    trace_path = root / ".devcouncil" / "logs" / "traces.jsonl"
    console.print("[bold]Agent Flow setup[/bold]")
    console.print(f"Trace JSONL: {trace_path}")
    console.print("Replay/tail locally with: dev trace tail --follow")
    console.print("External visualizers can watch the trace JSONL path above.")

    if not apply:
        console.print("[yellow]Preview only. Rerun with --apply to record this integration in config.[/yellow]")
        return

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
    (docs_dir / "agent-flow.md").write_text(
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
    console.print("[green]Agent Flow trace integration recorded in .devcouncil/config.yaml.[/green]")


@setup_app.command("code-review-graph")
def setup_code_review_graph(
    apply: bool = typer.Option(False, "--apply", help="Write DevCouncil config and ignore file."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Configure optional code-review-graph context enrichment."""
    root = _project_root(project_root)
    executable = shutil.which("code-review-graph")
    ignore_path = root / ".code-review-graphignore"
    console.print("[bold]code-review-graph setup[/bold]")
    console.print(f"Binary: {executable or 'not found on PATH'}")
    console.print("Install separately with: pipx install code-review-graph")
    console.print("Build graph separately with: code-review-graph build")

    if not apply:
        console.print("[yellow]Preview only. Rerun with --apply to record this integration.[/yellow]")
        return

    if not ignore_path.exists():
        ignore_path.write_text(
            "\n".join([
                ".devcouncil/**",
                ".git/**",
                ".venv/**",
                "dist/**",
                "node_modules/**",
                "",
            ]),
            encoding="utf-8",
        )
        console.print(f"[green]Created {ignore_path}.[/green]")

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
    console.print("[green]code-review-graph adapter recorded in .devcouncil/config.yaml.[/green]")
