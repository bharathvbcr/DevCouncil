"""CLI coverage for `dev graph` command wiring (query/trace/html and the
missing-graph guard on graph-backed subcommands)."""

import json

import devcouncil.codeintel as codeintel
import devcouncil.codeintel.languages as codeintel_languages
import devcouncil.indexing.graph as graph_pkg
import devcouncil.indexing.graph.build as graph_build
import devcouncil.indexing.viz as viz
from devcouncil.cli.main import app
from typer.testing import CliRunner

runner = CliRunner()


def test_graph_doctor_reports_actionable_embedded_grammar_gaps(tmp_path, monkeypatch):
    monkeypatch.setattr(
        codeintel,
        "get_codeintel_service",
        lambda root: SimpleNamespace(
            status=lambda: {"state": "committed", "schema_version": 1}
        ),
    )
    monkeypatch.setattr(
        codeintel_languages,
        "grammar_status",
        lambda: {
            "ok": False,
            "available_count": 34,
            "required_count": 35,
            "languages": [{
                "language": "Svelte",
                "available": False,
                "missing_grammars": ["css", "html"],
            }],
            "action": (
                "Install the platform-matched devcouncil-codeintel-grammars wheel; "
                "runtime grammar downloads are disabled."
            ),
        },
    )

    result = runner.invoke(
        app,
        ["graph", "doctor", "--project-root", str(tmp_path)],
    )

    assert result.exit_code == 1
    assert "Svelte (css, html)" in result.output
    assert "platform-matched devcouncil-codeintel-grammars" in result.output
    assert "runtime grammar downloads are disabled" in result.output


# --- query ------------------------------------------------------------------------


def test_graph_query_json(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_pkg, "query_symbol",
        lambda root, name: {"definitions": [{"id": "m.f", "kind": "function", "path": "m.py", "line": 1}]},
    )
    result = runner.invoke(app, ["graph", "query", "f", "--json", "--project-root", str(tmp_path)])
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
    result = runner.invoke(app, ["graph", "query", "f", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "m.f" in result.output
    assert "callers" in result.output


def test_graph_query_no_matches(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "query_symbol", lambda root, name: {"definitions": []})
    result = runner.invoke(app, ["graph", "query", "ghost", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No matches" in result.output


def test_graph_query_error_exits(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "query_symbol", lambda root, name: {"error": "no graph"})
    result = runner.invoke(app, ["graph", "query", "f", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# --- trace ------------------------------------------------------------------------


def test_graph_trace_found(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_pkg, "trace_path",
        lambda root, start, end: {"found": True, "path": ["a", "b", "c"]},
    )
    result = runner.invoke(app, ["graph", "trace", "a", "c", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a" in result.output and "c" in result.output


def test_graph_trace_json(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_pkg, "trace_path", lambda root, start, end: {"found": True, "path": ["a", "b"]}
    )
    result = runner.invoke(app, ["graph", "trace", "a", "b", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["found"] is True


def test_graph_trace_no_path(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "trace_path", lambda root, start, end: {"found": False})
    result = runner.invoke(app, ["graph", "trace", "a", "z", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No path" in result.output


def test_graph_trace_error(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_pkg, "trace_path", lambda root, start, end: {"error": "boom"})
    result = runner.invoke(app, ["graph", "trace", "a", "z", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# --- graph-backed commands: missing-graph guard -----------------------------------


def test_graph_dead_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["graph", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No code graph" in result.output


def test_graph_check_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["graph", "check", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_process_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["graph", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_impact_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["graph", "impact", "--diff", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_export_requires_graph(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: None)
    result = runner.invoke(app, ["graph", "export", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


# --- html -------------------------------------------------------------------------


def test_graph_html_success(tmp_path, monkeypatch):
    out = tmp_path / "graph.html"
    out.write_text("<html></html>", encoding="utf-8")
    monkeypatch.setattr(viz, "write_graph_html", lambda root, open_browser=False, symbols=False: out)

    result = runner.invoke(app, ["graph", "html", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Wrote" in result.output


def test_graph_html_missing_graph(tmp_path, monkeypatch):
    def boom(root, open_browser=False, symbols=False):
        raise FileNotFoundError("no graph json")

    monkeypatch.setattr(viz, "write_graph_html", boom)
    result = runner.invoke(app, ["graph", "html", "--project-root", str(tmp_path)])
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
    result = runner.invoke(app, ["graph", "dead", "--project-root", str(tmp_path)])
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
        ["graph", "dead", "--json", "--confidence", "extracted", "--project-root", str(tmp_path)],
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert len(data) == 1
    assert data[0]["id"] == "a.f"


def test_graph_dead_empty_with_hidden(tmp_path, monkeypatch):
    entries = [_DeadEntry("b.py", 5, "b.g", "function", "no callers", confidence="ambiguous")]
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph(dead=entries))
    # default min-confidence "inferred" hides the ambiguous entry.
    result = runner.invoke(app, ["graph", "dead", "--project-root", str(tmp_path)])
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
        },
    )
    result = runner.invoke(app, ["graph", "check", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "God nodes" in result.output
    assert "m.big" in result.output
    assert "Circular imports" in result.output


def test_graph_check_json_no_cycles(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "graph_check",
        lambda graph, top_n=15: {"god_nodes": [], "circular_imports": []},
    )
    result = runner.invoke(app, ["graph", "check", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["god_nodes"] == []


# --- process ----------------------------------------------------------------------


def test_graph_process_human(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "extract_processes",
        lambda graph, entry=None, max_depth=6: [{"name": "flow", "depth": 2, "steps": ["a", "b"]}],
    )
    result = runner.invoke(app, ["graph", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "flow" in result.output


def test_graph_process_empty(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(intel_mod, "extract_processes", lambda graph, entry=None, max_depth=6: [])
    result = runner.invoke(app, ["graph", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No processes found" in result.output


# --- impact -----------------------------------------------------------------------


def test_graph_impact_requires_paths_or_diff(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    result = runner.invoke(app, ["graph", "impact", "--project-root", str(tmp_path)])
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
    result = runner.invoke(app, ["graph", "impact", "a.py", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a.py" in result.output
    assert "depth 1" in result.output


def test_graph_impact_no_paths(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(
        intel_mod, "diff_impact",
        lambda root, graph, paths=None, use_diff=False, max_depth=3: {"paths": []},
    )
    result = runner.invoke(app, ["graph", "impact", "--diff", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No impacted paths" in result.output


# --- export -----------------------------------------------------------------------


def test_graph_export_graphml_stdout(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(export_mod, "export_graphml", lambda graph: "<graphml/>")
    result = runner.invoke(app, ["graph", "export", "--format", "graphml", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "<graphml/>" in result.output


def test_graph_export_graphml_to_file(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    monkeypatch.setattr(export_mod, "export_graphml", lambda graph: "<graphml/>")
    out = tmp_path / "out" / "g.graphml"
    result = runner.invoke(
        app, ["graph", "export", "--format", "graphml", "-o", str(out), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert out.read_text(encoding="utf-8") == "<graphml/>"


def test_graph_export_okf_requires_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    result = runner.invoke(app, ["graph", "export", "--format", "okf", "--project-root", str(tmp_path)])
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
        app, ["graph", "export", "--format", "okf", "-o", str(out_dir), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    assert "OKF bundle" in result.output


def test_graph_export_okf_missing_graph_file(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())

    def boom(root, target, graph=None):
        raise FileNotFoundError("no okf source")

    monkeypatch.setattr(export_mod, "write_code_graph_okf", boom)
    result = runner.invoke(
        app, ["graph", "export", "--format", "okf", "-o", str(tmp_path / "okf"), "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 1


def test_graph_export_okf_links(tmp_path, monkeypatch):
    edges = [
        SimpleNamespace(kind="imports", source="a", target="b"),
        SimpleNamespace(kind="calls", source="b", target="c"),
        SimpleNamespace(kind="inherits", source="c", target="d"),
    ]
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph(edges=edges))
    result = runner.invoke(app, ["graph", "export", "--format", "okf-links", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a --imports--> b" in result.output
    assert "inherits" not in result.output


def test_graph_export_unknown_format(tmp_path, monkeypatch):
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: _fake_graph())
    result = runner.invoke(app, ["graph", "export", "--format", "bogus", "--project-root", str(tmp_path)])
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

    result = runner.invoke(app, ["graph", "view", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "Stopped" in result.output


def test_graph_view_missing_graph(tmp_path, monkeypatch):
    def boom(root, open_browser=False):
        raise FileNotFoundError("no graph")

    monkeypatch.setattr(viz, "write_graph_html", boom)
    result = runner.invoke(app, ["graph", "view", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
