"""Coverage for campaign.notify (ntfy push notifier, best-effort semantics)."""

from __future__ import annotations

import sys
import types


from devcouncil.campaign.notify import NullNotifier, Notifier


def test_disabled_when_no_topic(monkeypatch):
    monkeypatch.delenv("DIRECTOR_NTFY_TOPIC", raising=False)
    n = Notifier()
    assert n.enabled is False
    assert n.notify("hello") is False
    assert n.sent == ["hello"]  # still recorded


def test_topic_from_env(monkeypatch):
    monkeypatch.setenv("DIRECTOR_NTFY_TOPIC", "my-topic")
    monkeypatch.delenv("DIRECTOR_NTFY_SERVER", raising=False)
    n = Notifier()
    assert n.topic == "my-topic"
    assert n.enabled is True
    assert n.server == "https://ntfy.sh"


def test_server_override_strips_trailing_slash():
    n = Notifier(topic="t", server="https://push.example.com/")
    assert n.server == "https://push.example.com"


def test_null_notifier_never_sends():
    n = NullNotifier()
    assert n.enabled is False
    assert n.notify("x", title="T") is False
    assert n.sent == ["x"]


def test_notify_posts_and_reports_success(monkeypatch):
    calls = {}

    class FakeResponse:
        status_code = 200

    def fake_post(url, content=None, headers=None, timeout=None):
        calls["url"] = url
        calls["content"] = content
        calls["headers"] = headers
        return FakeResponse()

    fake_httpx = types.ModuleType("httpx")
    fake_httpx.post = fake_post
    monkeypatch.setitem(sys.modules, "httpx", fake_httpx)
    monkeypatch.setenv("DIRECTOR_NTFY_TOKEN", "secret")

    n = Notifier(topic="alerts", server="https://ntfy.sh")
    ok = n.notify("done", title="Campaign", priority="high", tags=["tada"])
    assert ok is True
    assert calls["url"] == "https://ntfy.sh/alerts"
    assert calls["content"] == b"done"
    assert calls["headers"]["Title"] == "Campaign"
    assert calls["headers"]["Priority"] == "high"
    assert calls["headers"]["Tags"] == "tada"
    assert calls["headers"]["Authorization"] == "Bearer secret"


def test_notify_reports_failure_on_4xx(monkeypatch):
    class FakeResponse:
        status_code = 500

    fake_httpx = types.ModuleType("httpx")
    fake_httpx.post = lambda *a, **k: FakeResponse()
    monkeypatch.setitem(sys.modules, "httpx", fake_httpx)

    n = Notifier(topic="alerts")
    assert n.notify("boom") is False


def test_notify_swallows_exceptions(monkeypatch):
    fake_httpx = types.ModuleType("httpx")

    def raiser(*a, **k):
        raise RuntimeError("network down")

    fake_httpx.post = raiser
    monkeypatch.setitem(sys.modules, "httpx", fake_httpx)

    n = Notifier(topic="alerts")
    # never raises, returns False
    assert n.notify("boom") is False


def test_enabled_override():
    n = Notifier(topic="", enabled=True)
    # forced enabled but no topic -> notify short-circuits False
    assert n.enabled is True
    assert n.notify("x") is False
