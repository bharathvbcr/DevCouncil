"""DELIBERATELY UNINSTRUMENTED: this command's subject *is* the trace/log
stream. Emitting log records or trace events from here mutates what the user
asked to inspect (``log_step`` appends to the very traces.jsonl being tailed)
and pollutes the machine-readable JSONL/--json output consumers parse. An
introspection command must not modify what it introspects."""

import json
import time
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console

from devcouncil.telemetry.traces import read_trace_events, read_trace_events_since

app = typer.Typer(help="Inspect DevCouncil trace events.")
console = Console()


@app.command("tail")
def tail(
    follow: bool = typer.Option(
        False, "--follow/--no-follow", "-f", help="Continue polling for new events (default is a single shot)."
    ),
    limit: int = typer.Option(50, "--limit", "-n", help="Maximum events to print before following."),
    jsonl: bool = typer.Option(True, "--jsonl/--pretty", help="Print JSONL or compact text rows."),
    since: Optional[int] = typer.Option(
        None,
        "--since",
        help="Byte-offset cursor from a previous run; emit only events after it (stateless incremental polling).",
    ),
    json_summary: bool = typer.Option(
        False,
        "--json",
        help="Emit a single {events, next_cursor} JSON object (incremental mode). Implies --no-follow.",
    ),
    project_root: Path = typer.Option(Path("."), "--project-root", help="Repository root containing .devcouncil/."),
):
    """Print the DevCouncil trace JSONL stream for replay or debugging.

    With ``--since <cursor> --no-follow`` (or ``--json``) a single-shot supervising
    agent gets only the events appended after the cursor plus a ``next_cursor`` to
    pass back on the next poll, so each poll is O(new) rather than O(all).
    """
    root = project_root.expanduser().resolve()

    # Incremental cursor mode: any of --since / --json / explicit --no-follow.
    incremental = since is not None or json_summary
    if incremental:
        events, next_cursor = read_trace_events_since(root, since)
        if json_summary:
            typer.echo(
                json.dumps(
                    {
                        "events": [event.model_dump(by_alias=True) for event in events],
                        "next_cursor": next_cursor,
                    }
                )
            )
            return
        for event in events:
            if jsonl:
                typer.echo(event.model_dump_json())
            else:
                console.print(
                    f"{event.timestamp} {event.type} "
                    f"{event.task_id or '-'} {event.summary or json.dumps(event.details)}"
                )
        console.print(f"[dim]next_cursor: {next_cursor}[/dim]")
        return

    printed = 0

    def emit_new(start_index: int) -> int:
        events = list(read_trace_events(root))
        selected = events[start_index:]
        for event in selected:
            if jsonl:
                typer.echo(event.model_dump_json())
            else:
                console.print(
                    f"{event.timestamp} {event.type} "
                    f"{event.task_id or '-'} {event.summary or json.dumps(event.details)}"
                )
        return len(events)

    events = list(read_trace_events(root))
    start = max(0, len(events) - limit)
    printed = emit_new(start)

    while follow:
        time.sleep(1)
        printed = emit_new(printed)
