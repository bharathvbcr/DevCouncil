"""Reviewer-facing export of the requirement→task→diff→evidence graph.

This is a pure serializer over an already-loaded :class:`ArtifactGraph` (no new
analysis): it flattens what the standard report loads into a single JSON document a
PR reviewer can read — or attach as a CI artifact — without installing DevCouncil.
Written by ``dev report --evidence-json PATH``.
"""

import json
from datetime import datetime, timezone
from typing import Any, Dict, List

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.verdict import classify_verdict

_SEVERITY_RANK = {"low": 0, "medium": 1, "high": 2, "critical": 3}


class EvidenceExportGenerator:
    """Serializes the artifact graph as a single reviewer-readable JSON file."""

    FORMAT = "devcouncil-evidence-export"
    VERSION = 1

    @staticmethod
    def generate(graph: ArtifactGraph, live_review: dict | None = None, wiki_refresh: dict | None = None) -> str:
        summary = graph.coverage_summary()
        live_blockers = len((live_review or {}).get("blocking_cards", []))

        verdict, incomplete_kind = classify_verdict(graph, live_blockers=live_blockers)

        # Index evidence and diffs once so requirements/tasks can nest their own slices.
        evidence_by_ac: Dict[str, List[Dict[str, Any]]] = {}
        for ev in graph.test_evidence:
            evidence_by_ac.setdefault(ev.acceptance_criterion_id, []).append(
                {
                    "requirement_id": ev.requirement_id,
                    "command": ev.command,
                    "status": ev.status,  # passed | failed | not_run
                    "mode": getattr(ev, "mode", "") or "unspecified",
                    "summary": ev.evidence_summary,
                }
            )

        diffs_by_task: Dict[str, List[Dict[str, Any]]] = {}
        for de in graph.diff_evidence:
            diffs_by_task.setdefault(de.task_id, []).append(
                {
                    "changed_files": de.changed_files,
                    "added_files": de.added_files,
                    "deleted_files": de.deleted_files,
                    "diff_summary": de.diff_summary,
                }
            )

        tasks_by_requirement: Dict[str, List[str]] = {}
        for task in graph.tasks.values():
            for req_id in task.requirement_ids:
                tasks_by_requirement.setdefault(req_id, []).append(task.id)

        unproven_ac_ids = {ac.id for _, ac in graph.acceptance_criteria_without_evidence()}

        requirements = [
            {
                "id": req.id,
                "title": req.title,
                "description": req.description,
                "priority": req.priority,
                "task_ids": sorted(tasks_by_requirement.get(req.id, [])),
                "acceptance_criteria": [
                    {
                        "id": ac.id,
                        "description": ac.description,
                        "verification_method": ac.verification_method,
                        "required": ac.required,
                        "proven": ac.id not in unproven_ac_ids,
                        "evidence": evidence_by_ac.get(ac.id, []),
                    }
                    for ac in req.acceptance_criteria
                ],
            }
            for req in sorted(graph.requirements.values(), key=lambda r: r.id)
        ]

        tasks = [
            {
                "id": task.id,
                "title": task.title,
                "status": task.status,
                "requirement_ids": task.requirement_ids,
                "acceptance_criterion_ids": task.acceptance_criterion_ids,
                "diffs": diffs_by_task.get(task.id, []),
            }
            for task in sorted(graph.tasks.values(), key=lambda t: t.id)
        ]

        # All gaps (each carries its own ``blocking`` flag), blocking-first then by
        # severity so a reviewer reads the show-stoppers before the advisories.
        gaps = [
            gap.model_dump()
            for gap in sorted(
                graph.gaps.values(),
                key=lambda g: (not g.blocking, -_SEVERITY_RANK.get(g.severity, 0), g.id),
            )
        ]

        report: Dict[str, Any] = {
            "format": EvidenceExportGenerator.FORMAT,
            "version": EvidenceExportGenerator.VERSION,
            "generated_at": datetime.now(timezone.utc).isoformat(),
            "verdict": verdict,
            "coverage_summary": summary,
            "requirements": requirements,
            "tasks": tasks,
            "gaps": gaps,
        }
        if incomplete_kind is not None:
            report["incomplete_kind"] = incomplete_kind
        if live_review is not None:
            report["live_review"] = live_review
        if wiki_refresh is not None:
            report["wiki_refresh"] = wiki_refresh

        return json.dumps(report, indent=2)
