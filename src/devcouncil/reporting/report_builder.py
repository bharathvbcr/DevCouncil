from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.markdown_report import MarkdownReportGenerator
from devcouncil.reporting.json_report import JsonReportGenerator

class ReportBuilder:
    """Builds reports in various formats from the artifact graph."""
    
    @staticmethod
    def build_markdown(graph: ArtifactGraph) -> str:
        return MarkdownReportGenerator.generate(graph)

    @staticmethod
    def build_json(graph: ArtifactGraph) -> str:
        return JsonReportGenerator.generate(graph)
