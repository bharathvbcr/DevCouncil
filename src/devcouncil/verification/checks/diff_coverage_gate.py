"""Diff↔coverage gate extracted from Verifier.verify_task."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, List, Optional

from devcouncil.domain.evidence import DiffCoverageEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.task import Task
from devcouncil.verification import diff_coverage as dc


@dataclass
class DiffCoverageGateResult:
    gaps: List[Gap]
    evidence: List[DiffCoverageEvidence]
    coverage_measured: bool
    coverage_skipped_reason: Optional[str]


def run_diff_coverage_gate(
    *,
    task: Task,
    diff_content: str,
    any_passing: bool,
    enforce_cov: bool,
    measure_cov: bool,
    min_ratio: float,
    measure_diff_coverage: Callable[[Task, str], dc.DiffCoverageResult],
    coverage_target_commands: Callable[[Task], List[str]],
    next_gap_id: Callable[[str, str], str],
) -> DiffCoverageGateResult:
    """Run the diff↔coverage gate and return gaps + evidence."""
    gaps: List[Gap] = []
    evidence: List[DiffCoverageEvidence] = []
    coverage_measured = False
    coverage_skipped_reason: Optional[str] = None

    if not measure_cov:
        coverage_skipped_reason = "diff coverage disabled in config"
    elif not diff_content:
        coverage_skipped_reason = "no diff to measure"
    elif not task.acceptance_criterion_ids:
        coverage_skipped_reason = "task has no acceptance criteria"
    elif not any_passing:
        coverage_skipped_reason = "no passing verification command to instrument"

    if measure_cov and diff_content and task.acceptance_criterion_ids and any_passing:
        cov = measure_diff_coverage(task, diff_content)
        if not cov.measured:
            coverage_skipped_reason = cov.reason or "diff coverage could not be measured"
        if cov.measured:
            coverage_measured = True
            coverage_skipped_reason = None
            evidence.append(DiffCoverageEvidence(
                task_id=task.id,
                tool=cov.tool,
                measured=True,
                changed_lines=cov.changed_executable_lines,
                covered_lines=cov.covered_changed_lines,
                coverage_ratio=cov.ratio,
                uncovered_by_file=cov.uncovered_by_file,
                absent_files=cov.absent_files,
                summary=cov.summary(),
            ))
            failing = cov.covered_changed_lines == 0 if min_ratio <= 0 else cov.ratio < min_ratio
            if failing:
                first_file = next(iter(cov.uncovered_by_file), None)
                first_lines = cov.uncovered_by_file.get(first_file or "", [])
                target_cmds = coverage_target_commands(task)
                gaps.append(Gap(
                    id=next_gap_id(task.id, "DIFFCOV"),
                    severity="high" if enforce_cov else "medium",
                    gap_type="diff_not_exercised",
                    task_id=task.id,
                    description=(
                        f"Verification commands passed but exercised "
                        f"{cov.covered_changed_lines}/{cov.changed_executable_lines} changed line(s): "
                        f"{cov.summary()}. The acceptance criteria are not proven because the new "
                        "logic was never executed by the tests."
                    ),
                    evidence=[cov.summary()] + [
                        f"{path}: lines {lines}" for path, lines in list(cov.uncovered_by_file.items())[:5]
                    ],
                    recommended_fix=(
                        "Add or extend a test that executes the changed lines, then re-verify. "
                        "A passing suite that does not run the new code is not acceptance evidence."
                    ),
                    blocking=enforce_cov,
                    file=first_file,
                    line=first_lines[0] if first_lines else None,
                    suggested_command=target_cmds[0] if target_cmds else None,
                ))

    return DiffCoverageGateResult(
        gaps=gaps,
        evidence=evidence,
        coverage_measured=coverage_measured,
        coverage_skipped_reason=coverage_skipped_reason,
    )
