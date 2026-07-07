import asyncio
import logging

import typer

from devcouncil.telemetry.stages import log_stage, log_step

app = typer.Typer()
logger = logging.getLogger(__name__)


@app.callback(invoke_without_command=True)
def mcp_server(ctx: typer.Context):
    """
    Start the DevCouncil MCP server over stdio.
    """
    if ctx.invoked_subcommand is not None:
        return

    # Deferred import: the MCP SDK (and the server module's large import surface)
    # loads only when this command actually runs, not on every `dev` invocation —
    # main.py imports every command module even for `dev --help`.
    from devcouncil.integrations.mcp.server import run

    logger.info("dev mcp-server: starting stdio server")
    with log_stage("mcp_server"):
        log_step("mcp_server/1: initializing stdio transport", trace=True)
        asyncio.run(run())
