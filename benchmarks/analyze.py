#!/usr/bin/env python
"""Analyze a benchmark results JSON into category-level insight.

Beyond the headline scores, this breaks down *which kinds of requirements* each
arm misses — DevCouncil's thesis is that gating mainly catches the edge-case and
error-handling traps a happy-path agent skips, so that breakdown is the point.

Usage: python benchmarks/analyze.py benchmarks/results/<timestamp>.json
"""
from __future__ import annotations

import json
import sys
from collections import defaultdict
from pathlib import Path


def categorize(check_name: str) -> str:
    n = check_name.lower()
    if any(k in n for k in ("rais", "valueerror", "typeerror", "empty", "invalid", "zero")):
        return "error/empty handling"
    if any(k in n for k in ("mut", "mutate")):
        return "input immutability"
    if any(k in n for k in ("partial", "unsorted", "adjacent", "lowercase", "wrong", "interleav",
                            "larger", "first_equals", "blank", "comment", "ignore", "duplicate", "subtractive")):
        return "edge cases"
    return "core / happy path"


def main():
    if len(sys.argv) < 2:
        # default to the most recent results file
        results_dir = Path(__file__).parent / "results"
        files = sorted(results_dir.glob("*.json"))
        if not files:
            sys.exit("No results file given and none found in benchmarks/results/.")
        path = files[-1]
    else:
        path = Path(sys.argv[1])

    data = json.loads(path.read_text(encoding="utf-8"))
    records = data["records"]
    arms = sorted({a for rec in records for a in rec["arms"]})

    # category -> arm -> [pass(1)/fail(0) per check]
    cat = defaultdict(lambda: defaultdict(list))
    arm_fracs = defaultdict(list)
    for rec in records:
        for arm, r in rec["arms"].items():
            if "fraction" in r:
                arm_fracs[arm].append(r["fraction"])
            for check, ok in (r.get("detail") or {}).items():
                cat[categorize(check)][arm].append(1 if ok else 0)

    print(f"# Benchmark analysis: {path.name}")
    print(f"\nplanner={data.get('model')}  executor={data.get('executor')}  "
          f"tasks={len(records)}  arms={arms}\n")

    print("## Mean correctness by arm")
    for a in arms:
        vals = arm_fracs[a]
        if vals:
            print(f"- arm {a}: {sum(vals)/len(vals):.2f}  (n={len(vals)})")

    print("\n## Pass rate by requirement category (the diagnostic view)")
    header = "| category | " + " | ".join(f"arm {a}" for a in arms) + " |"
    print(header)
    print("|" + "---|" * (len(arms) + 1))
    for category in ("core / happy path", "edge cases", "input immutability", "error/empty handling"):
        cells = [category]
        for a in arms:
            vals = cat[category][a]
            cells.append(f"{(sum(vals)/len(vals)):.0%} ({len(vals)})" if vals else "-")
        print("| " + " | ".join(cells) + " |")

    print("\n_Read across a row: where do the arms diverge? DevCouncil's value should "
          "show up most in the error/empty-handling and edge-case rows, where a "
          "happy-path agent under a terse prompt tends to fall short._")


if __name__ == "__main__":
    main()
