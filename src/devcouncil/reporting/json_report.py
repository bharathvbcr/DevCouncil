import json
from typing import Any

from devcouncil.artifacts.coverage import (
    acceptance_criteria_evidence_matrix,
    requirement_task_matrix,
)
from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.verdict import classify_verdict

class JsonReportGenerator:
    """Generates a JSON evidence report."""
    
    @staticmethod
    def generate(graph: ArtifactGraph, live_review: dict | None = None, wiki_refresh: dict | None = None) -> str:
        summary = graph.coverage_summary()
        live_blockers = len((live_review or {}).get("blocking_cards", []))
        
        verdict, incomplete_kind = classify_verdict(graph, live_blockers=live_blockers)
        # Proof-rigor breakdown: of the criteria that ARE proven, how were they proven?
        # ``compiled``/``vote`` are precise per-criterion checks (trustworthy); ``coarse``
        # means proven only by a passing acceptance-capable command (weak). Surfacing this
        # lets an auditor see that a "passed" verdict rests on rigorous, not coarse, evidence
        # — the difference that matters most when a weak/local reviewer compiled the checks.
        proof_modes: dict[str, int] = {}
        for ev in getattr(graph, "test_evidence", []):
            if getattr(ev, "status", "") == "passed":
                key = getattr(ev, "mode", "") or "unspecified"
                proof_modes[key] = proof_modes.get(key, 0) + 1
        report: dict[str, Any] = {
            "verdict": verdict,
            "coverage_summary": summary,
            "proof_modes": proof_modes,
            "requirement_task_matrix": requirement_task_matrix(graph),
            "acceptance_criteria_evidence_matrix": acceptance_criteria_evidence_matrix(graph),
            "blocking_gaps": [g.model_dump() for g in graph.blocking_gaps()]
        }
        if incomplete_kind is not None:
            report["incomplete_kind"] = incomplete_kind
        if live_review is not None:
            report["live_review"] = live_review
        if wiki_refresh is not None:
            report["wiki_refresh"] = wiki_refresh
        
        return json.dumps(report, indent=2)
