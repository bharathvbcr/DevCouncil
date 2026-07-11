"""Rich/JSON renderers for dev integrate subcommands."""

from __future__ import annotations

import json
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.executors.agent_registry import (
    BUILTIN_CODING_EXECUTOR_NAMES,
    CODING_CLI_INTEGRATION_INFO,
    detect_available_coding_cli,
    integration_tier_label,
    resolve_automated_executor,
    resolve_coding_cli_executable,
    resolve_coding_cli_probe_order,
)
from devcouncil.integrations.check import build_integration_check_report, integration_status_summary
from devcouncil.integrations.clients.common import _load_raw_config

PREFERRED_COMMAND = "dev integrate"


def print_recommendations(root: Path, console: Console) -> None:
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


def print_integration_status(root: Path, console: Console, *, as_json: bool) -> None:
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
    table.add_row("Grok resume mode", summary.get("grok_resume_mode", "off"))

    for name in ("cursor", "grok", "opencode", "antigravity", "warp", "aider"):
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


def print_integration_matrix(console: Console) -> None:
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


def run_integration_check(
    root: Path,
    console: Console,
    *,
    strict: bool,
    as_json: bool,
    report_file: Path | None,
    legacy_command: str = "dev setup --integrate",
) -> None:
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
                f"(or {legacy_command} --apply)."
            )
        raise typer.Exit(code=1)

    if not as_json:
        console.print(f"\n[green]Ready.[/green] Run: {PREFERRED_COMMAND} all --apply (or {legacy_command} --apply).")
