from devcouncil.utils.json_persist import dump_json
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
            typer.echo(dump_json(summary, indent=2))
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


@app.command("budget")
def budget(
    set_value: float = typer.Option(
        None,
        "--set",
        help="Set the advisory spend budget in USD (telemetry.cost_budget_usd in config.yaml).",
    ),
    clear: bool = typer.Option(False, "--clear", help="Remove the configured budget."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Show or configure the advisory model-spend budget.

    Reports the configured budget (``telemetry.cost_budget_usd``), spend-to-date from
    the local ``model_calls.jsonl`` ledger, and the remainder. WARN-ONLY: when
    cumulative spend crosses the budget the telemetry tracker logs a warning, but no
    run is ever blocked.
    """
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev cost budget: set=%s clear=%s json=%s", set_value, clear, json_format)

    if set_value is not None and clear:
        console.print("[red]Use either --set or --clear, not both.[/red]")
        raise typer.Exit(code=2)

    with log_stage("cost", project_root=root, subcommand="budget"):
        if set_value is not None or clear:
            import yaml

            config_path = root / ".devcouncil" / "config.yaml"
            if not config_path.exists():
                console.print(f"[red]Config not found at {config_path}. Run 'dev init' first.[/red]")
                raise typer.Exit(code=1)
            if set_value is not None and set_value <= 0:
                console.print("[red]--set expects a positive USD amount (e.g. --set 5.00).[/red]")
                raise typer.Exit(code=2)

            log_step("cost/1: updating telemetry.cost_budget_usd", project_root=root, trace=True)
            with open(config_path, encoding="utf-8") as f:
                raw_config = yaml.safe_load(f) or {}
            if set_value is not None:
                raw_config.setdefault("telemetry", {})["cost_budget_usd"] = float(set_value)
            else:
                telemetry_section = raw_config.get("telemetry")
                if isinstance(telemetry_section, dict):
                    telemetry_section.pop("cost_budget_usd", None)
                    if not telemetry_section:
                        raw_config.pop("telemetry", None)
            with open(config_path, "w", encoding="utf-8") as f:
                yaml.dump(raw_config, f, default_flow_style=False)
            if set_value is not None:
                console.print(f"[green]Set telemetry.cost_budget_usd = {float(set_value):.2f}[/green]")
            else:
                console.print("[green]Cleared telemetry.cost_budget_usd.[/green]")

        log_step("cost/2: aggregating model-call ledger", project_root=root, trace=True)
        try:
            from devcouncil.app.config import load_config

            budget_usd = load_config(root).telemetry.cost_budget_usd
        except Exception:
            budget_usd = None
        spend = float(group_cost(root)["total_cost"])
        remaining = None if budget_usd is None else budget_usd - spend

        if json_format:
            typer.echo(
                dump_json(
                    {
                        "budget_usd": budget_usd,
                        "spend_usd": spend,
                        "remaining_usd": remaining,
                        "over_budget": bool(budget_usd is not None and spend > budget_usd),
                    },
                    indent=2,
                )
            )
            log_step("cost/complete", project_root=root, trace=True)
            return

        if budget_usd is None:
            console.print("[bold]Budget:[/bold] not configured (set one with 'dev cost budget --set 5.00')")
            console.print(f"[bold]Spend to date:[/bold] ${spend:.4f}")
        else:
            console.print(f"[bold]Budget:[/bold] ${budget_usd:.2f}")
            console.print(f"[bold]Spend to date:[/bold] ${spend:.4f}")
            if remaining is not None and remaining < 0:
                console.print(
                    f"[bold]Remaining:[/bold] [red]-${abs(remaining):.4f} "
                    "(over budget — warn-only, runs are never blocked)[/red]"
                )
            else:
                console.print(f"[bold]Remaining:[/bold] ${remaining:.4f}")
        log_step("cost/complete", project_root=root, trace=True)
