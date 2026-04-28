from devcouncil.artifacts.graph import ArtifactGraph

class GitHubCheckGenerator:
    """Generates GitHub Checks API payloads."""
    
    @staticmethod
    def generate(graph: ArtifactGraph) -> dict:
        summary = graph.coverage_summary()
        blocking_gaps = graph.blocking_gaps()
        
        status = "completed"
        conclusion = "failure" if summary["blocking_gaps"] > 0 else "success"
        
        text = f"**Requirements**: {summary['total_requirements']} | "
        text += f"**Tasks**: {summary['total_tasks']} | "
        text += f"**Gaps**: {summary['blocking_gaps']} blocking\n\n"
        
        if blocking_gaps:
            text += "### Blocking Gaps\n"
            for gap in blocking_gaps:
                text += f"- **{gap.id}**: {gap.description}\n"
                
        return {
            "name": "DevCouncil Verification",
            "status": status,
            "conclusion": conclusion,
            "output": {
                "title": f"DevCouncil: {conclusion.capitalize()}",
                "summary": f"Found {summary['blocking_gaps']} blocking gaps.",
                "text": text
            }
        }
