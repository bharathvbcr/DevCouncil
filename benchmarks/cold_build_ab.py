#!/usr/bin/env python3
"""Interleaved A/B of two DevMap executables on a cold `devmap build --full`.

Each sample builds the corpus into a store that does not exist yet, so every
run is genuinely cold: schema creation, a full parse (``--full`` ignores the
extraction cache, and a new store has none) and a full write. Rounds alternate
which arm goes first (ABBA), the report is n/min/median/max per arm, and both
arms must agree on what they built (files, symbols, edges, unresolved calls)
before any timing is reported: a faster build that wrote something different is
not a measurement.

``--index NAME`` names an index the two arms are expected to differ on. After
every build it is asserted present in arm A's store and absent from arm B's, so
an A/B whose arms converged — `Store::open` heals every index the fresh schema
declares, so a dropped index cannot survive into a CLI build and the B binary
must be built without it — fails instead of comparing a thing with itself.

    python3 benchmarks/cold_build_ab.py --a BIN_A --b BIN_B --corpus DIR \
        [--index idx_unresolved_rows_callee] --rounds 12 --out NEW_DIR

The corpus should be frozen (`git archive`), not a live checkout.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import statistics
import subprocess
import time

ARMS = ("a", "b")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def has_index(db: Path, index: str) -> bool:
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        return conn.execute(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?", (index,)
        ).fetchone()[0] == 1
    finally:
        conn.close()


def find_stage(stages: list[dict], name: str) -> dict | None:
    for stage in stages:
        if stage.get("stage") == name:
            return stage
        found = find_stage(stage.get("sub", []), name)
        if found is not None:
            return found
    return None


def schedule(rounds: int) -> list[tuple[str, str]]:
    """ABBA: each arm leads in exactly half the rounds, and in each half-block."""
    if rounds < 2 or rounds % 2:
        raise ValueError("rounds must be an even number of at least 2")
    return [ARMS if r % 4 in (0, 3) else ARMS[::-1] for r in range(rounds)]


def summarize(values: list[float]) -> dict:
    return {"n": len(values), "min": min(values), "median": statistics.median(values),
            "max": max(values)}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--a", type=Path, required=True)
    parser.add_argument("--b", type=Path, required=True)
    parser.add_argument("--corpus", type=Path, required=True)
    parser.add_argument("--index")
    parser.add_argument("--rounds", type=int, default=12)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    order = schedule(args.rounds)
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    binaries = {"a": args.a.resolve(), "b": args.b.resolve()}
    hashes = {arm: sha256(path) for arm, path in binaries.items()}
    if hashes["a"] == hashes["b"]:
        raise SystemExit("the two arms are the same executable")
    corpus = args.corpus.resolve()

    records: list[dict] = []
    for round_index, arms in enumerate(order):
        for position, arm in enumerate(arms):
            state = out / f"store-{arm}"
            shutil.rmtree(state, ignore_errors=True)
            state.mkdir()
            db = state / "devmap.sqlite"
            load = os.getloadavg()
            begin = time.perf_counter()
            done = subprocess.run([str(binaries[arm]), "--db", str(db), "--json", "--progress", "never",
                                   "build", "--full", str(corpus)], capture_output=True)
            wall = time.perf_counter() - begin
            if done.returncode != 0:
                raise SystemExit(f"round {round_index} arm {arm} failed: {done.stderr.decode()[-2000:]}")
            payload = json.loads(done.stdout.splitlines()[0])
            if args.index and has_index(db, args.index) != (arm == "a"):
                raise SystemExit(f"round {round_index}: arm {arm} ended with {args.index} "
                                 f"{'present' if arm == 'b' else 'absent'}; the arms converged")
            write = find_stage(payload["timings"]["stages"], "persist:write")
            parts = {sub["stage"]: sub["seconds"] for sub in (write or {}).get("sub", [])}
            records.append({
                "round": round_index, "position": position, "arm": arm, "load": load,
                "wall_s": wall,
                "persist_write_s": write["seconds"] if write else None,
                "persist_unresolved_s": parts.get("unresolved"),
                "persist_commit_s": parts.get("commit"),
                "files": payload["files_indexed"], "symbols": payload["symbols"],
                "edges": payload["edges"], "unresolved_calls": payload["unresolved_calls"],
                "store_bytes": db.stat().st_size,
            })
            print(json.dumps(records[-1]), flush=True)

    answers = {(r["files"], r["symbols"], r["edges"], r["unresolved_calls"]) for r in records}
    if len(answers) != 1:
        raise SystemExit(f"arms or rounds disagree on what was built: {sorted(answers)}")
    if {arm: sha256(path) for arm, path in binaries.items()} != hashes:
        raise SystemExit("an executable changed during the A/B; timings invalid")
    metrics = ("wall_s", "persist_write_s", "persist_unresolved_s", "persist_commit_s", "store_bytes")
    summary = {
        "binaries": {arm: {"path": str(binaries[arm]), "sha256": hashes[arm]} for arm in ARMS},
        "corpus": str(corpus), "index": args.index, "rounds": args.rounds,
        "schedule": [list(a) for a in order],
        "built": dict(zip(("files", "symbols", "edges", "unresolved_calls"), answers.pop())),
        "arms": {arm: {key: summarize([r[key] for r in records if r["arm"] == arm])
                       for key in metrics}
                 for arm in ARMS},
        "load_1m_range": [min(r["load"][0] for r in records), max(r["load"][0] for r in records)],
    }
    (out / "records.json").write_text(json.dumps(records, indent=2) + "\n")
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary["arms"], indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
