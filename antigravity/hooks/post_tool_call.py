"""
post_tool_call.py - Antigravity Agent Lifecycle Hook: Post-Tool Execution.

Triggers incremental DevMap CAS index warming and appends tool execution
and file mutation events to the GitPulse append-only ledger.
"""

import asyncio
import logging
import os
import sys
import time
from typing import Any, Dict

from antigravity.config.mcp_bridge import call_mcp_tool
from antigravity.hooks.session_state import ag_hooks, state

logger = logging.getLogger("antigravity.hooks.post_tool_call")
if not logger.handlers:
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(logging.Formatter("[%(levelname)s] post_tool_call: %(message)s"))
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)

MUTATING_FILE_TOOLS = {
    "edit_file",
    "replace_file_content",
    "create_file",
    "write_to_file",
    "append_file",
    "delete_file",
}


@ag_hooks.post_tool_call
async def post_tool_call_hook(data: Any) -> Dict[str, Any]:
    """
    Executes immediately after any tool finishes.
    1. Records tool mutation event in GitPulse append-only ledger.
    2. If a file was modified, triggers DevMap incremental index warming.
    """
    tool_name = getattr(data, "name", None) or (data.get("name") if isinstance(data, dict) else "unknown_tool")
    tool_args = getattr(data, "arguments", None) or (data.get("arguments") if isinstance(data, dict) else {})
    has_error = getattr(data, "error", False) or (data.get("error", False) if isinstance(data, dict) else False)

    logger.debug("Processing post_tool_call for tool '%s'", tool_name)
    audit_record = {
        "ok": True,
        "tool_name": tool_name,
        "ledger_appended": False,
        "devmap_warmed": False,
    }

    try:
        # 1. Append tool mutation event to GitPulse ledger
        ledger_event = {
            "timestamp": time.time(),
            "repo_path": state.repo_path,
            "agent_id": state.client_id,
            "task_id": state.task_id,
            "event_type": "TOOL_EXECUTION",
            "tool_name": tool_name,
            "success": not bool(has_error),
            "target_file": (
                tool_args.get("TargetFile")
                or tool_args.get("target_file")
                or tool_args.get("path")
                or None
            ),
        }

        ledger_res = await call_mcp_tool(
            "gitpulse", "gitpulse_ledger_events", {"event": ledger_event}
        )
        audit_record["ledger_appended"] = ledger_res.get("ok", True)

        # 2. If a file was modified, trigger DevMap warm build / CAS cache update
        if tool_name.lower() in MUTATING_FILE_TOOLS and not has_error:
            devmap_res = await call_mcp_tool(
                "devmap", "devmap_status", {"repo_path": state.repo_path}
            )
            audit_record["devmap_warmed"] = devmap_res.get("ok", True)
            logger.info(
                "Triggered DevMap incremental CAS index warm for %s [Gen: %s]",
                state.repo_path,
                devmap_res.get("generation_id", "latest"),
            )

    except Exception as exc:
        # Post-tool telemetry warnings should be non-blocking
        logger.warning("Post-tool notification warning: %s", exc)
        audit_record["warning"] = str(exc)

    return audit_record


if __name__ == "__main__":
    test_data = {"name": "write_to_file", "arguments": {"path": "src/main.rs"}, "error": False}
    result = asyncio.run(post_tool_call_hook(test_data))
    print(f"Post-tool hook executed: {result}")
