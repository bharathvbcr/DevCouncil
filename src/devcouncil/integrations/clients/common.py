
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


def seed_hook_gate_for_write_gate(config: dict, *, write_gate: bool) -> None:
    """Align ``execution.hook_gate.mode`` with assist vs containment install.

    Assist (``write_gate=False``) forces ``off`` so interactive Shell/Write is not
    fail-closed even if PreToolUse was left installed. Containment (``write_gate=True``)
    forces ``contain`` so PreToolUse actually requires a leased task.
    """
    execution = config.setdefault("execution", {})
    hook_gate = execution.setdefault("hook_gate", {})
    if not isinstance(hook_gate, dict):
        hook_gate = {}
        execution["hook_gate"] = hook_gate
    # Quote-safe string; yaml.safe_dump will emit a quoted "off" when needed.
    hook_gate["mode"] = "contain" if write_gate else "off"

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


def resolve_devcouncil_executable(project_root: Path) -> str:
    """Resolve the ``devcouncil`` stdio MCP entry point, absolute when one exists.

    A stdio MCP server is spawned by the *host* process with the host's own PATH, which
    routinely lacks the project venv — so a bare ``devcouncil`` either fails to start or
    silently binds a different DevCouncil install. Built on :func:`resolve_dev_executable`
    (same venv-first search) and shared by every generator that emits an MCP ``command``,
    so there is one resolution rule rather than one per host. Falls back to the bare PATH
    name when no local binary exists; status checks accept both.
    """
    resolved = Path(resolve_dev_executable(project_root))
    name = resolved.name.lower()
    if name in {"dev", "dev.exe"}:
        sibling = resolved.with_name("devcouncil.exe" if name.endswith(".exe") else "devcouncil")
        if sibling.is_file():
            return str(sibling)
    if name in {"devcouncil", "devcouncil.exe"}:
        return str(resolved)
    # Fall back to PATH name when no local binary exists.
    return "devcouncil"


def venv_bin_dir(project_root: Path) -> Path | None:
    """The project venv's executable directory, or None when the repo has no venv."""
    root = project_root.expanduser().resolve()
    venv_bin = root / ".venv" / ("Scripts" if os.name == "nt" else "bin")
    return venv_bin if venv_bin.is_dir() else None


def venv_augmented_path(project_root: Path) -> str:
    """``PATH`` for a spawned MCP server: the project venv's bin dir, then the current PATH.

    Belt-and-braces alongside :func:`resolve_devcouncil_executable` — the absolute command
    starts the right binary, and this makes any child process it spawns (``dev``, ``git``
    wrappers) resolve out of the same venv.
    """
    existing_path = os.environ.get("PATH", "/usr/bin:/bin")
    venv_bin = venv_bin_dir(project_root)
    if venv_bin is not None:
        return f"{venv_bin}{os.pathsep}{existing_path}"
    return existing_path


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


# Clients that record integrations.<name>.enabled on apply.
CLIENTS_WITH_ENABLED = frozenset({
    "claude", "cursor", "grok", "opencode", "antigravity", "warp", "aider",
})
# Clients that may record integrations.<name>.write_gate.
CLIENTS_WITH_WRITE_GATE = frozenset({
    "claude", "cursor", "grok", "opencode", "codex", "gemini",
})


def _clear_client_integration_config(project_root: Path, client: str) -> list[str]:
    """Clear enablement / write_gate for a client after uninstall. Idempotent."""
    changed: list[str] = []

    def mutate(config: dict) -> None:
        integrations = config.get("integrations")
        if not isinstance(integrations, dict):
            return
        entry = integrations.get(client)
        if not isinstance(entry, dict):
            return
        if client in CLIENTS_WITH_ENABLED or "enabled" in entry:
            if entry.get("enabled") is not False:
                entry["enabled"] = False
                changed.append(f"integrations.{client}.enabled=false")
        if client in CLIENTS_WITH_WRITE_GATE or "write_gate" in entry:
            if entry.get("write_gate") is not False:
                entry["write_gate"] = False
                changed.append(f"integrations.{client}.write_gate=false")

    _mutate_raw_config(project_root, mutate)
    return changed


def _apply_decouple_config_flags(project_root: Path, client: str | None = None) -> list[str]:
    """Force assist posture after containment strip: write_gate off, hook_gate off, stop_gate block→assist."""
    changed: list[str] = []

    def mutate(config: dict) -> None:
        if client:
            entry = config.setdefault("integrations", {}).setdefault(client, {})
            if isinstance(entry, dict) and entry.get("write_gate") is not False:
                entry["write_gate"] = False
                changed.append(f"integrations.{client}.write_gate=false")
        execution = config.setdefault("execution", {})
        hook_gate = execution.setdefault("hook_gate", {})
        if not isinstance(hook_gate, dict):
            hook_gate = {}
            execution["hook_gate"] = hook_gate
        if hook_gate.get("mode") != "off":
            hook_gate["mode"] = "off"
            changed.append("execution.hook_gate.mode=off")
        stop_gate = execution.setdefault("stop_gate", {})
        if not isinstance(stop_gate, dict):
            stop_gate = {}
            execution["stop_gate"] = stop_gate
        mode = str(stop_gate.get("mode") or "").strip().lower()
        if mode == "block":
            stop_gate["mode"] = "assist"
            changed.append("execution.stop_gate.mode=assist")

    _mutate_raw_config(project_root, mutate)
    return changed


def _remove_unmodified_library_skills(
    project_root: Path,
    destinations: tuple[str, ...] | list[str] | None = None,
) -> list[str]:
    """Delete scaffolded library skill dirs whose SKILL.md still matches packaged content."""
    from devcouncil.skills.registry import (
        DEFAULT_SKILL_DESTINATIONS,
        LIBRARY_DIR,
        load_skills,
    )

    dest_rels = tuple(destinations) if destinations is not None else DEFAULT_SKILL_DESTINATIONS
    # Packaged library only — never use repo-local overrides as the ownership baseline.
    library = {
        skill.name: skill
        for skill in load_skills(LIBRARY_DIR, project_root=None, include_okf=False)
    }
    removed: list[str] = []
    root = project_root.expanduser().resolve()
    for rel in dest_rels:
        skills_root = root / rel
        if not skills_root.is_dir():
            continue
        for child in sorted(skills_root.iterdir()):
            if not child.is_dir() or child.name not in library:
                continue
            skill_md = child / "SKILL.md"
            if not skill_md.is_file():
                continue
            try:
                on_disk = skill_md.read_text(encoding="utf-8")
            except OSError:
                continue
            if on_disk != library[child.name].to_skill_md():
                continue
            shutil.rmtree(child)
            try:
                removed.append(str(child.relative_to(root)))
            except ValueError:
                removed.append(str(child))
    return removed


def _remove_toml_table(text: str, header: str) -> str:
    """Remove a TOML table section (header + body until the next table)."""
    target = header.strip()
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    skipping = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            skipping = stripped == target
            if skipping:
                continue
        if skipping:
            continue
        out.append(line)
    return "".join(out)
