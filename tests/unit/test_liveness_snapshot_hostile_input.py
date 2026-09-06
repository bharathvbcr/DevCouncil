"""Adversarial input for the W3.1 kernel-backed liveness snapshot.

The snapshot is written once, at checkout, and every later verification diffs
against it. A snapshot that absorbs a malformed manifest and still marks itself
``complete`` poisons every subsequent run — silently, because the failure is a
*missing* regression rather than a spurious one.

So the property under test throughout is the conservative direction: whatever
arrives, the answer is either a usable snapshot or no snapshot, never a
confident wrong one.
"""

from __future__ import annotations

import json

import pytest

from devcouncil.verification.checks.liveness_ratchet import (
    kernel_liveness_snapshot,
    load_liveness_baseline,
    snapshot_liveness_baseline,
)

COMPLETE = {
    "entry_roots": ["pyproject.toml"],
    "unwired_candidates": [],
    "unreachable_files": [],
    "dead_symbol_candidates": [],
    "liveness_unreachable_unreliable": False,
    "liveness_meta": {
        "dead_symbol": {"shown": 0, "total": 0, "truncated": False},
        "unwired": {"shown": 0, "total": 0, "truncated": False},
    },
    "dead_clusters_truncated": 0,
    "generated_head": "abc123",
    "resolution_rate": {"net_permille": 500},
}


@pytest.fixture
def manifest(monkeypatch):
    """Drive `kernel_liveness_snapshot` from a manifest under test control."""
    state = {"payload": dict(COMPLETE)}

    class FakeClient:
        def __init__(self, _root):
            pass

        def manifest(self):
            payload = state["payload"]
            if isinstance(payload, Exception):
                raise payload
            return payload

    monkeypatch.setattr("devcouncil.devmap_client.DevMapClient", FakeClient)
    monkeypatch.setattr(
        "devcouncil.verification.checks.liveness_ratchet._symbol_index_from_graph",
        lambda _root, _head: (["pkg/mod.py::thing"], True),
    )
    return state


def test_a_well_formed_manifest_produces_a_usable_snapshot(manifest):
    """The ON direction, without which every test below is satisfied by None."""
    snap = kernel_liveness_snapshot("/nowhere")
    assert snap is not None
    assert snap["engine"] == "devmap_rust"
    assert snap["truncated_lists"] == []
    assert snap["net_resolution_permille"] == 500


@pytest.mark.parametrize(
    "payload",
    [
        None,
        [],
        "a string",
        42,
        {},
    ],
)
def test_a_manifest_that_is_not_a_mapping_yields_nothing(manifest, payload):
    manifest["payload"] = payload
    snap = kernel_liveness_snapshot("/nowhere")
    # `{}` is a mapping and produces an empty-but-honest snapshot; everything
    # else is refused outright. Either way nothing claims to be complete.
    if snap is not None:
        assert snap["entry_roots"] == []
        assert "dead_symbol_candidates" in snap["truncated_lists"]


@pytest.mark.parametrize(
    "meta",
    [
        None,
        "not a mapping",
        {},
        {"dead_symbol": None},
        {"dead_symbol": {"shown": "x", "total": "y"}},
        {"dead_symbol": {"shown": 5}},           # total missing
        {"dead_symbol": {"shown": 5, "total": 9}},  # shown < total
    ],
)
def test_unreadable_truncation_metadata_is_treated_as_truncated(manifest, meta):
    """A cut that cannot be measured is not a cut that did not happen.

    Reading unmeasurable metadata as "nothing was truncated" is what would let a
    capped sample become a ratchet baseline, and diffing two capped samples
    manufactures regressions from wherever the two cuts fell.
    """
    payload = dict(COMPLETE)
    payload["liveness_meta"] = meta
    manifest["payload"] = payload
    snap = kernel_liveness_snapshot("/nowhere")
    assert snap is not None
    assert "dead_symbol_candidates" in snap["truncated_lists"], meta


def test_a_non_integer_resolution_rate_is_dropped_not_coerced(manifest):
    """A rate that cannot be read must be absent, never zero.

    `detect_liveness_regressions` compares `int` to `int`; a coerced 0 would
    make the very next verification report a catastrophic resolution collapse
    that never happened.
    """
    for bad in [None, "500", 5.0, {"net_permille": 500}, [], True]:
        payload = dict(COMPLETE)
        payload["resolution_rate"] = {"net_permille": bad}
        manifest["payload"] = payload
        snap = kernel_liveness_snapshot("/nowhere")
        assert snap is not None
        assert snap["net_resolution_permille"] is None or isinstance(
            snap["net_resolution_permille"], int
        ), bad
        # `True` is an `int` in Python; the guard must not let it through as a
        # rate of 1 permille.
        if bad is True:
            assert snap["net_resolution_permille"] != 1 or snap["net_resolution_permille"] is None


def test_an_exception_from_the_kernel_yields_no_snapshot(manifest):
    from devcouncil.devmap_client import DevMapClientError

    for error in [
        DevMapClientError("store unreachable"),
        OSError("disk gone"),
        ValueError("garbage"),
        RuntimeError("unexpected"),
    ]:
        manifest["payload"] = error
        assert kernel_liveness_snapshot("/nowhere") is None, error


def test_a_hostile_snapshot_never_becomes_a_complete_baseline(manifest, tmp_path):
    """The property the whole file exists for.

    A baseline marked `complete` is diffed against on every later run. One built
    from a manifest nobody could read would report the difference between two
    unrelated measurements as this task's regressions.
    """
    hostile = [
        {**COMPLETE, "entry_roots": []},
        {**COMPLETE, "liveness_unreachable_unreliable": True},
        {**COMPLETE, "liveness_meta": {"unwired": {"shown": 200, "total": 4096}}},
        {**COMPLETE, "dead_clusters_truncated": 7},
        {**COMPLETE, "liveness_meta": "garbage"},
    ]
    for index, payload in enumerate(hostile):
        manifest["payload"] = payload
        task_id = f"TASK-{index}"
        assert snapshot_liveness_baseline(tmp_path, task_id) is None, payload
        assert load_liveness_baseline(tmp_path, task_id) is None, payload
        written = tmp_path / ".devcouncil" / "liveness_baseline" / f"{task_id}.json"
        if written.is_file():
            assert json.loads(written.read_text())["complete"] is False
