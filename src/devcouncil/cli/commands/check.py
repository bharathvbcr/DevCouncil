"""`dev check` — a one-shot audit of the current uncommitted changes.

The lowest-friction entry point: no planning, no task graph. You let a coding
agent change your repo, then `dev check` tells you what is out of scope, what
edge cases look unhandled, what's risky, and whether any secrets leaked —
grounded in the real diff. With ``--goal`` it also compiles acceptance checks
from the stated intent and runs them as evidence.
"""

from __future__ import annotations

import asyncio
from devcouncil.utils.json_persist import dump_json
import logging
import os
import time
from datetime import datetime
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
from devcouncil.verification.gate_cache import GateResultCache
from devcouncil.verification.incremental_check import (
    run_incremental_gates,
    selected_gate_specs,
)
from devcouncil.verification.implementation_reviewer import ImplementationReviewer
from devcouncil.verification.verifier import Verifier
from devcouncil.telemetry.stages import log_stage, log_step

console = Console()
logger = logging.getLogger(__name__)


def _diff(root: Path, base: str | None) -> str:
    if not base:
        return Verifier(root).get_diff()
    from devcouncil.utils.proc import git_output

    return git_output(["diff", base, "--"], cwd=root, default="")


def check(
    goal: str | None = typer.Option(None, "--goal", "-g", help="What the change was meant to do — sharpens the review and enables acceptance checks. Also accepts a GitHub issue/PR reference (#142, owner/repo#142, or a github.com URL)."),
    base: str | None = typer.Option(None, "--base", help="Diff against this git ref instead of the uncommitted working tree."),
    test_commands: list[str] | None = typer.Option(None, "--test", "-t", help="A verification command proving the change works (repeatable). Switches to the deterministic evidence gate."),
    verify: bool = typer.Option(False, "--verify", help="Run the deterministic evidence gate (orphan-diff, acceptance evidence, diff↔coverage, next actions) instead of the LLM audit. No provider keys needed."),
    watch: bool = typer.Option(False, "--watch", help="Keep watching the project tree and re-run the deterministic evidence gate on every change (implies --verify). Prints one compact verdict line per run; Ctrl-C to stop."),
    list_gates: bool = typer.Option(False, "--list-gates", help="Preview which incremental stack gates would run for the current working-tree changes (no execution)."),
    enforce_coverage: bool = typer.Option(False, "--enforce-coverage", help="Evidence gate: block when the tests do not exercise the changed lines."),
    min_coverage: float = typer.Option(0.0, "--min-coverage", help="Evidence gate: minimum fraction of changed lines that must be exercised (implies --enforce-coverage)."),
    persist: bool = typer.Option(False, "--persist", help="Evidence gate: persist gaps/evidence for dev report (typical in CI)."),
    json_format: bool = typer.Option(False, "--json", help="Machine-readable output."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root."),
):
    """Audit the current changes — scope, risks, missing edge cases, secrets — no planning required."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info(
        "dev check: verify=%s goal=%s base=%s watch=%s list_gates=%s",
        verify, bool(goal), base or "working-tree", watch, list_gates,
    )
    initialize_project(root, quiet=True)

    with log_stage("check", project_root=root, verify=verify):
        if list_gates:
            _list_gates(root, test_commands, json_format)
            return
        _run_check_body(
            root, goal, base, test_commands, verify, enforce_coverage,
            min_coverage, json_format, watch, persist,
        )


def _list_gates(root: Path, test_commands: list[str] | None, json_format: bool) -> None:
    """Dry-run: show incremental gates for the current working-tree diff."""
    from devcouncil.verification.incremental_check import (
        _default_changed_files,
        _default_commands,
    )

    changed = _default_changed_files(root)
    cmds = _default_commands(root)
    if test_commands:
        cmds = {**cmds, "test": list(cmds.get("test") or []) + list(test_commands)}
    gates = selected_gate_specs(root, changed, commands=cmds, narrow=True)
    payload = {
        "changed_files": changed,
        "gates": [
            {
                "name": g.name,
                "kind": g.kind,
                "command": list(g.command),
                "narrowed": g.narrowed,
            }
            for g in gates
        ],
    }
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        return
    if not changed:
        console.print("[dim]No working-tree changes — no gates selected.[/dim]")
        return
    if not gates:
        console.print(f"[dim]{len(changed)} changed file(s), no matching stack gates.[/dim]")
        return
    console.print(f"[bold]{len(gates)}[/bold] gate(s) for {len(changed)} changed file(s):")
    for g in gates:
        narrow = " [narrowed]" if g.narrowed else ""
        console.print(f"  • {g.kind}/{g.name}{narrow}: {' '.join(g.command)}")


def _run_check_body(
    root, goal, base, test_commands, verify, enforce_coverage, min_coverage, json_format, watch, persist,
):
    log_step("check/1: resolving diff scope", project_root=root, trace=True)

    # A --goal of "#142" or a GitHub issue/PR URL is a reference, not a spec —
    # expand it into the issue/PR title + body so acceptance checks and the review
    # are grounded in the real intent (same behavior as `dev go`).
    if goal:
        expanded_goal, intent_note = resolve_goal_intent(goal, root)
        if intent_note and not json_format:
            console.print(f"[dim]{intent_note}[/dim]")
        goal = expanded_goal

    # Watch mode: run the deterministic gate, then re-run it whenever the project tree
    # changes, printing one compact verdict line per run. Implies --verify — looping the
    # LLM audit would burn provider tokens on every save.
    if watch:
        if json_format:
            raise typer.BadParameter("--watch is interactive; it cannot be combined with --json.")
        _watch_gate(
            root, goal,
            test_commands=list(test_commands or []),
            enforce_coverage=enforce_coverage,
            min_coverage=min_coverage,
        )
        return

    # Evidence-gate mode: deterministic verification of the working-tree diff against an
    # inline requirement (--goal), with the diff↔coverage gate and the typed next-actions
    # contract. Provider-key-free — this is the lite path that lets you taste the gate.
    if verify or test_commands:
        result = run_working_tree_check(
            root,
            goal,
            base=base,
            test_commands=list(test_commands or []),
            enforce_coverage=enforce_coverage,
            min_ratio=min_coverage,
            persist=persist,
        )
        if json_format:
            typer.echo(dump_json(result.to_dict(), indent=2))
        else:
            _render_gate(result)
        raise typer.Exit(code=0 if result.passed else 1)

    log_step("check/2: running LLM audit", project_root=root)
    verifier = Verifier(root)

    logger.info("dev check (LLM audit): base=%s goal=%s", base or "working-tree", "set" if goal else "none")
    diff = _diff(root, base)
    if not diff.strip():
        logger.info("dev check: clean working tree; nothing to audit")
        msg = "No changes to check (clean working tree)."
        typer.echo(dump_json({"ok": True, "message": msg}) if json_format else msg)
        return

    changed_files = verifier.get_changed_files()
    secret_gaps = verifier.secret_scanner.scan_diff(diff, "check")
    if secret_gaps:
        logger.warning("dev check: %d possible secret(s) in diff across %d changed file(s)", len(secret_gaps), len(changed_files))

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
        provider = create_provider(config.models.provider, api_key, project_root=root, provider_prefs=config.provider)
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
        logger.warning("dev check: LLM review unavailable: %s", exc)
        review_note = f"LLM review unavailable: {exc}"
    except Exception as exc:  # pragma: no cover - best effort
        logger.warning("dev check: LLM review unavailable: %s", exc)
        review_note = f"LLM review unavailable: {exc}"
    logger.info("dev check audit complete: %d secret finding(s), %d review finding(s)", len(secret_gaps), len(findings))

    if json_format:
        typer.echo(dump_json({
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
    log_step("check/complete", project_root=root, trace=True)


def _render_gate(result: AdHocCheckResult) -> None:
    """Render the deterministic evidence-gate result for humans."""
    if result.reason == "no_changes":
        console.print(
            "[yellow]No changes to verify. Make a change or pass --base for PR-scoped diff.[/yellow]"
        )
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


# --watch plumbing. execution/fs_watcher.FilesystemWatcher was considered and rejected:
# it exists to ATTRIBUTE changes to a leased task (needs a task_id; records file-change
# events and orphan-diff gaps in the DB through the policy engine), not to signal
# "something changed". A 2s mtime scan plus a short settle debounce is the genuinely
# simpler fit here — no watchdog dependency, identical behavior on every platform.
_WATCH_IGNORED_DIRS = frozenset({
    ".git", ".hg", ".svn", ".devcouncil", "__pycache__", ".pytest_cache", ".mypy_cache",
    ".ruff_cache", "node_modules", "dist", "build", "target", ".venv", "venv", ".tox",
    ".idea", ".vscode",
})
_WATCH_POLL_SECONDS = 2.0
_WATCH_DEBOUNCE_SECONDS = 1.5


def _watch_snapshot(root: Path) -> dict:
    """One cheap pass over the tree: ``{path: mtime_ns}`` for non-ignored files."""
    snapshot: dict = {}
    stack = [str(root)]
    while stack:
        directory = stack.pop()
        try:
            with os.scandir(directory) as entries:
                for entry in entries:
                    try:
                        if entry.is_dir(follow_symlinks=False):
                            if entry.name not in _WATCH_IGNORED_DIRS:
                                stack.append(entry.path)
                        elif entry.is_file(follow_symlinks=False):
                            snapshot[entry.path] = entry.stat(follow_symlinks=False).st_mtime_ns
                    except OSError:
                        continue
        except OSError:
            continue
    return snapshot


def _watch_evidence_once(root: Path, goal, test_commands, enforce_coverage, min_coverage, stamp) -> None:
    """Full deterministic evidence gate — the fallback when no stack gate applies."""
    try:
        result = run_working_tree_check(
            root, goal,
            test_commands=test_commands,
            enforce_coverage=enforce_coverage,
            min_ratio=min_coverage,
        )
    except Exception as exc:  # keep watching through transient failures (git hiccups, ...)
        logger.warning("dev check --watch: run failed: %s", exc)
        console.print(f"[dim]{stamp}[/dim] [red]ERROR[/red] — {str(exc)[:120]}")
        return
    verdict = "[green]PASS[/green]" if result.passed else "[red]FAIL[/red]"
    blocking = len([g for g in result.gaps if g.blocking])
    detail = f"{len(result.gaps)} gap(s)" + (f", {blocking} blocking" if blocking else "")
    if result.reason == "no_changes":
        detail = "no working-tree changes"
    console.print(f"[dim]{stamp}[/dim] {verdict} — {detail} [dim](evidence gate)[/dim]")


def _watch_gate_once(root: Path, goal, test_commands, enforce_coverage, min_coverage, cache) -> None:
    """Run the incremental gate once and print a compact one-line verdict.

    Only the gates whose inputs actually changed are executed; the rest are served from
    the content-hash cache, so iterative edits stay sub-second. When no stack-relevant
    gate is configured for the change, fall back to the full evidence gate so the user
    still gets orphan/secret/coverage feedback."""
    stamp = datetime.now().strftime("%H:%M:%S")
    try:
        result = run_incremental_gates(
            root, extra_test_commands=test_commands, cache=cache,
        )
    except Exception as exc:  # keep watching through transient failures (git hiccups, ...)
        logger.warning("dev check --watch: incremental run failed: %s", exc)
        console.print(f"[dim]{stamp}[/dim] [red]ERROR[/red] — {str(exc)[:120]}")
        return

    if result.no_changes:
        console.print(f"[dim]{stamp}[/dim] [dim]— no working-tree changes[/dim]")
        return
    if result.no_gates:
        # No configured command targets the changed stack(s); give the full picture.
        _watch_evidence_once(root, goal, test_commands, enforce_coverage, min_coverage, stamp)
        return

    verdict = "[green]PASS[/green]" if result.passed else "[red]FAIL[/red]"
    ran, cached = len(result.ran), len(result.cached)
    detail = f"{ran} gate(s) run, {cached} cached"
    failing = [o for o in result.outcomes if not o.passed]
    if failing:
        detail += " — failed: " + ", ".join(o.kind for o in failing[:4])
    ms = result.duration_s * 1000
    console.print(f"[dim]{stamp}[/dim] {verdict} — {detail} [dim]({ms:.0f}ms)[/dim]")
    if result.narrowed:
        logger.info("dev check --watch: narrowed — full verify recommended before commit")
        console.print(
            "[dim]narrowed — full verify recommended before commit[/dim]"
        )


def _watch_gate(root: Path, goal, *, test_commands, enforce_coverage, min_coverage) -> None:
    """Initial gate run, then a poll/debounce loop re-running it until Ctrl-C.

    A single :class:`GateResultCache` is shared across iterations so a gate whose inputs
    did not change between saves is skipped rather than re-run."""
    console.print(
        f"[dim]Watching {root} — re-running only the gates affected by each change "
        f"(poll {_WATCH_POLL_SECONDS:.0f}s, debounce {_WATCH_DEBOUNCE_SECONDS:.1f}s). "
        "Ctrl-C to stop. Run without --watch for full findings.[/dim]"
    )
    cache = GateResultCache(root)
    try:
        _watch_gate_once(root, goal, test_commands, enforce_coverage, min_coverage, cache)
        baseline = _watch_snapshot(root)
        while True:
            time.sleep(_WATCH_POLL_SECONDS)
            current = _watch_snapshot(root)
            if current == baseline:
                continue
            # Debounce: let an editor save-storm settle before burning a run.
            time.sleep(_WATCH_DEBOUNCE_SECONDS)
            _watch_gate_once(root, goal, test_commands, enforce_coverage, min_coverage, cache)
            # Re-baseline AFTER the run so artifacts the gate itself writes (coverage
            # data, logs outside .devcouncil/) can never re-trigger it. Trade-off:
            # edits made while the gate was running are absorbed — save again.
            baseline = _watch_snapshot(root)
    except KeyboardInterrupt:
        console.print("\n[dim]Watch stopped.[/dim]")
