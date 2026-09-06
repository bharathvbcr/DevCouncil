"""Wave-8: graph_cmd + plan helpers toward 90% coverage."""

from __future__ import annotations

import json
from types import SimpleNamespace

from typer.testing import CliRunner

from devcouncil.cli.main import app

runner = CliRunner()


def _coord(**kwargs):
    defaults = {
        "reconcile": lambda: ["a.py"],
        "sync_now": lambda paths: True,
        "start": lambda: SimpleNamespace(backend="poll", state="watching"),
        "stop": lambda **_k: None,
        "status": lambda: SimpleNamespace(
            as_dict=lambda: {
                "state": "idle",
                "backend": "poll",
                "build_id": "b1",
                "build_completed": 1,
                "build_total": 2,
                "build_state": "building",
                "build_phase": "extract",
                "build_pid": 9,
                "compatibility_export": "degraded",
                "pending": ["x.py"],
                "degraded_reason": "slow",
                "last_error": None,
            }
        ),
    }
    defaults.update(kwargs)
    return SimpleNamespace(**defaults)


def test_graph_init_busy_and_status_text(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)

    from devcouncil.devmap_engine import DevMapEngineError

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(DevMapEngineError("kernel unavailable")),
    )
    assert runner.invoke(app, ["map", "init", "--project-root", str(tmp_path)]).exit_code == 1
    down_json = runner.invoke(app, ["map", "init", "--json", "--project-root", str(tmp_path)])
    assert down_json.exit_code == 1
    assert "engine_unavailable" in down_json.output

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: SimpleNamespace(
            degraded=False,
            reason="",
            mode="devmap-rust",
            generation=3,
            kernel_status=None,
        ),
    )
    assert runner.invoke(app, ["map", "init", "--project-root", str(tmp_path)]).exit_code == 0

    monkeypatch.setattr(
        "devcouncil.devmap_health.kernel_status",
        lambda root: {
            "generation_id": 3,
            "pending_count": 1,
            "quarantined_count": 1,
            "node_count": 10,
            "edge_count": 20,
            "is_fresh": False,
            "degraded_reason": "slow",
        },
    )
    st = runner.invoke(app, ["map", "status", "--project-root", str(tmp_path)])
    assert st.exit_code == 0
    assert "generation" in st.output
    assert "degraded" in st.output


def test_graph_sync_watch_search_ingest(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    from devcouncil.devmap_engine import DevMapEngineError

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    healthy = SimpleNamespace(
        generation=2, mode="devmap-rust", degraded=False, reason="", kernel_status=None
    )
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: healthy,
    )
    ok = runner.invoke(app, ["map", "sync", "--project-root", str(tmp_path)])
    assert ok.exit_code == 0
    js = runner.invoke(app, ["map", "sync", "--json", "--project-root", str(tmp_path)])
    assert js.exit_code == 0
    assert json.loads(js.stdout)["generation"] == 2

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(DevMapEngineError("kernel unavailable")),
    )
    bad = runner.invoke(app, ["map", "sync", "--project-root", str(tmp_path)])
    assert bad.exit_code == 1

    # watch: interrupt at the first wait — the loop is event-driven now, so
    # `time.sleep` is not the seam; the wait is.
    def _interrupt(_changed, _timeout):
        raise KeyboardInterrupt

    monkeypatch.setattr("devcouncil.cli.commands.map._wait_for_change", _interrupt)
    watch = runner.invoke(app, ["map", "watch", "--project-root", str(tmp_path)])
    assert watch.exit_code == 0
    assert "Stopped" in watch.output

    monkeypatch.setattr(
        "devcouncil.devmap_client.try_connect", lambda _root: _KernelStub()
    )
    search = runner.invoke(app, ["map", "search", "foo", "--project-root", str(tmp_path)])
    assert search.exit_code == 0

    # `--semantic` is answered by the kernel now, so with no devmap index in
    # this fixture it reports unavailable rather than silently downgrading to
    # prefix matching and calling the result semantic. The stub is withdrawn
    # first: "no index" is the condition under test, and a stub that answers is
    # not that condition.
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda _root: None)
    sem = runner.invoke(
        app, ["map", "search", "foo", "--semantic", "--json", "--project-root", str(tmp_path)]
    )
    assert sem.exit_code == 3

    refresh = SimpleNamespace(
        generation=1,
        mode="devmap-rust",
        degraded=False,
        reason="",
        kernel_status=None,
    )
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: refresh,
    )
    ingest = runner.invoke(app, ["map", "ingest", "--json", "--project-root", str(tmp_path)])
    assert ingest.exit_code == 0
    assert json.loads(ingest.stdout)["mode"] == "devmap-rust"

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(DevMapEngineError("writer lock held")),
    )
    busy = runner.invoke(app, ["map", "ingest", "--json", "--project-root", str(tmp_path)])
    assert busy.exit_code == 1
    assert "engine_unavailable" in busy.output

    path_fail = runner.invoke(
        app, ["map", "ingest", "a.py", "--project-root", str(tmp_path)]
    )
    assert path_fail.exit_code == 1


def test_graph_routes_shape_api_demo_hooks(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project
    import devcouncil.indexing.graph.build as graph_build
    import devcouncil.indexing.graph.api_routes as api_routes
    import devcouncil.indexing.viz as viz

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    graph = SimpleNamespace(dead_code=[], edges=[])
    monkeypatch.setattr(graph_build, "load_code_graph", lambda root: graph)

    monkeypatch.setattr(
        api_routes,
        "route_map",
        lambda root, g: {
            "routes": [
                {
                    "verb": "GET",
                    "path": "/x",
                    "framework": "fastapi",
                    "handlers": [{"id": "h"}],
                    "consumers": [{"id": "c"}],
                }
            ]
        },
    )
    routes = runner.invoke(app, ["map", "routes", "--project-root", str(tmp_path)])
    assert routes.exit_code == 0
    empty = runner.invoke(app, ["map", "routes", "--json", "--project-root", str(tmp_path)])
    assert empty.exit_code == 0

    monkeypatch.setattr(api_routes, "route_map", lambda root, g: {"routes": []})
    assert "No routes" in runner.invoke(
        app, ["map", "routes", "--project-root", str(tmp_path)]
    ).output

    monkeypatch.setattr(
        api_routes,
        "shape_check",
        lambda root, g, route_filter=None: {
            "mismatches": [
                {
                    "verb": "GET",
                    "route": "/x",
                    "missing_in_handler": ["id"],
                }
            ]
        },
    )
    shape = runner.invoke(app, ["map", "shape-check", "--project-root", str(tmp_path)])
    assert shape.exit_code == 0

    monkeypatch.setattr(
        api_routes, "shape_check", lambda root, g, route_filter=None: {"mismatches": []}
    )
    assert "No shape" in runner.invoke(
        app, ["map", "shape-check", "--project-root", str(tmp_path)]
    ).output

    monkeypatch.setattr(
        api_routes,
        "api_impact",
        lambda root, route, g: {
            "found": True,
            "verb": "GET",
            "route": "/x",
            "risk": "low",
            "consumers": [1],
            "middleware": [],
            "shape_mismatches": [{}],
        },
    )
    impact = runner.invoke(app, ["map", "api-impact", "/x", "--project-root", str(tmp_path)])
    assert impact.exit_code == 0

    monkeypatch.setattr(
        api_routes,
        "api_impact",
        lambda root, route, g: {"found": False},
    )
    assert (
        runner.invoke(
            app, ["map", "api-impact", "/missing", "--project-root", str(tmp_path)]
        ).exit_code
        == 1
    )

    monkeypatch.setattr(
        viz,
        "write_graph_demo",
        lambda root, open_browser=False: {
            "html": tmp_path / "d.html",
            "svg": tmp_path / "d.svg",
        },
    )
    demo = runner.invoke(app, ["map", "demo", "--json", "--project-root", str(tmp_path)])
    assert demo.exit_code == 0
    assert "html" in json.loads(demo.stdout)

    # hooks install
    git = tmp_path / ".git" / "hooks"
    git.mkdir(parents=True)
    hooks = runner.invoke(
        app, ["map", "hooks", "install", "--project-root", str(tmp_path)]
    )
    assert hooks.exit_code == 0


def test_plan_helper_branches(tmp_path, monkeypatch):
    from devcouncil.app.config import ModelRoleConfig
    from devcouncil.cli.commands import plan as plan_cmd

    assert plan_cmd._decision_ids(["a", {"id": "b"}, {"finding_id": "c"}, 3]) == {
        "a",
        "b",
        "c",
    }
    assert plan_cmd._decision_ids([]) == set()

    class _Finding:
        def __init__(self, fid: str, status: str = "open"):
            self.id = fid
            self.status = status

        def model_copy(self, *, update):
            return _Finding(self.id, update.get("status", self.status))

    findings = [_Finding("1"), _Finding("2"), _Finding("3")]
    decision = SimpleNamespace(
        accepted_finding_ids=["1"],
        rejected_finding_ids=["2", {"id": "3"}],
    )
    out = plan_cmd._reconcile_findings(findings, decision)
    assert {f.id: f.status for f in out} == {
        "1": "converted",
        "2": "rejected",
        "3": "rejected",
    }

    cfg = SimpleNamespace(
        models=SimpleNamespace(provider="openai", roles={}),
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=True),
    )
    monkeypatch.setattr(
        "devcouncil.cli.commands.plan.build_role_model_config",
        lambda provider: {"spec_writer": {"model": "m"}},
    )
    plan_cmd._ensure_planning_roles(cfg)
    assert "spec_writer" in cfg.models.roles or len(cfg.models.roles) >= 1

    # Existing fallback when roles already populated
    cfg2 = SimpleNamespace(
        models=SimpleNamespace(
            provider="openai",
            roles={"spec_writer": ModelRoleConfig(model="x")},
        ),
        planning=SimpleNamespace(auto_convert_blocking_questions_in_noninteractive=False),
    )
    plan_cmd._ensure_planning_roles(cfg2)

    monkeypatch.setattr("devcouncil.cli.commands.plan.sys.stdin.isatty", lambda: False)
    assert plan_cmd._should_auto_convert_blocking_questions(cfg) is True
    monkeypatch.setattr("devcouncil.cli.commands.plan.sys.stdin.isatty", lambda: True)
    assert plan_cmd._should_auto_convert_blocking_questions(cfg) is False
    assert plan_cmd._should_auto_convert_blocking_questions(cfg2) is False


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

