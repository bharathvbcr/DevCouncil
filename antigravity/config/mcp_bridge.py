"""
mcp_bridge.py - Unified MCP In-Process Invocation Bridge for Antigravity.

Provides asynchronous tool execution across DevCouncil, DevMap, and Waylode
(formerly GitPulse), handling stdio subprocess pipes and modern Streamable HTTP
JSON-RPC calls transparently. All unreachable, timed-out, or errored server
calls fail closed and never fabricate passing verdicts.

Waylode is the new name of the GitPulse server: its tools are ``waylode_<x>``
(formerly ``gitpulse_<x>``) and the server key is ``waylode`` (formerly
``gitpulse``). During the transition either server key and either tool spelling
is accepted here.
"""

import asyncio
import base64
import json
import logging
import os
import shutil
import urllib.error
import urllib.request
from typing import Any, Callable, Dict, List, Optional

logger = logging.getLogger("antigravity.mcp_bridge")

# Registry for in-process mock handlers (for hermetic testing or embedded runtimes)
_in_process_handlers: Dict[str, Callable[[str, Dict[str, Any]], Any]] = {}

# Timeout defaults (seconds). Configurable via environment variables so slow
# verification suites never time out prematurely or silently auto-pass.
DEFAULT_STDIO_TIMEOUT_S = 60.0
DEFAULT_VERIFY_TIMEOUT_S = 360.0
DEFAULT_HTTP_TIMEOUT_S = 30.0

MCP_STDIO_PROTOCOL_VERSION = "2025-06-18"
MCP_MODERN_PROTOCOL_VERSION = "2026-07-28"

# The GitPulse -> Waylode rename. Each server key maps to the tool-name prefix its
# generation of the server lists. Delete the "gitpulse" row (and the swapping
# below) once no host serves the old names.
_SERVER_TOOL_PREFIX = {"waylode": "waylode_", "gitpulse": "gitpulse_"}
_SERVER_COUNTERPART = {"waylode": "gitpulse", "gitpulse": "waylode"}


def _fail_closed_error(error: Any, **extra: Any) -> Dict[str, Any]:
    """Builds a uniform fail-closed error payload for any failed MCP call."""
    payload: Dict[str, Any] = {
        "ok": False,
        "passed": False,
        "allowed": False,
        "status": "error",
        "error": error,
    }
    payload.update(extra)
    return payload


def _parse_positive_float(raw: Optional[str]) -> Optional[float]:
    if not raw:
        return None
    try:
        val = float(raw.strip())
        return val if val > 0 else None
    except ValueError:
        return None


def _timeout_for_tool(tool_name: str) -> float:
    """Returns the stdio timeout budget in seconds for the given MCP tool."""
    if tool_name == "devcouncil_verify_task":
        verify_override = _parse_positive_float(
            os.environ.get("ANTIGRAVITY_MCP_VERIFY_TIMEOUT_S")
        )
        if verify_override is not None:
            return verify_override
        ms_override = _parse_positive_float(
            os.environ.get("DEVCOUNCIL_MCP_CALL_TIMEOUT_MS")
        )
        if ms_override is not None:
            return ms_override / 1000.0
        return DEFAULT_VERIFY_TIMEOUT_S

    env_override = _parse_positive_float(os.environ.get("ANTIGRAVITY_MCP_TIMEOUT_S"))
    if env_override is not None:
        return env_override
    ms_override = _parse_positive_float(
        os.environ.get("DEVCOUNCIL_MCP_CALL_TIMEOUT_MS")
    )
    if ms_override is not None:
        return ms_override / 1000.0
    return DEFAULT_STDIO_TIMEOUT_S


def _respell_tool(server_key: str, tool_name: str) -> str:
    """Rewrites a Waylode/GitPulse tool name to the spelling ``server_key`` uses.

    Names outside the renamed family (``devmap_*``, ``devcouncil_*``) and servers
    outside it are returned untouched.
    """
    target = _SERVER_TOOL_PREFIX.get(server_key)
    if target is None:
        return tool_name
    for prefix in _SERVER_TOOL_PREFIX.values():
        if tool_name.startswith(prefix):
            return target + tool_name[len(prefix):]
    return tool_name


def _other_spelling(tool_name: str) -> Optional[str]:
    """The same tool under the other generation's name, or None outside the family."""
    for server_key, prefix in _SERVER_TOOL_PREFIX.items():
        if tool_name.startswith(prefix):
            return _respell_tool(_SERVER_COUNTERPART[server_key], tool_name)
    return None


def _is_unknown_tool(result: Dict[str, Any]) -> bool:
    """True when a reply says the server does not list that tool name.

    That is a naming mismatch, not a failure of the call: retrying the other
    spelling is right, and nothing else is.
    """
    if result.get("ok", True) is not False:
        return False  # a success whose text happens to mention it is not a rejection
    text = str(result.get("error", "")).lower()
    return "unknown tool" in text or "tool not found" in text or "no such tool" in text


def register_in_process_handler(
    server_name: str, handler: Callable[[str, Dict[str, Any]], Any]
) -> None:
    """Registers an in-process tool handler for a specific server (e.g. for testing)."""
    _in_process_handlers[server_name] = handler


def clear_in_process_handlers() -> None:
    """Clears in-process mock handlers."""
    _in_process_handlers.clear()


def _is_executable(path: str) -> bool:
    return bool(path) and os.path.isfile(path) and os.access(path, os.X_OK)


def _resolve_executable(
    env_vars: List[str],
    binary_names: List[str],
    fallback_candidates: Optional[List[str]] = None,
    skip_venv: bool = False,
) -> Optional[str]:
    """Resolves a binary via env override, PATH (shutil.which), or ~/.local/bin fallbacks."""
    for env_var in env_vars:
        raw = os.environ.get(env_var, "").strip()
        if raw:
            expanded = os.path.expanduser(raw)
            if _is_executable(expanded):
                return expanded
            logger.warning(
                "Configured %s=%r is not an executable file; refusing silent fallback.",
                env_var,
                raw,
            )
            return None

    for name in binary_names:
        found = shutil.which(name)
        if found and (not skip_venv or ".venv" not in found):
            return found

    local_bin = os.path.expanduser("~/.local/bin")
    cargo_bin = os.path.expanduser("~/.cargo/bin")
    candidates: List[str] = []
    for name in binary_names:
        candidates.append(os.path.join(local_bin, name))
        candidates.append(os.path.join(cargo_bin, name))
    if fallback_candidates:
        candidates.extend(fallback_candidates)

    for candidate in candidates:
        if _is_executable(candidate):
            return candidate
    return None


def _resolve_binary_command(server_name: str) -> Optional[List[str]]:
    """Resolves the executable command for an MCP stdio server."""
    repo_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

    if server_name == "devcouncil":
        exe = _resolve_executable(
            env_vars=["DEVCOUNCIL_BIN"],
            binary_names=["devcouncil"],
            fallback_candidates=[
                os.path.join(repo_root, "backend", "go_orchestrator", "bin", "devcouncil"),
            ],
            skip_venv=True,
        )
        return [exe, "mcp"] if exe else None

    if server_name in ("waylode", "gitpulse"):
        exe = _resolve_executable(
            env_vars=["WAYLODE_MCP_BIN", "GITPULSE_MCP_BIN"],
            binary_names=["waylode-mcp", "gitpulse-mcp"],
        )
        return [exe] if exe else None

    if server_name == "devmap":
        exe = _resolve_executable(
            env_vars=["DEVMAP_BIN"],
            binary_names=["devmap"],
            fallback_candidates=[
                os.path.join(repo_root, "rust", "target", "release", "devmap"),
                os.path.join(repo_root, "rust", "target", "debug", "devmap"),
                os.path.join(repo_root, "target", "release", "devmap"),
                os.path.join(repo_root, "target", "debug", "devmap"),
            ],
        )
        return [exe, "mcp"] if exe else None

    return None


def _encode_mcp_header_value(value: str) -> str:
    """Encodes non-ASCII MCP mirrored headers using the =?base64?...?= envelope."""
    try:
        value.encode("ascii")
        return value
    except UnicodeEncodeError:
        b64 = base64.b64encode(value.encode("utf-8")).decode("ascii")
        return f"=?base64?{b64}?="


def _parse_jsonrpc_tool_reply(msg: Dict[str, Any]) -> Dict[str, Any]:
    """Parses a JSON-RPC 2.0 tools/call response envelope into a bridge dict."""
    if "error" in msg and msg["error"] is not None:
        return _fail_closed_error(msg["error"], server_answered=True)

    result = msg.get("result")
    if not isinstance(result, dict):
        return _fail_closed_error("malformed JSON-RPC result from MCP server", server_answered=True)

    content = result.get("content", [])
    if result.get("isError"):
        # An MCP tool error (bad arguments, a refused call) is an error:
        # reading its text as {"ok": True, "output": ...} would mask refusals.
        text = (
            content[0].get("text", "")
            if content and isinstance(content, list) and isinstance(content[0], dict)
            else ""
        )
        return _fail_closed_error(text or "tool error", server_answered=True)

    if content and isinstance(content, list) and isinstance(content[0], dict):
        text = content[0].get("text", "")
        try:
            parsed_text = json.loads(text)
            if isinstance(parsed_text, dict):
                if parsed_text.get("ok", True) is False:
                    return {
                        "passed": False,
                        "allowed": False,
                        **parsed_text,
                        "ok": False,
                        "server_answered": True,
                    }
                return parsed_text
        except json.JSONDecodeError:
            return {"ok": True, "output": text}
    return {"ok": True, **result}


async def _exec_devmap_http_jsonrpc(
    url: str,
    tool_name: str,
    arguments: Dict[str, Any],
    timeout: float,
) -> Optional[Dict[str, Any]]:
    """Sends an MCP 2026-07-28 single-exchange JSON-RPC POST to a DevMap HTTP daemon.

    Returns None when the HTTP daemon is unreachable so the caller can fall back
    to stdio. Returns a parsed dict (including server errors) whenever the HTTP
    daemon answered.
    """
    body = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": MCP_MODERN_PROTOCOL_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
                "name": tool_name,
                "arguments": arguments,
            },
        }
    ).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json",
            "MCP-Protocol-Version": MCP_MODERN_PROTOCOL_VERSION,
            "Mcp-Method": "tools/call",
            "Mcp-Name": _encode_mcp_header_value(tool_name),
            "User-Agent": "AntigravityMcpBridge/1.0",
        },
    )
    loop = asyncio.get_running_loop()

    def _http_post() -> Optional[Dict[str, Any]]:
        try:
            with urllib.request.urlopen(req, timeout=timeout) as response:
                raw = response.read().decode("utf-8", errors="replace")
            msg = json.loads(raw)
            if isinstance(msg, dict):
                return _parse_jsonrpc_tool_reply(msg)
            return _fail_closed_error("non-object JSON-RPC reply from DevMap HTTP", server_answered=True)
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode("utf-8", errors="replace")
            try:
                msg = json.loads(raw)
                if isinstance(msg, dict) and ("error" in msg or "result" in msg):
                    return _parse_jsonrpc_tool_reply(msg)
            except json.JSONDecodeError:
                pass
            logger.debug("DevMap HTTP endpoint %s returned HTTP %s; falling back to stdio", url, exc.code)
            return None
        except Exception as exc:
            logger.debug("DevMap HTTP MCP call to %s failed, falling back to stdio: %s", url, exc)
            return None

    return await loop.run_in_executor(None, _http_post)


async def _exec_mcp_stdio_jsonrpc(
    cmd: List[str],
    tool_name: str,
    arguments: Dict[str, Any],
    timeout: Optional[float] = None,
) -> Dict[str, Any]:
    """Sends JSON-RPC 2.0 initialize + tools/call to an MCP stdio server."""
    call_timeout = timeout if timeout is not None else _timeout_for_tool(tool_name)
    proc: Optional[asyncio.subprocess.Process] = None
    try:
        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        init_req = json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": MCP_STDIO_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "antigravity-bridge", "version": "1.0.0"},
                },
            }
        ) + "\n"
        initialized_notif = json.dumps(
            {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
            }
        ) + "\n"
        call_req = json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": tool_name,
                    "arguments": arguments,
                },
            }
        ) + "\n"

        try:
            stdout, stderr = await asyncio.wait_for(
                proc.communicate(
                    input=(init_req + initialized_notif + call_req).encode("utf-8")
                ),
                timeout=call_timeout,
            )
        except asyncio.TimeoutError:
            try:
                proc.kill()
            except ProcessLookupError:
                pass
            try:
                await proc.wait()
            except Exception:
                pass
            return _fail_closed_error(
                f"MCP stdio call {tool_name} timed out after {call_timeout:.1f}s",
                timed_out=True,
            )

        lines = stdout.decode("utf-8", errors="replace").splitlines()
        for line in lines:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
                if isinstance(msg, dict) and msg.get("id") == 2:
                    return _parse_jsonrpc_tool_reply(msg)
            except json.JSONDecodeError:
                continue

        if proc.returncode != 0:
            err = stderr.decode("utf-8", errors="replace").strip()
            return _fail_closed_error(
                err or f"Process exited with code {proc.returncode}"
            )

    except Exception as exc:
        return _fail_closed_error(str(exc))

    # The process ended without ever answering tools/call. That is no answer, not
    # an empty success.
    return _fail_closed_error("no tools/call reply from the MCP server")


async def call_mcp_tool(
    server_name: str, tool_name: str, arguments: Dict[str, Any]
) -> Dict[str, Any]:
    """
    Executes an MCP tool call against the target server subsystem.
    Handles registered in-process handlers, DevMap Streamable HTTP JSON-RPC,
    and stdio process pipes. Always fails closed when a server is unavailable,
    times out, or errors.
    """
    # 1. Check for in-process registered handler (hermetic / embedded).
    # A handler registered under the other generation's server key still serves
    # this call, and is handed the tool spelling that generation lists.
    handler_key = next(
        (
            key
            for key in (server_name, _SERVER_COUNTERPART.get(server_name, ""))
            if key in _in_process_handlers
        ),
        None,
    )
    if handler_key is not None:
        tool_name = _respell_tool(handler_key, tool_name)
        try:
            handler = _in_process_handlers[handler_key]
            result = handler(tool_name, arguments)
            if asyncio.iscoroutine(result):
                result = await result
            if isinstance(result, dict):
                return result
            return {"ok": True, "result": result}
        except Exception as exc:
            logger.error("In-process handler for %s.%s failed: %s", server_name, tool_name, exc)
            return _fail_closed_error(str(exc))

    # 2. Streamable HTTP JSON-RPC execution for DevMap daemon (if running).
    if server_name == "devmap" and os.environ.get("DEVMAP_DISABLE_HTTP") != "1":
        http_url = os.environ.get("DEVMAP_HTTP_URL", "").strip()
        if not http_url:
            port = os.environ.get("DEVMAP_PORT", "9095")
            http_url = f"http://127.0.0.1:{port}/mcp"
        http_timeout = min(_timeout_for_tool(tool_name), DEFAULT_HTTP_TIMEOUT_S)
        http_res = await _exec_devmap_http_jsonrpc(
            http_url, tool_name, arguments, timeout=http_timeout
        )
        if http_res is not None:
            return http_res

    # 3. Stdio execution via resolved binary and JSON-RPC
    cmd = _resolve_binary_command(server_name)
    if not cmd:
        logger.error(
            "MCP server '%s' binary could not be resolved for tool '%s'; failing closed.",
            server_name,
            tool_name,
        )
        return _fail_closed_error(
            f"MCP server '{server_name}' is unavailable for tool '{tool_name}': executable not found",
            unavailable=True,
        )

    res = await _exec_mcp_stdio_jsonrpc(cmd, tool_name, arguments)
    alternate = _other_spelling(tool_name)
    if alternate is not None and _is_unknown_tool(res):
        # A server from before (or after) the rename lists the other name.
        res = await _exec_mcp_stdio_jsonrpc(cmd, alternate, arguments)
    if _is_unknown_tool(res):
        tried = tool_name if alternate is None else f"{tool_name} or {alternate}"
        return _fail_closed_error(
            f"unknown tool: {server_name} does not list {tried}",
            unknown_tool=True,
            server_answered=True,
        )
    if res.get("ok", False):
        return res
    if res.get("server_answered"):
        return res

    logger.error(
        "Stdio MCP execution for %s.%s failed (%s); failing closed.",
        server_name,
        tool_name,
        res.get("error"),
    )
    return {
        "passed": False,
        "allowed": False,
        "status": "error",
        **res,
        "ok": False,
    }
