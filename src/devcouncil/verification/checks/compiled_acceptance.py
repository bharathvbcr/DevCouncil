"""Compiled per-criterion acceptance check orchestration extracted from Verifier."""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Dict, List, Tuple

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task

logger = logging.getLogger(__name__)


@dataclass
class CompiledAcceptanceResult:
    gaps: List[Gap] = field(default_factory=list)
    compiled_pass: Dict[str, bool] = field(default_factory=dict)
    compiled_cmds_by_ac: Dict[str, List[str]] = field(default_factory=dict)
    failing_results_by_ac: Dict[str, List[CommandResult]] = field(default_factory=dict)
    compiled_vote: Dict[str, Tuple[int, int, bool]] = field(default_factory=dict)
    inconclusive_acs: set[str] = field(default_factory=set)
    command_results: List[CommandResult] = field(default_factory=list)
    evidence: List[Any] = field(default_factory=list)
    genuine_failure: bool = False
    had_unrunnable: bool = False


async def run_compiled_acceptance_checks(
    *,
    task: Task,
    requirements: List[Requirement],
    compile_future: Any | None,
    diff_content: str,
    ac_repair_attempts: int,
    acceptance_compiler: Any | None,
    command_applicable: Callable[[str], Tuple[bool, str]],
    run_command: Callable[[str], CommandResult],
    command_is_malformed: Callable[[CommandResult], bool],
    failure_location: Callable[[CommandResult], Tuple[str | None, int | None]],
    next_gap_id: Callable[[str, str], str],
    repair_command: Callable[
        [str, str, str, str, str],
        Awaitable[str | None],
    ] | None = None,
    had_unrunnable: bool = False,
    genuine_failure: bool = False,
) -> CompiledAcceptanceResult:
    """Run DevCouncil-owned per-criterion checks from the acceptance compiler."""
    result = CompiledAcceptanceResult(
        had_unrunnable=had_unrunnable,
        genuine_failure=genuine_failure,
    )
    if compile_future is None:
        return result

    try:
        compiled = await compile_future
    except Exception as exc:  # pragma: no cover - best effort
        logger.warning("Acceptance compiler failed for %s: %s", task.id, exc)
        compiled = {}

    ac_meta = {ac.id: ac for req in requirements for ac in req.acceptance_criteria}
    for ac_id, raw_cmds in compiled.items():
        candidates = [c for c in raw_cmds if command_applicable(c)[0]]
        result.compiled_cmds_by_ac[ac_id] = list(candidates)
        if not candidates:
            result.had_unrunnable = True
            continue

        passes = 0
        genuine_fails = 0
        repaired = False
        fail_results: List[Tuple[str, CommandResult]] = []
        for cmd in candidates:
            cmd_result = run_command(cmd)
            result.command_results.append(cmd_result)
            result.evidence.append(cmd_result)
            attempts = 0
            while (
                cmd_result.exit_code != 0
                and command_is_malformed(cmd_result)
                and attempts < ac_repair_attempts
                and repair_command is not None
            ):
                attempts += 1
                ac_desc = ac_meta[ac_id].description if ac_id in ac_meta else ac_id
                try:
                    fixed = await repair_command(
                        ac_id, ac_desc, cmd, cmd_result.summary[:800], diff_content
                    )
                except Exception:
                    fixed = None
                if not fixed or not command_applicable(fixed)[0]:
                    break
                cmd = fixed
                result.compiled_cmds_by_ac[ac_id].append(cmd)
                cmd_result = run_command(cmd)
                result.command_results.append(cmd_result)
                result.evidence.append(cmd_result)
            if cmd_result.exit_code == 0:
                passes += 1
                if attempts > 0:
                    repaired = True
            elif command_is_malformed(cmd_result):
                result.had_unrunnable = True
                result.failing_results_by_ac.setdefault(ac_id, []).append(cmd_result)
            else:
                genuine_fails += 1
                fail_results.append((cmd, cmd_result))
                result.failing_results_by_ac.setdefault(ac_id, []).append(cmd_result)

        decisive = passes + genuine_fails
        if decisive == 0:
            continue

        ac_proven = passes > genuine_fails
        result.compiled_pass[ac_id] = ac_proven
        if ac_proven:
            result.compiled_vote[ac_id] = (passes, decisive, repaired)
            continue
        if passes == 0 and genuine_fails > 0:
            result.genuine_failure = True
            cmd, cmd_result = fail_results[0]
            fail_file, fail_line = failure_location(cmd_result)
            agree = (
                f" {genuine_fails}/{decisive} independent checks agreed it fails."
                if decisive > 1 else ""
            )
            result.gaps.append(Gap(
                id=next_gap_id(task.id, "ACCHK"),
                severity="high",
                gap_type="test_failed",
                task_id=task.id,
                description=(
                    f"Acceptance check for {ac_id} failed: '{cmd}' "
                    f"(exit {cmd_result.exit_code}).{agree}"
                ),
                evidence=[cmd_result.summary[:500]],
                recommended_fix=f"Fix the implementation so acceptance criterion {ac_id} holds.",
                blocking=True,
                acceptance_criterion_id=ac_id,
                suggested_command=cmd,
                file=fail_file,
                line=fail_line,
                stdout_path=cmd_result.stdout_path or None,
                stderr_path=cmd_result.stderr_path or None,
            ))
        elif passes > 0 and genuine_fails > 0:
            result.inconclusive_acs.add(ac_id)

    return result


def promote_demoted_failures_when_compiler_incomplete(
    *,
    task: Task,
    compiler_active: bool,
    compiled_cmds_by_ac: Dict[str, List[str]],
    demoted_failures: List[Gap],
    genuine_failure: bool,
) -> bool:
    """Re-promote demoted planner test failures when compile coverage is partial."""
    compiler_covered_all = bool(task.acceptance_criterion_ids) and all(
        compiled_cmds_by_ac.get(ac_id) for ac_id in task.acceptance_criterion_ids
    )
    if compiler_active and not compiler_covered_all and demoted_failures:
        for gap in demoted_failures:
            gap.blocking = True
            gap.severity = "high"
            genuine_failure = True
            logger.info(
                "Re-promoted demoted test failure %s to blocking: acceptance compiler "
                "did not produce a check for every criterion of task %s.",
                gap.id, task.id,
            )
    return genuine_failure
