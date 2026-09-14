#!/usr/bin/env python3
"""Interleaved A/B control: DevMap v0.2.1 against v0.2.2 on the same corpora.

The competitor matrix cannot answer "did v0.2.2 get faster". Comparing this
run's DevMap column against the 20260913-multirepo report would be a SEQUENTIAL
before/after taken hours apart on a shared interactive machine, which this
repository's own benchmarks/README calls out as unreliable: three consecutive
cold runs of one unchanged binary measured 4.35s, 2.96s and 5.20s here.

So the two binaries are alternated A,B,A,B against the same corpus, each round
rebuilding a scratch store from cold, so both arms absorb the same load
excursions. The leading arm swaps every round so neither binary systematically
owns the cold-cache position.

Reports minimum AND median, and says so when the difference is smaller than the
spread. Also compares the graphs the two builds produce: if they are identical,
the intervening commits did not change what was extracted from this workload,
which is itself the finding.
"""

import hashlib
import json
import os
import shutil
import subprocess
import statistics
import sys
import time
from pathlib import Path

OUT = Path(__file__).resolve().parent
PLAN = json.loads((OUT / "plan.json").read_text())
SCRATCH = Path(PLAN["scratch"])
AB = SCRATCH / "ab"
RESULTS = OUT / "ab-measurements.jsonl"
REPOS = ("DevPrism", "DevCouncil", "GitPulse", "scholarlm")
STAGES = ("cold", "warm", "touch")


def binaries():
    """Resolve the two arms, and refuse to run if either is not the pinned build.

    The v0.2.1 arm lives in session scratch and does not survive. Silently
    comparing against whatever happens to sit at that path -- or against nothing
    -- would produce a table that looks exactly like a real result, so the digest
    is checked rather than assumed.
    """
    doc = json.loads((OUT / "ab-binaries.json").read_text())
    pins = {}
    for label, spec in doc["binaries"].items():
        path = Path(spec["path"])
        if not path.exists():
            raise SystemExit(
                "%s: %s does not exist. Rebuild it:\n  %s"
                % (label, path, spec.get("build", "(no build command recorded)")))
        got = hashlib.sha256(path.read_bytes()).hexdigest()
        want = spec.get("sha256", "")
        if want and not got.startswith(want):
            raise SystemExit("%s: sha256 %s does not match pinned %s -- this is not "
                             "the build the report measured" % (label, got[:20], want))
        pins[label] = str(path)
    return pins


def corpus(label, repo):
    dest = AB / ("%s-%s" % (label, repo))
    if dest.exists():
        shutil.rmtree(dest)
    subprocess.run(["cp", "-c", "-R", PLAN["snapshots"][repo], str(dest)],
                   check=True, capture_output=True)
    return dest


def fresh_db(label, repo, rnd):
    db = AB / ("%s-%s-%d.sqlite" % (label, repo, rnd))
    for suffix in ("", "-wal", "-shm", ".owner", ".lock"):
        stale = Path(str(db) + suffix)
        if stale.exists():
            stale.unlink()
    sidecars = Path(str(db) + ".d")
    if sidecars.is_dir():
        shutil.rmtree(sidecars)
    return db


def build(binary, db, tree):
    start = time.perf_counter()
    proc = subprocess.run([binary, "--db", str(db), "--progress", "never",
                           "--json", "build", str(tree)],
                          capture_output=True, text=True, cwd=str(tree))
    elapsed = time.perf_counter() - start
    if proc.returncode != 0:
        raise SystemExit("build failed (%s): %s" % (proc.returncode, proc.stderr[-2000:]))
    return elapsed, json.loads(proc.stdout)


def run_cell(label, binary, repo, rnd):
    tree = corpus(label, repo)
    db = fresh_db(label, repo, rnd)
    target = tree / PLAN["edit_targets"][repo]

    cold_s, cold_json = build(binary, db, tree)
    cold_store = db.stat().st_size if db.exists() else None
    warm_s, _ = build(binary, db, tree)
    original = target.read_bytes()
    target.write_bytes(original + b"\n# devmap-bench edit\n")
    touch_s, _ = build(binary, db, tree)
    target.write_bytes(original)
    final_store = db.stat().st_size if db.exists() else None

    record = {
        "binary": label, "repo": repo, "round": rnd,
        "cold": cold_s, "warm": warm_s, "touch": touch_s,
        "cold_store_bytes": cold_store, "final_store_bytes": final_store,
        "files_indexed": cold_json.get("files_indexed"),
        "symbols": cold_json.get("symbols"), "edges": cold_json.get("edges"),
        "dead_candidates": cold_json.get("dead_candidates"),
        "communities": cold_json.get("communities"),
        "load_average": os.getloadavg(),
    }
    with RESULTS.open("a") as stream:
        stream.write(json.dumps(record) + "\n")
    print("  %-8s %-11s cold %6.3fs  warm %6.3fs  touch %6.3fs" %
          (label, repo, cold_s, warm_s, touch_s), flush=True)
    return record


def report(rows):
    labels = sorted({r["binary"] for r in rows})
    if len(labels) != 2:
        raise SystemExit("expected two binaries, got %s" % labels)
    a, b = labels

    print("\n=== graph equality (does v0.2.2 extract the same thing?) ===")
    for repo in REPOS:
        fields = ("files_indexed", "symbols", "edges", "dead_candidates", "communities")
        va = {f: {r[f] for r in rows if r["binary"] == a and r["repo"] == repo} for f in fields}
        vb = {f: {r[f] for r in rows if r["binary"] == b and r["repo"] == repo} for f in fields}
        if not va["symbols"] or not vb["symbols"]:
            continue
        same = all(va[f] == vb[f] for f in fields)
        detail = "identical" if same else ", ".join(
            "%s %s->%s" % (f, sorted(va[f]), sorted(vb[f])) for f in fields if va[f] != vb[f])
        print("%-12s %s" % (repo, detail))

    print("\n=== cold store size (MB) ===")
    for repo in REPOS:
        sa = [r["cold_store_bytes"] for r in rows
              if r["binary"] == a and r["repo"] == repo and r.get("cold_store_bytes")]
        sb = [r["cold_store_bytes"] for r in rows
              if r["binary"] == b and r["repo"] == repo and r.get("cold_store_bytes")]
        if not sa or not sb:
            continue
        ma, mb = min(sa) / 1048576, min(sb) / 1048576
        print("%-12s %s %7.0f MB   %s %7.0f MB   %+6.1f%%"
              % (repo, a, ma, b, mb, (mb - ma) / ma * 100))

    print("\n=== timings: %s vs %s (minimum, with median) ===" % (a, b))
    print("%-12s %-6s %9s %9s %9s   %9s %9s %9s   %8s" %
          ("repo", "stage", a + " min", "med", "", b + " min", "med", "", "min delta"))
    for repo in REPOS:
        for stage in STAGES:
            xa = sorted(r[stage] for r in rows if r["binary"] == a and r["repo"] == repo)
            xb = sorted(r[stage] for r in rows if r["binary"] == b and r["repo"] == repo)
            if not xa or not xb:
                continue
            # A difference smaller than either arm's own spread is not a result.
            spread = max(xa[-1] - xa[0], xb[-1] - xb[0])
            delta = xb[0] - xa[0]
            verdict = "" if abs(delta) > spread else "  (< spread)"
            print("%-12s %-6s %8.3fs %8.3fs %9s   %8.3fs %8.3fs %9s   %+7.1f%%%s" %
                  (repo, stage, xa[0], statistics.median(xa), "",
                   xb[0], statistics.median(xb), "", delta / xa[0] * 100, verdict))


def main():
    rounds = int(sys.argv[1]) if len(sys.argv) > 1 else 3
    pins = binaries()
    order = list(pins.items())
    AB.mkdir(parents=True, exist_ok=True)
    if RESULTS.exists():
        RESULTS.unlink()

    print("A/B: %s" % " vs ".join("%s=%s" % (k, Path(v).name) for k, v in order))
    for rnd in range(1, rounds + 1):
        # Swap which arm leads each round, so neither owns the cold-cache slot.
        arms = order if rnd % 2 else list(reversed(order))
        for repo in REPOS:
            print("--- round %d: %s" % (rnd, repo), flush=True)
            for label, binary in arms:
                run_cell(label, binary, repo, rnd)

    rows = [json.loads(l) for l in RESULTS.read_text().splitlines() if l.strip()]
    report(rows)


if __name__ == "__main__":
    main()
