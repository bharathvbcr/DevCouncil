#!/usr/bin/env python3
"""Gortex's half of the semantics campaign, against its resident daemon.

Gortex is not a standalone CLI, so it cannot ride in `semantics.py`: its queries
go to a daemon that must already be warm, and its node identity is prefixed by
the repository name registered in config.yaml. The daemon is registered as
`devcouncil-bench` so the identifiers match the other tools' project name and
the build-48cd3c7 report's.

The daemon is always stopped in a finally block -- this runner owns its
lifecycle and must not leave a resident process behind.
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
sys.path.insert(0, str(OUT))
import semantics as S  # noqa: E402
import audit_semantics as A  # noqa: E402

BIN = json.loads((OUT / "tool-paths.json").read_text())["gortex"]
ENV = {**os.environ, **json.loads((OUT / "gortex-env.json").read_text())}
SEM = S.SEM
PROJECT = S.PROJECT
GATE_ENRICHED = '"msg":"daemon: enrichment complete"'
PREFIX = PROJECT + "/"


def corpus():
    dest = SEM / ("gortex-%s" % S.REPO)
    if dest.exists():
        shutil.rmtree(dest)
    subprocess.run(["cp", "-c", "-R", S.PLAN["snapshots"][S.REPO], str(dest)],
                   check=True, capture_output=True)
    return dest


def write_config(tree):
    """Register the corpus, or the daemon warms over an empty graph and still
    reports both gates -- a green measurement of nothing."""
    config_dir = Path(ENV["XDG_CONFIG_HOME"]) / "gortex"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "config.yaml").write_text(
        "repos:\n  - path: %s\n    name: %s\n" % (tree, PROJECT))


def start(tree, db, log_path):
    stream = log_path.open("w")
    proc = subprocess.Popen([BIN, "daemon", "start", "--backend-path", str(db)],
                            cwd=str(tree), env=ENV, stdout=stream, stderr=stream,
                            start_new_session=True)
    start_t = time.perf_counter()
    while time.perf_counter() - start_t < 1800:
        if proc.poll() is not None:
            raise RuntimeError("daemon exited %s" % proc.returncode)
        if GATE_ENRICHED in log_path.read_text():
            break
        time.sleep(0.1)
    else:
        raise TimeoutError("daemon never reported enrichment complete")
    summary = None
    for line in log_path.read_text().splitlines():
        if '"msg":"daemon: warmup summary"' in line:
            summary = json.loads(line)
    if not (summary and summary.get("files_reindexed")):
        raise RuntimeError("daemon indexed nothing: %s" % summary)
    return proc, stream, summary


def symbol_query(symbol, tree):
    """`--index` takes the TRACKED CORPUS PATH, not the backend database."""
    return ["query", "symbol", symbol, "--index", str(tree),
            "--format", "json", "--limit", "100"]


def callers_query(symbol, path, tree, extra=()):
    return ["query", "callers", "%s%s::%s" % (PREFIX, path, symbol),
            "--index", str(tree), "--format", "json", "--depth", "1",
            "--limit", "100", *extra]


def record(label, tail, tree):
    return S.record(label, [BIN, *tail], tree, env_extra={
        k: ENV[k] for k in ENV if k.startswith(("XDG_", "GORTEX_", "DO_NOT"))})


def main():
    tree = corpus()
    write_config(tree)
    db = SEM / "gortex.sqlite"
    for suffix in ("", "-wal", "-shm"):
        stale = Path(str(db) + suffix)
        if stale.exists():
            stale.unlink()
    log_path = OUT / "gortex-semantics.log"
    proc, stream, summary = start(tree, db, log_path)
    print("daemon warm: %s files" % summary["files_reindexed"], flush=True)
    target = tree / S.EDIT_FILE
    original = target.read_bytes()
    try:
        record("coverage-gortex", ["daemon", "status"], tree)
        for rep in range(1, S.REPS + 1):
            for symbol, path in A.PATHS.items():
                record("search-gortex-%s-%d" % (symbol, rep),
                       symbol_query(symbol, tree), tree)
                record("callers-gortex-%s-%d" % (symbol, rep),
                       callers_query(symbol, path, tree), tree)
        print("  queries done", flush=True)

        for i in (1, 2, 3):
            name = "devmap_competition_revision_%d" % i
            target.write_bytes(target.read_bytes() + S.probe_source(name).encode())
            record("edit-gortex-%d" % i,
                   ["call", "reindex_repository", "--arg", "path=%s" % tree], tree)
            record("validate-edit-gortex-%d" % i, symbol_query(name, tree), tree)
        print("  edits done", flush=True)

        text = target.read_bytes().decode()
        target.write_bytes(
            text.replace(S.probe_source("devmap_competition_revision_3"), "").encode())
        record("restore-gortex",
               ["call", "reindex_repository", "--arg", "path=%s" % tree], tree)
        for name in ("devmap_competition_revision_3", S.NEVER_EXISTED):
            record("negative-gortex-%s" % name, symbol_query(name, tree), tree)

        # The prior report's supplemental opt-in request: name-only inclusion.
        for symbol, path in A.PATHS.items():
            # Opt-in tiers are only reachable through the RPC surface; the
            # `query callers` subcommand has no --min-tier flag.
            payload = json.dumps({"id": "%s%s::%s" % (PREFIX, path, symbol),
                                  "depth": 1, "limit": 100,
                                  "min_tier": "text_matched",
                                  "exclude_tests": False})
            record("gortex-callers-nameonly-%s" % symbol,
                   ["call", "get_callers", "--json", payload,
                    "--index", str(tree), "--format", "json"], tree)
        print("  controls done", flush=True)
    finally:
        target.write_bytes(original)
        proc.send_signal(signal.SIGTERM)
        try:
            proc.wait(timeout=60)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)
        stream.close()


if __name__ == "__main__":
    main()
