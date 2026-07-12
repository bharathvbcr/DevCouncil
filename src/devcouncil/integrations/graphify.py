from datetime import UTC, datetime
from pathlib import Path

import yaml
from rich.console import Console

from devcouncil.utils.fsio import atomic_write_text
from devcouncil.utils.json_persist import write_json

console = Console()


class GraphifyIntegration:
    """
    Thin companion for graphify-style always-on graph context.

    Does not write AGENTS.md / CLAUDE.md — those are owned by ``dev map``.
    ``initialize`` writes a small config and ensures the real map/graph pipeline runs.
    """

    def __init__(self, project_root: Path):
        self.project_root = project_root

    def initialize(self):
        console.print("[magenta]Initializing Graphify engine...[/magenta]")
        graphify_config = self.project_root / ".devcouncil" / "graphify.yaml"
        graphify_config.parent.mkdir(parents=True, exist_ok=True)
        content = """
graph:
  engine: internal
  persist: true
agents:
  shared_context: true
hooks:
  enabled: true
"""
        atomic_write_text(graphify_config, content.strip())
        # Agent guides are owned by ``dev map`` only — never write AGENTS.md/CLAUDE.md here.
        map_path = self.project_root / ".devcouncil" / "repo_map.json"
        if map_path.is_file():
            console.print("  - Graphify engine configured (using existing repo map).")
        else:
            console.print(
                "  - Graphify config ready. Run [bold]dev map[/bold] to build the code graph "
                "and agent guides (map.py owns AGENTS.md/CLAUDE.md)."
            )

    def apply_rules(self):
        """Apply graph-based rules from ``.devcouncil/graphify.yaml``."""
        console.print("  - Graphify checking architectural rules...")
        graphify_config = self.project_root / ".devcouncil" / "graphify.yaml"
        if not graphify_config.exists():
            raise FileNotFoundError(
                f"Graphify config not found at {graphify_config}. Run initialize() first."
            )
        raw = yaml.safe_load(graphify_config.read_text(encoding="utf-8")) or {}
        if not isinstance(raw, dict):
            raise ValueError(f"Invalid graphify config at {graphify_config}: expected a mapping.")

        out_dir = self.project_root / ".devcouncil" / "graphify"
        out_dir.mkdir(parents=True, exist_ok=True)
        payload = {
            "status": "applied",
            "applied_at": datetime.now(UTC).isoformat(),
            "config_path": str(graphify_config.relative_to(self.project_root)),
            "engine": (raw.get("graph") or {}).get("engine", "internal"),
            "rules_count": len(raw.get("rules") or []),
        }
        out_path = out_dir / "rules_applied.json"
        write_json(out_path, payload)
        console.print(f"  - Graphify rules recorded at {out_path.name}.")
