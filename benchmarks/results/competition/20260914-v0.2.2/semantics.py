#!/usr/bin/env python3
"""Correctness and query-latency campaign: the half the timing matrix does not cover.

`run.py` measures how fast each tool indexes. It says nothing about whether the
resulting graph is *right*. This script reproduces the build-48cd3c7 report's
semantic campaign against DevMap v0.2.2:

  definition search   -- does the tool return the exact symbol at its exact source
                         path, 5 repetitions per symbol per tool
  callers             -- which of five source-inspected direct caller pairs come
                         back in the tool's default response
  edit visibility     -- three functions appended to one file; after the tool's
                         own refresh, can each be found
  deleted probe       -- one of those three removed; after an ordinary refresh,
                         is it correctly absent (a stale index answers "present")
  text baseline       -- ripgrep literal search over the same corpus

Scored by `audit_semantics.py`, which owns the per-tool response parsers.

Only the DevCouncil corpus is used, because that is where the caller pairs were
source-inspected. Every pair was re-verified against THIS corpus commit before
the run -- a ground truth carried forward unchecked is not a ground truth.
"""

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

OUT = Path(__file__).resolve().parent
PLAN = json.loads((OUT / "plan.json").read_text())
TOOLS_BIN = json.loads((OUT / "tool-paths.json").read_text())
SCRATCH = Path(PLAN["scratch"])
SEM = SCRATCH / "sem"
RESULTS = OUT / "semantics-measurements.jsonl"
REPO = "DevCouncil"
PROJECT = "devcouncil-bench"
REPS = 5
TOOLS = ("devmap", "codegraph", "cbm", "gitnexus", "graphify")

# Source-inspected at this corpus commit; see REPORT.md for the call sites.
PROBES = {
    "db_size_gate_bytes": "rust/devmap-extract/src/model.rs",
    "loadBounded": "backend/go_orchestrator/repomap/repomap.go",
    "describe_corpus": "benchmarks/map_bench.py",
}
CALLER_NAMES = ("size_gate_constants_are_their_declared_magnitudes",
                "db_size_gate_scales_with_file_count_and_keeps_a_floor",
                "TestAnInternedGraphIsHeldToTheSameBound", "Load", "main")
EDIT_FILE = "benchmarks/tasks.py"
NEVER_EXISTED = "devmap_competition_nonexistent_8aef7e52"


def corpus_for(tool):
    return SEM / ("%s-%s" % (tool, REPO))


def db_for(tool):
    return SEM / ("%s.sqlite" % tool)


def make_corpus(tool):
    dest = corpus_for(tool)
    if dest.exists():
        shutil.rmtree(dest)
    subprocess.run(["cp", "-c", "-R", PLAN["snapshots"][REPO], str(dest)],
                   check=True, capture_output=True)
    return dest


def reset_out_of_tree(tool):
    """Cold must be cold: delete index state kept outside the corpus."""
    if tool == "devmap":
        db = db_for(tool)
        for suffix in ("", "-wal", "-shm", ".owner", ".lock"):
            stale = Path(str(db) + suffix)
            if stale.exists():
                stale.unlink()
        sidecars = Path(str(db) + ".d")
        if sidecars.is_dir():
            shutil.rmtree(sidecars)
    elif tool == "gitnexus":
        home = SEM / "gitnexus-home"
        if home.exists():
            shutil.rmtree(home)


def record(label, argv, cwd, timeout=1800, env_extra=None):
    """Run one command, persist stdout to <label>.stdout, append a timing row."""
    env = dict(os.environ)
    env["GITNEXUS_HOME"] = str(SEM / "gitnexus-home")
    env.update(env_extra or {})
    start = time.perf_counter()
    timed_out = False
    try:
        proc = subprocess.run([str(a) for a in argv], cwd=str(cwd), env=env,
                              capture_output=True, text=True, timeout=timeout)
        code, stdout, stderr = proc.returncode, proc.stdout, proc.stderr
    except subprocess.TimeoutExpired as exc:
        timed_out = True
        code, stdout, stderr = None, exc.stdout or "", exc.stderr or ""
        if isinstance(stdout, bytes):
            stdout = stdout.decode("utf-8", "replace")
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", "replace")
    elapsed = time.perf_counter() - start
    (OUT / (label + ".stdout")).write_text(stdout)
    (OUT / (label + ".stderr")).write_text(stderr)
    row = {"label": label, "argv": [str(a) for a in argv], "cwd": str(cwd),
           "seconds": elapsed, "exit_code": code, "timed_out": timed_out,
           "measurement_ok": code == 0 and not timed_out,
           "load_average": os.getloadavg()}
    with RESULTS.open("a") as stream:
        stream.write(json.dumps(row) + "\n")
    if not row["measurement_ok"]:
        print("    !! %s exit=%s timed_out=%s" % (label, code, timed_out), flush=True)
    return row


def index_argv(tool, corpus, cold):
    if tool == "devmap":
        return [TOOLS_BIN["devmap"], "--db", db_for(tool), "--progress", "never",
                "--json", "build", corpus]
    if tool == "codegraph":
        return ([TOOLS_BIN["codegraph"], "init", corpus, "--yes"] if cold
                else [TOOLS_BIN["codegraph"], "sync", corpus])
    if tool == "cbm":
        return [TOOLS_BIN["cbm"], "cli", "index_repository", "--repo-path", corpus,
                "--name", PROJECT, "--mode", "full", "--persistence", "false"]
    if tool == "gitnexus":
        return [TOOLS_BIN["gitnexus"], "analyze", corpus, "--index-only",
                "--name", PROJECT]
    if tool == "graphify":
        return [TOOLS_BIN["graphify"], "extract", corpus, "--code-only", "--no-cluster"]
    raise ValueError(tool)


def search_argv(tool, corpus, symbol, path):
    if tool == "devmap":
        return [TOOLS_BIN["devmap"], "--db", db_for(tool), "--progress", "never",
                "--json", "search", symbol]
    if tool == "codegraph":
        return [TOOLS_BIN["codegraph"], "query", symbol, "--path", corpus,
                "--limit", "100", "--json"]
    if tool == "cbm":
        return [TOOLS_BIN["cbm"], "cli", "search_graph", "--name-pattern",
                "^%s$" % symbol, "--project", PROJECT, "--format", "json",
                "--limit", "100"]
    if tool == "gitnexus":
        return [TOOLS_BIN["gitnexus"], "context", symbol, "-r", corpus,
                "-f", path, "-l", "100", "--content"]
    if tool == "graphify":
        return [TOOLS_BIN["graphify"], "explain", symbol,
                "--graph", corpus / "graphify-out" / "graph.json"]
    raise ValueError(tool)


def callers_argv(tool, corpus, symbol, path):
    if tool == "devmap":
        return [TOOLS_BIN["devmap"], "--db", db_for(tool), "--progress", "never",
                "--json", "explore", symbol, "--budget", "8000"]
    if tool == "codegraph":
        return [TOOLS_BIN["codegraph"], "callers", symbol, "--path", corpus,
                "--limit", "100", "--json"]
    if tool == "cbm":
        return [TOOLS_BIN["cbm"], "cli", "trace_path", "--function-name", symbol,
                "--direction", "inbound", "--depth", "1", "--include-tests", "true",
                "--project", PROJECT, "--format", "json", "--limit", "100"]
    if tool == "gitnexus":
        # GitNexus answers definition and incoming calls from one context response.
        return search_argv(tool, corpus, symbol, path)
    if tool == "graphify":
        return [TOOLS_BIN["graphify"], "affected", symbol, "--relation", "calls",
                "--depth", "1", "--graph", corpus / "graphify-out" / "graph.json"]
    raise ValueError(tool)


def probe_source(name):
    return ("\n\ndef %s(value):\n    \"\"\"Benchmark probe appended by semantics.py.\"\"\"\n"
            "    return value\n" % name)


def run_tool(tool):
    print("=== %s" % tool, flush=True)
    corpus = make_corpus(tool)
    reset_out_of_tree(tool)

    record("coverage-%s" % tool, index_argv(tool, corpus, cold=True), corpus)

    for rep in range(1, REPS + 1):
        for symbol, path in PROBES.items():
            record("search-%s-%s-%d" % (tool, symbol, rep),
                   search_argv(tool, corpus, symbol, path), corpus)
            record("callers-%s-%s-%d" % (tool, symbol, rep),
                   callers_argv(tool, corpus, symbol, path), corpus)
    print("  queries done", flush=True)

    if tool == "cbm":
        # CBM's caller rows carry names, not files; this resolves them once.
        record("cbm-caller-locations",
               [TOOLS_BIN["cbm"], "cli", "search_graph", "--project", PROJECT,
                "--name-pattern", "^(%s)$" % "|".join(CALLER_NAMES),
                "--format", "json", "--limit", "100"], corpus)

    target = corpus / EDIT_FILE
    original = target.read_bytes()
    try:
        for i in (1, 2, 3):
            name = "devmap_competition_revision_%d" % i
            target.write_bytes(target.read_bytes() + probe_source(name).encode())
            record("edit-%s-%d" % (tool, i), index_argv(tool, corpus, cold=False), corpus)
            record("validate-edit-%s-%d" % (tool, i),
                   search_argv(tool, corpus, name, EDIT_FILE), corpus)
        print("  edits done", flush=True)

        # Remove revision_3 only, leaving 1 and 2 in place, then refresh normally.
        text = target.read_bytes().decode()
        text = text.replace(probe_source("devmap_competition_revision_3"), "")
        target.write_bytes(text.encode())
        record("restore-%s" % tool, index_argv(tool, corpus, cold=False), corpus)
        for name in ("devmap_competition_revision_3", NEVER_EXISTED):
            record("negative-%s-%s" % (tool, name),
                   search_argv(tool, corpus, name, EDIT_FILE), corpus)

        if tool == "gitnexus":
            # The prior run found GitNexus retained a deleted probe until forced.
            record("gitnexus-repeat-normal-refresh",
                   index_argv(tool, corpus, cold=False), corpus)
            record("gitnexus-repeat-stale-query",
                   search_argv(tool, corpus, "devmap_competition_revision_3",
                               EDIT_FILE), corpus)
            record("gitnexus-force-recovery",
                   [TOOLS_BIN["gitnexus"], "analyze", corpus, "--index-only",
                    "--name", PROJECT, "--force"], corpus)
            record("gitnexus-after-force-query",
                   search_argv(tool, corpus, "devmap_competition_revision_3",
                               EDIT_FILE), corpus)
    finally:
        target.write_bytes(original)
    print("  controls done", flush=True)


def run_ripgrep():
    print("=== ripgrep text baseline", flush=True)
    corpus = corpus_for("devmap")
    for rep in range(1, REPS + 1):
        for symbol in PROBES:
            record("text-ripgrep-%s-%d" % (symbol, rep),
                   ["rg", "--json", "--fixed-strings", symbol, "."], corpus,
                   env_extra={"RIPGREP_CONFIG_PATH": ""})


def main():
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    SEM.mkdir(parents=True, exist_ok=True)
    started = time.perf_counter()
    if which == "all":
        selected = TOOLS
    elif which in TOOLS:
        selected = (which,)
    elif which == "rg":
        selected = ()
    else:
        raise SystemExit("unknown target %r; expected one of %s, 'rg' or 'all'"
                         % (which, ", ".join(TOOLS)))
    for tool in selected:
        run_tool(tool)
    if which in ("all", "rg"):
        run_ripgrep()
    print("total %.1fs" % (time.perf_counter() - started))


if __name__ == "__main__":
    main()
