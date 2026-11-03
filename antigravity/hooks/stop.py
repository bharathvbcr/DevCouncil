"""
stop.py - Antigravity Agent Lifecycle Hook: The Hero Loop (Deterministic Self-Repair).

Intercepts agent completion and invokes devcouncil_verify_task.
If blocking verification gaps exist, returns StopDecision.CONTINUE with
typed next actions to drive autonomous self-repair.
"""

import asyncio
import json
import logging
import os
import sys
from typing import Any, Dict, List, Optional, Union

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


@ag_hooks.stop
async def stop_hook(data: Union[StopArgs, Any] = None) -> StopHookResult:
    """
    Executes when the root trajectory reaches idle and the agent attempts to stop.
    Invokes DevCouncil deterministic verification gates.

    If blocking gaps exist:
      - Intercepts termination.
      - Injects typed next_actions (e.g. fix_code, add_test, fix_stub).
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
        verify_payload = {
            "task_id": state.task_id,
            "lease_token": state.lease_token,
        }
        verify_result = await call_mcp_tool(
            "devcouncil", "devcouncil_verify_task", verify_payload
        )

        passed = verify_result.get("passed", False)
        blocking_gaps: List[Dict[str, Any]] = verify_result.get("blocking_gaps", [])
        typed_next_actions: List[Dict[str, Any]] = verify_result.get("typed_next_actions", [])

        if passed and not blocking_gaps:
            logger.info("DevCouncil Verification PASSED with 0 blocking gaps. Completion approved.")
            return StopHookResult(decision=StopDecision.ALLOW_STOP)

        # 4. Invariant B2: Construct structured self-repair diagnostic prompt
        repair_prompt_lines = [
            f"### DevCouncil Verification Gate Refusal (Hero Loop Attempt {continuation_count + 1}/{state.continuation_threshold})",
            "Your proposed completion was rejected by deterministic verification because blocking gaps remain in your task implementation:",
            "",
            "#### Blocking Gaps:",
        ]

        if not blocking_gaps:
            repair_prompt_lines.append("- [BLOCKING] `VERIFICATION_FAILED`: Task gates did not pass.")
        else:
            for gap in blocking_gaps:
                gap_id = gap.get("gap_id", "GAP_UNKNOWN")
                severity = gap.get("severity", "BLOCKING")
                message = gap.get("message", "No description provided.")
                repair_prompt_lines.append(f"- [{severity}] `{gap_id}`: {message}")

        repair_prompt_lines.append("")
        repair_prompt_lines.append("#### Required Typed Next Actions:")
        if not typed_next_actions:
            repair_prompt_lines.append("- **REPAIR**: Inspect test failures, fix failing assertions, and re-run verification.")
        else:
            for action in typed_next_actions:
                act_type = action.get("type", "REPAIR")
                target = action.get("target", "")
                instruction = action.get("instruction", "")
                repair_prompt_lines.append(f"- **{act_type}** (`{target}`): {instruction}")

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
            len(typed_next_actions),
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
