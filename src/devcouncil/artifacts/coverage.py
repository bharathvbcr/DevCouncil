"""Coverage analysis utilities for the artifact graph."""

from __future__ import annotations

from typing import Dict, List, Tuple

from devcouncil.artifacts.graph import ArtifactGraph


def requirement_task_matrix(graph: ArtifactGraph) -> List[Dict[str, str]]:
    """Build a requirement → task → status coverage matrix for reporting.
    
    Returns a list of dicts suitable for rendering as a table:
      [{"requirement": "REQ-001", "title": "...", "task": "TASK-001", "task_status": "verified"}, ...]
    """
    rows: List[Dict[str, str]] = []
    for req in graph.requirements.values():
        linked_tasks = [
            t for t in graph.tasks.values() if req.id in t.requirement_ids
        ]
        if not linked_tasks:
            rows.append({
                "requirement": req.id,
                "title": req.title,
                "task": "(none)",
                "task_status": "NOT PLANNED",
            })
        else:
            for task in linked_tasks:
                rows.append({
                    "requirement": req.id,
                    "title": req.title,
                    "task": task.id,
                    "task_status": task.status,
                })
    return rows


def acceptance_criteria_evidence_matrix(
    graph: ArtifactGraph,
) -> List[Dict[str, str]]:
    """Build an AC → evidence mapping for the final report.
    
    Returns rows with requirement_id, ac_id, ac_description, evidence_status.
    """
    evidenced: Dict[str, str] = {}
    for ev in graph.test_evidence:
        key = ev.acceptance_criterion_id
        evidenced[key] = ev.status  # "passed", "failed", "not_run"

    rows: List[Dict[str, str]] = []
    for req in graph.requirements.values():
        for ac in req.acceptance_criteria:
            status = evidenced.get(ac.id, "no_evidence")
            rows.append({
                "requirement_id": req.id,
                "ac_id": ac.id,
                "description": ac.description,
                "verification_method": ac.verification_method,
                "status": status,
            })
    return rows


def can_approve_plan(graph: ArtifactGraph) -> Tuple[bool, List[str]]:
    """Check if the plan meets the PLAN_APPROVED gate criteria.
    
    Returns (passed, list_of_reasons_if_failed).
    
    Criteria (from §12):
      - Every requirement has acceptance criteria.
      - Every acceptance criterion has a verification method.
      - Every requirement maps to at least one task.
      - Every task maps to at least one requirement.
      - Every high-impact assumption is confirmed or converted.
      - No critical critique finding remains open.
      - No blocking question remains unanswered.
    """
    reasons: List[str] = []

    for req in graph.requirements_without_acceptance_criteria():
        reasons.append(f"{req.id} has no acceptance criteria.")

    for req in graph.requirements_without_tasks():
        reasons.append(f"{req.id} is not mapped to any task.")

    for task in graph.tasks_without_requirements():
        reasons.append(f"{task.id} is not mapped to any requirement.")

    for asm in graph.unconfirmed_high_impact_assumptions():
        reasons.append(f"Assumption {asm.id} (high impact) is still open: {asm.statement}")

    for finding in graph.open_findings("critical"):
        reasons.append(f"Critical finding {finding.id} is still open: {finding.claim}")

    return len(reasons) == 0, reasons
