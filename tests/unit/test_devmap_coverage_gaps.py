"""`dev map doctor` names the files the kernel could not read, and the edges it doubts.

The kernel's `degraded_reason` has always carried the *counts* — "2 file(s)
failed to parse, 1 recovered by pattern (no calls extracted), 1 refused by
discovery and never read at all" — and never a path, so an operator could not
tell a correct refusal (this repository's 30.6 MB vendored `parser.c` against a
1 MiB ceiling) from a broken one without opening the store. The kernel now
inventories them; these tests pin the seam that carries the inventory to the
person reading `dev map doctor`.

Three answers must stay three answers all the way through: an inventory the
kernel took, `null` because there was no store to inventory, and a key this
binary never sent because it predates the listing. Collapsing the last two into
"nothing was refused" is the Class A failure — a check that could not run
reporting what a check that ran and passed reports.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path
from typing import Any, Dict, List, Optional

import pytest

import devcouncil.devmap_health as health
from devcouncil.devmap_client import UNREPORTED, DevMapClient, DevMapClientError


# --- fixtures ----------------------------------------------------------------


def _store(root: Path) -> Path:
    """A real (empty) kernel store, so the doctor runs its store-backed checks."""
    from devcouncil.devmap_engine import DEFAULT_DB_RELPATH

    path = root / DEFAULT_DB_RELPATH
    path.parent.mkdir(parents=True, exist_ok=True)
    sqlite3.connect(path).close()
    return path


def _gap(*rows: tuple, total: Optional[int] = None) -> Dict[str, Any]:
    paths = [{"path": path, "reason": reason} for path, reason in rows]
    shown = len(paths)
    return {
        "total": total if total is not None else shown,
        "shown": shown,
        "truncated": total is not None and total > shown,
        "paths": paths,
    }


def _empty_gaps() -> Dict[str, Any]:
    return {kind: _gap() for kind in ("discovery_refused", "parse_failed", "pattern_recovered")}


def _doctor(
    root: Path,
    monkeypatch: pytest.MonkeyPatch,
    **kernel: Any,
) -> Dict[str, Any]:
    """`run_doctor` over a store that exists and a kernel status we dictate.

    Only the keys given are present on the kernel row: absence is the signal
    under test, so it cannot be faked with a `None`.
    """
    _store(root)
    row: Dict[str, Any] = {
        "generation_id": 7,
        "pending_count": 0,
        "quarantined_count": 0,
        "node_count": 100,
        "edge_count": 400,
        "is_fresh": True,
        "degraded_reason": None,
    }
    row.update(kernel)
    monkeypatch.setattr(health, "kernel_status", lambda _root: row)
    return health.run_doctor(root)


def _check(result: Dict[str, Any], name: str) -> Dict[str, Any]:
    for item in result["checks"]:
        if item["name"] == name:
            return item
    raise AssertionError(f"no {name!r} check in {[c['name'] for c in result['checks']]}")


def _rendered(result: Dict[str, Any], name: str) -> List[str]:
    """The `name` check's line plus the indented lines that belong to it."""
    lines = health.render_doctor(result)
    for index, line in enumerate(lines):
        if line[4:].lstrip().startswith(f"{name}:"):
            block = [line]
            for following in lines[index + 1 :]:
                if not following.startswith("    "):
                    break
                block.append(following)
            return block
    raise AssertionError(f"no {name!r} line in {lines}")


# --- the client carries three answers, not two -------------------------------


def _status(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, **extra: Any):
    client = DevMapClient(tmp_path)
    payload: Dict[str, Any] = {
        "generation_id": 3,
        "pending_count": 0,
        "node_count": 10,
        "edge_count": 20,
        "is_fresh": True,
    }
    payload.update(extra)
    monkeypatch.setattr(client, "_request", lambda *_a, **_k: payload)
    return client.status()


def test_status_carries_the_inventory_the_kernel_took(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gaps = _empty_gaps()
    gaps["discovery_refused"] = _gap(("vendor/parser.c", "oversized"))
    status = _status(
        tmp_path,
        monkeypatch,
        coverage_gaps=gaps,
        edge_resolution_source="stored",
        edge_confidence_mismatches=0,
    )

    assert status.coverage_gaps == gaps
    assert status.edge_resolution_source == "stored"
    assert status.edge_confidence_mismatches == 0


def test_a_kernel_that_never_sent_the_fields_is_not_read_as_one_that_measured(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    status = _status(tmp_path, monkeypatch)

    assert status.coverage_gaps is UNREPORTED
    assert status.edge_resolution_source is UNREPORTED
    assert status.edge_confidence_mismatches is UNREPORTED
    # The distinction the whole design rests on: absent is not null.
    assert status.coverage_gaps is not None


def test_a_null_inventory_is_a_store_that_was_not_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    status = _status(
        tmp_path, monkeypatch, coverage_gaps=None, edge_resolution_source=None
    )

    assert status.coverage_gaps is None
    assert status.edge_resolution_source is None
    assert status.coverage_gaps is not UNREPORTED


@pytest.mark.parametrize(
    "field, value",
    [
        ("coverage_gaps", "two files"),
        ("coverage_gaps", 4),
        ("edge_resolution_source", 1),
        ("edge_confidence_mismatches", "3"),
        ("edge_confidence_mismatches", -1),
        ("edge_confidence_mismatches", True),
    ],
)
def test_status_refuses_a_malformed_inventory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, field: str, value: Any
) -> None:
    with pytest.raises(DevMapClientError, match="must be"):
        _status(tmp_path, monkeypatch, **{field: value})


def test_kernel_status_omits_what_this_kernel_did_not_report(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from devcouncil import devmap_client

    def fake(self: Any) -> Any:
        return devmap_client.DevMapStatus(
            generation_id=2,
            pending_count=0,
            node_count=1,
            edge_count=1,
            coverage_gaps=None,
            edge_resolution_source="reconstructed",
        )

    monkeypatch.setattr(devmap_client.DevMapClient, "status", fake)
    row = health.kernel_status(tmp_path)

    # Reported as null: the key is there, carrying the kernel's own answer.
    assert "coverage_gaps" in row and row["coverage_gaps"] is None
    assert row["edge_resolution_source"] == "reconstructed"
    # Never sent by this kernel: no key at all, so no reader can mistake the
    # silence for a measurement.
    assert "edge_confidence_mismatches" not in row


# --- the doctor names the paths ----------------------------------------------


def test_the_kernel_check_names_the_files_it_could_not_read(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gaps = _empty_gaps()
    gaps["discovery_refused"] = _gap(
        (
            "rust-port/vendor/grammars/cobol/parser.c",
            "oversized: 30,660,349 B over the 1,048,576 B ceiling",
        )
    )
    gaps["parse_failed"] = _gap(("a/broken.rs", "unexpected end of file"))
    gaps["pattern_recovered"] = _gap(("b/huge.ts", "parse budget exhausted"))
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=False,
        degraded_reason=(
            "call extraction did not cover the whole corpus: 1 file(s) failed to parse, "
            "1 recovered by pattern (no calls extracted), 1 refused by discovery"
        ),
        coverage_gaps=gaps,
    )

    item = _check(result, "kernel")
    assert item["ok"] is False
    # Structured for `--json`, not only prose in `detail`.
    assert item["coverage_gaps"] == gaps

    block = "\n".join(_rendered(result, "kernel"))
    assert "refused by discovery: rust-port/vendor/grammars/cobol/parser.c" in block
    assert "30,660,349 B over the 1,048,576 B ceiling" in block
    assert "failed to parse: a/broken.rs (unexpected end of file)" in block
    assert "recovered by pattern" in block and "b/huge.ts" in block


def test_a_capped_listing_says_how_many_it_did_not_show(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gaps = _empty_gaps()
    gaps["parse_failed"] = _gap(("one.py", "syntax error"), ("two.py", "syntax error"), total=57)
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=False,
        degraded_reason="57 file(s) failed to parse",
        coverage_gaps=gaps,
    )

    block = "\n".join(_rendered(result, "kernel"))
    assert "one.py" in block and "two.py" in block
    assert "and 55 more" in block


def test_a_kernel_that_cannot_list_them_says_so_rather_than_nothing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=False,
        degraded_reason="2 file(s) failed to parse",
    )

    item = _check(result, "kernel")
    # In `--json` the two silences would both be a null `coverage_gaps`; this
    # is the key that keeps them apart for a caller that never reads the prose.
    assert item["coverage_gaps_reported"] is False
    assert item["coverage_gaps"] is None

    block = "\n".join(_rendered(result, "kernel")).lower()
    assert "predates" in block


def test_a_store_that_was_never_read_is_not_a_clean_inventory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=False,
        degraded_reason="store schema is 12, this binary speaks 14",
        coverage_gaps=None,
    )

    block = "\n".join(_rendered(result, "kernel")).lower()
    assert "not measured" in block
    item = _check(result, "kernel")
    assert item["coverage_gaps"] is None
    assert item["coverage_gaps_reported"] is True, "the kernel did answer; the answer was null"


def test_render_doctor_keeps_one_line_per_check(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gaps = _empty_gaps()
    gaps["discovery_refused"] = _gap(("vendor/parser.c", "oversized"))
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=False,
        degraded_reason="1 refused by discovery",
        coverage_gaps=gaps,
    )

    lines = health.render_doctor(result)
    unindented = [line for line in lines if not line.startswith("    ")]
    # One line per check, plus the verdict; the paths ride underneath, indented.
    assert len(unindented) == len(result["checks"]) + 1
    assert any(line.startswith("    ") for line in lines)


# --- the edges check ---------------------------------------------------------


def test_edges_is_ok_when_every_stored_confidence_matches_its_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result = _doctor(
        tmp_path,
        monkeypatch,
        coverage_gaps=_empty_gaps(),
        edge_resolution_source="stored",
        edge_confidence_mismatches=0,
    )

    item = _check(result, "edges")
    assert item["ok"] is True
    assert item["critical"] is False
    assert "stored" in item["detail"]


def test_edges_fails_when_a_stored_confidence_contradicts_its_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result = _doctor(
        tmp_path,
        monkeypatch,
        coverage_gaps=_empty_gaps(),
        edge_resolution_source="stored",
        edge_confidence_mismatches=3,
    )

    item = _check(result, "edges")
    assert item["ok"] is False
    assert item["critical"] is False
    assert "3" in item["detail"]
    assert item["code"]
    assert item["fix_command"] == "dev map"


def test_edges_is_unknown_when_this_kernel_does_not_report_it(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result = _doctor(tmp_path, monkeypatch, coverage_gaps=_empty_gaps())

    item = _check(result, "edges")
    assert item["ok"] is None, "an unknown must never render as ok"
    assert item["detail"], "and it must say why it is unknown"


def test_edges_is_unknown_when_the_kernel_read_no_store(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=False,
        degraded_reason="no devmap store at this path",
        coverage_gaps=None,
        edge_confidence_mismatches=0,
    )

    item = _check(result, "edges")
    assert item["ok"] is None, "0 mismatches over a store nobody read is not a pass"


def test_edges_is_unknown_when_the_kernel_holds_no_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A store that exists but whose build never landed.

    The kernel answers `coverage_gaps` with an empty inventory (there is a
    store, and it refused nothing) and `edge_confidence_mismatches` with
    `null` (there is no generation whose edges could disagree), so the
    no-store branch does not catch this one.
    """
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=False,
        degraded_reason="this store holds no generation: nothing has been indexed yet",
        coverage_gaps=_empty_gaps(),
        edge_resolution_source=None,
        edge_confidence_mismatches=None,
    )

    item = _check(result, "edges")
    assert item["ok"] is None, "no generation is not a passing edge check"
    assert "generation" in item["detail"]


def test_a_reconstructed_resolution_is_called_out(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    result = _doctor(
        tmp_path,
        monkeypatch,
        coverage_gaps=_empty_gaps(),
        edge_resolution_source="reconstructed",
        edge_confidence_mismatches=0,
    )

    item = _check(result, "edges")
    assert item["ok"] is True
    assert "reconstructed" in item["detail"]
    assert "predates" in item["detail"]


def test_a_failing_edges_check_is_one_a_build_resolves(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """`--fix` must not drop a failing check on the floor.

    `apply_fixes` only reports a failing code it recognises; one in neither
    `_BUILD_RESOLVES` nor the report-only sets is applied by nothing and listed
    by nothing, which is a failing check that vanishes from `--fix` output.
    """
    result = _doctor(
        tmp_path,
        monkeypatch,
        coverage_gaps=_empty_gaps(),
        edge_resolution_source="stored",
        edge_confidence_mismatches=2,
    )

    code = _check(result, "edges")["code"]
    assert code in health._BUILD_RESOLVES


# --- a corpus the kernel cannot read is not pending work ---------------------


def test_a_fully_walked_corpus_with_unreadable_files_is_partial_not_pending(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """`pending_count == 0`, nothing quarantined, and yet degraded: the gaps are
    files the kernel walked and could not read. A rebuild re-measures exactly
    the same gaps, so the remedy must not be `repair --pending` and `--fix`
    must not pretend to act on it."""
    gaps = _empty_gaps()
    gaps["discovery_refused"] = _gap(
        ("rust-port/vendor/grammars/cobol/parser.c", "30660349 bytes exceeds the 1048576 byte source ceiling")
    )
    result = _doctor(
        tmp_path,
        monkeypatch,
        is_fresh=True,
        degraded_reason="partial: 1 refused by discovery and never read at all",
        coverage_gaps=gaps,
    )
    kernel = _check(result, "kernel")
    assert kernel["ok"] is False
    assert kernel["code"] == "coverage_partial", kernel
    assert kernel["fix_command"] == "", "there is no command that reads an unreadable file"
    assert "repair --pending" not in kernel["fix"]
    assert "no rebuild changes this" in kernel["fix"]
    assert kernel["coverage_gaps"] == gaps
    block = _rendered(result, "kernel")
    assert any("parser.c" in line for line in block), block


@pytest.mark.parametrize("kernel_row", [{"pending_count": 3, "is_fresh": False}, {"quarantined_count": 2}])
def test_queued_or_quarantined_paths_are_still_pending_work(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, kernel_row: Dict[str, Any]
) -> None:
    result = _doctor(
        tmp_path,
        monkeypatch,
        degraded_reason="3 path(s) queued",
        coverage_gaps=_empty_gaps(),
        **kernel_row,
    )
    kernel = _check(result, "kernel")
    assert kernel["ok"] is False
    assert kernel["code"] == "pending_paths", kernel
    assert kernel["fix_command"] == "dev map doctor --fix"
    assert "repair --pending" in kernel["fix"]
