"""Wave-9: build_worker, graph doctor/cypher/explore/corpus/pdg, lease tip-over."""

from __future__ import annotations

from types import SimpleNamespace

import pytest
from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.codeintel.sync.lease import WriterLease
from devcouncil.indexing.graph.build import CompatibilityGraphTooLarge

runner = CliRunner()


def test_build_worker_main_healthy_and_degraded(tmp_path, monkeypatch, capsys):
    from devcouncil.codeintel import build_worker

    graph = SimpleNamespace(meta={})

    def _build(root, changed_paths=None, liveness=True, progress=None):
        if progress:
            progress("extract", 0, 1)
            progress("extract", 1, 1)
        return graph

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.build_code_graph",
        _build,
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.write_code_graph",
        lambda *a, **k: None,
    )
    monkeypatch.setattr(
        "sys.argv",
        [
            "build_worker",
            "--root",
            str(tmp_path),
            "--build-id",
            "b1",
            "--changed-path",
            "a.py",
        ],
    )
    assert build_worker.main() == 0
    out = capsys.readouterr().out
    assert "complete" in out
    assert "healthy" in out or '"compatibility_export":"healthy"' in out

    def _boom(*a, **k):
        raise CompatibilityGraphTooLarge("too big")

    monkeypatch.setattr("devcouncil.indexing.graph.build.write_code_graph", _boom)
    monkeypatch.setattr(
        "sys.argv",
        ["build_worker", "--root", str(tmp_path), "--build-id", "b2", "--no-liveness"],
    )
    assert build_worker.main() == 0
    out2 = capsys.readouterr().out
    assert "degraded" in out2


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

    # Doctor audits the compatibility-export handshake for committed stores;
    # an uninitialized store with installed grammars reports healthy.
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda root: SimpleNamespace(
            status=lambda: {"state": "uninitialized", "schema_version": 1}
        ),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.languages.grammar_status",
        lambda: {
            "ok": True,
            "available_count": 35,
            "required_count": 35,
            "languages": [],
            "action": "",
        },
    )
    ok = runner.invoke(app, ["graph", "doctor", "--json", "--project-root", str(tmp_path)])
    assert ok.exit_code == 0
    human = runner.invoke(app, ["graph", "doctor", "--project-root", str(tmp_path)])
    assert human.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.indexing.graph.cypher.run_cypher",
        lambda root, query: {"ok": True, "rows": [{"n": "a"}]},
    )
    cy = runner.invoke(
        app, ["graph", "cypher", "MATCH (n) RETURN n", "--project-root", str(tmp_path)]
    )
    assert cy.exit_code == 0
    cy_json = runner.invoke(
        app,
        ["graph", "cypher", "MATCH (n) RETURN n", "--json", "--project-root", str(tmp_path)],
    )
    assert cy_json.exit_code == 0
    monkeypatch.setattr(
        "devcouncil.indexing.graph.cypher.run_cypher",
        lambda root, query: {"ok": False, "error": "bad"},
    )
    assert (
        runner.invoke(
            app, ["graph", "cypher", "X", "--project-root", str(tmp_path)]
        ).exit_code
        == 1
    )

    class _Engine:
        def __init__(self, root):
            pass

        def explore(self, query, limit=20):
            return {
                "definitions": [
                    {
                        "id": "a.f",
                        "path": "a.py",
                        "line": 1,
                        "source": "def f():\n  pass",
                        "callers": [],
                        "callees": ["a.g"],
                    }
                ]
            }

        def affected_tests(self, targets):
            return {"tests": ["tests/test_a.py"]}

    monkeypatch.setattr("devcouncil.codeintel.query.CodeIntelQueryEngine", _Engine)
    ex = runner.invoke(app, ["graph", "explore", "f", "--project-root", str(tmp_path)])
    assert ex.exit_code == 0
    aff = runner.invoke(
        app, ["graph", "affected", "a.f", "--project-root", str(tmp_path)]
    )
    assert aff.exit_code == 0
    monkeypatch.setattr(
        "devcouncil.codeintel.query.CodeIntelQueryEngine.affected_tests",
        lambda self, targets: {"tests": []},
    )
    # rebind engine for empty tests
    class _Empty(_Engine):
        def affected_tests(self, targets):
            return {"tests": []}

    monkeypatch.setattr("devcouncil.codeintel.query.CodeIntelQueryEngine", _Empty)
    empty = runner.invoke(
        app, ["graph", "affected", "a.f", "--project-root", str(tmp_path)]
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
        runner.invoke(app, ["graph", "explain", "--project-root", str(tmp_path)]).exit_code
        == 0
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.explain_pdg_taint",
        lambda *a, **k: {"ok": True, "findings": []},
    )
    assert "No taint" in runner.invoke(
        app, ["graph", "explain", "--project-root", str(tmp_path)]
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
            "graph",
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
            "graph",
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
        app, ["graph", "pdg", "build", "--json", "--project-root", str(tmp_path)]
    )
    assert pb.exit_code == 0


def test_graph_status_json_and_hooks_refuse(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda root: SimpleNamespace(
            status=lambda: {"state": "ready", "generation": 1, "node_count": 1, "edge_count": 1}
        ),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda root: SimpleNamespace(
            status=lambda: SimpleNamespace(
                as_dict=lambda: {"state": "idle", "backend": None, "pending": []}
            )
        ),
    )
    js = runner.invoke(app, ["graph", "status", "--json", "--project-root", str(tmp_path)])
    assert js.exit_code == 0

    # no .git
    no_git = runner.invoke(
        app, ["graph", "hooks", "install", "--project-root", str(tmp_path)]
    )
    assert no_git.exit_code == 1

    git = tmp_path / ".git" / "hooks"
    git.mkdir(parents=True)
    existing = git / "post-checkout"
    existing.write_text("#!/bin/sh\necho other\n", encoding="utf-8")
    refuse = runner.invoke(
        app, ["graph", "hooks", "install", "--project-root", str(tmp_path)]
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
