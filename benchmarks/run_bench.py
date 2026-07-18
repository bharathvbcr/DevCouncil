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
import re
import shutil
import signal
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

# Arm-B defaults: OpenRouter plans and monitors on GLM-5.2. (Qwen3 Max's
# endpoint is capped at ~20 RPM and tripped 429s mid-run; the earlier free
# Nemotron default supported no ``response_format`` variant, which with
# ``require_parameters: true`` 404'd every planning call — see the OpenRouter
# provider's degrade chain. GLM-5.2 supports structured output.)
DEFAULT_DC_TIMEOUT = 2400
DEFAULT_OPENROUTER_MODEL = "z-ai/glm-5.2"
DEFAULT_MONITOR_MODEL = DEFAULT_OPENROUTER_MODEL
DEFAULT_MONITOR_PROVIDER = "openrouter"
DEFAULT_OLLAMA_TIMEOUT = "900"  # per-call when --monitor-provider ollama
SECRETS_PATH = REPO_ROOT / ".devcouncil" / "secrets.env"


def _load_repo_secrets() -> dict[str, str]:
    if not SECRETS_PATH.exists():
        return {}
    secrets: dict[str, str] = {}
    for line in SECRETS_PATH.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, value = stripped.split("=", 1)
        secrets[key.strip()] = value.strip().strip('"').strip("'")
    return secrets


def _resolve_openrouter_key() -> str:
    return os.environ.get("OPENROUTER_API_KEY", "") or _load_repo_secrets().get("OPENROUTER_API_KEY", "")


# Explicit CLI overrides for the arm-B monitor (set from --monitor-think /
# --monitor-num-predict in main). They beat the inherited environment so a
# thinking-budget sweep is driven by flags recorded in the results header,
# not by whatever OLLAMA_* happened to be exported in the shell.
OLLAMA_ARG_OVERRIDES: dict[str, str] = {}


def _bench_env() -> dict:
    """Child-process env for arm B.

    When the monitor uses Ollama, OLLAMA_THINK=false cuts latency materially.
    Raise OLLAMA_TIMEOUT when unset so a single compile call is not aborted while the
    harness still has headroom under DEFAULT_DC_TIMEOUT.
    """
    env = dict(os.environ)
    env.update(OLLAMA_ARG_OVERRIDES)
    env.setdefault("OLLAMA_THINK", "false")
    env.setdefault("OLLAMA_TIMEOUT", DEFAULT_OLLAMA_TIMEOUT)
    env.setdefault("OPENROUTER_MAX_CONCURRENCY", "2")
    # Client-side request pacing (requests/min). Keeps the whole run under the
    # ~20 RPM caps common on OpenRouter endpoints instead of tripping 429s
    # mid-run and burning the router's retry budget (observed: tasks dying
    # blocked on limit_rpm). Override with OPENROUTER_RPM=off to disable.
    env.setdefault("OPENROUTER_RPM", "15")
    return env


def _ollama_model_names() -> set[str]:
    import urllib.error
    import urllib.request

    try:
        with urllib.request.urlopen("http://localhost:11434/api/tags", timeout=5) as resp:
            data = json.loads(resp.read().decode("utf-8"))
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError, OSError):
        return set()
    return {str(m.get("name", "")) for m in data.get("models", []) if m.get("name")}


def _ollama_has_model(tag: str) -> bool:
    if not tag:
        return True
    names = _ollama_model_names()
    if not names:
        return False
    if tag in names:
        return True
    base = tag.split(":")[0]
    return any(n == tag or n.startswith(f"{tag}:") or n.split(":")[0] == base for n in names)


def _preflight(arms: list[str], monitor_model: str, monitor_provider: str,
               executor: str = "claude", probe_executor: bool = True, executor_model: str = "") -> None:
    # The executor is the sweep's single point of failure every arm shares:
    # verify it can actually complete a call before spending on planning.
    uses_claude = "A" in arms or "C" in arms or ("B" in arms and executor == "claude")
    if probe_executor and uses_claude and CLAUDE is not None:
        ok, detail = _probe_executor(executor_model=executor_model)
        if not ok:
            sys.exit(
                "Executor preflight failed — the coding agent cannot run right now "
                f"(session/usage limit or login problem?):\n{detail.strip()[-300:]}\n"
                "Fix the executor (or pass --no-executor-preflight to override) and rerun."
            )
    if "B" not in arms:
        return
    if not _resolve_openrouter_key():
        sys.exit(
            "Set OPENROUTER_API_KEY for DevCouncil planning (arm B), or store it in "
            f"{SECRETS_PATH.relative_to(REPO_ROOT)}."
        )
    if monitor_model and monitor_provider == "ollama":
        names = _ollama_model_names()
        if not names:
            sys.exit("Ollama server not reachable at http://localhost:11434 (arm B monitor).")
        if not _ollama_has_model(monitor_model):
            sys.exit(
                f"Ollama model {monitor_model!r} not found locally. "
                f"Pull it (ollama pull {monitor_model}) or pass --monitor-model <tag>."
            )


def _normalize_cli_output(text: str) -> str:
    """Strip ANSI and Rich markdown bold so stdout parsing matches report text."""
    cleaned = re.sub(r"\x1b\[[0-9;]*m", "", text)
    return re.sub(r"\*\*([^*]+)\*\*", r"\1", cleaned)


def _report_json(ws: Path, env: dict) -> dict | None:
    rc, out = run([DEV, "report", "--json"], cwd=ws, timeout=60, env=env)
    if rc != 0:
        return None
    try:
        data = json.loads(out)
    except (json.JSONDecodeError, TypeError):
        return None
    return data if isinstance(data, dict) else None


def _classify_verdict_from_report(ws: Path, env: dict) -> str | None:
    """Authoritative verdict from persisted graph state (immune to Rich formatting)."""
    data = _report_json(ws, env)
    if data is None:
        return None
    verdict = data.get("verdict")
    return verdict if verdict in {"passed", "blocked", "incomplete"} else None


def _blocking_gap_summary(ws: Path, env: dict) -> list[dict]:
    """What actually blocked this run — gap types + trimmed descriptions.

    A 400-char output tail cannot answer "WHY was correct code blocked?" (the
    median false negative took source-diving to attribute). Persisting the
    blocking gaps with each record makes every future false negative
    self-diagnosing from the results JSON alone."""
    data = _report_json(ws, env)
    if data is None:
        return []
    gaps = data.get("blocking_gaps") or []
    summary = []
    for g in gaps[:10]:
        if not isinstance(g, dict):
            continue
        summary.append({
            "gap_type": g.get("gap_type", "?"),
            "severity": g.get("severity", "?"),
            "description": str(g.get("description", ""))[:300],
        })
    return summary


# Markers that the EXECUTOR never (fully) ran — subscription session caps,
# exhausted credits, or the agent failing to launch at all. A run like this
# measures the infrastructure, not DevCouncil: the gate "blocking" an empty
# workspace is not a calibration data point. Classified as ``error`` so the
# harness retries it on a fresh workspace and, failing that, EXCLUDES it from
# means/calibration instead of polluting them (observed: a session-limited
# sweep reporting 8 tasks 0/N-blocked that never executed a single agent turn).
_EXECUTOR_INFRA_PATTERNS = (
    "failed to start or execute",   # dev run: the executor process never launched
    "hit your session limit",       # claude CLI subscription session cap
    "usage limit reached",
    "credit balance is too low",
    "out of credits",
    # Executor prerequisites missing entirely — the sweep can never produce data
    # (observed 2026-07-03: an 11-task sweep scored arm B 0/N on every task in
    # ~8s each because claude-agent-sdk wasn't installed in the bench venv).
    "agent sdk is not installed",
    "not found on path",            # coding-CLI executable missing (gemini, codex, ...)
    "unknown agent profile",        # executor misconfigured; never starts
)


def _executor_infra_failure(out: str) -> bool:
    text = _normalize_cli_output(out).lower()
    return any(p in text for p in _EXECUTOR_INFRA_PATTERNS)


def _classify_verdict(code: int, out: str, *, ws: Path | None = None, env: dict | None = None) -> str:
    if code == 124 or out.startswith("TIMEOUT"):
        return "timeout"
    verdict: str | None = None
    if ws is not None and env is not None:
        verdict = _classify_verdict_from_report(ws, env)
    if verdict is None:
        text = _normalize_cli_output(out)
        if re.search(r"Passed:\s*Ready for release", text, re.IGNORECASE):
            verdict = "passed"
        elif re.search(r"Blocked:\s*", text):
            verdict = "blocked"
        elif re.search(r"Incomplete:\s*", text):
            verdict = "incomplete"
        else:
            verdict = "error"
    # A "passed" survives (the pipeline demonstrably completed); any other
    # verdict on a run whose executor failed to start is infrastructure noise.
    if verdict != "passed" and _executor_infra_failure(out):
        return "error"
    return verdict


# Infra failures that an IMMEDIATE retry cannot fix: a session/usage-limited or
# out-of-credits executor fails identically on the next attempt while still
# burning the full planning cost first (~$0.10 and ~3 min per wasted retry).
# "failed to start or execute" alone stays retryable — a transient spawn
# failure is exactly what a fresh-workspace retry exists for.
_NONRETRYABLE_INFRA_PATTERNS = (
    "hit your session limit",
    "usage limit reached",
    "credit balance is too low",
    "out of credits",
    # A missing package/executable/profile fails identically on a fresh
    # workspace; retrying only doubles the wasted planning cost.
    "agent sdk is not installed",
    "not found on path",
    "unknown agent profile",
)


def _is_retryable_error(run_info: dict) -> bool:
    """Retry planner/setup flakiness and transient provider limits — not harness
    timeouts, and not executor limits that will fail identically on retry."""
    verdict = run_info.get("verdict")
    tail = (run_info.get("output_tail") or "").lower()
    if verdict == "error":
        return not any(p in tail for p in _NONRETRYABLE_INFRA_PATTERNS)
    if verdict == "timeout":
        return False
    transient = (
        "rate limit" in tail
        or "limit_rpm" in tail
        or "429" in tail
        or "too many requests" in tail
    )
    # A blocked run with zero score that died on OpenRouter throttling is a planner/
    # monitor infra flake, not a measured data point — retry on a fresh workspace.
    if verdict == "blocked" and run_info.get("fraction", 1.0) == 0 and transient:
        return True
    return False


def _probe_executor(timeout: int = 90, executor_model: str = "") -> tuple[bool, str]:
    """One cheap agent call in an empty temp dir.

    Catches session/usage limits, exhausted credits, and login problems for the
    price of a single trivial completion — BEFORE a sweep burns full planning
    cost on tasks whose executor can never run (observed: $1.13 across 8 tasks
    that never executed a single agent turn). Probes on the SAME model the sweep
    will actually use (``executor_model``) — a probe on the CLI's own default model
    can pass or fail independently of whether the model the run is pinned to works."""
    if CLAUDE is None:
        return True, "claude not resolved; probe skipped"
    tmp = tempfile.mkdtemp(prefix="dc_probe_")
    cmd = [CLAUDE, "-p"]
    if executor_model:
        cmd += ["--model", executor_model]
    cmd.append("Reply with exactly: ok")
    try:
        code, out = run(cmd, cwd=tmp, timeout=timeout)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    ok = code == 0 and not _executor_infra_failure(out)
    return ok, out[-300:]


def _sweep_halt_reason(arms_res: dict, probe=None) -> str | None:
    """Decide, after a task, whether continuing the sweep can still produce data.

    An executor stuck behind a session/usage limit poisons every subsequent task
    too — each still pays full planning cost before dying. When the latest task
    shows an executor infra failure, re-probe the executor once; if the probe
    also fails, halt the sweep instead of marching through the remaining tasks.
    Returns a human-readable reason to halt, or None to continue."""
    tails: list[str] = []
    b = arms_res.get("B")
    if b is not None and b.get("verdict") == "error":
        tails.append(b.get("output_tail") or "")
    a = arms_res.get("A")
    if a is not None and a.get("exit") not in (0, None):
        tails.append(a.get("output_tail") or "")
    if not any(_executor_infra_failure(t) for t in tails):
        return None
    probe = probe or _probe_executor
    ok, detail = probe()
    if ok:
        return None
    return f"executor unavailable: {detail.strip()[-200:]}"


SCORE_SCAFFOLD = '''
import importlib.util, copy, sys, json

TARGET = {target!r}
CHECKS = {checks!r}

def raises(fn, *args, exc=Exception):
    try:
        fn(*args)
    except exc:
        return True
    except BaseException:
        # Wrong exception type — including SystemExit/KeyboardInterrupt, which do
        # not subclass Exception and would otherwise kill the whole scaffold and
        # zero every remaining check.
        return False
    return False

def no_mut(fn, arg):
    before = copy.deepcopy(arg)
    try:
        fn(arg)
    except BaseException:
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
    except BaseException:
        # BaseException: an implementation calling sys.exit() (or raising
        # SystemExit) inside one check must fail THAT check, not abort scoring
        # and zero the entire arm.
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


def _kill_process_tree(proc) -> None:
    """Kill the subprocess AND all its descendants.

    The child ran in its own session (``start_new_session``), so signalling the process
    GROUP reaches the whole tree. ``dev e2e`` spawns ``claude`` (and other) grandchildren;
    a plain kill of the direct child leaves those orphaned — still running and spending
    budget long past the per-task timeout. Falls back to a direct kill where process
    groups are unavailable (e.g. Windows)."""
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
    except Exception:
        try:
            proc.kill()
        except Exception:
            pass


def run(cmd, cwd, timeout, input_text=None, env=None):
    # Launch in a new session/process group so a timeout can terminate the ENTIRE tree,
    # not just the direct child — otherwise a slow executor's orphaned grandchildren blow
    # straight through the per-task budget (observed: a 27-min claude attempt under a
    # 20-min cap). This makes ``timeout`` an authoritative bound on the run.
    try:
        proc = subprocess.Popen(
            cmd, cwd=cwd, env=env,
            stdin=subprocess.PIPE if input_text is not None else None,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8", errors="replace",
            start_new_session=True,
        )
    except Exception as exc:  # pragma: no cover
        return 1, f"HARNESS_ERROR: {exc}"
    try:
        out, err = proc.communicate(input=input_text, timeout=timeout)
        return proc.returncode, (out or "") + (err or "")
    except subprocess.TimeoutExpired:
        _kill_process_tree(proc)
        try:
            out, err = proc.communicate(timeout=15)  # drain whatever was buffered
        except Exception:
            out, err = "", ""
        return 124, "TIMEOUT\n" + (out or "") + (err or "")
    except Exception as exc:  # pragma: no cover
        _kill_process_tree(proc)
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


def arm_raw(ws: Path, prompt: str, timeout: int, executor_model: str = "") -> dict:
    t0 = time.monotonic()
    cmd = [CLAUDE, "-p", "--permission-mode", "acceptEdits"]
    if executor_model:
        cmd += ["--model", executor_model]
    code, out = run(cmd, cwd=ws, timeout=timeout, input_text=prompt)
    return {"exit": code, "seconds": round(time.monotonic() - t0, 1), "verdict": None, "cost_usd": 0.0,
            "output_tail": out[-400:]}


_EXECUTOR_PROFILE = "bench"


def _apply_executor_model(ws: Path, model: str) -> None:
    """Pin the ``claude`` executor's own ``--model`` flag for arm B.

    Distinct from the OpenRouter planner/monitor ``model`` role config: this writes a
    CLI-agent profile (``integrations.cli_agents.profiles.bench.model``) that
    ``CodingCliExecutor`` injects as ``claude --model <value>`` (see
    ``_apply_model_override`` in executors/coding_cli.py). ``dev e2e --profile bench``
    then picks it up while ``models.roles.*`` (planning/monitoring) stays untouched.
    """
    import yaml

    cfg_path = ws / ".devcouncil" / "config.yaml"
    cfg = yaml.safe_load(cfg_path.read_text(encoding="utf-8")) or {}
    profiles = cfg.setdefault("integrations", {}).setdefault("cli_agents", {}).setdefault("profiles", {})
    profiles.setdefault(_EXECUTOR_PROFILE, {})["model"] = model
    cfg_path.write_text(yaml.safe_dump(cfg, sort_keys=False), encoding="utf-8")


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


def _apply_acceptance_check_config(ws: Path, samples: int, repair_attempts: int, per_criterion: bool = False) -> None:
    """Pin the per-criterion acceptance-check knobs for this run.

    ``samples`` generates that many INDEPENDENT checks per criterion and decides by
    majority vote; ``repair_attempts`` regenerates a check that failed to RUN from its
    launcher error. Both target a weak/local reviewer: higher ``samples`` outvotes a
    single mis-generated check (the false-block), and ``repair_attempts`` rescues an
    unrunnable check (the under-credited ``incomplete``). Written into the workspace
    config so a single ``dev e2e`` run uses them — and recorded in the results so a
    run's numbers are tied to the settings that produced them.
    """
    import yaml

    cfg_path = ws / ".devcouncil" / "config.yaml"
    cfg = yaml.safe_load(cfg_path.read_text(encoding="utf-8")) or {}
    ac = cfg.setdefault("verification", {}).setdefault("acceptance_checks", {})
    ac["samples"] = samples
    ac["repair_attempts"] = repair_attempts
    ac["per_criterion"] = per_criterion
    cfg_path.write_text(yaml.safe_dump(cfg, sort_keys=False), encoding="utf-8")


def arm_devcouncil(ws: Path, goal: str, model: str, executor: str, timeout: int,
                   monitor_model: str = "", monitor_provider: str = DEFAULT_MONITOR_PROVIDER,
                   monitor_roles: tuple[str, ...] = (),
                   ac_samples: int = 1, ac_repair_attempts: int = 1,
                   ac_per_criterion: bool = False, executor_model: str = "") -> dict:
    t0 = time.monotonic()
    bench_env = _bench_env()
    key = _resolve_openrouter_key()
    if key:
        bench_env["OPENROUTER_API_KEY"] = key
    init_code, init_out = run([DEV, "init"], cwd=ws, timeout=120, env=bench_env)
    dc_dir = ws / ".devcouncil"
    dc_dir.mkdir(parents=True, exist_ok=True)  # defensive: never fail on a missing dir
    cfg_code, cfg_out = run([DEV, "config", "models", "--provider", "openrouter", "-m", model],
                            cwd=ws, timeout=120, env=bench_env)
    # If config.yaml was not produced, `dev init`/`dev config models` failed. Fail loudly
    # with their output instead of crashing cryptically later when the routing/AC helpers
    # try to read a non-existent config (e.g. the init regression where logging pre-created
    # .devcouncil/ so `dev init` skipped writing config).
    if not (dc_dir / "config.yaml").exists():
        tail = (init_out + "\n" + cfg_out).strip()[-600:]
        return {"exit": cfg_code or init_code or 1, "seconds": round(time.monotonic() - t0, 1),
                "verdict": "error", "cost_usd": 0.0,
                "output_tail": f"SETUP FAILED: no .devcouncil/config.yaml after init/config.\n{tail}"}
    # Route execution-time review gates to the monitor provider (OpenRouter by default).
    if monitor_model and monitor_roles:
        _apply_monitor_routing(ws, list(monitor_roles), monitor_provider, monitor_model)
    _apply_acceptance_check_config(ws, ac_samples, ac_repair_attempts, ac_per_criterion)
    e2e_cmd = [DEV, "e2e", goal, "--executor", executor, "--force", "--continue-on-blocked"]
    if executor_model and executor == "claude":
        _apply_executor_model(ws, executor_model)
        e2e_cmd += ["--profile", _EXECUTOR_PROFILE]
    code, out = run(e2e_cmd, cwd=ws, timeout=timeout, env=bench_env)
    verdict = _classify_verdict(code, out, ws=ws, env=bench_env)
    cost = 0.0
    sc, so = run([DEV, "status", "--json"], cwd=ws, timeout=60, env=bench_env)
    try:
        cost = float(json.loads(so).get("total_cost", 0.0))
    except Exception:
        pass
    # Persist WHAT blocked, not just THAT it blocked — false negatives must be
    # attributable from the results JSON without re-running or source-diving.
    blocking = _blocking_gap_summary(ws, bench_env) if verdict == "blocked" else []
    return {"exit": code, "seconds": round(time.monotonic() - t0, 1), "verdict": verdict,
            "cost_usd": round(cost, 4), "output_tail": out[-400:],
            "blocking_gaps": blocking}


def run_task(task, arms, model, executor, raw_timeout, dc_timeout, score_python, keep, base, dc_retries=1,
             monitor_model="", monitor_provider=DEFAULT_MONITOR_PROVIDER, monitor_roles=(),
             ac_samples=1, ac_repair_attempts=1, ac_per_criterion=False, executor_model=""):
    results = {}
    for arm in arms:
        ws = make_workspace(base / arm, task)
        if arm == "A":
            run_info = arm_raw(ws, task.goal, raw_timeout, executor_model)
        elif arm == "C":
            run_info = arm_raw(ws, task.spec, raw_timeout, executor_model)
        elif arm == "B":
            run_info = arm_devcouncil(ws, task.goal, model, executor, dc_timeout,
                                      monitor_model, monitor_provider, monitor_roles,
                                      ac_samples, ac_repair_attempts, ac_per_criterion, executor_model)
            run_info["attempts"] = 1
            # Planners occasionally emit malformed JSON for a non-degradable role,
            # which surfaces as verdict=error and no usable result. Retry a fresh
            # workspace so transient planner flakiness does not poison the data point.
            # Harness timeouts are NOT retried — they already consumed the full budget.
            attempt = 0
            while _is_retryable_error(run_info) and attempt < dc_retries:
                attempt += 1
                if not keep:
                    shutil.rmtree(ws, ignore_errors=True)
                ws = make_workspace(base / f"{arm}_retry{attempt}", task)
                retry_info = arm_devcouncil(ws, task.goal, model, executor, dc_timeout,
                                            monitor_model, monitor_provider, monitor_roles,
                                            ac_samples, ac_repair_attempts, ac_per_criterion, executor_model)
                retry_info["attempts"] = attempt + 1
                # Total spend must include the failed attempt(s): the retried run's own
                # cost alone under-reports what the data point actually cost.
                retry_info["cost_usd"] = round(
                    retry_info.get("cost_usd", 0.0) + run_info.get("cost_usd", 0.0), 4
                )
                run_info = retry_info
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
    decisive = 0    # hard claims only: passed or blocked (not incomplete/error)
    decisive_ok = 0
    # "incomplete" is DevCouncil explicitly declining to certify done (some acceptance
    # criterion lacks passing evidence). It is not a hard claim, but it is still a
    # calibratable signal: cautious-correct when the code is genuinely imperfect, and an
    # under-credit (too conservative) when the code was actually fully correct. Tracking
    # it lets calibration cover EVERY non-error task instead of silently dropping these.
    incomplete_total = 0
    incomplete_cautious = 0      # incomplete + code not full  → correctly withheld a pass
    incomplete_undercredit = 0   # incomplete + code full      → failed to recognize done
    covered = 0                  # all non-error verdicts (passed/blocked/incomplete)
    covered_ok = 0               # verdict consistent with ground truth, incl. incomplete
    silent_total = 0
    silent_surfaced = 0
    timeouts = 0
    infra_errors = 0
    false_neg_detail: list[str] = []  # task → what blocked its (actually correct) code
    for rec in records:
        row = [rec["task"]]
        for a in arms:
            r = rec["arms"].get(a, {})
            row.append(f"{r.get('score','?')}/{r.get('total','?')}")
            # An arm-B infra error (executor never ran / setup failed) measured
            # nothing — its 0-score must not drag the mean correctness down.
            if "fraction" in r and not (a == "B" and r.get("verdict") == "error"):
                agg[a].append(r["fraction"])
        if "B" in arms:
            b = rec["arms"].get("B", {})
            v = b.get("verdict", "?")
            full = b.get("fraction", 0) == 1.0
            if v == "timeout":
                timeouts += 1
            if v == "error":
                infra_errors += 1
            if v == "blocked":
                decisive += 1
                if full:
                    false_neg += 1
                    kinds = ", ".join(
                        f"{g.get('gap_type', '?')} ({g.get('description', '')[:120]})"
                        for g in (b.get("blocking_gaps") or [])[:3]
                    ) or "no blocking-gap detail recorded"
                    false_neg_detail.append(f"{rec['task']}: {kinds}")
                else:
                    decisive_ok += 1
            elif v == "passed":
                decisive += 1
                if full:
                    decisive_ok += 1
                else:
                    false_pos += 1
            elif v == "incomplete":
                incomplete_total += 1
                if full:
                    incomplete_undercredit += 1
                else:
                    incomplete_cautious += 1
            # Overall calibration coverage: judge every verdict DevCouncil actually made
            # (everything except a harness "error") against ground truth. A verdict is
            # consistent when it matches reality: passed↔full, blocked↔not-full, and
            # incomplete↔not-full (a hedge is right only when the code really wasn't done).
            if v in ("passed", "blocked", "incomplete"):
                covered += 1
                consistent = (
                    (v == "passed" and full)
                    or (v == "blocked" and not full)
                    or (v == "incomplete" and not full)
                )
                if consistent:
                    covered_ok += 1
            note = {
                "blocked": "NO" if full else "yes",
                "passed": "yes" if full else "NO",
                "incomplete": "under" if full else "cautious",
                "timeout": "timed out",
                "error": "infra (excluded)",
            }.get(v, "—")
            row.append(v)
            row.append(note)
            # silent-failure surfacing: A shipped a defect; did B avoid silently passing it?
            # Only verdicts B actually MADE count — an error/timeout run made no claim, so
            # crediting it as "did not rubber-stamp" would let a completely broken arm B
            # score a perfect no-silent-pass (observed: a run with 11/11 error verdicts
            # reporting 4/4).
            a = rec["arms"].get("A", {})
            if "A" in arms and a.get("fraction", 1.0) < 1.0 and v in ("passed", "blocked", "incomplete"):
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
        for detail in false_neg_detail:
            lines.append(f"  - {detail}")
        lines.append(f"- **False positives (passed incorrect code):** {false_pos}")
        if timeouts:
            lines.append(f"- **Harness timeouts (arm B):** {timeouts}/{len(records)}")
        if infra_errors:
            lines.append(
                f"- **Infra errors (arm B, excluded from means/calibration):** "
                f"{infra_errors}/{len(records)} — executor/session/provider failures, not verdicts"
            )
        if decisive:
            lines.append(f"- **Decisive-verdict accuracy (passed/blocked):** {decisive_ok}/{decisive} = {decisive_ok/decisive:.0%}")
        if covered:
            lines.append(
                f"- **Verdict calibration incl. incomplete:** {covered_ok}/{covered} = {covered_ok/covered:.0%} "
                f"(covers all {covered} non-error task(s))"
            )
        if incomplete_total:
            lines.append(
                f"- **Incomplete verdicts:** {incomplete_total} "
                f"(cautious on imperfect code: {incomplete_cautious}, "
                f"under-credited correct code: {incomplete_undercredit})"
            )
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
    ap.add_argument("--model", default=DEFAULT_OPENROUTER_MODEL, help="DevCouncil planner model (OpenRouter).")
    ap.add_argument("--executor", default="claude", help="DevCouncil executor (agent).")
    ap.add_argument("--executor-model", default="sonnet",
                    help="Model the Claude Code executor itself runs on (arms A/C's `claude -p` "
                         "and arm B's `dev e2e --profile bench`), passed via `claude --model`. "
                         "Independent of --model/--monitor-model, which are the OpenRouter "
                         "planner/monitor. Pass '' to leave the executor on its own default.")
    ap.add_argument("--repeats", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=300, help="Per-arm raw-agent timeout (s).")
    ap.add_argument("--dc-timeout", type=int, default=DEFAULT_DC_TIMEOUT,
                    help="Per-task DevCouncil e2e timeout (s).")
    ap.add_argument("--dc-retries", type=int, default=2,
                    help="Retries when a DevCouncil run errors or hits transient provider limits.")
    ap.add_argument("--task-gap", type=int, default=15,
                    help="Seconds to pause between arm-B tasks (spreads OpenRouter RPM).")
    ap.add_argument("--score-python", default=sys.executable, help="Interpreter used to run hidden tests.")
    ap.add_argument("--out", default=str(Path(__file__).parent / "results"))
    ap.add_argument("--keep-workspaces", action="store_true")
    ap.add_argument("--monitor-model", default=DEFAULT_MONITOR_MODEL,
                    help="Route execution-time review roles to this model on --monitor-provider "
                         f"(default: {DEFAULT_MONITOR_MODEL} on OpenRouter). Pass '' to disable.")
    ap.add_argument("--monitor-provider", default=DEFAULT_MONITOR_PROVIDER,
                    help="Provider for the execution-time review roles when --monitor-model is set.")
    ap.add_argument("--monitor-roles", default="implementation_reviewer,live_reviewer",
                    help="Comma list of roles to route to --monitor-provider/--monitor-model. "
                         "Both fire DURING e2e execution: implementation_reviewer is the "
                         "verification gate that drives the self-repair loop (it can block a "
                         "task); live_reviewer critiques the agent's turn and records an "
                         "advisory card (non-gating).")
    ap.add_argument("--ac-samples", type=int, default=3,
                    help="Arm B: independent per-criterion acceptance checks generated and "
                         "majority-voted. >1 outvotes a single mis-generated check (cuts false "
                         "blocks); cost-free on a local monitor, so raise it (e.g. 3) there.")
    ap.add_argument("--ac-repair-attempts", type=int, default=2,
                    help="Arm B: times a compiled acceptance check that FAILED TO RUN "
                         "(wrong import / broken one-liner) is regenerated from its error and "
                         "re-run. Rescues the under-credited 'incomplete'; can't weaken the gate.")
    ap.add_argument("--ac-per-criterion", action=argparse.BooleanOptionalAction, default=True,
                    help="Arm B: compile ONE acceptance check per model call instead of batching "
                         "all criteria into one prompt. A weak/local monitor omits/mis-attributes "
                         "some when batched; focused per-criterion prompts are far more reliable "
                         "(N× the calls — cheap on a local monitor).")
    ap.add_argument("--monitor-think", default=None,
                    choices=["false", "true", "low", "medium", "high"],
                    help="Thinking mode/budget for the local monitor (sets OLLAMA_THINK for "
                         "arm B; low|medium|high are budget levels on models that support them, "
                         "Ollama >= 0.12). Default: false (latency).")
    ap.add_argument("--executor-preflight", action=argparse.BooleanOptionalAction, default=True,
                    help="Probe the coding agent with one trivial call before the sweep, and "
                         "halt the sweep when a mid-run executor infra failure re-probes as "
                         "still down. Catches session/usage limits before they burn the "
                         "planning budget on tasks that can never run.")
    ap.add_argument("--monitor-num-predict", type=int, default=None,
                    help="Hard cap on monitor generation tokens (sets OLLAMA_NUM_PREDICT for "
                         "arm B). Bounds a runaway thinking spiral to a fast, healable "
                         "truncation instead of an HTTP-timeout stall.")
    args = ap.parse_args()
    monitor_roles = tuple(r.strip() for r in args.monitor_roles.split(",") if r.strip())
    if args.monitor_think:
        OLLAMA_ARG_OVERRIDES["OLLAMA_THINK"] = args.monitor_think
    if args.monitor_num_predict:
        OLLAMA_ARG_OVERRIDES["OLLAMA_NUM_PREDICT"] = str(args.monitor_num_predict)

    if DEV is None and "B" in args.arms:
        sys.exit("Could not find `dev`. Activate the project venv or add it to PATH.")
    if CLAUDE is None and ("A" in args.arms or "C" in args.arms or args.executor == "claude"):
        sys.exit("Could not find `claude`. Install it or pass a different --executor.")

    arms = [a.strip().upper() for a in args.arms.split(",") if a.strip()]
    _preflight(arms, args.monitor_model, args.monitor_provider,
               executor=args.executor, probe_executor=args.executor_preflight,
               executor_model=args.executor_model)
    task_names = list(TASKS_BY_NAME) if args.tasks == "all" else [t.strip() for t in args.tasks.split(",")]
    tasks = [TASKS_BY_NAME[n] for n in task_names if n in TASKS_BY_NAME]

    out_dir = Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    base = Path(tempfile.mkdtemp(prefix="dc_bench_"))
    records = []
    print(f"Running {len(tasks)} task(s) x arms {arms} x {args.repeats} repeat(s). Workspaces: {base}")
    if "B" in arms:
        print(f"  dev e2e timeout: {args.dc_timeout}s")
        if args.monitor_model:
            print(f"  monitor: {args.monitor_provider}/{args.monitor_model} "
                  f"({', '.join(monitor_roles)})")
        print(f"  acceptance checks: samples={args.ac_samples} repair_attempts={args.ac_repair_attempts} "
              f"per_criterion={args.ac_per_criterion}")
        if args.task_gap:
            print(f"  task gap: {args.task_gap}s")
    first = True
    halt_reason: str | None = None
    for task in tasks:
        if halt_reason:
            break
        for rep in range(args.repeats):
            if not first and "B" in arms and args.task_gap > 0:
                time.sleep(args.task_gap)
            first = False
            label = task.name + (f"#{rep+1}" if args.repeats > 1 else "")
            print(f"  -> {label} ...", flush=True)
            run_base = base / f"{task.name}_{rep}"
            arms_res = run_task(task, arms, args.model, args.executor, args.timeout, args.dc_timeout,
                                args.score_python, args.keep_workspaces, run_base, dc_retries=args.dc_retries,
                                monitor_model=args.monitor_model, monitor_provider=args.monitor_provider,
                                monitor_roles=monitor_roles,
                                ac_samples=args.ac_samples, ac_repair_attempts=args.ac_repair_attempts,
                                ac_per_criterion=args.ac_per_criterion, executor_model=args.executor_model)
            for a, r in arms_res.items():
                extra = ""
                if a == "B":
                    extra = f"  verdict={r['verdict']}  ${r['cost_usd']}"
                    if r.get("attempts", 1) > 1:
                        extra += f"  attempts={r['attempts']}"
                print(f"      arm {a}: {r['score']}/{r['total']}{extra}  {r['seconds']}s")
            records.append({"task": label, "arms": arms_res})
            if args.executor_preflight:
                halt_reason = _sweep_halt_reason(
                    arms_res, probe=lambda: _probe_executor(executor_model=args.executor_model)
                )
                if halt_reason:
                    print(f"  !! Halting sweep — {halt_reason}")
                    print("     Completed tasks are kept; rerun the remaining tasks once the executor is back.")
                    break

    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    raw_path = out_dir / f"{ts}.json"
    raw_path.write_text(json.dumps({
        "model": args.model,
        "executor": args.executor,
        "executor_model": args.executor_model,
        "dc_timeout": args.dc_timeout,
        "ollama_env": {
            "OLLAMA_THINK": _bench_env().get("OLLAMA_THINK"),
            "OLLAMA_TIMEOUT": _bench_env().get("OLLAMA_TIMEOUT"),
            "OLLAMA_NUM_PREDICT": _bench_env().get("OLLAMA_NUM_PREDICT"),
        },
        "monitor": ({"provider": args.monitor_provider, "model": args.monitor_model,
                     "roles": list(monitor_roles)} if args.monitor_model else None),
        "acceptance_checks": {"samples": args.ac_samples, "repair_attempts": args.ac_repair_attempts,
                              "per_criterion": args.ac_per_criterion},
        "halted": halt_reason,
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
