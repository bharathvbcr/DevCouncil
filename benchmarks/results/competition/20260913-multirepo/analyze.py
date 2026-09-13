#!/usr/bin/env python3
"""Summarise measurements.jsonl into per-repository, per-tool, per-stage tables.

Reports the MINIMUM elapsed time as the headline, with median and max beside
it: background load can only make a run slower, so the minimum measures the
tool and the mean measures the machine. Speed factors are computed from
unrounded minimums and are descriptive only -- these tools do not do the same
work, and a factor is not an accuracy or quality ranking.
"""

import json
import statistics
from collections import defaultdict
from pathlib import Path

OUT = Path(__file__).resolve().parent
PLAN = json.loads((OUT / "plan.json").read_text())
TOOLS = ("devmap", "codegraph", "cbm", "gitnexus", "graphify")
STAGES = ("cold", "warm", "touch")


def load():
    rows = defaultdict(list)
    failures = []
    for line in (OUT / "measurements.jsonl").read_text().splitlines():
        if not line.strip():
            continue
        rec = json.loads(line)
        stage, repo, tool, _ = rec["label"].split("-")
        if not rec.get("measurement_ok"):
            failures.append(rec["label"])
            continue
        rows[(repo, tool, stage)].append(rec)
    return rows, failures


def peak_rss(records):
    """Prefer the sampled process-tree peak.

    BSD rusage sees only the direct child, so a tool that supervises workers
    (CBM reports ~3 MB that way) is badly understated by rss_bytes alone.
    """
    vals = [r.get("sampled_tree_peak_rss_bytes") or r.get("rss_bytes") or 0
            for r in records]
    return max(vals) if vals else None


def main():
    rows, failures = load()
    repos = sorted({k[0] for k in rows})

    summary = {}
    for repo in repos:
        summary[repo] = {}
        for tool in TOOLS:
            summary[repo][tool] = {}
            for stage in STAGES:
                recs = rows.get((repo, tool, stage), [])
                if not recs:
                    continue
                secs = sorted(r["seconds"] for r in recs)
                summary[repo][tool][stage] = {
                    "n": len(secs),
                    "min": secs[0],
                    "median": statistics.median(secs),
                    "max": secs[-1],
                    "peak_rss_bytes": peak_rss(recs),
                }

    for stage in STAGES:
        print("\n=== %s (minimum seconds; xN = times DevMap's) ===" % stage)
        header = "%-12s" % "repo"
        for tool in TOOLS:
            header += " %18s" % tool
        print(header)
        for repo in repos:
            base = summary[repo].get("devmap", {}).get(stage, {}).get("min")
            line = "%-12s" % repo
            for tool in TOOLS:
                cell = summary[repo].get(tool, {}).get(stage)
                if not cell:
                    line += " %18s" % "-"
                    continue
                if base and tool != "devmap":
                    line += " %11.3fs %5.1fx" % (cell["min"], cell["min"] / base)
                else:
                    line += " %11.3fs %5s" % (cell["min"], "1.0x")
            print(line)

    print("\n=== peak RSS, sampled process tree (MB) ===")
    header = "%-12s" % "repo"
    for tool in TOOLS:
        header += " %11s" % tool
    print(header)
    for repo in repos:
        line = "%-12s" % repo
        for tool in TOOLS:
            cell = summary[repo].get(tool, {}).get("cold")
            line += " %11s" % (("%.0f" % (cell["peak_rss_bytes"] / 1048576))
                               if cell and cell["peak_rss_bytes"] else "-")
        print(line)

    if failures:
        print("\nFAILED / unusable measurements (%d): %s" % (len(failures), failures))
    else:
        print("\nAll recorded measurements succeeded.")

    (OUT / "summary.json").write_text(json.dumps({
        "summary": summary,
        "failures": failures,
        "corpus_commits": PLAN["corpus_commits"],
        "tool_identities": PLAN["tool_identities"],
    }, indent=1) + "\n")
    print("wrote summary.json")


if __name__ == "__main__":
    main()
