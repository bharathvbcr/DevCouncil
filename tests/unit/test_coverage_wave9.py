"""Wave-9: graph doctor/cypher/explore/corpus/pdg, lease tip-over."""

from __future__ import annotations

from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.codeintel.sync.lease import WriterLease
from devcouncil.indexing.graph.build import CompatibilityGraphTooLarge

runner = CliRunner()


def test_writer_lease_busy_and_context(tmp_path, monkeypatch):
    path = tmp_path / "lease.lock"
    first = WriterLease(path)
    assert first.acquire() is True
    second = WriterLease(path)
    assert second.acquire() is False
    first.release()
    first.release()  # idempotent

    with WriterLease(path) as held:
        assert held._handle is not None
    with pytest.raises(BlockingIOError):
        lease = WriterLease(path)
        monkeypatch.setattr(lease, "acquire", lambda: False)
        with lease:
            pass

    # Bounded retry eventually succeeds when the holder releases mid-wait.
    holder = WriterLease(path)
    assert holder.acquire()
    waits: list[float] = []

    def release_on_sleep(seconds: float) -> None:
        waits.append(seconds)
        if len(waits) == 1:
            holder.release()

    contender = WriterLease(path)
    assert contender.acquire_with_retry(
        timeout=1.0, initial_delay=0.01, max_delay=0.05, sleep=release_on_sleep
    )
    contender.release()


def test_graph_doctor_cypher_explore_affected_corpus(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)

    # Doctor audits the kernel: a usable binary with no store yet is healthy.
    monkeypatch.setattr(
        "devcouncil.devmap_health.engine_info",
        lambda root: {
            "binary": "/opt/devmap",
            "built_at": "2026-09-02T16:51:00",
            "version": "devmap 0.1.0 (schema 12)",
            "schema_version": 12,
            "error": None,
        },
    )
    ok = runner.invoke(app, ["map", "doctor", "--json", "--project-root", str(tmp_path)])
    assert ok.exit_code == 0
    human = runner.invoke(app, ["map", "doctor", "--project-root", str(tmp_path)])
    assert human.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.indexing.graph.cypher.run_cypher",
        lambda root, query: {"ok": True, "rows": [{"n": "a"}]},
    )
    cy = runner.invoke(
        app, ["map", "cypher", "MATCH (n) RETURN n", "--project-root", str(tmp_path)]
    )
    assert cy.exit_code == 0
    cy_json = runner.invoke(
        app,
        ["map", "cypher", "MATCH (n) RETURN n", "--json", "--project-root", str(tmp_path)],
    )
    assert cy_json.exit_code == 0
    monkeypatch.setattr(
        "devcouncil.indexing.graph.cypher.run_cypher",
        lambda root, query: {"ok": False, "error": "bad"},
    )
    assert (
        runner.invoke(
            app, ["map", "cypher", "X", "--project-root", str(tmp_path)]
        ).exit_code
        == 1
    )

    monkeypatch.setattr(
        "devcouncil.devmap_client.try_connect", lambda _root: _KernelStub()
    )
    ex = runner.invoke(app, ["map", "explore", "f", "--project-root", str(tmp_path)])
    assert ex.exit_code == 0
    aff = runner.invoke(
        app, ["map", "affected", "a.f", "--project-root", str(tmp_path)]
    )
    assert aff.exit_code == 0
    monkeypatch.setattr(
        "devcouncil.devmap_client.try_connect", lambda _root: _KernelStub(tests=())
    )
    empty = runner.invoke(
        app, ["map", "affected", "a.f", "--project-root", str(tmp_path)]
    )
    assert "No affected" in empty.output

    monkeypatch.setattr(
        "devcouncil.indexing.wiring.build_corpus", lambda root, path=None: None
    )
    monkeypatch.setattr(
        "devcouncil.indexing.wiring.corpus_status",
        lambda root: {
            "enabled": True,
            "node_count": 2,
            "edge_count": 1,
            "graph_path": "g.json",
            "built_at": "now",
        },
    )
    cb = runner.invoke(app, ["corpus", "build", "--project-root", str(tmp_path)])
    assert cb.exit_code == 0
    cs = runner.invoke(
        app, ["corpus", "status", "--json", "--project-root", str(tmp_path)]
    )
    assert cs.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.indexing.wiring.query_corpus",
        lambda root, query, limit=20: {
            "matches": [{"label": "Doc", "kind": "doc", "path": "d.md", "score": 1.0}]
        },
    )
    cq = runner.invoke(
        app, ["corpus", "query", "Doc", "--project-root", str(tmp_path)]
    )
    assert cq.exit_code == 0
    monkeypatch.setattr(
        "devcouncil.indexing.wiring.query_corpus",
        lambda root, query, limit=20: {"matches": []},
    )
    assert "No matches" in runner.invoke(
        app, ["corpus", "query", "x", "--project-root", str(tmp_path)]
    ).output


def test_graph_explain_pdg_query(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project
    import devcouncil.indexing.graph.build as graph_build

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    monkeypatch.setattr(
        graph_build, "load_code_graph", lambda root: SimpleNamespace(meta={}, dead_code=[])
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.explain_pdg_taint",
        lambda root, path=None, category=None: {
            "ok": True,
            "findings": [
                {
                    "path": "a.py",
                    "sink_line": 3,
                    "category": "sql",
                    "function": "f",
                    "source_expr": "x",
                    "sink_expr": "y",
                }
            ],
        },
    )
    assert (
        runner.invoke(app, ["map", "explain", "--project-root", str(tmp_path)]).exit_code
        == 0
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.explain_pdg_taint",
        lambda *a, **k: {"ok": True, "findings": []},
    )
    assert "No taint" in runner.invoke(
        app, ["map", "explain", "--project-root", str(tmp_path)]
    ).output

    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.query_pdg_controls",
        lambda root, target: {
            "ok": True,
            "functions": [{"qualname": "f", "path": "a.py", "cdg": ["1->2"]}],
        },
    )
    pq = runner.invoke(
        app,
        [
            "map",
            "pdg-query",
            "--mode",
            "controls",
            "--target",
            "f",
            "--project-root",
            str(tmp_path),
        ],
    )
    assert pq.exit_code == 0
    bad = runner.invoke(
        app,
        [
            "map",
            "pdg-query",
            "--mode",
            "nope",
            "--target",
            "f",
            "--project-root",
            str(tmp_path),
        ],
    )
    assert bad.exit_code == 2

    # pdg build
    layer = SimpleNamespace(files={"a.py": {}})
    monkeypatch.setattr(
        graph_build, "build_pdg_for_paths", lambda root, graph, paths=None: layer
    )
    monkeypatch.setattr(
        graph_build, "merge_pdg_into_graph", lambda graph, layer: {"a.py": {"x": 1}}
    )
    monkeypatch.setattr(graph_build, "write_code_graph", lambda *a, **k: None)
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda root: SimpleNamespace(store=SimpleNamespace(analysis_shards=lambda: {})),
    )
    graph = SimpleNamespace(meta={"pdg": {"stats": {"function_count": 1, "taint_count": 0, "file_count": 1}}})
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: graph)
    pb = runner.invoke(
        app, ["map", "pdg", "build", "--json", "--project-root", str(tmp_path)]
    )
    assert pb.exit_code == 0


def test_graph_pdg_build_survives_oversized_compatibility_export(tmp_path, monkeypatch):
    """Stub-tier export raises after a successful write; `dev graph pdg` must
    report degraded-export success, not crash (SQLite already committed)."""
    import json

    from devcouncil.indexing.graph import build as graph_build

    layer = SimpleNamespace(files={"a.py": {}})
    monkeypatch.setattr(
        graph_build, "build_pdg_for_paths", lambda root, graph, paths=None: layer
    )
    monkeypatch.setattr(
        graph_build, "merge_pdg_into_graph", lambda graph, layer: {"a.py": {"x": 1}}
    )

    def _too_large(*_a, **_k):
        raise CompatibilityGraphTooLarge("exceeded cap; wrote stub JSON")

    monkeypatch.setattr(graph_build, "write_code_graph", _too_large)
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda root: SimpleNamespace(store=SimpleNamespace(analysis_shards=lambda: {})),
    )
    graph = SimpleNamespace(
        meta={"pdg": {"stats": {"function_count": 1, "taint_count": 0, "file_count": 1}}}
    )
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: graph)
    result = runner.invoke(
        app, ["map", "pdg", "build", "--json", "--project-root", str(tmp_path)]
    )
    assert result.exit_code == 0
    payload = json.loads(result.stdout)
    assert payload["ok"] is True
    assert payload["compatibility_export"] == "degraded"
    assert "stub" in payload["compatibility_export_reason"]


def test_graph_status_json_and_hooks_refuse(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda root: SimpleNamespace(
            status=lambda: {"state": "ready", "generation": 1, "node_count": 1, "edge_count": 1}
        ),
    )
    js = runner.invoke(app, ["map", "status", "--json", "--project-root", str(tmp_path)])
    assert js.exit_code == 0

    # no .git
    no_git = runner.invoke(
        app, ["map", "hooks", "install", "--project-root", str(tmp_path)]
    )
    assert no_git.exit_code == 1

    git = tmp_path / ".git" / "hooks"
    git.mkdir(parents=True)
    existing = git / "post-checkout"
    existing.write_text("#!/bin/sh\necho other\n", encoding="utf-8")
    refuse = runner.invoke(
        app, ["map", "hooks", "install", "--project-root", str(tmp_path)]
    )
    assert refuse.exit_code == 1


def _repo_map(**kwargs):
    from devcouncil.indexing.repo_mapper import RepoMap

    base = dict(
        languages=[],
        frameworks=[],
        package_managers=[],
        test_commands=[],
        important_files=[],
        candidate_files=[],
    )
    base.update(kwargs)
    return RepoMap(**base)


def test_map_liveness_summary_and_if_stale(tmp_path, monkeypatch):
    from devcouncil.cli.commands import map as map_cmd

    empty = _repo_map()
    assert map_cmd._liveness_summary(empty) is None

    rich = _repo_map(
        entry_roots=["src"],
        unwired_candidates=["a.py", "b.py", "c.py", "d.py"],
        unreachable_files=["u.py"],
        dead_symbol_candidates=["x.f", "y.g"],
    )
    summary = map_cmd._liveness_summary(rich)
    assert summary is not None
    assert "entry roots" in summary

    unreliable = _repo_map(
        entry_roots=[],
        unreachable_files=["u.py"],
        liveness_unreachable_unreliable=True,
    )
    assert map_cmd._liveness_summary(unreliable) is not None


def _kernel_response(items):
    return {
        "items": items,
        "shown": len(items),
        "hidden": 0,
        "total": len(items),
        "truncated": False,
        "tokens_used": 0,
        "resolution": "Available",
    }


class _KernelStub:
    """The kernel seam the CLI query commands now go through.

    `dev map search|explore|affected` used to be stubbed by replacing
    `CodeIntelQueryEngine`; that class was deleted with the rest of the Python
    query surface. The payloads here are the kernel's own wire shapes — nested
    `Response` objects carrying their counters — because that is what the
    commands read.
    """

    def __init__(self, tests=("tests/test_a.py",)):
        self._tests = list(tests)

    def search(self, query, limit=2000, semantic=False):
        from devcouncil.devmap_client import BudgetedResponse

        items = [{"symbol_name": query, "file_path": "a.py", "kind": "Function",
                  "span": [1, 1], "score": 0.9}]
        return BudgetedResponse(
            shown=1, hidden=0, total=1, truncated=False, tokens_used=0,
            items=items, resolution="Available",
        )

    def explore(self, query, limit=20, **_kwargs):
        return {
            "query": query,
            "limit": limit,
            "definitions": _kernel_response([{
                "id": "a.py::f",
                "symbol_name": "f",
                "qualified_name": "f",
                "file_path": "a.py",
                "kind": "Function",
                "span": [1, 2],
                "source": "def f():\n  pass",
                "score": 1.0,
                "callers": _kernel_response([]),
                "callees": _kernel_response([{"source_symbol": "a.py::g"}]),
            }]),
            "blast_radius": {"seeds": [], "unmatched_targets": [],
                             "layers": _kernel_response([]), "total_impacted": 0},
            "budget": {"total": 8000, "definitions": 4000,
                       "edges_per_direction": 500, "blast_radius": 2000},
        }

    def affected_tests(self, targets, **_kwargs):
        rows = [{"path": path, "depth": 1, "symbols": [], "reached_symbols": 0}
                for path in self._tests]
        return {
            "targets": list(targets),
            "tests": _kernel_response(rows),
            "blast_radius": {"seeds": [], "unmatched_targets": [],
                             "layers": _kernel_response([]), "total_impacted": 0},
        }

