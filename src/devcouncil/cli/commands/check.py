"""`dev check` — a one-shot audit of the current uncommitted changes.

The lowest-friction entry point: no planning, no task graph. You let a coding
agent change your repo, then `dev check` tells you what is out of scope, what
edge cases look unhandled, what's risky, and whether any secrets leaked —
grounded in the real diff. With ``--goal`` it also compiles acceptance checks
from the stated intent and runs them as evidence.
"""

from __future__ import annotations

import asyncio
import json
import subprocess
from pathlib import Path

import typer
from rich.console import Console
from rich.table import Table

from devcouncil.app.config import get_api_key, load_config
from devcouncil.cli.commands.init import initialize_project
from devcouncil.domain.task import Task
from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter
from devcouncil.integrations.github_intent import resolve_goal_intent
from devcouncil.llm.provider import ProviderRequestError, create_provider, validate_model_provider
from devcouncil.llm.router import ModelRouter, StructuredOutputError
from devcouncil.verification.ad_hoc_check import AdHocCheckResult, run_working_tree_check
from devcouncil.verification.implementation_reviewer import ImplementationReviewer
from devcouncil.verification.verifier import Verifier

console = Console()


def _diff(root: Path, base: str | None) -> str:
    if not base:
        return Verifier(root).get_diff()
    try:
        return subprocess.check_output(
            ["git", "diff", base, "--"], cwd=root, text=True, encoding="utf-8", errors="replace"
        )
    except Exception:
        return ""


def check(
    goal: str | None = typer.Option(None, "--goal", "-g", help="What the change was meant to do — sharpens the review and enables acceptance checks. Also accepts a GitHub issue/PR reference (#142, owner/repo#142, or a github.com URL)."),
    base: str | None = typer.Option(None, "--base", help="Diff against this git ref instead of the uncommitted working tree."),
    test_commands: list[str] | None = typer.Option(None, "--test", "-t", help="A verification command proving the change works (repeatable). Switches to the deterministic evidence gate."),
    verify: bool = typer.Option(False, "--verify", help="Run the deterministic evidence gate (orphan-diff, acceptance evidence, diff↔coverage, next actions) instead of the LLM audit. No provider keys needed."),
    enforce_coverage: bool = typer.Option(False, "--enforce-coverage", help="Evidence gate: block when the tests do not exercise the changed lines."),
    min_coverage: float = typer.Option(0.0, "--min-coverage", help="Evidence gate: minimum fraction of changed lines that must be exercised (implies --enforce-coverage)."),
    json_format: bool = typer.Option(False, "--json", help="Machine-readable output."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Audit the current changes — scope, risks, missing edge cases, secrets — no planning required."""
    root = project_root.expanduser().resolve()
    initialize_project(root, quiet=True)

    # A --goal of "#142" or a GitHub issue/PR URL is a reference, not a spec —
    # expand it into the issue/PR title + body so acceptance checks and the review
    # are grounded in the real intent (same behavior as `dev go`).
    if goal:
        expanded_goal, intent_note = resolve_goal_intent(goal, root)
        if intent_note and not json_format:
            console.print(f"[dim]{intent_note}[/dim]")
        goal = expanded_goal

    # Evidence-gate mode: deterministic verification of the working-tree diff against an
    # inline requirement (--goal), with the diff↔coverage gate and the typed next-actions
    # contract. Provider-key-free — this is the lite path that lets you taste the gate.
    if verify or test_commands:
        result = run_working_tree_check(
            root,
            goal,
            test_commands=list(test_commands or []),
            enforce_coverage=enforce_coverage,
            min_ratio=min_coverage,
        )
        if json_format:
            typer.echo(json.dumps(result.to_dict(), indent=2))
        else:
            _render_gate(result)
        raise typer.Exit(code=0 if result.passed else 1)

    verifier = Verifier(root)

    diff = _diff(root, base)
    if not diff.strip():
        msg = "No changes to check (clean working tree)."
        typer.echo(json.dumps({"ok": True, "message": msg}) if json_format else msg)
        return

    changed_files = verifier.get_changed_files()
    secret_gaps = verifier.secret_scanner.scan_diff(diff, "check")

    # Blast radius: what do the changed files ripple into? Surfaced from the
    # structural graph when the code-review-graph integration is enabled, so a
    # reviewer sees the impact (and which tests to run) before approving.
    graph_context = CodeReviewGraphAdapter(root).get_context(changed_files)

    findings = []
    review_note = None
    try:
        config = load_config(root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, root)
        provider = create_provider(config.models.provider, api_key, project_root=root)
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        router = ModelRouter(provider, role_config, project_root=root)
        synthetic = Task(
            id="CHECK",
            title="Ad-hoc change review",
            description=(
                goal
                or "Review these changes for correctness, missing edge cases, error handling, "
                "risky shortcuts, and scope creep."
            ),
        )
        review = asyncio.run(ImplementationReviewer(router).review_changes(synthetic, [], diff))
        findings = review.findings
    except (ProviderRequestError, StructuredOutputError) as exc:
        review_note = f"LLM review unavailable: {exc}"
    except Exception as exc:  # pragma: no cover - best effort
        review_note = f"LLM review unavailable: {exc}"

    if json_format:
        typer.echo(json.dumps({
            "ok": not secret_gaps,
            "changed_files": changed_files,
            "secret_findings": [g.model_dump() for g in secret_gaps],
            "review_findings": [g.model_dump() for g in findings],
            "review_note": review_note,
            "blast_radius": {
                "available": graph_context.available,
                "impacted_files": graph_context.impacted_files,
                "related_tests": graph_context.related_tests,
            },
        }, indent=2))
        return

    console.print(f"[bold]Changed files ({len(changed_files)}):[/bold] " + ", ".join(changed_files[:20]) or "(none)")
    if secret_gaps:
        console.print(f"\n[red bold]⚠ Possible secrets in the diff ({len(secret_gaps)}):[/red bold]")
        for g in secret_gaps[:10]:
            console.print(f"  - {g.description[:100]}")
    if findings:
        console.print(f"\n[bold]Review findings ({len(findings)}):[/bold]")
        for f in findings[:15]:
            sev = getattr(f, "severity", "info")
            colour = {"critical": "red", "high": "red", "medium": "yellow"}.get(sev, "white")
            console.print(f"  - [{colour}]{sev}[/{colour}]: {f.description[:140]}")
    if graph_context.available and (graph_context.impacted_files or graph_context.related_tests):
        console.print("\n[bold]Blast radius[/bold] [dim](from the structural graph)[/dim]:")
        if graph_context.impacted_files:
            console.print(f"  [cyan]Impacted files ({len(graph_context.impacted_files)}):[/cyan] "
                          + ", ".join(graph_context.impacted_files[:15]))
        if graph_context.related_tests:
            console.print(f"  [cyan]Related tests ({len(graph_context.related_tests)}):[/cyan] "
                          + ", ".join(graph_context.related_tests[:15]))
    if not secret_gaps and not findings:
        console.print("\n[green]No secrets or review concerns found in the diff.[/green]")
    if review_note:
        console.print(f"\n[dim]{review_note}[/dim]")
    console.print(
        "\n[dim]Tip: `dev check --goal \"what this change should do\"` sharpens the review.[/dim]"
    )
    console.print(
        "[dim]Tip: `dev check --verify --test \"<cmd>\"` runs the deterministic evidence gate "
        "(no provider keys).[/dim]"
    )


def _render_gate(result: AdHocCheckResult) -> None:
    """Render the deterministic evidence-gate result for humans."""
    if result.reason == "no_changes":
        console.print("[yellow]No working-tree changes to verify. Make a change first, then re-run.[/yellow]")
        return

    console.print(f"[bold]Checking:[/bold] {result.requirement}")
    console.print(f"[dim]{len(result.changed_files)} changed file(s) in scope.[/dim]")
    if result.diff_coverage and result.diff_coverage.measured:
        console.print(f"[dim]Diff coverage: {result.diff_coverage.summary}[/dim]")

    if not result.gaps:
        console.print("\n[green]Verified: the change is backed by passing evidence.[/green]")
        return

    table = Table(title="Findings")
    table.add_column("Type", style="cyan")
    table.add_column("Severity", style="magenta")
    table.add_column("Finding", style="white")
    table.add_column("Blocking", style="red")
    for gap in result.gaps[:20]:
        table.add_row(gap.gap_type, gap.severity, gap.description, "YES" if gap.blocking else "no")
    console.print(table)

    if result.next_actions:
        console.print("\n[bold]Next actions:[/bold]")
        for action in result.next_actions:
            location = ""
            if action.file:
                location = f" [dim]({action.file}{':' + str(action.line) if action.line else ''})[/dim]"
            console.print(f"  • [[cyan]{action.category}[/cyan]] {action.action}{location}")

    if result.passed:
        console.print("\n[green]Verified with non-blocking signals only.[/green]")
    else:
        console.print("\n[red]Not verified: blocking gaps must be resolved.[/red]")
