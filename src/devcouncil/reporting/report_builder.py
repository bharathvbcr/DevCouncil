from pathlib import Path

from devcouncil.artifacts.graph import ArtifactGraph
from devcouncil.reporting.markdown_report import MarkdownReportGenerator
from devcouncil.reporting.json_report import JsonReportGenerator
from devcouncil.reporting.evidence_export import EvidenceExportGenerator

class ReportBuilder:
    """Builds reports in various formats from the artifact graph."""

    @staticmethod
    def build_markdown(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        return MarkdownReportGenerator.generate(graph, live_review=live_review)

    @staticmethod
    def build_json(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        return JsonReportGenerator.generate(graph, live_review=live_review)

    @staticmethod
    def build_evidence_export(graph: ArtifactGraph, live_review: dict | None = None) -> str:
        """Reviewer-readable requirement→task→diff→evidence JSON (see evidence_export)."""
        return EvidenceExportGenerator.generate(graph, live_review=live_review)

    @staticmethod
    def build_okf_bundle(
        graph: ArtifactGraph,
        output_dir: Path,
        repo_map=None,
        project_name: str = "DevCouncil Project",
        timestamp: str = "",
    ) -> list[Path]:
        """Export the artifact graph as an Open Knowledge Format bundle on disk."""
        from devcouncil.reporting.okf_bundle_writer import OKFBundleWriter

        return OKFBundleWriter.generate(
            graph, output_dir, repo_map=repo_map, project_name=project_name, timestamp=timestamp
        )
