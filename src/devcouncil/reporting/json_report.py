import json
from devcouncil.artifacts.graph import ArtifactGraph

class JsonReportGenerator:
    """Generates a JSON evidence report."""
    
    @staticmethod
    def generate(graph: ArtifactGraph) -> str:
        summary = graph.coverage_summary()
        
        report = {
            "verdict": "blocked" if summary["blocking_gaps"] > 0 else "passed",
            "coverage_summary": summary,
            "blocking_gaps": [g.model_dump() for g in graph.blocking_gaps()]
        }
        
        return json.dumps(report, indent=2)
