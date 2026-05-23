from devcouncil.storage.db import Database
import socketserver

from devcouncil.ui.dashboard import dashboard_html, dashboard_payload, logo_asset_bytes, logo_svg, run_dashboard


def test_dashboard_payload_handles_uninitialized_project(tmp_path):
    payload = dashboard_payload(tmp_path)

    assert payload["initialized"] is False
    assert payload["phase"] == "UNINITIALIZED"


def test_dashboard_payload_reads_initialized_project(tmp_path):
    from devcouncil.storage.repositories import StateRepository

    dev_dir = tmp_path / ".devcouncil"
    dev_dir.mkdir()
    db = Database(dev_dir / "state.sqlite")
    db.create_db_and_tables()
    with db.get_session() as session:
        StateRepository(session).record_phase("TASK_VERIFYING")

    payload = dashboard_payload(tmp_path)

    assert payload["initialized"] is True
    assert payload["phase"] == "TASK_VERIFYING"


def test_dashboard_html_contains_live_status_endpoint():
    html = dashboard_html()

    assert "/api/status" in html
    assert "/assets/devcouncil_logo_premium.png" in html
    assert "DevCouncil Dashboard" in html
    assert "replaceChildren" in html
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
