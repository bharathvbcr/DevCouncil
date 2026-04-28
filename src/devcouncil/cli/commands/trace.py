import json
import time
from pathlib import Path

import typer
from rich.console import Console

from devcouncil.telemetry.traces import read_trace_events

app = typer.Typer(help="Inspect DevCouncil trace events.")
console = Console()


@app.command("tail")
def tail(
    follow: bool = typer.Option(False, "--follow", "-f", help="Continue polling for new events."),
    limit: int = typer.Option(50, "--limit", "-n", help="Maximum events to print before following."),
    jsonl: bool = typer.Option(True, "--jsonl/--pretty", help="Print JSONL or compact text rows."),
):
    """Print the DevCouncil trace JSONL stream for replay or debugging."""
    project_root = Path(".")
    printed = 0

    def emit_new(start_index: int) -> int:
        events = list(read_trace_events(project_root))
        selected = events[start_index:]
        for event in selected:
            if jsonl:
                console.print(event.model_dump_json())
            else:
                console.print(
                    f"{event.timestamp} {event.type} "
                    f"{event.task_id or '-'} {event.summary or json.dumps(event.details)}"
                )
        return len(events)

    events = list(read_trace_events(project_root))
    start = max(0, len(events) - limit)
    printed = emit_new(start)

    while follow:
        time.sleep(1)
        printed = emit_new(printed)
