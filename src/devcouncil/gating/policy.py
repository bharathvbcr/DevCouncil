import logging
from collections import deque
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

logger = logging.getLogger(__name__)


def _log_gate(name: str, gaps: List[Gap], *, routine: bool = False, **context: Any) -> bool:
    """Log a gate decision and return whether it passed (no blocking gaps).

    A passing gate is logged at INFO for once-per-plan checks (a real milestone) but at
    DEBUG for ``routine`` per-task checks (e.g. task_ready, which fires for every task and
    every repair attempt) so the ``-v`` stream stays milestone-level. A FAILED gate is
    always WARNING — that's the signal you actually chase.
    """
    blocking = [g for g in gaps if g.blocking]
    passed = not blocking
    suffix = "".join(f" {k}={v}" for k, v in context.items())
    if passed:
        log = logger.debug if routine else logger.info
        log("Gate %s PASSED (%d advisory gap(s))%s", name, len(gaps), suffix)
    else:
        logger.warning(
            "Gate %s FAILED%s: %s",
            name, suffix,
            "; ".join(f"{g.gap_type}: {g.description}" for g in blocking),
        )
    return passed


class GateResult(BaseModel):
    passed: bool
    gaps: List[Gap]


def _find_dependency_cycle(tasks: List[Task]) -> Optional[List[str]]:
    """Return one dependency cycle as an id path (e.g. [A, B, A]), or None. Only edges
    to known task ids are followed; unknown deps are reported separately."""
    ids = {t.id for t in tasks}
    graph = {t.id: [d for d in t.depends_on if d in ids] for t in tasks}
    WHITE, GREY, BLACK = 0, 1, 2
    color = {tid: WHITE for tid in graph}
    stack: List[str] = []

    def visit(node: str) -> Optional[List[str]]:
        color[node] = GREY
        stack.append(node)
        for nxt in graph.get(node, []):
            if color[nxt] == GREY:
                return stack[stack.index(nxt):] + [nxt]
            if color[nxt] == WHITE:
                found = visit(nxt)
                if found:
                    return found
        stack.pop()
        color[node] = BLACK
        return None

    for tid in graph:
        if color[tid] == WHITE:
            found = visit(tid)
            if found:
                return found
    return None


def topological_order(tasks: List[Task]) -> List[Task]:
    """Order tasks so every task follows the ones it depends on. Stable: preserves the
    given order among independent tasks. Falls back to the original order if a cycle
    makes a full ordering impossible (the plan gate blocks cycles separately)."""
    by_id = {t.id: t for t in tasks}
    indegree = {t.id: 0 for t in tasks}
    dependents: dict[str, List[str]] = {t.id: [] for t in tasks}
    for task in tasks:
        for dep in task.depends_on:
            if dep in by_id:
                indegree[task.id] += 1
                dependents[dep].append(task.id)
    # Kahn's algorithm, seeded in original order for stability.
    ready = deque(t.id for t in tasks if indegree[t.id] == 0)
    ordered: List[str] = []
    while ready:
        current = ready.popleft()
        ordered.append(current)
        for child in dependents[current]:
            indegree[child] -= 1
            if indegree[child] == 0:
                ready.append(child)
    if len(ordered) != len(tasks):  # cycle — fall back to original order
        return list(tasks)
    return [by_id[tid] for tid in ordered]

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

        # Surface read-only-only tasks at PLANNING time (advisory): a task that declares
        # planned files but none writable can implement nothing, and was previously only
        # caught at execution by the task-readiness gate.
        for task in tasks:
            if task.planned_files and not any(
                pf.allowed_change in ("create", "modify", "delete") for pf in task.planned_files
            ):
                gaps.append(Gap(
                    id=f"GAP-PLAN-{task.id}-READ-ONLY",
                    severity="medium",
                    gap_type="task_not_implemented",
                    task_id=task.id,
                    description=(
                        f"Task {task.id} declares planned files but none are writable "
                        "(all read_only); it cannot implement any change. Expected only if "
                        "this is an analysis-only task."
                    ),
                    recommended_fix=(
                        "Grant 'create', 'modify', or 'delete' to at least one planned file, "
                        "or confirm the task is intentionally analysis-only."
                    ),
                    blocking=False,
                ))

        # Surface overlapping ownership (advisory): when 2+ tasks each declare a
        # writable (create/modify/delete) change to the SAME file, the plan is
        # over-decomposed — the later task tends to duplicate or conflict with the
        # earlier one (e.g. both add the same function), which then fails per-task
        # verification. Consolidating a file's work into one task avoids this.
        writers_by_file: dict[str, list[str]] = {}
        writer_sets: dict[str, set[str]] = {}
        for task in tasks:
            for pf in task.planned_files:
                if pf.allowed_change in ("create", "modify", "delete"):
                    path = pf.path.replace("\\", "/")
                    owners = writers_by_file.setdefault(path, [])
                    seen = writer_sets.setdefault(path, set())
                    if task.id not in seen:
                        seen.add(task.id)
                        owners.append(task.id)
        for path, owners in writers_by_file.items():
            if len(owners) > 1:
                gaps.append(Gap(
                    id=f"GAP-PLAN-OVERLAP-{owners[0]}-{path.replace('/', '_')}",
                    severity="medium",
                    gap_type="task_not_implemented",
                    description=(
                        f"{len(owners)} tasks ({', '.join(owners)}) each declare writable "
                        f"changes to {path}; overlapping ownership over-decomposes the plan "
                        "and tends to cause duplicate/conflicting edits at execution."
                    ),
                    recommended_fix=(
                        f"Consolidate the work on {path} into a single task, or scope the "
                        "others to read_only."
                    ),
                    blocking=False,
                ))

        # Validate the task dependency DAG: unknown depends_on ids and cycles would make
        # execution ordering impossible / stall the run, so block the plan on them.
        gaps.extend(self._validate_task_dependencies(tasks))

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
            passed=_log_gate("plan_approval", gaps),
            gaps=gaps
        )

    def check_task_ready(self, task: Task, project_root: Path) -> GateResult:
        """Determines if a specific task can begin execution."""
        gaps = []
        
        # 1. Check Git state
        gaps.extend(self.clean_git.check(project_root, task.id))
        
        # 2. Check planned files
        gaps.extend(self.planned_files.check(task))

        # 3. Surface a missing execution/verification contract — but do NOT block
        # execution on it. The executor still needs to run to implement the code,
        # and the evidence requirement is genuinely enforced at verify time
        # (acceptance_criteria_unproven / NOAC gaps). Blocking here only prevents
        # implementation and stalls multi-task plans when the planner under-specs a
        # task; these stay advisory so the work can proceed and be judged on output.
        if not task.allowed_commands and not task.expected_tests:
            gaps.append(Gap(
                id=f"GAP-{task.id}-NO-COMMANDS",
                severity="medium",
                gap_type="missing_test",
                task_id=task.id,
                description=f"Task {task.id} has no allowed commands for execution or verification.",
                recommended_fix="Add explicit allowed_commands or expected_tests so verification can prove the acceptance criteria.",
                blocking=False,
            ))

        if not task.expected_tests:
            gaps.append(Gap(
                id=f"GAP-{task.id}-NO-EXPECTED-EVIDENCE",
                severity="medium",
                gap_type="missing_test",
                task_id=task.id,
                description=f"Task {task.id} has no expected verification evidence.",
                recommended_fix="Add expected_tests or targeted static/manual review commands that prove the acceptance criteria.",
                blocking=False,
            ))

        return GateResult(
            passed=_log_gate("task_ready", gaps, routine=True, task_id=task.id),
            gaps=gaps
        )

    def _validate_task_dependencies(self, tasks: List[Task]) -> List[Gap]:
        """Block on a malformed dependency DAG: unknown depends_on ids and cycles."""
        gaps: List[Gap] = []
        ids = {t.id for t in tasks}
        for task in tasks:
            unknown = [dep for dep in task.depends_on if dep not in ids]
            if unknown:
                gaps.append(Gap(
                    id=f"GAP-PLAN-{task.id}-UNKNOWN-DEP",
                    severity="high",
                    gap_type="task_not_implemented",
                    task_id=task.id,
                    description=f"Task {task.id} depends on unknown task(s): {', '.join(unknown)}.",
                    recommended_fix="Reference only task IDs that exist in this plan, or remove the dependency.",
                    blocking=True,
                ))
        cycle = _find_dependency_cycle(tasks)
        if cycle:
            gaps.append(Gap(
                id="GAP-PLAN-DEP-CYCLE",
                severity="high",
                gap_type="task_not_implemented",
                description=f"Task dependency cycle detected: {' -> '.join(cycle)}.",
                recommended_fix="Break the cycle so the tasks can be ordered and executed.",
                blocking=True,
            ))
        return gaps
