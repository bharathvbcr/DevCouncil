#!/usr/bin/env python3
"""Assert the published docs quote numbers this run actually measured.

README.md, the DevMap guide, the comparison page and the website copy all carry
benchmark figures. Those are hand-written prose, not generated tables, so
nothing stops one from drifting away from the evidence — which is exactly how a
site becomes a stale claim. This checks each quoted figure against
comparison.json / semantics-audit.json and fails loudly on a mismatch.

It checks the headline figures, not every sentence. A number not listed here is
not verified by it.
"""

import json
import pathlib
import re
import sys

N = pathlib.Path(__file__).resolve().parent
ROOT = N.parents[3]
C = json.loads((N / "comparison.json").read_text())
A = json.loads((N / "semantics-audit.json").read_text())
T = C["timings"]["DevCouncil"]
LAT = A["query_latency_seconds"]

DOCS = ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md",
        "DevCouncil-website-benchmark.md", "DevCouncil-blog.md",
        "benchmarks/README.md")


def s(tool, stage):
    return T[tool][stage]["min"]


def ms(tool, cell):
    return LAT[tool][cell]["median"] * 1000


# (label, expected string, docs it must appear in)
CHECKS = [
    ("devmap cold", "%.3f" % s("devmap", "cold"),
     ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md")),
    ("devmap warm", "%.3f" % s("devmap", "warm"),
     ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md")),
    ("devmap edit", "%.3f" % s("devmap", "touch"),
     ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md")),
    ("codegraph cold", "%.3f" % s("codegraph", "cold"),
     ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md")),
    ("codegraph edit", "%.3f" % s("codegraph", "touch"),
     ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md")),
    ("gitnexus cold", "%.3f" % s("gitnexus", "cold"),
     ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md")),
    ("devmap search ms", "%.1f" % ms("devmap", "search/Rust"),
     ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md",
      "DevCouncil-website-benchmark.md", "DevCouncil-blog.md")),
    ("devmap cold 2dp", "%.2f" % s("devmap", "cold"),
     ("DevCouncil-website-benchmark.md",)),
    ("codegraph edit ms", "%d" % round(s("codegraph", "touch") * 1000),
     ("DevCouncil-website-benchmark.md", "DevCouncil-blog.md")),
    ("devmap edit ms", "%d" % round(s("devmap", "touch") * 1000),
     ("DevCouncil-website-benchmark.md", "DevCouncil-blog.md")),
]


def main():
    text = {d: (ROOT / d).read_text() for d in DOCS}
    failures = []
    for label, needle, docs in CHECKS:
        for doc in docs:
            if needle not in text[doc]:
                failures.append("%s: %r (%s) not found" % (doc, needle, label))

    # The edit loss must be stated wherever the edit win is quoted.
    for doc in ("README.md", "docs/devmap/comparison.md", "docs/devmap/README.md",
                "DevCouncil-website-benchmark.md", "DevCouncil-blog.md"):
        if not re.search(r"CodeGraph[^.]{0,120}(faster|lower edit|wins the edit)",
                         text[doc]):
            failures.append("%s: does not state CodeGraph's edit win" % doc)

    # No CURRENT-CLAIM document may still quote the superseded headline figures.
    # benchmarks/README.md is exempt: it is an archive index, and each run's
    # section there is supposed to keep describing that run's own corpus and
    # medians. Deleting those would falsify the history, not refresh the claim.
    for stale in ("2.031", "957.8", "417.2", "610 MiB", "30–33 ms", "1,186-file"):
        for doc in DOCS:
            if doc == "benchmarks/README.md":
                continue
            if stale in text[doc]:
                failures.append("%s: still quotes superseded %r" % (doc, stale))

    print("checked %d figures across %d documents" % (len(CHECKS), len(DOCS)))
    if failures:
        print("FAILURES (%d):" % len(failures))
        for f in failures:
            print("  ", f)
        return 1
    print("every checked figure matches this run, and every doc states the loss")
    return 0


if __name__ == "__main__":
    sys.exit(main())
