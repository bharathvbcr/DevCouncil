
"""Shared integration utilities."""
from __future__ import annotations

import json
import logging
import os
import shlex
import shutil
import subprocess
import sys
from contextlib import contextmanager
from pathlib import Path

import yaml
from rich.console import Console

from devcouncil.executors.agent_registry import (
    CODING_CLI_INTEGRATION_INFO,
    normalize_agent_name,
)
from devcouncil.utils.subprocess_env import clean_subprocess_env

console = Console()
logger = logging.getLogger(__name__)

OPENCODE_HOOK_PLUGIN_NAME = "opencode_devcouncil_plugin.mjs"
SUPPORTED_HOOK_TOOLS = ("claude", "codex", "cursor", "grok")
# Clients whose native hook installers wire Stop/SubagentStop handlers.
STOP_HOOK_TOOLS = frozenset({"claude", "codex"})


def seed_stop_gate_assist_if_unset(config: dict) -> None:
    """Default ``execution.stop_gate.mode`` to ``assist`` when hooks are installed.

    Code default remains ``off`` (``StopGateConfig.mode``); integrate/first-run setup
    for Stop-hook clients opts into assistive warnings without blocking completion.
    """
    execution = config.setdefault("execution", {})
    stop_gate = execution.setdefault("stop_gate", {})
    if not isinstance(stop_gate, dict):
        stop_gate = {}
        execution["stop_gate"] = stop_gate
    if not stop_gate.get("mode"):
        stop_gate["mode"] = "assist"
    stop_gate.setdefault("check_claims", True)
    stop_gate.setdefault("verify_active_task", True)

# Recorded when hooks are installed so `dev integrate hooks --check` can detect drift
# between a stale global CLI and the project venv.
HOOK_DEV_EXECUTABLE_REL = Path(".devcouncil") / "cache" / "hook_dev_executable"


def resolve_dev_executable(project_root: Path) -> str:
    """Resolve the ``dev`` CLI to invoke from hooks (project venv first, then PATH).

    Preferring the project ``.venv`` avoids a globally installed stale ``dev`` silently
    fighting the working tree over cache versions and map schema.
    """
    root = project_root.expanduser().resolve()
    candidates: list[Path] = []
    if sys.platform == "win32":
        candidates.extend(
            [
                root / ".venv" / "Scripts" / "dev.exe",
                root / ".venv" / "Scripts" / "devcouncil.exe",
                root / "venv" / "Scripts" / "dev.exe",
            ]
        )
    else:
        candidates.extend(
            [
                root / ".venv" / "bin" / "dev",
                root / ".venv" / "bin" / "devcouncil",
                root / "venv" / "bin" / "dev",
                root / "venv" / "bin" / "devcouncil",
            ]
        )
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate.resolve())
    for name in ("dev", "devcouncil"):
        found = shutil.which(name)
        if found:
            return found
    return "dev"


def record_hook_dev_executable(project_root: Path, executable: str | None = None) -> Path:
    """Persist the resolved ``dev`` path used in installed hook commands."""
    root = project_root.expanduser().resolve()
    path = root / HOOK_DEV_EXECUTABLE_REL
    path.parent.mkdir(parents=True, exist_ok=True)
    resolved = executable or resolve_dev_executable(root)
    path.write_text(resolved + "\n", encoding="utf-8")
    return path


def recorded_hook_dev_executable(project_root: Path) -> str | None:
    path = project_root.expanduser().resolve() / HOOK_DEV_EXECUTABLE_REL
    if not path.is_file():
        return None
    text = path.read_text(encoding="utf-8").strip()
    return text or None


def check_hook_dev_executable(project_root: Path) -> tuple[bool, str]:
    """Compare recorded hook executable against the currently resolved one."""
    root = project_root.expanduser().resolve()
    current = resolve_dev_executable(root)
    recorded = recorded_hook_dev_executable(root)
    if recorded is None:
        return True, f"No recorded hook executable (current: {current})"
    if Path(recorded).resolve() == Path(current).resolve() or recorded == current:
        return True, f"Hook executable matches: {current}"
    return False, f"Hook executable mismatch: recorded={recorded} current={current}"


def _project_root(path: str | Path | None) -> Path:
    root = Path(path or ".").expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir

    set_log_dir(root)
    return root

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
    """Return the exact MCP server command clients should persist.

    Project integrations must execute the checkout they were generated from.  A
    bare ``devcouncil`` command can resolve to an older global installation and
    silently serve a different schema than the working tree.
    """
    return [resolve_dev_executable(project_root), "mcp-server"]

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

def _load_json_strict(path: Path, label: str = "JSON") -> dict:
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8")) or {}
    except json.JSONDecodeError as exc:
        raise ValueError(f"{path} is not valid JSON. Fix the {label} config before rerunning integration setup.") from exc

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

def _probe_mcp_tools(root: Path, *, timeout_seconds: float = 30.0) -> list[str]:
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    import asyncio
    import os

    async def _list_tools() -> list[str]:
        env = os.environ.copy()
        env["DEVCOUNCIL_PROJECT_ROOT"] = str(root)
        command = _server_args(root)
        params = StdioServerParameters(
            command=command[0],
            args=command[1:],
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

_CLI_RUN_TIMEOUT_SECONDS = 120

def _run(command: list[str]) -> int:
    executable = shutil.which(command[0])
    if not executable:
        return 127
    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    invocation = subprocess.list2cmdline(resolved) if use_shell else resolved
    try:
        result = subprocess.run(
            invocation,
            text=True,
            shell=use_shell,
            timeout=_CLI_RUN_TIMEOUT_SECONDS,
            env=clean_subprocess_env(),
        )
    except subprocess.TimeoutExpired:
        return 124
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
    from devcouncil.utils.json_persist import write_json

    write_json(path, data)

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
