from __future__ import annotations

from devcouncil.utils.json_persist import dump_json
import asyncio
import logging
import time
from pathlib import Path

import typer
from rich.console import Console
from rich.panel import Panel
from rich.table import Table

from devcouncil.live.cards import (
    card_path,
    filter_cards,
    get_card,
    load_cards,
    review_turn,
    save_card,
    update_card_status,
)
from devcouncil.live.repair_prompt import build_bulk_live_repair_prompt, build_live_repair_prompt
from devcouncil.live.reviewer import LiveReviewService
from devcouncil.live.signals import ReviewSignal, load_signals, mark_processed
from devcouncil.live.summary import live_review_summary
from devcouncil.live.tasks import active_task_id
from devcouncil.live.transcripts import discover_sessions, latest_assistant_turn, load_turns
from devcouncil.app.config import get_api_key, load_config
from devcouncil.llm.provider import create_provider, validate_model_provider
from devcouncil.llm.router import ModelRouter
from devcouncil.telemetry.traces import TraceLogger
from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer(help="Review active coding-agent sessions and emit critique cards.")
console = Console()
logger = logging.getLogger(__name__)


@app.command("sessions")
def sessions(
    client: str = typer.Option("claude", "--client", help="Agent client: claude or generic."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """List coding-agent transcripts DevCouncil can review."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev watch sessions: client=%s", client)
    with log_stage("watch", project_root=root, subcommand="sessions", client=client):
        log_step("watch/1: discovering sessions", project_root=root, trace=True)
        found = discover_sessions(root, client=client)
        if json_format:
            typer.echo(dump_json({"sessions": [item.model_dump() for item in found]}, indent=2))
            log_step("watch/complete", project_root=root, count=len(found), trace=True)
            return

        table = Table(title="DevCouncil Watch Sessions")
        table.add_column("Client", style="cyan")
        table.add_column("Session")
        table.add_column("Turns", justify="right")
        table.add_column("Transcript")
        for item in found:
            table.add_row(item.client, item.id, str(item.turns), item.transcript_path)
        console.print(table)
        log_step("watch/complete", project_root=root, count=len(found), trace=True)


@app.command("review")
def review(
    transcript: Path | None = typer.Option(None, "--transcript", "-t", help="JSONL transcript to review."),
    session: str | None = typer.Option(None, "--session", help="Discovered session ID to review, or 'latest'."),
    latest: bool = typer.Option(False, "--latest", help="Review the most recently updated discovered session."),
    client: str = typer.Option("generic", "--client", help="Agent client: claude, codex, gemini, cursor, or generic."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    persist: bool = typer.Option(True, "--persist/--no-persist", help="Save the critique card under .devcouncil/live/cards."),
    llm: bool = typer.Option(False, "--llm", help="Use the configured live_reviewer or implementation_reviewer model."),
    force: bool = typer.Option(False, "--force", help="Regenerate and overwrite an existing card for this turn."),
    task_id: str | None = typer.Option(None, "--task-id", help="Associate the critique card with a DevCouncil task."),
):
    """Review the latest assistant response in a coding-agent transcript."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev watch review: client=%s llm=%s", client, llm)
    transcript_path = _resolve_transcript(root, client, transcript=transcript, session=session, latest=latest)
    if transcript_path is None:
        message = "No transcript selected. Use --transcript, --session, or --latest."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=2)
    turn = latest_assistant_turn(transcript_path, client=client)
    if turn is None:
        message = f"No assistant turn found in {transcript_path}."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[yellow]{message}[/yellow]")
        raise typer.Exit(code=1)

    scoped_task_id = task_id or active_task_id(root)
    card = asyncio.run(_review_turn(turn, root, client, llm, task_id=scoped_task_id))
    logger.info("dev watch review: card %s verdict=%s task=%s", card.id, card.verdict, scoped_task_id or "(unscoped)")
    saved_path, duplicate = _save_card_once(root, card, persist=persist, force=force)
    if saved_path:
        _log_card_reviewed(root, card, saved_path, duplicate=duplicate, source="review")
    payload = card.model_dump()
    if saved_path:
        payload["path"] = str(saved_path)
    payload["duplicate"] = duplicate

    if json_format:
        typer.echo(dump_json(payload, indent=2))
        return

    _print_card(card)
    if saved_path:
        console.print(f"[green]Saved critique card:[/green] {saved_path}")


@app.command("cards")
def cards(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    limit: int = typer.Option(10, "--limit", "-n", help="Maximum cards to show."),
    task_id: str | None = typer.Option(None, "--task-id", help="Filter cards by task ID."),
    status: str | None = typer.Option(None, "--status", help="Filter by status: open, resolved, or ignored."),
    verdict: str | None = typer.Option(None, "--verdict", help="Filter by verdict: approved, concerns, or critical."),
    client: str | None = typer.Option(None, "--client", help="Filter by coding-agent client."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Show previously emitted critique cards."""
    root = project_root.expanduser().resolve()
    if limit < 1:
        message = "--limit must be greater than 0."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=2)
    found, error, _argument = filter_cards(
        load_cards(root),
        task_id=task_id,
        status=status,
        verdict=verdict,
        client=client,
    )
    if error:
        if json_format:
            typer.echo(dump_json({"ok": False, "error": error}, indent=2))
        else:
            console.print(f"[red]{error}[/red]")
        raise typer.Exit(code=2)
    total = len(found)
    found = found[:limit]
    if json_format:
        typer.echo(dump_json({
            "cards": [item.model_dump() for item in found],
            "filters": {
                "task_id": task_id,
                "status": status,
                "verdict": verdict,
                "client": client,
            },
            "limit": limit,
            "total": total,
        }, indent=2))
        return
    for card in found:
        _print_card(card)


@app.command("status")
def status(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    task_id: str | None = typer.Option(None, "--task-id", help="Task scope for blocking-card calculation."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Show compact live-review status, blockers, pending signals, and recent cards."""
    root = project_root.expanduser().resolve()
    payload = live_review_summary(root, task_id=task_id)
    if json_format:
        typer.echo(dump_json(payload, indent=2))
        return
    blockers = payload["blocking_cards"]

    summary = Table(title="DevCouncil Live Review Status")
    summary.add_column("Metric", style="cyan")
    summary.add_column("Value")
    summary.add_row("Active task", payload["active_task_id"] or "(none)")
    summary.add_row("Scope task", payload["scope_task_id"] or "(unscoped)")
    summary.add_row("Pending signals", str(payload["pending_signals"]))
    summary.add_row("Cards", str(payload["cards"]["total"]))
    summary.add_row("Open cards", str(payload["cards"]["open"]))
    summary.add_row("Open critical cards", str(payload["cards"]["critical_open"]))
    summary.add_row("Blocking cards in scope", str(len(blockers)))
    console.print(summary)

    if blockers:
        table = Table(title="Blocking Live-Review Cards")
        table.add_column("Card", style="red")
        table.add_column("Task")
        table.add_column("Summary")
        for card in blockers:
            table.add_row(card["id"], card.get("task_id") or "(unscoped)", card["summary"])
        console.print(table)

    if payload["pending_signal_items"]:
        table = Table(title="Pending Agent Responses")
        table.add_column("Client", style="cyan")
        table.add_column("Task")
        table.add_column("Transcript", overflow="fold")
        table.add_column("Review Command", overflow="fold")
        for signal in payload["pending_signal_items"]:
            table.add_row(
                signal.get("client") or "",
                signal.get("task_id") or "(unscoped)",
                signal.get("transcript_path") or "",
                signal.get("review_command") or "",
            )
        console.print(table)
        commands = [signal.get("review_command") for signal in payload["pending_signal_items"] if signal.get("review_command")]
        if commands:
            console.print("[cyan]Pending review commands:[/cyan]")
            for command in commands:
                console.print(command)


@app.command("resolve")
def resolve(
    card_id: str = typer.Argument(..., help="Critique card ID to update."),
    status: str = typer.Option("resolved", "--status", help="New status: resolved or ignored."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Mark a critique card resolved or ignored after the concern has been handled."""
    if status not in {"resolved", "ignored"}:
        message = "--status must be resolved or ignored."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=2)
    root = project_root.expanduser().resolve()
    card = update_card_status(root, card_id, status)  # type: ignore[arg-type]
    if card is None:
        message = f"Critique card {card_id} not found."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=1)
    if json_format:
        typer.echo(dump_json({"ok": True, "card": card.model_dump()}, indent=2))
        _log_card_resolved(root, card)
        return
    _log_card_resolved(root, card)
    console.print(f"[green]Updated {card.id}:[/green] {card.status}")


@app.command("repair")
def repair(
    card_id: str = typer.Argument(..., help="Critique card ID to turn into a repair prompt."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Generate a ready-to-paste repair prompt for a critique card."""
    root = project_root.expanduser().resolve()
    card = get_card(root, card_id)
    if card is None:
        message = f"Critique card {card_id} not found."
        if json_format:
            typer.echo(dump_json({"ok": False, "error": message}, indent=2))
        else:
            console.print(f"[red]{message}[/red]")
        raise typer.Exit(code=1)
    prompt = build_live_repair_prompt(root, card)
    if json_format:
        typer.echo(dump_json({"ok": True, "card": card.model_dump(), "prompt": prompt}, indent=2))
        return
    typer.echo(prompt)


@app.command("repair-all")
def repair_all(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    task_id: str | None = typer.Option(None, "--task-id", help="Task scope for blocking-card repair prompts."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Generate repair prompts for all blocking live-review cards in scope."""
    root = project_root.expanduser().resolve()
    summary = live_review_summary(root, task_id=task_id)
    all_cards = {card.id: card for card in load_cards(root)}
    # Guard the membership test with isinstance(str): a malformed (non-hashable) id would
    # otherwise raise TypeError on `in`, whereas the old per-id lookup just skipped it.
    cards = [
        all_cards[item["id"]]
        for item in summary["blocking_cards"]
        if isinstance(item.get("id"), str) and item["id"] in all_cards
    ]
    resolved_cards = [card for card in cards if card is not None]
    prompt = build_bulk_live_repair_prompt(root, resolved_cards)
    if json_format:
        typer.echo(dump_json({
            "ok": True,
            "scope_task_id": summary["scope_task_id"],
            "cards": [card.model_dump() for card in resolved_cards],
            "prompt": prompt,
        }, indent=2))
        return
    typer.echo(prompt)


@app.command("signals")
def signals(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    client: str | None = typer.Option(None, "--client", help="Filter by client."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """List response-ready hook signals waiting for review."""
    root = project_root.expanduser().resolve()
    found = _filtered_signals(root, client)
    if json_format:
        typer.echo(dump_json({"signals": [item.model_dump() for item in found]}, indent=2))
        return
    table = Table(title="DevCouncil Watch Signals")
    table.add_column("Client", style="cyan")
    table.add_column("Transcript")
    table.add_column("Command")
    table.add_column("Signal")
    for item in found:
        table.add_row(item.client, item.transcript_path or "", item.review_command or "", item.path or "")
    console.print(table)


@app.command("pending")
def pending(
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    client: str | None = typer.Option(None, "--client", help="Filter by client."),
    task_id: str | None = typer.Option(None, "--task-id", help="Associate reviewed pending signals with a DevCouncil task."),
    llm: bool = typer.Option(False, "--llm", help="Use the configured live_reviewer or implementation_reviewer model."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
    keep: bool = typer.Option(False, "--keep", help="Keep signals pending after successful review."),
    force: bool = typer.Option(False, "--force", help="Regenerate and overwrite existing cards for reviewed turns."),
):
    """Review every pending response-ready signal that includes a transcript path."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    logger.info("dev watch pending: reviewing signals (client=%s, llm=%s)", client or "all", llm)
    fallback_task_id = active_task_id(root)
    reviewed = []
    skipped = []
    for signal in _filtered_signals(root, client):
        transcript_path = _resolve_signal_transcript(root, signal)
        if transcript_path is None:
            skipped.append({"signal": signal.model_dump(), "reason": "No transcript path in signal payload."})
            continue
        turn = latest_assistant_turn(transcript_path, client=signal.client)
        if turn is None:
            skipped.append({"signal": signal.model_dump(), "reason": f"No assistant turn found in {transcript_path}."})
            continue
        scoped_task_id = task_id or signal.task_id or fallback_task_id
        card = asyncio.run(_review_turn(turn, root, signal.client, llm, task_id=scoped_task_id))
        saved_path, duplicate = _save_card_once(root, card, persist=True, force=force)
        if saved_path:
            _log_card_reviewed(root, card, saved_path, duplicate=duplicate, source="pending")
        if not keep:
            processed_path = mark_processed(signal, root)
            _log_signal_processed(root, signal, processed_path, card)
        reviewed.append({
            "card": card.model_dump(),
            "path": str(saved_path) if saved_path else None,
            "duplicate": duplicate,
            "signal": signal.model_dump(),
        })
        if not json_format:
            _print_card(card)
            if duplicate:
                console.print(f"[yellow]Already reviewed:[/yellow] {saved_path}")
            else:
                console.print(f"[green]Saved critique card:[/green] {saved_path}")

    logger.info("dev watch pending complete: %d reviewed, %d skipped", len(reviewed), len(skipped))
    if json_format:
        typer.echo(dump_json({"reviewed": reviewed, "skipped": skipped}, indent=2))
        return
    for item in skipped:
        console.print(f"[yellow]Skipped signal:[/yellow] {item['reason']}")


@app.command("follow")
def follow(
    transcript: Path | None = typer.Option(None, "--transcript", "-t", help="JSONL transcript to poll."),
    session: str | None = typer.Option(None, "--session", help="Discovered session ID to follow, or 'latest'."),
    latest: bool = typer.Option(False, "--latest", help="Follow the most recently updated discovered session."),
    client: str = typer.Option("generic", "--client", help="Agent client: claude, codex, gemini, cursor, or generic."),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
    interval: float = typer.Option(2.0, "--interval", help="Polling interval in seconds."),
    llm: bool = typer.Option(False, "--llm", help="Use the configured live_reviewer or implementation_reviewer model."),
    once: bool = typer.Option(False, "--once", help="Review once and exit after a new assistant turn is found."),
    force: bool = typer.Option(False, "--force", help="Regenerate and overwrite an existing card for this turn."),
    task_id: str | None = typer.Option(None, "--task-id", help="Associate critique cards with a DevCouncil task."),
):
    """Poll a transcript and emit a critique card whenever the latest assistant turn changes."""
    root = project_root.expanduser().resolve()
    from devcouncil.telemetry.logging_setup import set_log_dir
    set_log_dir(root)
    transcript_path = _resolve_transcript(root, client, transcript=transcript, session=session, latest=latest)
    if transcript_path is None:
        logger.warning("dev watch follow: no transcript selected")
        console.print("[red]No transcript selected. Use --transcript, --session, or --latest.[/red]")
        raise typer.Exit(code=2)
    seen_turn_id: str | None = None
    logger.info("dev watch follow: watching %s (client=%s, interval=%ss, llm=%s)", transcript_path, client, interval, llm)
    console.print(f"[cyan]Watching transcript:[/cyan] {transcript_path}")
    while True:
        turn = latest_assistant_turn(transcript_path, client=client)
        if turn and turn.turn_id != seen_turn_id:
            seen_turn_id = turn.turn_id
            scoped_task_id = task_id or active_task_id(root)
            card = asyncio.run(_review_turn(turn, root, client, llm, task_id=scoped_task_id))
            logger.info("dev watch follow: new turn %s → card %s verdict=%s", turn.turn_id, card.id, card.verdict)
            saved_path, duplicate = _save_card_once(root, card, persist=True, force=force)
            if saved_path:
                _log_card_reviewed(root, card, saved_path, duplicate=duplicate, source="follow")
            _print_card(card)
            if duplicate:
                console.print(f"[yellow]Already reviewed:[/yellow] {saved_path}")
            else:
                console.print(f"[green]Saved critique card:[/green] {saved_path}")
            if once:
                return
        if once:
            console.print("[yellow]No new assistant turn found.[/yellow]")
            raise typer.Exit(code=1)
        time.sleep(interval)


@app.command("import")
def import_transcript(
    transcript: Path = typer.Argument(..., help="Transcript JSONL file to normalize."),
    client: str = typer.Option("generic", "--client", help="Agent client label."),
    json_format: bool = typer.Option(False, "--json", help="Output machine-readable JSON."),
):
    """Normalize a transcript into DevCouncil turn records without reviewing it."""
    turns = load_turns(transcript.expanduser().resolve(), client=client)
    if json_format:
        typer.echo(dump_json({"turns": [turn.model_dump() for turn in turns]}, indent=2))
        return
    console.print(f"Loaded {len(turns)} turns from {transcript}")


def _print_card(card) -> None:
    color = "green" if card.verdict == "Approved" else "yellow"
    if card.verdict == "Critical Issues":
        color = "red"
    body = [
        f"[bold]Verdict:[/bold] [{color}]{card.verdict}[/{color}]",
        f"[bold]Status:[/bold] {card.status}",
        f"[bold]Task:[/bold] {card.task_id or '(unscoped)'}",
        card.summary,
    ]
    if card.concerns:
        body.append("\n[bold]Concerns[/bold]")
        body.extend(f"- {item}" for item in card.concerns)
    if card.alternatives:
        body.append("\n[bold]Alternatives[/bold]")
        body.extend(f"- {item}" for item in card.alternatives)
    if card.evidence_requests:
        body.append("\n[bold]Evidence Requests[/bold]")
        body.extend(f"- {item}" for item in card.evidence_requests)
    body.append(f"\n[bold]Message for agent:[/bold] {card.message_for_agent}")
    console.print(Panel("\n".join(body), title=f"Critique Card {card.id}"))


def _filtered_signals(root: Path, client: str | None) -> list[ReviewSignal]:
    found = load_signals(root)
    if client is None:
        return found
    normalized = client.lower()
    return [item for item in found if item.client == normalized]


def _resolve_signal_transcript(root: Path, signal: ReviewSignal) -> Path | None:
    if not signal.transcript_path:
        return None
    path = Path(signal.transcript_path).expanduser()
    if not path.is_absolute():
        path = root / path
    return path.resolve()


def _resolve_transcript(
    root: Path,
    client: str,
    transcript: Path | None = None,
    session: str | None = None,
    latest: bool = False,
    task_id: str | None = None,
) -> Path | None:
    if transcript is not None:
        path = transcript.expanduser()
        if not path.is_absolute():
            path = root / path
        return path.resolve()

    if client.lower() == "claude" and task_id:
        from devcouncil.live.transcripts import claude_transcript_for_task

        pinned = claude_transcript_for_task(root, task_id)
        if pinned is not None:
            return pinned

    selector = "latest" if latest else session
    if selector is None:
        return None

    discovered = discover_sessions(root, client=client)
    if not discovered:
        return None
    if selector == "latest":
        return Path(discovered[0].transcript_path).expanduser().resolve()
    for item in discovered:
        if item.id == selector:
            return Path(item.transcript_path).expanduser().resolve()
    return None


def _save_card_once(root: Path, card, persist: bool, force: bool) -> tuple[Path | None, bool]:
    if not persist:
        return None, False
    path = card_path(root, card.id)
    if path.exists() and not force:
        return path, True
    if path.exists() and force:
        existing_cards = [item for item in load_cards(root) if item.id == card.id]
        if existing_cards:
            card = card.model_copy(update={"status": existing_cards[0].status})
    return save_card(root, card), False


def _log_card_reviewed(root: Path, card, path: Path, *, duplicate: bool, source: str) -> None:
    event_type = "live_review_card_reused" if duplicate else "live_review_card_saved"
    TraceLogger(root).log_event(
        event_type,
        {
            "card_id": card.id,
            "client": card.client,
            "verdict": card.verdict,
            "status": card.status,
            "path": str(path),
            "source": source,
            "duplicate": duplicate,
        },
        task_id=card.task_id,
        summary=f"Live-review card {card.id} {'reused' if duplicate else 'saved'} from {source}.",
    )


def _log_card_resolved(root: Path, card) -> None:
    TraceLogger(root).log_event(
        "live_review_card_status_updated",
        {
            "card_id": card.id,
            "client": card.client,
            "verdict": card.verdict,
            "status": card.status,
        },
        task_id=card.task_id,
        summary=f"Live-review card {card.id} marked {card.status}.",
    )


def _log_signal_processed(root: Path, signal: ReviewSignal, processed_path: Path | None, card) -> None:
    TraceLogger(root).log_event(
        "live_review_signal_processed",
        {
            "client": signal.client,
            "signal_path": signal.path,
            "processed_path": str(processed_path) if processed_path else None,
            "transcript_path": signal.transcript_path,
            "card_id": card.id,
        },
        task_id=card.task_id or signal.task_id,
        summary=f"Live-review signal processed into card {card.id}.",
    )


async def _review_turn(turn, root: Path, client: str, use_llm: bool, task_id: str | None = None):
    if not use_llm:
        # Pass task_id so the deterministic review can ground completion/evidence
        # claims against the task's real verification state.
        return review_turn(turn, root, client=client, task_id=task_id)
    try:
        config = load_config(root)
        validate_model_provider(config.models.provider)
        api_key = get_api_key(config.models.provider, root)
        provider = create_provider(config.models.provider, api_key, project_root=root, provider_prefs=config.provider)
        role_config = {name: role.model_dump() for name, role in config.models.roles.items()}
        router = ModelRouter(provider, role_config, project_root=root)
    except Exception as exc:
        logger.warning("Model-backed live review unavailable; using deterministic card: %s", exc)
        console.print(f"[yellow]Model-backed review unavailable; using deterministic card: {exc}[/yellow]")
        return review_turn(turn, root, client=client, task_id=task_id)
    # task_id flows through so the LLM review is grounded in the task's recorded
    # verification state (like the deterministic path), instead of judging the
    # agent's prose in a vacuum.
    card = await LiveReviewService(router).review(turn, root, client=client, use_llm=True, task_id=task_id)
    return card.model_copy(update={"task_id": task_id}) if task_id else card
