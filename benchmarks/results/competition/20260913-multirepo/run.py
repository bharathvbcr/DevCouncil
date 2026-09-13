#!/usr/bin/env python3
"""Cross-repository competitor comparison: DevMap against five graph tools.

Each tool indexes its OWN copy of every corpus, because these tools write index
state into or beside the tree they are given. Corpus copies are APFS clones of
disposable snapshots, never a working checkout and never a live product index.

Stages use this repository's established vocabulary:
  cold  -- index a corpus the tool has never seen
  warm  -- re-run the tool's update path with nothing changed
  touch -- re-run it after exactly one file's content changed

Timings are descriptive wall-clock for each tool's own native command. They are
not an accuracy score, and the tools do not do the same work: see REPORT.md.

Usage:  run.py <repo> <stage-set> [repeats]
        run.py DevPrism cold 1
        run.py all full 3
"""

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

OUT = Path(__file__).resolve().parent
SCRATCH = Path(json.loads((OUT / "plan.json").read_text())["scratch"])
TOOLS = json.loads((OUT / "tool-paths.json").read_text())
SNAPSHOTS = json.loads((OUT / "plan.json").read_text())["snapshots"]

sys.path.insert(0, str(OUT))
from measure import measure  # noqa: E402

# Tools whose index lives inside or beside the corpus need a private copy.
GRAPH_TOOLS = ("devmap", "codegraph", "cbm", "gitnexus", "graphify")


def corpus_for(tool, repo):
    return SCRATCH / ("%s-%s" % (tool, repo))


def project_name(repo):
    return "bench-%s" % repo.lower()


def make_corpus(tool, repo):
    """APFS clone of the pinned snapshot; removed and recreated for a cold run."""
    dest = corpus_for(tool, repo)
    if dest.exists():
        shutil.rmtree(dest)
    subprocess.run(
        ["cp", "-c", "-R", SNAPSHOTS[repo], str(dest)],
        check=True, capture_output=True,
    )
    return dest


def reset_out_of_tree_state(tool, repo, iteration):
    """Delete index state a tool keeps OUTSIDE the corpus, so cold is cold.

    Most of these tools write their index into or beside the tree, so
    recreating the corpus is enough. Two do not: DevMap's store is a --db path
    in scratch, and GitNexus keeps its index under GITNEXUS_HOME. Left in
    place, both turn a "cold" measurement into an unchanged-refresh -- observed
    as a 0.066s DevMap cold index of 595 files, which is the early-return, not
    a build. A stage that silently did not run must never report the same
    result as one that ran.
    """
    if tool == "devmap":
        db = SCRATCH / ("dm-%s-%d.sqlite" % (repo, iteration))
        for suffix in ("", "-wal", "-shm", ".owner", ".lock"):
            stale = Path(str(db) + suffix)
            if stale.exists():
                stale.unlink()
        sidecars = Path(str(db) + ".d")
        if sidecars.is_dir():
            shutil.rmtree(sidecars)
    elif tool == "gitnexus":
        home = SCRATCH / "gitnexus-home"
        if home.exists():
            shutil.rmtree(home)


def edit_target(repo):
    """The same file the map benchmark touched, so the two runs agree."""
    return json.loads((OUT / "plan.json").read_text())["edit_targets"][repo]


def argv_for(tool, stage, repo, corpus, iteration):
    name = project_name(repo)
    if tool == "devmap":
        db = SCRATCH / ("dm-%s-%d.sqlite" % (repo, iteration))
        return [str(SCRATCH / "devmap"), "--db", str(db),
                "--progress", "never", "--json", "build", str(corpus)]
    if tool == "codegraph":
        if stage == "cold":
            return [TOOLS["codegraph"], "init", str(corpus), "--yes"]
        return [TOOLS["codegraph"], "sync", str(corpus)]
    if tool == "cbm":
        return [TOOLS["cbm"], "cli", "index_repository", "--repo-path", str(corpus),
                "--name", name, "--mode", "full", "--persistence", "false"]
    if tool == "gitnexus":
        return [TOOLS["gitnexus"], "analyze", str(corpus), "--index-only", "--name", name]
    if tool == "graphify":
        return [TOOLS["graphify"], "extract", str(corpus), "--code-only", "--no-cluster"]
    raise ValueError(tool)


def apply_edit(corpus, repo):
    path = corpus / edit_target(repo)
    data = path.read_bytes()
    path.write_bytes(data + b"\n# devmap-bench edit\n")
    return data


def restore_edit(corpus, repo, data):
    (corpus / edit_target(repo)).write_bytes(data)


def run_cell(repo, tool, i, stages):
    """One (repository, tool, repeat) cell: cold, then warm, then touch."""
    corpus = make_corpus(tool, repo)  # cold always starts from a fresh copy
    reset_out_of_tree_state(tool, repo, i)
    timeout = 1800
    if "cold" in stages:
        measure("cold-%s-%s-%d" % (repo, tool, i),
                argv_for(tool, "cold", repo, corpus, i),
                cwd=str(corpus), timeout=timeout)
    if "warm" in stages:
        measure("warm-%s-%s-%d" % (repo, tool, i),
                argv_for(tool, "warm", repo, corpus, i),
                cwd=str(corpus), timeout=timeout)
    if "touch" in stages:
        original = apply_edit(corpus, repo)
        measure("touch-%s-%s-%d" % (repo, tool, i),
                argv_for(tool, "touch", repo, corpus, i),
                cwd=str(corpus), timeout=timeout)
        restore_edit(corpus, repo, original)


def main():
    which = sys.argv[1]
    stage_set = sys.argv[2] if len(sys.argv) > 2 else "cold"
    repeats = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    stages = {"cold": ("cold",), "full": ("cold", "warm", "touch")}[stage_set]
    repos = sorted(SNAPSHOTS) if which == "all" else [which]
    started = time.perf_counter()

    # Repeat is the OUTERMOST loop, so the three samples of any one cell land in
    # three different load windows. Running a cell's repeats back-to-back instead
    # charges a single load excursion entirely to that cell: the first pass of
    # this matrix did exactly that and reported DevMap's GitPulse cold index at
    # 10.1s against a re-measured 3.2s, while every other repository landed
    # within 7% of the independent map benchmark. Interleaving is what makes a
    # minimum across repeats mean anything.
    for rep in range(1, repeats + 1):
        for repo in repos:
            for tool in GRAPH_TOOLS:
                print("--- repeat %d: %s / %s" % (rep, repo, tool), flush=True)
                run_cell(repo, tool, rep, stages)
    print("total %.1fs" % (time.perf_counter() - started))


if __name__ == "__main__":
    main()
