#!/usr/bin/env python
"""Calibration dashboard over benchmark result files.

Aggregates false-positive / false-negative rates and incomplete breakdowns across
``benchmarks/results/*.json`` so verdict calibration can be tracked over time.

Usage:
    python benchmarks/calibration_dashboard.py
    python benchmarks/calibration_dashboard.py benchmarks/results/run1.json benchmarks/results/run2.json
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

BENCH_DIR = Path(__file__).parent
if str(BENCH_DIR) not in sys.path:
    sys.path.insert(0, str(BENCH_DIR))

from run_bench import summarize  # noqa: E402


def _extract_metrics(summary_text: str) -> dict[str, str | int | float]:
    """Pull headline calibration numbers from summarize() markdown."""
    metrics: dict[str, str | int | float] = {}
    for line in summary_text.splitlines():
        line = line.strip()
        if "Decisive-verdict accuracy" in line and "**" in line:
            # e.g. - **Decisive-verdict accuracy (passed/blocked):** 2/2 = 100%
            part = line.split(":**", 1)[-1].strip()
            metrics["decisive"] = part
        elif "Verdict calibration incl. incomplete" in line:
            metrics["overall"] = line.split(":**", 1)[-1].strip()
        elif "False negatives (blocked on correct code)" in line:
            metrics["false_neg"] = line.split(":**", 1)[-1].strip()
        elif "False positives (passed on imperfect code)" in line:
            metrics["false_pos"] = line.split(":**", 1)[-1].strip()
        elif "Incomplete verdicts" in line and "cautious" in line:
            metrics["incomplete"] = line.split(":**", 1)[-1].strip()
        elif "Infra errors (arm B" in line:
            metrics["infra_errors"] = line.split(":**", 1)[-1].strip()
    return metrics


def _load_run(path: Path) -> dict:
    data = json.loads(path.read_text(encoding="utf-8"))
    records = data.get("records") or []
    arms = sorted({a for rec in records for a in rec.get("arms", {})})
    summary = summarize(records, arms)
    return {
        "path": path.name,
        "timestamp": data.get("timestamp") or path.stem,
        "model": data.get("model", "?"),
        "executor": data.get("executor", "?"),
        "tasks": len(records),
        "metrics": _extract_metrics(summary),
    }


def render_dashboard(runs: list[dict]) -> str:
    lines = ["# DevCouncil Verdict Calibration Dashboard", ""]
    if not runs:
        lines.append("_No benchmark result files found._")
        return "\n".join(lines)

    lines.append("| run | tasks | decisive | overall (incl. incomplete) | false neg | false pos | infra excl. |")
    lines.append("| --- | ---: | --- | --- | --- | --- | --- |")
    for run in runs:
        m = run["metrics"]
        lines.append(
            f"| {run['path']} | {run['tasks']} | {m.get('decisive', '—')} | "
            f"{m.get('overall', '—')} | {m.get('false_neg', '—')} | "
            f"{m.get('false_pos', '—')} | {m.get('infra_errors', '—')} |"
        )
    lines.append("")
    lines.append(
        "_Infra errors are executor/session/SDK failures excluded from calibration means. "
        "Incomplete breakdown separates cautious-correct withholds from under-credited passes._"
    )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    args = argv if argv is not None else sys.argv[1:]
    if args:
        paths = [Path(p) for p in args]
    else:
        results_dir = BENCH_DIR / "results"
        paths = sorted(results_dir.glob("*.json")) if results_dir.exists() else []

    runs = [_load_run(p) for p in paths if p.exists()]
    print(render_dashboard(runs))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
