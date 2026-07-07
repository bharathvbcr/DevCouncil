"""Run trace and timeline MCP tool handlers."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any

from mcp.types import TextContent

from devcouncil.integrations.mcp.util import (
    error_text,
    int_argument,
    json_text,
    required_string_argument,
)
from devcouncil.telemetry.traces import read_trace_events


async def handle_tail_trace(root: Path, arguments: dict) -> list[TextContent]:
    limit = int_argument(arguments, "limit", 20, minimum=1, maximum=200)
    events = list(read_trace_events(root))[-limit:]
    return json_text({"events": [event.model_dump(by_alias=True) for event in events]})


async def handle_run_timeline(root: Path, arguments: dict) -> list[TextContent]:
    from devcouncil.execution.run_trace import load_timeline

    reference, arg_error = required_string_argument(arguments, "reference")
    if arg_error:
        return arg_error
    assert reference is not None
    limit = int_argument(arguments, "limit", 40, minimum=1, maximum=500)
    try:
        tl = load_timeline(root, reference, event_limit=limit)
    except ValueError as exc:
        return error_text(str(exc), code="not_found", reference=reference)
    return json_text({"ok": True, **tl.model_dump(mode="json")})


async def handle_run_supervise(
    root: Path,
    arguments: dict,
    *,
    load_router: Callable[[Path], Any],
) -> list[TextContent]:
    from devcouncil.execution.run_trace import load_timeline, supervise_run

    reference, arg_error = required_string_argument(arguments, "reference")
    if arg_error:
        return arg_error
    assert reference is not None
    try:
        tl = load_timeline(root, reference)
    except ValueError as exc:
        return error_text(str(exc), code="not_found", reference=reference)
    verdict = await supervise_run(root, tl, load_router(root))
    return json_text({
        "ok": True,
        "reference": reference,
        "run_id": tl.run_id,
        "task_id": tl.task_id,
        "reversible": tl.reversible,
        **verdict.model_dump(mode="json"),
    })
