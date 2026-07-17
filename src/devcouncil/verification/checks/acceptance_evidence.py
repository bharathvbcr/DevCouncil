"""Acceptance-criteria evidence mapping extracted from Verifier.verify_task."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable, Dict, List, Literal, Optional, Set, Tuple

from devcouncil.domain.evidence import CommandResult, TestEvidence
from devcouncil.domain.gap import Gap
from devcouncil.domain.requirement import Requirement
from devcouncil.domain.task import Task
from devcouncil.verification.command_evidence import command_is_trivial_evidence
from devcouncil.verification.difficulty import RigorPolicy

_AD_HOC_TASK_ID = "CHECK"


@dataclass
class AcceptanceEvidenceResult:
    gaps: List[Gap] = field(default_factory=list)
    evidence: List[TestEvidence] = field(default_factory=list)


def requirement_id_for_ac(requirements: List[Requirement], ac_id: str) -> Optional[str]:
    for req in requirements:
        if any(ac.id == ac_id for ac in req.acceptance_criteria):
            return req.id
    return None


def _declared_test_proves_ac(
    task: Task,
    ac_id: str,
    successful_commands: List[CommandResult],
) -> bool:
    """True when a user/planner declared test command proves this criterion.

    For ad-hoc ``CHECK`` runs, ``--test`` commands are criterion-specific by
    construction (one inline AC). They must not fall through to the coarse
    "any passing command" bucket.
    """
    declared = list(task.expected_tests or [])
    if not declared:
        return False
    passing_declared = [r.command for r in successful_commands if r.command in declared]
    if not passing_declared:
        return False
    # Only ad-hoc CHECK treats inline --test commands as per-criterion proof.
    # Workflow tasks with a single expected_test still use the coarse bucket so
    # hard-rigor coarse_acceptance_proof blocking remains meaningful.
    return task.id == _AD_HOC_TASK_ID


def map_acceptance_criteria_evidence(
    *,
    task: Task,
    requirements: List[Requirement],
    evidence_results: List[CommandResult],
    compiled_pass: Dict[str, bool],
    compiled_cmds_by_ac: Dict[str, List[str]],
    failing_results_by_ac: Dict[str, List[CommandResult]],
    compiled_vote: Dict[str, Tuple[int, int, bool]],
    inconclusive_acs: Set[str],
    work_present: bool,
    genuine_failure: bool,
    had_unrunnable: bool,
    rigor: RigorPolicy,
    is_quality_only_command: Callable[[str], bool],
    next_gap_id: Callable[[str, str], str],
) -> AcceptanceEvidenceResult:
    """Map compiled + coarse verification evidence to gaps and TestEvidence records."""
    result = AcceptanceEvidenceResult()
    agent_appended = set(task.agent_appended_expected_tests or []) | set(
        task.agent_appended_allowed_commands or []
    )
    successful_commands = [
        item for item in evidence_results
        if item.exit_code == 0
        and not is_quality_only_command(item.command)
        and item.command not in agent_appended
        and not command_is_trivial_evidence(item.command)
    ]
    coarse_proof_available = work_present and bool(successful_commands)

    if task.acceptance_criterion_ids:
        req_by_ac = {ac.id: req.id for req in requirements for ac in req.acceptance_criteria}
        unproven_acs: List[str] = []
        coarse_proven_acs: List[str] = []
        for ac_id in task.acceptance_criterion_ids:
            proven: Optional[bool] = compiled_pass.get(ac_id)
            coarse = False
            if proven is None:
                if _declared_test_proves_ac(task, ac_id, successful_commands):
                    proven = True
                else:
                    proven = coarse_proof_available
                    coarse = proven
            if proven:
                if coarse:
                    coarse_proven_acs.append(ac_id)
                if not (coarse and genuine_failure):
                    proof_mode: Literal["compiled", "vote", "coarse", ""]
                    if coarse:
                        proof_summary = (
                            "Acceptance criterion proven only by a COARSE signal (a passing "
                            "acceptance-capable command, not a per-criterion check); behavior "
                            "not precisely verified."
                        )
                        proof_mode = "coarse"
                    elif task.id == _AD_HOC_TASK_ID and task.expected_tests:
                        proof_mode = "compiled"
                        proof_summary = (
                            "Acceptance criterion proven by user-supplied verification "
                            "command(s) for this check."
                        )
                    else:
                        passes_n, decisive_n, was_repaired = compiled_vote.get(ac_id, (1, 1, False))
                        proof_mode = "vote" if decisive_n > 1 else "compiled"
                        how = (
                            f"a majority vote of independent compiled checks ({passes_n}/{decisive_n} passed)"
                            if decisive_n > 1 else
                            "a per-criterion compiled check"
                        )
                        repaired_note = (
                            " (one check was regenerated from its launcher error to run)"
                            if was_repaired else ""
                        )
                        proof_summary = f"Acceptance criterion proven by {how}.{repaired_note}"
                    result.evidence.append(TestEvidence(
                        requirement_id=req_by_ac.get(
                            ac_id, task.requirement_ids[0] if task.requirement_ids else ""
                        ),
                        acceptance_criterion_id=ac_id,
                        command="(devcouncil acceptance check)",
                        status="passed",
                        evidence_summary=proof_summary,
                        mode=proof_mode,
                    ))
            else:
                unproven_acs.append(ac_id)

        if coarse_proven_acs and rigor.coarse_acceptance_enabled:
            result.gaps.append(Gap(
                id=next_gap_id(task.id, "COARSE"),
                severity="high" if rigor.coarse_acceptance_blocking else "low",
                gap_type="coarse_acceptance_proof",
                task_id=task.id,
                description=(
                    "Verification mode = COARSE for "
                    f"{', '.join(coarse_proven_acs)}: proven by a passing acceptance-capable "
                    "command, not a per-criterion check. Behavior is not precisely verified."
                ),
                evidence=[f"coarse-proven: {', '.join(coarse_proven_acs)}"],
                recommended_fix=(
                    "Add a verification command (or test) that exercises each listed criterion "
                    "specifically, so DevCouncil can compile a per-criterion check instead of "
                    "relying on the coarse fallback."
                ),
                blocking=rigor.coarse_acceptance_blocking,
            ))

        if unproven_acs:
            couldnt_verify = had_unrunnable and not genuine_failure and work_present
            ac_by_id = {ac.id: ac for req in requirements for ac in req.acceptance_criteria}
            automatable_methods = {"unit_test", "integration_test"}
            for ac_id in unproven_acs:
                ac = ac_by_id.get(ac_id)
                method = ac.verification_method if ac else "unit_test"
                is_automatable = (ac.required if ac else True) and method in automatable_methods
                if not is_automatable:
                    blocks = False
                    optional = "" if (ac is None or ac.required) else " optional"
                    fix = (
                        f"This{optional} criterion's verification method is '{method}'; it cannot be "
                        "proven by running code. Review it manually (it does not block the gate)."
                    )
                    suffix = f" (non-blocking: {method})"
                elif ac_id in inconclusive_acs:
                    blocks = False
                    fix = (
                        "Auto-generated acceptance checks disagreed on this criterion (some "
                        "passed, some failed). Add a precise verification command that "
                        "unambiguously proves it so the result is decisive."
                    )
                    suffix = " (auto-checks inconclusive)"
                elif couldnt_verify:
                    blocks = False
                    fix = (
                        "Could not verify this criterion: the verification commands did not run "
                        "(missing tooling or tests). Regenerate them with 'dev repair' to confirm the work."
                    )
                    suffix = " (verification commands could not run)"
                else:
                    blocks = True
                    fix = "Add or fix a verification command that proves this acceptance criterion."
                    suffix = ""
                ac_compiled = compiled_cmds_by_ac.get(ac_id, [])
                ac_failures = failing_results_by_ac.get(ac_id, [])
                ac_evidence: List[str] = []
                suggested_cmd: Optional[str] = None
                if ac_compiled:
                    suggested_cmd = ac_compiled[0]
                    ac_evidence.extend(f"compiled check: {c}" for c in ac_compiled)
                    ac_evidence.extend(r.summary[:500] for r in ac_failures)
                else:
                    ac_evidence.append(
                        f"no DevCouncil check compiled for {ac_id} "
                        f"(expected verification method: {method})"
                    )
                result.gaps.append(Gap(
                    id=next_gap_id(task.id, f"AC-{ac_id}"),
                    severity="high" if blocks else "medium",
                    gap_type="acceptance_criteria_unproven",
                    requirement_id=requirement_id_for_ac(requirements, ac_id),
                    task_id=task.id,
                    description=(
                        f"Acceptance criterion {ac_id} has no passing verification evidence "
                        f"for task {task.id}.{suffix}"
                    ),
                    evidence=ac_evidence,
                    recommended_fix=fix,
                    blocking=blocks,
                    acceptance_criterion_id=ac_id,
                    expected_verification_method=method,
                    suggested_command=suggested_cmd,
                ))
    elif task.requirement_ids:
        result.gaps.append(Gap(
            id=next_gap_id(task.id, "NOAC"),
            severity="high",
            gap_type="acceptance_criteria_unproven",
            requirement_id=task.requirement_ids[0],
            task_id=task.id,
            description=f"Task {task.id} is linked to requirements but no acceptance criteria.",
            recommended_fix="Link the task to specific acceptance_criterion_ids before verification.",
            blocking=True,
        ))

    return result
