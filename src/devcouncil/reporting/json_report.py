import json
from devcouncil.artifacts.graph import ArtifactGraph

class JsonReportGenerator:
    """Generates a JSON evidence report."""
    
    @staticmethod
    def generate(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        summary = graph.coverage_summary()
        live_blockers = len((live_review or {}).get("blocking_cards", []))
        
        report = {
            "verdict": "blocked" if summary["blocking_gaps"] > 0 or live_blockers > 0 else "passed",
            "coverage_summary": summary,
            "blocking_gaps": [g.model_dump() for g in graph.blocking_gaps()]
        }
        if live_review is not None:
            report["live_review"] = live_review
        
        return json.dumps(report, indent=2)
