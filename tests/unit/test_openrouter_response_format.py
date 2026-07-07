"""OpenRouter structured-output degrade chain.

Regression for the arm-B benchmark failure where a model whose endpoints support
no ``response_format`` variant (e.g. free-tier endpoints) combined with
``provider.require_parameters: true`` made OpenRouter 404 every planning call:
json_schema degraded to json_object, which is still ``response_format``, which
still 404'd — and planning died in seconds. The provider must degrade all the
way to prompt-only JSON, remember the model, and never degrade on transient
statuses (429/5xx).
"""

import copy

import pytest

from devcouncil.llm.provider import OpenRouterProvider, ProviderRequestError


@pytest.fixture
def anyio_backend():
    return "asyncio"


class FakeResponse:
    def __init__(self, status_code, data=None, text=""):
        self.status_code = status_code
        self._data = data or {}
        self.text = text

    def json(self):
        return self._data


OK_BODY = {
    "choices": [{"message": {"content": '{"ok": true}'}}],
    "model": "test/model",
    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
}


def make_client(calls, responder):
    class FakeClient:
        async def post(self, url, headers, json):
            # Deep-copy: the provider mutates the SAME payload dict between degrade
            # retries, so recording the reference would rewrite history.
            calls.append(copy.deepcopy(json))
            return responder(json)

    return FakeClient()


def _provider(monkeypatch, calls, responder, tmp_path):
    provider = OpenRouterProvider("key", project_root=tmp_path)
    monkeypatch.setattr(
        provider, "_get_async_client", lambda timeout: make_client(calls, responder)
    )
    return provider


SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}}
MESSAGES = [{"role": "user", "content": "hi"}]


@pytest.mark.anyio
async def test_degrades_past_json_object_to_no_response_format(monkeypatch, tmp_path):
    calls = []

    def responder(payload):
        if "response_format" in payload:
            return FakeResponse(404, text="No endpoints found matching your data policy")
        return FakeResponse(200, OK_BODY)

    provider = _provider(monkeypatch, calls, responder, tmp_path)
    resp = await provider.complete(
        model="test/model", messages=MESSAGES, json_mode=True, json_schema=SCHEMA
    )

    assert resp.content == '{"ok": true}'
    formats = [c.get("response_format", {}).get("type", "absent") for c in calls]
    assert formats == ["json_schema", "json_object", "absent"]

    # The rejection is remembered per model: the next call goes straight to
    # prompt-only JSON instead of paying two failed requests again.
    calls.clear()
    resp = await provider.complete(
        model="test/model", messages=MESSAGES, json_mode=True, json_schema=SCHEMA
    )
    assert resp.content == '{"ok": true}'
    assert len(calls) == 1
    assert "response_format" not in calls[0]


@pytest.mark.anyio
async def test_json_schema_rejection_still_degrades_to_json_object(monkeypatch, tmp_path):
    calls = []

    def responder(payload):
        fmt = payload.get("response_format", {}).get("type")
        if fmt == "json_schema":
            return FakeResponse(400, text="schema not supported")
        return FakeResponse(200, OK_BODY)

    provider = _provider(monkeypatch, calls, responder, tmp_path)
    resp = await provider.complete(
        model="test/model", messages=MESSAGES, json_mode=True, json_schema=SCHEMA
    )
    assert resp.content == '{"ok": true}'
    formats = [c.get("response_format", {}).get("type", "absent") for c in calls]
    assert formats == ["json_schema", "json_object"]


@pytest.mark.anyio
async def test_transient_429_does_not_disable_structured_output(monkeypatch, tmp_path):
    calls = []

    def responder(payload):
        return FakeResponse(429, text="rate limited")

    provider = _provider(monkeypatch, calls, responder, tmp_path)
    with pytest.raises(ProviderRequestError) as excinfo:
        await provider.complete(
            model="test/model", messages=MESSAGES, json_mode=True, json_schema=SCHEMA
        )
    assert excinfo.value.status_code == 429
    # No degrade retries were burned on a transient status, and the model was NOT
    # blacklisted from structured output.
    assert len(calls) == 1
    assert "test/model" not in provider._schema_format_unsupported
    assert "test/model" not in provider._response_format_unsupported


# --- benchmark-surfaced fixes: client-side RPM pacing + shared 429 cooldown ----


def test_resolve_rpm_parsing(monkeypatch):
    for raw, expected in [("15", 15.0), ("7.5", 7.5), ("off", None), ("0", None),
                          ("none", None), ("", None), ("garbage", None)]:
        monkeypatch.setenv("OPENROUTER_RPM", raw)
        assert OpenRouterProvider._resolve_rpm() == expected, raw
    monkeypatch.delenv("OPENROUTER_RPM")
    assert OpenRouterProvider._resolve_rpm() is None  # off unless explicitly set


@pytest.mark.anyio
async def test_rpm_pacing_spaces_request_starts(monkeypatch, tmp_path):
    """With OPENROUTER_RPM set, request starts are spaced by 60/rpm seconds so a
    fan-out never trips the endpoint's RPM cap in the first place."""
    monkeypatch.setenv("OPENROUTER_RPM", "1200")  # 50ms interval — fast test
    calls = []
    provider = _provider(monkeypatch, calls, lambda p: FakeResponse(200, OK_BODY), tmp_path)
    assert provider.requests_per_minute == 1200

    import time

    t0 = time.monotonic()
    for _ in range(3):
        await provider.complete(model="test/model", messages=MESSAGES)
    elapsed = time.monotonic() - t0
    # 3 requests = 2 inter-request gaps of ~50ms (first one starts immediately).
    assert elapsed >= 0.08


@pytest.mark.anyio
async def test_429_pushes_shared_cooldown_even_without_rpm(monkeypatch, tmp_path):
    """A 429 must back off EVERY subsequent request (shared pacer slot honoring
    Retry-After), not just the one that received it — and it must do so even
    when RPM pacing is disabled."""
    monkeypatch.delenv("OPENROUTER_RPM", raising=False)
    calls = []
    responses = [FakeResponse(429, text="rate limited"), FakeResponse(200, OK_BODY)]
    provider = _provider(monkeypatch, calls, lambda p: responses.pop(0), tmp_path)
    assert provider.requests_per_minute is None

    import time

    with pytest.raises(ProviderRequestError) as excinfo:
        await provider.complete(model="test/model", messages=MESSAGES)
    assert excinfo.value.status_code == 429
    # The shared slot was pushed forward (fallback cooldown: no Retry-After header).
    assert provider._next_request_at > time.monotonic()

    # The next request waits out the cooldown before posting.
    provider._next_request_at = time.monotonic() + 0.05  # shrink for test speed
    t0 = time.monotonic()
    resp = await provider.complete(model="test/model", messages=MESSAGES)
    assert resp.content == '{"ok": true}'
    assert time.monotonic() - t0 >= 0.04
