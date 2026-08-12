import asyncio
import logging
import os
from contextlib import asynccontextmanager
from functools import lru_cache
from pathlib import Path

import jsonschema
from mcp.server import Server, ServerRequestContext
from mcp.server.stdio import stdio_server
from mcp.types import (
    CallToolRequestParams,
    CallToolResult,
    GetPromptRequestParams,
    GetPromptResult,
    ListPromptsResult,
    ListResourcesResult,
    ListToolsResult,
    PaginatedRequestParams,
    Prompt,
    ReadResourceRequestParams,
    ReadResourceResult,
    Resource,
    TextContent,
    TextResourceContents,
    Tool,
)

from devcouncil.integrations.check import integration_status_summary
from devcouncil.integrations.mcp.handlers import ast_lsp as ast_lsp_handlers
from devcouncil.integrations.mcp.handlers import checkout as checkout_handlers
from devcouncil.integrations.mcp.handlers import codeintel as codeintel_handlers
from devcouncil.integrations.mcp.handlers import debug as debug_handlers
from devcouncil.integrations.mcp.handlers import cli_gate as cli_gate_handlers
from devcouncil.integrations.mcp.handlers import evidence as evidence_handlers
from devcouncil.integrations.mcp.handlers import git as git_handlers
from devcouncil.integrations.mcp.handlers import graph as graph_handlers
from devcouncil.integrations.mcp.handlers import handoff as handoff_handlers
from devcouncil.integrations.mcp.handlers import knowledge as knowledge_handlers
from devcouncil.integrations.mcp.handlers import live as live_handlers
from devcouncil.integrations.mcp.handlers import map as map_handlers
from devcouncil.integrations.mcp.handlers import next_task as next_task_handlers
from devcouncil.integrations.mcp.handlers import policy as policy_handlers
from devcouncil.integrations.mcp.handlers import prompts as prompt_handlers
from devcouncil.integrations.mcp.handlers import provenance as provenance_handlers
from devcouncil.integrations.mcp.handlers import read as read_handlers
from devcouncil.integrations.mcp.handlers import router_cache
from devcouncil.integrations.mcp.handlers import run as run_handlers
from devcouncil.integrations.mcp.handlers import runs as runs_handlers
from devcouncil.integrations.mcp.handlers import scope as scope_handlers
from devcouncil.integrations.mcp.handlers import status as status_handlers
from devcouncil.integrations.mcp.handlers import task as task_handlers
from devcouncil.integrations.mcp.handlers import tool_specs
from devcouncil.integrations.mcp.handlers import trace as trace_handlers
from devcouncil.integrations.mcp.handlers import verify as verify_handlers
from devcouncil.integrations.mcp.handlers import wiki as wiki_handlers
from devcouncil.integrations.mcp.handlers import write as write_handlers
from devcouncil.integrations.mcp.handlers import lease as lease_handlers
from devcouncil.integrations.mcp.util import (
    error_text as _error_text,
    json_text as _json_text,
    normalize_arguments as _normalize_arguments,
)
from devcouncil.integrations.mcp import util as _mcp_util
from devcouncil.storage.db import get_db
from devcouncil.telemetry.stages import log_step

# Re-exported for tests and backward compatibility.
_CLI_OUTPUT_LIMIT = _mcp_util._CLI_OUTPUT_LIMIT
_CLI_TIMEOUT_SECONDS = _mcp_util._CLI_TIMEOUT_SECONDS
_allowed_next_tools = _mcp_util.allowed_next_tools

logger = logging.getLogger(__name__)


@asynccontextmanager
async def _lifespan(_server):  # noqa: ANN001
    """Keep one native watcher alive for the MCP process lifecycle."""
    coordinator = None
    root = _project_root().expanduser().resolve()
    try:
        from devcouncil.app.config import load_config
        from devcouncil.codeintel import get_codeintel_service
        from devcouncil.codeintel.sync import get_sync_coordinator

        config = load_config(root).code_intelligence
        service = get_codeintel_service(root)
        graph_export = root / ".devcouncil" / "graph" / "code_graph.json"
        if config.enabled and config.auto_sync and (service.store.exists() or graph_export.is_file()):
            coordinator = get_sync_coordinator(
                root,
                debounce_seconds=config.debounce_ms / 1000.0,
                reconcile_seconds=float(config.reconcile_seconds),
                allow_polling_fallback=config.allow_polling_fallback,
            )
            coordinator.start()
    except Exception:
        logger.warning("MCP code-intelligence watcher did not start", exc_info=True)
    try:
        yield {"codeintel": coordinator}
    finally:
        if coordinator is not None:
            coordinator.stop()


_DB_REQUIRED_TOOLS = {
    "devcouncil_status",
    "devcouncil_report",
    "devcouncil_get_task",
    "devcouncil_list_tasks",
    "devcouncil_get_gaps",
    "devcouncil_get_next_actions",
    "devcouncil_get_task_provenance",
    "devcouncil_list_leases",
    "devcouncil_renew_lease",
    "devcouncil_get_prompt",
    "devcouncil_tail_trace",
    "devcouncil_policy_check_write",
    "devcouncil_graph_context",
    "devcouncil_prepare_execution",
    "devcouncil_checkout_task",
    "devcouncil_release_task",
    "devcouncil_update_task_scope",
    "devcouncil_append_evidence",
    "devcouncil_record_command",
    "devcouncil_write_file",
    "devcouncil_apply_patch",
    "devcouncil_verify_task",
    "devcouncil_handoff_agent",
    "devcouncil_get_evidence",
    "devcouncil_run_command",
    "devcouncil_next_task",
}


def _reset_caches() -> None:
    """Drop all per-root MCP caches. Re-exported for test isolation."""
    router_cache.reset_caches()
    _tool_input_schemas.cache_clear()


def _project_root() -> Path:
    configured = os.environ.get("DEVCOUNCIL_PROJECT_ROOT")
    return Path(configured).expanduser().resolve() if configured else Path(".")


async def list_tools() -> list[Tool]:
    return tool_specs.all_tools()


async def list_resources() -> list[Resource]:
    return await provenance_handlers.list_resources(_project_root())


async def read_resource(uri: str) -> str:
    return await provenance_handlers.read_resource(_project_root(), uri)


async def list_prompts() -> list[Prompt]:
    return prompt_handlers.list_prompts()


async def get_prompt(name: str, arguments: dict | None) -> GetPromptResult:
    return prompt_handlers.get_prompt(name, arguments, _project_root())


async def call_tool(name: str, arguments: dict) -> list[TextContent]:
    arguments = _normalize_arguments(arguments)
    root = _project_root()
    log_step(f"mcp/{name}: invoked", project_root=root)
    logger.info("MCP call_tool: %s args=%s", name, sorted(arguments) if isinstance(arguments, dict) else arguments)
    db = get_db(root)
    if name in _DB_REQUIRED_TOOLS and not db:
        logger.warning("MCP tool %s rejected: project not initialized at %s", name, root)
        return _error_text("DevCouncil not initialized in this directory.", code="not_initialized")

    registered = await codeintel_handlers.dispatch(name, root, arguments)
    if registered is not None:
        return registered
    registered = await debug_handlers.dispatch(name, root, arguments)
    if registered is not None:
        return registered

    if name == "devcouncil_integration_status":
        return _json_text(integration_status_summary(root))

    if name == "devcouncil_status":
        assert db is not None
        return await status_handlers.handle_status(root, db, arguments)

    if name == "devcouncil_report":
        assert db is not None
        return await status_handlers.handle_report(root, db, arguments)

    if name == "devcouncil_live_review":
        return await live_handlers.handle_live_review(root, arguments)

    if name == "devcouncil_live_cards":
        return await live_handlers.handle_live_cards(root, arguments)

    if name == "devcouncil_live_repair_prompt":
        return await live_handlers.handle_live_repair_prompt(root, arguments)

    if name == "devcouncil_live_repair_all":
        return await live_handlers.handle_live_repair_all(root, arguments)

    if name == "devcouncil_get_task":
        assert db is not None
        return await task_handlers.handle_get_task(root, db, arguments)

    if name == "devcouncil_get_gaps":
        assert db is not None
        return await status_handlers.handle_get_gaps(root, db, arguments)

    if name == "devcouncil_get_next_actions":
        assert db is not None
        return await status_handlers.handle_get_next_actions(root, db, arguments)

    if name == "devcouncil_get_task_provenance":
        assert db is not None
        return await provenance_handlers.handle_get_task_provenance(root, db, arguments)

    if name == "devcouncil_list_tasks":
        assert db is not None
        return await status_handlers.handle_list_tasks(root, db, arguments)

    if name == "devcouncil_get_prompt":
        assert db is not None
        return await task_handlers.handle_get_prompt(root, db, arguments)

    if name == "devcouncil_tail_trace":
        return await trace_handlers.handle_tail_trace(root, arguments)

    if name == "devcouncil_policy_check_write":
        assert db is not None
        return await policy_handlers.handle_policy_check_write(root, db, arguments)

    if name == "devcouncil_graph_context":
        return await graph_handlers.handle_graph_context(root, arguments)

    if name == "devcouncil_repo_map":
        return await map_handlers.handle_repo_map(root, arguments)

    if name == "devcouncil_impact":
        return await map_handlers.handle_impact(root, arguments)

    if name == "devcouncil_liveness":
        return await map_handlers.handle_liveness(root, arguments)

    if name == "devcouncil_graph_query":
        return await map_handlers.handle_graph_query(root, arguments)

    if name == "devcouncil_graph_trace":
        return await map_handlers.handle_graph_trace(root, arguments)

    if name == "devcouncil_graph_impact":
        return await map_handlers.handle_graph_impact(root, arguments)

    if name == "devcouncil_graph_ingest":
        return await map_handlers.handle_graph_ingest(root, arguments)

    if name == "devcouncil_graph_cypher":
        return await map_handlers.handle_graph_cypher(root, arguments)

    if name == "devcouncil_pdg_query":
        return await map_handlers.handle_pdg_query(root, arguments)

    if name == "devcouncil_explain":
        return await map_handlers.handle_explain(root, arguments)

    if name == "devcouncil_route_map":
        return await map_handlers.handle_route_map(root, arguments)

    if name == "devcouncil_shape_check":
        return await map_handlers.handle_shape_check(root, arguments)

    if name == "devcouncil_api_impact":
        return await map_handlers.handle_api_impact(root, arguments)

    if name == "devcouncil_lsp_status":
        return await ast_lsp_handlers.handle_lsp_status(root, arguments)

    if name == "devcouncil_ast_match":
        return await ast_lsp_handlers.handle_ast_match(root, arguments)

    if name == "devcouncil_cli":
        return await cli_gate_handlers.handle_cli(root, arguments)

    if name == "devcouncil_prepare_execution":
        assert db is not None
        return await task_handlers.handle_prepare_execution(root, db, arguments)

    if name == "devcouncil_checkout_task":
        assert db is not None
        return await checkout_handlers.handle_checkout_task(
            root, db, arguments, load_router=router_cache.load_router,
        )

    if name == "devcouncil_release_task":
        assert db is not None
        return await lease_handlers.handle_release_task(root, db, arguments)

    if name == "devcouncil_renew_lease":
        assert db is not None
        return await lease_handlers.handle_renew_lease(root, db, arguments)

    if name == "devcouncil_list_leases":
        assert db is not None
        return await lease_handlers.handle_list_leases(root, db, arguments)

    if name == "devcouncil_update_task_scope":
        assert db is not None
        return await scope_handlers.handle_update_task_scope(root, db, arguments)

    if name == "devcouncil_append_evidence":
        assert db is not None
        return await evidence_handlers.handle_append_evidence(root, db, arguments)

    if name == "devcouncil_record_command":
        assert db is not None
        return await policy_handlers.handle_record_command(root, db, arguments)

    if name == "devcouncil_write_file":
        assert db is not None
        return await write_handlers.handle_write_file(root, db, arguments)

    if name == "devcouncil_apply_patch":
        assert db is not None
        return await write_handlers.handle_apply_patch(root, db, arguments)

    if name == "devcouncil_verify_task":
        assert db is not None
        return await verify_handlers.handle_verify_task(
            root, db, arguments, load_router=router_cache.load_router,
        )

    if name == "devcouncil_handoff_agent":
        assert db is not None
        return await handoff_handlers.handle_handoff_agent(root, db, arguments)

    if name == "devcouncil_read_file":
        return await read_handlers.handle_read_file(root, arguments, db=db)

    if name == "devcouncil_get_diff":
        return await git_handlers.handle_get_diff(root, db, arguments)

    if name == "devcouncil_get_evidence":
        assert db is not None
        return await evidence_handlers.handle_get_evidence(root, db, arguments)

    if name == "devcouncil_run_command":
        assert db is not None
        return await run_handlers.handle_run_command(root, db, arguments)

    if name == "devcouncil_next_task":
        assert db is not None
        return await next_task_handlers.handle_next_task(root, db, arguments)

    if name == "devcouncil_list_agent_runs":
        return await runs_handlers.handle_list_agent_runs(root, arguments)

    if name == "devcouncil_get_run":
        return await runs_handlers.handle_get_run(root, arguments)

    if name == "devcouncil_select_knowledge":
        return await knowledge_handlers.handle_select_knowledge(root, arguments)

    if name == "devcouncil_wiki_page":
        return await wiki_handlers.handle_wiki_page(root, arguments)

    if name == "devcouncil_run_timeline":
        return await trace_handlers.handle_run_timeline(root, arguments)

    if name == "devcouncil_run_supervise":
        return await trace_handlers.handle_run_supervise(
            root, arguments, load_router=router_cache.load_router,
        )

    logger.warning("MCP unknown tool requested: %s", name)
    return _error_text(f"Unknown tool: {name}", code="unknown_tool", tool=name)


# --- mcp 2.x transport adapters -------------------------------------------------
# mcp 2.x replaced the ``@app.call_tool()`` decorators with ``on_*`` constructor
# callables that take ``(ctx, params)`` and return a full protocol result. The
# functions above stay the canonical implementation -- one owner per operation --
# and everything below only translates between that contract and the wire types.


@lru_cache(maxsize=1)
def _tool_input_schemas() -> dict[str, dict]:
    return {tool.name: tool.input_schema for tool in tool_specs.all_tools()}


def _input_schema_violation(name: str, arguments: dict) -> str | None:
    """Return a message when ``arguments`` violate the advertised inputSchema.

    mcp 2.x advertises inputSchema but never enforces it, so the check the v1
    ``call_tool`` decorator performed is reproduced here rather than dropped.
    Unknown tools are not rejected here: ``call_tool`` owns that response.
    """
    schema = _tool_input_schemas().get(name)
    if schema is None:
        return None
    try:
        jsonschema.validate(instance=arguments, schema=schema)
    except jsonschema.ValidationError as exc:
        return f"Input validation error: {exc.message}"
    return None


def _tool_error(message: str) -> CallToolResult:
    return CallToolResult(content=[TextContent(type="text", text=message)], is_error=True)


async def _on_list_tools(
    _ctx: ServerRequestContext, _params: PaginatedRequestParams | None
) -> ListToolsResult:
    return ListToolsResult(tools=await list_tools())


async def _on_list_resources(
    _ctx: ServerRequestContext, _params: PaginatedRequestParams | None
) -> ListResourcesResult:
    return ListResourcesResult(resources=await list_resources())


async def _on_read_resource(
    _ctx: ServerRequestContext, params: ReadResourceRequestParams
) -> ReadResourceResult:
    text = await read_resource(params.uri)
    return ReadResourceResult(
        contents=[TextResourceContents(uri=params.uri, text=text, mime_type="text/plain")],
    )


async def _on_list_prompts(
    _ctx: ServerRequestContext, _params: PaginatedRequestParams | None
) -> ListPromptsResult:
    return ListPromptsResult(prompts=await list_prompts())


async def _on_get_prompt(
    _ctx: ServerRequestContext, params: GetPromptRequestParams
) -> GetPromptResult:
    return await get_prompt(params.name, params.arguments)


async def _on_call_tool(
    _ctx: ServerRequestContext, params: CallToolRequestParams
) -> CallToolResult:
    """Validate, dispatch, and wrap a tool call for the mcp 2.x wire contract.

    mcp 2.x lets handler exceptions escape as JSON-RPC transport errors; v1
    turned them into ``isError`` results, and clients here still expect a
    result they can read, so the boundary keeps catching them.
    """
    arguments = _normalize_arguments(params.arguments)
    try:
        violation = _input_schema_violation(params.name, arguments)
        if violation is not None:
            return _tool_error(violation)
        contents = await call_tool(params.name, arguments)
    except Exception as exc:
        logger.exception("MCP tool %s raised", params.name)
        return _tool_error(str(exc))
    return CallToolResult(content=list(contents), is_error=False)


app = Server(
    "devcouncil",
    lifespan=_lifespan,
    on_list_tools=_on_list_tools,
    on_call_tool=_on_call_tool,
    on_list_resources=_on_list_resources,
    on_read_resource=_on_read_resource,
    on_list_prompts=_on_list_prompts,
    on_get_prompt=_on_get_prompt,
)


async def run():
    from devcouncil.telemetry.logging_setup import configure_logging, set_log_dir

    configure_logging()
    root = Path(os.environ.get("DEVCOUNCIL_PROJECT_ROOT", ".")).expanduser().resolve()
    set_log_dir(root)
    logger.info("MCP server starting (project_root=%s)", root)
    async with stdio_server() as (read_stream, write_stream):
        await app.run(read_stream, write_stream, app.create_initialization_options())


if __name__ == "__main__":
    asyncio.run(run())
