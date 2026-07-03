import json
import logging
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.telemetry.cost import group_cost
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="Inspect DevCouncil model-call cost, grouped by task and run.")
console = Console()
logger = logging.getLogger(__name__)


@app.command("show")
def show(
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Report estimated model-call cost grouped by task_id and run_id.

    Reads the local ``model_calls.jsonl`` ledger only — fully offline. Records
    written before per-task attribution (or made without a task/run context) are
    grouped under ``(unattributed)``.
    """
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev cost show: json=%s", json_format)

    with log_stage("cost", project_root=root):
        log_step("cost/1: aggregating model-call ledger", project_root=root, trace=True)
        summary = group_cost(root)

        if json_format:
            typer.echo(json.dumps(summary, indent=2))
            log_step("cost/complete", project_root=root, trace=True)
            return

        console.print(
            f"[bold]Total Cost:[/bold] ${summary['total_cost']:.4f} "
            f"across {summary['total_calls']} model call(s)"
        )

        def _render(title: str, groups: dict) -> None:
            if not groups:
                return
            table = Table(title=title)
            table.add_column("Group", style="cyan")
            table.add_column("Cost ($)", justify="right")
            table.add_column("Calls", justify="right")
            table.add_column("Prompt", justify="right")
            table.add_column("Completion", justify="right")
            for name, stats in sorted(groups.items(), key=lambda kv: kv[1]["cost"], reverse=True):
                table.add_row(
                    name,
                    f"{stats['cost']:.4f}",
                    str(stats["calls"]),
                    str(stats["prompt_tokens"]),
                    str(stats["completion_tokens"]),
                )
            console.print(table)

        _render("Cost by Task", summary["by_task"])
        _render("Cost by Run", summary["by_run"])
        log_step("cost/complete", project_root=root, trace=True)
