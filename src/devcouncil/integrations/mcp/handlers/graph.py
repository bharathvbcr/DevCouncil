"""Code-review graph context MCP tool handlers."""

from __future__ import annotations

import asyncio
from pathlib import Path

from mcp.types import TextContent


async def handle_graph_context(root: Path, arguments: dict) -> list[TextContent]:
    """Answer in-process.

    This used to shell out to ``python -m devcouncil graph-context --json`` and
    keep an identical in-process call as its fallback. The CLI command's entire
    body (:func:`devcouncil.cli.commands.map.graph_context_cmd`) is the same
    ``CodeReviewGraphAdapter(root).get_context(files)`` and the same
    ``model_dump_json(indent=2)``, so the subprocess bought a second Python
    interpreter and ~0.58s of imports to compute a value this process already
    had. The two-branch shape also discarded the CLI's error, leaving a
    subprocess failure and a genuine negative answer indistinguishable — the
    adapter's own ``available``/``summary`` is the honest report either way.

    The adapter is synchronous and reads the code graph off disk, so it runs in
    a worker thread rather than on the asyncio event loop: a parked loop stops
    answering ``ping`` and cannot receive the ``notifications/cancelled`` a
    client sends to give up, which is the reasoning ``util.run_cli_command``
    already documents for the CLI helper. Only where the work runs changes.
    """
    from devcouncil.integrations.code_review_graph import CodeReviewGraphAdapter

    files = arguments.get("files", [])
    if not isinstance(files, list):
        files = []
    wanted = [file for file in files if isinstance(file, str) and file]
    context = await asyncio.to_thread(
        lambda: CodeReviewGraphAdapter(root.expanduser().resolve()).get_context(wanted)
    )
    return [TextContent(type="text", text=context.model_dump_json(indent=2))]
