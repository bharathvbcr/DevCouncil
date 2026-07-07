from devcouncil.utils.json_persist import dump_json
import logging
import shutil
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table
from devcouncil.telemetry.stages import log_stage, log_step

from devcouncil.executors.agent_registry import (
    VALID_INPUT_MODES,
    agent_config_entry,
    is_reserved_agent_name,
    load_agent_profiles,
    normalize_agent_name,
)
from devcouncil.integrations.actions import apply_integration_target
from devcouncil.integrations.integration_cli import (
    print_integration_matrix,
    print_integration_status,
    print_recommendations,
    run_integration_check,
)
from devcouncil.integrations.setup import (
    apply_agent_flow_setup,
    apply_code_review_graph_setup,
    build_integrations_doctor_table,
)
from devcouncil.integrations.clients import (
    antigravity as antigravity_client,
    aider as aider_client,
    claude as claude_client,
    codex as codex_client,
    common,
    cursor as cursor_client,
    gemini as gemini_client,
    hooks as hooks_client,
    opencode as opencode_client,
    warp as warp_client,
)

app = typer.Typer(help="Set up DevCouncil integrations with coding CLIs.")
setup_app = typer.Typer(help="Set up optional external companion integrations.")
app.add_typer(setup_app, name="setup")
console = Console()
logger = logging.getLogger(__name__)

SUPPORTED_TOOLS = ("codex", "gemini", "claude", "cursor", "opencode", "antigravity", "warp", "aider")
SUPPORTED_HOOK_TOOLS = common.SUPPORTED_HOOK_TOOLS
OPENCODE_HOOK_PLUGIN_NAME = common.OPENCODE_HOOK_PLUGIN_NAME
PREFERRED_COMMAND = "dev integrate"
LEGACY_COMMAND = "dev setup --integrate"

# Back-compat re-exports for tests and apply_integration_target
_project_root = common._project_root
_warn_if_verify_only = common._warn_if_verify_only
_server_args = common._server_args
_codex_command = codex_client._codex_command
_gemini_command = gemini_client._gemini_command
_claude_command = claude_client._claude_command
_cursor_config_path = cursor_client._cursor_config_path
_configure_cursor = cursor_client._configure_cursor
_configure_opencode = opencode_client._configure_opencode
_configure_antigravity = antigravity_client._configure_antigravity
_configure_warp = warp_client._configure_warp
_configure_aider = aider_client._configure_aider
_write_cursor_config = cursor_client._write_cursor_config
_write_opencode_config = opencode_client._write_opencode_config
_write_antigravity_mcp_config = antigravity_client._write_antigravity_mcp_config
_write_warp_mcp_config = warp_client._write_warp_mcp_config
_record_cursor_config = cursor_client._record_cursor_config
_record_claude_config = claude_client._record_claude_config
_record_opencode_config = opencode_client._record_opencode_config
_record_antigravity_config = antigravity_client._record_antigravity_config
_record_warp_config = warp_client._record_warp_config
_record_aider_config = aider_client._record_aider_config
_batched_raw_config = common._batched_raw_config
_mutate_raw_config = common._mutate_raw_config
_load_raw_config = common._load_raw_config
_save_raw_config = common._save_raw_config
_config_path = common._config_path
_load_json = common._load_json
_save_json = common._save_json
_format_command = common._format_command
_configure = common._configure
_run = common._run
_run_capture = common._run_capture
_probe_mcp_tools = common._probe_mcp_tools
_cursor_mcp_config = cursor_client._cursor_mcp_config
_opencode_plugin_source = opencode_client._opencode_plugin_source
_install_claude_hooks = hooks_client._install_claude_hooks
_install_claude_assets = claude_client._install_claude_assets
_install_claude_plugin = claude_client._install_claude_plugin
_uninstall_claude = claude_client._uninstall_claude
_configure_native_hooks = hooks_client._configure_native_hooks
_opencode_config_path = opencode_client._opencode_config_path
_opencode_plugin_path = opencode_client._opencode_plugin_path


@app.callback(invoke_without_command=True)
def overview(ctx: typer.Context):
    """
    Show integration options for supported coding CLIs.
    """
    if ctx.invoked_subcommand is not None:
        return

    logger.info("dev integrate: overview")
    with log_stage("integrate", subcommand="overview"):
        log_step("integrate/1: listing integration options", trace=True)
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
        log_step("integrate/complete", trace=True)


@app.command("doctor")
def integrations_doctor(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Check optional integration tools and local client wiring prerequisites."""
    root = _project_root(project_root)
    console.print(build_integrations_doctor_table(root))


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
            _record_claude_config(root, scope=scope, write_gate=write_gate)
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


@app.command("claude-github")
def claude_github(
    apply: bool = typer.Option(False, "--apply", help="Write the workflow file instead of previewing it."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Generate a GitHub Actions workflow that runs DevCouncil + Claude Code on repo events.

    Read-only DevCouncil verification on pull requests, and gated autonomous task pickup via
    headless Claude Code on manual dispatch or a nightly schedule. This is the stable-primitive
    alternative to Claude Code's experimental cloud Routines; the autonomous job needs an
    ANTHROPIC_API_KEY repository secret.
    """
    from devcouncil.integrations.claude_assets import build_github_workflow

    root = _project_root(project_root)
    asset = build_github_workflow(root)
    if not apply:
        console.print("[bold]DevCouncil GitHub Actions workflow (preview)[/bold]")
        console.print(f"Would write: [dim]{asset.path}[/dim]")
        console.print("[yellow]Preview only. Rerun with --apply to write the workflow.[/yellow]")
        return
    changed = asset.write_if_changed()
    rel = asset.path.relative_to(root)
    if changed:
        console.print(f"[green]Wrote GitHub Actions workflow[/green] at {rel}")
    else:
        console.print(f"[dim]GitHub Actions workflow already up to date at {rel}[/dim]")
    console.print("Add an [bold]ANTHROPIC_API_KEY[/bold] repository secret for the autonomous runs.")


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
        console.print(dump_json(entry, indent=2), soft_wrap=True)
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
    print_recommendations(_project_root(project_root), console)


@app.command("status")
def status(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
    as_json: bool = typer.Option(False, "--json", help="Emit machine-readable JSON."),
):
    """Show a compact integration summary without running the MCP server probe."""
    print_integration_status(_project_root(project_root), console, as_json=as_json)


@app.command("matrix")
def matrix(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Print built-in coding CLI integration tiers and capabilities."""
    _ = _project_root(project_root)
    print_integration_matrix(console)


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
    run_integration_check(
        _project_root(project_root),
        console,
        strict=strict,
        as_json=as_json,
        report_file=report_file,
        legacy_command=LEGACY_COMMAND,
    )


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

    trace_path = apply_agent_flow_setup(root)
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

    _executable, ignore_path, created = apply_code_review_graph_setup(root)
    if created:
        console.print(f"[green]Created {ignore_path}.[/green]")
    console.print("[green]code-review-graph adapter recorded in .devcouncil/config.yaml.[/green]")
