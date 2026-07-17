import json
from io import BytesIO
from types import SimpleNamespace

from devcouncil.ui import dashboard as dashboard_module


def test_load_run_manifest_handles_missing_invalid_and_cached_copy(tmp_path):
    dashboard_module._RUN_MANIFEST_CACHE.clear()
    missing = tmp_path / "missing.json"
    assert dashboard_module._load_run_manifest(missing) is None

    manifest = tmp_path / "agent-run.json"
    manifest.write_text("{bad json", encoding="utf-8")
    assert dashboard_module._load_run_manifest(manifest) is None

    manifest.write_text('{"run_id": "run-1"}', encoding="utf-8")
    loaded = dashboard_module._load_run_manifest(manifest)
    assert loaded == {"run_id": "run-1"}
    loaded["run_id"] = "mutated"
    assert dashboard_module._load_run_manifest(manifest) == {"run_id": "run-1"}


def test_recent_runs_cache_limit_and_invalidation(tmp_path, monkeypatch):
    dashboard_module._RECENT_RUNS_CACHE.clear()
    monkeypatch.setattr(dashboard_module.time, "monotonic", lambda: 100.0)

    assert dashboard_module.recent_run_artifacts(tmp_path) == []
    assert str(tmp_path) in dashboard_module._RECENT_RUNS_CACHE

    dashboard_module._invalidate_recent_runs(tmp_path)
    runs_dir = tmp_path / ".devcouncil" / "runs"
    for run_id in ("old", "new", "bad"):
        (runs_dir / run_id).mkdir(parents=True)
    (runs_dir / "old" / "agent-run.json").write_text('{"run_id": "old"}', encoding="utf-8")
    (runs_dir / "new" / "agent-run.json").write_text('{"run_id": "new"}', encoding="utf-8")
    (runs_dir / "bad" / "agent-run.json").write_text("{bad", encoding="utf-8")

    artifacts = dashboard_module.recent_run_artifacts(tmp_path, limit=1)
    assert len(artifacts) == 1
    assert artifacts[0]["manifest_path"].endswith("agent-run.json")

    cached = dashboard_module.recent_run_artifacts(tmp_path, limit=1)
    assert cached is artifacts


def test_integration_and_artifact_graph_caches(tmp_path, monkeypatch):
    dashboard_module._INTEGRATION_SUMMARY_CACHE.clear()
    dashboard_module._ARTIFACT_GRAPH_CACHE.clear()
    now = {"value": 1.0}
    monkeypatch.setattr(dashboard_module.time, "monotonic", lambda: now["value"])

    calls = {"integrations": 0, "graph": 0}

    def fake_summary(project_root):
        calls["integrations"] += 1
        return {"call": calls["integrations"]}

    monkeypatch.setattr(dashboard_module, "integration_status_summary", fake_summary)
    assert dashboard_module._integration_summary_cached(tmp_path) == {"call": 1}
    assert dashboard_module._integration_summary_cached(tmp_path) == {"call": 1}
    dashboard_module._invalidate_integration_summary(tmp_path)
    assert dashboard_module._integration_summary_cached(tmp_path) == {"call": 2}

    graph = object()

    class FakeRepo:
        def __init__(self, session):
            self.session = session

        def load_graph(self):
            calls["graph"] += 1
            return graph

    monkeypatch.setattr(dashboard_module, "ArtifactGraphRepository", FakeRepo)
    assert dashboard_module._artifact_graph_cached(tmp_path, object()) is graph
    assert dashboard_module._artifact_graph_cached(tmp_path, object()) is graph
    assert calls["graph"] == 1
    dashboard_module._invalidate_artifact_graph(tmp_path)
    assert dashboard_module._artifact_graph_cached(tmp_path, object()) is graph
    assert calls["graph"] == 2


def test_recent_trace_events_cache_appends_and_resets_on_truncation(tmp_path, monkeypatch):
    dashboard_module._TRACE_EVENTS_CACHE.clear()
    first = SimpleNamespace(model_dump=lambda **kwargs: {"id": "first"})
    second = SimpleNamespace(model_dump=lambda **kwargs: {"id": "second"})
    responses = [([first], 10), ([second], 5)]

    monkeypatch.setattr(dashboard_module, "read_trace_events_since", lambda root, cursor: responses.pop(0))

    assert dashboard_module._recent_trace_events_cached(tmp_path) == [first]
    assert dashboard_module._recent_trace_events_cached(tmp_path) == [second]


def test_dashboard_payload_serializes_initialized_graph(monkeypatch, tmp_path):
    dashboard_module._ARTIFACT_GRAPH_CACHE.clear()

    graph = SimpleNamespace(
        tasks={
            "T": SimpleNamespace(model_dump=lambda: {"id": "T", "status": "done", "title": "Task"}),
        },
        coverage_summary=lambda: {"requirements": 1},
    )
    state = SimpleNamespace(current_phase="TASK_VERIFYING")
    session = object()
    db = SimpleNamespace(get_session=lambda: _Context(session))

    monkeypatch.setattr(dashboard_module, "get_db", lambda root: db)
    monkeypatch.setattr(dashboard_module, "_artifact_graph_cached", lambda root, session_arg: graph)
    monkeypatch.setattr(dashboard_module, "StateRepository", lambda session_arg: SimpleNamespace(get_state=lambda: state))
    monkeypatch.setattr(dashboard_module, "compute_phase", lambda graph_arg, phase: f"computed:{phase}")
    monkeypatch.setattr(dashboard_module, "_recent_trace_events_cached", lambda root: [SimpleNamespace(model_dump=lambda by_alias=False: {"event": "x"})])
    monkeypatch.setattr(dashboard_module, "_integration_summary_cached", lambda root: {"integrated": True})
    monkeypatch.setattr(dashboard_module, "recent_run_artifacts", lambda root: [{"run_id": "R"}])

    payload = dashboard_module.dashboard_payload(tmp_path)

    assert payload["initialized"] is True
    assert payload["phase"] == "computed:TASK_VERIFYING"
    assert payload["tasks"] == [{"id": "T", "status": "done", "title": "Task"}]
    assert payload["events"] == [{"event": "x"}]


class _Context:
    def __init__(self, value):
        self.value = value

    def __enter__(self):
        return self.value

    def __exit__(self, exc_type, exc, tb):
        return False


def test_dashboard_apply_payload_rejects_bad_bodies_and_value_errors(tmp_path, monkeypatch):
    assert dashboard_module.dashboard_apply_payload(tmp_path, b"{}", token="secret", provided_token="bad") == {
        "ok": False,
        "error": "invalid dashboard token",
    }
    assert dashboard_module.dashboard_apply_payload(tmp_path, b"{bad", token="secret", provided_token="secret") == {
        "ok": False,
        "error": "invalid JSON body",
    }
    assert dashboard_module.dashboard_apply_payload(tmp_path, b"[]", token="secret", provided_token="secret") == {
        "ok": False,
        "error": "request body must be a JSON object",
    }

    def fake_apply(*args, **kwargs):
        raise ValueError("unknown target")

    monkeypatch.setattr(dashboard_module, "apply_integration_target", fake_apply)
    assert dashboard_module.dashboard_apply_payload(
        tmp_path,
        b'{"target": "bad", "strict": true}',
        token="secret",
        provided_token="secret",
    ) == {"ok": False, "error": "unknown target"}


def test_dashboard_get_handlers_cover_assets_apis_html_and_not_found(tmp_path, monkeypatch):
    handler_class = _capture_handler(monkeypatch, tmp_path)
    monkeypatch.setattr(dashboard_module, "logo_asset_bytes", lambda: b"png")
    monkeypatch.setattr(dashboard_module, "logo_svg", lambda: "<svg></svg>")
    monkeypatch.setattr(
        dashboard_module,
        "build_integration_check_report",
        lambda root: SimpleNamespace(as_dict=lambda: {"ok": True}),
    )
    monkeypatch.setattr(dashboard_module, "dashboard_payload", lambda root: {"phase": "READY"})

    png = _FakeHandler(handler_class, f"/assets/{dashboard_module.LOGO_ASSET}")
    png.do_GET()
    assert png.status == 200
    assert png.sent_headers["Content-Type"] == "image/png"
    assert png.wfile.getvalue() == b"png"

    svg = _FakeHandler(handler_class, f"/assets/{dashboard_module.LEGACY_LOGO_ASSET}")
    svg.do_GET()
    assert svg.status == 200
    assert "image/svg+xml" in svg.sent_headers["Content-Type"]

    check = _FakeHandler(handler_class, "/api/integrations/check")
    check.do_GET()
    assert json.loads(check.wfile.getvalue()) == {"ok": True}

    status = _FakeHandler(handler_class, "/api/status")
    status.do_GET()
    assert json.loads(status.wfile.getvalue()) == {"phase": "READY"}

    missing = _FakeHandler(handler_class, "/api/missing")
    missing.do_GET()
    assert missing.status == 404

    html = _FakeHandler(handler_class, "/")
    html.do_GET()
    assert html.status == 200
    assert b"DevCouncil Dashboard" in html.wfile.getvalue()


def test_dashboard_post_handlers_cover_not_found_loopback_and_success(tmp_path, monkeypatch):
    handler_class = _capture_handler(monkeypatch, tmp_path)
    not_found = _FakeHandler(handler_class, "/api/else", method="POST")
    not_found.do_POST()
    assert not_found.status == 404

    remote = _FakeHandler(
        handler_class,
        "/api/integrations/apply",
        method="POST",
        client_address=("10.0.0.1", 4444),
    )
    remote.do_POST()
    assert remote.status == 403
    assert b"loopback" in remote.wfile.getvalue()

    monkeypatch.setattr(
        dashboard_module,
        "dashboard_apply_payload",
        lambda root, raw, token, provided_token: {"ok": True, "raw": raw.decode("utf-8"), "token": bool(token), "provided": provided_token},
    )
    ok = _FakeHandler(
        handler_class,
        "/api/integrations/apply",
        method="POST",
        body=b'{"target":"all"}',
        headers={"Content-Length": "16", "X-DevCouncil-Dashboard-Token": "provided"},
    )
    ok.do_POST()
    assert ok.status == 200
    assert json.loads(ok.wfile.getvalue())["provided"] == "provided"


def _capture_handler(monkeypatch, project_root):
    captured = {}

    class FakeServer:
        def __init__(self, address, handler):
            captured["handler"] = handler

        def serve_forever(self):
            return None

    monkeypatch.setattr(dashboard_module, "ThreadingHTTPServer", FakeServer)
    dashboard_module.run_dashboard(project_root, host="127.0.0.1", port=9999)
    return captured["handler"]


class _FakeHandler:
    def __init__(
        self,
        handler_class,
        path,
        *,
        method="GET",
        body=b"",
        headers=None,
        client_address=("127.0.0.1", 12345),
    ):
        self.__class__ = type("FakeDashboardHandler", (_FakeHandler, handler_class), {})
        self.path = path
        self.command = method
        self.headers = headers or {"Content-Length": str(len(body))}
        self.rfile = BytesIO(body)
        self.wfile = BytesIO()
        self.client_address = client_address
        self.status = None
        self.sent_headers = {}

    def send_response(self, code):
        self.status = code

    def send_header(self, key, value):
        self.sent_headers[key] = value

    def end_headers(self):
        return None
