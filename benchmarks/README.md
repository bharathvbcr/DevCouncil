# DevCouncil Effectiveness Benchmark

A reproducible benchmark that measures whether DevCouncil's gated loop actually
improves AI-generated code — and whether its **verdict can be trusted** — versus
running the same coding agent alone.

It is deliberately *adversarial*: every task ships a **hidden** ground-truth test
suite that encodes subtle requirements (edge cases, input mutation, error
handling) which a terse natural-language goal omits. The agent never sees the
hidden tests; they are applied only at scoring time. This is what separates
"passes the happy path" from "actually correct".

## What it measures

For the **same terse goal**, run two (optionally three) arms and score each arm's
final code against the hidden ground truth:

| Arm | Description |
|-----|-------------|
| **A. raw** | The coding agent alone, terse prompt (`claude -p`). The realistic baseline. |
| **B. devcouncil** | `dev e2e --force` — full plan → gated execution → verify → report. |
| **C. raw+spec** *(optional)* | The agent alone, but with an elaborated prompt that lists edge cases. Isolates whether DevCouncil's value is just "a better prompt" vs. the gating/verification. |

### Metrics

Per task:
- **ground_truth_score** — `passed / total` hidden checks for each arm's final code.
- **devcouncil_verdict** — `passed` / `blocked` (arm B only).
- **cost_usd**, **wall_seconds** — planning cost (OpenRouter) and time.

Aggregate (the headline numbers):
- **Correctness lift** — `mean(B.score) − mean(A.score)`. Does the gated loop produce more-correct code?
- **Verdict calibration** — of arm-B tasks DevCouncil reported `passed`, what
  fraction actually scored `== 1.0` (precision); of those it `blocked`, what
  fraction actually scored `< 1.0` (recall of real problems). This is the core
  trust metric: *when DevCouncil says done, is it done?* Reported two ways:
  - **Decisive-verdict accuracy** — over hard claims only (`passed`/`blocked`).
  - **Verdict calibration incl. incomplete** — over *every* non-error task, so a
    run is never silently dropped from the denominator. DevCouncil's third verdict,
    `incomplete` (nothing failing, but some acceptance criterion lacks passing
    evidence), is scored as *cautious-correct* when the code really wasn't full and
    as an *under-credit* (too conservative) when the code actually scored `1.0`.
    The summary also prints the incomplete breakdown (cautious vs. under-credited),
    which typically rises when the reviewer model is too weak to prove the criteria.
- **Silent-failure conversion** — fraction of tasks where the raw agent shipped
  `< 1.0` (an undetected defect) that DevCouncil instead surfaced as `blocked`.
  This is DevCouncil's headline value: turning false confidence into honest gaps.
- **Overhead** — total cost and added wall-clock vs. the lift.

## Running it

Requires: `dev` on PATH (or in the project venv), a coding agent (`claude`),
and `OPENROUTER_API_KEY` in the environment for DevCouncil planning.

```bash
export OPENROUTER_API_KEY=sk-or-...           # for DevCouncil planning (arm B)
python benchmarks/run_bench.py \
    --arms A,B \
    --tasks all \
    --model z-ai/glm-5.2 \                      # OpenRouter planner
    --executor claude \                         # Claude Code
    --out benchmarks/results
```

Defaults for arm B (OpenRouter planner + OpenRouter monitor + Claude executor):
- `--model z-ai/glm-5.2` — OpenRouter planner for all council roles.
- `--dc-timeout 2400` — generous e2e budget for planning + monitoring API calls.
- `--monitor-model z-ai/glm-5.2` with `--monitor-provider openrouter`
  (pass `--monitor-model ''` to skip per-role monitor routing).
- `OPENROUTER_RPM=15` (unless set) — client-side request pacing so a run stays
  under common ~20 RPM endpoint caps instead of tripping 429s mid-run; 429s that
  still occur get their own retry budget (`DEVCOUNCIL_RATE_LIMIT_RETRIES`, default 8).
- Executor infra failures (agent session/usage limits, exhausted credits, the
  executor failing to launch) classify as `verdict=error`: retried on a fresh
  workspace, and excluded from means/calibration — they measure the
  infrastructure, not DevCouncil. Session/usage-limit errors are NOT retried
  (an immediate retry fails identically after burning full planning cost).
- Executor preflight & halt (`--executor-preflight`, on by default): one trivial
  agent call before the sweep catches a session-limited/logged-out executor up
  front; a mid-sweep executor infra failure re-probes the agent and halts the
  sweep (results so far are kept, `"halted"` reason recorded in the JSON) if it
  is still down. Disable with `--no-executor-preflight`.
- A 429 response pushes a cooldown shared by ALL in-flight OpenRouter calls
  (honoring `Retry-After`), so concurrent acceptance-check fan-out backs off
  together instead of each call slamming the same exhausted window.
- `--ac-samples 1` — single check per criterion (no per-criterion flag).
- Loads `OPENROUTER_API_KEY` from `.devcouncil/secrets.env` when the env var is unset.
- When `--monitor-provider ollama`, sets `OLLAMA_THINK=false` and `OLLAMA_TIMEOUT=900` unless
  already set. Override per run with `--monitor-think false|true|low|medium|high` and
  `--monitor-num-predict N` (recorded in results JSON as `ollama_env`).
- Preflight: verifies `OPENROUTER_API_KEY` is set; when monitor provider is Ollama, also checks
  that the local server is up and the model is pulled.
- Harness timeouts surface as `verdict=timeout` and are **not** retried (planner/setup errors are).

Useful flags:
- `--tasks median,chunk` — run a subset.
- `--arms A,B,C` — include the elaborated-prompt control.
- `--repeats 3` — repeat each task to measure variance (agents are stochastic).
- `--timeout 360` — per-arm raw-agent wall-clock cap.
- `--dc-timeout 3600` — raise further if a larger local monitor still runs long.
- `--keep-workspaces` — keep the temp repos for inspection.

Arm-B acceptance-check tuning (how DevCouncil proves each acceptance criterion):
- `--ac-samples N` — generate `N` *independent* per-criterion checks and decide by
  **majority vote** (default `1` for the benchmark pass). A criterion is proven only
  when a strict majority pass; an all-fail is a real defect (blocks); a split is
  inconclusive (non-blocking). `>1` outvotes a single mis-generated check — useful
  for `local_monitor_probe.py` calibration, not the default e2e run.
- `--ac-repair-attempts N` — when a compiled check *fails to run* (wrong import, broken
  one-liner) feed the error back and regenerate the command up to `N` times (default
  `1`). Rescues the under-credited `incomplete`; can never weaken the gate, since a
  check that never ran proves nothing.
- `--ac-per-criterion` — compile **one** acceptance check per model call instead of
  batching every criterion into a single prompt. A weak/local monitor batching N criteria
  into one JSON routinely omits or mis-attributes some (a false `incomplete`); a focused
  single-criterion prompt is far more reliable. Costs N× the calls — cheap on a local
  monitor. Compounds with `--ac-samples`.

For calibrating a weak/local reviewer (not the default e2e pass), use
`local_monitor_probe.py` with `--samples 3 --per-criterion`, or the e2e flags
`--ac-samples 3 --ac-repair-attempts 2 --ac-per-criterion` alongside a larger
`--monitor-model`.

## Local-monitor calibration probe (no cloud key / no agent)

`local_monitor_probe.py` isolates the link the full benchmark showed failing on
local monitors: can the `implementation_reviewer` model compile runnable,
faithful per-criterion acceptance checks? It drives the REAL production stack
(OllamaProvider → ModelRouter → AcceptanceTestCompiler), compiles checks for a
few fixed tasks, then executes those checks against a **reference**
implementation (every criterion must be proven — anything less is exactly the
under-credited `incomplete`/false `blocked` from the e2e benchmark) and a
**buggy** one (the criteria the bug breaks must NOT be proven — a proven one is
a rubber-stamped defect).

```bash
uv run python benchmarks/local_monitor_probe.py                 # config's reviewer model
uv run python benchmarks/local_monitor_probe.py --model qwen3:8b --samples 3 --per-criterion
OLLAMA_THINK=false uv run python benchmarks/local_monitor_probe.py   # latency/quality tradeoff
```

Requires only a running Ollama server. Results land in
`results/local_monitor_<ts>.{json,md}`. Useful Ollama runtime knobs (all env):
`OLLAMA_NUM_CTX` (base context window; requests auto-grow up to
`OLLAMA_MAX_NUM_CTX` when a prompt would otherwise be silently truncated),
`OLLAMA_THINK=true|false|low|medium|high` (thinking models: reasoning dominates
latency — measured ~65x on one compile call — but usually improves check
quality; `low|medium|high` set an explicit thinking BUDGET on models that
support levels, Ollama >= 0.12), `OLLAMA_NUM_PREDICT` (hard cap on generated
tokens — bounds a runaway thinking spiral to a fast, healable truncation
instead of a 600s HTTP timeout), `OLLAMA_MAX_CONCURRENCY` (client-side cap on
in-flight requests, default 2 — fan-out callers otherwise queue server-side
where their read timeouts tick while waiting; `off` disables),
`OLLAMA_KEEP_ALIVE`, `OLLAMA_TIMEOUT`.

Output: a `results/<timestamp>.json` (raw per-run data, including the
`acceptance_checks` settings used) and a printed Markdown summary table.

## Methodology notes & honesty caveats

- **Cheap planner, capable executor.** DevCouncil planning runs on a small model
  (gemini-flash) to keep cost near-zero; execution uses the agent (Claude). The
  brutal investigation showed *free* models break structured planning, so this
  benchmark uses a cheap-but-capable planner by default. Run with
  `--model <stronger>` to measure the ceiling.
- **Stochasticity.** Coding agents are non-deterministic; use `--repeats` for
  signal. A single run is illustrative, not conclusive.
- **Small, self-contained tasks.** Tasks are single-module functions so the
  ground truth is unambiguous and runs fast. This favors clarity over realism;
  it does not measure large-codebase behavior.
- **The hidden tests are the benchmark's bias.** They encode *one* reasonable
  reading of each goal's full intent. They are intentionally strict about edge
  cases a terse prompt omits — that strictness is the point, but it means the
  absolute scores depend on how demanding the hidden suite is. Relative
  arm-to-arm comparison is the trustworthy signal, not the absolute numbers.

See `tasks.py` for the task suite and each task's hidden checks.
