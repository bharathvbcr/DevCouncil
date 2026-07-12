"""Unit coverage for the dashboard payload/caching/apply helpers.

Targets the pure helpers in ``devcouncil.ui.dashboard`` — run-manifest loading and
caching, recent-run assembly, integration-summary caching, the apply-endpoint token
and validation branches, and HTML/logo rendering — without standing up the HTTP
server.
"""

from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

import devcouncil.ui.dashboard as dash


def _write_manifest(root, run_id, data):
    run_dir = root / ".devcouncil" / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)
    (run_dir / "agent-run.json").write_text(json.dumps(data), encoding="utf-8")
    return run_dir / "agent-run.json"


@pytest.fixture(autouse=True)
def _clear_caches():
    dash._RUN_MANIFEST_CACHE.clear()
    dash._RECENT_RUNS_CACHE.clear()
    dash._INTEGRATION_SUMMARY_CACHE.clear()
    dash._ARTIFACT_GRAPH_CACHE.clear()
    dash._TRACE_EVENTS_CACHE.clear()
    yield


def test_load_run_manifest_missing_returns_none(tmp_path):
    assert dash._load_run_manifest(tmp_path / "nope.json") is None


def test_load_run_manifest_reads_and_caches(tmp_path):
    path = _write_manifest(tmp_path, "run1", {"run_id": "run1", "status": "finished"})
    first = dash._load_run_manifest(path)
    assert first["run_id"] == "run1"
    # Second read is served from the mtime cache (same content).
    assert dash._load_run_manifest(path) == first
    assert str(path) in dash._RUN_MANIFEST_CACHE


def test_load_run_manifest_invalid_json_returns_none(tmp_path):
    run_dir = tmp_path / ".devcouncil" / "runs" / "bad"
    run_dir.mkdir(parents=True)
    path = run_dir / "agent-run.json"
    path.write_text("{not json", encoding="utf-8")
    assert dash._load_run_manifest(path) is None


def test_recent_run_artifacts_empty(tmp_path):
    assert dash.recent_run_artifacts(tmp_path) == []


def test_recent_run_artifacts_orders_and_limits(tmp_path):
    for i in range(3):
        _write_manifest(tmp_path, f"run{i}", {"run_id": f"run{i}", "status": "finished"})
    runs = dash.recent_run_artifacts(tmp_path, limit=2)
    assert len(runs) == 2
    assert all("manifest_path" in r for r in runs)


def test_recent_run_artifacts_uses_ttl_cache(tmp_path):
    _write_manifest(tmp_path, "run0", {"run_id": "run0"})
    first = dash.recent_run_artifacts(tmp_path, limit=5)
    # Add another run; cached result should be returned within the TTL window.
    _write_manifest(tmp_path, "run1", {"run_id": "run1"})
    cached = dash.recent_run_artifacts(tmp_path, limit=5)
    assert cached == first


def test_integration_summary_cached_and_invalidate(tmp_path, monkeypatch):
    calls = {"n": 0}

    def fake_summary(root):
        calls["n"] += 1
        return {"default_executor": "manual", "n": calls["n"]}

    monkeypatch.setattr(dash, "integration_status_summary", fake_summary)
    s1 = dash._integration_summary_cached(tmp_path)
    s2 = dash._integration_summary_cached(tmp_path)
    assert s1 == s2
    assert calls["n"] == 1  # cached
    dash._invalidate_integration_summary(tmp_path)
    dash._integration_summary_cached(tmp_path)
    assert calls["n"] == 2


def test_recent_trace_events_cached_empty(tmp_path):
    assert dash._recent_trace_events_cached(tmp_path) == []


def test_dashboard_payload_uninitialized(tmp_path, monkeypatch):
    monkeypatch.setattr(dash, "get_db", lambda root: None)
    monkeypatch.setattr(dash, "integration_status_summary", lambda root: {"default_executor": "manual"})
    payload = dash.dashboard_payload(tmp_path)
    assert payload["initialized"] is False
    assert payload["phase"] == "UNINITIALIZED"
    assert payload["tasks"] == []


def test_is_loopback_client():
    ok = SimpleNamespace(client_address=("127.0.0.1", 1234))
    remote = SimpleNamespace(client_address=("10.0.0.5", 1234))
    assert dash._is_loopback_client(ok) is True
    assert dash._is_loopback_client(remote) is False


def test_dashboard_apply_payload_invalid_token(tmp_path):
    out = dash.dashboard_apply_payload(tmp_path, b"{}", token="secret", provided_token="wrong")
    assert out["ok"] is False
    assert "invalid dashboard token" in out["error"]


def test_dashboard_apply_payload_invalid_json(tmp_path):
    out = dash.dashboard_apply_payload(tmp_path, b"{bad", token="t", provided_token="t")
    assert out["ok"] is False
    assert "invalid JSON body" in out["error"]


def test_dashboard_apply_payload_non_object_body(tmp_path):
    out = dash.dashboard_apply_payload(tmp_path, b"[1,2]", token="t", provided_token="t")
    assert out["ok"] is False
    assert "must be a JSON object" in out["error"]


def test_dashboard_apply_payload_success(tmp_path, monkeypatch):
    invalidated = {"called": False}
    monkeypatch.setattr(
        dash, "apply_integration_target",
        lambda root, target, **k: SimpleNamespace(as_dict=lambda: {"ok": True, "target": target}),
    )
    monkeypatch.setattr(
        dash, "_invalidate_integration_summary",
        lambda root: invalidated.__setitem__("called", True),
    )
    body = json.dumps({"target": "cursor"}).encode("utf-8")
    out = dash.dashboard_apply_payload(tmp_path, body, token="t", provided_token="t")
    assert out["ok"] is True
    assert out["target"] == "cursor"
    assert invalidated["called"] is True


def test_dashboard_apply_payload_value_error(tmp_path, monkeypatch):
    def boom(root, target, **k):
        raise ValueError("unknown target")

    monkeypatch.setattr(dash, "apply_integration_target", boom)
    body = json.dumps({"target": "nope"}).encode("utf-8")
    out = dash.dashboard_apply_payload(tmp_path, body, token="t", provided_token="t")
    assert out["ok"] is False
    assert "unknown target" in out["error"]


def test_dashboard_html_embeds_token_and_sections():
    html = dash.dashboard_html('tok"en')
    # Quote is stripped so the meta attribute stays well-formed.
    assert 'content="token"' in html
    assert "DevCouncil Dashboard" in html
    assert "Recent Agent Runs" in html


def test_logo_assets_load():
    assert dash.logo_svg().strip().startswith("<")
    assert isinstance(dash.logo_asset_bytes(), bytes)
    assert dash.logo_asset_bytes()[:4] == b"\x89PNG"


# ---- additional helper branches -----------------------------------------------

def test_invalidate_recent_runs(tmp_path):
    dash._RECENT_RUNS_CACHE[str(tmp_path)] = (0.0, 5, [])
    dash._invalidate_recent_runs(tmp_path)
    assert str(tmp_path) not in dash._RECENT_RUNS_CACHE


def test_recent_run_artifacts_skips_unreadable_manifest(tmp_path):
    _write_manifest(tmp_path, "good", {"run_id": "good"})
    bad_dir = tmp_path / ".devcouncil" / "runs" / "bad"
    bad_dir.mkdir(parents=True)
    (bad_dir / "agent-run.json").write_text("{not json", encoding="utf-8")
    runs = dash.recent_run_artifacts(tmp_path, limit=10)
    ids = {r.get("run_id") for r in runs}
    assert "good" in ids and None not in ids


def test_artifact_graph_cached_and_invalidate(tmp_path, monkeypatch):
    calls = {"n": 0}

    class FakeRepo:
        def __init__(self, session):
            self.session = session

        def load_graph(self):
            calls["n"] += 1
            return f"graph-{calls['n']}"

    monkeypatch.setattr(dash, "ArtifactGraphRepository", FakeRepo)
    g1 = dash._artifact_graph_cached(tmp_path, session=object())
    g2 = dash._artifact_graph_cached(tmp_path, session=object())
    assert g1 == g2  # served from TTL cache
    assert calls["n"] == 1
    dash._invalidate_artifact_graph(tmp_path)
    dash._artifact_graph_cached(tmp_path, session=object())
    assert calls["n"] == 2


def test_recent_trace_events_cached_append_then_truncate(tmp_path, monkeypatch):
    from devcouncil.telemetry.traces import TraceEvent

    def make_event(name):
        return TraceEvent(type=name)

    state = {"phase": 0}

    def fake_read(root, cursor):
        state["phase"] += 1
        if state["phase"] == 1:
            return [make_event("a"), make_event("b")], 100
        # Cursor goes backwards -> simulate rotation/truncation.
        return [make_event("c")], 10

    monkeypatch.setattr(dash, "read_trace_events_since", fake_read)
    first = dash._recent_trace_events_cached(tmp_path)
    assert [e.type for e in first] == ["a", "b"]
    second = dash._recent_trace_events_cached(tmp_path)
    # Truncation path rebuilds the buffer from just the new events.
    assert [e.type for e in second] == ["c"]


def test_dashboard_payload_initialized(tmp_path, monkeypatch):
    class FakeSessionCtx:
        def __enter__(self):
            return object()

        def __exit__(self, *a):
            return False

    fake_db = SimpleNamespace(get_session=lambda: FakeSessionCtx())
    fake_graph = SimpleNamespace(
        tasks={"T1": SimpleNamespace(model_dump=lambda: {"id": "T1", "status": "done"})},
        coverage_summary=lambda: {"pct": 91},
    )

    class FakeStateRepo:
        def __init__(self, session):
            pass

        def get_state(self):
            return SimpleNamespace(current_phase="EXECUTION")

    monkeypatch.setattr(dash, "get_db", lambda root: fake_db)
    monkeypatch.setattr(dash, "_artifact_graph_cached", lambda root, session: fake_graph)
    monkeypatch.setattr(dash, "StateRepository", FakeStateRepo)
    monkeypatch.setattr(dash, "compute_phase", lambda graph, phase: "PHASE-X")
    monkeypatch.setattr(dash, "_recent_trace_events_cached", lambda root: [])
    monkeypatch.setattr(dash, "_integration_summary_cached", lambda root: {"capabilities": []})
    monkeypatch.setattr(dash, "recent_run_artifacts", lambda root: [])

    payload = dash.dashboard_payload(tmp_path)
    assert payload["initialized"] is True
    assert payload["phase"] == "PHASE-X"
    assert payload["coverage"] == {"pct": 91}
    assert payload["tasks"][0]["id"] == "T1"


def test_json_response_writes_body():
    written = {}

    class FakeHandler:
        def __init__(self):
            self.headers_sent = []

        def send_response(self, status):
            written["status"] = status

        def send_header(self, key, value):
            self.headers_sent.append((key, value))

        def end_headers(self):
            written["ended"] = True

        @property
        def wfile(self):
            class W:
                def write(self, body):
                    written["body"] = body

            return W()

    handler = FakeHandler()
    dash._json_response(handler, 201, {"ok": True})
    assert written["status"] == 201
    assert written["ended"] is True
    assert json.loads(written["body"].decode("utf-8")) == {"ok": True}


# ---- HTTP server end-to-end ---------------------------------------------------

def test_run_dashboard_serves_all_routes(tmp_path, monkeypatch):
    import socket
    import threading
    import time
    import urllib.error
    import urllib.request

    created: dict = {}
    real_server = dash.ThreadingHTTPServer

    class RecordingServer(real_server):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            created["server"] = self

    monkeypatch.setattr(dash, "ThreadingHTTPServer", RecordingServer)
    monkeypatch.setattr(dash, "get_db", lambda root: None)
    monkeypatch.setattr(
        dash, "integration_status_summary", lambda root: {"default_executor": "manual", "capabilities": []}
    )
    monkeypatch.setattr(
        dash,
        "build_integration_check_report",
        lambda root: SimpleNamespace(as_dict=lambda: {"ok": True}),
    )

    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()

    thread = threading.Thread(
        target=dash.run_dashboard, args=(tmp_path, "127.0.0.1", port), daemon=True
    )
    thread.start()

    base = f"http://127.0.0.1:{port}"

    def get(path):
        for _ in range(100):
            try:
                return urllib.request.urlopen(base + path, timeout=2)
            except (urllib.error.URLError, ConnectionError):
                time.sleep(0.02)
        raise AssertionError(f"server never came up for {path}")

    try:
        assert b"DevCouncil Dashboard" in get("/").read()
        assert b"UNINITIALIZED" in get("/api/status").read()
        assert b"ok" in get("/api/integrations/check").read()
        assert get(f"/assets/{dash.LOGO_ASSET}").read()[:4] == b"\x89PNG"
        assert get(f"/assets/{dash.LEGACY_LOGO_ASSET}").read().strip().startswith(b"<")

        # Unknown /api/ path -> 404.
        with pytest.raises(urllib.error.HTTPError) as exc:
            urllib.request.urlopen(base + "/api/unknown", timeout=2)
        assert exc.value.code == 404

        # POST to apply with no/invalid token -> 403 (loopback allowed, token rejected).
        req = urllib.request.Request(
            base + "/api/integrations/apply", data=b"{}", method="POST"
        )
        with pytest.raises(urllib.error.HTTPError) as exc_post:
            urllib.request.urlopen(req, timeout=2)
        assert exc_post.value.code == 403

        # POST to an unknown path -> 404.
        req2 = urllib.request.Request(base + "/api/nope", data=b"{}", method="POST")
        with pytest.raises(urllib.error.HTTPError) as exc_post2:
            urllib.request.urlopen(req2, timeout=2)
        assert exc_post2.value.code == 404
    finally:
        created["server"].shutdown()
        thread.join(timeout=5)
