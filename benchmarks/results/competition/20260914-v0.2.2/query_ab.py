#!/usr/bin/env python3
"""Controlled query-latency and caller-accuracy A/B: DevMap v0.2.1 vs v0.2.2.

The semantics campaign measured v0.2.2's query latency at roughly a third of the
build-48cd3c7 report's figures. That report and this one were taken on different
days against different corpus commits, so the comparison across them is not
controlled -- and that report itself showed its own campaign and its own
alternating control disagreeing by more across sessions than the two builds
disagreed within one.

So both binaries are measured here, alternating, against the same corpus in the
same session, from their own freshly built stores (the schema differs: 20 vs 22,
so they cannot share one). Correctness is scored alongside latency, because a
faster answer that returns fewer callers is not a faster answer.
"""

import json
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

OUT = Path(__file__).resolve().parent
sys.path.insert(0, str(OUT))
import audit_semantics as A  # noqa: E402

PLAN = json.loads((OUT / "plan.json").read_text())
SCRATCH = Path(PLAN["scratch"])
QAB = SCRATCH / "qab"
RESULTS = OUT / "query-ab.jsonl"
REPS = 5


def binaries():
    doc = json.loads((OUT / "ab-binaries.json").read_text())
    return {k: v["path"] for k, v in doc["binaries"].items()}


def prepare(label, binary):
    """One corpus copy and one freshly built store per arm."""
    corpus = QAB / ("corpus-%s" % label)
    if corpus.exists():
        shutil.rmtree(corpus)
    subprocess.run(["cp", "-c", "-R", PLAN["snapshots"]["DevCouncil"], str(corpus)],
                   check=True, capture_output=True)
    db = QAB / ("%s.sqlite" % label)
    for suffix in ("", "-wal", "-shm", ".owner", ".lock"):
        stale = Path(str(db) + suffix)
        if stale.exists():
            stale.unlink()
    sidecars = Path(str(db) + ".d")
    if sidecars.is_dir():
        shutil.rmtree(sidecars)
    subprocess.run([binary, "--db", str(db), "--progress", "never", "--json",
                    "build", str(corpus)], check=True, capture_output=True)
    return corpus, db


def query(binary, db, corpus, argv_tail, label):
    start = time.perf_counter()
    proc = subprocess.run([binary, "--db", str(db), "--progress", "never", "--json",
                           *argv_tail], cwd=str(corpus), capture_output=True, text=True)
    elapsed = time.perf_counter() - start
    if proc.returncode != 0:
        raise SystemExit("%s failed: %s" % (label, proc.stderr[-500:]))
    return elapsed, proc.stdout


def main():
    rounds = int(sys.argv[1]) if len(sys.argv) > 1 else REPS
    pins = binaries()
    QAB.mkdir(parents=True, exist_ok=True)
    if RESULTS.exists():
        RESULTS.unlink()

    built = {}
    for label, binary in pins.items():
        print("building %s store..." % label, flush=True)
        built[label] = prepare(label, binary)

    order = list(pins.items())
    for rnd in range(1, rounds + 1):
        arms = order if rnd % 2 else list(reversed(order))
        for label, binary in arms:
            corpus, db = built[label]
            for symbol in A.PATHS:
                for op, tail in (("search", ["search", symbol]),
                                 ("callers", ["explore", symbol, "--budget", "8000"])):
                    secs, stdout = query(binary, db, corpus, tail,
                                         "%s-%s-%s" % (label, op, symbol))
                    if op == "search":
                        ok = A.found("devmap", stdout, symbol, A.PATHS[symbol])
                        matched = None
                    else:
                        ok = None
                        got = A.callers("devmap", stdout, symbol)
                        matched = sorted(got & A.EXPECTED[symbol])
                    row = {"binary": label, "round": rnd, "op": op, "symbol": symbol,
                           "seconds": secs, "exact_definition": ok,
                           "matched_pairs": matched}
                    with RESULTS.open("a") as stream:
                        stream.write(json.dumps(row) + "\n")
        print("round %d done" % rnd, flush=True)

    rows = [json.loads(l) for l in RESULTS.read_text().splitlines() if l.strip()]
    labels = list(pins)
    print("\n=== query latency, median ms (%d alternating rounds) ===" % rounds)
    print("%-10s %-20s %10s %10s %9s" % ("op", "symbol", labels[0], labels[1], "change"))
    for op in ("search", "callers"):
        for symbol in A.PATHS:
            a = [r["seconds"] for r in rows
                 if r["binary"] == labels[0] and r["op"] == op and r["symbol"] == symbol]
            b = [r["seconds"] for r in rows
                 if r["binary"] == labels[1] and r["op"] == op and r["symbol"] == symbol]
            ma, mb = statistics.median(a), statistics.median(b)
            spread = max(max(a) - min(a), max(b) - min(b))
            note = "" if abs(mb - ma) > spread else "  (< spread)"
            print("%-10s %-20s %9.1f %9.1f %+8.1f%%%s"
                  % (op, symbol, ma * 1000, mb * 1000, (mb - ma) / ma * 100, note))

    print("\n=== correctness, both arms ===")
    for label in labels:
        defs = [r for r in rows if r["binary"] == label and r["op"] == "search"]
        cal = [r for r in rows if r["binary"] == label and r["op"] == "callers"]
        pairs = set()
        for r in cal:
            pairs |= set(r["matched_pairs"] or [])
        total = sum(len(A.EXPECTED[s]) for s in A.PATHS)
        print("  %-8s exact definitions %d/%d   caller pairs %d/%d"
              % (label, sum(1 for r in defs if r["exact_definition"]), len(defs),
                 len(pairs), total))


if __name__ == "__main__":
    main()
