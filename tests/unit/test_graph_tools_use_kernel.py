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
from pathlib import Path

import pytest

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


def test_graph_query_falls_back_to_python_and_says_so(tmp_path, monkeypatch):
    monkeypatch.setattr(
        mapmod, "_devmap_query_payload", lambda *a, **k: None, raising=False
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query_symbol",
        lambda root, name: {"definitions": [], "matches": []},
        raising=False,
    )

    payload = _payload(
        asyncio.run(mapmod.handle_graph_query(tmp_path, {"name_or_path": "f"}))
    )

    assert payload.get("source") == "code_graph", (
        "a Python answer must name itself, or a caller cannot tell which engine "
        f"replied: {payload}"
    )


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


def test_graph_trace_falls_back_to_python_and_says_so(tmp_path, monkeypatch):
    monkeypatch.setattr(
        mapmod, "_devmap_query_payload", lambda *a, **k: None, raising=False
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.trace_path",
        lambda root, a, b: {"path": []},
        raising=False,
    )

    payload = _payload(
        asyncio.run(mapmod.handle_graph_trace(tmp_path, {"from": "a", "to": "b"}))
    )
    assert payload.get("source") == "code_graph", payload


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
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query_symbol",
        lambda root, name: {"definitions": []},
        raising=False,
    )
    monkeypatch.setattr(
        "devcouncil.indexing.graph.trace_path",
        lambda root, a, b: {"path": []},
        raising=False,
    )

    asyncio.run(mapmod.handle_graph_query(tmp_path, {"name_or_path": "f"}))
    asyncio.run(mapmod.handle_graph_trace(tmp_path, {"from": "a", "to": "b"}))

    assert set(seen["query"]) == {"name_or_path"}, seen["query"]
    assert set(seen["trace"]) == {"start", "end"}, seen["trace"]
