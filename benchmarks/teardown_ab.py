#!/usr/bin/env python3
"""Interleaved A/B of the interval between a build's JSON line and its exit.

`devmap --json build` prints its one result line and then tears the process
down. A caller that waits for the exit (a hook, a script, a benchmark) waits
for that teardown too. This times exactly that interval, for two executables
alternating (ABBA) on one corpus and one store, after the same one-file edit:
each sample appends or removes one comment line in EDIT_FILE, so every build is
an incremental build of one changed file.

    python3 benchmarks/teardown_ab.py --a BIN_A --b BIN_B --corpus DIR \
        --edit-file DIR/path/in/corpus --rounds 20 --out NEW_DIR

The corpus should be frozen (`git archive`) and is restored byte-for-byte at the
end. Both arms must report the same generation shape (files, symbols, edges)
on matching edits, or the run is refused.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import time

ARMS = ("a", "b")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def schedule(rounds: int) -> list[tuple[str, str]]:
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
    parser.add_argument("--edit-file", type=Path, required=True)
    parser.add_argument("--rounds", type=int, default=20)
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
    edit = args.edit_file.resolve()
    if corpus not in edit.parents:
        raise SystemExit("--edit-file must be inside --corpus")
    db = out / "store" / "devmap.sqlite"
    db.parent.mkdir()
    original = edit.read_bytes()

    def build(arm: str) -> tuple[dict, float, float]:
        begin = time.perf_counter()
        process = subprocess.Popen([str(binaries[arm]), "--db", str(db), "--json", "--progress", "never",
                                    "build", str(corpus)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        line = process.stdout.readline()
        printed = time.perf_counter()
        _, stderr = process.communicate(timeout=600)
        exited = time.perf_counter()
        if process.returncode != 0:
            raise SystemExit(f"arm {arm} failed: {stderr.decode()[-2000:]}")
        return json.loads(line), (exited - printed) * 1000, (exited - begin) * 1000

    records: list[dict] = []
    try:
        build("a")  # cold build, untimed: both arms then share an incremental store
        sample = 0
        for round_index, arms in enumerate(order):
            for position, arm in enumerate(arms):
                edited = sample % 2 == 0
                edit.write_bytes(original + (b"\n# teardown A/B edit\n" if edited else b""))
                sample += 1
                load = os.getloadavg()
                payload, tail_ms, total_ms = build(arm)
                records.append({"round": round_index, "position": position, "arm": arm,
                                "edited": edited, "load": load, "line_to_exit_ms": tail_ms,
                                "total_ms": total_ms, "files": payload["files_indexed"],
                                "symbols": payload["symbols"], "edges": payload["edges"]})
                print(json.dumps(records[-1]), flush=True)
    finally:
        edit.write_bytes(original)

    for edited in (True, False):
        shapes = {(r["files"], r["symbols"], r["edges"]) for r in records if r["edited"] == edited}
        if len(shapes) != 1:
            raise SystemExit(f"arms disagree on what was built (edited={edited}): {sorted(shapes)}")
    if {arm: sha256(path) for arm, path in binaries.items()} != hashes:
        raise SystemExit("an executable changed during the A/B; timings invalid")
    summary = {
        "binaries": {arm: {"path": str(binaries[arm]), "sha256": hashes[arm]} for arm in ARMS},
        "corpus": str(corpus), "edit_file": str(edit.relative_to(corpus)), "rounds": args.rounds,
        "schedule": [list(a) for a in order],
        "arms": {arm: {key: summarize([r[key] for r in records if r["arm"] == arm])
                       for key in ("line_to_exit_ms", "total_ms")} for arm in ARMS},
        "load_1m_range": [min(r["load"][0] for r in records), max(r["load"][0] for r in records)],
    }
    (out / "records.json").write_text(json.dumps(records, indent=2) + "\n")
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary["arms"], indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
