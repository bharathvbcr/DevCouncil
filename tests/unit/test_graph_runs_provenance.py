"""`devcouncil_graph_runs` must not report "I could not look" as "nothing happened".

Three different outcomes collapsed into the same `{"ok": True, "runs": []}`:

* the project has no trace log at all (`read_trace_events` returns `[]` when the
  file is missing — `telemetry/traces.py:84`),
* the log exists but every line failed to parse (they are skipped at *debug*
  level — `telemetry/traces.py:91-94`), and
* the kernel genuinely has not run.

Only the third is the answer `runs: []` reads as. This is the same class the rest
of this codebase already fails closed on: *a check that could not run must never
report what a check that ran and passed reports.*

Separately, `limit` silently truncated. A caller asking for 10 got 10 whether the
log held 10 or 10,000, with nothing saying which — a capped sample presented as
complete coverage.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

from devcouncil.devmap_engine import read_run_history, read_runs
from devcouncil.integrations.mcp.handlers.map import handle_graph_runs
from devcouncil.telemetry.traces import trace_file_path


def _root(tmp_path: Path) -> Path:
    root = tmp_path / "proj"
    (root / ".devcouncil" / "logs").mkdir(parents=True)
    return root


def _write_runs(root: Path, count: int, *, ok: bool = True) -> None:
    path = trace_file_path(root)
    lines = [
        json.dumps({
            "type": "devmap_run",
            "run_id": f"r{index}",
            "timestamp": f"2026-09-04T00:00:{index:02d}+00:00",
            "details": {"stage": "build", "ok": ok},
        })
        for index in range(count)
    ]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _payload(root: Path, **args) -> dict:
    return json.loads(asyncio.run(handle_graph_runs(root, args))[0].text)


def test_a_missing_trace_log_is_not_reported_as_zero_runs(tmp_path):
    root = _root(tmp_path)
    assert not trace_file_path(root).exists()

    payload = _payload(root, limit=5)
    assert payload["runs"] == []
    assert payload["log_present"] is False, (
        "a project with no trace log reported the same thing as one whose kernel "
        "has never run; the caller cannot tell 'nothing happened' from 'I could "
        "not look'"
    )


def test_an_existing_but_empty_log_is_distinguishable_from_a_missing_one(tmp_path):
    root = _root(tmp_path)
    trace_file_path(root).write_text("", encoding="utf-8")

    payload = _payload(root, limit=5)
    assert payload["runs"] == [] and payload["log_present"] is True


def test_unparseable_lines_are_counted_not_silently_dropped(tmp_path):
    root = _root(tmp_path)
    trace_file_path(root).write_text("{not json\n{\"also\": \"not an event\"\n", encoding="utf-8")

    payload = _payload(root, limit=5)
    assert payload["log_present"] is True
    assert payload["unparsed_lines"] >= 1, (
        "every line of the log was unreadable and the tool still answered "
        "'no runs' with no sign that it had failed to read anything"
    )


def test_a_truncated_run_list_says_so_and_carries_the_true_total(tmp_path):
    root = _root(tmp_path)
    _write_runs(root, 25)

    payload = _payload(root, limit=5)
    assert len(payload["runs"]) == 5
    assert payload["shown"] == 5
    assert payload["total"] == 25, (
        "the total must be what a complete answer would have held, not the "
        "length of the capped sample"
    )
    assert payload["truncated"] is True


def test_an_untruncated_list_is_not_flagged_as_truncated(tmp_path):
    root = _root(tmp_path)
    _write_runs(root, 3)

    payload = _payload(root, limit=10)
    assert payload["shown"] == 3 and payload["total"] == 3
    assert payload["truncated"] is False


def test_read_runs_still_returns_a_plain_list_for_its_other_callers(tmp_path):
    """`devmap_health` and `graph_cmd` want the runs, not the provenance.

    `read_runs` stays a thin adapter over the one implementation rather than a
    second reader beside it, so the two cannot drift.
    """
    root = _root(tmp_path)
    _write_runs(root, 4)

    # Compared by run_id: `TraceEvent.from_legacy` stamps a fresh timestamp on
    # any event whose own is unusable, so equality on the whole dict would be
    # comparing two clock reads, not two code paths.
    assert [r["run_id"] for r in read_runs(root, limit=2)] == [
        r["run_id"] for r in read_run_history(root, limit=2).runs
    ]
    assert isinstance(read_runs(root, limit=2), list)
    assert read_runs(root, failed_only=True) == []
