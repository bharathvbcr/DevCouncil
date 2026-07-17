"""Stable gap IDs and normalization tests."""

from __future__ import annotations

from devcouncil.domain.gap import Gap
from devcouncil.verification.gap_ids import gap_identity, normalize_verify_gaps, stable_gap_id


def test_stable_gap_id_is_deterministic():
    a = stable_gap_id("TASK-1", "DEAD", "src/a.py:10:foo")
    b = stable_gap_id("TASK-1", "DEAD", "src/a.py:10:foo")
    c = stable_gap_id("TASK-1", "DEAD", "src/a.py:11:foo")
    assert a == b
    assert a != c
    assert a.startswith("GAP-TASK-1-")


def test_normalize_verify_gaps_dedupes_and_sorts():
    g1 = Gap(
        id=stable_gap_id("T", "DEAD", "src/a.py:1:x"),
        severity="medium",
        gap_type="dead_symbol",
        task_id="T",
        description="symbol x unused",
        recommended_fix="wire it",
        blocking=False,
        file="src/a.py",
        line=1,
    )
    g2 = g1.model_copy(update={"id": "other-id", "severity": "high", "blocking": True})
    g3 = Gap(
        id=stable_gap_id("T", "STUB", "src/b.py:2"),
        severity="low",
        gap_type="stub_detected",
        task_id="T",
        description="stub at b",
        recommended_fix="fix",
        blocking=False,
        file="src/b.py",
        line=2,
    )
    out = normalize_verify_gaps([g3, g1, g2])
    assert len(out) == 2
    assert out[0].blocking is True
    assert out[0].gap_type == "dead_symbol"
    assert [g.gap_type for g in out] == ["dead_symbol", "stub_detected"]


def test_gap_identity_ignores_unstable_id():
    base = dict(
        severity="medium",
        gap_type="orphan_diff",
        task_id="T",
        description="File src/x.py was modified but not planned for this task.",
        recommended_fix="revert",
        blocking=True,
        file="src/x.py",
    )
    a = Gap(id="GAP-A", **base)
    b = Gap(id="GAP-B", **base)
    assert gap_identity(a) == gap_identity(b)
