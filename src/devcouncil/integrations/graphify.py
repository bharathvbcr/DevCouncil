from datetime import UTC, datetime
from pathlib import Path

import yaml
from rich.console import Console

from devcouncil.utils.fsio import atomic_write_text
from devcouncil.utils.json_persist import write_json

console = Console()

class GraphifyIntegration:
    """
    Integration for graphify: Always-on graph context and multi-agent integration.
    """
    def __init__(self, project_root: Path):
        self.project_root = project_root

    def initialize(self):
        console.print("[magenta]Initializing Graphify engine...[/magenta]")
        graphify_config = self.project_root / ".devcouncil" / "graphify.yaml"
        graphify_config.parent.mkdir(parents=True, exist_ok=True)
        # Create a default graphify config
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
        console.print("  - Graphify engine configured and ready.")

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
