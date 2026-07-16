"""Expected verification command execution extracted from Verifier."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Dict, List, Tuple

from devcouncil.domain.evidence import CommandResult
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task


@dataclass
class CommandEvidenceRunResult:
    gaps: List[Gap] = field(default_factory=list)
    command_results: List[CommandResult] = field(default_factory=list)
    evidence_results: List[CommandResult] = field(default_factory=list)
    genuine_failure: bool = False
    had_unrunnable: bool = False
    demoted_failures: List[Gap] = field(default_factory=list)


def run_verification_commands(
    *,
    task: Task,
    commands_for_task: Dict[str, List[str]],
    compiler_active: bool,
    command_applicable: Callable[[str], Tuple[bool, str]],
    run_command: Callable[[str], CommandResult],
    command_can_prove_acceptance: Callable[[str, str], bool],
    command_is_malformed: Callable[[CommandResult], bool],
    failure_location: Callable[[CommandResult], Tuple[str | None, int | None]],
    is_quality_only_command: Callable[[str], bool],
    next_gap_id: Callable[[str, str], str],
    retry_flaky: bool = True,
) -> CommandEvidenceRunResult:
    """Run planner/config verification commands and collect gaps + evidence.

    With ``retry_flaky`` (config ``verification.retry_flaky``, default on), a genuinely
    failing acceptance-capable command gets ONE immediate re-run; a pass on the re-run
    counts as passed with its summary tagged ``[flaky: passed on retry 2/2]`` so
    next-actions/reports can distinguish a flaky pass from a stable one.
    """
    result = CommandEvidenceRunResult()
    for cmd_type, cmds in commands_for_task.items():
        for cmd in cmds:
            applicable, skip_reason = command_applicable(cmd)
            if not applicable:
                result.gaps.append(Gap(
                    id=next_gap_id(task.id, "SKIP"),
                    severity="low",
                    gap_type="skipped_verification_command",
                    task_id=task.id,
                    description=f"Skipped verification command '{cmd}': {skip_reason}.",
                    evidence=[skip_reason],
                    recommended_fix=(
                        "Replace it with a command for this repo's stack, or remove it "
                        "from .devcouncil/config.yaml / the task's expected_tests."
                    ),
                    blocking=False,
                    suggested_command=cmd,
                ))
                continue
            cmd_result = run_command(cmd)
            can_prove = command_can_prove_acceptance(cmd_type, cmd)
            if (
                retry_flaky
                and can_prove
                and cmd_result.exit_code > 0
                and not cmd_result.timed_out
                and not command_is_malformed(cmd_result)
            ):
                # Flaky-evidence retry: one immediate re-run of a genuinely failing
                # acceptance-capable command. Only exit codes > 0 qualify — the
                # synthetic -1 (unlaunchable / timed out) and malformed commands
                # prove nothing and would fail identically or burn a second full
                # timeout window. On a failed re-run the ORIGINAL result is kept.
                retry_result = run_command(cmd)
                if retry_result.exit_code == 0:
                    cmd_result = retry_result.model_copy(update={
                        "summary": f"[flaky: passed on retry 2/2] {retry_result.summary}",
                    })
            result.command_results.append(cmd_result)
            if can_prove:
                result.evidence_results.append(cmd_result)
            if cmd_result.exit_code != 0:
                if command_is_malformed(cmd_result):
                    result.had_unrunnable = True
                    result.gaps.append(Gap(
                        id=next_gap_id(task.id, "BADCMD"),
                        severity="medium",
                        gap_type="invalid_verification_command",
                        task_id=task.id,
                        description=(
                            f"Verification command could not run (not a code failure): '{cmd}'. "
                            "It appears malformed or its tooling is unavailable, so this command "
                            "proves nothing either way."
                        ),
                        evidence=[cmd_result.summary[:500]],
                        recommended_fix=(
                            "Regenerate the task's verification commands with 'dev repair', or edit "
                            "them to be a single runnable command (e.g. 'python -m pytest <file>')."
                        ),
                        blocking=False,
                        suggested_command=cmd,
                        stdout_path=cmd_result.stdout_path or None,
                        stderr_path=cmd_result.stderr_path or None,
                    ))
                else:
                    is_quality_gate = cmd_type in {"lint", "typecheck"} or is_quality_only_command(cmd)
                    blocking = cmd_result.timed_out or (
                        (not compiler_active) and not is_quality_gate
                    )
                    if blocking:
                        result.genuine_failure = True
                    fail_file, fail_line = failure_location(cmd_result)
                    failure_label = (
                        "timed out after running for the configured limit"
                        if cmd_result.timed_out
                        else f"failed with exit code {cmd_result.exit_code}"
                    )
                    gap = Gap(
                        id=next_gap_id(task.id, cmd_type.upper()),
                        severity="high" if blocking else "medium",
                        gap_type="quality_gate_failed" if is_quality_gate else "test_failed",
                        task_id=task.id,
                        description=(
                            f"{'Quality gate' if is_quality_gate else 'Command'} '{cmd}' "
                            f"{failure_label}"
                            + (
                                " (advisory: style/type, not a correctness gate)."
                                if is_quality_gate and not cmd_result.timed_out
                                else "."
                            )
                        ),
                        evidence=[cmd_result.summary[:500]],
                        recommended_fix=f"Fix the issues reported by '{cmd}'.",
                        blocking=blocking,
                        suggested_command=cmd,
                        file=fail_file,
                        line=fail_line,
                        stdout_path=cmd_result.stdout_path or None,
                        stderr_path=cmd_result.stderr_path or None,
                    )
                    result.gaps.append(gap)
                    if compiler_active and not is_quality_gate and not blocking:
                        result.demoted_failures.append(gap)
    return result
