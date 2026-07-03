#!/usr/bin/env python
"""Local-monitor calibration probe — no cloud key, no coding agent required.

Measures the exact link the full benchmark showed failing on local (Ollama)
monitor models: can the implementation_reviewer role compile RUNNABLE, FAITHFUL
per-criterion acceptance checks?  It exercises the real production stack
(OllamaProvider -> ModelRouter -> AcceptanceTestCompiler) against implementations
with KNOWN ground truth:

  * a reference implementation  -> every criterion must be PROVEN
    (a criterion not proven here is exactly the under-credited `incomplete` /
    false `blocked` the e2e benchmark reports);
  * a buggy implementation      -> the criteria the bug breaks must NOT be proven
    (proving one is a false pass — a rubber-stamped defect).

Usage (requires a running Ollama server):
    uv run python benchmarks/local_monitor_probe.py                      # config model
    uv run python benchmarks/local_monitor_probe.py --model qwen3:8b
    uv run python benchmarks/local_monitor_probe.py --samples 3 --per-criterion
    OLLAMA_THINK=false uv run python benchmarks/local_monitor_probe.py   # latency mode

Writes benchmarks/results/local_monitor_<ts>.json and prints a summary table.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT / "src"))

from devcouncil.domain.requirement import AcceptanceCriterion, Requirement  # noqa: E402
from devcouncil.domain.task import PlannedFile, Task  # noqa: E402
from devcouncil.llm.provider import OllamaProvider  # noqa: E402
from devcouncil.llm.router import ModelRouter  # noqa: E402
from devcouncil.verification.acceptance_compiler import AcceptanceTestCompiler  # noqa: E402

# --- fixtures: natural-language ACs + reference / buggy implementations ------
# AC descriptions mirror what DevCouncil's spec writer produces (behavioral,
# terse). `broken_by_bug` lists the AC ids the buggy implementation violates —
# those are the ones a calibrated monitor must refuse to prove.

PROBES = {
    "median": {
        "file": "stats.py",
        "reference": (
            "def mean(values):\n"
            "    return sum(values) / len(values)\n"
            "def median(values):\n"
            "    if not values:\n"
            "        raise ValueError('empty')\n"
            "    s = sorted(values); n = len(s); mid = n // 2\n"
            "    return s[mid] if n % 2 else (s[mid - 1] + s[mid]) / 2\n"
        ),
        "buggy": (
            "def mean(values):\n"
            "    return sum(values) / len(values)\n"
            "def median(values):\n"
            "    values.sort()\n"
            "    n = len(values); mid = n // 2\n"
            "    if n % 2:\n"
            "        return values[mid]\n"
            "    return (values[mid - 1] + values[mid]) // 2\n"
        ),
        "acs": {
            "AC-001": "median() returns the middle value for odd-length unsorted input (e.g. median([3, 1, 2]) == 2)",
            "AC-002": "median() returns the average of the two middle values as a float for even-length input (e.g. median([1, 2, 3, 4]) == 2.5)",
            "AC-003": "median() does not mutate the input list",
            "AC-004": "median([]) raises ValueError",
        },
        "broken_by_bug": {"AC-002", "AC-003", "AC-004"},
    },
    "chunk": {
        "file": "lists.py",
        "reference": (
            "def chunk(items, size):\n"
            "    if size <= 0:\n"
            "        raise ValueError('size must be positive')\n"
            "    return [items[i:i + size] for i in range(0, len(items), size)]\n"
        ),
        "buggy": (
            "def chunk(items, size):\n"
            "    out = []\n"
            "    while items:\n"
            "        out.append(items[:size])\n"
            "        items = items[size:]\n"
            "    return out\n"
        ),
        "acs": {
            "AC-001": "chunk() splits a list into sublists of the given size, last chunk shorter if needed (chunk([1,2,3,4,5], 2) == [[1,2],[3,4],[5]])",
            "AC-002": "chunk() raises ValueError when size <= 0",
            "AC-003": "chunk() returns a single chunk containing all items when size >= len(items)",
        },
        # The buggy version loops forever on size<=0 with a non-empty list ONLY;
        # for the AC-002 check (usually chunk([1,2], 0)) it hangs or returns [] —
        # either way the check must not pass.
        "broken_by_bug": {"AC-002"},
    },
    "parse_kv": {
        "file": "config.py",
        "reference": (
            "def parse_kv(text):\n"
            "    out = {}\n"
            "    for line in text.splitlines():\n"
            "        line = line.strip()\n"
            "        if not line or line.startswith('#') or '=' not in line:\n"
            "            continue\n"
            "        k, v = line.split('=', 1)\n"
            "        out[k.strip()] = v.strip()\n"
            "    return out\n"
        ),
        "buggy": (
            "def parse_kv(text):\n"
            "    out = {}\n"
            "    for line in text.splitlines():\n"
            "        if '=' in line:\n"
            "            parts = line.split('=')\n"
            "            out[parts[0].strip()] = parts[1].strip()\n"
            "    return out\n"
        ),
        "acs": {
            "AC-001": "parse_kv() parses newline-separated key=value lines into a dict with whitespace stripped from keys and values",
            "AC-002": "parse_kv() skips blank lines and lines starting with '#'",
            "AC-003": "parse_kv() splits only on the FIRST '=' so values may contain '=' (parse_kv('url=http://x?a=b') == {'url': 'http://x?a=b'})",
        },
        "broken_by_bug": {"AC-002", "AC-003"},
    },
}


def _make_task(name: str, spec: dict) -> tuple[Task, Requirement]:
    req = Requirement(
        id="REQ-001",
        title=name,
        description=f"Implement {name} per acceptance criteria",
        priority="high",
        source="user",
        acceptance_criteria=[
            AcceptanceCriterion(id=ac_id, description=desc, verification_method="unit_test")
            for ac_id, desc in spec["acs"].items()
        ],
    )
    task = Task(
        id=f"TASK-{name}",
        title=f"Implement {name}",
        description=f"Implement {name} in {spec['file']}",
        requirement_ids=[req.id],
        acceptance_criterion_ids=list(spec["acs"]),
        planned_files=[PlannedFile(path=spec["file"], reason="implementation", allowed_change="modify")],
        expected_tests=[],
    )
    return task, req


def _code_context(spec: dict, impl: str) -> str:
    # What the verifier hands the compiler: the current content of the changed file,
    # presented diff-style (all lines added).
    body = "".join(f"+{line}\n" for line in impl.splitlines())
    return f"--- /dev/null\n+++ b/{spec['file']}\n@@ -0,0 +1 @@\n{body}"


def _run_check(command: str, cwd: Path, timeout: int = 30) -> dict:
    try:
        proc = subprocess.run(
            command, shell=True, cwd=cwd, capture_output=True, text=True, timeout=timeout,
        )
        stderr = (proc.stderr or "")[-400:]
        unrunnable = proc.returncode != 0 and any(
            marker in stderr for marker in
            ("SyntaxError", "ModuleNotFoundError", "command not found", "No such file",
             "IndentationError", "not recognized")
        )
        return {"exit": proc.returncode, "unrunnable": unrunnable, "stderr": stderr}
    except subprocess.TimeoutExpired:
        return {"exit": 124, "unrunnable": False, "stderr": "TIMEOUT"}
    except Exception as exc:  # noqa: BLE001
        return {"exit": 1, "unrunnable": True, "stderr": f"launcher: {exc}"}


def _config_model() -> str:
    cfg = REPO_ROOT / ".devcouncil" / "config.yaml"
    if cfg.exists():
        import yaml

        data = yaml.safe_load(cfg.read_text(encoding="utf-8")) or {}
        roles = (data.get("models") or {}).get("roles") or {}
        entry = roles.get("implementation_reviewer") or {}
        if entry.get("model"):
            return str(entry["model"])
    sys.path.insert(0, str(REPO_ROOT / "benchmarks"))
    from run_bench import DEFAULT_MONITOR_MODEL  # noqa: E402

    return DEFAULT_MONITOR_MODEL


async def probe(model: str, samples: int, per_criterion: bool, task_names: list[str]) -> dict:
    results: dict = {
        "model": model,
        "samples": samples,
        "per_criterion": per_criterion,
        # Record the thinking setting so probe runs at different budgets are
        # comparable from the results directory alone.
        "think": os.environ.get("OLLAMA_THINK", "(model default)"),
        "tasks": {},
    }
    for name in task_names:
        spec = PROBES[name]
        task, req = _make_task(name, spec)
        task_result: dict = {"compile_seconds": 0.0, "arms": {}}

        with tempfile.TemporaryDirectory(prefix=f"dc_probe_{name}_") as tmp:
            root = Path(tmp)
            router = ModelRouter(
                OllamaProvider(project_root=root),
                {"implementation_reviewer": {"model": model}},
                project_root=root,
            )
            compiler = AcceptanceTestCompiler(router)

            # Compile ONCE against the reference code context (matches production:
            # the compiler sees the diff under review). The same commands are then
            # run against both implementations, isolating check QUALITY.
            (root / spec["file"]).write_text(spec["reference"], encoding="utf-8")
            t0 = time.monotonic()
            compiled = await compiler.compile_candidates(
                task, [req], _code_context(spec, spec["reference"]),
                samples=samples, per_criterion=per_criterion,
            )
            task_result["compile_seconds"] = round(time.monotonic() - t0, 1)
            task_result["compiled"] = compiled
            task_result["coverage"] = f"{len(compiled)}/{len(spec['acs'])}"

            for arm, impl in (("reference", spec["reference"]), ("buggy", spec["buggy"])):
                (root / spec["file"]).write_text(impl, encoding="utf-8")
                per_ac: dict = {}
                for ac_id in spec["acs"]:
                    candidates = compiled.get(ac_id, [])
                    if not candidates:
                        per_ac[ac_id] = {"verdict": "no_check", "runs": []}
                        continue
                    runs = [{"command": c, **_run_check(c, root)} for c in candidates]
                    passes = sum(1 for r in runs if r["exit"] == 0)
                    fails = sum(1 for r in runs if r["exit"] != 0 and not r["unrunnable"])
                    if passes + fails == 0:
                        verdict = "unrunnable"
                    elif passes > fails:
                        verdict = "proven"
                    elif fails > passes:
                        verdict = "failed"
                    else:
                        verdict = "split"
                    per_ac[ac_id] = {"verdict": verdict, "runs": runs}
                task_result["arms"][arm] = per_ac

        results["tasks"][name] = task_result
    return results


def summarize(results: dict) -> str:
    lines = [
        "",
        f"## Local monitor probe — model `{results['model']}` "
        f"(samples={results['samples']}, per_criterion={results['per_criterion']}, "
        f"think={results.get('think', '(model default)')})",
        "",
        "| task | compile s | coverage | ref proven | buggy caught | false passes |",
        "|---|---|---|---|---|---|",
    ]
    tot_ref_ok = tot_ref = tot_caught = tot_broken = tot_falsepass = 0
    for name, tr in results["tasks"].items():
        spec = PROBES[name]
        ref = tr["arms"].get("reference", {})
        bug = tr["arms"].get("buggy", {})
        ref_ok = sum(1 for v in ref.values() if v["verdict"] == "proven")
        broken = spec["broken_by_bug"]
        caught = sum(1 for ac in broken if bug.get(ac, {}).get("verdict") != "proven")
        false_pass = sum(1 for ac in broken if bug.get(ac, {}).get("verdict") == "proven")
        tot_ref_ok += ref_ok
        tot_ref += len(spec["acs"])
        tot_caught += caught
        tot_broken += len(broken)
        tot_falsepass += false_pass
        lines.append(
            f"| {name} | {tr['compile_seconds']} | {tr['coverage']} "
            f"| {ref_ok}/{len(spec['acs'])} | {caught}/{len(broken)} | {false_pass} |"
        )
    lines += [
        "",
        f"- **Reference proven (higher = fewer false blocks/incompletes):** {tot_ref_ok}/{tot_ref}",
        f"- **Buggy criteria caught (higher = fewer rubber-stamped defects):** {tot_caught}/{tot_broken}",
        f"- **False passes on buggy code (must be 0):** {tot_falsepass}",
    ]
    return "\n".join(lines)


def main() -> None:
    ap = argparse.ArgumentParser(description="Local-monitor acceptance-check calibration probe")
    ap.add_argument("--model", default=None, help="Ollama model tag (default: implementation_reviewer from .devcouncil/config.yaml)")
    ap.add_argument("--samples", type=int, default=1)
    ap.add_argument("--per-criterion", action="store_true")
    ap.add_argument("--tasks", default="all", help="'all' or comma list of: " + ",".join(PROBES))
    ap.add_argument("--out", default=str(Path(__file__).parent / "results"))
    ap.add_argument(
        "--think",
        default=None,
        choices=["false", "true", "low", "medium", "high"],
        help="Thinking mode/budget for the monitor (sets OLLAMA_THINK for this run; "
        "low|medium|high are budget levels on models that support them, Ollama >= 0.12). "
        "Omit to inherit the environment / model default.",
    )
    args = ap.parse_args()

    if args.think is not None:
        # Set BEFORE the provider is constructed — OllamaProvider reads the env once.
        os.environ["OLLAMA_THINK"] = args.think

    model = args.model or _config_model()
    names = list(PROBES) if args.tasks == "all" else [t.strip() for t in args.tasks.split(",") if t.strip() in PROBES]
    print(f"Probing local monitor {model!r} on task(s): {', '.join(names)} ...", flush=True)

    results = asyncio.run(probe(model, args.samples, args.per_criterion, names))
    summary = summarize(results)

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    (out_dir / f"local_monitor_{ts}.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
    (out_dir / f"local_monitor_{ts}.md").write_text(summary, encoding="utf-8")
    print(summary)
    print(f"\nRaw results: {out_dir / f'local_monitor_{ts}.json'}")


if __name__ == "__main__":
    main()
