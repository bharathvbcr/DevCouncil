"""Registry-backed MCP debugger and runtime-trace tools."""

from __future__ import annotations

from pathlib import Path
from typing import Awaitable, Callable

from mcp.types import TextContent, Tool

from devcouncil.codeintel.debug.consent import debug_consent_enabled, require_debug_consent
from devcouncil.codeintel.debug.discovery import adapter_by_id, discover_adapters
from devcouncil.codeintel.debug.session import get_debug_manager
from devcouncil.codeintel.debug.tracing import NodeCpuProfileProvider, PythonTraceProvider, import_runtime_trace
from devcouncil.integrations.mcp.handlers.codeintel import ProjectPathOutsideRoot, resolve_root
from devcouncil.integrations.mcp.util import error_text, json_text, within_root

Handler = Callable[[Path, dict], Awaitable[list[TextContent]]]


class DebugPathOutsideRoot(ValueError):
    """A debug path argument that resolves outside the server's project root."""


def _contained(root: Path, arguments: dict, name: str) -> Path:
    """The one containment rule every debug *path* argument passes through.

    `script`, `path` and `source` are filenames a caller supplies and the
    debugger then executes, reads or breakpoints. `_trace` handed `script`
    straight to a provider whose first line is
    ``script if script.is_absolute() else self.root / script`` — no `resolve()`,
    no containment — and from there to `subprocess.run`, so
    ``{"provider": "python", "script": "/tmp/x.py"}`` executed an arbitrary file
    outside the project under the project's interpreter. `resolve_root` only
    ever constrained `projectPath`.

    Deliberately the *same* owner `projectPath` uses one layer up
    (`within_root`), not a second copy of the rule: symlinks are followed before
    the verdict, absolute paths outside the root and `../` traversal are both
    refused, and a relative path is taken against the root rather than the
    server process's working directory.
    """
    raw = arguments.get(name)
    if not isinstance(raw, str) or not raw.strip():
        raise KeyError(name)
    resolved = within_root(root, raw)
    if resolved is None:
        raise DebugPathOutsideRoot(
            f"{name} {raw!r} resolves outside this server's project root {root}"
        )
    return resolved


def _schema(properties: dict, required: list[str] | None = None) -> dict:
    schema: dict = {"type": "object", "properties": {
        "projectPath": {"type": "string", "description": "Repository path inside the server root."},
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
            input_schema=_schema({"consent": {
                "type": "boolean",
                "default": False,
                "description": (
                    "Asks for a consent-state report only. Consent is granted by the user "
                    "via `dev debug discover --consent` or "
                    "code_intelligence.debug.auto_discover in .devcouncil/config.yaml; "
                    "this argument cannot grant it."
                ),
            }}),
        ),
        Tool(
            name="devcouncil_debug_start",
            description="Launch or attach a capability-negotiated DAP session.",
            input_schema=_schema({
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
            input_schema=_schema({
                "sessionId": {"type": "string"},
                "source": {"type": "string", "description": "Source file inside the server root."},
                "lines": {"type": "array", "items": {"type": "integer"}},
            }, ["sessionId", "source", "lines"]),
        ),
        Tool(
            name="devcouncil_debug_control",
            description="Continue, pause, or step a DAP session.",
            input_schema=_schema({
                "sessionId": {"type": "string"},
                "action": {"type": "string", "enum": ["continue", "pause", "next", "stepIn", "stepOut"]},
                "threadId": {"type": "integer"},
            }, ["sessionId", "action"]),
        ),
        Tool(
            name="devcouncil_debug_inspect",
            description="Inspect threads, stack frames, scopes, variables, source, or disassembly.",
            input_schema=_schema({
                "sessionId": {"type": "string"},
                "operation": {"type": "string", "enum": ["threads", "stackTrace", "scopes", "variables", "source", "disassemble"]},
                "arguments": {"type": "object"},
            }, ["sessionId", "operation"]),
        ),
        Tool(
            name="devcouncil_debug_evaluate",
            description="Side-effectful DAP evaluate; allowSideEffects must be explicitly true.",
            input_schema=_schema({
                "sessionId": {"type": "string"},
                "expression": {"type": "string"},
                "frameId": {"type": "integer"},
                "allowSideEffects": {"type": "boolean", "const": True},
            }, ["sessionId", "expression", "allowSideEffects"]),
        ),
        Tool(
            name="devcouncil_debug_trace",
            description="Capture a DAP stack, run exact Python tracing, or import JSONL/Node CPU profile evidence.",
            input_schema=_schema({
                "provider": {"type": "string", "enum": ["dap-stack", "python", "node", "import"]},
                "sessionId": {"type": "string"},
                "threadId": {"type": "integer"},
                "script": {"type": "string", "description": "Script to run, inside the server root."},
                "args": {"type": "array", "items": {"type": "string"}},
                "path": {"type": "string", "description": "Trace file to import, inside the server root."},
            }, ["provider"]),
        ),
        Tool(
            name="devcouncil_debug_stop",
            description="Disconnect a DAP session and optionally leave the debuggee running.",
            input_schema=_schema({
                "sessionId": {"type": "string"},
                "terminateDebuggee": {"type": "boolean", "default": True},
            }, ["sessionId"]),
        ),
    ]


async def _discover(root: Path, arguments: dict) -> list[TextContent]:
    """Report adapters once the *user* has consented — never grant that consent.

    This used to call `set_debug_consent(root, True)` when the caller passed
    ``{"consent": true}``, which writes ``code_intelligence.debug.auto_discover:
    true`` into `.devcouncil/config.yaml` and permanently unlocks the other
    seven debug tools — process launch, breakpoints, side-effectful evaluate and
    script execution among them. A capability gate a caller can set by passing
    an argument is not a gate; the argument now does the one thing an argument
    may do, which is say the gate is shut and name the two ways a human opens it.
    """
    if arguments.get("consent") is True and not debug_consent_enabled(root):
        raise PermissionError(
            "consent=true cannot grant debugger consent from a tool argument. "
            "Run `dev debug discover --consent` or set "
            "code_intelligence.debug.auto_discover: true in .devcouncil/config.yaml."
        )
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
    source = _contained(root, arguments, "source")
    return json_text(get_debug_manager().set_breakpoints(
        str(arguments["sessionId"]), str(source), [int(value) for value in arguments["lines"]]
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
            _contained(root, arguments, "script"),
            [str(value) for value in arguments.get("args") or []],
        ))
    if provider == "node":
        return json_text(NodeCpuProfileProvider(root).run(
            _contained(root, arguments, "script"),
            [str(value) for value in arguments.get("args") or []],
        ))
    if provider == "import":
        return json_text(import_runtime_trace(root, _contained(root, arguments, "path")))
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
    # One owner for "which root does this call act on": this used to be a second
    # copy of `resolve_root`'s body, and a containment check added to one copy
    # would have left the debugger — which launches processes and grants
    # per-root debug consent — reachable outside the server's root.
    try:
        root = resolve_root(default_root, arguments)
    except ProjectPathOutsideRoot as exc:
        return error_text(str(exc), code="project_path_outside_root", tool=name)
    except (OSError, ValueError) as exc:
        return error_text(
            f"projectPath is not a usable path: {exc}",
            code="invalid_arguments", tool=name, argument="projectPath",
        )
    try:
        return await handler(root, arguments)
    except PermissionError as exc:
        return error_text(str(exc), code="debug_consent_required", tool=name)
    # Before the ValueError arm below, which would otherwise flatten a refused
    # path into the generic `debug_error` an agent reads as "retry differently".
    except DebugPathOutsideRoot as exc:
        return error_text(str(exc), code="path_escape", tool=name)
    except FileNotFoundError as exc:
        return error_text(str(exc), code="not_found", tool=name)
    except (KeyError, TypeError, ValueError, RuntimeError, TimeoutError) as exc:
        return error_text(str(exc), code="debug_error", tool=name)
