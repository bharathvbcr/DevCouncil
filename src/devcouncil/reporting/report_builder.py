from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.markdown_report import MarkdownReportGenerator
from devcouncil.reporting.json_report import JsonReportGenerator

class ReportBuilder:
    """Builds reports in various formats from the artifact graph."""
    
    @staticmethod
    def build_markdown(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        return MarkdownReportGenerator.generate(graph, live_review=live_review)

    @staticmethod
    def build_json(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        return JsonReportGenerator.generate(graph, live_review=live_review)
