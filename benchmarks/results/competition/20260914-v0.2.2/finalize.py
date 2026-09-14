#!/usr/bin/env python3
"""Consolidate the matrix, the Gortex gates and on-disk index sizes into one file.

Adapted from 20260913-multirepo. Every caveat that was a *hardcoded observation*
in that run's copy is derived from this run's own data here, or dropped. A
carried-forward number is a claim about a measurement this run did not take, and
this file is the artifact the report cites.
"""

import datetime
import json
import subprocess
from pathlib import Path

OUT = Path(__file__).resolve().parent
PLAN = json.loads((OUT / "plan.json").read_text())
SCRATCH = Path(PLAN["scratch"])
REPOS = ("DevPrism", "DevCouncil", "GitPulse", "scholarlm")

# Where each tool leaves its index. CBM ran with --persistence false and leaves
# none, which is why it has no row here rather than a zero.
INDEX_PATHS = {
    "devmap": lambda r: SCRATCH / ("dm-%s-3.sqlite" % r),
    "codegraph": lambda r: SCRATCH / ("codegraph-%s/.codegraph" % r),
    "graphify": lambda r: SCRATCH / ("graphify-%s/graphify-out" % r),
    "gitnexus": lambda r: SCRATCH / ("gitnexus-%s/.gitnexus" % r),
    "gortex": lambda r: SCRATCH / ("gortex-%s-1.sqlite" % r),
}

FILES_INDEXED = {}  # DevMap's own count, from its cold build JSON


def du_bytes(path):
    if not path.exists():
        return None
    out = subprocess.run(["du", "-sk", str(path)], capture_output=True, text=True)
    if out.returncode != 0:
        return None
    return int(out.stdout.split()[0]) * 1024


def cbm_rss_observed(summary):
    """What THIS run's sampler actually caught for CBM, so the caveat is checkable."""
    seen = {}
    for repo in REPOS:
        cell = summary.get(repo, {}).get("cbm", {}).get("cold")
        if cell and cell.get("peak_rss_bytes"):
            seen[repo] = round(cell["peak_rss_bytes"] / 1048576)
    return seen


def gortex_loads(gates):
    """One-minute load average recorded at each Gortex run, from the run itself."""
    return {g["repo"]: round(g["load_average"][0], 1)
            for g in gates if g.get("load_average")}


def matrix_loads():
    """Range of 1-minute load averages across every matrix sample."""
    loads = []
    for line in (OUT / "measurements.jsonl").read_text().splitlines():
        if line.strip():
            rec = json.loads(line)
            if rec.get("load_average"):
                loads.append(rec["load_average"][0])
    return {"min": round(min(loads), 1), "max": round(max(loads), 1),
            "samples": len(loads)} if loads else None


def main():
    summary_doc = json.loads((OUT / "summary.json").read_text())
    summary = summary_doc["summary"]

    for repo in REPOS:
        stdout = OUT / ("cold-%s-devmap-1.stdout" % repo)
        if stdout.exists():
            payload = json.loads(stdout.read_text())
            FILES_INDEXED[repo] = {
                "devmap_files_indexed": payload.get("files_indexed"),
                "devmap_symbols": payload.get("symbols"),
                "devmap_edges": payload.get("edges"),
            }

    index_sizes = {}
    for repo in REPOS:
        index_sizes[repo] = {}
        for tool, resolve in INDEX_PATHS.items():
            index_sizes[repo][tool] = du_bytes(resolve(repo))

    gates_path = OUT / "gortex-gates.jsonl"
    gortex = ([json.loads(l) for l in gates_path.read_text().splitlines() if l.strip()]
              if gates_path.exists() else [])

    caveats = {
        "not_an_accuracy_ranking": "these tools do not do the same work. Timings "
                                   "are descriptive of each tool's own native "
                                   "command, never a quality or accuracy score.",
        "cbm_no_index_size": "CBM ran with --persistence false and leaves no "
                             "on-disk index to measure.",
        "cbm_memory_carried_forward": "the 20260913-multirepo run measured CBM's "
                                      "sampled peak RSS varying -75% to -96% between "
                                      "two passes, because its workers are too "
                                      "short-lived for the 50ms sampler. This run did "
                                      "ONE pass and so cannot confirm or refute that; "
                                      "cbm_cold_peak_rss_mb records what it caught.",
        "cbm_cold_peak_rss_mb": cbm_rss_observed(summary),
        "machine_shared": "this is an interactive machine. Minimums are the "
                          "defensible statistic; per-sample load_average is in "
                          "measurements.jsonl.",
        "matrix_load_average_1min": matrix_loads(),
        "gortex_load_average_1min": gortex_loads(gortex),
        "competitor_corpora_moved": "three of the four repositories advanced past "
                                    "the commits 20260913-multirepo measured, so "
                                    "competitor timings are NOT a controlled A/B "
                                    "against that report. DevPrism did not move.",
        "edit_stage_appends_a_hash_comment": "the touch stage appends "
                                             "'# devmap-bench edit' to a .rs/.tsx/.go "
                                             "file, where '#' does not start a comment. "
                                             "Every tool sees the same edit, and it is "
                                             "the same edit the prior runs made, but it "
                                             "is a content change that may not parse.",
    }

    result = {
        "benchmark": "DevMap v0.2.2 vs five code-graph tools, four repositories",
        "generated_utc": datetime.datetime.now(datetime.timezone.utc)
                                 .strftime("%Y%m%dT%H%M%SZ"),
        "devmap_version": PLAN["devmap_version"],
        "prior_run": PLAN["prior_run"],
        "method": {
            "repeats": 3,
            "statistic": "minimum of 3 interleaved repeats; median and max in summary",
            "interleaving": "repeat is the outermost loop, so a cell's samples land "
                            "in different load windows",
            "corpora": "each tool indexes its own APFS clone of a snapshot pinned "
                       "at the repository's HEAD; no working checkout is touched",
            "cold_integrity": "index state a tool keeps outside the corpus (DevMap's "
                              "--db store, GitNexus's GITNEXUS_HOME) is deleted "
                              "before every cold run",
            "gortex": "resident daemon, measured at its own log gates with a "
                      "files_reindexed>0 assertion; n=1, not CLI-comparable",
            "tool_provenance": "the five competitor executables were deleted by a "
                               "2026-09-13 cleanup pass and reinstalled from their "
                               "pinned releases; each was verified byte-for-byte "
                               "against the sha256 recorded by 20260913-48cd3c7.",
            "host": " ".join(PLAN["host"]),
        },
        "caveats": caveats,
        "corpus_commits": PLAN["corpus_commits"],
        "tool_identities": PLAN["tool_identities"],
        "devmap_graph": FILES_INDEXED,
        "timings": summary,
        "failures": summary_doc.get("failures", []),
        "gortex_gates": gortex,
        "index_bytes": index_sizes,
    }
    (OUT / "comparison.json").write_text(json.dumps(result, indent=1) + "\n")
    print("wrote comparison.json")

    print("\n%-12s %9s %9s %9s %9s %9s" % ("repo", "devmap", "codegraph",
                                           "graphify", "gitnexus", "gortex"))
    for repo in REPOS:
        row = index_sizes[repo]
        print("%-12s %8s %8s %8s %8s %8s" % (
            repo,
            *["%.0fMB" % (row[t] / 1048576) if row.get(t) else "-"
              for t in ("devmap", "codegraph", "graphify", "gitnexus", "gortex")]))


if __name__ == "__main__":
    main()
