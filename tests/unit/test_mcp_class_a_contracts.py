"""Class A contracts for MCP read surfaces.

Two rules are asserted here, and nothing else:

* A check that could not run must never report what a check that ran and
  passed reports.
* A capped sample is never presented as complete coverage: a truncating
  surface reports ``shown`` and ``truncated`` beside its items, and either a
  real ``total`` or an explicit "not counted" marker — never a total invented
  from the capped slice.
"""

from __future__ import annotations

import json

import pytest

from devcouncil.codeintel.query.engine import CodeIntelQueryEngine
from devcouncil.indexing.graph.schema import (
    CodeGraph,
    Confidence,
    GraphEdge,
    GraphNode,
    NodeKind,
)
from devcouncil.integrations.mcp import util as mcp_util
from devcouncil.integrations.mcp.handlers import ast_lsp as ast_handlers
from devcouncil.integrations.mcp.handlers import trace as trace_handlers
from devcouncil.telemetry.traces import TraceLogger


@pytest.fixture
def anyio_backend():
    return "asyncio"


def _parse(contents):
    return json.loads(contents[0].text)


# ---- devcouncil_tail_trace ----------------------------------------------------


@pytest.mark.anyio
async def test_tail_trace_reports_total_beside_the_cap(tmp_path):
    logger = TraceLogger(tmp_path)
    for index in range(7):
        logger.log_event("unit_test_event", {"index": index})

    out = _parse(await trace_handlers.handle_tail_trace(tmp_path, {"limit": 3}))

    assert len(out["events"]) == 3
    assert out["shown"] == 3
    assert out["total"] == 7
    assert out["truncated"] is True
    assert out["limit_applied"] == 3


@pytest.mark.anyio
async def test_tail_trace_untruncated_says_so(tmp_path):
    TraceLogger(tmp_path).log_event("unit_test_event", {"index": 0})

    out = _parse(await trace_handlers.handle_tail_trace(tmp_path, {"limit": 20}))

    assert out["shown"] == 1
    assert out["total"] == 1
    assert out["truncated"] is False


# ---- devcouncil_ast_match -----------------------------------------------------


@pytest.mark.anyio
async def test_ast_match_capped_result_is_not_reported_as_complete(tmp_path):
    ast_handlers.reset_caches()
    (tmp_path / "a.py").write_text(
        "".join(f"def fn_{i}():\n    return {i}\n" for i in range(10)), encoding="utf-8"
    )

    out = _parse(await ast_handlers.handle_ast_match(tmp_path, {"query": "fn_", "limit": 3}))

    assert len(out["matches"]) == 3
    assert out["shown"] == 3
    assert out["truncated"] is True
    assert out["limit_applied"] == 3
    # The matcher stops scanning at the limit, so the unfiltered total was never
    # measured. It must be reported as unmeasured, never as `shown`.
    assert out["total"] is None
    assert out["total_reason"]


@pytest.mark.anyio
async def test_ast_match_under_the_cap_reports_a_real_total(tmp_path):
    ast_handlers.reset_caches()
    (tmp_path / "a.py").write_text("def only_one():\n    return 1\n", encoding="utf-8")

    out = _parse(await ast_handlers.handle_ast_match(tmp_path, {"query": "only_one", "limit": 50}))

    assert out["shown"] == 1
    assert out["total"] == 1
    assert out["truncated"] is False


# ---- CodeIntelQueryEngine.explore ---------------------------------------------


class _Store:
    def content_for_path(self, path):  # noqa: ARG002 - stub
        return None


class _Service:
    store = _Store()


class _StubEngine(CodeIntelQueryEngine):
    """Engine over a hand-built graph; the store and envelope are stubbed out."""

    def __init__(self, graph: CodeGraph) -> None:  # noqa: D107
        self._stub_graph = graph
        self.service = _Service()
        self._devmap_client = None

    def _graph(self) -> CodeGraph:
        return self._stub_graph

    def _envelope(self, payload):
        return {"ok": True, **payload}


def _explore_graph(match_count: int = 5, caller_count: int = 0) -> CodeGraph:
    nodes = [
        GraphNode(id=f"m{i}.py::widget_{i}", kind=NodeKind.FUNCTION, path=f"m{i}.py", name=f"widget_{i}")
        for i in range(match_count)
    ]
    edges: list[GraphEdge] = []
    if match_count:
        target = nodes[0].id
        for i in range(caller_count):
            caller = GraphNode(
                id=f"c{i}.py::caller_{i}", kind=NodeKind.FUNCTION, path=f"c{i}.py", name=f"caller_{i}"
            )
            nodes.append(caller)
            edges.append(
                GraphEdge(
                    source=caller.id, target=target, kind="calls", confidence=Confidence.EXTRACTED
                )
            )
    return CodeGraph(nodes=nodes, edges=edges)


def test_explore_reports_match_total_beside_the_cap():
    out = _StubEngine(_explore_graph(match_count=5)).explore("widget_", limit=2)

    assert len(out["definitions"]) == 2
    assert out["match_count"] == 2
    assert out["match_total"] == 5
    assert out["matches_truncated"] is True
    assert out["limit_applied"] == 2


def test_explore_reports_caller_and_callee_totals():
    out = _StubEngine(_explore_graph(match_count=1, caller_count=60)).explore("widget_0", limit=5)

    definition = out["definitions"][0]
    assert len(definition["callers"]) == 50
    assert definition["callers_total"] == 60
    assert definition["callers_truncated"] is True
    assert definition["callees_total"] == 0
    assert definition["callees_truncated"] is False


# ---- freshness: unknown is never rendered as verified-fresh --------------------


@pytest.mark.anyio
async def test_freshness_unknown_when_the_fingerprint_check_cannot_run(tmp_path, monkeypatch):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text(json.dumps({"languages": ["python"]}), encoding="utf-8")
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)

    def boom(self, data):
        raise RuntimeError("git unavailable")

    monkeypatch.setattr("devcouncil.indexing.repo_mapper.RepoMapper.map_is_stale", boom)

    async def produce():
        return mcp_util.json_text({"ok": True})

    out = _parse(await mcp_util.with_codeintel_freshness(tmp_path, produce))

    # Neither fresh nor verified stale: the check raised.
    assert out["stale"] is None
    assert out["fresh"] is None
    assert out["sync"]["state"] == "unknown"
    assert "git unavailable" in out["sync"]["reason"]


@pytest.mark.anyio
async def test_freshness_from_artifact_names_the_unreachable_kernel(tmp_path, monkeypatch):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text(json.dumps({"languages": ["python"]}), encoding="utf-8")
    monkeypatch.setattr("devcouncil.devmap_client.try_connect", lambda root: None)
    monkeypatch.setattr(
        "devcouncil.indexing.repo_mapper.RepoMapper.map_is_stale", lambda self, data: False
    )

    async def produce():
        return mcp_util.json_text({"ok": True})

    out = _parse(await mcp_util.with_codeintel_freshness(tmp_path, produce))

    # The artifact fingerprint check ran and passed, so `stale` stays False —
    # but the payload must say the kernel never answered and which check did.
    assert out["stale"] is False
    assert out["sync"]["state"] == "artifact"
    assert out["sync"]["verified_by"] == "repo_map_fingerprint"
    assert "devmap store" in out["sync"]["kernel_unavailable"]


class _SearchStore(_Store):
    def __init__(self, rows):
        self._rows = rows

    def search(self, query, *, limit=50):  # noqa: ARG002 - stub
        return self._rows[:limit]


class _SearchEngine(_StubEngine):
    def __init__(self, rows):
        super().__init__(CodeGraph(nodes=[], edges=[]))
        store = _SearchStore(rows)
        self.service = type("S", (), {"store": store, "cached_query": staticmethod(
            lambda kind, key, produce: produce()
        )})()


def test_search_capped_rows_are_not_reported_as_complete():
    out = _SearchEngine([{"id": f"n{i}"} for i in range(10)]).search("n", limit=3)

    assert len(out["matches"]) == 3
    assert out["shown"] == 3
    assert out["truncated"] is True
    # The store applies the limit in SQL, so the unfiltered count is unmeasured.
    assert out["total"] is None
    assert out["total_reason"]


def test_search_under_the_cap_reports_a_real_total():
    out = _SearchEngine([{"id": "n0"}]).search("n", limit=50)

    assert out["shown"] == 1
    assert out["total"] == 1
    assert out["truncated"] is False
