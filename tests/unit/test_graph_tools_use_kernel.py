"""`devcouncil_graph_query` / `_trace` must ask the Rust kernel before Python.

Measured on this repository (14,057 nodes / 71,195 edges, warm daemon,
in-process, min-of-3): the Python-backed `devcouncil_graph_query` cost 1.057 s
and `devcouncil_graph_trace` 1.133 s, while the kernel-backed siblings answering
the same class of question cost 0.154 s and 0.202 s. ~90% of the Python cost is
re-materialising the whole graph as pydantic models from `index.sqlite` — a
242 MB SQLite cache of `code_graph.json` that the *query* path itself maintains,
so the first call after any kernel build also took 6.5-7.5 s and wrote 242 MB
under a writer lease, from a tool an agent reads as read-only.

Two-signal proof the kernel was not in the answer path before this change:
static (`handle_graph_query` imported only `query_symbol`), and runtime (with
`DEVMAP_BINARY=/nonexistent` the handler still returned `ok=True` with
definitions, while `devcouncil_code_search` correctly failed closed).
"""

from __future__ import annotations

import asyncio
import json

from devcouncil.integrations.mcp.handlers import map as mapmod


def _payload(result):
    return json.loads(result[0].text)


def test_graph_query_prefers_the_kernel(tmp_path, monkeypatch):
    calls: list[tuple] = []

    def _kernel(root, kind, **kwargs):
        calls.append((kind, kwargs))
        return {"ok": True, "definitions": [{"id": "a.py::f"}], "source": "devmap"}

    monkeypatch.setattr(mapmod, "_devmap_query_payload", _kernel, raising=False)

    payload = _payload(
        asyncio.run(mapmod.handle_graph_query(tmp_path, {"name_or_path": "f"}))
    )

    assert calls and calls[0][0] == "query", (
        "the kernel must be asked first; it was never called: " f"{calls}"
    )
    assert payload["source"] == "devmap"
    assert payload["definitions"] == [{"id": "a.py::f"}]


def test_graph_query_without_a_kernel_is_an_error_not_a_python_answer(
    tmp_path, monkeypatch
):
    """Rewritten: this test used to assert the Python fallback names itself.

    Naming the second engine was the right rule while there were two. There is
    one. `query_symbol` walked `load_code_graph` — the retired engine's
    whole-graph read, 855 ms and ~690 MB RSS on this repository, 3.8 s and a
    102 MB `index.sqlite` write on the first call — from a tool an agent reads
    as read-only, and its answers do not agree with the kernel's (see the
    trace case below, where the disagreement is a *false path*).
    """
    monkeypatch.setattr(
        mapmod, "_devmap_query_payload", lambda *a, **k: None, raising=False
    )

    payload = _payload(
        asyncio.run(mapmod.handle_graph_query(tmp_path, {"name_or_path": "f"}))
    )

    assert payload.get("ok") is False, payload
    assert payload.get("source") != "code_graph", payload
    assert "devmap" in json.dumps(payload).lower(), payload


def test_graph_trace_prefers_the_kernel(tmp_path, monkeypatch):
    calls: list[tuple] = []

    def _kernel(root, kind, **kwargs):
        calls.append((kind, kwargs))
        return {"ok": True, "path": [], "source": "devmap"}

    monkeypatch.setattr(mapmod, "_devmap_query_payload", _kernel, raising=False)

    payload = _payload(
        asyncio.run(mapmod.handle_graph_trace(tmp_path, {"from": "a", "to": "b"}))
    )

    assert calls and calls[0][0] == "trace", f"the kernel must be asked first: {calls}"
    assert payload["source"] == "devmap"


def test_graph_trace_without_a_kernel_is_an_error_not_a_python_answer(
    tmp_path, monkeypatch
):
    """The Python tracer was not merely slower; it was wrong.

    Its BFS was *undirected* over `imports`/`calls`/`contains`/`defines`/
    `inherits`, while the kernel walks resolved edges directionally. On a real
    probe Python reported a two-hop path between two functions through a shared
    test module where the kernel correctly reported no indexed path — a
    fabricated path is worse than an absent answer, because a caller acts on it.
    """
    monkeypatch.setattr(
        mapmod, "_devmap_query_payload", lambda *a, **k: None, raising=False
    )

    payload = _payload(
        asyncio.run(mapmod.handle_graph_trace(tmp_path, {"from": "a", "to": "b"}))
    )
    assert payload.get("ok") is False, payload
    assert payload.get("source") != "code_graph", payload
    assert "devmap" in json.dumps(payload).lower(), payload


def test_handlers_call_the_kernel_with_the_kwargs_it_actually_declares(tmp_path, monkeypatch):
    """Guard the seam the mocks above cannot see.

    The two tests that assert "the kernel is asked first" monkeypatch
    `_devmap_query_payload`, so they accept any keyword name. The real function
    reads `kwargs["name_or_path"]` for `query` and `kwargs["start"]`/`["end"]`
    for `trace` — and the first version of this wiring passed `query=`, which
    would have raised `KeyError` against the real callee on every call while
    every mocked test stayed green.

    This drives the real `_devmap_query_payload`, stopping only at
    `try_connect`, so the keyword contract is checked without needing a store.
    """
    seen: dict = {}

    def _no_daemon(root, **kwargs):
        return None

    monkeypatch.setattr("devcouncil.devmap_client.try_connect", _no_daemon)

    real = mapmod._devmap_query_payload

    def _record(root, kind, **kwargs):
        seen[kind] = kwargs
        return real(root, kind, **kwargs)

    monkeypatch.setattr(mapmod, "_devmap_query_payload", _record)

    asyncio.run(mapmod.handle_graph_query(tmp_path, {"name_or_path": "f"}))
    asyncio.run(mapmod.handle_graph_trace(tmp_path, {"from": "a", "to": "b"}))

    assert set(seen["query"]) == {"name_or_path"}, seen["query"]
    assert set(seen["trace"]) == {"start", "end"}, seen["trace"]


# ---- the HTTP route surfaces --------------------------------------------------
#
# `devcouncil_route_map`, `devcouncil_shape_check` and `devcouncil_api_impact`
# each did `load_code_graph(root)` and then ran a Python re-implementation of a
# command the kernel already has. Measured on a tmp copy of this repository's
# tree (A/B interleaved, n=11, both warm, release kernel):
#
#   route_map     load_code_graph + Python  p50 2058.2 ms   kernel  860.3 ms
#   shape_check   load_code_graph + Python  p50 2451.9 ms   kernel 1031.5 ms
#   api_impact    load_code_graph + Python  p50 2674.4 ms   kernel  843.8 ms
#
# and the kernel's answers are supersets: `routes` carries `capabilities` and
# `scan`, `shape-check` and `api-impact` carry `scan` — what the bounded client
# scan read and whether it finished. The Python versions carried no such record,
# so a scan that stopped at its file cap was published as a complete inventory.


class _RouteClient:
    """A `DevMapClient` double for the three route commands."""

    def __init__(self, *, routes=None, shape=None, impact=None):
        self._routes = routes or {"count": 0, "routes": [], "capabilities": {}, "scan": {}}
        self._shape = shape or {"checks": 0, "mismatches": [], "mismatch_count": 0, "scan": {}}
        self._impact = impact or {"found": False, "route": "", "scan": {}}
        self.calls: list[tuple] = []

    def is_map_stale(self):
        return False

    def routes(self, route_filter=None):
        self.calls.append(("routes", route_filter))
        return self._routes

    def shape_check(self, route_filter=None):
        self.calls.append(("shape_check", route_filter))
        return self._shape

    def api_impact(self, route):
        self.calls.append(("api_impact", route))
        return self._impact


def test_the_route_double_matches_the_client_it_stands_in_for():
    """The double must accept what the real client accepts."""
    import inspect

    from devcouncil.devmap_client import DevMapClient

    for name in ("routes", "shape_check", "api_impact"):
        real = set(inspect.signature(getattr(DevMapClient, name)).parameters)
        fake = set(inspect.signature(getattr(_RouteClient, name)).parameters)
        assert real <= fake, f"_RouteClient.{name} is missing {sorted(real - fake)}"


def _no_python_graph(monkeypatch):
    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda root: (_ for _ in ()).throw(
            AssertionError("the route tools must not load the Python graph")
        ),
    )


def test_route_map_asks_the_kernel(tmp_path, monkeypatch):
    client = _RouteClient(routes={
        "count": 2,
        "shown": 2,
        "routes": [{"id": "a.py::GET /x", "path": "/x", "verb": "GET"}],
        "capabilities": {"framework_available": True, "route_nodes": 2},
        "scan": {"complete": False, "files_read": 10, "files_eligible": 99},
    })
    _no_python_graph(monkeypatch)
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: client)

    payload = _payload(asyncio.run(mapmod.handle_route_map(tmp_path, {})))

    assert client.calls == [("routes", None)], client.calls
    assert payload["ok"] is True
    assert payload["count"] == 2
    # The kernel's coverage record must reach the caller: a scan that stopped
    # at its file cap published as a complete route inventory is the reading
    # that gets a live route treated as unused.
    assert payload["scan"]["complete"] is False
    assert payload["capabilities"]["route_nodes"] == 2


def test_shape_check_asks_the_kernel(tmp_path, monkeypatch):
    client = _RouteClient(shape={
        "checks": 3,
        "mismatch_count": 1,
        "mismatches": [{"route": "/x", "missing_keys": ["id"]}],
        "scan": {"complete": True, "files_read": 99, "files_eligible": 99},
    })
    _no_python_graph(monkeypatch)
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: client)

    payload = _payload(asyncio.run(mapmod.handle_shape_check(tmp_path, {})))

    assert client.calls == [("shape_check", None)], client.calls
    assert payload["ok"] is True
    assert payload["mismatch_count"] == 1
    assert payload["scan"]["complete"] is True


def test_shape_check_passes_the_route_filter_through(tmp_path, monkeypatch):
    """A filter the caller asked for must reach the kernel, not be dropped.

    A request that silently drops the filter answers a broader question than
    the caller asked while looking like it answered theirs.
    """
    client = _RouteClient()
    _no_python_graph(monkeypatch)
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: client)

    asyncio.run(mapmod.handle_shape_check(tmp_path, {"route": "/api/items"}))

    assert client.calls == [("shape_check", "/api/items")], client.calls


def test_api_impact_asks_the_kernel(tmp_path, monkeypatch):
    client = _RouteClient(impact={
        "found": True,
        "route": "/api/items",
        "risk": "high",
        "risk_reason": "3 consumers read keys the handler does not return",
        "scan": {"complete": True},
    })
    _no_python_graph(monkeypatch)
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: client)

    payload = _payload(
        asyncio.run(mapmod.handle_api_impact(tmp_path, {"route_or_path": "/api/items"}))
    )

    assert client.calls == [("api_impact", "/api/items")], client.calls
    assert payload["ok"] is True
    assert payload["risk"] == "high"
    # `risk_reason` and `matched_routes` are kernel-only fields; dropping them
    # would republish a richer answer as the thinner one Python used to give.
    assert "risk_reason" in payload


def test_the_route_tools_name_the_kernel_when_it_cannot_answer(tmp_path, monkeypatch):
    """No kernel is an error naming the kernel, never a Python answer."""
    _no_python_graph(monkeypatch)
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)

    for handler, args in (
        (mapmod.handle_route_map, {}),
        (mapmod.handle_shape_check, {}),
        (mapmod.handle_api_impact, {"route_or_path": "/x"}),
    ):
        payload = _payload(asyncio.run(handler(tmp_path, args)))
        assert payload["ok"] is False, payload
        assert "devmap" in json.dumps(payload).lower(), payload
