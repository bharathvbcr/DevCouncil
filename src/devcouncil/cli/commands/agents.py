from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.cli.commands import run as run_command
from devcouncil.cli.commands.integrate import _load_raw_config, _project_root, _save_raw_config
from devcouncil.executors.agent_registry import (
    VALID_INPUT_MODES,
    agent_config_entry,
    detect_available_coding_cli,
    is_reserved_agent_name,
    load_agent_profiles,
    load_cli_agent_specs,
    normalize_agent_name,
    resolve_automated_executor,
    resolve_cursor_agent_executable,
)
from devcouncil.optimization.gepa_agent import GepaUnavailableError, optimize_agent_profile
from devcouncil.telemetry.stages import log_stage, log_step
import logging

app = typer.Typer(help="Manage DevCouncil CLI agents.")
console = Console()
logger = logging.getLogger(__name__)


@app.callback(invoke_without_command=True)
def list_agents(
    ctx: typer.Context,
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """List built-in and configured CLI agents."""
    if ctx.invoked_subcommand is not None:
        return

    root = _project_root(project_root)
    logger.info("dev agents list")
    with log_stage("agents", project_root=root, subcommand="list"):
        log_step("agents/1: loading agent registry", project_root=root, trace=True)
        table = Table(title="DevCouncil Agents")
        table.add_column("Agent", style="cyan")
        table.add_column("Type")
        table.add_column("Command")
        table.add_column("Profile")
        table.add_column("MCP")
        table.add_column("Diff Review")

        for name, spec in sorted(load_cli_agent_specs(root).items()):
            table.add_row(
                name,
                "built-in" if spec.built_in else spec.kind,
                " ".join(spec.base_command()),
                spec.default_profile,
                "yes" if spec.supports_mcp else "no",
                "yes" if spec.supports_diff_review else "no",
            )
        console.print(table)
        log_step("agents/complete", project_root=root, trace=True)


@app.command("add")
def add_agent(
    name: str = typer.Argument(..., help="Agent name, for example opencode or aider."),
    command: str = typer.Option(..., "--command", help="Executable to launch."),
    arg: list[str] | None = typer.Option(None, "--arg", help="Argument to pass to the CLI. Repeat for multiple args."),
    input_mode: str = typer.Option("stdin", "--input-mode", help="Prompt input mode: stdin, argument, or prompt-file."),
    prompt_arg: str | None = typer.Option(None, "--prompt-arg", help="Flag used before the prompt or prompt file."),
    timeout_seconds: int | None = typer.Option(None, "--timeout-seconds", help="Agent-specific timeout override."),
    display_name: str | None = typer.Option(None, "--display-name", help="Human-readable agent name."),
    kind: str = typer.Option("custom", "--kind", help="Agent kind, for example coding-cli or review-cli."),
    supports_mcp: bool = typer.Option(False, "--supports-mcp", help="Mark this agent as MCP-capable."),
    supports_diff_review: bool = typer.Option(False, "--supports-diff-review", help="Mark this agent as able to review diffs."),
    default_profile: str = typer.Option("default", "--default-profile", help="Default execution profile for this agent."),
    help_arg: list[str] | None = typer.Option(None, "--help-arg", help="Argument for the agent help command. Repeat for multiple args."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Register an arbitrary prompt-taking CLI as a DevCouncil agent."""
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
    config = _load_raw_config(root)
    agents = config.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("agents", {})
    agents[normalized] = entry
    _save_raw_config(root, config)
    console.print(f"[green]Registered CLI agent '{normalized}' in .devcouncil/config.yaml.[/green]")


@app.command("doctor")
def doctor(
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Check configured CLI agents and execution profiles."""
    root = _project_root(project_root)
    profiles = load_agent_profiles(root)
    table = Table(title="DevCouncil Agent Doctor")
    table.add_column("Agent", style="cyan")
    table.add_column("Status")
    table.add_column("Details", no_wrap=True)

    for name, spec in sorted(load_cli_agent_specs(root).items()):
        if spec.name == "cursor":
            executable = resolve_cursor_agent_executable()
        else:
            executable = _which(spec.executable)
        mode_ok = spec.input_mode in VALID_INPUT_MODES
        profile_ok = spec.default_profile in profiles
        help_ok, help_detail = _check_help(spec.help_command or [spec.executable, "--help"])

        if executable and mode_ok and profile_ok:
            status = "[green]OK[/green]"
        elif executable:
            status = "[red]Invalid[/red]"
        else:
            status = "[yellow]Missing[/yellow]"

        details = []
        details.append(executable or f"{spec.executable} not found on PATH")
        if not mode_ok:
            details.append(f"invalid input_mode={spec.input_mode}")
        if not profile_ok:
            details.append(f"missing profile={spec.default_profile}")
        if help_ok:
            details.append("help command OK")
        elif not spec.built_in:
            details.append(help_detail)
        table.add_row(name, status, "; ".join(details))

    console.print(table)
    detected = detect_available_coding_cli(root)
    if detected:
        resolved = resolve_automated_executor(root, None)
        console.print(
            f"\n[dim]Auto-pick for dev go / dev run:[/dim] [cyan]{resolved}[/cyan] "
            f"(first built-in CLI on PATH in probe order)"
        )
    else:
        console.print("\n[dim]No built-in coding CLI on PATH for auto-pick.[/dim]")


@app.command("run")
def run_agent(
    task_id: str = typer.Argument(..., help="ID of the task to run."),
    agent: str = typer.Option(..., "--agent", "-a", help="Agent name to execute."),
    profile: str | None = typer.Option(None, "--profile", help="Execution profile: default, yolo, prod, or configured."),
    stream: bool = typer.Option(
        False,
        "--stream",
        help="Stream coding CLI stdout/stderr live (also enabled by execution.stream_cli_output).",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Run a DevCouncil task with a named CLI agent and profile."""
    run_command.run(task_id, executor=agent, profile=profile, stream=stream, project_root=project_root)


def _which(command: str) -> str | None:
    from shutil import which

    return which(command)


@app.command("help")
def agent_help(
    name: str = typer.Argument(..., help="Agent name, for example codex or opencode."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """Show the underlying CLI agent's own help output."""
    root = _project_root(project_root)
    spec = load_cli_agent_specs(root).get(normalize_agent_name(name))
    if spec is None:
        console.print(f"[red]Unknown agent '{name}'. Use 'dev agents' to list available agents.[/red]")
        raise typer.Exit(code=1)
    command = list(spec.help_command) or [spec.executable, "--help"]
    executable = _which(command[0])
    if not executable:
        console.print(f"[red]{command[0]} is not installed or not on PATH.[/red]")
        raise typer.Exit(code=1)
    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    try:
        result = subprocess.run(
            subprocess.list2cmdline(resolved) if use_shell else resolved,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=15,
            shell=use_shell,
        )
    except (subprocess.TimeoutExpired, FileNotFoundError, OSError) as exc:
        console.print(f"[red]Failed to run {' '.join(command)}: {exc}[/red]")
        raise typer.Exit(code=1)
    output = (result.stdout or "") + (result.stderr or "")
    typer.echo(output.strip())
    raise typer.Exit(code=result.returncode)


def _check_help(command: list[str]) -> tuple[bool, str]:
    executable = _which(command[0]) if command else None
    if not command or not executable:
        return False, "help command unavailable"
    resolved = [executable, *command[1:]]
    use_shell = sys.platform == "win32" and Path(executable).suffix.lower() in {".bat", ".cmd", ".ps1"}
    try:
        result = subprocess.run(
            subprocess.list2cmdline(resolved) if use_shell else resolved,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=5,
            shell=use_shell,
        )
    except subprocess.TimeoutExpired:
        return False, "help command timed out"
    return result.returncode == 0, f"help command exited {result.returncode}"


@app.command("optimize")
def optimize_agent(
    agent: str = typer.Option(..., "--agent", "-a", help="Agent name to optimize, for example codex or opencode."),
    profile_name: str = typer.Option("default", "--profile", help="Profile name to optimize: default, yolo, prod, or configured."),
    evals_path: Path = typer.Option(..., "--evals", help="JSON or JSONL GEPA evaluation dataset."),
    max_metric_calls: int = typer.Option(40, "--max-metric-calls", min=1, help="Maximum GEPA metric calls."),
    objective: str | None = typer.Option(None, "--objective", help="Override the default optimization objective."),
    apply: bool = typer.Option(
        False,
        "--apply/--dry-run",
        help="Write the optimized preamble into .devcouncil/config.yaml. Defaults to dry-run.",
    ),
    output_path: Path | None = typer.Option(None, "--output", help="Write the optimization artifact to this path."),
    project_root: Path | None = typer.Option(None, "--project-root", help="Repository root containing .devcouncil/."),
):
    """
    Optimize a CLI-agent profile prompt preamble with GEPA.
    """
    root = _project_root(project_root)
    try:
        result = optimize_agent_profile(
            project_root=root,
            agent=agent,
            profile_name=profile_name,
            evals_path=evals_path,
            max_metric_calls=max_metric_calls,
            objective=objective,
            apply=apply,
            output_path=output_path,
        )
    except GepaUnavailableError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=1) from exc
    except ValueError as exc:
        console.print(f"[red]{exc}[/red]")
        raise typer.Exit(code=2) from exc

    mode = "applied" if result.applied else "dry-run"
    score = "unknown" if result.best_score is None else f"{result.best_score:.3f}"
    console.print(
        f"[green]GEPA optimization complete ({mode}) for {result.agent}:{result.profile_name}.[/green]"
    )
    console.print(f"Score: [bold]{score}[/bold]")
    console.print(f"Artifact: [dim]{result.artifact_path}[/dim]")
    console.print("\n[bold]Optimized preamble[/bold]")
    console.print(result.best_preamble or "[dim](empty)[/dim]")
