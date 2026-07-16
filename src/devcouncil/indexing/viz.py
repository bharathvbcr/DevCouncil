"""Interactive HTML visualizer for repository import graphs."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any


def graph_path(root: Path) -> Path:
    """Return the canonical code-graph JSON path under ``.devcouncil/graph/``."""
    return root / ".devcouncil" / "graph" / "code-graph.json"


def sample_demo_graph() -> dict[str, Any]:
    """Synthetic import graph for ``dev graph demo`` (no ``dev map`` required)."""
    return {
        "nodes": [
            {"id": "src/app/main.py", "label": "main.py", "group": "file"},
            {"id": "src/app/router.py", "label": "router.py", "group": "file"},
            {"id": "src/app/models.py", "label": "models.py", "group": "file"},
            {"id": "src/app/db.py", "label": "db.py", "group": "file"},
            {"id": "tests/test_app.py", "label": "test_app.py", "group": "test"},
        ],
        "links": [
            {"source": "src/app/main.py", "target": "src/app/router.py", "type": "imports"},
            {"source": "src/app/router.py", "target": "src/app/models.py", "type": "imports"},
            {"source": "src/app/models.py", "target": "src/app/db.py", "type": "imports"},
            {"source": "tests/test_app.py", "target": "src/app/main.py", "type": "imports"},
        ],
    }


def render_graph_html(graph: dict[str, Any], *, file_level: bool = False) -> str:
    """Render a self-contained HTML page with an interactive ForceGraph view."""
    _ = file_level  # reserved for symbol-level expansion
    graph_json = json.dumps(graph)
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>DevCouncil code graph</title>
  <style>
    html, body {{ margin: 0; height: 100%; background: #0b1020; color: #e8eefc; font-family: system-ui, sans-serif; }}
    #header {{ padding: 0.75rem 1rem; border-bottom: 1px solid #24304d; }}
    #graph {{ width: 100%; height: calc(100% - 3.25rem); }}
    .hint {{ opacity: 0.75; font-size: 0.9rem; }}
  </style>
</head>
<body>
  <div id="header">
    <strong>DevCouncil code graph</strong>
    <div class="hint">Drag nodes, scroll to zoom, hover for labels.</div>
  </div>
  <div id="graph"></div>
  <script src="https://unpkg.com/force-graph"></script>
  <script>
    const data = {graph_json};
    const palette = {{ file: "#5b8def", test: "#6bcf7f", default: "#c084fc" }};
    ForceGraph()(document.getElementById("graph"))
      .graphData(data)
      .nodeId("id")
      .nodeLabel(node => node.label || node.id)
      .nodeColor(node => palette[node.group] || palette.default)
      .linkDirectionalArrowLength(4)
      .linkDirectionalArrowRelPos(1)
      .linkLabel(link => link.type || "")
      .backgroundColor("#0b1020");
  </script>
</body>
</html>
"""


def write_graph_demo(
    root: Path,
    *,
    open_browser: bool = True,
) -> Path:
    """Write sample ``demo.html`` (real visualizer + synthetic graph) under ``.devcouncil/graph/``."""
    out_dir = graph_path(root).parent
    out_dir.mkdir(parents=True, exist_ok=True)
    html_path = out_dir / "demo.html"
    html_path.write_text(render_graph_html(sample_demo_graph(), file_level=True), encoding="utf-8")
    if open_browser:
        import webbrowser

        webbrowser.open(html_path.resolve().as_uri())
    return html_path
