import asyncio

import typer

from devcouncil.integrations.mcp.server import run

app = typer.Typer()


@app.callback(invoke_without_command=True)
def mcp_server(ctx: typer.Context):
    """
    Start the DevCouncil MCP server over stdio.
    """
    if ctx.invoked_subcommand is not None:
        return

    asyncio.run(run())
