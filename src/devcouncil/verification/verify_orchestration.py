"""Core verify_task gate orchestration extracted from Verifier."""

from __future__ import annotations

import logging
from typing import Any, List, TYPE_CHECKING

from devcouncil.domain.evidence import CommandResult, DiffEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.live.cards import unresolved_blocking_cards
from devcouncil.telemetry.stages import log_step
from devcouncil.verification.checks.acceptance_evidence import map_acceptance_criteria_evidence
from devcouncil.verification.checks.compiled_acceptance import (
    promote_demoted_failures_when_compiler_incomplete,
    run_compiled_acceptance_checks,
)
from devcouncil.verification.checks.command_evidence import run_verification_commands
from devcouncil.verification.checks.diff_coverage_gate import run_diff_coverage_gate
from devcouncil.verification.checks.planned_files import (
    detect_dependency_risk_gaps,
    detect_no_work_gap,
    detect_planned_file_gaps,
)
from devcouncil.verification.checks.orphan_diff import detect_orphan_diff_gaps
from devcouncil.verification.checks.stub_scan import detect_stub_gaps
from devcouncil.verification.effort_heuristics import detect_effort_anomalies
from devcouncil.verification.verify_setup import VerifyRunContext

if TYPE_CHECKING:
    from devcouncil.verification.verifier import Verifier

logger = logging.getLogger(__name__)


async def run_verify_orchestration(
    verifier: "Verifier",
    task: Task,
    requirements: List[Requirement],
    ctx: VerifyRunContext,
    compile_future: Any,
    review_future: Any,
) -> tuple[list[Gap], list[Any]]:
    """Run all verification gates and return gaps plus evidence to persist."""
    gaps: list[Gap] = []
    evidence_to_save: list[Any] = []
    _cfg = ctx.config
    rigor = ctx.rigor
    ac_repair_attempts = ctx.ac_repair_attempts
    changed_files = ctx.changed_files
    diff_content = ctx.diff_content
    diff_empty = ctx.diff_empty
    coverage_measured = False
    coverage_skipped_reason: str | None = None

    work_present = (not diff_empty) or verifier._task_produced_changes(task.id)

    no_work_gap = detect_no_work_gap(
        task=task,
        work_present=work_present,
        next_gap_id=verifier._next_gap_id,
    )
    if no_work_gap is not None:
        gaps.append(no_work_gap)

    if diff_content:
        added_files, deleted_files = verifier._classify_change_paths(changed_files)
        diff_ev = DiffEvidence(
            task_id=task.id,
            changed_files=changed_files,
            added_files=added_files,
            deleted_files=deleted_files,
            diff_summary=f"Diff captured for {len(changed_files)} files.",
        )
        evidence_to_save.append(diff_ev)

    planned_paths = {pf.path for pf in task.planned_files}
    gaps.extend(detect_planned_file_gaps(
        task=task,
        changed_files=changed_files,
        next_gap_id=verifier._next_gap_id,
    ))

    gaps.extend(detect_orphan_diff_gaps(
        task=task,
        changed_files=changed_files,
        planned_paths=planned_paths,
        diff_content=diff_content,
        project_root=verifier.project_root,
        get_untracked_files=verifier._get_untracked_files,
        next_gap_id=verifier._next_gap_id,
        classify_fn=verifier._classify_change_paths,
    ))

    log_step(
        "verify/3: planned-file and orphan-diff checks",
        project_root=verifier.project_root,
        task_id=task.id,
        gaps_so_far=len(gaps),
    )
    gaps.extend(verifier._check_semantic_diff(task, requirements))

    gaps.extend(detect_dependency_risk_gaps(
        task=task,
        changed_files=changed_files,
        planned_paths=planned_paths,
        next_gap_id=verifier._next_gap_id,
    ))

    log_step(
        "verify/4: semantic diff and dependency checks",
        project_root=verifier.project_root,
        task_id=task.id,
        gaps_so_far=len(gaps),
    )
    gaps.extend(detect_stub_gaps(
        task=task,
        diff_content=diff_content,
        project_root=verifier.project_root,
        stub_enabled=rigor.stub_enabled,
        stub_blocking=rigor.stub_blocking,
        next_gap_id=verifier._next_gap_id,
    ))

    if rigor.effort_enabled and diff_content:
        for eff in detect_effort_anomalies(
            task, diff_content, requirements,
            min_added_lines_per_planned_file=rigor.min_added_lines_per_planned_file,
        ):
            gaps.append(Gap(
                id=verifier._next_gap_id(task.id, "EFFORT"),
                severity=eff.severity,
                gap_type="suspicious_effort",
                task_id=task.id,
                description=eff.detail,
                evidence=[eff.reason],
                recommended_fix=(
                    "Review whether the implementation actually fulfills the task scope; "
                    "complete the planned work (or restore removed tests), then re-verify."
                ),
                blocking=rigor.effort_blocking,
                file=eff.file,
            ))

    compiler_active = bool(verifier.acceptance_compiler and diff_content and task.acceptance_criterion_ids)
    log_step(
        "verify/5: stub and effort heuristics",
        project_root=verifier.project_root,
        task_id=task.id,
        compiler_active=compiler_active,
        gaps_so_far=len(gaps),
    )

    cmd_run = run_verification_commands(
        task=task,
        commands_for_task=verifier._commands_for_task(task),
        compiler_active=compiler_active,
        command_applicable=verifier._command_applicable,
        run_command=lambda cmd: verifier._run_command(cmd, task_id=task.id),
        command_can_prove_acceptance=verifier._command_can_prove_acceptance,
        command_is_malformed=verifier._command_is_malformed,
        failure_location=verifier._failure_location,
        is_quality_only_command=verifier._is_quality_only_command,
        next_gap_id=verifier._next_gap_id,
        retry_flaky=bool(_cfg.verification.retry_flaky) if _cfg is not None else True,
    )
    gaps.extend(cmd_run.gaps)
    command_results: list[CommandResult] = list(cmd_run.command_results)
    evidence_results: list[CommandResult] = list(cmd_run.evidence_results)
    genuine_failure = cmd_run.genuine_failure
    had_unrunnable = cmd_run.had_unrunnable
    demoted_failures: list[Gap] = list(cmd_run.demoted_failures)
    evidence_to_save.extend(command_results)

    log_step(
        "verify/6: verification commands",
        project_root=verifier.project_root,
        task_id=task.id,
        commands_run=len(command_results),
        gaps_so_far=len(gaps),
    )
    _repair_fn = getattr(verifier.acceptance_compiler, "repair", None)
    compiled_run = await run_compiled_acceptance_checks(
        task=task,
        requirements=requirements,
        compile_future=compile_future,
        diff_content=diff_content,
        ac_repair_attempts=ac_repair_attempts,
        acceptance_compiler=verifier.acceptance_compiler,
        command_applicable=verifier._command_applicable,
        run_command=lambda cmd: verifier._run_command(cmd, task_id=task.id),
        command_is_malformed=verifier._command_is_malformed,
        failure_location=verifier._failure_location,
        next_gap_id=verifier._next_gap_id,
        repair_command=_repair_fn,
        had_unrunnable=had_unrunnable,
        genuine_failure=genuine_failure,
    )
    gaps.extend(compiled_run.gaps)
    command_results.extend(compiled_run.command_results)
    evidence_to_save.extend(compiled_run.evidence)
    had_unrunnable = compiled_run.had_unrunnable
    genuine_failure = compiled_run.genuine_failure
    compiled_pass = compiled_run.compiled_pass
    compiled_cmds_by_ac = compiled_run.compiled_cmds_by_ac
    failing_results_by_ac = compiled_run.failing_results_by_ac
    compiled_vote = compiled_run.compiled_vote
    inconclusive_acs = compiled_run.inconclusive_acs
    genuine_failure = promote_demoted_failures_when_compiler_incomplete(
        task=task,
        compiler_active=compiler_active,
        compiled_cmds_by_ac=compiled_cmds_by_ac,
        demoted_failures=demoted_failures,
        genuine_failure=genuine_failure,
    )

    log_step(
        "verify/7: compiled acceptance checks",
        project_root=verifier.project_root,
        task_id=task.id,
        compiled_criteria=len(compiled_pass),
        gaps_so_far=len(gaps),
    )
    ac_evidence = map_acceptance_criteria_evidence(
        task=task,
        requirements=requirements,
        evidence_results=evidence_results,
        compiled_pass=compiled_pass,
        compiled_cmds_by_ac=compiled_cmds_by_ac,
        failing_results_by_ac=failing_results_by_ac,
        compiled_vote=compiled_vote,
        inconclusive_acs=inconclusive_acs,
        work_present=work_present,
        genuine_failure=genuine_failure,
        had_unrunnable=had_unrunnable,
        rigor=rigor,
        is_quality_only_command=verifier._is_quality_only_command,
        next_gap_id=verifier._next_gap_id,
    )
    gaps.extend(ac_evidence.gaps)
    evidence_to_save.extend(ac_evidence.evidence)

    log_step(
        "verify/8: acceptance-criteria evidence mapping",
        project_root=verifier.project_root,
        task_id=task.id,
        gaps_so_far=len(gaps),
    )
    measure_cov, enforce_cov, min_ratio = verifier._diff_coverage_settings()
    if rigor.enforce_coverage:
        enforce_cov = True
    any_passing = any(
        result.exit_code == 0 and not verifier._is_quality_only_command(result.command)
        for result in evidence_results
    ) or any(compiled_pass.values())
    dc_result = run_diff_coverage_gate(
        task=task,
        diff_content=diff_content,
        any_passing=any_passing,
        enforce_cov=enforce_cov,
        measure_cov=measure_cov,
        min_ratio=min_ratio,
        measure_diff_coverage=verifier.measure_diff_coverage,
        coverage_target_commands=verifier._coverage_target_commands,
        next_gap_id=verifier._next_gap_id,
    )
    gaps.extend(dc_result.gaps)
    evidence_to_save.extend(dc_result.evidence)
    coverage_measured = dc_result.coverage_measured
    coverage_skipped_reason = dc_result.coverage_skipped_reason

    log_step(
        "verify/9: diff coverage",
        project_root=verifier.project_root,
        task_id=task.id,
        coverage_measured=coverage_measured,
        gaps_so_far=len(gaps),
    )
    if diff_content:
        gaps.extend(verifier.secret_scanner.scan_diff(diff_content, task.id))

    log_step(
        "verify/10: secret scan",
        project_root=verifier.project_root,
        task_id=task.id,
        gaps_so_far=len(gaps),
    )
    if review_future is not None:
        try:
            review_result = await review_future
            for finding in review_result.findings:
                finding.id = verifier._next_gap_id(task.id, "REVIEW")
                finding.blocking = bool(
                    rigor.reviewer_required and finding.severity == "critical"
                )
                gaps.append(finding)
        except Exception as exc:
            logger.error("Implementation review failed: %s", exc)

    log_step(
        "verify/11: implementation review",
        project_root=verifier.project_root,
        task_id=task.id,
        gaps_so_far=len(gaps),
    )
    for card in unresolved_blocking_cards(verifier.project_root, task_id=task.id):
        gaps.append(Gap(
            id=verifier._next_gap_id(task.id, "LIVE"),
            severity="critical",
            gap_type="architecture_drift",
            task_id=task.id,
            description=f"Open critical live-review card remains: {card.summary}",
            evidence=[card.id, card.message_for_agent],
            recommended_fix=(
                f"Address the critique card, then run `dev watch resolve {card.id}` "
                "or mark it ignored with justification outside the verification gate."
            ),
            blocking=True,
        ))

    blocking_count = len([g for g in gaps if g.blocking])
    log_step(
        "verify/complete",
        project_root=verifier.project_root,
        task_id=task.id,
        total_gaps=len(gaps),
        blocking_gaps=blocking_count,
        evidence_items=len(evidence_to_save),
        trace=True,
    )
    from devcouncil.verification.verifier import VerificationOutcome

    verifier.last_outcome = VerificationOutcome(
        mode="compiled" if verifier.acceptance_compiler else "coarse",
        compiler_active=compiler_active,
        diff_empty=diff_empty,
        coverage_measured=coverage_measured,
        coverage_skipped_reason=coverage_skipped_reason,
        difficulty=rigor.difficulty,
        rigor_applied=list(rigor.applied),
    )
    return gaps, evidence_to_save
