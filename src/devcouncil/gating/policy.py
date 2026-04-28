from pydantic import BaseModel
from typing import Any, List, Optional
from pathlib import Path

from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.domain.gap import Gap
from devcouncil.domain.assumption import Assumption
from devcouncil.domain.critique import CritiqueFinding
from devcouncil.gating.checks.requirement_coverage import RequirementCoverageCheck
from devcouncil.gating.checks.planned_files_check import PlannedFilesCheck
from devcouncil.gating.checks.clean_git import CleanGitCheck

class GateResult(BaseModel):
    passed: bool
    gaps: List[Gap]

class GatePolicy:
    """Central engine for executing project and task level quality gates."""
    
    def __init__(self):
        self.req_coverage = RequirementCoverageCheck()
        self.planned_files = PlannedFilesCheck()
        self.clean_git = CleanGitCheck()

    def check_plan_approval(
        self,
        requirements: List[Requirement],
        tasks: List[Task],
        assumptions: Optional[List[Assumption]] = None,
        findings: Optional[List[CritiqueFinding]] = None,
        blocking_questions: Optional[List[Any]] = None,
    ) -> GateResult:
        """Determines if the overall project plan is ready for execution."""
        gaps = []
        known_req_ids = {req.id for req in requirements}
        known_ac_ids = {
            ac.id
            for req in requirements
            for ac in req.acceptance_criteria
        }
        
        # 1. Check requirement coverage
        gaps.extend(self.req_coverage.check(requirements, tasks))
        
        # 2. Check for acceptance criteria presence
        for req in requirements:
            if not req.acceptance_criteria:
                gaps.append(Gap(
                    id=f"GAP-PLAN-{req.id}-NO-AC",
                    severity="high",
                    gap_type="requirement_not_planned",
                    requirement_id=req.id,
                    description=f"Requirement {req.id} has no acceptance criteria.",
                    recommended_fix="Define at least one measurable AC.",
                    blocking=True
                ))

            for ac in req.acceptance_criteria:
                if not ac.verification_method:
                    gaps.append(Gap(
                        id=f"GAP-PLAN-{ac.id}-NO-VERIFY",
                        severity="high",
                        gap_type="acceptance_criteria_unproven",
                        requirement_id=req.id,
                        description=f"Acceptance criterion {ac.id} has no verification method.",
                        recommended_fix="Define a deterministic verification method for this AC.",
                        blocking=True,
                    ))

        for task in tasks:
            if not task.requirement_ids:
                gaps.append(Gap(
                    id=f"GAP-PLAN-{task.id}-NO-REQ",
                    severity="high",
                    gap_type="requirement_not_planned",
                    task_id=task.id,
                    description=f"Task {task.id} is not mapped to any requirement.",
                    recommended_fix="Map each task to at least one requirement.",
                    blocking=True,
                ))

            unknown_req_ids = [req_id for req_id in task.requirement_ids if req_id not in known_req_ids]
            if unknown_req_ids:
                gaps.append(Gap(
                    id=f"GAP-PLAN-{task.id}-UNKNOWN-REQ",
                    severity="high",
                    gap_type="requirement_not_planned",
                    task_id=task.id,
                    description=f"Task {task.id} references unknown requirement(s): {', '.join(unknown_req_ids)}.",
                    recommended_fix="Remove invalid requirement links or add the missing requirements.",
                    blocking=True,
                ))

            unknown_ac_ids = [ac_id for ac_id in task.acceptance_criterion_ids if ac_id not in known_ac_ids]
            if unknown_ac_ids:
                gaps.append(Gap(
                    id=f"GAP-PLAN-{task.id}-UNKNOWN-AC",
                    severity="high",
                    gap_type="acceptance_criteria_unproven",
                    task_id=task.id,
                    description=f"Task {task.id} references unknown acceptance criteria: {', '.join(unknown_ac_ids)}.",
                    recommended_fix="Link tasks only to acceptance criteria declared by requirements.",
                    blocking=True,
                ))

        for assumption in assumptions or []:
            if (
                assumption.impact == "high"
                and assumption.status == "open"
                and assumption.requires_user_confirmation
            ):
                gaps.append(Gap(
                    id=f"GAP-PLAN-{assumption.id}-OPEN",
                    severity="high",
                    gap_type="assumption_violated",
                    requirement_id=assumption.linked_requirement_ids[0] if assumption.linked_requirement_ids else None,
                    description=f"High-impact assumption {assumption.id} is still open: {assumption.statement}",
                    recommended_fix="Confirm, reject, or convert this assumption before approving the plan.",
                    blocking=True,
                ))

        for finding in findings or []:
            if finding.severity in {"high", "critical"} and finding.status == "open":
                gaps.append(Gap(
                    id=f"GAP-PLAN-{finding.id}-OPEN",
                    severity=finding.severity,
                    gap_type="architecture_drift",
                    requirement_id=finding.linked_requirement_id,
                    description=f"Open {finding.severity} critique finding remains: {finding.claim}",
                    recommended_fix="Convert, rebut with evidence, or mark this finding resolved before approval.",
                    blocking=True,
                ))

        for question in blocking_questions or []:
            question_id = getattr(question, "id", "QUESTION")
            question_text = getattr(question, "question", str(question))
            gaps.append(Gap(
                id=f"GAP-PLAN-{question_id}-BLOCKING",
                severity="high",
                gap_type="requirement_not_planned",
                description=f"Blocking question remains unanswered: {question_text}",
                recommended_fix="Answer or convert the blocking question before approving the plan.",
                blocking=True,
            ))

        return GateResult(
            passed=len([g for g in gaps if g.blocking]) == 0, 
            gaps=gaps
        )

    def check_task_ready(self, task: Task, project_root: Path) -> GateResult:
        """Determines if a specific task can begin execution."""
        gaps = []
        
        # 1. Check Git state
        gaps.extend(self.clean_git.check(project_root, task.id))
        
        # 2. Check planned files
        gaps.extend(self.planned_files.check(task))
        
        # 3. Check for task dependencies (if implemented)
        
        return GateResult(
            passed=len([g for g in gaps if g.blocking]) == 0, 
            gaps=gaps
        )
