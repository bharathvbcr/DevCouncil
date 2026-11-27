"""
stop.py - Antigravity Agent Lifecycle Hook: The Hero Loop (Deterministic Self-Repair).

Intercepts agent completion and invokes devcouncil_verify_task.
If blocking verification gaps exist (or verification fails/degrades), returns
StopDecision.CONTINUE with typed next_actions from verify.MCPResult to drive
autonomous self-repair.
"""

import asyncio
import json
import logging
import os
import sys
from typing import Any, Dict, List, Union

from antigravity.config.mcp_bridge import call_mcp_tool
from antigravity.hooks.session_state import (
    StopArgs,
    StopDecision,
    StopHookResult,
    ag_hooks,
    state,
)

logger = logging.getLogger("antigravity.hooks.hero_loop")
if not logger.handlers:
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(logging.Formatter("[%(levelname)s] hero_loop: %(message)s"))
    logger.addHandler(handler)
    logger.setLevel(logging.INFO)


def _format_location(item: Dict[str, Any]) -> str:
    file_path = item.get("file") or ""
    line = item.get("line")
    if file_path and line is not None:
        return f"{file_path}:{line}"
    return str(file_path)


@ag_hooks.stop
async def stop_hook(data: Union[StopArgs, Any] = None) -> StopHookResult:
    """
    Executes when the root trajectory reaches idle and the agent attempts to stop.
    Invokes DevCouncil deterministic verification gates.

    Reads the real verify.MCPResult contract (backend/go_orchestrator/devcouncil/verify/types.go):
      - ok, passed, status, degraded_reasons
      - blocking_gaps[].{id, severity, description, file, line, recommended_fix}
      - next_actions[].{category, action, file, line, gap_id, suggested_command}

    If verification fails, degrades, or blocking gaps exist:
      - Intercepts termination.
      - Injects typed next_actions (e.g. fix_code, add_test, scope, security).
      - Returns StopDecision.CONTINUE with structured repair prompt.
    """
    continuation_count = 0
    if data:
        raw_val = None
        if isinstance(data, str):
            try:
                parsed = json.loads(data)
                if isinstance(parsed, dict):
                    raw_val = (
                        parsed.get("continuation_count")
                        or parsed.get("args", {}).get("continuation_count")
                        or parsed.get("payload", {}).get("continuation_count")
                    )
            except Exception:
                pass
        elif isinstance(data, dict):
            raw_val = (
                data.get("continuation_count")
                or data.get("args", {}).get("continuation_count")
                or data.get("payload", {}).get("continuation_count")
            )
        else:
            raw_val = getattr(data, "continuation_count", None)

        if raw_val is not None:
            try:
                continuation_count = max(0, int(raw_val))
            except (ValueError, TypeError):
                continuation_count = 0
    logger.info(
        "Hero Loop Interceptor triggered [Attempt %d/%d, Task: %s]",
        continuation_count + 1,
        state.continuation_threshold,
        state.task_id,
    )

    # 1. Prevent infinite self-repair loops if threshold is reached
    if continuation_count >= state.continuation_threshold:
        logger.error(
            "Hero Loop self-repair threshold (%d) reached. Permitting termination with logged gaps.",
            state.continuation_threshold,
        )
        return StopHookResult(
            decision=StopDecision.ALLOW_STOP,
            reason=f"Self-repair continuation threshold ({state.continuation_threshold}) reached.",
        )

    # 2. If no leased task or unleased mode, allow normal completion
    if not state.task_id or not state.lease_token:
        logger.info("No active task lease to verify. Permitting normal completion.")
        return StopHookResult(decision=StopDecision.ALLOW_STOP)

    try:
        # 3. Invoke DevCouncil Deterministic Task Verification
        verify_payload: Dict[str, Any] = {
            "task_id": state.task_id,
            "lease_token": state.lease_token,
        }
        if state.repo_path and os.path.isabs(state.repo_path):
            verify_payload["repo_path"] = state.repo_path

        verify_result = await call_mcp_tool(
            "devcouncil", "devcouncil_verify_task", verify_payload
        )

        if not isinstance(verify_result, dict) or verify_result.get("ok") is not True:
            err_msg = (
                verify_result.get("error") or verify_result.get("code") or "verification call failed"
                if isinstance(verify_result, dict)
                else str(verify_result)
            )
            logger.error("DevCouncil verification call failed closed: %s", err_msg)
            return StopHookResult(
                decision=StopDecision.CONTINUE,
                reason=(
                    f"### DevCouncil Verification Unavailable (Hero Loop Attempt {continuation_count + 1}/{state.continuation_threshold})\n"
                    f"`devcouncil_verify_task` failed closed and did not produce a passing verdict: {err_msg}\n\n"
                    "Do NOT declare completion on an unverified task. Check the DevCouncil MCP server "
                    "or run `dev tool call devcouncil_verify_task` from the shell and resolve the failure."
                ),
            )

        passed = verify_result.get("passed") is True
        status = str(verify_result.get("status", "")).lower()
        raw_gaps = verify_result.get("blocking_gaps")
        blocking_gaps: List[Dict[str, Any]] = (
            [g for g in raw_gaps if isinstance(g, dict)]
            if isinstance(raw_gaps, list)
            else []
        )
        raw_actions = verify_result.get("next_actions")
        next_actions: List[Dict[str, Any]] = (
            [a for a in raw_actions if isinstance(a, dict)]
            if isinstance(raw_actions, list)
            else []
        )
        raw_degraded = verify_result.get("degraded_reasons")
        degraded_reasons: List[str] = (
            [str(r) for r in raw_degraded if r]
            if isinstance(raw_degraded, list)
            else []
        )

        if passed and not blocking_gaps and status != "degraded" and not degraded_reasons:
            logger.info("DevCouncil Verification PASSED with 0 blocking gaps. Completion approved.")
            return StopHookResult(decision=StopDecision.ALLOW_STOP)

        # 4. Invariant B2: Construct structured self-repair diagnostic prompt
        repair_prompt_lines = [
            f"### DevCouncil Verification Gate Refusal (Hero Loop Attempt {continuation_count + 1}/{state.continuation_threshold})",
            "Your proposed completion was rejected by deterministic verification because blocking gaps remain in your task implementation:",
            "",
        ]

        if status == "degraded" or degraded_reasons:
            repair_prompt_lines.append("#### Degraded Verification Checks (Never Passed):")
            if degraded_reasons:
                for reason in degraded_reasons:
                    repair_prompt_lines.append(f"- {reason}")
            else:
                repair_prompt_lines.append("- Verification status is `degraded`; required checks did not complete.")
            repair_prompt_lines.append("")

        repair_prompt_lines.append("#### Blocking Gaps:")
        if not blocking_gaps:
            repair_prompt_lines.append("- [BLOCKING] `VERIFICATION_FAILED`: Task gates did not pass.")
        else:
            for gap in blocking_gaps:
                gap_id = gap.get("id") or "GAP_UNKNOWN"
                severity = gap.get("severity") or "BLOCKING"
                description = gap.get("description") or "No description provided."
                location = _format_location(gap)
                loc_suffix = f" ({location})" if location else ""
                repair_prompt_lines.append(f"- [{severity}] `{gap_id}`{loc_suffix}: {description}")

        repair_prompt_lines.append("")
        repair_prompt_lines.append("#### Required Typed Next Actions:")
        if not next_actions:
            repair_prompt_lines.append("- **REPAIR**: Inspect test failures, fix failing assertions, and re-run verification.")
        else:
            for act in next_actions:
                category = act.get("category") or "repair"
                action_text = act.get("action") or "Inspect and resolve the blocking gap."
                target = _format_location(act) or str(act.get("gap_id") or "")
                suggested_cmd = act.get("suggested_command")
                cmd_suffix = f" (run: `{suggested_cmd}`)" if suggested_cmd else ""
                if target:
                    repair_prompt_lines.append(
                        f"- **{category}** (`{target}`): {action_text}{cmd_suffix}"
                    )
                else:
                    repair_prompt_lines.append(f"- **{category}**: {action_text}{cmd_suffix}")

        repair_prompt_lines.extend([
            "",
            "**MANDATORY NEXT STEP**:",
            "Do NOT terminate or summarize your work yet.",
            "1. Inspect the failing files and tests identified above.",
            "2. Execute the required typed next actions to resolve each blocking gap.",
            "3. Re-run tests to confirm that your repairs succeed.",
            "4. Only attempt completion once all blocking gaps have been resolved.",
        ])

        reason_text = "\n".join(repair_prompt_lines)
        logger.info(
            "Hero Loop self-repair triggered. Resuming agent loop with %d typed actions.",
            len(next_actions),
        )

        return StopHookResult(
            decision=StopDecision.CONTINUE,
            reason=reason_text,
        )

    except Exception as exc:
        logger.critical("Error during DevCouncil verification check: %s", exc, exc_info=True)
        # Fail safe: request manual verification rather than dropping the gate silently
        return StopHookResult(
            decision=StopDecision.CONTINUE,
            reason=f"Verification gate execution encountered an unexpected error: {exc}. Please verify task status manually.",
        )


if __name__ == "__main__":
    test_args = StopArgs(continuation_count=0)
    result = asyncio.run(stop_hook(test_args))
    print(f"Stop hook decision: {result.decision}")
    if result.reason:
        print(f"Reason:\n{result.reason}")
