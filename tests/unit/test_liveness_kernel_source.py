"""W3.1 — the liveness ratchet measures the engine that ships.

``snapshot_liveness_baseline`` called ``RepoMapper.liveness_snapshot``, which
ran the Python ``_compute_liveness`` and a regex token scanner. Everything else
in the system reads the Rust kernel, so the ratchet compared a Python baseline
against a Python current on a code path nothing else used, and both differed
from what ``dev map dead`` reports — a regression in it did not mean what a user
would see.

These tests drive a **real** ``devmap`` build in a temporary repository, because
the defect was precisely that the mocked-out path and the shipped path were
different code. They skip rather than fail when no capable kernel is on this
machine: a missing binary is not evidence the rebase is wrong.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import pytest

from devcouncil.verification.checks.liveness_ratchet import (
    LIVENESS_SCHEMA_VERSION,
    detect_liveness_regressions,
    kernel_liveness_snapshot,
    load_liveness_baseline,
    snapshot_liveness_baseline,
)

REPO_ROOT = Path(__file__).resolve().parents[2]


@pytest.fixture
def kernel_binary(monkeypatch):
    """Pin the client to a capable kernel, or skip.

    ``find_engine_binary`` searches ``<root>/rust-port/target`` first, and the
    root here is a ``tmp_path`` that has none — so without this the tests would
    silently fall through to whatever ``devmap`` sits on ``PATH``, which on a
    developer machine is routinely an older ``~/.cargo/bin`` build. That is how
    a test comes to report the machine instead of the code.
    """
    from devcouncil.devmap_engine import BINARY_ENV_VAR, DevMapEngineError, find_engine_binary

    try:
        binary = find_engine_binary(REPO_ROOT)
    except DevMapEngineError as exc:
        pytest.skip(f"no capable devmap kernel available: {exc}")
    monkeypatch.setenv(BINARY_ENV_VAR, binary)
    return binary


def _repo(root: Path) -> None:
    """A tiny corpus with a declared entry root and one genuinely stranded module."""
    (root / "pkg").mkdir(parents=True, exist_ok=True)
    (root / "pkg" / "__init__.py").write_text("", encoding="utf-8")
    (root / "pkg" / "cli.py").write_text(
        "from pkg.lib import helper\n\n\ndef main():\n    return helper()\n", encoding="utf-8"
    )
    (root / "pkg" / "lib.py").write_text("def helper():\n    return 1\n", encoding="utf-8")
    (root / "pkg" / "orphan.py").write_text("def nobody_calls_me():\n    return 2\n", encoding="utf-8")
    (root / "pyproject.toml").write_text(
        '[project]\nname="x"\nversion="0"\n[project.scripts]\ncli="pkg.cli:main"\n',
        encoding="utf-8",
    )
    subprocess.run(["git", "init", "-q"], cwd=root, check=True, capture_output=True)
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"],
        cwd=root, check=True, capture_output=True,
    )
    subprocess.run(
        ["git", "-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"],
        cwd=root, check=True, capture_output=True,
    )
    subprocess.run(["devmap", "build", "."], cwd=root, check=False, capture_output=True)


def test_the_snapshot_comes_from_the_kernel_not_the_python_scanner(tmp_path, kernel_binary):
    """The claim the whole work order rests on, asserted on the artifact itself."""
    _repo(tmp_path)
    snap = kernel_liveness_snapshot(tmp_path)
    assert snap is not None, "the kernel could not answer for a freshly built corpus"
    assert snap["engine"] == "devmap_rust"
    assert snap["generated_head"], "the snapshot must name the generation it measured"
    # Every key the ratchet diffs must be present — a missing one would read as
    # an empty list, which is "nothing was stranded".
    for key in (
        "entry_roots",
        "unwired_candidates",
        "unreachable_files",
        "dead_symbol_candidates",
        "symbol_index",
    ):
        assert key in snap, key


def test_the_symbol_index_is_the_kernels_own_symbols(tmp_path, kernel_binary):
    """Without an index the symbol half of the diff refuses to flag anything.

    That was the risk in moving off the Python scanner: the index it produced
    had no obvious kernel counterpart, and an empty one turns the symbol half
    into a branch that cannot execute — the same defect this work order removes
    from the unreachable half.
    """
    _repo(tmp_path)
    snap = kernel_liveness_snapshot(tmp_path)
    assert snap is not None
    index = snap["symbol_index"]
    assert index, "an empty symbol index silently disables the symbol ratchet"
    assert all("::" in entry for entry in index), index[:5]
    assert any(entry.endswith("::helper") for entry in index), (
        "the index must name real symbols from the corpus"
    )
    assert "symbol_index" not in snap["truncated_lists"]


def test_the_baseline_records_the_engine_and_the_new_schema(tmp_path, kernel_binary):
    _repo(tmp_path)
    out = snapshot_liveness_baseline(tmp_path, "TASK-1")
    assert out is not None, "a freshly built corpus must produce a usable baseline"
    payload = json.loads(out.read_text(encoding="utf-8"))
    assert payload["source"] == "kernel_manifest"
    assert payload["engine"] == "devmap_rust"
    assert payload["schema_version"] == LIVENESS_SCHEMA_VERSION == 2
    assert payload["complete"] is True
    assert payload["truncated_lists"] == []


def test_a_kernel_that_cannot_answer_writes_no_baseline(tmp_path, monkeypatch):
    """The direction that matters most.

    An empty baseline reads as "nothing was stranded" and would clear every
    later diff — a ratchet that silently stops ratcheting. Declining to write
    one leaves the ratchet visibly absent instead.
    """
    monkeypatch.setattr(
        "devcouncil.verification.checks.liveness_ratchet.kernel_liveness_snapshot",
        lambda _root: None,
    )
    assert snapshot_liveness_baseline(tmp_path, "TASK-1") is None
    assert load_liveness_baseline(tmp_path, "TASK-1") is None


@pytest.mark.parametrize("cut", ["dead_symbol_candidates", "unwired_candidates", "symbol_index"])
def test_a_capped_list_is_not_a_ratchet_input(tmp_path, monkeypatch, cut):
    """Diffing two capped samples manufactures regressions.

    ``unwired_candidates`` and ``dead_symbol_candidates`` are capped at 200 in
    the manifest and published beside their real totals. A file inside the
    baseline's 200 and outside the current 200 reads as "newly stranded" when
    nothing about it changed — a capped sample presented as complete coverage.
    """
    monkeypatch.setattr(
        "devcouncil.verification.checks.liveness_ratchet.kernel_liveness_snapshot",
        lambda _root: {
            "entry_roots": ["pyproject.toml"],
            "unwired_candidates": ["pkg/orphan.py"],
            "unreachable_files": [],
            "dead_symbol_candidates": [],
            "symbol_index": ["pkg/lib.py::helper"],
            "liveness_unreachable_unreliable": False,
            "truncated_lists": [cut],
            "net_resolution_permille": 500,
            "generated_head": "abc",
            "engine": "devmap_rust",
        },
    )
    assert snapshot_liveness_baseline(tmp_path, "TASK-1") is None
    # Written to disk for a reader, but marked incomplete so the diff skips it.
    written = tmp_path / ".devcouncil" / "liveness_baseline" / "TASK-1.json"
    assert written.is_file()
    payload = json.loads(written.read_text(encoding="utf-8"))
    assert payload["complete"] is False
    assert payload["truncated_lists"] == [cut]
    assert load_liveness_baseline(tmp_path, "TASK-1") is None


# `_graph_liveness` reads `code_graph.json` — the kernel's own export — with a
# plain bounded `json.load`. It used to go through
# `indexing.graph.build.load_code_graph`, which imports that same JSON into the
# Python `index.sqlite` cache and re-materialises every node and edge as
# pydantic models. Measured on a tmp copy of this repository's tree (17,505
# symbols, 120 unwired candidates) the two produce byte-identical lists, at
# p50 815.0 ms / min 784.9 ms against p50 133.8 ms / min 128.0 ms.
#
# So the fixtures below are the artifact itself rather than a fake model.


def _write_graph(root: Path, *, head, tier=None, nodes=None, **extra):
    meta: dict = {"legacy_dead_symbol_candidates": []}
    if tier is not None:
        meta["compatibility_export_tier"] = tier
    payload = {
        "generated_head": head,
        "meta": meta,
        "nodes": (
            [{"path": "pkg/lib.py", "name": "helper"}] if nodes is None else nodes
        ),
        "edges": [],
        "entry_roots": [],
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_code": [],
        **extra,
    }
    path = root / ".devcouncil" / "graph" / "code_graph.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def _index(tmp_path, head, *, graph_head=None, tier=None, nodes=None):
    from devcouncil.verification.checks import liveness_ratchet

    _write_graph(tmp_path, head=graph_head if graph_head is not None else head,
                 tier=tier, nodes=nodes)
    graph = liveness_ratchet._graph_liveness(tmp_path, head)
    return (graph["symbol_index"] if graph else [], graph is not None)


def test_graph_liveness_never_touches_the_python_store(tmp_path, monkeypatch):
    """The lists come from the kernel's JSON export, not from `index.sqlite`.

    `load_code_graph` is monkeypatched to raise: reaching it at all is the
    defect, and it is also what wrote a 102 MB `index.sqlite` the first time a
    ratchet ran on a fresh checkout.
    """
    from devcouncil.verification.checks import liveness_ratchet

    monkeypatch.setattr(
        "devcouncil.indexing.graph.build.load_code_graph",
        lambda _root: (_ for _ in ()).throw(
            AssertionError("the ratchet must not read the Python graph store")
        ),
    )
    _write_graph(tmp_path, head="h", unwired_candidates=["pkg/orphan.py"])

    got = liveness_ratchet._graph_liveness(tmp_path, "h")

    assert got is not None, "the kernel's own export must be readable on its own"
    assert got["symbol_index"] == ["pkg/lib.py::helper"]
    assert got["unwired_candidates"] == ["pkg/orphan.py"]
    assert (tmp_path / ".devcouncil" / "codeintel" / "index.sqlite").exists() is False


def test_a_missing_graph_export_is_not_an_empty_snapshot(tmp_path):
    """No export is "could not look" — an empty snapshot clears the ratchet."""
    from devcouncil.verification.checks import liveness_ratchet

    assert liveness_ratchet._graph_liveness(tmp_path, "h") is None


def test_a_graph_export_that_is_not_json_is_not_a_snapshot(tmp_path):
    from devcouncil.verification.checks import liveness_ratchet

    path = tmp_path / ".devcouncil" / "graph" / "code_graph.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("{not json", encoding="utf-8")
    assert liveness_ratchet._graph_liveness(tmp_path, "h") is None


def test_an_oversized_graph_export_is_refused_rather_than_read(tmp_path, monkeypatch):
    """The read is bounded, as the import it replaces was.

    An unbounded `json.load` of an artifact this process did not write is how a
    verification check becomes the thing that exhausts the machine.
    """
    from devcouncil.verification.checks import liveness_ratchet

    _write_graph(tmp_path, head="h")
    monkeypatch.setattr(liveness_ratchet, "_graph_json_max_bytes", lambda root: 8)
    assert liveness_ratchet._graph_liveness(tmp_path, "h") is None


def test_a_graph_from_another_generation_is_not_a_symbol_index(tmp_path):
    """The straddle guard.

    Pairing a symbol index from one generation with candidate lists from
    another turns the difference between two generations into a "regression" —
    the exact shape of a false stranding report.
    """
    index, usable = _index(tmp_path, "newer", graph_head="older")
    assert not usable and index == []

    index, usable = _index(tmp_path, "same")
    assert usable and index == ["pkg/lib.py::helper"]


@pytest.mark.parametrize("tier, usable", [("slim", True), ("compact", False), ("stub", False)])
def test_a_capped_graph_export_is_not_a_symbol_index(tmp_path, tier, usable):
    """``compact`` strips node extras and ``stub`` drops nodes entirely.

    Either one yields an index that is missing symbols for reasons that have
    nothing to do with the diff, and a symbol absent from the baseline index is
    exempted from the ratchet — so a capped export silently narrows the check
    rather than failing it.
    """
    got_index, got_usable = _index(tmp_path, "h", tier=tier)
    assert got_usable is usable, tier
    assert bool(got_index) is usable


# --------------------------------------------------------------------------
# The resolution-rate ratchet
# --------------------------------------------------------------------------


def _sides(base_rate, cur_rate):
    common = {
        "entry_roots": ["pkg/cli.py"],
        "unwired_candidates": [],
        "unreachable_files": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
        "liveness_unreachable_unreliable": False,
    }
    return (
        {**common, "complete": True, "net_resolution_permille": base_rate},
        {**common, "net_resolution_permille": cur_rate},
    )


def test_a_resolution_drop_is_flagged(tmp_path):
    """ON: the earliest signal that extraction regressed.

    A call the resolver stopped attributing appears in no list here — the edge
    simply is not there, and a symbol whose only caller became unresolvable
    reads as "nothing calls it". The rate moves before any candidate list does.
    """
    baseline, current = _sides(620, 410)
    gaps = detect_liveness_regressions(baseline, current, set(), blocking=True)
    rate_gaps = [g for g in gaps if g.gap_type == "resolution_regression"]
    assert len(rate_gaps) == 1
    assert "62.0%" in rate_gaps[0].description
    assert "41.0%" in rate_gaps[0].description
    assert rate_gaps[0].blocking


@pytest.mark.parametrize(
    "base_rate, cur_rate",
    [
        (620, 620),  # unchanged
        (410, 620),  # improved
        (None, 620),  # baseline never measured it
        (620, None),  # current could not measure it
        (None, None),
    ],
)
def test_no_gap_without_an_actual_drop(base_rate, cur_rate):
    """OFF, including both "not measured" cases.

    A missing figure on either side is not a drop to zero. Folding it into one
    would make every pre-W2.1 baseline report a catastrophic regression.
    """
    baseline, current = _sides(base_rate, cur_rate)
    gaps = detect_liveness_regressions(baseline, current, set(), blocking=True)
    assert not [g for g in gaps if g.gap_type == "resolution_regression"]


# --------------------------------------------------------------------------
# The unreachable half
# --------------------------------------------------------------------------


def _unreachable_sides(*, base, cur, unreliable):
    common = {
        "entry_roots": ["pkg/cli.py"],
        "unwired_candidates": [],
        "dead_symbol_candidates": [],
        "symbol_index": [],
    }
    return (
        {
            **common,
            "complete": True,
            "unreachable_files": base,
            "liveness_unreachable_unreliable": False,
        },
        {
            **common,
            "unreachable_files": cur,
            "liveness_unreachable_unreliable": unreliable,
        },
    )


def test_the_unreachable_half_can_actually_fire():
    """The branch that had never executed.

    The diff is skipped whenever either side is unreliable, and the kernel wrote
    ``liveness_unreachable_unreliable: true`` unconditionally — so this half of
    the ratchet has not run since cutover. W1.1 made the flag a computed
    verdict; this asserts the consequence, because a work order that says "do
    not leave a branch that cannot execute" is only satisfied by showing it now
    does.
    """
    baseline, current = _unreachable_sides(
        base=[], cur=["pkg/stranded.py"], unreliable=False
    )
    gaps = detect_liveness_regressions(baseline, current, set(), blocking=True)
    stranded = [g for g in gaps if g.gap_type == "stranded_code"]
    assert len(stranded) == 1, gaps
    assert "unreachable" in stranded[0].description
    assert stranded[0].file == "pkg/stranded.py"


def test_an_unreliable_side_still_suppresses_the_unreachable_half():
    """OFF: the suppression is a live guard, not a dead one either.

    When the kernel says its reachability answer cannot be trusted — an
    oversized component graph, or call coverage with a hole in it — the same
    input must produce nothing.
    """
    baseline, current = _unreachable_sides(
        base=[], cur=["pkg/stranded.py"], unreliable=True
    )
    gaps = detect_liveness_regressions(baseline, current, set(), blocking=True)
    assert not [g for g in gaps if g.gap_type == "stranded_code"]
