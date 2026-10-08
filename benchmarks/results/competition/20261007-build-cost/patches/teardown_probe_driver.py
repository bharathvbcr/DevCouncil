"""Throwaway driver: attribute the interval between a build's JSON line and its exit.

argv: BINARY CORPUS EDIT_FILE ROUNDS
Each round appends or removes one comment line in EDIT_FILE, runs
`BINARY --json build --root CORPUS` with DEVMAP_TEARDOWN_PROBE=1, and prints the
interval from each probe mark to process exit as observed by this parent.
"""
import json
import os
import statistics
import subprocess
import sys
import time

binary, corpus, edit, rounds = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
env = dict(os.environ, DEVMAP_TEARDOWN_PROBE="1")
original = open(edit, "rb").read()
rows = []
try:
    for i in range(rounds):
        with open(edit, "wb") as f:
            f.write(original + (b"\n// teardown probe edit\n" if i % 2 == 0 else b""))
        start = time.time()
        p = subprocess.Popen([binary, "--json", "build", "--root", corpus],
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
        line = p.stdout.readline()
        t_line = time.time()
        out_rest, err = p.communicate()
        t_exit = time.time()
        payload = json.loads(line)
        marks = {}
        for raw in err.decode().splitlines():
            if raw.startswith("teardown-probe "):
                _, label, us = raw.split()
                marks[label] = int(us) / 1e6
        order = sorted(marks.items(), key=lambda kv: kv[1])
        steps, prev = {}, marks.get("json:emitted")
        for label, t in order:
            if prev is not None and t >= prev and label != "json:emitted":
                steps[label] = (t - prev) * 1000
                prev = t
        steps["after_last_mark_to_exit"] = (t_exit - order[-1][1]) * 1000
        rows.append({
            "total_ms": (t_exit - start) * 1000,
            "line_to_exit_ms": (t_exit - t_line) * 1000,
            "emitted_to_exit_ms": (t_exit - marks["json:emitted"]) * 1000,
            "steps": steps,
            "generation": payload.get("generation_id"),
        })
        print(json.dumps(rows[-1]), flush=True)
finally:
    with open(edit, "wb") as f:
        f.write(original)

keys = rows[0]["steps"].keys()
print("min/median over", len(rows), "rounds:")
for k in ["total_ms", "line_to_exit_ms", "emitted_to_exit_ms"]:
    v = [r[k] for r in rows]
    print(f"  {k:28} min {min(v):8.2f}  median {statistics.median(v):8.2f}")
for k in keys:
    v = [r["steps"].get(k, 0.0) for r in rows]
    print(f"  {k:28} min {min(v):8.2f}  median {statistics.median(v):8.2f}")
