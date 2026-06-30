import json
from devcouncil.artifacts.graph import ArtifactGraph

class JsonReportGenerator:
    """Generates a JSON evidence report."""
    
    @staticmethod
    def generate(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        summary = graph.coverage_summary()
        live_blockers = len((live_review or {}).get("blocking_cards", []))
        
        # Three honest states (see markdown_report for rationale):
        #   blocked    - positive evidence of a problem.
        #   incomplete - nothing failing, but not every AC has passing evidence.
        #   passed     - no blocking gaps and every AC proven.
        if summary["blocking_gaps"] > 0 or live_blockers > 0:
            verdict = "blocked"
        elif summary["ac_without_evidence"] > 0:
            verdict = "incomplete"
        else:
            verdict = "passed"
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
        report = {
            "verdict": verdict,
            "coverage_summary": summary,
            "proof_modes": proof_modes,
            "blocking_gaps": [g.model_dump() for g in graph.blocking_gaps()]
        }
        if live_review is not None:
            report["live_review"] = live_review
        
        return json.dumps(report, indent=2)
