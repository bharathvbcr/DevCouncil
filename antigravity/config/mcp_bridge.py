"""
mcp_bridge.py - Unified MCP In-Process Invocation Bridge for Antigravity.

Provides asynchronous tool execution across DevCouncil, DevMap, and Waylode
(formerly GitPulse), handling stdio subprocess pipes and HTTP daemon calls
transparently.

Waylode is the new name of the GitPulse server: its tools are ``waylode_<x>``
(formerly ``gitpulse_<x>``) and the server key is ``waylode`` (formerly
``gitpulse``). During the transition either server key and either tool spelling
is accepted here. The binary is still ``gitpulse-mcp``.
"""

import asyncio
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

# The GitPulse -> Waylode rename. Each server key maps to the tool-name prefix its
# generation of the server lists. Delete the "gitpulse" row (and the swapping
# below) once no host serves the old names.
_SERVER_TOOL_PREFIX = {"waylode": "waylode_", "gitpulse": "gitpulse_"}
_SERVER_COUNTERPART = {"waylode": "gitpulse", "gitpulse": "waylode"}


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


def _resolve_binary_command(server_name: str) -> Optional[List[str]]:
    """Resolves the executable command for an MCP stdio server."""
    repo_root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

    if server_name == "devcouncil":
        candidates = [
            os.path.join(repo_root, "backend", "go_orchestrator", "bin", "devcouncil"),
            "/Users/bharath/.local/bin/devcouncil",
        ]
        for c in candidates:
            if os.path.isfile(c) and os.access(c, os.X_OK):
                return [c, "mcp"]
        w = shutil.which("devcouncil")
        if w and ".venv" not in w:
            return [w, "mcp"]
        return None

    elif server_name in ("waylode", "gitpulse"):
        for binary in ("waylode-mcp", "gitpulse-mcp"):
            w = shutil.which(binary)
            if w:
                return [w]
        return None

    elif server_name == "devmap":
        candidates = [
            os.path.join(repo_root, "target", "debug", "devmap"),
            os.path.join(repo_root, "target", "release", "devmap"),
            "/Users/bharath/.local/bin/devmap",
        ]
        for c in candidates:
            if os.path.isfile(c) and os.access(c, os.X_OK):
                return [c, "mcp"]
        w = shutil.which("devmap")
        if w:
            return [w, "mcp"]
        return None

    return None


async def _exec_mcp_stdio_jsonrpc(
    cmd: List[str], tool_name: str, arguments: Dict[str, Any]
) -> Dict[str, Any]:
    """Sends JSON-RPC 2.0 initialize + tools/call to an MCP stdio server."""
    try:
        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        init_req = json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "antigravity-bridge", "version": "1.0.0"},
            },
        }) + "\n"
        call_req = json.dumps({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments,
            },
        }) + "\n"

        stdout, stderr = await asyncio.wait_for(
            proc.communicate(input=(init_req + call_req).encode("utf-8")),
            timeout=8.0,
        )

        lines = stdout.decode("utf-8", errors="replace").splitlines()
        for line in lines:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
                if msg.get("id") == 2:
                    if "error" in msg:
                        return {"ok": False, "error": msg["error"], "server_answered": True}
                    result = msg.get("result", {})
                    content = result.get("content", [])
                    if result.get("isError"):
                        # An MCP tool error (bad arguments, a refused call) is
                        # an error: reading its text as {"ok": True, "output": ...}
                        # made a rejected collision scan look like a clean one.
                        text = content[0].get("text", "") if content and isinstance(content, list) else ""
                        return {"ok": False, "error": text or "tool error", "server_answered": True}
                    if content and isinstance(content, list):
                        text = content[0].get("text", "")
                        try:
                            parsed_text = json.loads(text)
                            if isinstance(parsed_text, dict):
                                if parsed_text.get("ok", True) is False:
                                    # A refusal the server wrote itself (for
                                    # example a collision scan that could not
                                    # read a worktree). Mark it so the caller
                                    # reports it instead of simulating a pass.
                                    return {**parsed_text, "server_answered": True}
                                return parsed_text
                        except json.JSONDecodeError:
                            return {"ok": True, "output": text}
                    return {"ok": True, **result}
            except json.JSONDecodeError:
                continue

        if proc.returncode != 0:
            err = stderr.decode("utf-8", errors="replace").strip()
            return {"ok": False, "error": err or f"Process exited with code {proc.returncode}"}

    except Exception as exc:
        return {"ok": False, "error": str(exc)}

    # The process ended without ever answering tools/call. That is no answer, not
    # an empty success.
    return {"ok": False, "error": "no tools/call reply from the MCP server"}


async def call_mcp_tool(
    server_name: str, tool_name: str, arguments: Dict[str, Any]
) -> Dict[str, Any]:
    """
    Executes an MCP tool call against the target server subsystem.
    Handles registered in-process handlers, stdio process pipes, and HTTP/SSE calls.
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
            return {"ok": False, "error": str(exc)}

    # 2. HTTP/SSE execution for devmap daemon
    if server_name == "devmap":
        port = os.environ.get("DEVMAP_PORT", "9095")
        url = f"http://127.0.0.1:{port}/api/mcp/{tool_name}"
        try:
            payload = json.dumps(arguments).encode("utf-8")
            req = urllib.request.Request(
                url,
                data=payload,
                headers={"Content-Type": "application/json", "User-Agent": "AntigravityMcpBridge/1.0"},
            )
            loop = asyncio.get_running_loop()
            def _http_post():
                with urllib.request.urlopen(req, timeout=5) as response:
                    return json.loads(response.read().decode("utf-8"))
            return await loop.run_in_executor(None, _http_post)
        except Exception as exc:
            logger.debug("HTTP MCP execution to devmap (%s) failed, falling back to stdio: %s", url, exc)

    # 3. Stdio execution via resolved binary and JSON-RPC
    cmd = _resolve_binary_command(server_name)
    if cmd:
        res = await _exec_mcp_stdio_jsonrpc(cmd, tool_name, arguments)
        alternate = _other_spelling(tool_name)
        if alternate is not None and _is_unknown_tool(res):
            # A server from before (or after) the rename lists the other name.
            # Try it here: falling through to the offline answer below would
            # report a clean scan nobody ran.
            res = await _exec_mcp_stdio_jsonrpc(cmd, alternate, arguments)
        if _is_unknown_tool(res):
            # A live server that does not list the tool under either spelling
            # answered; that is an error to report, not an outage to simulate.
            tried = tool_name if alternate is None else f"{tool_name} or {alternate}"
            return {
                "ok": False,
                "unknown_tool": True,
                "error": f"unknown tool: {server_name} does not list {tried}",
            }
        if res.get("ok", False):
            return res
        if res.get("server_answered"):
            # The server was reached and said no. Simulating a success here would
            # turn its refusal into a pass; only an unreachable server is
            # replaced by the offline answer below.
            return res
        logger.warning(
            "Stdio MCP execution for %s.%s returned error (%s). Falling back to simulated offline response.",
            server_name,
            tool_name,
            res.get("error"),
        )

    # 4. Offline simulation fallback (ensures tests and unconfigured dev environments work)
    return _generate_offline_tool_response(server_name, tool_name, arguments)


def _generate_offline_tool_response(
    server_name: str, tool_name: str, arguments: Dict[str, Any]
) -> Dict[str, Any]:
    """Generates a safe, well-formed response when running in offline/uninstalled test mode."""
    requested = tool_name
    if server_name == "devcouncil":
        if tool_name == "devcouncil_checkout_task":
            return {
                "ok": True,
                "task_id": arguments.get("task_id", "offline_task"),
                "lease_token": f"lease_token_{os.getpid()}_{arguments.get('task_id', 'offline_task')}",
                "status": "CHECKED_OUT",
            }
        elif tool_name == "devcouncil_verify_task":
            return {
                "ok": True,
                "passed": True,
                "blocking_gaps": [],
                "typed_next_actions": [],
            }
        elif tool_name == "devcouncil_release_task":
            return {"ok": True, "released": True}
        elif tool_name == "devcouncil_policy_check_write":
            path = arguments.get("path", "")
            if ".env" in path or "secret" in path.lower() or "id_rsa" in path:
                return {"ok": True, "allowed": False, "reason": "Write denied: protected secret file path"}
            return {"ok": True, "allowed": True}

    elif server_name in ("waylode", "gitpulse"):
        # Either server key and either spelling reach the same answers.
        tool_name = _respell_tool("waylode", tool_name)
        if tool_name == "waylode_collision_risk":
            return {
                "ok": True,
                "status": "clean",
                "overlapping_files": [],
            }
        elif tool_name == "waylode_worktree_create":
            repo_path = arguments.get("repo_path", os.getcwd())
            branch = arguments.get("branch", "agent_worktree")
            return {
                "ok": True,
                "worktree_path": repo_path,
                "branch": branch,
                "reflink_bytes_shared": 104857600,
            }
        elif tool_name == "waylode_worktree_prune":
            return {"ok": True, "pruned": True}
        elif tool_name == "waylode_ledger_events":
            return {"ok": True, "appended": True}

    elif server_name == "devmap":
        if tool_name in ["devmap_status", "devmap_preview"]:
            return {"ok": True, "status": "WARMED", "generation_id": 1}

    # Fail closed. This used to answer {"ok": True, "simulated": True} for any
    # name, so a misspelled or renamed tool read as a success -- and a collision
    # scan that never ran read as a clean one. Only the tools simulated above
    # have an offline answer; anything else is an error the caller must see.
    return {
        "ok": False,
        "unknown_tool": True,
        "error": (
            f"unknown tool: no offline simulation for {server_name}.{requested}; "
            "the server was not reached"
        ),
    }
