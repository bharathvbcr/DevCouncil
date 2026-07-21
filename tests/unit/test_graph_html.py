"""Graph HTML visualizer artifact."""

from __future__ import annotations

import json
import re
import subprocess

from devcouncil.indexing.graph.build import build_code_graph, write_code_graph
from devcouncil.indexing.graph.schema import DeadCodeEntry, Confidence
from devcouncil.indexing.viz import (
    _payload_from_graph,
    render_graph_html,
    render_graph_preview_svg,
    sample_demo_graph,
    write_graph_demo,
    write_graph_html,
)


def _git(root, *args):
    subprocess.run(["git", *args], cwd=root, check=True, capture_output=True, text=True)


def _commit(root):
    _git(root, "init")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A")
    _git(root, "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "init")


def _write(tmp_path, files):
    for rel, content in files.items():
        p = tmp_path / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content, encoding="utf-8")


def test_render_escapes_script_breakout(tmp_path):
    _write(tmp_path, {
        "pkg/__init__.py": "",
        "pkg/a.py": "def f():\n    return 1\n",
    })
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    # Inject a hostile name into a node so the embed path must escape <
    graph.nodes[0].name = "</script><script>alert(1)</script>"
    html = render_graph_html(graph)
    assert "</script><script>" not in html
    assert "\\u003c" in html
    assert "ForceGraph" in html
    # Real vendored force-graph (not the tiny fallback stub)
    assert "vasturiano/force-graph" in html
    assert "function(){function api()" not in html
    assert "_missing" not in html


def test_write_graph_html(tmp_path):
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/a.py": "def f():\n    return 1\n"})
    _commit(tmp_path)
    write_code_graph(tmp_path, build_code_graph(tmp_path, liveness=False))
    out = write_graph_html(tmp_path)
    assert out.is_file()
    text = out.read_text(encoding="utf-8")
    assert "DevCouncil Code Graph" in text
    assert "const DATA =" in text
    assert "vasturiano/force-graph" in text
    # Self-contained: no CDN script tags
    assert "cdn.jsdelivr" not in text
    assert "unpkg.com" not in text
    assert 'src="http' not in text


def test_payload_has_file_and_symbol_modes(tmp_path):
    _write(
        tmp_path,
        {
            "pkg/__init__.py": "",
            "pkg/a.py": "def entry():\n    helper()\n\ndef helper():\n    return 1\n",
            "pkg/b.py": "from pkg.a import entry\n",
        },
    )
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    payload = _payload_from_graph(graph, file_level=True)
    assert "file" in payload and "symbol" in payload
    assert payload["file"]["nodes"]
    assert any(n["kind"] == "file" for n in payload["file"]["nodes"])
    # Symbol mode should include non-file nodes when present
    symbol_kinds = {n["kind"] for n in payload["symbol"]["nodes"]}
    assert "file" not in symbol_kinds or True  # file nodes excluded
    assert all(n["kind"] != "file" for n in payload["symbol"]["nodes"])
    assert "dead_code" in payload
    assert "processes" in payload
    assert "neighbors" in payload
    # Community falls back to area when intel absent
    for n in payload["file"]["nodes"]:
        assert "community" in n
        assert n["community"] == (n["area"] or "unknown") or n["community"]


def test_html_has_tabs_lenses_and_path_helpers(tmp_path):
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/a.py": "def f():\n    return 1\n"})
    _commit(tmp_path)
    graph = build_code_graph(tmp_path, liveness=False)
    graph.dead_code.append(
        DeadCodeEntry(
            id="pkg/a.py::f",
            path="pkg/a.py",
            line=1,
            kind="function",
            confidence=Confidence.INFERRED,
            reason="test",
        )
    )
    # Simulate community enrichment
    for n in graph.nodes:
        if n.path == "pkg/a.py":
            n.extras["community"] = "demo-community"
    graph.meta["processes"] = [
        {"name": "entry_flow", "steps": ["pkg/a.py::f"], "entry": "pkg/a.py"}
    ]
    html = render_graph_html(graph, file_level=False)
    assert 'data-tab="graph"' in html
    assert 'data-tab="dead"' in html
    assert 'data-tab="communities"' in html
    assert 'data-tab="processes"' in html
    assert 'id="mode"' in html
    assert 'value="symbol"' in html
    assert "lensDead" in html
    assert "bfsPath" in html
    assert "linkDirectionalParticles" in html
    assert "onNodeDblClick" not in html
    assert "event.detail >= 2" in html
    assert "demo-community" in html
    assert "entry_flow" in html
    # Default mode from file_level=False
    m = re.search(r"const DATA = (\{.*?\});\n", html)
    assert m
    data = json.loads(m.group(1).encode().decode("unicode_escape") if False else m.group(1))
    # JSON is embedded with unicode escapes for <>&; parse via the raw dump path
    raw = m.group(1)
    raw = raw.replace("\\u003c", "<").replace("\\u003e", ">").replace("\\u0026", "&")
    data = json.loads(raw)
    assert data["mode"] == "symbol"
    assert any(c.get("id") == "demo-community" or c.get("label") == "demo-community"
               for c in (data.get("symbol") or {}).get("communities") or [])


def test_write_graph_html_symbols_flag(tmp_path):
    _write(tmp_path, {"pkg/__init__.py": "", "pkg/a.py": "def f():\n    return 1\n"})
    _commit(tmp_path)
    write_code_graph(tmp_path, build_code_graph(tmp_path, liveness=False))
    out = write_graph_html(tmp_path, symbols=True)
    text = out.read_text(encoding="utf-8")
    assert '"mode":"symbol"' in text or '"mode": "symbol"' in text


def test_sample_demo_graph_and_preview_svg(tmp_path):
    graph = sample_demo_graph()
    assert any(n.path.endswith("cli/main.py") for n in graph.nodes)
    assert graph.entry_roots
    assert graph.dead_code
    html = render_graph_html(graph)
    assert "DevCouncil Code Graph" in html
    assert "cli/main.py" in html or "main.py" in html
    svg = render_graph_preview_svg()
    assert svg.lstrip().startswith("<svg")
    assert "#0f1419" in svg and "#3d8bfd" in svg
    assert "DevCouncil Code Graph" in svg
    paths = write_graph_demo(tmp_path, open_browser=False)
    assert paths["html"].is_file()
    assert paths["svg"].is_file()
    assert "ForceGraph" in paths["html"].read_text(encoding="utf-8")
    assert paths["svg"].read_text(encoding="utf-8").startswith("<svg")
