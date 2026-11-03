"""
on_session_start.py - Antigravity Agent Lifecycle Hook: Session Startup.

Leases task via devcouncil_checkout_task and commands GitPulse to provision
an isolated worktree with Copy-on-Write (CoW) build cache cloning.
"""

import asyncio
import logging
import os
import sys

from antigravity.config.mcp_bridge import call_mcp_tool
from antigravity.hooks.session_state import ag_hooks, state

logger = logging.getLogger("antigravity.hooks.on_session_start")
if not logger.handlers:
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(logging.Formatter("[%(levelname)s] on_session_start: %(message)s"))
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)


@ag_hooks.on_session_start
async def on_session_start_hook() -> dict:
    """
    Executes at agent startup.
    1. Checks out a task lease from DevCouncil via devcouncil_checkout_task.
    2. Provisions or registers an isolated worktree in GitPulse with CoW cache cloning.
    3. Binds the lease token and worktree path into session state.
    """
    logger.info(
        "Initializing Antigravity Session [Client: %s, Task: %s, Repo: %s]",
        state.client_id,
        state.task_id,
        state.repo_path,
    )

    if not state.task_id:
        logger.warning(
            "No DEVCOUNCIL_TASK_ID provided in environment. Operating in unleased read-only mode."
        )
        return {"ok": True, "status": "UNLEASED_READ_ONLY"}

    try:
        # Step 1: Checkout Task Lease in DevCouncil
        checkout_payload = {
            "task_id": state.task_id,
            "client_id": state.client_id,
            "agent": "antigravity",
            "force": False,
        }
        lease_result = await call_mcp_tool(
            "devcouncil", "devcouncil_checkout_task", checkout_payload
        )
        if not lease_result.get("ok", True):
            err_msg = lease_result.get("error", "Unknown error during checkout")
            raise RuntimeError(f"Failed to acquire task lease for {state.task_id}: {err_msg}")

        state.lease_token = lease_result.get("lease_token")
        logger.info("Acquired DevCouncil Lease Token: %s", state.lease_token)

        # Step 2: Provision or Register Worktree in GitPulse with CoW cache sharing
        branch_name = f"agent/{state.task_id}/{state.client_id}"
        state.active_branch = branch_name
        worktree_payload = {
            "repo_path": state.repo_path,
            "branch": branch_name,
            "share_cache": True,
            "task_id": state.task_id,
        }
        worktree_res = await call_mcp_tool(
            "gitpulse", "gitpulse_worktree_create", worktree_payload
        )

        state.worktree_path = worktree_res.get("worktree_path", state.repo_path)
        shared_bytes = worktree_res.get("reflink_bytes_shared", 0)
        logger.info(
            "GitPulse worktree assigned at %s (Reflink CoW shared bytes: %s)",
            state.worktree_path,
            shared_bytes,
        )

        return {
            "ok": True,
            "status": "INITIALIZED",
            "task_id": state.task_id,
            "lease_token": state.lease_token,
            "worktree_path": state.worktree_path,
            "reflink_bytes_shared": shared_bytes,
        }

    except Exception as exc:
        logger.critical("Session start initialization failed: %s", exc, exc_info=True)
        # Invariant B4 & Fail Closed: halt session if lease acquisition fails
        raise RuntimeError(f"Antigravity session initialization halted: {exc}") from exc


if __name__ == "__main__":
    result = asyncio.run(on_session_start_hook())
    print(f"Session startup completed: {result}")
