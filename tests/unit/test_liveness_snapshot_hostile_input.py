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
        "devcouncil.verification.checks.liveness_ratchet._graph_liveness",
        lambda _root, _head: {
            "entry_roots": ["pyproject.toml"],
            "unwired_candidates": [],
            "unreachable_files": [],
            "dead_symbol_candidates": [],
            "symbol_index": ["pkg/mod.py::thing"],
            "liveness_unreachable_unreliable": False,
            "generated_head": "abc123",
        },
    )
    return state


def test_a_well_formed_manifest_produces_a_usable_snapshot(manifest):
    """The ON direction, without which every test below is satisfied by None."""
    snap = kernel_liveness_snapshot("/nowhere")
    assert snap is not None
    assert snap["engine"] == "devmap_rust"
    assert snap["truncated_lists"] == []
    assert snap["net_resolution_permille"] == 500


@pytest.mark.parametrize("payload", [None, [], "a string", 42, {}, {"generated_head": ""}])
def test_an_unusable_manifest_yields_nothing(manifest, payload):
    """Including the empty one.

    `{}` is a mapping, so it survives the type check — but it carries no
    `generated_head`, and without one the graph beside it cannot be shown to
    describe the same generation. Pairing lists from one generation with a rate
    from another turns the difference between two builds into a "regression".
    """
    manifest["payload"] = payload
    assert kernel_liveness_snapshot("/nowhere") is None, payload


def test_a_graph_that_cannot_be_used_yields_no_snapshot(manifest, monkeypatch):
    """The lists come from the graph, so an unusable graph means no snapshot.

    The manifest caps `unwired_candidates` and `dead_symbol_candidates` at 200
    and publishes the real totals beside them; the graph carries all of them.
    Falling back to the manifest when the graph is unusable would silently hand
    the ratchet two capped samples to diff, and diffing two samples manufactures
    regressions from wherever the cuts fell. Refusing is the honest answer.
    """
    monkeypatch.setattr(
        "devcouncil.verification.checks.liveness_ratchet._graph_liveness",
        lambda _root, _head: None,
    )
    assert kernel_liveness_snapshot("/nowhere") is None


def test_an_unreliable_reachability_verdict_still_produces_a_snapshot(manifest):
    """It suppresses the unreachable half of the diff, not the whole baseline.

    Since W1.1 made it a computed verdict, `liveness_unreachable_unreliable` is
    true on any corpus with a parse failure or an import-blind language — most
    real ones. Gating completeness on it threw away the unwired and dead-symbol
    halves too, which the flag says nothing about, and no baseline was written
    at all. `_liveness_roots_unreliable` already suppresses exactly the half the
    flag describes, at diff time.
    """
    payload = dict(COMPLETE)
    payload["liveness_unreachable_unreliable"] = True
    manifest["payload"] = payload
    snap = kernel_liveness_snapshot("/nowhere")
    assert snap is not None
    # The flag rides on the snapshot so the diff can act on it.
    assert "liveness_unreachable_unreliable" in snap


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


def test_a_hostile_snapshot_never_becomes_a_complete_baseline(manifest, monkeypatch, tmp_path):
    """The property the whole file exists for.

    A baseline marked `complete` is diffed against on every later run. One built
    from something nobody could read would report the difference between two
    unrelated measurements as this task's regressions.

    The lists now come from the graph, so the hostile inputs are the graph's.
    """
    hostile = [
        # No entry roots: every file looks unreachable, so the diff would flood
        # or, after a fail-soft empty list, falsely look clean.
        {"entry_roots": [], "symbol_index": ["a::b"]},
        # No symbol index: the symbol half of the diff would refuse to flag
        # anything, which is a check that cannot execute rather than one that
        # passed.
        None,
    ]
    for index, graph in enumerate(hostile):
        if graph is None:
            monkeypatch.setattr(
                "devcouncil.verification.checks.liveness_ratchet._graph_liveness",
                lambda _root, _head: None,
            )
        else:
            payload = {
                "unwired_candidates": [],
                "unreachable_files": [],
                "dead_symbol_candidates": [],
                "liveness_unreachable_unreliable": False,
                "generated_head": "abc123",
                **graph,
            }
            monkeypatch.setattr(
                "devcouncil.verification.checks.liveness_ratchet._graph_liveness",
                lambda _root, _head, _p=payload: _p,
            )
        task_id = f"TASK-{index}"
        assert snapshot_liveness_baseline(tmp_path, task_id) is None, graph
        assert load_liveness_baseline(tmp_path, task_id) is None, graph
        written = tmp_path / ".devcouncil" / "liveness_baseline" / f"{task_id}.json"
        if written.is_file():
            assert json.loads(written.read_text())["complete"] is False
