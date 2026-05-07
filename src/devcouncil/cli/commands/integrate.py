import json
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

import typer
import yaml  # type: ignore[import-untyped]
from rich.console import Console
from rich.table import Table

from devcouncil.executors.agent_registry import (
    VALID_INPUT_MODES,
    agent_config_entry,
    is_reserved_agent_name,
    load_agent_profiles,
    load_cli_agent_specs,
    normalize_agent_name,
)

app = typer.Typer(help="Set up DevCouncil integrations with coding CLIs.")
setup_app = typer.Typer(help="Set up optional external companion integrations.")
app.add_typer(setup_app, name="setup")
console = Console()

SUPPORTED_TOOLS = ("codex", "gemini", "claude", "cursor", "warp")
SUPPORTED_HOOK_TOOLS = ("codex", "gemini", "claude")
PREFERRED_COMMAND = "dev integrate"
LEGACY_COMMAND = "dev setup --integrate"


def _project_root(path: Path | None) -> Path:
    return (path or Path(".")).expanduser().resolve()


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
    return [
        "claude",
        "mcp",
        "add",
        "--scope",
        scope,
        "--env",
        f"DEVCOUNCIL_PROJECT_ROOT={project_root}",
        "devcouncil",
        "--",
        *_server_args(project_root),
    ]


def _cursor_command(project_root: Path) -> list[str]:
    server = {
        "name": "devcouncil",
        "command": "devcouncil",
        "args": ["mcp-server"],
        "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
    }
    return ["cursor", "--add-mcp", json.dumps(server, separators=(",", ":"))]


def _warp_mcp_config(project_root: Path) -> dict:
    return {
        "mcpServers": {
            "devcouncil": {
                "command": "devcouncil",
                "args": ["mcp-server"],
                "env": {"DEVCOUNCIL_PROJECT_ROOT": str(project_root)},
                "working_directory": str(project_root),
            }
        }
    }


def _warp_mcp_path(project_root: Path) -> Path:
    return project_root / ".devcouncil" / "integrations" / "warp-mcp.json"


def _write_warp_mcp_config(project_root: Path) -> Path:
    path = _warp_mcp_path(project_root)
    _save_json(path, _warp_mcp_config(project_root))
    return path


def _record_warp_config(project_root: Path) -> None:
    config = _load_raw_config(project_root)
    integrations = config.setdefault("integrations", {})
    warp = integrations.setdefault("warp", {})
    warp.update({
        "enabled": True,
        "command": warp.get("command", "oz"),
        "run_mode": warp.get("run_mode", "local"),
        "mcp_config_path": str(_warp_mcp_path(project_root).relative_to(project_root)),
    })
    _save_raw_config(project_root, config)


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


def _run(command: list[str]) -> int:
    executable = shutil.which(command[0])
    if not executable:
        return 127
    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    invocation = subprocess.list2cmdline(resolved) if use_shell else resolved
    result = subprocess.run(invocation, text=True, shell=use_shell)
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
        )
    except subprocess.TimeoutExpired:
        return 124, "timed out"
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


def _install_claude_hooks(project_root: Path) -> list[Path]:
    path = project_root / ".claude" / "settings.local.json"
    settings = _load_json(path)
    matcher = "Bash|Write|Edit|MultiEdit"
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
    _save_json(path, settings)
    return [path]


def _preview_hook_paths(project_root: Path, tool: str) -> list[tuple[str, Path]]:
    paths = {
        "codex": [project_root / ".codex" / "hooks.json", project_root / ".codex" / "config.toml"],
        "gemini": [project_root / ".gemini" / "settings.json"],
        "claude": [project_root / ".claude" / "settings.local.json"],
    }
    selected = SUPPORTED_HOOK_TOOLS if tool == "all" else (tool,)
    return [(client, path) for client in selected for path in paths[client]]


def _configure_native_hooks(project_root: Path, tool: str = "all", apply: bool = False) -> None:
    if tool not in {"all", *SUPPORTED_HOOK_TOOLS}:
        console.print("[red]--tool must be one of: all, codex, gemini, claude.[/red]")
        raise typer.Exit(code=2)

    if not apply:
        console.print("[bold]Native hook config preview[/bold]")
        for client, path in _preview_hook_paths(project_root, tool):
            console.print(f"{client}: {path}")
        console.print("[yellow]Preview only. Rerun with --apply to write hook config files.[/yellow]")
        return

    selected = SUPPORTED_HOOK_TOOLS if tool == "all" else (tool,)
    installers = {
        "codex": _install_codex_hooks,
        "gemini": _install_gemini_hooks,
        "claude": _install_claude_hooks,
    }
    for client in selected:
        written = installers[client](project_root)
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
    table.add_row("Claude Code", f"{PREFERRED_COMMAND} claude --apply", "Adds DevCouncil as a Claude Code MCP server.")
    table.add_row("Cursor", f"{PREFERRED_COMMAND} cursor --apply", "Adds DevCouncil as a Cursor MCP server.")
    table.add_row("Warp / Oz", f"{PREFERRED_COMMAND} warp --apply", "Writes a Warp-compatible MCP JSON file for local agents and Oz CLI.")
    table.add_row("Bring your own CLI", f"{PREFERRED_COMMAND} cli-agent NAME --command TOOL --apply", "Registers any prompt-taking CLI as a DevCouncil executor.")
    table.add_row("All", f"{PREFERRED_COMMAND} all --apply", "Runs MCP setup and installs native hooks.")
    table.add_row("Native hooks", f"{PREFERRED_COMMAND} hooks --apply", "Installs Codex, Gemini, and Claude hook files.")
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
        ("Cursor", "cursor", "Optional MCP client and agent companion."),
        ("Warp / Oz", "oz", "Optional Warp/Oz CLI companion and agent executor."),
        ("Aider", "aider", "Optional prompt/stdin sidecar; no first-party MCP setup command."),
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
):
    """
    Set up DevCouncil MCP tools for Claude Code.
    """
    if scope not in {"local", "project", "user"}:
        console.print("[red]--scope must be 'local', 'project', or 'user'.[/red]")
        raise typer.Exit(code=2)

    root = _project_root(project_root)
    command = _claude_command(root, scope)
    ok = _configure("Claude Code", command, apply)
    if not ok and apply:
        raise typer.Exit(code=1)


@app.command("cursor")
def cursor(
    apply: bool = typer.Option(False, "--apply", help="Run the setup command instead of printing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Set up DevCouncil MCP tools for Cursor.
    """
    root = _project_root(project_root)
    command = _cursor_command(root)
    ok = _configure("Cursor", command, apply)
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
    _configure_warp(root, apply)


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
    commands = [
        ("Codex CLI", _codex_command(root)),
        ("Gemini CLI", _gemini_command(root, gemini_scope)),
        ("Claude Code", _claude_command(root, claude_scope)),
        ("Cursor", _cursor_command(root)),
    ]
    results = []
    for tool, command in commands:
        if apply and not shutil.which(command[0]):
            console.print(f"[yellow]{tool} CLI not found on PATH. Skipping optional integration.[/yellow]")
            continue
        results.append(_configure(tool, command, apply))
    results.append(_configure_warp(root, apply))
    if hooks:
        _configure_native_hooks(root, "all", apply)
    if apply and not all(results):
        raise typer.Exit(code=1)


@app.command("hooks")
def hooks(
    apply: bool = typer.Option(False, "--apply", help="Write native hook config files instead of previewing paths."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    tool: str = typer.Option("all", "--tool", help="Hook target: all, codex, gemini, or claude."),
):
    """
    Install DevCouncil native hook configuration for hook-capable coding CLIs.
    """
    root = _project_root(project_root)
    _configure_native_hooks(root, tool, apply)


@app.command("check")
def check(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Check whether DevCouncil is ready to integrate with coding CLIs.
    """
    root = _project_root(project_root)
    table = Table(title="DevCouncil Integration Check")
    table.add_column("Check", style="cyan")
    table.add_column("Status", style="magenta")
    table.add_column("Details")

    failures = 0

    def add(ok: bool, name: str, details: str):
        nonlocal failures
        table.add_row(name, "[green]OK[/green]" if ok else "[red]FAIL[/red]", details)
        if not ok:
            failures += 1

    def add_optional(ok: bool, name: str, details: str):
        table.add_row(name, "[green]OK[/green]" if ok else "[yellow]Missing[/yellow]", details)

    add((root / ".devcouncil").exists(), "Project state", str(root / ".devcouncil"))

    devcouncil_path = shutil.which("devcouncil")
    add(devcouncil_path is not None, "devcouncil CLI", devcouncil_path or "Install DevCouncil first.")

    code, output = _run_capture(["devcouncil", "--help"])
    add(code == 0, "devcouncil command", output.splitlines()[0] if output else "No output")

    raw_config = _load_raw_config(root)

    code, output = _run_capture(["gemini", "--version"])
    add_optional(code == 0, "Gemini CLI", output.splitlines()[0] if output else "Optional; install Gemini CLI to use this integration.")

    code, output = _run_capture(["codex", "--version"])
    add_optional(code == 0, "Codex CLI", output.splitlines()[0] if output else "Optional; install Codex to use this integration.")

    code, output = _run_capture(["claude", "--version"])
    add_optional(code == 0, "Claude Code", output.splitlines()[0] if output else "Optional; install Claude Code to use this integration.")

    code, output = _run_capture(["cursor", "--version"])
    add_optional(code == 0, "Cursor", output.splitlines()[0] if output else "Optional; install Cursor to use this integration.")

    code, output = _run_capture(["oz", "--version"])
    add_optional(code == 0, "Warp / Oz", output.splitlines()[0] if output else "Optional; install Warp/Oz to use this integration.")

    warp_config = _warp_mcp_path(root)
    warp_enabled = bool(raw_config.get("integrations", {}).get("warp", {}).get("enabled"))
    if warp_enabled:
        add(warp_config.exists(), "Warp MCP config", str(warp_config) if warp_config.exists() else f"Run {PREFERRED_COMMAND} warp --apply.")
    else:
        table.add_row("Warp MCP config", "[dim]SKIP[/dim]", f"Run {PREFERRED_COMMAND} warp --apply to enable.")

    custom_agents = raw_config.get("integrations", {}).get("cli_agents", {}).get("agents", {})
    if custom_agents:
        for name, agent in sorted(custom_agents.items()):
            command = str(agent.get("command", "")).strip()
            found = shutil.which(command) if command else None
            add(found is not None, f"CLI agent: {name}", found or f"{command or 'command'} not found on PATH")
    else:
        table.add_row("Custom CLI agents", "[dim]SKIP[/dim]", "No agents registered.")

    try:
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client

        async def _list_tools() -> list[str]:
            import os

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

        import asyncio

        tools = asyncio.run(_list_tools())
        expected = {"devcouncil_status", "devcouncil_report", "devcouncil_get_task"}
        add(expected.issubset(set(tools)), "MCP server", ", ".join(tools))
    except Exception as exc:
        add(False, "MCP server", str(exc))

    console.print(table)
    if failures:
        console.print(
            f"\n[yellow]Fix failed checks, then run:[/yellow] {PREFERRED_COMMAND} all --apply "
            f"(or {LEGACY_COMMAND} --apply)."
        )
        raise typer.Exit(code=1)

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
