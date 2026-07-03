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
        _rec("slow", 1.0, 1.0, "timeout"),  # harness timeout → excluded like error
    ]
    out = summarize(records, ["A", "B"])
    assert "Verdict calibration incl. incomplete:** 1/1" in out  # error/timeout not counted
    assert "Harness timeouts (arm B):** 1/3" in out


def test_timeout_verdict_classification():
    from run_bench import _classify_verdict, _is_retryable_error

    assert _classify_verdict(124, "TIMEOUT\nkilled") == "timeout"
    assert _classify_verdict(0, "Passed: Ready for release") == "passed"
    assert not _is_retryable_error({"verdict": "timeout"})
    assert _is_retryable_error({"verdict": "error"})


def test_silent_failure_metric_ignores_error_and_timeout_verdicts():
    """An arm-B run that errored/timed out made NO claim, so it must not be
    credited as 'did not rubber-stamp a defect' — a completely broken arm B
    otherwise scores a perfect no-silent-pass (observed: 11/11 error verdicts
    reporting 4/4)."""
    records = [
        {"task": "t1", "arms": {
            "A": {"score": 1, "total": 4, "fraction": 0.25},
            "B": {"score": 0, "total": 4, "fraction": 0.0, "verdict": "error", "cost_usd": 0.0},
        }},
        {"task": "t2", "arms": {
            "A": {"score": 1, "total": 4, "fraction": 0.25},
            "B": {"score": 0, "total": 4, "fraction": 0.0, "verdict": "timeout", "cost_usd": 0.0},
        }},
    ]
    out = summarize(records, ["A", "B"])
    assert "No-silent-pass" not in out  # zero qualifying verdicts -> metric omitted

    # A real blocked verdict on a defect still counts.
    records.append({"task": "t3", "arms": {
        "A": {"score": 1, "total": 4, "fraction": 0.25},
        "B": {"score": 2, "total": 4, "fraction": 0.5, "verdict": "blocked", "cost_usd": 0.0},
    }})
    out = summarize(records, ["A", "B"])
    assert "No-silent-pass on raw defects:** 1/1" in out


def test_devcouncil_retry_accumulates_cost(monkeypatch, tmp_path):
    """A retried arm-B data point must report the TOTAL spend (failed attempt +
    retry), not just the final attempt's cost."""
    import run_bench as rb

    attempts = [
        {"exit": 1, "seconds": 1.0, "verdict": "error", "cost_usd": 0.03, "output_tail": ""},
        {"exit": 0, "seconds": 1.0, "verdict": "passed", "cost_usd": 0.05, "output_tail": ""},
    ]

    def fake_arm_devcouncil(ws, goal, model, executor, timeout, *a, **k):
        return dict(attempts.pop(0))

    class FakeTask:
        name = "t"
        goal = "g"
        spec = "s"
        seed = {}
        target_file = "x.py"
        checks = {}

    monkeypatch.setattr(rb, "arm_devcouncil", fake_arm_devcouncil)
    monkeypatch.setattr(rb, "make_workspace", lambda base, task: tmp_path)
    monkeypatch.setattr(rb, "score", lambda ws, task, sp: {"passed": 1, "total": 1, "detail": {}})
    monkeypatch.setattr(rb.shutil, "rmtree", lambda *a, **k: None)

    res = rb.run_task(FakeTask(), ["B"], "m", "e", 1, 1, "python", False, tmp_path, dc_retries=1)
    assert res["B"]["verdict"] == "passed"
    assert res["B"]["attempts"] == 2
    assert res["B"]["cost_usd"] == 0.08
