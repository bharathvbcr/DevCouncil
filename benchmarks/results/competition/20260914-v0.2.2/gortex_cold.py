#!/usr/bin/env python3
"""Gortex cold index per repository, measured at the daemon's own log gates.

Gortex is a resident daemon, not a one-shot CLI, so a wall-clock wrapper around
a command would measure the wrong thing. It reports two gates in its log:
  query_ready -- the graph can answer queries
  enriched    -- background enrichment finished
Both are recorded. Neither is directly comparable to another tool's CLI cold
index, and the report keeps them in a separate table for that reason.

The daemon is always stopped in a finally block: this runner owns its lifecycle
and must not leave a resident process behind.
"""

import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

OUT = Path(__file__).resolve().parent
PLAN = json.loads((OUT / "plan.json").read_text())
SCRATCH = Path(PLAN["scratch"])
BIN = json.loads((OUT / "tool-paths.json").read_text())["gortex"]
ENV = {**os.environ, **json.loads((OUT / "gortex-env.json").read_text())}
GATE_READY = '"msg":"daemon: graph queryable"'
GATE_ENRICHED = '"msg":"daemon: enrichment complete"'
TIMEOUT = 1800


def corpus(repo):
    dest = SCRATCH / ("gortex-%s" % repo)
    if dest.exists():
        shutil.rmtree(dest)
    subprocess.run(["cp", "-c", "-R", PLAN["snapshots"][repo], str(dest)],
                   check=True, capture_output=True)
    return dest


def write_config(tree, repo):
    """Register the corpus with the daemon.

    Without this the daemon starts, warms up over an EMPTY graph, and reports
    both gates in under half a second -- an exit-0, gates-found, JSON-written
    measurement of nothing. Observed: every repository returning ~0.4s with a
    byte-identical 552,960-byte (schema-only) database.
    """
    config_dir = Path(ENV["XDG_CONFIG_HOME"]) / "gortex"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "config.yaml").write_text(
        "repos:\n  - path: %s\n    name: bench-%s\n" % (tree, repo.lower())
    )


def assert_indexed(log_text, repo):
    """Refuse to record a run whose own warmup summary says it indexed nothing."""
    summary = None
    for line in log_text.splitlines():
        if '"msg":"daemon: warmup summary"' in line:
            summary = json.loads(line)
    if summary is None:
        raise RuntimeError("%s: no warmup summary in log" % repo)
    if not summary.get("files_reindexed"):
        raise RuntimeError(
            "%s: daemon reported files_reindexed=%s, repos_changed=%s -- it "
            "indexed nothing, so these gates measure an empty graph"
            % (repo, summary.get("files_reindexed"), summary.get("repos_changed"))
        )
    return summary


def run(repo, iteration):
    tree = corpus(repo)
    write_config(tree, repo)
    db = SCRATCH / ("gortex-%s-%d.sqlite" % (repo, iteration))
    for suffix in ("", "-wal", "-shm"):
        stale = Path(str(db) + suffix)
        if stale.exists():
            stale.unlink()
    log_path = OUT / ("gortex-%s-%d.log" % (repo, iteration))

    start = time.perf_counter()
    stream = log_path.open("w")
    proc = subprocess.Popen(
        [BIN, "daemon", "start", "--backend-path", str(db)],
        cwd=str(tree), env=ENV, stdout=stream, stderr=stream,
        start_new_session=True,
    )
    ready = enriched = None
    try:
        while time.perf_counter() - start < TIMEOUT:
            if proc.poll() is not None:
                raise RuntimeError("daemon exited %s" % proc.returncode)
            text = log_path.read_text()
            if ready is None and GATE_READY in text:
                ready = time.perf_counter() - start
            if GATE_ENRICHED in text:
                enriched = time.perf_counter() - start
                break
            time.sleep(0.05)
        if ready is None or enriched is None:
            raise TimeoutError("gates not reached in %ss (ready=%s enriched=%s)"
                               % (TIMEOUT, ready, enriched))
        summary = assert_indexed(log_path.read_text(), repo)
        record = {
            "repo": repo, "iteration": iteration, "tool": "gortex",
            "query_ready_seconds": ready, "enriched_seconds": enriched,
            "files_reindexed": summary.get("files_reindexed"),
            "daemon_total_seconds": summary.get("total_s"),
            "database_bytes": db.stat().st_size if db.exists() else None,
            "load_average": os.getloadavg(), "log": log_path.name,
        }
        with (OUT / "gortex-gates.jsonl").open("a") as handle:
            handle.write(json.dumps(record) + "\n")
        print(json.dumps(record), flush=True)
        return record
    finally:
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(timeout=60)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)
        stream.close()


def main():
    repos = sorted(PLAN["snapshots"]) if sys.argv[1] == "all" else [sys.argv[1]]
    repeats = int(sys.argv[2]) if len(sys.argv) > 2 else 1
    for repo in repos:
        for i in range(1, repeats + 1):
            run(repo, i)


if __name__ == "__main__":
    main()
