"""CLI coverage for `dev map` command wiring (graph ops under the map umbrella) (query/trace/html and the
missing-graph guard on graph-backed subcommands)."""

import json

import devcouncil.indexing.graph as graph_pkg
import devcouncil.indexing.graph.build as graph_build
import devcouncil.indexing.viz as viz
from devcouncil.cli.main import app
from typer.testing import CliRunner

runner = CliRunner()


def test_graph_doctor_reports_a_kernel_older_than_the_store(tmp_path, monkeypatch):
    """The failure that took `dev map` down on 2026-09-02, as doctor must show it.

    Doctor used to audit the Python engine — grammar wheels, `index.sqlite`,
    the writer lease — none of which the kernel uses. The one thing an operator
    needed it to say that day was: the binary that will run is older than the
    store it is asked to open, and here is the command that fixes it.
    """
    import sqlite3

    store = tmp_path / ".devcouncil" / "codeintel" / "devmap.sqlite"
    store.parent.mkdir(parents=True)
    conn = sqlite3.connect(store)
    conn.execute("PRAGMA user_version = 12")
    conn.execute("CREATE TABLE t (x)")
    conn.commit()
    conn.close()

    monkeypatch.setattr(
        "devcouncil.devmap_health.engine_info",
        lambda root: {
            "binary": "/opt/old/devmap",
            "built_at": "2026-09-01T13:37:00",
            "version": "devmap 0.1.0 (schema 11)",
            "schema_version": 11,
            "error": None,
        },
    )
    monkeypatch.setattr(
        "devcouncil.devmap_health.kernel_status",
        lambda root: {"error": "unsupported future schema version 12"},
    )

    result = runner.invoke(app, ["map", "doctor", "--project-root", str(tmp_path)])

    assert result.exit_code == 1
    said = " ".join(result.output.split())
    assert "schema 12" in said
    assert "kernel supports 11" in said
    assert "cargo build --release -p devmap-cli" in said

    as_json = runner.invoke(app, ["map", "doctor", "--json", "--project-root", str(tmp_path)])
    assert as_json.exit_code == 1
    payload = json.loads(as_json.stdout)
    assert payload["ok"] is False
    schema = next(check for check in payload["checks"] if check["name"] == "schema")
    assert schema["ok"] is False and schema["critical"] is True


# --- query ------------------------------------------------------------------------


def test_graph_query_json(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_pkg, "query_symbol",
        lambda root, name: {"definitions": [{"id": "m.f", "kind": "function", "path": "m.py", "line": 1}]},
    )
    result = runner.invoke(app, ["map", "query", "f", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["definitions"][0]["id"] == "m.f"


def test_graph_query_human_with_defs(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_pkg, "query_symbol",
        lambda root, name: {"definitions": [{
            "id": "m.f", "kind": "function", "path": "m.py", "line": 1,
            "callers": ["m.g"], "callees": [], "importers": [],
        }]},
    )
    result = runner.invoke(app, ["map", "query", "f", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "m.f" in result.output
    assert "callers" in result.output


def test_graph_query_no_matches(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "query_symbol", lambda root, name: {"definitions": []})
    result = runner.invoke(app, ["map", "query", "ghost", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No matches" in result.output


def test_graph_query_error_exits(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "query_symbol", lambda root, name: {"error": "no graph"})
    result = runner.invoke(app, ["map", "query", "f", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# --- trace ------------------------------------------------------------------------


def test_graph_trace_found(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_pkg, "trace_path",
        lambda root, start, end: {"found": True, "path": ["a", "b", "c"]},
    )
    result = runner.invoke(app, ["map", "trace", "a", "c", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a" in result.output and "c" in result.output


def test_graph_trace_json(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_pkg, "trace_path", lambda root, start, end: {"found": True, "path": ["a", "b"]}
    )
    result = runner.invoke(app, ["map", "trace", "a", "b", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["found"] is True


def test_graph_trace_no_path(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "trace_path", lambda root, start, end: {"found": False})
    result = runner.invoke(app, ["map", "trace", "a", "z", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No path" in result.output


def test_graph_trace_error(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "trace_path", lambda root, start, end: {"error": "boom"})
    result = runner.invoke(app, ["map", "trace", "a", "z", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# --- graph-backed commands: missing-graph guard -----------------------------------


def test_graph_dead_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No code graph" in result.output


def test_graph_check_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["map", "check", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_process_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["map", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_impact_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["map", "impact", "--diff", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_export_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["map", "export", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# --- html -------------------------------------------------------------------------


def test_graph_html_success(tmp_path, monkeypatch):
    out = tmp_path / "graph.html"
    out.write_text("<html></html>", encoding="utf-8")
    monkeypatch.setattr(viz, "write_graph_html", lambda root, open_browser=False, symbols=False: out)

    result = runner.invoke(app, ["map", "graph-html", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Wrote" in result.output


def test_graph_html_missing_graph(tmp_path, monkeypatch):
    def boom(root, open_browser=False, symbols=False):
        raise FileNotFoundError("no graph json")

    monkeypatch.setattr(viz, "write_graph_html", boom)
    result = runner.invoke(app, ["map", "graph-html", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# --- shared fakes -----------------------------------------------------------------

from types import SimpleNamespace  # noqa: E402

import devcouncil.indexing.graph.intel as intel_mod  # noqa: E402
import devcouncil.indexing.graph.export as export_mod  # noqa: E402


class _DeadEntry:
    def __init__(self, path, line, sid, kind, reason, confidence="inferred"):
        self.path = path
        self.line = line
        self.id = sid
        self.kind = kind
        self.reason = reason
        self.confidence = confidence

    def model_dump(self):
        return {"path": self.path, "line": self.line, "id": self.id, "reason": self.reason}


def _fake_graph(dead=None, edges=None):
    return SimpleNamespace(dead_code=list(dead or []), edges=list(edges or []))


# --- dead -------------------------------------------------------------------------


def test_graph_dead_human_with_entries(tmp_path, monkeypatch):
    entries = [
        _DeadEntry("a.py", 3, "a.f", "function", "no callers"),
        _DeadEntry("b.py", 5, "b.g", "function", "no callers"),
    ]
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph(dead=entries))
    result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a.f" in result.output
    assert "Reason summary" in result.output


def test_graph_dead_json_and_confidence_filter(tmp_path, monkeypatch):
    entries = [
        _DeadEntry("a.py", 3, "a.f", "function", "no callers", confidence="extracted"),
        _DeadEntry("b.py", 5, "b.g", "function", "no callers", confidence="ambiguous"),
    ]
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph(dead=entries))
    result = runner.invoke(
        app,
        ["map", "dead", "--json", "--confidence", "extracted", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert len(data["dead_code"]) == 1
    assert data["dead_code"][0]["id"] == "a.f"
    assert data.get("graph_degraded") is False


def test_graph_dead_empty_with_hidden(tmp_path, monkeypatch):
    entries = [_DeadEntry("b.py", 5, "b.g", "function", "no callers", confidence="ambiguous")]
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph(dead=entries))
    # default min-confidence "inferred" hides the ambiguous entry.
    result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No dead-code entries" in result.output
    assert "hidden" in result.output


# --- check ------------------------------------------------------------------------


def test_graph_check_human(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "graph_check",
        lambda graph, top_n=15: {
            "god_nodes": [{"degree": 9, "id": "m.big", "kind": "function"}],
            "circular_imports": [{"nodes": ["a", "b"]}],
            "package_init_count": 2,
        },
    )
    result = runner.invoke(app, ["map", "check", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "God nodes" in result.output
    assert "m.big" in result.output
    assert "Circular imports" in result.output
    assert "2 package-__init__ component(s) suppressed" in result.output


def test_graph_check_json_no_cycles(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "graph_check",
        lambda graph, top_n=15: {"god_nodes": [], "circular_imports": []},
    )
    result = runner.invoke(app, ["map", "check", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["god_nodes"] == []


# --- process ----------------------------------------------------------------------


def test_graph_process_human(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "extract_processes",
        lambda graph, entry=None, max_depth=6: [{"name": "flow", "depth": 2, "steps": ["a", "b"]}],
    )
    result = runner.invoke(app, ["map", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "flow" in result.output


def test_graph_process_empty(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(intel_mod, "extract_processes", lambda graph, entry=None, max_depth=6: [])
    result = runner.invoke(app, ["map", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No processes found" in result.output


# --- impact -----------------------------------------------------------------------


def test_graph_impact_requires_paths_or_diff(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    result = runner.invoke(app, ["map", "impact", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "Provide paths or --diff" in result.output


def test_graph_impact_human_with_results(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "diff_impact",
        lambda root, graph, paths=None, use_diff=False, max_depth=3: {
            "paths": [{
                "path": "a.py",
                "symbols": [{"id": "a.f"}],
                "blast": {"layers": [{"depth": 1, "confidence": "high", "nodes": ["b.g", "c.h"]}]},
            }]
        },
    )
    result = runner.invoke(app, ["map", "impact", "a.py", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a.py" in result.output
    assert "depth 1" in result.output


def test_graph_impact_no_paths(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "diff_impact",
        lambda root, graph, paths=None, use_diff=False, max_depth=3: {"paths": []},
    )
    result = runner.invoke(app, ["map", "impact", "--diff", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No impacted paths" in result.output


# --- export -----------------------------------------------------------------------


def _no_python_graph(monkeypatch):
    """GraphML comes from the kernel; the Python graph must not even be loaded."""

    def _never(root):
        raise AssertionError("graphml export must not load the Python code graph")

    monkeypatch.setattr(graph_build, "load_code_graph", _never)


def test_graph_export_graphml_stdout(tmp_path, monkeypatch):
    import devcouncil.devmap_engine as devmap_engine

    _no_python_graph(monkeypatch)
    calls = []

    def fake_export(root, *, output=None, timeout=120.0):
        calls.append((root, output))
        return {"text": "<graphml/>\n"}

    monkeypatch.setattr(devmap_engine, "export_graphml", fake_export)
    result = runner.invoke(app, ["map", "export", "--format", "graphml", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    assert "<graphml/>" in result.output
    assert calls == [(tmp_path.resolve(), None)]


def test_graph_export_graphml_to_file_reports_the_kernel_counts(tmp_path, monkeypatch):
    import devcouncil.devmap_engine as devmap_engine

    _no_python_graph(monkeypatch)

    def fake_export(root, *, output=None, timeout=120.0):
        output.write_text("<graphml/>", encoding="utf-8")
        return {"nodes": 3, "edges": 2, "edges_dangling": 1, "characters_replaced": 0}

    monkeypatch.setattr(devmap_engine, "export_graphml", fake_export)
    out = tmp_path / "out" / "g.graphml"
    result = runner.invoke(
        app, ["map", "export", "--format", "graphml", "-o", str(out), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0, result.output
    assert out.read_text(encoding="utf-8") == "<graphml/>"
    # The repairs the Python exporter never made, and never reported. Rich
    # wraps the line at the runner's 80 columns, so compare on words.
    assert "1 edge(s) omitted" in " ".join(result.output.split())


def test_graph_export_graphml_kernel_failure_is_red(tmp_path, monkeypatch):
    import devcouncil.devmap_engine as devmap_engine
    from devcouncil.devmap_engine import DevMapEngineError

    _no_python_graph(monkeypatch)

    def fake_export(root, *, output=None, timeout=120.0):
        raise DevMapEngineError("no devmap binary", code="engine_unavailable", stage="export")

    monkeypatch.setattr(devmap_engine, "export_graphml", fake_export)
    result = runner.invoke(app, ["map", "export", "--format", "graphml", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "no devmap binary" in result.output


def test_graph_export_okf_requires_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    result = runner.invoke(app, ["map", "export", "--format", "okf", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "requires -o" in result.output


def test_graph_export_okf_writes_bundle(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    out_dir = tmp_path / "okf"
    monkeypatch.setattr(
        export_mod, "write_code_graph_okf",
        lambda root, target, graph=None: (target, ["a.json", "b.json"]),
    )
    result = runner.invoke(
        app, ["map", "export", "--format", "okf", "-o", str(out_dir), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "OKF bundle" in result.output


def test_graph_export_okf_missing_graph_file(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())

    def boom(root, target, graph=None):
        raise FileNotFoundError("no okf source")

    monkeypatch.setattr(export_mod, "write_code_graph_okf", boom)
    result = runner.invoke(
        app, ["map", "export", "--format", "okf", "-o", str(tmp_path / "okf"), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1


def test_graph_export_okf_links(tmp_path, monkeypatch):
    edges = [
        SimpleNamespace(kind="imports", source="a", target="b"),
        SimpleNamespace(kind="calls", source="b", target="c"),
        SimpleNamespace(kind="inherits", source="c", target="d"),
    ]
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph(edges=edges))
    result = runner.invoke(app, ["map", "export", "--format", "okf-links", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a --imports--> b" in result.output
    assert "inherits" not in result.output


def test_graph_export_unknown_format(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    result = runner.invoke(app, ["map", "export", "--format", "bogus", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "Unknown format" in result.output


# --- view -------------------------------------------------------------------------


def test_graph_view_serves_and_stops(tmp_path, monkeypatch):
    out = tmp_path / "graph.html"
    out.write_text("<html></html>", encoding="utf-8")
    monkeypatch.setattr(viz, "write_graph_html", lambda root, open_browser=False: out)

    import socketserver
    import threading
    import webbrowser

    class _FakeServer:
        def __init__(self, addr, handler):
            pass

        def __enter__(self):
            return self

        def __exit__(self, *a):
            return False

        def serve_forever(self):
            raise KeyboardInterrupt

    monkeypatch.setattr(socketserver, "TCPServer", _FakeServer)
    monkeypatch.setattr(threading, "Timer", lambda *a, **k: SimpleNamespace(start=lambda: None))
    monkeypatch.setattr(webbrowser, "open", lambda url: None)

    result = runner.invoke(app, ["map", "view", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Stopped" in result.output


def test_graph_view_missing_graph(tmp_path, monkeypatch):
    def boom(root, open_browser=False):
        raise FileNotFoundError("no graph")

    monkeypatch.setattr(viz, "write_graph_html", boom)
    result = runner.invoke(app, ["map", "view", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# ── devmap query: edge scoping and fail-closed edge lists ────────────────────
#
# These pin three defects found 2026-08-24 against a real 36k-node store:
#   1. `callees` was queried with the FILE path, so a function's callees were
#      its whole file's outbound edges (35 reported vs 0 symbol-scoped).
#   2. edges were not filtered by kind, so `Contains`/`MemberOf` structural
#      edges appeared as calls — which is why symbols looked self-calling.
#   3. a failed or unavailable edge query silently became `[]`, indistinguishable
#      from "nothing calls this".

class _FakeResp:
    def __init__(self, items, resolution="Available", total=None, truncated=False):
        self.items = items
        self.resolution = resolution
        self.total = len(items) if total is None else total
        self.truncated = truncated
        self.tokens_used = 0


class _FakeClient:
    """Records the targets it is asked about so scoping can be asserted."""

    def __init__(self, *, deps_items=None, impact_items=None, deps_exc=None,
                 impact_resolution="Available"):
        self.deps_items = deps_items or []
        self.impact_items = impact_items or []
        self.deps_exc = deps_exc
        self.impact_resolution = impact_resolution
        self.deps_targets = []
        self.impact_targets = []

    def search(self, query, limit=2000):
        return _FakeResp([{
            "file_path": "pkg/mod.py",
            "symbol_name": "widget",
            "kind": "function",
            "span": (12, 20),
        }])

    def impact(self, target, depth=1):
        self.impact_targets.append(target)
        return _FakeResp(self.impact_items, resolution=self.impact_resolution)

    def deps(self, target, depth=1):
        self.deps_targets.append(target)
        if self.deps_exc is not None:
            raise self.deps_exc
        return _FakeResp(self.deps_items)


def _query(monkeypatch, tmp_path, client):
    from devcouncil.cli.commands import graph_cmd
    import devcouncil.devmap_client as devmap_client

    monkeypatch.setattr(devmap_client, "try_connect", lambda root: client)
    return graph_cmd._devmap_query_payload(tmp_path, "query", name_or_path="widget")


def test_query_scopes_callees_to_the_symbol_not_the_file(monkeypatch, tmp_path):
    client = _FakeClient(deps_items=[
        {"edge_kind": "Calls", "target_symbol": "pkg/mod.py::helper"},
    ])
    result = _query(monkeypatch, tmp_path, client)

    # Both directions must ask about the same node.
    assert client.deps_targets == ["pkg/mod.py::widget"]
    assert client.impact_targets == ["pkg/mod.py::widget"]
    assert result["definitions"][0]["callees"] == ["pkg/mod.py::helper"]


def test_query_drops_structural_edges_from_call_lists(monkeypatch, tmp_path):
    client = _FakeClient(
        deps_items=[
            {"edge_kind": "Contains", "target_symbol": "pkg/mod.py::widget"},
            {"edge_kind": "MemberOf", "target_symbol": "pkg/mod.py::Widget"},
            {"edge_kind": "Calls", "target_symbol": "pkg/mod.py::helper"},
        ],
        impact_items=[
            {"edge_kind": "Contains", "source_symbol": "pkg/mod.py"},
            {"edge_kind": "Calls", "source_symbol": "pkg/other.py::caller"},
        ],
    )
    definition = _query(monkeypatch, tmp_path, client)["definitions"][0]

    # A `Contains` edge from the file to this very symbol is what made symbols
    # appear to call themselves.
    assert definition["callees"] == ["pkg/mod.py::helper"]
    assert definition["callers"] == ["pkg/other.py::caller"]


def test_query_reports_a_failed_edge_query_as_unknown_not_empty(monkeypatch, tmp_path):
    from devcouncil.devmap_client import DevMapClientError

    client = _FakeClient(
        impact_items=[{"edge_kind": "Calls", "source_symbol": "pkg/other.py::caller"}],
        deps_exc=DevMapClientError("socket closed"),
    )
    definition = _query(monkeypatch, tmp_path, client)["definitions"][0]

    # None, not [] — "I could not check" must not read as "nothing".
    assert definition["callees"] is None
    assert "socket closed" in definition["callees_unavailable"]
    # The direction that DID succeed still reports normally.
    assert definition["callers"] == ["pkg/other.py::caller"]
    assert definition["callers_unavailable"] is None


def test_query_reports_unavailable_resolution_as_unknown(monkeypatch, tmp_path):
    client = _FakeClient(impact_resolution={"Unavailable": {"reason": "index rebuilding"}})
    definition = _query(monkeypatch, tmp_path, client)["definitions"][0]

    assert definition["callers"] is None
    assert "index rebuilding" in definition["callers_unavailable"]


def test_query_does_not_present_uncomputed_importers_as_empty(monkeypatch, tmp_path):
    definition = _query(monkeypatch, tmp_path, _FakeClient())["definitions"][0]

    # Was a hardcoded [], which reads as "nothing imports this".
    assert definition["importers"] is None
    assert definition["importers_unavailable"]


def test_query_distinguishes_empty_from_unknown_when_rendering(monkeypatch, tmp_path):
    from devcouncil.cli.commands.graph_cmd import _render_edge_field

    assert _render_edge_field({"callers": []}, "callers") == "(none)"
    rendered = _render_edge_field(
        {"callers": None, "callers_unavailable": "impact failed: boom"}, "callers"
    )
    assert "unknown" in rendered and "boom" in rendered
