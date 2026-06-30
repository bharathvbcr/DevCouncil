"""Artifact graph: the persistent, directed graph that links requirements to evidence.

This is DevCouncil's core data structure:
  Requirement -> AcceptanceCriterion -> Task -> PlannedFile -> ChangedFile
              -> CommandResult -> TestEvidence -> Gap

The graph enables coverage queries like:
  - Which requirements have no tasks?
  - Which tasks produced no changed files?
  - Which acceptance criteria have no evidence?
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Set, Tuple

from devcouncil.domain.requirement import Requirement, AcceptanceCriterion
from devcouncil.domain.task import Task
from devcouncil.domain.assumption import Assumption
from devcouncil.domain.evidence import CommandResult, DiffCoverageEvidence, DiffEvidence, TestEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.critique import CritiqueFinding


@dataclass
class ArtifactGraph:
    """Directed graph of all planning and execution artifacts."""

    requirements: Dict[str, Requirement] = field(default_factory=dict)
    tasks: Dict[str, Task] = field(default_factory=dict)
    assumptions: Dict[str, Assumption] = field(default_factory=dict)
    findings: Dict[str, CritiqueFinding] = field(default_factory=dict)
    gaps: Dict[str, Gap] = field(default_factory=dict)
    test_evidence: List[TestEvidence] = field(default_factory=list)
    diff_evidence: List[DiffEvidence] = field(default_factory=list)
    command_results: List[CommandResult] = field(default_factory=list)
    diff_coverage_evidence: List[DiffCoverageEvidence] = field(default_factory=list)

    # --- Mutation ---

    def add_requirement(self, req: Requirement) -> None:
        self.requirements[req.id] = req

    def add_task(self, task: Task) -> None:
        self.tasks[task.id] = task

    def add_assumption(self, asm: Assumption) -> None:
        self.assumptions[asm.id] = asm

    def add_finding(self, finding: CritiqueFinding) -> None:
        self.findings[finding.id] = finding

    def add_gap(self, gap: Gap) -> None:
        self.gaps[gap.id] = gap

    def add_test_evidence(self, ev: TestEvidence) -> None:
        self.test_evidence.append(ev)

    def add_diff_evidence(self, ev: DiffEvidence) -> None:
        self.diff_evidence.append(ev)

    def add_command_result(self, cr: CommandResult) -> None:
        self.command_results.append(cr)

    def add_diff_coverage_evidence(self, ev: DiffCoverageEvidence) -> None:
        self.diff_coverage_evidence.append(ev)

    def diff_coverage_findings(self) -> List[DiffCoverageEvidence]:
        """Measured diff-coverage runs where the changed lines were NOT fully exercised
        — i.e. a green suite that did not actually run the new code."""
        return [
            ev for ev in self.diff_coverage_evidence
            if ev.measured and ev.changed_lines and ev.covered_lines < ev.changed_lines
        ]

    # --- Coverage Queries ---

    def requirements_without_tasks(self) -> List[Requirement]:
        """Requirements that are not mapped to any task."""
        task_req_ids: Set[str] = set()
        for task in self.tasks.values():
            task_req_ids.update(task.requirement_ids)
        return [r for r in self.requirements.values() if r.id not in task_req_ids]

    def tasks_without_requirements(self) -> List[Task]:
        """Tasks that don't map back to any requirement."""
        return [t for t in self.tasks.values() if not t.requirement_ids]

    def requirements_without_acceptance_criteria(self) -> List[Requirement]:
        """Requirements with no acceptance criteria defined."""
        return [r for r in self.requirements.values() if not r.acceptance_criteria]

    def acceptance_criteria_without_evidence(self) -> List[Tuple[str, AcceptanceCriterion]]:
        """AC IDs that have no *passing* test evidence mapped to them.

        Only ``passed`` evidence counts: a failed or not-run check is not proof, so
        it must not remove a criterion from the unproven list (which would feed a
        falsely-green coverage summary to ``dev status`` and the MCP surface).
        """
        evidenced_ac_ids: Set[str] = set()
        for ev in self.test_evidence:
            if getattr(ev, "status", "passed") == "passed":
                evidenced_ac_ids.add(ev.acceptance_criterion_id)

        results: List[Tuple[str, AcceptanceCriterion]] = []
        for req in self.requirements.values():
            for ac in req.acceptance_criteria:
                if ac.id not in evidenced_ac_ids:
                    results.append((req.id, ac))
        return results

    def tasks_without_changed_files(self) -> List[Task]:
        """Tasks that have not produced any diff evidence."""
        tasks_with_diffs: Set[str] = set()
        for de in self.diff_evidence:
            tasks_with_diffs.add(de.task_id)
        return [
            t for t in self.tasks.values()
            if t.id not in tasks_with_diffs and t.status not in ("planned", "ready")
        ]

    def open_findings(self, min_severity: str = "low") -> List[CritiqueFinding]:
        """Critique findings that are still open."""
        severity_order = {"low": 0, "medium": 1, "high": 2, "critical": 3}
        min_rank = severity_order.get(min_severity, 0)
        return [
            f for f in self.findings.values()
            if f.status == "open" and severity_order.get(f.severity, 0) >= min_rank
        ]

    def blocking_gaps(self) -> List[Gap]:
        """Gaps that are blocking progress."""
        return [g for g in self.gaps.values() if g.blocking]

    def unconfirmed_high_impact_assumptions(self) -> List[Assumption]:
        """Assumptions with high impact that are still open."""
        return [
            a for a in self.assumptions.values()
            if a.status == "open" and a.impact == "high"
        ]

    # --- Aggregate ---

    def coverage_summary(self) -> Dict[str, Any]:
        """Produce a coverage summary for reporting."""
        # Single pass over open findings; the "high"-filtered count reuses the same
        # severity_order/rank predicate as open_findings("high") (rank >= 2).
        all_open = self.open_findings()
        severity_order = {"low": 0, "medium": 1, "high": 2, "critical": 3}
        high_critical_open = sum(
            1 for f in all_open if severity_order.get(f.severity, 0) >= severity_order["high"]
        )
        return {
            "total_requirements": len(self.requirements),
            "requirements_without_tasks": len(self.requirements_without_tasks()),
            "requirements_without_ac": len(self.requirements_without_acceptance_criteria()),
            "total_tasks": len(self.tasks),
            "tasks_without_requirements": len(self.tasks_without_requirements()),
            "tasks_without_diffs": len(self.tasks_without_changed_files()),
            "total_ac": sum(len(r.acceptance_criteria) for r in self.requirements.values()),
            "ac_without_evidence": len(self.acceptance_criteria_without_evidence()),
            "total_gaps": len(self.gaps),
            "blocking_gaps": len(self.blocking_gaps()),
            "diff_coverage_runs": len(self.diff_coverage_evidence),
            "unexercised_diff_findings": len(self.diff_coverage_findings()),
            "open_findings": len(all_open),
            "high_critical_open_findings": high_critical_open,
            "unconfirmed_high_assumptions": len(self.unconfirmed_high_impact_assumptions()),
        }
