"""CLI coverage for `dev map` command wiring (graph ops under the map umbrella) (query/trace/html and the
missing-graph guard on graph-backed subcommands)."""

import json

import pytest

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
#
# These render whatever payload the engine returns. The engine used to be either
# the kernel or `indexing.graph.query.query_symbol` over `load_code_graph`; it is
# now only the kernel, so the double is `_devmap_query_payload` — the one seam
# both `dev map query` and the MCP tool go through. The rendering assertions are
# unchanged.


def _kernel_says(monkeypatch, payload):
    from devcouncil.cli.commands import graph_cmd

    monkeypatch.setattr(
        graph_cmd, "_devmap_query_payload", lambda *a, **k: payload, raising=False
    )


def test_graph_query_json(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {
        "definitions": [{"id": "m.f", "kind": "function", "path": "m.py", "line": 1}]
    })
    result = runner.invoke(app, ["map", "query", "f", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["definitions"][0]["id"] == "m.f"


def test_graph_query_human_with_defs(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {
        "definitions": [{
            "id": "m.f", "kind": "function", "path": "m.py", "line": 1,
            "callers": ["m.g"], "callees": [], "importers": [],
        }]
    })
    result = runner.invoke(app, ["map", "query", "f", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "m.f" in result.output
    assert "callers" in result.output


def test_graph_query_no_matches(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {"definitions": []})
    result = runner.invoke(app, ["map", "query", "ghost", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No matches" in result.output


def test_graph_query_error_exits(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {"error": "no graph"})
    result = runner.invoke(app, ["map", "query", "f", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_query_without_a_kernel_refuses_rather_than_answering(tmp_path, monkeypatch):
    """No kernel is an error naming it, not a second engine's answer.

    `query_symbol` walked the whole Python graph out of `index.sqlite` to
    answer this — and the sibling `trace` fallback did not merely cost more, it
    returned *different* paths (its BFS was undirected).
    """
    _kernel_says(monkeypatch, None)
    result = runner.invoke(app, ["map", "query", "f", "--project-root", str(tmp_path)])
    assert result.exit_code != 0, result.output
    assert not isinstance(result.exception, AssertionError), result.exception
    assert "dev map" in result.output or "devmap" in result.output, result.output


# --- trace ------------------------------------------------------------------------


def test_graph_trace_found(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {"found": True, "path": ["a", "b", "c"]})
    result = runner.invoke(app, ["map", "trace", "a", "c", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a" in result.output and "c" in result.output


def test_graph_trace_json(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {"found": True, "path": ["a", "b"]})
    result = runner.invoke(app, ["map", "trace", "a", "b", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["found"] is True


def test_graph_trace_no_path(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {"found": False})
    result = runner.invoke(app, ["map", "trace", "a", "z", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No path" in result.output


def test_graph_trace_error(tmp_path, monkeypatch):
    _kernel_says(monkeypatch, {"error": "boom"})
    result = runner.invoke(app, ["map", "trace", "a", "z", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_trace_without_a_kernel_refuses_rather_than_answering(tmp_path, monkeypatch):
    """A fabricated path is worse than an absent answer: a caller acts on it."""
    _kernel_says(monkeypatch, None)
    result = runner.invoke(app, ["map", "trace", "a", "z", "--project-root", str(tmp_path)])
    assert result.exit_code != 0, result.output
    assert not isinstance(result.exception, AssertionError), result.exception
    assert "dev map" in result.output or "devmap" in result.output, result.output


# --- graph-backed commands: missing-graph guard -----------------------------------


def test_graph_dead_requires_graph(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, None)
    result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No code graph" in result.output


def test_graph_check_requires_graph(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, None)
    result = runner.invoke(app, ["map", "check", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_process_requires_graph(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, None)
    result = runner.invoke(app, ["map", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_impact_requires_graph(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, None)
    result = runner.invoke(app, ["map", "impact", "--diff", "--project-root", str(tmp_path)])
    assert result.exit_code == 1


def test_graph_export_requires_graph(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, None)
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

import devcouncil.cli.commands.graph_cmd as graph_cmd_mod  # noqa: E402
import devcouncil.indexing.graph.intel as intel_mod  # noqa: E402
import devcouncil.indexing.graph.export as export_mod  # noqa: E402


def _never_load_the_python_graph(root):
    """Two-signal proof for the commands that have left the Python read path.

    Patched over ``load_code_graph`` so a command that still reaches the retired
    engine's whole-graph read fails here loudly, rather than passing because the
    fixture happened to have no graph to load.
    """
    raise AssertionError(
        "this command reached the retired Python engine's whole-graph read; "
        "the kernel is the only graph engine"
    )


def _kernel_artifact(monkeypatch, graph, root=None):
    """Feed the graph-backed commands the kernel's own artifact.

    These are renderer tests: they assert what ``dev map check`` / ``process`` /
    ``dead`` / ``export`` *print* for a given graph, and the double is how the
    graph arrives. The seam moved. ``_require_graph`` used to call
    ``load_code_graph`` — the retired engine's read through the Python
    ``index.sqlite`` cache — and now calls ``read_code_graph``, a bounded
    ``json.load`` of ``code_graph.json``, the artifact the kernel writes.

    ``root`` writes an unstamped ``repo_map.json`` beside it. The staleness
    banner used to read the Python store's generation, found none, and printed
    nothing; it reads the map artifact now, so a root with a graph and no map is
    "freshness unknown" and says so. The kernel writes both files in one run, so
    the fixture writes both. Unstamped is deliberate: a map with no
    ``generated_head`` has nothing to compare and is not stale.
    """
    monkeypatch.setattr(graph_build, "read_code_graph", lambda _root: graph)
    if root is not None:
        map_path = root / ".devcouncil" / "repo_map.json"
        map_path.parent.mkdir(parents=True, exist_ok=True)
        map_path.write_text('{"files": []}', encoding="utf-8")


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


def _fake_graph(
    dead=None, edges=None, clusters=None, clusters_truncated=0, clusters_incomplete=None
):
    """A stand-in for a loaded ``CodeGraph``, derived from the model itself.

    Hand-listing the fields is how this fake went stale twice: ``graph_dead``
    reads whatever ``CodeGraph`` declares, and a ``SimpleNamespace`` missing a
    field raises ``AttributeError`` for the fake's reason rather than the
    production one. Taking the defaults off ``model_fields`` means a new field
    on the model appears here with its real default and nothing has to remember.
    """
    from devcouncil.indexing.graph.schema import CodeGraph

    fields = {
        name: field.get_default(call_default_factory=True)
        for name, field in CodeGraph.model_fields.items()
    }
    fields.update(
        dead_code=list(dead or []),
        edges=list(edges or []),
        dead_clusters=clusters,
        dead_clusters_truncated=clusters_truncated,
        dead_clusters_incomplete=clusters_incomplete,
    )
    return SimpleNamespace(**fields)


# --- dead -------------------------------------------------------------------------


def test_graph_dead_human_with_entries(tmp_path, monkeypatch):
    entries = [
        _DeadEntry("a.py", 3, "a.f", "function", "no callers"),
        _DeadEntry("b.py", 5, "b.g", "function", "no callers"),
    ]
    _kernel_artifact(monkeypatch, _fake_graph(dead=entries))
    result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a.f" in result.output
    assert "Reason summary" in result.output


def test_graph_dead_json_and_confidence_filter(tmp_path, monkeypatch):
    entries = [
        _DeadEntry("a.py", 3, "a.f", "function", "no callers", confidence="extracted"),
        _DeadEntry("b.py", 5, "b.g", "function", "no callers", confidence="ambiguous"),
    ]
    _kernel_artifact(monkeypatch, _fake_graph(dead=entries))
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
    _kernel_artifact(monkeypatch, _fake_graph(dead=entries))
    # default min-confidence "inferred" hides the ambiguous entry.
    result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No dead-code entries" in result.output
    assert "hidden" in result.output


def test_graph_dead_distinguishes_all_three_cluster_outcomes(tmp_path, monkeypatch):
    """The component scan has three outcomes and two of them look alike.

    "not computed" (the kernel refused an oversized graph), "not recorded" (this
    generation predates the pass) and "none" (it ran and found nothing) differ
    by a word, and the first two give *opposite* advice: rebuilding fixes the
    second and re-refuses on the first. What is pinned here is the branch order,
    which no single-state test can see.
    """
    cases = [
        # (kwargs, must contain, must not contain)
        (
            {"clusters_incomplete": "the call graph exceeded 400000 symbols"},
            ["not computed", "400000"],
            ["rebuild", "run `dev map`", "none."],
        ),
        ({"clusters": None}, ["not recorded", "dev map"], ["not computed"]),
        ({"clusters": []}, ["none."], ["not computed", "not recorded"]),
    ]
    for kwargs, expected, forbidden in cases:
        _kernel_artifact(monkeypatch, _fake_graph(**kwargs), root=tmp_path)
        result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
        assert result.exit_code == 0, result.output
        for text in expected:
            assert text in result.output, f"{kwargs} must print {text!r}: {result.output}"
        for text in forbidden:
            assert text not in result.output, (
                f"{kwargs} must not print {text!r}: {result.output}"
            )

    # A refusal that also carries a list is the kernel contradicting itself; the
    # readout must not silently pick the reassuring half.
    _kernel_artifact(
        monkeypatch, _fake_graph(clusters=[], clusters_incomplete="graph too large")
    )
    result = runner.invoke(app, ["map", "dead", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    assert "not computed" in result.output and "none." not in result.output


def test_graph_dead_json_carries_the_refusal(tmp_path, monkeypatch):
    """And the machine-readable path, which is what agents read."""
    _kernel_artifact(monkeypatch, _fake_graph(clusters_incomplete="graph too large"))
    result = runner.invoke(
        app, ["map", "dead", "--json", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    data = json.loads(result.stdout)
    assert data["dead_clusters"] is None, (
        "an empty list here would say the pass ran and found nothing"
    )
    assert data["dead_clusters_incomplete"] == "graph too large"


# --- check ------------------------------------------------------------------------


def test_graph_check_human(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, _fake_graph())
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
    _kernel_artifact(monkeypatch, _fake_graph())
    monkeypatch.setattr(
        intel_mod, "graph_check",
        lambda graph, top_n=15: {"god_nodes": [], "circular_imports": []},
    )
    result = runner.invoke(app, ["map", "check", "--json", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert json.loads(result.stdout)["god_nodes"] == []


# --- process ----------------------------------------------------------------------


def test_graph_process_human(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, _fake_graph())
    monkeypatch.setattr(
        intel_mod, "extract_processes",
        lambda graph, entry=None, max_depth=6: [{"name": "flow", "depth": 2, "steps": ["a", "b"]}],
    )
    result = runner.invoke(app, ["map", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "flow" in result.output


def test_graph_process_empty(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, _fake_graph())
    monkeypatch.setattr(intel_mod, "extract_processes", lambda graph, entry=None, max_depth=6: [])
    result = runner.invoke(app, ["map", "process", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No processes found" in result.output


# --- impact -----------------------------------------------------------------------


def test_graph_impact_requires_paths_or_diff(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, _fake_graph())
    result = runner.invoke(app, ["map", "impact", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "Provide paths or --diff" in result.output


def test_graph_impact_human_with_results(tmp_path, monkeypatch):
    """Rewritten: this fed the renderer `diff_impact`'s Python output.

    There is no Python engine under `dev map impact` any more — the kernel bands
    its own walk and the command renders that. The double is now the kernel
    payload, and the assertions cover what the Python shape could not carry: a
    band's measured confidence, its unlisted remainder, and the walk's own "I
    stopped early".
    """
    monkeypatch.setattr(
        graph_cmd_mod,
        "_devmap_query_payload",
        lambda root, kind, **kw: {
            "ok": True,
            "source": "devmap",
            "paths": [{
                "path": "a.py",
                "symbols": [{"id": "a.py::f"}],
                "symbols_are_walk_seeds": True,
                "blast": {
                    "seeds": ["a.py::f"],
                    "layers": [{
                        "depth": 1,
                        "nodes": ["b.g", "c.h"],
                        "count": 9,
                        "confidence": "extracted",
                        "confidence_score": 1.0,
                        "nodes_omitted": 7,
                    }],
                    "total_impacted": 9,
                    "walk_incomplete": "the walk did not complete: stopped at depth 3",
                },
            }],
        },
    )
    result = runner.invoke(app, ["map", "impact", "a.py", "--project-root", str(tmp_path)])
    assert result.exit_code == 0, result.output
    assert "a.py" in result.output
    assert "depth 1" in result.output
    # Square brackets here were eaten by Rich as console markup, so the tier
    # never printed on either engine. See the renderer's own note.
    assert "(extracted)" in result.output, result.output
    assert "7 not listed" in result.output, result.output
    assert "stopped at depth 3" in result.output, result.output


def test_graph_impact_no_paths(tmp_path, monkeypatch):
    monkeypatch.setattr(
        graph_cmd_mod,
        "_devmap_query_payload",
        lambda root, kind, **kw: {"ok": True, "source": "devmap", "paths": []},
    )
    result = runner.invoke(app, ["map", "impact", "--diff", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "No paths to analyse" in result.output


# --- export -----------------------------------------------------------------------


def _no_python_graph(monkeypatch):
    """GraphML comes from the kernel; the Python graph must not even be loaded."""

    def _never(root):
        raise AssertionError("graphml export must not load the Python code graph")



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
    _kernel_artifact(monkeypatch, _fake_graph())
    result = runner.invoke(app, ["map", "export", "--format", "okf", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "requires -o" in result.output


def test_graph_export_okf_writes_bundle(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, _fake_graph())
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
    _kernel_artifact(monkeypatch, _fake_graph())

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
    _kernel_artifact(monkeypatch, _fake_graph(edges=edges))
    result = runner.invoke(app, ["map", "export", "--format", "okf-links", "--project-root", str(tmp_path)])
    assert result.exit_code == 0
    assert "a --imports--> b" in result.output
    assert "inherits" not in result.output


def test_graph_export_unknown_format(tmp_path, monkeypatch):
    _kernel_artifact(monkeypatch, _fake_graph())
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
    """Records the targets and floors it is asked about, so both can be asserted.

    The signatures mirror `DevMapClient`'s, checked below by
    `test_the_edge_fake_matches_the_client_it_stands_in_for`: a fake that
    accepts fewer arguments than the real client turns a production call into a
    `TypeError` here and nowhere else, which is how it went stale when
    `min_rung` landed.
    """

    def __init__(self, *, deps_items=None, impact_items=None, deps_exc=None,
                 impact_resolution="Available"):
        self.deps_items = deps_items or []
        self.impact_items = impact_items or []
        self.deps_exc = deps_exc
        self.impact_resolution = impact_resolution
        self.deps_targets = []
        self.impact_targets = []
        self.rungs = []

    def search(self, query, limit=2000):
        return _FakeResp([{
            "file_path": "pkg/mod.py",
            "symbol_name": "widget",
            "kind": "function",
            "span": (12, 20),
        }])

    def impact(self, target, depth=1, min_rung=None, *, layers=False):
        self.impact_targets.append(target)
        self.rungs.append(("impact", min_rung))
        return _FakeResp(self.impact_items, resolution=self.impact_resolution)

    def deps(self, target, depth=1, min_rung=None):
        self.deps_targets.append(target)
        self.rungs.append(("deps", min_rung))
        if self.deps_exc is not None:
            raise self.deps_exc
        return _FakeResp(self.deps_items)


def _query(monkeypatch, tmp_path, client, min_rung=None):
    from devcouncil.cli.commands import graph_cmd
    import devcouncil.devmap_client as devmap_client

    monkeypatch.setattr(devmap_client, "try_connect", lambda root: client)
    return graph_cmd._devmap_query_payload(
        tmp_path, "query", name_or_path="widget", min_rung=min_rung
    )


def test_the_edge_fake_matches_the_client_it_stands_in_for():
    """The fake's edge methods must accept what the real client accepts.

    `_call_edges` passes `min_rung` to whichever method it was named, so a fake
    one argument short raises `TypeError` from production code — an error that
    says nothing about the behaviour under test and everything about the
    double. Derived from the real signatures rather than restated, so the next
    parameter to land is caught here instead of in five unrelated tests.
    """
    import inspect

    from devcouncil.devmap_client import DevMapClient

    for name in ("impact", "deps"):
        real = set(inspect.signature(getattr(DevMapClient, name)).parameters)
        fake = set(inspect.signature(getattr(_FakeClient, name)).parameters)
        assert real <= fake, (
            f"_FakeClient.{name} is missing {sorted(real - fake)}, which "
            "`_call_edges` will pass and production code will raise on"
        )


def test_the_query_payload_carries_the_floor_to_both_directions(monkeypatch, tmp_path):
    """`dev map query`'s edge lists must be filterable, on both sides.

    A floor applied to callers and not callees — or to neither — produces one
    answer whose halves were measured against different evidence. The kernel
    already guards that for `min_confidence`; this is the same rule for the
    rung, at the surface a human types.
    """
    client = _FakeClient(
        deps_items=[{"edge_kind": "Calls", "target_symbol": "pkg/mod.py::helper"}],
        impact_items=[{"edge_kind": "Calls", "source_symbol": "pkg/other.py::caller"}],
    )
    _query(monkeypatch, tmp_path, client, min_rung="deterministic")
    assert set(client.rungs) == {("impact", "deterministic"), ("deps", "deterministic")}, (
        f"both directions must carry the floor: {client.rungs}"
    )

    # OFF: no floor asked for, none sent — the shape every existing call makes.
    plain = _FakeClient()
    _query(monkeypatch, tmp_path, plain)
    assert {rung for _, rung in plain.rungs} == {None}, plain.rungs


def test_an_unknown_floor_is_refused_not_reported_as_a_missing_index(monkeypatch, tmp_path):
    """A typo must not be diagnosed as "no devmap index".

    `_devmap_query_payload` catches `DevMapClientError` and returns `None`, and
    every caller reads `None` as "the kernel could not answer". A rung name the
    client rejects would therefore surface as a missing index, and the reader
    would go and build one — twice — and still not get their filter. So the
    name is checked at the command, before the request is attempted.
    """
    for command in (
        ["map", "query", "widget", "--min-rung", "exact"],
        ["map", "trace", "a", "b", "--min-rung", "exact"],
    ):
        result = runner.invoke(app, [*command, "--project-root", str(tmp_path)])
        assert result.exit_code == 2, f"{command}: {result.output}"
        assert "deterministic" in result.output, (
            f"{command}: the refusal must name the values that work: {result.output}"
        )
        assert "index" not in result.output.lower(), (
            f"{command}: a typo is not a missing index: {result.output}"
        )


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


def test_a_client_that_cannot_apply_the_floor_takes_the_slow_path_not_a_broad_answer():
    """A batched command without the parameter must not answer at full breadth.

    The batched `neighbors` call is an optimisation; the per-target path asks
    `impact`/`deps` directly and has taken a floor since it landed. So a client
    too old for the batched floor has a correct answer available and must be
    routed to it — the failure mode being guarded is the other one, where the
    floor is quietly dropped and a broad edge list comes back looking exactly
    like the narrow one that was asked for.

    Probed rather than caught: `except TypeError` around the call would also
    swallow one raised inside the response handling, which is a genuine shape
    error and the reason `_neighbor_edges` probes for `neighbors` in the first
    place.
    """
    from devcouncil.cli.commands import graph_cmd
    from devcouncil.devmap_client import DevMapClientError

    calls = []

    def old_batched(targets):  # no `min_rung` parameter
        calls.append(targets)
        return []

    # No floor: unchanged, and the batched path is used.
    assert graph_cmd._batched_neighbors(old_batched, ["a.py::f"], None) == []
    assert calls == [["a.py::f"]]

    # A floor it cannot apply: refused, so the caller falls through.
    with pytest.raises(DevMapClientError, match="rung floor"):
        graph_cmd._batched_neighbors(old_batched, ["a.py::f"], "deterministic")
    assert calls == [["a.py::f"]], "the broad call must not have been made"

    # And a client that does accept it gets it.
    seen = {}

    def new_batched(targets, min_rung=None):
        seen["min_rung"] = min_rung
        return []

    graph_cmd._batched_neighbors(new_batched, ["a.py::f"], "high")
    assert seen == {"min_rung": "high"}


def test_a_direction_that_cannot_take_the_floor_is_unmeasured_not_unfiltered():
    """The per-target path applies the same rule, through its own contract.

    `_call_edges` returns `(None, reason)` for a direction it could not measure
    — never `[]`, which would read as "nothing calls this". A client that cannot
    apply a rung floor has not measured the direction the caller asked about, so
    that is exactly what it reports. The alternative — asking without the floor
    — returns a *wider* edge list under the caller's narrow question.
    """
    from devcouncil.cli.commands import graph_cmd

    class _Old:
        def impact(self, target, depth=1):  # no `min_rung`
            raise AssertionError("must not be called when the floor cannot be applied")

    edges, reason = graph_cmd._call_edges(
        _Old(), "impact", "a.py::f", "source_symbol", "source_file", "deterministic"
    )
    assert edges is None, "an unfilterable direction is unknown, not empty"
    assert "rung floor" in reason, reason

    # OFF: with no floor asked for, the same old client answers normally.
    class _OldAnswering(_Old):
        def impact(self, target, depth=1):
            return _FakeResp([{"edge_kind": "Calls", "source_symbol": "b.py::g"}])

    edges, reason = graph_cmd._call_edges(
        _OldAnswering(), "impact", "a.py::f", "source_symbol", "source_file"
    )
    assert reason is None and edges == ["b.py::g"], (edges, reason)


# --- `Abandoned cycles`: the state that had no test was the one with output ---
#
# `test_graph_dead_distinguishes_all_three_cluster_outcomes` above pins the
# three *absences* — refused, not recorded, none — through the whole command,
# which is where the branch order matters. The fourth state, a scan that ran and
# found something, had no assertion anywhere: not the size-versus-sample
# distinction (the line prints a four-name sample and the *true* membership, and
# a reader who reads the sample as the size under-counts a dead subsystem), not
# the truncation tail, and not what a confidence the payload did not carry
# renders as.
#
# These drive `_render_dead_clusters` directly rather than through `runner`.
# The absence cases are deliberately re-covered at this level: when the branch
# order breaks, a renderer test says which branch, and the command test says the
# reader sees it.


class _Lines:
    """A console that keeps what was printed, in order."""

    def __init__(self) -> None:
        self.lines: list[str] = []

    def print(self, text: str = "") -> None:
        self.lines.append(str(text))

    def text(self) -> str:
        return "\n".join(self.lines)


def _render(clusters, truncated=0, incomplete=None) -> str:
    from devcouncil.cli.commands.graph_cmd import _render_dead_clusters

    console_ = _Lines()
    _render_dead_clusters(console_, clusters, truncated, incomplete)
    return console_.text()


def test_a_refused_component_scan_says_so_and_does_not_advise_a_rebuild():
    """The refusal is checked first: the next build refuses the same graph."""
    out = _render([], incomplete="the call graph exceeded 400000 distinct symbols")

    assert "not computed" in out
    assert "exceeded 400000 distinct symbols" in out
    assert "none" not in out, "a refusal must not read as a finding of none"
    assert "dev map" not in out, "advising a rebuild of a scan that refuses is wrong"


def test_a_generation_predating_the_pass_is_not_a_finding_of_none():
    out = _render(None)

    assert "not recorded for this generation" in out
    assert "dev map" in out, "this is the one state where a rebuild does help"


def test_a_scan_that_ran_and_found_nothing_says_none():
    out = _render([])

    assert out.strip() == "Abandoned cycles: none."


def test_a_capped_list_carries_what_the_cap_cut():
    out = _render(
        [{"members": ["a", "b"], "size": 2, "confidence": 0.75}],
        truncated=7,
    )

    assert "1 component(s)" in out
    assert "0.75  2 symbols: a, b" in out
    assert "7 further component(s) were found and not listed" in out


def test_a_sampled_cluster_prints_its_true_size_not_its_sample_size():
    """The sample is four; the size is what the kernel counted."""
    out = _render([{"members": [f"s{i}" for i in range(9)], "size": 9, "confidence": 1.0}])

    assert "1.00  9 symbols: s0, s1, s2, s3, +5 more" in out


@pytest.mark.parametrize(
    "confidence",
    [None, "", "n/a", [], {}, {"value": 1}, float("nan")],
)
def test_a_confidence_the_payload_did_not_carry_prints_as_unknown(confidence):
    """No non-number is ever rendered as a number.

    ``nan`` is the exception that proves the rule: it is a float, so it formats,
    and it formats as ``nan`` — which is not a confidence a reader can mistake
    for one.
    """
    out = _render([{"members": ["a"], "size": 1, "confidence": confidence}])

    body = out.splitlines()[-1].strip()
    assert body.startswith("?") or body.startswith("nan"), body


def test_a_cluster_of_the_wrong_shape_is_skipped_not_crashed_on():
    out = _render(["not-a-dict", {"members": ["a"], "size": 1, "confidence": 0.5}])

    assert "0.50  1 symbols: a" in out
    assert "not-a-dict" not in out


# The wire shape is `members: Vec<String>`, `size: usize`, `confidence: f32`
# (`dead_clusters.rs:77`), so a well-formed generation cannot carry anything
# else. This renderer does not read a generation, though — it reads
# `code_graph.json` off disk, which a half-written build, a truncated copy or a
# hand edit can leave in any shape at all. A wrong type here used to raise
# through `dev map dead`: `int("many")` and iterating a non-iterable both
# escape, and a traceback is a worse answer than a missing field, because the
# reader loses the findings that *were* well-formed alongside it.
@pytest.mark.parametrize(
    "cluster",
    [
        {"members": 5, "size": 5, "confidence": 0.5},
        {"members": {"a": 1}, "size": 1, "confidence": 0.5},
        {"members": "abc", "size": 3, "confidence": 0.5},
        {"members": ["a"], "size": "many", "confidence": 0.5},
        {"members": ["a"], "size": [1], "confidence": 0.5},
        {"members": ["a"], "size": 1.7, "confidence": 0.5},
        {},
    ],
)
def test_a_malformed_cluster_does_not_take_the_well_formed_ones_with_it(cluster):
    out = _render([cluster, {"members": ["real"], "size": 1, "confidence": 0.9}])

    assert "0.90  1 symbols: real" in out, out


@pytest.mark.parametrize(
    ("members", "fabricated"),
    [("abc", "a, b, c"), ({"alpha": 1}, "alpha")],
)
def test_a_member_field_that_is_not_a_list_invents_no_members(members, fabricated):
    """Both of these are iterable, so a naive comprehension makes members of them.

    This is the failure that does not crash and so does not announce itself: the
    line reads as a dead cluster whose symbols do not exist. A crash at least
    tells the reader something is wrong with the file.
    """
    out = _render([{"members": members, "size": 3, "confidence": 0.5}])

    assert fabricated not in out


def test_a_count_spelled_as_an_integral_float_is_not_thrown_away():
    """JSON has one number type, and the fallback is the *sample* length.

    Refusing `3.0` would print `1 symbols` for a three-symbol component — an
    under-report of exactly the kind the `size` field exists to prevent.
    """
    out = _render([{"members": ["a"], "size": 3.0, "confidence": 0.5}])

    assert "0.50  3 symbols: a, +2 more" in out, out


# --- The raw pattern that stays raw ---------------------------------------------
#
# Five renderers in `graph_cmd` still join or format a bare `.get()`:
# `graph_trace`'s path, `graph_check_cmd`'s degree and cycle nodes,
# `graph_process`'s steps, `graph_routes`'s handler ids. Driven with the shapes
# that broke `_render_dead_clusters`, each misbehaved the same way — an `int` or
# a `[1]` raised out of the render, a `str` joined into members that do not
# exist. They stay raw because those shapes cannot arrive: everything they
# render is derived from fields `CodeGraph` types, and `load_code_graph` refuses
# the file before a renderer sees it. `graph_affected` reads the kernel, whose
# counters the client refuses unless they are non-negative ints. These pin that
# premise at the loader, at the schema and at the client; loosen `GraphNode.id`
# to `Any` and the schema test fails before a renderer does.


@pytest.mark.parametrize(
    "document",
    [
        {"nodes": "abc"},
        {"nodes": [{"id": 5, "kind": "file"}]},
        {"entry_roots": "abc"},
    ],
)
def test_a_malformed_graph_file_is_refused_at_load_not_rendered(tmp_path, document):
    """The real loader, the real file, the real command: refused, not rendered.

    The message is the missing-graph one, and that is the right advice — a
    rebuild replaces a truncated or hand-edited export.
    """
    from devcouncil.indexing.graph.build import read_code_graph

    graph_file = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    graph_file.parent.mkdir(parents=True)
    graph_file.write_text(json.dumps({"schema_version": 2, **document}))

    assert read_code_graph(tmp_path) is None

    result = runner.invoke(app, ["map", "check", "--project-root", str(tmp_path)])
    assert result.exit_code == 1
    assert "No code graph" in result.output
    assert "abc" not in result.output


@pytest.mark.parametrize(
    ("loc", "document"),
    [
        (("nodes",), {"nodes": "abc"}),
        (("nodes", 0, "id"), {"nodes": [{"id": 5, "kind": "file"}]}),
        (("nodes", 0, "id"), {"nodes": [{"id": ["a"], "kind": "file"}]}),
        (("edges", 0, "source"), {"edges": [{"source": [1], "target": "b", "kind": "calls"}]}),
        (("edges", 0, "target"), {"edges": [{"source": "a", "target": 7, "kind": "calls"}]}),
        (("entry_roots",), {"entry_roots": "abc"}),
        (("entry_roots", 0), {"entry_roots": [1]}),
    ],
)
def test_the_fields_the_raw_renderers_derive_from_are_typed_at_load(loc, document):
    """`GraphNode.id`, the edge endpoints and `entry_roots` refuse the trap shapes.

    These are the fields the five raw renderers derive from. A `str` where a
    list goes is the shape that does not crash — it joins — so it is the one
    that must be refused here rather than tolerated downstream.
    """
    from pydantic import ValidationError

    from devcouncil.indexing.graph.schema import CodeGraph

    with pytest.raises(ValidationError) as caught:
        CodeGraph.model_validate(document)
    assert caught.value.errors()[0]["loc"] == loc


@pytest.mark.parametrize("shown", ["abc", [1], 1.0, True])
def test_a_counter_the_kernel_did_not_type_is_refused_before_it_renders(
    tmp_path, monkeypatch, shown
):
    """`dev map affected` prints `shown N of M` raw; the client makes that safe.

    Driven through the real `DevMapClient` with only the transport replaced:
    `_validate_budgeted_sections` refuses the section, the command reports the
    refusal and exits 3, and no counter line is printed from the bad value.
    """
    import devcouncil.devmap_client as devmap_client
    from devcouncil.devmap_client import DevMapClient

    payload = {
        "tests": {
            "shown": shown,
            "hidden": 0,
            "total": 1,
            "truncated": False,
            "tokens_used": 1,
            "items": [{"path": "t/x.py", "depth": 1}],
        }
    }
    client = DevMapClient(root_dir=tmp_path, autospawn=False)
    monkeypatch.setattr(client, "_request", lambda body, cli_args, timeout=120.0: payload)
    monkeypatch.setattr(devmap_client, "try_connect", lambda root: client)

    result = runner.invoke(app, ["map", "affected", "x", "--project-root", str(tmp_path)])

    assert result.exit_code == 3
    assert "affected is unavailable" in result.output
    assert f"shown {shown} of" not in result.output
