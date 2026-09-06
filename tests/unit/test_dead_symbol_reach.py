"""W3.2 — the dead-symbol gate cannot fail open without saying it failed.

Three defects, each pinned in both directions below.

**A boolean could not carry the answer.** The gate asked ``impact`` and folded
every failure into "no inbound edge", so a kernel outage and a genuinely
unreferenced symbol produced the same value. :class:`SymbolReach` has three
states and refuses to be truthiness-tested, so the fold is no longer spellable.

**An unconfirmed finding blocked like a confirmed one.** Only the evidence list
differed: same severity, same ``blocking``, same flat sentence — "is never
referenced" — over a check that never ran.

**The whole gate degraded to ``return []`` at debug level.** A check that
silently returns nothing reports what a check that ran and found nothing
reports.
"""

from __future__ import annotations

import logging

import pytest

from devcouncil.devmap_client import (
    REACH_UNKNOWN,
    REACHED,
    UNREACHED,
    BudgetedResponse,
    DevMapClient,
    DevMapClientError,
    SymbolReach,
)
from devcouncil.domain.task import Task
from devcouncil.verification.checks.dead_symbols import detect_dead_symbol_gaps

BODY = "def lonely_helper():\n    return 1\n"
PATH = "mod.py"


def _gap_id(task_id: str, kind: str) -> str:
    return f"{task_id}-{kind}-1"


def _diff(path: str, body: str) -> str:
    lines = body.splitlines()
    hunk = "\n".join(f"+{ln}" for ln in lines)
    return (
        f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
        f"@@ -0,0 +1,{len(lines)} @@\n{hunk}\n"
    )


def _task() -> Task:
    return Task(
        id="TASK-1",
        title="t",
        description="d",
        planned_files=[],
        difficulty="hard",  # type: ignore[arg-type]
    )


def _response(**overrides) -> BudgetedResponse:
    base = dict(
        shown=0,
        hidden=0,
        total=0,
        truncated=False,
        tokens_used=0,
        items=[],
        resolution="Available",
    )
    base.update(overrides)
    return BudgetedResponse(**base)  # type: ignore[arg-type]


def _inbound(source_file: str) -> dict:
    return {
        "source_file": source_file,
        "source_symbol": "caller",
        "target_file": PATH,
        "target_symbol": "lonely_helper",
    }


def _run(tmp_path, monkeypatch, impact, *, blocking: bool = True):
    (tmp_path / PATH).write_text(BODY, encoding="utf-8")
    monkeypatch.setattr("devcouncil.devmap_client.DevMapClient.impact", impact)
    # The `code_graph.json` fallback would otherwise answer for the store and
    # make every result below untraceable to the branch under test.
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.symbol_has_non_test_inbound",
        lambda *_args, **_kwargs: False,
    )
    return detect_dead_symbol_gaps(
        task=_task(),
        project_root=tmp_path,
        diff_content=_diff(PATH, BODY),
        next_gap_id=_gap_id,
        dead_symbol_blocking=blocking,
    )


def _dead(gaps):
    return [g for g in gaps if g.gap_type == "dead_symbol"]


# --------------------------------------------------------------------------
# The tri-state itself
# --------------------------------------------------------------------------


def test_symbol_reach_refuses_to_be_truthiness_tested():
    """The mistake the type exists to prevent must fail loudly.

    ``if reach:`` would resolve ``unknown`` to falsy — the exact fold this work
    order removes — so the type does not answer that question at all.
    """
    for verdict in (REACHED, UNREACHED, REACH_UNKNOWN):
        with pytest.raises(TypeError, match="three states"):
            bool(SymbolReach(verdict, "x"))


def test_a_non_test_caller_is_reached(tmp_path, monkeypatch):
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(
        DevMapClient, "impact", lambda *_a, **_k: _response(shown=1, total=1, items=[_inbound("caller.py")])
    )
    reach = client.symbol_is_reached(PATH, "lonely_helper")
    assert reach.verdict == REACHED
    assert "caller.py" in reach.detail


def test_a_test_only_caller_is_not_reached(tmp_path, monkeypatch):
    """The OFF direction for the test-path filter.

    Without it, a symbol used only by its own test would clear the gate, which
    is the case the gate exists for.
    """
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(
        DevMapClient,
        "impact",
        lambda *_a, **_k: _response(shown=1, total=1, items=[_inbound("tests/test_mod.py")]),
    )
    assert client.symbol_is_reached(PATH, "lonely_helper").verdict == UNREACHED


def test_a_completed_walk_with_no_caller_is_unreached(tmp_path, monkeypatch):
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(DevMapClient, "impact", lambda *_a, **_k: _response())
    assert client.symbol_is_reached(PATH, "lonely_helper").verdict == UNREACHED


@pytest.mark.parametrize(
    "overrides, expected_in_detail",
    [
        # The one that was previously invisible. A depth- or node-capped walk
        # that found no caller has searched what it searched, which is not the
        # same statement as "there is none" — and it is the statement a
        # delete-this verdict would rest on.
        ({"walk_incomplete": "traversal stopped at depth 1"}, "stopped early"),
        ({"truncated": True, "shown": 5, "total": 90}, "truncated at 5 of 90"),
        ({"resolution": {"Unavailable": {"reason": "mod.py is not indexed"}}}, "not indexed"),
    ],
)
def test_an_unfinished_walk_is_unknown_not_unreached(
    tmp_path, monkeypatch, overrides, expected_in_detail
):
    client = DevMapClient(tmp_path)
    monkeypatch.setattr(DevMapClient, "impact", lambda *_a, **_k: _response(**overrides))
    reach = client.symbol_is_reached(PATH, "lonely_helper")
    assert reach.verdict == REACH_UNKNOWN, overrides
    assert expected_in_detail in reach.detail


def test_a_kernel_outage_is_unknown_not_unreached(tmp_path, monkeypatch):
    client = DevMapClient(tmp_path)

    def boom(*_args, **_kwargs):
        raise DevMapClientError("no devmap store")

    monkeypatch.setattr(DevMapClient, "impact", boom)
    reach = client.symbol_is_reached(PATH, "lonely_helper")
    assert reach.verdict == REACH_UNKNOWN
    assert "no devmap store" in reach.detail


# --------------------------------------------------------------------------
# What the gate does with each verdict
# --------------------------------------------------------------------------


def test_a_confirmed_finding_blocks(tmp_path, monkeypatch):
    """ON: the graph looked, found nothing, and the gate says so with weight."""
    gaps = _dead(_run(tmp_path, monkeypatch, lambda *_a, **_k: _response()))
    assert len(gaps) == 1
    assert gaps[0].blocking
    assert gaps[0].severity == "high"
    assert "is never referenced" in gaps[0].description
    assert "graph-confirmation:unavailable" not in gaps[0].evidence


def test_an_unconfirmed_finding_is_reported_but_does_not_block(tmp_path, monkeypatch):
    """OFF: the graph could not look, and every axis of the report says so.

    Previously only ``evidence`` differed, so a finding whose strongest check
    never ran blocked a task with the same severity and the same flat assertion
    as one the graph confirmed.
    """

    def boom(*_args, **_kwargs):
        raise DevMapClientError("kernel down")

    gaps = _dead(_run(tmp_path, monkeypatch, boom))
    assert len(gaps) == 1, "the token scan still ran, so the finding is not withdrawn"
    gap = gaps[0]
    assert not gap.blocking
    assert gap.severity == "low"
    assert "could not confirm" in gap.description
    assert "is never referenced" not in gap.description
    assert "graph-confirmation:unavailable" in gap.evidence


def test_a_capped_walk_does_not_block_either(tmp_path, monkeypatch):
    """The regression this work order is really about.

    A walk that stopped early used to return no matching edge and be read as
    "never referenced" — a blocking delete-this verdict built on a check that
    ran out of budget.
    """
    gaps = _dead(
        _run(
            tmp_path,
            monkeypatch,
            lambda *_a, **_k: _response(walk_incomplete="node cap reached"),
        )
    )
    assert len(gaps) == 1
    assert not gaps[0].blocking
    assert "could not confirm" in gaps[0].description


def test_a_reached_symbol_produces_no_gap_at_all(tmp_path, monkeypatch):
    gaps = _dead(
        _run(
            tmp_path,
            monkeypatch,
            lambda *_a, **_k: _response(shown=1, total=1, items=[_inbound("caller.py")]),
        )
    )
    assert gaps == []


def test_the_json_fallback_can_clear_but_never_confirm(tmp_path, monkeypatch):
    """A stale artifact's silence is not evidence.

    ``code_graph.json`` may predate the diff under check, so a symbol added five
    minutes ago is absent from it for reasons that have nothing to do with being
    dead. It may therefore only *clear* a symbol — the direction a stale graph
    cannot get wrong.
    """
    (tmp_path / PATH).write_text(BODY, encoding="utf-8")

    def boom(*_args, **_kwargs):
        raise DevMapClientError("store unreachable")

    monkeypatch.setattr(DevMapClient, "impact", boom)
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.symbol_has_non_test_inbound",
        lambda *_args, **_kwargs: True,
    )
    cleared = _dead(
        detect_dead_symbol_gaps(
            task=_task(),
            project_root=tmp_path,
            diff_content=_diff(PATH, BODY),
            next_gap_id=_gap_id,
            dead_symbol_blocking=True,
        )
    )
    assert cleared == [], "an inbound edge in the kernel's own artifact clears the symbol"

    # The other direction: the fallback saying "no inbound" must not upgrade the
    # verdict to confirmed, because it cannot distinguish "absent" from "new".
    monkeypatch.setattr(
        "devcouncil.indexing.graph.query.symbol_has_non_test_inbound",
        lambda *_args, **_kwargs: False,
    )
    unconfirmed = _dead(
        detect_dead_symbol_gaps(
            task=_task(),
            project_root=tmp_path,
            diff_content=_diff(PATH, BODY),
            next_gap_id=_gap_id,
            dead_symbol_blocking=True,
        )
    )
    assert len(unconfirmed) == 1
    assert not unconfirmed[0].blocking, (
        "a stale graph's silence must not promote an unconfirmed finding to a "
        "blocking one"
    )


# --------------------------------------------------------------------------
# The outermost guard
# --------------------------------------------------------------------------


def test_a_gate_that_degrades_says_so_at_error_level(tmp_path, monkeypatch, caplog):
    """``return []`` is indistinguishable from "ran and found nothing".

    It still degrades — a verification run must not be brought down by this
    check — but at a level somebody reads, naming the task, and saying plainly
    that its silence is not a pass.
    """
    (tmp_path / PATH).write_text(BODY, encoding="utf-8")
    monkeypatch.setattr(
        "devcouncil.verification.checks.dead_symbols.added_lines_by_file",
        lambda _diff: (_ for _ in ()).throw(RuntimeError("parser exploded")),
    )
    with caplog.at_level(logging.ERROR):
        gaps = detect_dead_symbol_gaps(
            task=_task(),
            project_root=tmp_path,
            diff_content=_diff(PATH, BODY),
            next_gap_id=_gap_id,
            dead_symbol_blocking=True,
        )
    errors = [r for r in caplog.records if r.levelno >= logging.ERROR]
    assert errors, "a gate that did not run must not be silent about it"
    assert "TASK-1" in errors[0].getMessage()
    assert "not a pass" in errors[0].getMessage()

    # And in the report, not only the log. `verify_orchestration` carries a
    # `quality_gate_failed` gap for this, reachable only through an exception
    # this function is documented never to raise — so the branch has never
    # fired and the outage never left the log file.
    assert [g.gap_type for g in gaps] == ["quality_gate_failed"]
    assert not gaps[0].blocking, "an outage is not evidence of a defect in the diff"
    assert "not a pass" in gaps[0].description
    assert "gate:dead_symbol" in gaps[0].evidence
    assert not _dead(gaps), "a crashed gate must not emit dead-symbol findings"


def test_the_outage_marker_never_masks_a_real_finding(tmp_path, monkeypatch):
    """OFF direction for the marker: a healthy run emits no gate-failure gap.

    A marker that is always present carries exactly as much information as one
    that is never present.
    """
    gaps = _run(tmp_path, monkeypatch, lambda *_a, **_k: _response())
    assert not any(g.gap_type == "quality_gate_failed" for g in gaps)
