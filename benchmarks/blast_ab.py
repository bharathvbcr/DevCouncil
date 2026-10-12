#!/usr/bin/env python3
"""Interleaved A/B of two DevMap executables on `devmap blast` and `devmap suspects`.

Both arms index the same frozen clone once, each into a store of its own, then
alternate (ABBA) on a read of that store. Every sample puts a `git` that logs
its spawns first on PATH, so the report carries the processes started as well
as the wall clock: the claim under test is that blob reads cost a bounded number
of processes, and a wall-clock win that came from somewhere else would not show
it. Both arms must produce the same changed-file list (and, for suspects, the
same suspect commits) before any timing is reported: a faster answer that said
something different is not a measurement.

    python3 benchmarks/blast_ab.py --a BIN_A --b BIN_B --repo CLONE \
        --since HEAD~20 --symptom SYMBOL --rounds 12 --out NEW_DIR

`--repo` should be a throwaway clone of the history under test, not a live
checkout: the run writes `.devcouncil` state into it.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
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


def schedule(rounds: int) -> list[tuple[str, str]]:
    """ABBA: each arm leads in exactly half the rounds, and in each half-block."""
    if rounds < 2 or rounds % 2:
        raise ValueError("rounds must be an even number of at least 2")
    return [ARMS if r % 4 in (0, 3) else ARMS[::-1] for r in range(rounds)]


def summarize(values: list[float]) -> dict:
    return {"n": len(values), "min": min(values), "median": statistics.median(values),
            "max": max(values)}


def real_git() -> str:
    for directory in os.environ["PATH"].split(os.pathsep):
        candidate = Path(directory) / "git"
        if candidate.is_file():
            return str(candidate)
    raise SystemExit("git is not on PATH")


def counting_path(scratch: Path) -> tuple[Path, str]:
    bin_dir = scratch / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    log = scratch / "spawns.log"
    script = bin_dir / "git"
    script.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\nexec '{real_git()}' \"$@\"\n")
    script.chmod(0o755)
    return log, f"{bin_dir}{os.pathsep}{os.environ['PATH']}"


def subcommand(line: str) -> str:
    words = iter(line.split(" "))
    for word in words:
        if word in ("-c", "-C"):
            next(words, None)
        elif not word.startswith("-"):
            return word
    return ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--a", type=Path, required=True)
    parser.add_argument("--b", type=Path, required=True)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--since", default="HEAD~20")
    parser.add_argument("--symptom")
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
    repo = args.repo.resolve()
    log, path = counting_path(out / "scratch")
    env = {**os.environ, "PATH": path, "GIT_CONFIG_GLOBAL": "/dev/null"}
    since = subprocess.run([real_git(), "-C", str(repo), "rev-parse", args.since],
                           capture_output=True, text=True, check=True).stdout.strip()

    dbs = {arm: out / "scratch" / f"store-{arm}" / "devmap.sqlite" for arm in ARMS}
    for arm in ARMS:
        dbs[arm].parent.mkdir(parents=True, exist_ok=True)
        done = subprocess.run([str(binaries[arm]), "--db", str(dbs[arm]), "--progress", "never",
                               "build", "--full", str(repo)], capture_output=True, env=env)
        if done.returncode != 0:
            raise SystemExit(f"arm {arm} could not index the repository: {done.stderr.decode()[-2000:]}")

    commands = {"blast": ["blast", "--since", since]}
    if args.symptom:
        commands["suspects"] = ["suspects", args.symptom, "--since", since]

    records: list[dict] = []
    answers: dict[str, set[str]] = {name: set() for name in commands}
    for round_index, arms in enumerate(order):
        for position, arm in enumerate(arms):
            for name, command in commands.items():
                log.unlink(missing_ok=True)
                load = os.getloadavg()
                begin = time.perf_counter()
                done = subprocess.run([str(binaries[arm]), "--db", str(dbs[arm]), "--root", str(repo),
                                       "--json", "--progress", "never", *command],
                                      capture_output=True, env=env)
                wall = time.perf_counter() - begin
                if done.returncode != 0:
                    raise SystemExit(f"round {round_index} arm {arm} {name} failed: "
                                     f"{done.stderr.decode()[-2000:]}")
                report = json.loads(done.stdout)
                if name == "blast":
                    answer = sorted(f["path"] for f in report["changed_files"])
                    detail = {"changed_files": len(answer), "seeds": len(report["seeds"]),
                              "complete": report["complete"],
                              "unavailable": [u["kind"] for u in report["unavailable"]]}
                else:
                    answer = sorted(s["commit"] for s in report["suspects"])
                    detail = {"suspects": len(answer), "complete": report["complete"],
                              "unavailable": [u["kind"] for u in report["unavailable"]]}
                answers[name].add(json.dumps(answer))
                spawned = [subcommand(line) for line in log.read_text().splitlines()] if log.exists() else []
                records.append({"round": round_index, "position": position, "arm": arm,
                                "command": name, "load": load, "wall_s": wall,
                                "git_spawns": len(spawned),
                                "git_spawns_by_subcommand": {s: spawned.count(s) for s in sorted(set(spawned))},
                                **detail})
                print(json.dumps(records[-1]), flush=True)

    for name, seen in answers.items():
        if len(seen) != 1:
            raise SystemExit(f"{name}: the arms or the rounds disagree on the answer")
    if {arm: sha256(path) for arm, path in binaries.items()} != hashes:
        raise SystemExit("an executable changed during the A/B; timings invalid")
    summary = {
        "binaries": {arm: {"path": str(binaries[arm]), "sha256": hashes[arm]} for arm in ARMS},
        "repo": str(repo), "since": since, "rounds": args.rounds,
        "schedule": [list(a) for a in order],
        "commands": {
            name: {arm: {key: summarize([r[key] for r in records
                                         if r["arm"] == arm and r["command"] == name])
                         for key in ("wall_s", "git_spawns")}
                   for arm in ARMS}
            for name in commands},
        "load_1m_range": [min(r["load"][0] for r in records), max(r["load"][0] for r in records)],
    }
    (out / "records.json").write_text(json.dumps(records, indent=2) + "\n")
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    shutil.rmtree(out / "scratch", ignore_errors=True)
    print(json.dumps(summary["commands"], indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
