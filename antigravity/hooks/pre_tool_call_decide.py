"""
pre_tool_call_decide.py - Antigravity Agent Lifecycle Hook: Pre-Tool Interception.

Guards all mutating tool calls:
1. GateSecretScan and .env* / credential lockdown.
2. Waylode line-level collision risk detection (fails closed if contended or scan fails).
3. DevCouncil policy_check_write scope and constitution validation (fails closed on errors,
   missing verdicts, or exceptions).
4. Command security policies for shell execution.
"""

import asyncio
import logging
import os
import re
import sys
from typing import Any, Optional, Union

from antigravity.config.mcp_bridge import call_mcp_tool
from antigravity.hooks.session_state import HookResult, ToolCall, ag_hooks, state

logger = logging.getLogger("antigravity.hooks.pre_tool_decide")
if not logger.handlers:
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(logging.Formatter("[%(levelname)s] pre_tool_decide: %(message)s"))
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)

# GateSecretScan patterns
SECRET_PATTERNS = [
    (re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"), "Private Key PEM block"),
    (re.compile(r"ghp_[a-zA-Z0-9]{36}"), "GitHub Personal Access Token"),
    (re.compile(r"gho_[a-zA-Z0-9]{36}"), "GitHub OAuth Token"),
    (re.compile(r"AIzaSy[a-zA-Z0-9_-]{33}"), "Google API Key"),
    (re.compile(r"AWS_SECRET_ACCESS_KEY\s*=\s*[A-Za-z0-9/\+=]{40}"), "AWS Secret Access Key"),
    (re.compile(r"AKIA[0-9A-Z]{16}"), "AWS Access Key ID"),
    (re.compile(r"sk-[a-zA-Z0-9]{48}"), "OpenAI Secret Key"),
    (re.compile(r"bearer\s+[a-zA-Z0-9_\-\.]{25,}", re.IGNORECASE), "Bearer Token"),
]

PROTECTED_FILE_PATTERNS = [
    re.compile(r"(^|/)\.env(\.[a-zA-Z0-9_-]+)?$"),
    re.compile(r"(^|/)\.git/credentials$"),
    re.compile(r"(^|/)id_rsa(\.pub)?$"),
    re.compile(r"(^|/)id_ed25519(\.pub)?$"),
    re.compile(r"(^|/)service[_-]?account.*\.json$", re.IGNORECASE),
]

FORBIDDEN_COMMAND_PATTERNS = [
    "rm -rf .",
    "rm -rf /",
    "git clean -fdx",
    "git reset --hard",
    "chmod -R 777",
    ":(){ :|:& };:",
]

MUTATING_FILE_TOOLS = {
    "edit_file",
    "replace_file_content",
    "create_file",
    "write_to_file",
    "append_file",
    "delete_file",
}


def scan_for_secrets(content: str) -> Optional[str]:
    """Scans text content for known credentials or private keys."""
    for pattern, desc in SECRET_PATTERNS:
        if pattern.search(content):
            return desc
    return None


def is_protected_secret_file(path: str) -> bool:
    """Checks if a file path is a protected secret file (e.g. .env)."""
    norm = path.replace("\\", "/")
    for pattern in PROTECTED_FILE_PATTERNS:
        if pattern.search(norm):
            return True
    return False


def _policy_operation_for_tool(tool_name: str) -> str:
    """Maps a mutating tool name to DevCouncil's policy operation enum."""
    if "delete" in tool_name:
        return "delete"
    if "edit" in tool_name or "replace" in tool_name or "append" in tool_name:
        return "modify"
    return "create"


@ag_hooks.pre_tool_call_decide
async def pre_tool_call_decide_hook(data: Union[ToolCall, Any]) -> HookResult:
    """
    Intercepts mutating actions before execution.
    Fails closed if another worktree holds the target file dirty, if the write
    contains secrets, or if DevCouncil constitution gates forbid or cannot
    verify the operation.
    """
    tool_name = getattr(data, "name", "").lower()
    raw_args = getattr(data, "arguments", None) or {}

    # Case A: File Mutation Tool
    if tool_name in MUTATING_FILE_TOOLS:
        target_path = (
            raw_args.get("TargetFile")
            or raw_args.get("target_file")
            or raw_args.get("path")
            or raw_args.get("file_path")
        )

        if not target_path:
            return HookResult(
                allow=False,
                reason="MUTATION REJECTED: No target file path specified in arguments.",
            )

        # 1. Normalize relative path
        if os.path.isabs(target_path):
            try:
                repo_rel_path = os.path.relpath(target_path, state.repo_path)
            except ValueError:
                repo_rel_path = target_path
        else:
            repo_rel_path = target_path

        # 2. GateSecretScan: Check for protected credential file paths (.env*, etc.)
        if is_protected_secret_file(repo_rel_path):
            logger.warning("Secret Guard intercepted protected file path: %s", repo_rel_path)
            return HookResult(
                allow=False,
                reason=(
                    f"SECURITY INTERCEPTION (GateSecretScan): Access or mutation to protected "
                    f"credential file '{repo_rel_path}' is strictly blocked by policy."
                ),
            )

        # 3. GateSecretScan: Scan payload content for leaked secrets
        content_to_check = (
            raw_args.get("CodeContent")
            or raw_args.get("ReplacementContent")
            or raw_args.get("content")
            or raw_args.get("text")
            or ""
        )
        leaked_secret = scan_for_secrets(content_to_check)
        if leaked_secret:
            logger.warning("Secret Guard detected leaked credential (%s) in payload", leaked_secret)
            return HookResult(
                allow=False,
                reason=(
                    f"SECURITY INTERCEPTION (GateSecretScan): Mutating content contains a leaked "
                    f"credential ({leaked_secret}). Token exfiltration blocked."
                ),
            )

        # 4. Invariant B1: Query Waylode Collision Risk
        # Only the arguments the server accepts: it rejects the rest outright
        # ("accepted: [path, repo_path]"), and a rejected scan is not a clean one.
        collision_payload = {
            "repo_path": state.repo_path,
            "path": repo_rel_path,
        }

        try:
            collision_res = await call_mcp_tool(
                "waylode", "waylode_collision_risk", collision_payload
            )
        except Exception as exc:
            # Invariant B1: An unavailable collision scan MUST fail closed
            logger.error("Waylode collision scan threw exception: %s", exc)
            return HookResult(
                allow=False,
                reason=f"SAFETY INTERCEPTION: Waylode collision scanner unavailable ({exc}). Fails closed.",
            )

        if not isinstance(collision_res, dict) or not collision_res.get("ok", False):
            # Invariant B1: A failed scan is not clean
            err_detail = collision_res.get("error") if isinstance(collision_res, dict) else collision_res
            return HookResult(
                allow=False,
                reason=(
                    f"SAFETY INTERCEPTION: Waylode collision scan could not verify '{repo_rel_path}' "
                    f"(ok=False, error={err_detail}). Write refused."
                ),
            )

        # A reply that carries neither answer a collision scan gives is not a clean
        # scan. The real server answers with `overlapping_files` (it has no
        # `status`); a reply with neither must not read as "nothing overlaps".
        if "overlapping_files" not in collision_res and "status" not in collision_res:
            return HookResult(
                allow=False,
                reason=(
                    f"SAFETY INTERCEPTION: Waylode collision scan returned no verdict for "
                    f"'{repo_rel_path}' (no overlapping_files or status in the reply). "
                    "A scan that did not answer is not clean. Write refused."
                ),
            )

        # Check for overlapping files or status=contended
        status = str(collision_res.get("status", "")).lower()
        overlapping = collision_res.get("overlapping_files", [])
        # The server reports `overlapping_files` as a COUNT with the rows in
        # `items`; older callers may supply a list of rows directly.
        rows = overlapping if isinstance(overlapping, list) else collision_res.get("items", [])
        rows = [row for row in rows if isinstance(row, dict)]
        if status == "contended" or overlapping:
            conflict = rows[0] if rows else collision_res
            holders = conflict.get("worktrees")
            holder = holders[0] if isinstance(holders, list) and holders and isinstance(holders[0], dict) else {}
            conflicting_agent = (
                conflict.get("conflicting_agent")
                or conflict.get("agent_id")
                or holder.get("branch")
                or "unknown_agent"
            )
            conflicting_worktree = (
                conflict.get("conflicting_worktree")
                or conflict.get("worktree_path")
                or holder.get("path")
                or "unknown_worktree"
            )
            logger.warning(
                "Collision detected on file %s with agent %s in %s",
                repo_rel_path,
                conflicting_agent,
                conflicting_worktree,
            )
            return HookResult(
                allow=False,
                reason=(
                    f"COLLISION HAZARD: File '{repo_rel_path}' is currently held with uncommitted changes by "
                    f"agent '{conflicting_agent}' in worktree '{conflicting_worktree}'. "
                    "Mutating this file would cause severe git merge conflicts. "
                    "Choose an alternate file or coordinate with the orchestrator."
                ),
            )

        # 5. Query DevCouncil Policy Check Write (fail closed on error, missing verdict, or exception)
        # Omit task_id when unleased: devcouncil_policy_check_write rejects an empty
        # task_id string with invalid_argument, and without task_id still enforces
        # hard-safety, restricted-path, and configured posture rules.
        policy_payload = {
            "path": repo_rel_path,
            "operation": _policy_operation_for_tool(tool_name),
        }
        if state.task_id and str(state.task_id).strip():
            policy_payload["task_id"] = str(state.task_id).strip()
        if state.repo_path and os.path.isabs(state.repo_path):
            policy_payload["repo_path"] = state.repo_path

        try:
            policy_res = await call_mcp_tool(
                "devcouncil", "devcouncil_policy_check_write", policy_payload
            )
        except Exception as exc:
            logger.error("DevCouncil policy check threw exception: %s", exc)
            return HookResult(
                allow=False,
                reason=(
                    f"POLICY INTERCEPTION: DevCouncil policy check unavailable for "
                    f"'{repo_rel_path}' ({exc}). Fails closed."
                ),
            )

        if not isinstance(policy_res, dict) or policy_res.get("ok") is not True:
            err_detail = (
                policy_res.get("error") or policy_res.get("code") or "policy check failed"
                if isinstance(policy_res, dict)
                else str(policy_res)
            )
            logger.warning(
                "DevCouncil policy check failed for %s: %s",
                repo_rel_path,
                err_detail,
            )
            return HookResult(
                allow=False,
                reason=(
                    f"POLICY INTERCEPTION: DevCouncil could not verify write to "
                    f"'{repo_rel_path}' (ok=False, error={err_detail}). Fails closed."
                ),
            )

        allowed = policy_res.get("allowed")
        if not isinstance(allowed, bool):
            logger.warning(
                "DevCouncil policy check omitted boolean 'allowed' for %s: %r",
                repo_rel_path,
                policy_res,
            )
            return HookResult(
                allow=False,
                reason=(
                    f"POLICY INTERCEPTION: DevCouncil policy check returned no boolean "
                    f"'allowed' verdict for '{repo_rel_path}'. Fails closed."
                ),
            )

        if not allowed:
            decision = policy_res.get("decision")
            decision_reason = (
                decision.get("reason")
                if isinstance(decision, dict) and decision.get("reason")
                else None
            )
            policy_reason = (
                policy_res.get("reason")
                or decision_reason
                or "Violates DevCouncil gate policies or scope boundary."
            )
            logger.warning(
                "DevCouncil Policy check denied write to %s: %s",
                repo_rel_path,
                policy_reason,
            )
            return HookResult(
                allow=False,
                reason=f"POLICY DENIAL: Write to '{repo_rel_path}' refused by DevCouncil: {policy_reason}",
            )

    # Case B: Shell Commands (run_command)
    elif tool_name == "run_command":
        cmd = (
            raw_args.get("CommandLine", "")
            or raw_args.get("command", "")
            or raw_args.get("cmd", "")
        )
        for forbidden in FORBIDDEN_COMMAND_PATTERNS:
            if forbidden in cmd:
                logger.warning("Command Security Policy blocked forbidden command: %s", cmd)
                return HookResult(
                    allow=False,
                    reason=(
                        f"COMMAND SECURITY POLICY: Dangerous destructive command pattern "
                        f"detected ('{forbidden}'). Execution blocked."
                    ),
                )

    return HookResult(allow=True)


if __name__ == "__main__":
    test_call = ToolCall(name="edit_file", arguments={"target_file": "src/lib.rs", "StartLine": 1, "EndLine": 10})
    result = asyncio.run(pre_tool_call_decide_hook(test_call))
    print(f"Pre-tool decision: allow={result.allow}, reason={result.reason}")
