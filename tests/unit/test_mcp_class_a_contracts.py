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

from devcouncil.integrations.mcp.handlers import codeintel as codeintel_handlers
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


# ---- devcouncil_code_explore: counts survive the engine change ----------------
#
# These asserted the same two properties against `CodeIntelQueryEngine.explore`,
# which loaded a whole `CodeGraph` into process memory. That engine is deleted;
# the tool answers from the Rust kernel. The properties are the tool's, not the
# engine's, so they move to the seam that is left — the handler that turns a
# kernel report into the tool's payload. The kernel side is covered by
# `rust-port/crates/devmap-query/tests/explore_and_affected.rs`.


def _kernel_response(items, *, total=None, truncated=None):
    """A kernel `Response` with honest counters."""
    shown = len(items)
    total = shown if total is None else total
    hidden = total - shown
    return {
        "items": items,
        "shown": shown,
        "hidden": hidden,
        "total": total,
        "truncated": hidden > 0 if truncated is None else truncated,
        "tokens_used": 0,
        "resolution": "Available",
    }


class _KernelStub:
    def __init__(self, report):
        self._report = report

    def explore(self, query, limit=20, **_kwargs):
        return self._report

    def status(self):
        import types

        return types.SimpleNamespace(
            generation_id=1,
            pending_count=0,
            node_count=1,
            edge_count=0,
            is_fresh=True,
            degraded_reason=None,
            quarantined_count=0,
            raw={"schema_version": 12, "analyzer_version": "devmap"},
        )


def _explore_report(*, shown_definitions, total_definitions, callers, callers_total,
                    source="def widget(): ...", source_unavailable_reason=None):
    definitions = [
        {
            "id": f"m{index}.py::widget_{index}",
            "symbol_name": f"widget_{index}",
            "qualified_name": f"widget_{index}",
            "file_path": f"m{index}.py",
            "kind": "Function",
            "language": "python",
            "span": [1, 2],
            "source": source,
            "source_unavailable_reason": source_unavailable_reason,
            "score": 1.0,
            "callers": _kernel_response(callers, total=callers_total),
            "callees": _kernel_response([]),
        }
        for index in range(shown_definitions)
    ]
    return {
        "query": "widget_",
        "limit": shown_definitions,
        "definitions": _kernel_response(definitions, total=total_definitions),
        "blast_radius": {
            "seeds": [],
            "unmatched_targets": [],
            "layers": _kernel_response([]),
            "total_impacted": 0,
        },
        "budget": {"total": 8000, "definitions": 4000, "edges_per_direction": 500,
                   "blast_radius": 2000},
    }


def test_explore_reports_match_total_beside_the_cap(tmp_path, monkeypatch):
    report = _explore_report(
        shown_definitions=2, total_definitions=5, callers=[], callers_total=0
    )
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.codeintel.try_connect", lambda _root: _KernelStub(report)
    )

    out = codeintel_handlers._explore_via_client(tmp_path, "widget_", 2)

    assert len(out["definitions"]) == 2
    assert out["match_count"] == 2
    assert out["match_total"] == 5
    assert out["matches_truncated"] is True
    assert out["limit_applied"] == 2


def test_explore_reports_caller_and_callee_totals(tmp_path, monkeypatch):
    report = _explore_report(
        shown_definitions=1,
        total_definitions=1,
        callers=[{"source_symbol": f"c{index}.py::caller_{index}"} for index in range(50)],
        callers_total=60,
    )
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.codeintel.try_connect", lambda _root: _KernelStub(report)
    )

    out = codeintel_handlers._explore_via_client(tmp_path, "widget_0", 5)

    definition = out["definitions"][0]
    assert len(definition["callers"]) == 50
    assert definition["callers_total"] == 60
    assert definition["callers_truncated"] is True
    assert definition["callees_total"] == 0
    assert definition["callees_truncated"] is False


def test_explore_source_that_could_not_be_read_is_not_an_empty_body(tmp_path, monkeypatch):
    """Class A, on the snippet: unread and empty must not share a shape."""
    report = _explore_report(
        shown_definitions=1,
        total_definitions=1,
        callers=[],
        callers_total=0,
        source="",
        source_unavailable_reason="source unavailable at query time for \"m0.py\"",
    )
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.codeintel.try_connect", lambda _root: _KernelStub(report)
    )

    out = codeintel_handlers._explore_via_client(tmp_path, "widget_0", 5)

    definition = out["definitions"][0]
    assert definition["source"] == ""
    assert "m0.py" in definition["source_unavailable_reason"]


def test_explore_without_a_kernel_is_unavailable_not_an_empty_match_set(tmp_path, monkeypatch):
    """An engine that could not run must not answer like one that found nothing."""
    monkeypatch.setattr("devcouncil.integrations.mcp.handlers.codeintel.try_connect", lambda _root: None)

    out = codeintel_handlers._explore_via_client(tmp_path, "widget_", 20)

    assert out["ok"] is False
    assert out["definitions"] == []
    assert out["match_total"] == 0
    assert "Unavailable" in out["resolution"]
    assert out["operation"] == "explore"


def test_affected_tests_without_a_kernel_is_unavailable_not_no_tests(tmp_path, monkeypatch):
    """The costliest possible false negative: "no tests are affected"."""
    monkeypatch.setattr("devcouncil.integrations.mcp.handlers.codeintel.try_connect", lambda _root: None)

    out = codeintel_handlers._affected_via_client(tmp_path, ["widget"], 3)

    assert out["ok"] is False
    assert out["tests"] == []
    assert "Unavailable" in out["resolution"]
    assert out["operation"] == "affected_tests"


# ---- freshness: unknown is never rendered as verified-fresh --------------------


@pytest.mark.anyio
async def test_freshness_unknown_when_the_fingerprint_check_cannot_run(tmp_path, monkeypatch):
    dev = tmp_path / ".devcouncil"
    dev.mkdir()
    (dev / "repo_map.json").write_text(json.dumps({"languages": ["python"]}), encoding="utf-8")
    monkeypatch.setattr("devcouncil.integrations.mcp.handlers.codeintel.try_connect", lambda root: None)

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
    monkeypatch.setattr("devcouncil.integrations.mcp.handlers.codeintel.try_connect", lambda root: None)
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


class _SearchKernelStub(_KernelStub):
    """A kernel whose search measures the corpus, not just the page it returned."""

    def __init__(self, rows, total):
        super().__init__({})
        self._rows = rows
        self._total = total

    def search(self, query, limit=2000, semantic=False):  # noqa: ARG002 - stub
        from devcouncil.devmap_client import BudgetedResponse

        return BudgetedResponse(
            shown=len(self._rows),
            hidden=self._total - len(self._rows),
            total=self._total,
            truncated=self._total > len(self._rows),
            tokens_used=0,
            items=self._rows,
            resolution="Available",
        )


def test_search_capped_rows_are_not_reported_as_complete(tmp_path, monkeypatch):
    """A capped page carries the corpus-wide total, not the page's length.

    The Python engine could not do this: its store applied the limit inside SQL,
    so it reported ``total: None`` with a ``total_reason`` explaining that the
    number had never been measured. That was the honest answer available to it.
    The kernel counts the match set with ``count_search_symbols`` before
    paging, so the same surface now reports a measured total — an upgrade from
    "not counted" to a count, with the capping still declared.
    """
    rows = [{"symbol_name": f"n{index}", "file_path": "a.py", "span": [1, 1]}
            for index in range(10)]
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.codeintel.try_connect",
        lambda _root: _SearchKernelStub(rows, total=42),
    )

    out = codeintel_handlers._search_via_client(tmp_path, "n", 3)

    assert len(out["matches"]) == 3
    assert out["shown"] == 3
    assert out["truncated"] is True
    assert out["total"] == 42


def test_search_under_the_cap_reports_a_real_total(tmp_path, monkeypatch):
    monkeypatch.setattr(
        "devcouncil.integrations.mcp.handlers.codeintel.try_connect",
        lambda _root: _SearchKernelStub(
            [{"symbol_name": "n0", "file_path": "a.py", "span": [1, 1]}], total=1
        ),
    )

    out = codeintel_handlers._search_via_client(tmp_path, "n", 50)

    assert out["shown"] == 1
    assert out["total"] == 1
    assert out["truncated"] is False
