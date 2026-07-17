from devcouncil.storage.db import Database
import socketserver

from devcouncil.ui.dashboard import dashboard_html, dashboard_payload, logo_asset_bytes, logo_svg, run_dashboard


def test_dashboard_payload_handles_uninitialized_project(tmp_path):
    payload = dashboard_payload(tmp_path)

    assert payload["initialized"] is False
    assert payload["phase"] == "UNINITIALIZED"
    assert payload["gaps"] == {"total": 0, "blocking": 0, "items": []}
    assert "integrations" in payload
    assert "recent_runs" in payload


def test_dashboard_payload_includes_integrations_and_recent_runs(tmp_path):
    (tmp_path / ".devcouncil" / "runs" / "run-1").mkdir(parents=True)
    (tmp_path / ".devcouncil" / "runs" / "run-1" / "agent-run.json").write_text(
        '{"run_id":"run-1","task_id":"TASK-001","agent":"codex","status":"finished"}\n',
        encoding="utf-8",
    )

    payload = dashboard_payload(tmp_path)

    assert "integrations" in payload
    assert "recent_runs" in payload
    assert payload["recent_runs"][0]["run_id"] == "run-1"


def test_dashboard_payload_reads_initialized_project(tmp_path):
    from devcouncil.domain.gap import Gap
    from devcouncil.storage.repositories import GapRepository, StateRepository

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        StateRepository(session).record_phase("TASK_VERIFYING")
        GapRepository(session).save(
            Gap(
                id="GAP-BLOCK-1",
                severity="high",
                gap_type="orphan_diff",
                task_id="TASK-001",
                description="Blocking orphan diff",
                recommended_fix="Revert",
                blocking=True,
            )
        )
        GapRepository(session).save(
            Gap(
                id="GAP-ADV-1",
                severity="medium",
                gap_type="stub_detected",
                task_id="TASK-002",
                description="Advisory stub",
                recommended_fix="Remove stub",
                blocking=False,
            )
        )

    payload = dashboard_payload(tmp_path)

    assert payload["initialized"] is True
    assert payload["phase"] == "TASK_VERIFYING"
    assert payload["gaps"]["total"] == 2
    assert payload["gaps"]["blocking"] == 1
    assert payload["gaps"]["items"][0]["blocking"] is True
    assert payload["gaps"]["items"][0]["task_id"] == "TASK-001"


def test_dashboard_html_contains_live_status_endpoint():
    html = dashboard_html()

    assert "/api/status" in html
    assert "/assets/devcouncil_logo_premium.png" in html
    assert "DevCouncil Dashboard" in html
    assert "replaceChildren" in html
    assert "innerHTML" not in html


def test_dashboard_html_contains_integration_sections():
    html = dashboard_html()

    assert "CLI Integrations" in html
    assert "Recent Agent Runs" in html
    assert "Verification Gaps" in html
    assert "integrations" in html
    assert "recent_runs" in html
    assert "gaps" in html
    assert "innerHTML" not in html


def test_dashboard_premium_logo_asset_is_packaged_png():
    logo = logo_asset_bytes()

    assert logo.startswith(b"\x89PNG\r\n\x1a\n")
    assert len(logo) > 100_000


def test_dashboard_logo_asset_is_packaged_svg():
    svg = logo_svg()

    assert svg.startswith("<svg")
    assert "#ff3333" in svg
    assert "#000000" in svg
    assert "linearGradient" in svg


def test_dashboard_html_contains_apply_controls_without_inner_html():
    html = dashboard_html("secret")

    assert "Apply Detected" in html
    assert "Install Hooks" in html
    assert "Run Check" in html
    assert "data-target" in html
    assert "X-DevCouncil-Dashboard-Token" in html
    assert "innerHTML" not in html


def test_dashboard_apply_endpoint_requires_token(monkeypatch, tmp_path):
    from io import BytesIO

    from devcouncil.ui import dashboard as dashboard_module

    captured = {}

    class FakeServer:
        def __init__(self, address, handler):
            captured["handler"] = handler
            self.allow_reuse_address = True
            self.daemon_threads = True

        def serve_forever(self):
            return None

    monkeypatch.setattr(dashboard_module, "ThreadingHTTPServer", FakeServer)
    dashboard_module.run_dashboard(tmp_path, host="127.0.0.1", port=9999)
    handler_class = captured["handler"]

    class FakeHandler(handler_class):
        def __init__(self):
            self.path = "/api/integrations/apply"
            self.headers = {"Content-Length": "19"}
            self.rfile = BytesIO(b'{"target":"cursor"}')
            self.wfile = BytesIO()
            self.status = None
            self.sent_headers = {}
            self.client_address = ("127.0.0.1", 12345)

        def send_response(self, code):
            self.status = code

        def send_header(self, key, value):
            self.sent_headers[key] = value

        def end_headers(self):
            return None

    handler = FakeHandler()
    handler.do_POST()

    assert handler.status == 403
    assert b"invalid dashboard token" in handler.wfile.getvalue()


def test_dashboard_apply_endpoint_calls_action_service(monkeypatch, tmp_path):
    from devcouncil.integrations.actions import IntegrationActionReport
    from devcouncil.ui import dashboard as dashboard_module

    captured = {}

    def fake_apply(project_root, target, **kwargs):
        captured["project_root"] = project_root
        captured["target"] = target
        captured["kwargs"] = kwargs
        return IntegrationActionReport(target=target, ok=True, results=[], warnings=[], check={"ok": True, "checks": []})

    monkeypatch.setattr(dashboard_module, "apply_integration_target", fake_apply)

    body = '{"target": "cursor", "include_hooks": false}'.encode("utf-8")
    response = dashboard_module.dashboard_apply_payload(
        tmp_path,
        body,
        token="secret",
        provided_token="secret",
    )

    assert response["ok"] is True
    assert captured["target"] == "cursor"
    assert captured["kwargs"]["include_hooks"] is False


def test_dashboard_server_uses_reusable_threaded_server(monkeypatch, tmp_path):
    captured = {}

    class FakeServer:
        def __init__(self, address, handler):
            captured["address"] = address
            captured["handler"] = handler
            captured["allow_reuse_address"] = self.allow_reuse_address
            captured["daemon_threads"] = self.daemon_threads

        def serve_forever(self):
            captured["served"] = True

    monkeypatch.setattr(socketserver, "ThreadingMixIn", socketserver.ThreadingMixIn)
    monkeypatch.setattr("devcouncil.ui.dashboard.ThreadingHTTPServer", FakeServer)

    run_dashboard(tmp_path, host="127.0.0.1", port=9999)

    assert captured["address"] == ("127.0.0.1", 9999)
    assert captured["allow_reuse_address"] is True
    assert captured["daemon_threads"] is True
    assert captured["served"] is True
