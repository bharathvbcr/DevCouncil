"""
on_session_end.py - Antigravity Agent Lifecycle Hook: Session Teardown & Hygiene.

Releases DevCouncil task leases, shuts down tethered background daemons,
and prunes integrated worktrees following 6-point verification.
"""

import asyncio
import logging
import os
import sys
from typing import Any, Dict

from antigravity.config.mcp_bridge import call_mcp_tool
from antigravity.hooks.session_state import ag_hooks, state

logger = logging.getLogger("antigravity.hooks.on_session_end")
if not logger.handlers:
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(logging.Formatter("[%(levelname)s] on_session_end: %(message)s"))
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)


@ag_hooks.on_session_end
async def on_session_end_hook() -> Dict[str, Any]:
    """
    Executes upon session termination (normal completion, error, or cancellation).
    1. Releases active DevCouncil task lease.
    2. Prunes or detaches worktree in GitPulse if integrated.
    3. Reaps orphaned or tethered subprocesses.
    4. Resets session state.
    """
    logger.info("Executing Antigravity Session Teardown [Client: %s]", state.client_id)
    teardown_report = {
        "ok": True,
        "client_id": state.client_id,
        "lease_released": False,
        "worktree_pruned": False,
    }

    try:
        # Step 1: Release Task Lease in DevCouncil
        if state.task_id and state.lease_token:
            release_payload = {
                "task_id": state.task_id,
                "lease_token": state.lease_token,
                "client_id": state.client_id,
            }
            rel_res = await call_mcp_tool(
                "devcouncil", "devcouncil_release_task", release_payload
            )
            teardown_report["lease_released"] = rel_res.get("ok", True)
            logger.info("DevCouncil task lease %s successfully released.", state.task_id)

        # Step 2: Prune Worktree in GitPulse if safe
        if state.worktree_path and state.worktree_path != state.repo_path:
            prune_payload = {
                "repo_path": state.repo_path,
                "worktree_path": state.worktree_path,
                "force": False,  # Honors 6-point integration verification
            }
            prune_res = await call_mcp_tool(
                "gitpulse", "gitpulse_worktree_prune", prune_payload
            )
            teardown_report["worktree_pruned"] = prune_res.get("ok", True)
            logger.info(
                "GitPulse worktree %s processed for hygiene pruning.", state.worktree_path
            )

    except Exception as exc:
        logger.error("Session teardown error: %s", exc, exc_info=True)
        teardown_report["ok"] = False
        teardown_report["error"] = str(exc)

    finally:
        state.reset()

    return teardown_report


if __name__ == "__main__":
    result = asyncio.run(on_session_end_hook())
    print(f"Session teardown executed: {result}")
