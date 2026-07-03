import json
import logging
from typing import NoReturn

import typer
from pathlib import Path
from rich.console import Console

from devcouncil.cli.commands.init import initialize_project
from devcouncil.execution.handoff import HandoffService
from devcouncil.executors.agent_registry import load_cli_agent_specs, normalize_agent_name
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def handoff(
    task_id: str = typer.Argument(...),
    from_agent: str = typer.Option(..., "--from"),
    to_agent: str = typer.Option(..., "--to"),
    instruction: str = typer.Option("", "--instruction"),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON the agent can chain on."),
    project_root: Path = typer.Option(Path("."), "--project-root"),
):
    """
    Hand off a task between coding CLI agents.
    """
    def _fail(message: str) -> NoReturn:
        if json_format:
            typer.echo(json.dumps({"ok": False, "task_id": task_id, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=1)

    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev handoff: task=%s from=%s to=%s", task_id, from_agent, to_agent)
    initialize_project(root, quiet=True)

    with log_stage("handoff", project_root=root, task_id=task_id, from_agent=from_agent, to_agent=to_agent):
        log_step("handoff/1: validating agents", project_root=root, task_id=task_id, trace=True)
        specs = load_cli_agent_specs(root)
        from_name = normalize_agent_name(from_agent)
        to_name = normalize_agent_name(to_agent)
        if from_name not in specs or to_name not in specs:
            _fail("Unknown agent name. Use dev agents list.")

        try:
            manifest, path, run_id = HandoffService(root).create(
                task_id,
                from_name,
                to_name,
                instruction=instruction,
            )
        except ValueError as exc:
            _fail(str(exc))

        next_command = f"dev run {task_id} --executor {to_name}"
        if json_format:
            typer.echo(json.dumps({
                "ok": True,
                "task_id": task_id,
                "from": from_name,
                "to": to_name,
                "manifest_path": str(path),
                "run_id": run_id,
                "next_command": next_command,
            }, indent=2))
            log_step("handoff/complete", project_root=root, task_id=task_id, trace=True)
            return

        console.print(f"[green]Handoff manifest:[/green] {path}")
        console.print(f"[cyan]Next:[/cyan] {next_command}")
        console.print(f"[dim]Run artifacts: .devcouncil/runs/{run_id}[/dim]")
        if instruction:
            console.print(f"[dim]Instruction: {instruction}[/dim]")
        _ = manifest
        log_step("handoff/complete", project_root=root, task_id=task_id, trace=True)
