"""Registry-backed MCP debugger and runtime-trace tools."""

from __future__ import annotations

from pathlib import Path
from typing import Awaitable, Callable

from mcp.types import TextContent, Tool

from devcouncil.codeintel.debug.consent import require_debug_consent, set_debug_consent
from devcouncil.codeintel.debug.discovery import adapter_by_id, discover_adapters
from devcouncil.codeintel.debug.session import get_debug_manager
from devcouncil.codeintel.debug.tracing import NodeCpuProfileProvider, PythonTraceProvider, import_runtime_trace
from devcouncil.codeintel.service import canonical_project_root
from devcouncil.integrations.mcp.util import error_text, json_text

Handler = Callable[[Path, dict], Awaitable[list[TextContent]]]


def _schema(properties: dict, required: list[str] | None = None) -> dict:
    schema: dict = {"type": "object", "properties": {
        "projectPath": {"type": "string", "description": "Explicit repository path."},
        **properties,
    }}
    if required:
        schema["required"] = required
    return schema


def tools() -> list[Tool]:
    return [
        Tool(
            name="devcouncil_debug_discover",
            description=(
                "Discover installed DAP adapters after explicit one-time consent; "
                "returns paths, versions, hashes, and launch/attach support."
            ),
            inputSchema=_schema({"consent": {"type": "boolean", "default": False}}),
        ),
        Tool(
            name="devcouncil_debug_start",
            description="Launch or attach a capability-negotiated DAP session.",
            inputSchema=_schema({
                "adapterId": {"type": "string"},
                "adapterCommand": {"type": "array", "items": {"type": "string"}},
                "request": {"type": "string", "enum": ["launch", "attach"], "default": "launch"},
                "configuration": {"type": "object"},
                "initialBreakpoints": {
                    "type": "object",
                    "additionalProperties": {"type": "array", "items": {"type": "integer"}},
                },
                "timeout": {"type": "number", "minimum": 1, "maximum": 120, "default": 30},
            }),
        ),
        Tool(
            name="devcouncil_debug_breakpoints",
            description="Replace all breakpoints for one source in a DAP session.",
            inputSchema=_schema({
                "sessionId": {"type": "string"},
                "source": {"type": "string"},
                "lines": {"type": "array", "items": {"type": "integer"}},
            }, ["sessionId", "source", "lines"]),
        ),
        Tool(
            name="devcouncil_debug_control",
            description="Continue, pause, or step a DAP session.",
            inputSchema=_schema({
                "sessionId": {"type": "string"},
                "action": {"type": "string", "enum": ["continue", "pause", "next", "stepIn", "stepOut"]},
                "threadId": {"type": "integer"},
            }, ["sessionId", "action"]),
        ),
        Tool(
            name="devcouncil_debug_inspect",
            description="Inspect threads, stack frames, scopes, variables, source, or disassembly.",
            inputSchema=_schema({
                "sessionId": {"type": "string"},
                "operation": {"type": "string", "enum": ["threads", "stackTrace", "scopes", "variables", "source", "disassemble"]},
                "arguments": {"type": "object"},
            }, ["sessionId", "operation"]),
        ),
        Tool(
            name="devcouncil_debug_evaluate",
            description="Side-effectful DAP evaluate; allowSideEffects must be explicitly true.",
            inputSchema=_schema({
                "sessionId": {"type": "string"},
                "expression": {"type": "string"},
                "frameId": {"type": "integer"},
                "allowSideEffects": {"type": "boolean", "const": True},
            }, ["sessionId", "expression", "allowSideEffects"]),
        ),
        Tool(
            name="devcouncil_debug_trace",
            description="Capture a DAP stack, run exact Python tracing, or import JSONL/Node CPU profile evidence.",
            inputSchema=_schema({
                "provider": {"type": "string", "enum": ["dap-stack", "python", "node", "import"]},
                "sessionId": {"type": "string"},
                "threadId": {"type": "integer"},
                "script": {"type": "string"},
                "args": {"type": "array", "items": {"type": "string"}},
                "path": {"type": "string"},
            }, ["provider"]),
        ),
        Tool(
            name="devcouncil_debug_stop",
            description="Disconnect a DAP session and optionally leave the debuggee running.",
            inputSchema=_schema({
                "sessionId": {"type": "string"},
                "terminateDebuggee": {"type": "boolean", "default": True},
            }, ["sessionId"]),
        ),
    ]


async def _discover(root: Path, arguments: dict) -> list[TextContent]:
    if arguments.get("consent") is True:
        set_debug_consent(root, True)
    require_debug_consent(root)
    return json_text({"consent": True, "adapters": [adapter.as_dict() for adapter in discover_adapters()]})


def _command(arguments: dict) -> list[str]:
    supplied = arguments.get("adapterCommand")
    if isinstance(supplied, list) and supplied:
        return [str(value) for value in supplied]
    adapter_id = str(arguments.get("adapterId") or "debugpy")
    adapter = adapter_by_id(adapter_id)
    if adapter is None:
        raise ValueError(f"adapter {adapter_id!r} was not discovered")
    return list(adapter.command)


async def _start(root: Path, arguments: dict) -> list[TextContent]:
    require_debug_consent(root)
    session = get_debug_manager().start(
        root,
        _command(arguments),
        request=str(arguments.get("request", "launch")),
        arguments=dict(arguments.get("configuration") or {}),
        initial_breakpoints={
            str(source): [int(line) for line in lines]
            for source, lines in dict(arguments.get("initialBreakpoints") or {}).items()
        },
        timeout=float(arguments.get("timeout", 30.0)),
    )
    return json_text(session.as_dict())


async def _breakpoints(root: Path, arguments: dict) -> list[TextContent]:
    require_debug_consent(root)
    return json_text(get_debug_manager().set_breakpoints(
        str(arguments["sessionId"]), str(arguments["source"]), [int(value) for value in arguments["lines"]]
    ))


async def _control(root: Path, arguments: dict) -> list[TextContent]:
    require_debug_consent(root)
    return json_text(get_debug_manager().control(
        str(arguments["sessionId"]),
        str(arguments["action"]),
        thread_id=int(arguments["threadId"]) if arguments.get("threadId") is not None else None,
    ))


async def _inspect(root: Path, arguments: dict) -> list[TextContent]:
    require_debug_consent(root)
    return json_text(get_debug_manager().inspect(
        str(arguments["sessionId"]), str(arguments["operation"]), dict(arguments.get("arguments") or {})
    ))


async def _evaluate(root: Path, arguments: dict) -> list[TextContent]:
    require_debug_consent(root)
    if arguments.get("allowSideEffects") is not True:
        raise PermissionError("debug evaluate requires allowSideEffects=true")
    return json_text(get_debug_manager().evaluate(
        str(arguments["sessionId"]),
        str(arguments["expression"]),
        frame_id=int(arguments["frameId"]) if arguments.get("frameId") is not None else None,
        allow_side_effects=True,
    ))


async def _trace(root: Path, arguments: dict) -> list[TextContent]:
    require_debug_consent(root)
    provider = str(arguments["provider"])
    if provider == "dap-stack":
        return json_text(get_debug_manager().capture_stack(
            str(arguments["sessionId"]), thread_id=int(arguments["threadId"])
        ))
    if provider == "python":
        return json_text(PythonTraceProvider(root).run(
            Path(str(arguments["script"])), [str(value) for value in arguments.get("args") or []]
        ))
    if provider == "node":
        return json_text(NodeCpuProfileProvider(root).run(
            Path(str(arguments["script"])), [str(value) for value in arguments.get("args") or []]
        ))
    if provider == "import":
        return json_text(import_runtime_trace(root, Path(str(arguments["path"]))))
    raise ValueError(f"unsupported trace provider: {provider}")


async def _stop(root: Path, arguments: dict) -> list[TextContent]:
    require_debug_consent(root)
    session_id = str(arguments["sessionId"])
    get_debug_manager().stop(session_id, terminate_debuggee=bool(arguments.get("terminateDebuggee", True)))
    return json_text({"stopped": session_id})


REGISTRY: dict[str, Handler] = {
    "devcouncil_debug_discover": _discover,
    "devcouncil_debug_start": _start,
    "devcouncil_debug_breakpoints": _breakpoints,
    "devcouncil_debug_control": _control,
    "devcouncil_debug_inspect": _inspect,
    "devcouncil_debug_evaluate": _evaluate,
    "devcouncil_debug_trace": _trace,
    "devcouncil_debug_stop": _stop,
}


async def dispatch(name: str, default_root: Path, arguments: dict) -> list[TextContent] | None:
    handler = REGISTRY.get(name)
    if handler is None:
        return None
    explicit = arguments.get("projectPath")
    root = canonical_project_root(Path(explicit) if isinstance(explicit, str) and explicit else default_root)
    try:
        return await handler(root, arguments)
    except PermissionError as exc:
        return error_text(str(exc), code="debug_consent_required", tool=name)
    except FileNotFoundError as exc:
        return error_text(str(exc), code="not_found", tool=name)
    except (KeyError, TypeError, ValueError, RuntimeError, TimeoutError) as exc:
        return error_text(str(exc), code="debug_error", tool=name)
