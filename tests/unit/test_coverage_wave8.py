"""Wave-8: graph_cmd + plan helpers toward 90% coverage."""

from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace

from typer.testing import CliRunner

from devcouncil.cli.main import app
from devcouncil.codeintel.build_control import GraphBuildBusy

runner = CliRunner()


def _coord(**kwargs):
    defaults = {
        "reconcile": lambda: ["a.py"],
        "sync_now": lambda paths: True,
        "start": lambda: SimpleNamespace(backend="poll", state="watching"),
        "stop": lambda: None,
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

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(GraphBuildBusy("busy")),
    )
    assert runner.invoke(app, ["graph", "init", "--project-root", str(tmp_path)]).exit_code == 1
    busy_json = runner.invoke(app, ["graph", "init", "--json", "--project-root", str(tmp_path)])
    assert busy_json.exit_code == 1
    assert "graph_writer_busy" in busy_json.output

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: SimpleNamespace(
            degraded=False,
            reason="",
            mode="full",
            generation=3,
            compatibility_export_degraded=False,
        ),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda _r: SimpleNamespace(
            status=lambda: {
                "generation": 3,
                "node_count": 10,
                "edge_count": 20,
                "state": "ready",
            }
        ),
    )
    assert runner.invoke(app, ["graph", "init", "--project-root", str(tmp_path)]).exit_code == 0

    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _r: _coord(),
    )
    st = runner.invoke(app, ["graph", "status", "--project-root", str(tmp_path)])
    assert st.exit_code == 0
    assert "generation" in st.output
    assert "degraded" in st.output


def test_graph_sync_watch_search_ingest(tmp_path, monkeypatch):
    from devcouncil.cli.commands.init import initialize_project

    initialize_project(tmp_path, quiet=True, with_map=False, with_skills=False)
    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _r: _coord(),
    )
    ok = runner.invoke(app, ["graph", "sync", "--project-root", str(tmp_path)])
    assert ok.exit_code == 0
    js = runner.invoke(app, ["graph", "sync", "--json", "--project-root", str(tmp_path)])
    assert js.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _r: _coord(sync_now=lambda paths: False),
    )
    bad = runner.invoke(app, ["graph", "sync", "--project-root", str(tmp_path)])
    assert bad.exit_code == 1

    # watch: interrupt immediately
    import time

    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _r: _coord(),
    )

    def _sleep(_s):
        raise KeyboardInterrupt

    monkeypatch.setattr(time, "sleep", _sleep)
    watch = runner.invoke(app, ["graph", "watch", "--project-root", str(tmp_path)])
    assert watch.exit_code == 0
    assert "Stopped" in watch.output

    class _Engine:
        def __init__(self, root):
            pass

        def search(self, query, limit=50):
            return {
                "matches": [
                    {"path": "a.py", "line": 1, "id": "a.f", "kind": "function"},
                    {"path": "b.py", "id": "b", "label": "b", "score": 0.9},
                ]
            }

    monkeypatch.setattr("devcouncil.codeintel.query.CodeIntelQueryEngine", _Engine)
    search = runner.invoke(app, ["graph", "search", "foo", "--project-root", str(tmp_path)])
    assert search.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.indexing.graph.embeddings.semantic_search",
        lambda *a, **k: {"ok": False},
    )
    sem = runner.invoke(
        app, ["graph", "search", "foo", "--semantic", "--json", "--project-root", str(tmp_path)]
    )
    assert sem.exit_code == 0

    refresh = SimpleNamespace(
        generation=1, mode="full", degraded=False, reason=None
    )
    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: refresh,
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.embeddings.build_embeddings",
        lambda root: 3,
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _r: _coord(),
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.get_codeintel_service",
        lambda _r: SimpleNamespace(load=lambda: SimpleNamespace()),
    )
    ingest = runner.invoke(app, ["graph", "ingest", "--json", "--project-root", str(tmp_path)])
    assert ingest.exit_code == 0

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: (_ for _ in ()).throw(GraphBuildBusy("busy")),
    )
    busy = runner.invoke(app, ["graph", "ingest", "--json", "--project-root", str(tmp_path)])
    assert busy.exit_code == 1

    monkeypatch.setattr(
        "devcouncil.indexing.map_artifacts.refresh_map_artifacts",
        lambda *a, **k: refresh,
    )
    monkeypatch.setattr(
        "devcouncil.codeintel.sync.get_sync_coordinator",
        lambda _r: _coord(sync_now=lambda paths: False),
    )
    path_fail = runner.invoke(
        app, ["graph", "ingest", "a.py", "--project-root", str(tmp_path)]
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
    routes = runner.invoke(app, ["graph", "routes", "--project-root", str(tmp_path)])
    assert routes.exit_code == 0
    empty = runner.invoke(app, ["graph", "routes", "--json", "--project-root", str(tmp_path)])
    assert empty.exit_code == 0

    monkeypatch.setattr(api_routes, "route_map", lambda root, g: {"routes": []})
    assert "No routes" in runner.invoke(
        app, ["graph", "routes", "--project-root", str(tmp_path)]
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
    shape = runner.invoke(app, ["graph", "shape-check", "--project-root", str(tmp_path)])
    assert shape.exit_code == 0

    monkeypatch.setattr(
        api_routes, "shape_check", lambda root, g, route_filter=None: {"mismatches": []}
    )
    assert "No shape" in runner.invoke(
        app, ["graph", "shape-check", "--project-root", str(tmp_path)]
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
    impact = runner.invoke(app, ["graph", "api-impact", "/x", "--project-root", str(tmp_path)])
    assert impact.exit_code == 0

    monkeypatch.setattr(
        api_routes,
        "api_impact",
        lambda root, route, g: {"found": False},
    )
    assert (
        runner.invoke(
            app, ["graph", "api-impact", "/missing", "--project-root", str(tmp_path)]
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
    demo = runner.invoke(app, ["graph", "demo", "--json", "--project-root", str(tmp_path)])
    assert demo.exit_code == 0
    assert "html" in json.loads(demo.stdout)

    # hooks install
    git = tmp_path / ".git" / "hooks"
    git.mkdir(parents=True)
    hooks = runner.invoke(
        app, ["graph", "hooks", "install", "--project-root", str(tmp_path)]
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
