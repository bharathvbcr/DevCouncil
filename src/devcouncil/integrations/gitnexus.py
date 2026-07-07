from __future__ import annotations

from datetime import UTC, datetime
from pathlib import Path
from typing import TYPE_CHECKING

from rich.console import Console

from devcouncil.utils.fsio import atomic_write_text
from devcouncil.utils.json_persist import write_json

if TYPE_CHECKING:
    from devcouncil.indexing.graph_index import GraphIndex

console = Console()
AGENT_GUIDE_MARKER = "<!-- Managed by dev map: keep this file in sync with .devcouncil/repo_map.json. -->"


def _agent_guide_text() -> str:
    return "\n".join(
        [
            AGENT_GUIDE_MARKER,
            "",
            "# Agent Workspace Guide",
            "",
            "Use `.devcouncil/repo_map.json` as the primary file index for this workspace.",
            "Repo map: `.devcouncil/repo_map.json`",
            "",
            "Workflow for agents:",
            "1. Open `.devcouncil/repo_map.json` before guessing at file locations.",
            "2. Use the `files` list to resolve module ownership and nearby siblings.",
            "3. Run `dev map` again after large refactors to refresh the map.",
            "",
            "Important surfaces:",
            "1. `src/devcouncil/cli/main.py` for CLI composition.",
            "2. `src/devcouncil/app/orchestrator.py` and `src/devcouncil/app/state_machine.py` for lifecycle control.",
            (
                "3. `src/devcouncil/artifacts/graph.py` and "
                "`src/devcouncil/storage/repositories.py` for persistence and evidence."
            ),
            "4. `src/devcouncil/execution/` and `src/devcouncil/executors/` for task execution.",
            "5. `src/devcouncil/verification/` and `src/devcouncil/gating/` for verification and policy gates.",
            "",
            "If the map and source disagree, trust the source and regenerate the map.",
        ]
    )

class GitNexusIntegration:
    """
    Integration for GitNexus: Codebase knowledge graph and structural awareness.
    """
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def initialize(self):
        console.print("[cyan]Initializing GitNexus context...[/cyan]")
        nexus_dir = self.project_root / ".devcouncil" / "nexus"
        nexus_dir.mkdir(exist_ok=True)
        # Mock initialization logic
        atomic_write_text(nexus_dir / "index_config.json", '{"mode": "structural", "version": "1.0"}')
        for filename in ("AGENTS.md", "CLAUDE.md"):
            path = self.project_root / filename
            if path.exists():
                existing = path.read_text(encoding="utf-8")
                if AGENT_GUIDE_MARKER not in existing:
                    continue
            atomic_write_text(path, _agent_guide_text() + "\n")
        console.print("  - GitNexus structural awareness active.")

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
