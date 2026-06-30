"""Benchmark verdict calibration must cover EVERY non-error task, including
DevCouncil's 'incomplete' verdict (cautious-correct vs under-credit), not just
decisive passed/blocked ones.
"""

import sys
from pathlib import Path

BENCH = str(Path(__file__).resolve().parents[2] / "benchmarks")
if BENCH not in sys.path:
    sys.path.insert(0, BENCH)

from run_bench import summarize  # noqa: E402


def _rec(name, a_frac, b_frac, b_verdict):
    def arm(frac):
        return {"score": int(round(frac * 5)), "total": 5, "fraction": frac}
    return {"task": name, "arms": {"A": arm(a_frac), "B": {**arm(b_frac), "verdict": b_verdict, "cost_usd": 0.0}}}


def test_incomplete_is_covered_in_calibration():
    records = [
        _rec("passed_ok", 0.8, 1.0, "passed"),       # decisive, consistent
        _rec("blocked_ok", 1.0, 0.4, "blocked"),     # decisive, consistent
        _rec("incomplete_full", 0.6, 1.0, "incomplete"),     # under-credit (full but hedged)
        _rec("incomplete_partial", 0.6, 0.4, "incomplete"),  # cautious-correct
    ]
    out = summarize(records, ["A", "B"])
    # Decisive accuracy counts only passed/blocked (2/2 here).
    assert "Decisive-verdict accuracy (passed/blocked):** 2/2" in out
    # Overall calibration covers all 4 non-error tasks; consistent = passed_ok, blocked_ok,
    # incomplete_partial → 3/4.
    assert "Verdict calibration incl. incomplete:** 3/4" in out
    assert "covers all 4 non-error task(s)" in out
    # Incomplete breakdown surfaced.
    assert "Incomplete verdicts:** 2 (cautious on imperfect code: 1, under-credited correct code: 1)" in out
    # Per-task notes.
    assert "| incomplete | under |" in out
    assert "| incomplete | cautious |" in out


def test_error_verdict_excluded_from_coverage():
    records = [
        _rec("ok", 1.0, 1.0, "passed"),
        _rec("broke", 0.5, 0.0, "error"),  # harness error → not a verdict, excluded
    ]
    out = summarize(records, ["A", "B"])
    assert "Verdict calibration incl. incomplete:** 1/1" in out  # error not counted
