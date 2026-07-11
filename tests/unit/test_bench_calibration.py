"""Benchmark verdict calibration must cover EVERY non-error task, including
DevCouncil's 'incomplete' verdict (cautious-correct vs under-credit), not just
decisive passed/blocked ones.
"""

import json
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
    assert _classify_verdict(0, "**Passed**: Ready for release.\n") == "passed"
    assert _classify_verdict(0, "\x1b[1mPassed\x1b[0m: Ready for release.") == "passed"
    assert _classify_verdict(0, "**Blocked**: 2 high-severity gap(s) remain.") == "blocked"
    assert _classify_verdict(0, "**Incomplete**: nothing is failing, but 1 acceptance") == "incomplete"
    assert not _is_retryable_error({"verdict": "timeout"})
    assert _is_retryable_error({"verdict": "error"})
    assert _is_retryable_error({
        "verdict": "blocked",
        "fraction": 0.0,
        "output_tail": "limit_rpm/qwen/qwen3-max rate limited 429",
    })
    assert not _is_retryable_error({
        "verdict": "blocked",
        "fraction": 0.0,
        "output_tail": "claude exited with code 1",
    })


def test_executor_infra_failure_classifies_as_error():
    """A run whose EXECUTOR never ran (session/usage limit, no credits, failed to
    launch) measures the infrastructure, not DevCouncil: it must classify as
    'error' (retried, excluded from calibration), not pollute blocked/incomplete
    stats with 0-score non-runs."""
    from run_bench import _classify_verdict

    session_limited = (
        "ERROR [devcouncil.cli.commands.run] claude failed to start or execute for TASK-1: "
        "claude exited with code 1: You've hit your session limit · resets 2pm\n"
        "WARNING [devcouncil.cli.commands.go] dev go finished with 1 unfinished task(s): TASK-1 (blocked)\n"
        "Blocked: 1 task"
    )
    assert _classify_verdict(1, session_limited) == "error"
    # Same failure but the pipeline still reached a genuine PASS → keep it.
    assert _classify_verdict(
        0, "You've hit your session limit\nPassed: Ready for release"
    ) == "passed"
    # A normal blocked run without infra markers stays blocked.
    assert _classify_verdict(1, "**Blocked**: 2 high-severity gap(s) remain.") == "blocked"
    # Timeout beats everything.
    assert _classify_verdict(124, "TIMEOUT\nhit your session limit") == "timeout"


def test_infra_errors_excluded_from_arm_b_mean():
    records = [
        _rec("good", 0.5, 1.0, "passed"),
        _rec("infra", 0.5, 0.0, "error"),  # executor never ran: 0-score must not drag the mean
    ]
    out = summarize(records, ["A", "B"])
    assert "Arm B mean correctness:** 1.00 (n=1)" in out
    assert "Infra errors (arm B, excluded from means/calibration):** 1/2" in out


def test_classify_verdict_prefers_json_report(tmp_path, monkeypatch):
    from run_bench import _classify_verdict

    ws = tmp_path / "ws"
    ws.mkdir()
    captured = {}

    def fake_run(cmd, cwd, timeout, env=None, input_text=None):
        captured["cmd"] = cmd
        return 0, json.dumps({"verdict": "passed", "coverage_summary": {}})

    monkeypatch.setattr("run_bench.run", fake_run)
    # stdout lacks the markdown verdict line (Rich/timeout misclassification case).
    verdict = _classify_verdict(0, "Final DevCouncil report\n(no verdict line)", ws=ws, env={})
    assert verdict == "passed"
    assert captured["cmd"][-2:] == ["report", "--json"]
    assert Path(captured["cmd"][0]).name in {"dev", "dev.exe"}


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


def test_session_limit_errors_are_not_retried():
    """A session/usage-limited executor fails identically on an immediate retry
    while still burning full planning cost first — don't retry those. Generic
    errors (planner flakes, spawn failures) stay retryable."""
    from run_bench import _is_retryable_error

    assert not _is_retryable_error({
        "verdict": "error",
        "output_tail": "claude exited with code 1: You've hit your session limit · resets 2pm",
    })
    assert not _is_retryable_error({"verdict": "error", "output_tail": "usage limit reached"})
    assert _is_retryable_error({"verdict": "error", "output_tail": ""})
    assert _is_retryable_error({
        "verdict": "error",
        "output_tail": "claude failed to start or execute for TASK-1: spawn EAGAIN",
    })


def test_sweep_halts_when_executor_stays_down():
    """A mid-sweep executor infra failure re-probes the executor; a failing
    probe halts the sweep (each further task burns planning cost for nothing),
    a passing probe continues, and non-infra errors never probe at all."""
    from run_bench import _sweep_halt_reason

    infra_b = {"B": {"verdict": "error",
                     "output_tail": "claude failed to start or execute: hit your session limit"}}
    # Probe confirms the executor is still down → halt with the probe's detail.
    reason = _sweep_halt_reason(infra_b, probe=lambda: (False, "session limit resets 2pm"))
    assert reason is not None and "session limit" in reason
    # Probe succeeds (limit lifted / transient) → keep sweeping.
    assert _sweep_halt_reason(infra_b, probe=lambda: (True, "ok")) is None
    # A genuine verdict or a non-infra error must not even probe.
    def boom():
        raise AssertionError("probe must not run")
    assert _sweep_halt_reason({"B": {"verdict": "blocked", "output_tail": "gaps remain"}},
                              probe=boom) is None
    assert _sweep_halt_reason({"B": {"verdict": "error", "output_tail": "planner emitted bad JSON"}},
                              probe=boom) is None
    # Arm A executor failure (nonzero exit + infra marker) also triggers the probe.
    infra_a = {"A": {"exit": 1, "output_tail": "You've hit your session limit"}}
    assert _sweep_halt_reason(infra_a, probe=lambda: (False, "still limited")) is not None


def test_blocking_gap_summary_parses_report(monkeypatch, tmp_path):
    """Blocked runs must record WHAT blocked them (gap types + descriptions) so a
    false negative is attributable from the results JSON alone — the median
    false negative took source-diving because only a 400-char tail was kept."""
    import run_bench as rb

    report = {
        "verdict": "blocked",
        "blocking_gaps": [
            {"gap_type": "architecture_drift", "severity": "critical",
             "description": "Open critical live-review card remains: " + "x" * 400},
            {"gap_type": "test_failed", "severity": "high", "description": "check failed"},
            "not-a-dict",
        ],
    }
    monkeypatch.setattr(rb, "run", lambda *a, **k: (0, json.dumps(report)))
    summary = rb._blocking_gap_summary(tmp_path, {})

    assert [g["gap_type"] for g in summary] == ["architecture_drift", "test_failed"]
    assert len(summary[0]["description"]) <= 300  # trimmed, not the whole card
    # Broken report → empty summary, never an exception.
    monkeypatch.setattr(rb, "run", lambda *a, **k: (1, "boom"))
    assert rb._blocking_gap_summary(tmp_path, {}) == []


def test_false_negative_detail_named_in_summary():
    rec = _rec("median", 0.8, 1.0, "blocked")  # blocked but actually 1.0 → false negative
    rec["arms"]["B"]["blocking_gaps"] = [
        {"gap_type": "architecture_drift", "severity": "critical",
         "description": "Open critical live-review card remains: vague response"},
    ]
    out = summarize([rec], ["A", "B"])
    assert "False negatives (blocked correct code):** 1" in out
    assert "median: architecture_drift" in out
    # A false negative WITHOUT recorded detail still gets a line, flagged as such.
    out2 = summarize([_rec("chunk", 0.8, 1.0, "blocked")], ["A", "B"])
    assert "chunk: no blocking-gap detail recorded" in out2
