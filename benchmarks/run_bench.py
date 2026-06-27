#!/usr/bin/env python
"""DevCouncil effectiveness benchmark harness.

Runs each task through the selected arms (raw agent / DevCouncil / raw+spec),
scores the resulting code against a hidden ground-truth suite, and reports
correctness lift, verdict calibration, and overhead. See README.md for the
design and metric definitions.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tasks import TASKS_BY_NAME  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parent.parent


def _resolve(name: str, *preferred: str) -> str | None:
    # Prefer explicit paths (e.g. the project venv) over whatever is on PATH, so
    # the benchmark exercises the LOCAL DevCouncil build rather than a global install.
    for p in preferred:
        if Path(p).exists():
            return p
    return shutil.which(name)


DEV = _resolve("dev", str(REPO_ROOT / ".venv" / "Scripts" / "dev.exe"), str(REPO_ROOT / ".venv" / "bin" / "dev"))
CLAUDE = _resolve("claude", str(Path.home() / ".local" / "bin" / "claude.EXE"), str(Path.home() / ".local" / "bin" / "claude"))


SCORE_SCAFFOLD = '''
import importlib.util, copy, sys, json

TARGET = {target!r}
CHECKS = {checks!r}

def raises(fn, *args, exc=Exception):
    try:
        fn(*args)
    except exc:
        return True
    except Exception:
        return False
    return False

def no_mut(fn, arg):
    before = copy.deepcopy(arg)
    try:
        fn(arg)
    except Exception:
        pass
    return arg == before

def _load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod

try:
    m = _load(TARGET, "bench_mod")
except Exception as e:
    print("LOAD_ERROR", type(e).__name__, e)
    print("BENCH_SCORE 0 %d" % len(CHECKS))
    sys.exit(0)

passed = 0
detail = {{}}
for name, expr in CHECKS.items():
    try:
        ok = bool(eval(expr))
    except Exception:
        ok = False
    detail[name] = ok
    passed += 1 if ok else 0
print("BENCH_DETAIL " + json.dumps(detail))
print("BENCH_SCORE %d %d" % (passed, len(CHECKS)))
'''


def _clean_env() -> dict:
    """Environment for launching a *different* interpreter (the scorer). Strip the
    venv-activation markers so a foreign python uses its own stdlib instead of the
    harness venv's — otherwise PYTHONHOME/VIRTUAL_ENV make even `import json` fail.
    """
    env = dict(os.environ)
    for key in ("PYTHONHOME", "VIRTUAL_ENV", "PYTHONPATH", "PYTHONSTARTUP", "UV_INTERNAL__PYTHONHOME"):
        env.pop(key, None)
    return env


def run(cmd, cwd, timeout, input_text=None, env=None):
    try:
        proc = subprocess.run(
            cmd, cwd=cwd, input=input_text, capture_output=True, text=True,
            encoding="utf-8", errors="replace", timeout=timeout, env=env,
        )
        return proc.returncode, (proc.stdout or "") + (proc.stderr or "")
    except subprocess.TimeoutExpired:
        return 124, "TIMEOUT"
    except Exception as exc:  # pragma: no cover
        return 1, f"HARNESS_ERROR: {exc}"


def git(args, cwd):
    subprocess.run(["git", "-c", "user.email=bench@local", "-c", "user.name=bench", *args],
                   cwd=cwd, capture_output=True, text=True)


def make_workspace(base: Path, task) -> Path:
    ws = base / f"{task.name}"
    ws.mkdir(parents=True, exist_ok=True)
    for fname, content in task.seed.items():
        (ws / fname).write_text(content, encoding="utf-8")
    (ws / "README.md").write_text(f"# {task.name}\n", encoding="utf-8")
    (ws / ".gitignore").write_text(".devcouncil/\n", encoding="utf-8")
    git(["init", "-q"], ws)
    git(["add", "-A"], ws)
    git(["commit", "-qm", "seed"], ws)
    return ws


def score(ws: Path, task, score_python: str) -> dict:
    target = str((ws / task.target_file).resolve())
    if not (ws / task.target_file).exists():
        return {"passed": 0, "total": len(task.checks), "detail": {}, "note": "target file missing"}
    scaffold = SCORE_SCAFFOLD.format(target=target, checks=task.checks)
    sf = ws.parent / f"_score_{task.name}.py"
    sf.write_text(scaffold, encoding="utf-8")
    code, out = run([score_python, str(sf)], cwd=ws, timeout=60, env=_clean_env())
    passed, total, detail = 0, len(task.checks), {}
    for line in out.splitlines():
        if line.startswith("BENCH_SCORE"):
            _, p, t = line.split()
            passed, total = int(p), int(t)
        elif line.startswith("BENCH_DETAIL"):
            try:
                detail = json.loads(line[len("BENCH_DETAIL "):])
            except Exception:
                pass
    note = "load error" if "LOAD_ERROR" in out else ""
    return {"passed": passed, "total": total, "detail": detail, "note": note}


def arm_raw(ws: Path, prompt: str, timeout: int) -> dict:
    t0 = time.monotonic()
    code, out = run([CLAUDE, "-p", "--permission-mode", "acceptEdits"], cwd=ws, timeout=timeout, input_text=prompt)
    return {"exit": code, "seconds": round(time.monotonic() - t0, 1), "verdict": None, "cost_usd": 0.0,
            "output_tail": out[-400:]}


def _apply_monitor_routing(ws: Path, roles: list[str], provider: str, model: str) -> None:
    """Route the execution-time review roles to a different (e.g. local) provider.

    After ``dev config models`` sets every role to the OpenRouter planner, this
    overrides the named review roles' ``provider``/``model`` in the workspace
    config so a single ``dev e2e`` run plans on OpenRouter while the gates that
    guide and monitor execution (implementation_reviewer / live_reviewer) run on
    the local provider. Relies on per-role provider support in ModelRouter.
    """
    import yaml

    cfg_path = ws / ".devcouncil" / "config.yaml"
    cfg = yaml.safe_load(cfg_path.read_text(encoding="utf-8")) or {}
    role_cfg = cfg.setdefault("models", {}).setdefault("roles", {})
    for role in roles:
        entry = role_cfg.setdefault(role, {})
        entry["provider"] = provider
        entry["model"] = model
    cfg_path.write_text(yaml.safe_dump(cfg, sort_keys=False), encoding="utf-8")


def arm_devcouncil(ws: Path, goal: str, model: str, executor: str, timeout: int,
                   monitor_model: str = "", monitor_provider: str = "ollama",
                   monitor_roles: tuple[str, ...] = ()) -> dict:
    t0 = time.monotonic()
    init_code, init_out = run([DEV, "init"], cwd=ws, timeout=120)
    key = os.environ.get("OPENROUTER_API_KEY", "")
    dc_dir = ws / ".devcouncil"
    dc_dir.mkdir(parents=True, exist_ok=True)  # defensive: never fail on a missing dir
    (dc_dir / "secrets.env").write_text(f"OPENROUTER_API_KEY={key}\n", encoding="utf-8")
    run([DEV, "config", "models", "--provider", "openrouter", "-m", model], cwd=ws, timeout=120)
    # Hybrid routing: keep planning on OpenRouter, push the execution-time review
    # gates to a local provider (Ollama) so monitoring is local and cost-free.
    if monitor_model and monitor_roles:
        _apply_monitor_routing(ws, list(monitor_roles), monitor_provider, monitor_model)
    code, out = run([DEV, "e2e", goal, "--executor", executor, "--force", "--continue-on-blocked"],
                    cwd=ws, timeout=timeout)
    if "Passed: Ready for release" in out:
        verdict = "passed"
    elif "Blocked:" in out:
        verdict = "blocked"
    elif "Incomplete:" in out:
        verdict = "incomplete"
    else:
        verdict = "error"
    cost = 0.0
    sc, so = run([DEV, "status", "--json"], cwd=ws, timeout=60)
    try:
        cost = float(json.loads(so).get("total_cost", 0.0))
    except Exception:
        pass
    return {"exit": code, "seconds": round(time.monotonic() - t0, 1), "verdict": verdict,
            "cost_usd": round(cost, 4), "output_tail": out[-400:]}


def run_task(task, arms, model, executor, raw_timeout, dc_timeout, score_python, keep, base, dc_retries=1,
             monitor_model="", monitor_provider="ollama", monitor_roles=()):
    results = {}
    for arm in arms:
        ws = make_workspace(base / arm, task)
        if arm == "A":
            run_info = arm_raw(ws, task.goal, raw_timeout)
        elif arm == "C":
            run_info = arm_raw(ws, task.spec, raw_timeout)
        elif arm == "B":
            run_info = arm_devcouncil(ws, task.goal, model, executor, dc_timeout,
                                      monitor_model, monitor_provider, monitor_roles)
            # Planners occasionally emit malformed JSON for a non-degradable role,
            # which surfaces as verdict=error and no usable result. Retry a fresh
            # workspace so transient planner flakiness does not poison the data point.
            attempt = 0
            while run_info["verdict"] == "error" and attempt < dc_retries:
                attempt += 1
                if not keep:
                    shutil.rmtree(ws, ignore_errors=True)
                ws = make_workspace(base / f"{arm}_retry{attempt}", task)
                run_info = arm_devcouncil(ws, task.goal, model, executor, dc_timeout,
                                          monitor_model, monitor_provider, monitor_roles)
        else:
            continue
        sc = score(ws, task, score_python)
        run_info["score"] = sc["passed"]
        run_info["total"] = sc["total"]
        run_info["fraction"] = round(sc["passed"] / sc["total"], 3) if sc["total"] else 0.0
        run_info["detail"] = sc["detail"]
        results[arm] = run_info
        if not keep:
            shutil.rmtree(ws, ignore_errors=True)
    return results


def summarize(records: list[dict], arms: list[str]) -> str:
    lines = ["", "## Results", ""]
    header = "| task | " + " | ".join(f"{a} score" for a in arms)
    if "B" in arms:
        header += " | B verdict | verdict ok? |"
    else:
        header += " |"
    lines.append(header)
    lines.append("|" + "---|" * (len(arms) + (3 if "B" in arms else 1)))
    agg = {a: [] for a in arms}
    false_neg = 0   # verdict=blocked but code is actually full (the bug we are fixing)
    false_pos = 0   # verdict=passed but code is not full
    decisive = 0    # passed or blocked (not incomplete/error)
    decisive_ok = 0
    silent_total = 0
    silent_surfaced = 0
    for rec in records:
        row = [rec["task"]]
        for a in arms:
            r = rec["arms"].get(a, {})
            row.append(f"{r.get('score','?')}/{r.get('total','?')}")
            if "fraction" in r:
                agg[a].append(r["fraction"])
        if "B" in arms:
            b = rec["arms"].get("B", {})
            v = b.get("verdict", "?")
            full = b.get("fraction", 0) == 1.0
            if v == "blocked":
                decisive += 1
                if full:
                    false_neg += 1
                else:
                    decisive_ok += 1
            elif v == "passed":
                decisive += 1
                if full:
                    decisive_ok += 1
                else:
                    false_pos += 1
            note = {"blocked": "NO" if full else "yes", "passed": "yes" if full else "NO"}.get(v, "—")
            row.append(v)
            row.append(note)
            # silent-failure surfacing: A shipped a defect; did B avoid silently passing it?
            a = rec["arms"].get("A", {})
            if "A" in arms and a.get("fraction", 1.0) < 1.0:
                silent_total += 1
                if v != "passed" or full:  # B did not falsely pass a defect (blocked/incomplete, or fixed it)
                    silent_surfaced += 1
        lines.append("| " + " | ".join(str(c) for c in row) + " |")
    lines.append("")
    lines.append("## Aggregate")
    for a in arms:
        vals = agg[a]
        if vals:
            lines.append(f"- **Arm {a} mean correctness:** {sum(vals)/len(vals):.2f} (n={len(vals)})")
    if "A" in arms and "B" in arms and agg["A"] and agg["B"]:
        lift = sum(agg["B"]) / len(agg["B"]) - sum(agg["A"]) / len(agg["A"])
        lines.append(f"- **Correctness lift (B − A):** {lift:+.2f}")
    if "B" in arms:
        lines.append(f"- **False negatives (blocked correct code):** {false_neg}  ← lower is better")
        lines.append(f"- **False positives (passed incorrect code):** {false_pos}")
        if decisive:
            lines.append(f"- **Decisive-verdict accuracy:** {decisive_ok}/{decisive} = {decisive_ok/decisive:.0%}")
    if "B" in arms and silent_total:
        lines.append(f"- **No-silent-pass on raw defects:** {silent_surfaced}/{silent_total} (B never rubber-stamped a defect)")
    if "B" in arms:
        total_cost = sum(rec["arms"].get("B", {}).get("cost_usd", 0.0) for rec in records)
        lines.append(f"- **Total DevCouncil planning cost:** ${total_cost:.4f}")
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description="DevCouncil effectiveness benchmark")
    ap.add_argument("--arms", default="A,B", help="Comma list of arms: A (raw), B (devcouncil), C (raw+spec).")
    ap.add_argument("--tasks", default="all", help="'all' or comma list of task names.")
    ap.add_argument("--model", default="google/gemini-2.5-flash", help="DevCouncil planner model (OpenRouter).")
    ap.add_argument("--executor", default="claude", help="DevCouncil executor (agent).")
    ap.add_argument("--repeats", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=300, help="Per-arm raw-agent timeout (s).")
    ap.add_argument("--dc-timeout", type=int, default=1200, help="Per-task DevCouncil e2e timeout (s).")
    ap.add_argument("--dc-retries", type=int, default=1, help="Retries when a DevCouncil run errors (planner flakiness).")
    ap.add_argument("--score-python", default=sys.executable, help="Interpreter used to run hidden tests.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "results"))
    ap.add_argument("--keep-workspaces", action="store_true")
    ap.add_argument("--monitor-model", default="",
                    help="If set, route the execution-time review roles to this model on --monitor-provider "
                         "(hybrid: OpenRouter plans, local model guides/monitors). e.g. an Ollama tag.")
    ap.add_argument("--monitor-provider", default="ollama",
                    help="Provider for the execution-time review roles when --monitor-model is set.")
    ap.add_argument("--monitor-roles", default="implementation_reviewer,live_reviewer",
                    help="Comma list of roles to route to --monitor-provider/--monitor-model.")
    args = ap.parse_args()
    monitor_roles = tuple(r.strip() for r in args.monitor_roles.split(",") if r.strip())

    if DEV is None and "B" in args.arms:
        sys.exit("Could not find `dev`. Activate the project venv or add it to PATH.")
    if CLAUDE is None and ("A" in args.arms or "C" in args.arms or args.executor == "claude"):
        sys.exit("Could not find `claude`. Install it or pass a different --executor.")
    if "B" in args.arms and not os.environ.get("OPENROUTER_API_KEY"):
        sys.exit("Set OPENROUTER_API_KEY for DevCouncil planning (arm B).")

    arms = [a.strip().upper() for a in args.arms.split(",") if a.strip()]
    task_names = list(TASKS_BY_NAME) if args.tasks == "all" else [t.strip() for t in args.tasks.split(",")]
    tasks = [TASKS_BY_NAME[n] for n in task_names if n in TASKS_BY_NAME]

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    base = Path(tempfile.mkdtemp(prefix="dc_bench_"))
    records = []
    print(f"Running {len(tasks)} task(s) x arms {arms} x {args.repeats} repeat(s). Workspaces: {base}")
    for task in tasks:
        for rep in range(args.repeats):
            label = task.name + (f"#{rep+1}" if args.repeats > 1 else "")
            print(f"  -> {label} ...", flush=True)
            run_base = base / f"{task.name}_{rep}"
            arms_res = run_task(task, arms, args.model, args.executor, args.timeout, args.dc_timeout,
                                args.score_python, args.keep_workspaces, run_base, dc_retries=args.dc_retries,
                                monitor_model=args.monitor_model, monitor_provider=args.monitor_provider,
                                monitor_roles=monitor_roles)
            for a, r in arms_res.items():
                print(f"      arm {a}: {r['score']}/{r['total']}"
                      + (f"  verdict={r['verdict']}  ${r['cost_usd']}" if a == "B" else "")
                      + f"  {r['seconds']}s")
            records.append({"task": label, "arms": arms_res})

    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    raw_path = out_dir / f"{ts}.json"
    raw_path.write_text(json.dumps({
        "model": args.model,
        "executor": args.executor,
        "monitor": ({"provider": args.monitor_provider, "model": args.monitor_model,
                     "roles": list(monitor_roles)} if args.monitor_model else None),
        "records": records,
    }, indent=2), encoding="utf-8")
    summary = summarize(records, arms)
    (out_dir / f"{ts}.md").write_text(summary, encoding="utf-8")
    print(summary)
    print(f"\nRaw results: {raw_path}")
    if not args.keep_workspaces:
        shutil.rmtree(base, ignore_errors=True)


if __name__ == "__main__":
    main()
