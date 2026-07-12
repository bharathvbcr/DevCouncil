from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING

from rich.console import Console

from devcouncil.utils.json_persist import write_json

if TYPE_CHECKING:
    from devcouncil.indexing.graph_index import GraphIndex

console = Console()


class GitNexusIntegration:
    """
    Thin companion for GitNexus-style structural awareness.

    Agent guides (AGENTS.md / CLAUDE.md) are owned exclusively by ``dev map`` —
    this integration must never write them. ``initialize`` records a small
    nexus config and ensures the real code-graph / repo-map pipeline has run.
    """

    def __init__(self, project_root: Path):
        self.project_root = project_root

    def initialize(self):
        console.print("[cyan]Initializing GitNexus context...[/cyan]")
        nexus_dir = self.project_root / ".devcouncil" / "nexus"
        nexus_dir.mkdir(parents=True, exist_ok=True)
        write_json(nexus_dir / "index_config.json", {"mode": "structural", "version": "1.0"})
        # Agent guides are owned by ``dev map`` only — never write AGENTS.md/CLAUDE.md here.
        map_path = self.project_root / ".devcouncil" / "repo_map.json"
        if map_path.is_file():
            console.print("  - GitNexus structural awareness active (using existing repo map).")
        else:
            console.print(
                "  - GitNexus config ready. Run [bold]dev map[/bold] to build the code graph "
                "and agent guides (map.py owns AGENTS.md/CLAUDE.md)."
            )

    def sync_graph(self, graph_index: GraphIndex):
        """Export DevCouncil artifact graph nodes/edges to GitNexus storage."""
        console.print("  - Syncing DevCouncil artifacts to GitNexus...")
        nexus_dir = self.project_root / ".devcouncil" / "nexus"
        nexus_dir.mkdir(parents=True, exist_ok=True)

        nodes = [
            {"id": node.id, "type": node.type, "metadata": node.metadata}
            for node in graph_index.graph.nodes
        ]
        edges = [
            {"source": edge.source, "target": edge.target, "relation": edge.relation}
            for edge in graph_index.graph.edges
        ]
        payload = {
            "exported_at": datetime.now(UTC).isoformat(),
            "node_count": len(nodes),
            "edge_count": len(edges),
            "nodes": nodes,
            "edges": edges,
        }
        out_path = nexus_dir / "artifact_graph.json"
        write_json(out_path, payload)
        console.print(f"  - Wrote {len(nodes)} node(s) and {len(edges)} edge(s) to {out_path.name}.")
