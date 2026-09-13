#!/usr/bin/env python3
"""Consolidate the matrix, the Gortex gates and on-disk index sizes into one file."""

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


def main():
    summary = json.loads((OUT / "summary.json").read_text())

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

    gortex = [json.loads(l) for l in
              (OUT / "gortex-gates.jsonl").read_text().splitlines() if l.strip()]

    result = {
        "benchmark": "DevMap vs five code-graph tools, four repositories",
        "generated_utc": "20260913T210000Z",
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
            "host": "Apple M5 Pro (Mac17,8), 18 cores, macOS Darwin 27.0.0",
        },
        "caveats": {
            "cbm_memory_unreliable": "CBM's sampled process-tree peak RSS varied "
                                     "-75% to -96% between two passes of the same "
                                     "measurement; its workers are too short-lived "
                                     "for the 50ms sampler. Not reported.",
            "cbm_no_index_size": "CBM ran with --persistence false and leaves no "
                                 "on-disk index to measure.",
            "gortex_scholarlm_load": "the scholarlm Gortex run recorded a 1-minute "
                                     "load average of 16.5; treat it as an upper bound.",
            "not_an_accuracy_ranking": "these tools do not do the same work. Timings "
                                       "are descriptive of each tool's own native "
                                       "command, never a quality or accuracy score.",
        },
        "corpus_commits": PLAN["corpus_commits"],
        "tool_identities": PLAN["tool_identities"],
        "devmap_graph": FILES_INDEXED,
        "timings": summary["summary"],
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
