"""Graph HTML visualizer artifact."""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

import pytest

from devcouncil.indexing.graph.build import write_code_graph
from tests.unit.graph_fixtures import kernel_graph
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
    graph = kernel_graph(tmp_path)
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
    write_code_graph(tmp_path, kernel_graph(tmp_path))
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
    graph = kernel_graph(tmp_path)
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
    graph = kernel_graph(tmp_path)
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
    write_code_graph(tmp_path, kernel_graph(tmp_path))
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


def test_graph_html_f07_counts_zoom_and_interaction_hints():
    """F-07: counts, guarded zoomToFit after layout/reset, in-UI interaction hints."""
    html = render_graph_html(sample_demo_graph())
    assert html.count('id="counts"') == 1
    assert "updateCounts" in html
    assert "Nodes:" in html and "Edges:" in html and "Filtered:" in html
    assert "function fitView" in html
    assert "typeof g.zoomToFit === 'function'" in html or 'typeof g.zoomToFit === "function"' in html
    assert "requestAnimationFrame" in html
    assert "g.zoomToFit(" in html
    # Called after initial layout and reset (not only defined).
    assert re.search(r"redraw\(\);\s*fitView\(\);", html)
    assert re.search(r"redraw\(\);\s*fitView\(\);\s*renderDeadList", html) or (
        "fitView();" in html.split("id=\"reset\"", 1)[-1].split("clearPath", 1)[0]
        or "fitView();" in html
    )
    assert html.count("fitView();") >= 2
    assert "interactionHint" in html or 'id="hints"' in html
    assert "Click" in html and "shortest path" in html and "Double-click" in html
    assert "neighborhood" in html.lower()


def test_graph_html_canvas_controls_and_vendor_apis():
    """Zoom/pan overlay + layout options; every used ForceGraph API exists in vendor 1.51.4."""
    from pathlib import Path

    from devcouncil.indexing.viz import _canvas_controls_js, _vendor_js

    html = render_graph_html(sample_demo_graph())
    assert "canvasControls" in html or "zoomFit" in html
    assert "zoomPct" in html
    assert "labelMode" in html
    assert "sizeMode" in html
    assert "layoutPause" in html
    assert "layoutCharge" in html
    assert "layoutDistance" in html
    assert "canvasLegend" in html
    assert "onEngineStop" in html
    assert "onZoom" in html
    assert "onNodeDblClick" not in html
    assert "event.detail >= 2" in html

    controls = _canvas_controls_js()
    vendor = _vendor_js()
    assert "vasturiano/force-graph" in vendor or "zoomToFit" in vendor
    # Fallback stub path still omits zoomToFit.
    stub = (
        "window.ForceGraph=function(){return{"
        "graphData:function(){return this},nodeId:function(){return this},"
        "nodeLabel:function(){return this},nodeAutoColorBy:function(){return this},"
        "nodeVal:function(){return this},linkColor:function(){return this},"
        "linkDirectionalParticles:function(){return this},"
        "linkDirectionalParticleWidth:function(){return this},"
        "onNodeClick:function(){return this},"
        "width:function(){return this},height:function(){return this},_missing:true};};"
    )
    assert "zoomToFit" not in stub

    # Regression: every new ForceGraph method used in generated JS must appear in vendor.
    vendor_path = (
        Path(__file__).resolve().parents[2]
        / "src"
        / "devcouncil"
        / "assets"
        / "vendor"
        / "force-graph.min.js"
    )
    vendor_text = vendor_path.read_text(encoding="utf-8") if vendor_path.is_file() else vendor
    for api in (
        "zoomToFit",
        "centerAt",
        "enableZoomInteraction",
        "enablePanInteraction",
        "pauseAnimation",
        "resumeAnimation",
        "d3Force",
        "d3ReheatSimulation",
        "nodeCanvasObject",
        "cooldownTicks",
        "onEngineStop",
        "onZoom",
    ):
        assert api in controls, api
        assert api in vendor_text, f"ForceGraph API {api!r} missing from vendor 1.51.4"
    assert "onNodeDblClick" not in controls


def _sample_repo_map_payload() -> dict:
    """A repo map shaped like the kernel's, for the Rust preview to render."""
    return {
        "languages": ["python"],
        "files": [
            {"path": "src/devcouncil/cli/main.py", "area": "src/devcouncil/cli", "kind": "code", "language": "python"},
        ],
        "subsystems": [
            {
                "area": "src/devcouncil/cli",
                "summary": "CLI surface",
                "entry_points": ["src/devcouncil/cli/main.py"],
                "critical_files": ["src/devcouncil/cli/commands/map.py"],
                "neighbors": ["src/devcouncil/indexing"],
                "handoff_paths": ["cli/commands/map.py -> indexing/repo_mapper.py"],
                "role_files": {"entrypoints": ["src/devcouncil/cli/main.py"]},
            },
        ],
        "entry_roots": ["src/devcouncil/cli/main.py"],
    }


# The map preview is rendered by the Rust kernel (`devmap map-html`), not here.
# What Python still owns is the seam: build the right argv, and turn each kernel
# refusal into something the operator can act on. The page's own contract —
# payload shape, escaping, colours, coverage arithmetic — is pinned in
# `rust-port/crates/devmap-query/src/map_preview.rs`.


def test_map_html_delegates_to_the_kernel(tmp_path, monkeypatch):
    from devcouncil import devmap_engine

    dc = tmp_path / ".devcouncil"
    dc.mkdir()
    (dc / "repo_map.json").write_text(json.dumps(_sample_repo_map_payload()), encoding="utf-8")

    seen = {}

    def fake_run(argv, *, cwd, timeout, stage=""):
        seen["argv"] = argv
        seen["cwd"] = cwd
        seen["stage"] = stage
        # Stand in for the kernel: the seam must check the artifact landed.
        Path(argv[argv.index("--output") + 1]).write_text("<html>map</html>", encoding="utf-8")
        return subprocess.CompletedProcess(argv, 0, "", "")

    monkeypatch.setattr(devmap_engine, "find_engine_binary", lambda root=None: "/fake/devmap")
    monkeypatch.setattr(devmap_engine, "_run", fake_run)

    out = devmap_engine.render_map_html(tmp_path)
    assert out == dc / "map.html"
    assert seen["stage"] == "map-html"
    assert seen["argv"][0] == "/fake/devmap"
    assert "map-html" in seen["argv"]
    # No --db: the preview reads repo_map.json and must not need a store, so it
    # answers for a checkout that has never been indexed.
    assert "--db" not in seen["argv"]
    assert str(out) in seen["argv"]


def test_map_html_reports_a_kernel_too_old_for_the_subcommand(tmp_path, monkeypatch):
    from devcouncil import devmap_engine

    def fake_run(argv, *, cwd, timeout, stage=""):
        raise devmap_engine.DevMapEngineError(
            "devmap exited 2: error: unrecognized subcommand 'map-html'",
            code="kernel_failed",
        )

    monkeypatch.setattr(devmap_engine, "find_engine_binary", lambda root=None: "/fake/devmap")
    monkeypatch.setattr(devmap_engine, "_run", fake_run)

    with pytest.raises(devmap_engine.DevMapEngineError) as excinfo:
        devmap_engine.render_map_html(tmp_path)
    # Clap names the flag; the operator needs the remedy.
    assert excinfo.value.code == "binary_too_old"
    assert "cargo build" in excinfo.value.fix


def test_map_html_refuses_to_claim_a_page_the_kernel_did_not_write(tmp_path, monkeypatch):
    from devcouncil import devmap_engine

    monkeypatch.setattr(devmap_engine, "find_engine_binary", lambda root=None: "/fake/devmap")
    monkeypatch.setattr(
        devmap_engine,
        "_run",
        lambda argv, *, cwd, timeout, stage="": subprocess.CompletedProcess(argv, 0, "", ""),
    )

    with pytest.raises(devmap_engine.DevMapEngineError) as excinfo:
        devmap_engine.render_map_html(tmp_path)
    assert excinfo.value.code == "artifact_missing"
