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

## Calibration dashboard

Track false-positive / false-negative rates over time across saved result files:

```bash
python benchmarks/calibration_dashboard.py
python benchmarks/calibration_dashboard.py benchmarks/results/<run>.json
```

Reports decisive accuracy (passed/blocked), overall calibration including `incomplete`
verdicts, and infra-error exclusions per run.


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
python3 benchmarks/local_monitor_probe.py                 # config's reviewer model
python3 benchmarks/local_monitor_probe.py --model qwen3:8b --samples 3 --per-criterion
OLLAMA_THINK=false python3 benchmarks/local_monitor_probe.py   # latency/quality tradeoff
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

## Code-intelligence performance

Python code-intel ratchets (`tests/performance`, `scripts/codeintel-benchmark.py`)
are retired. The mapping engine's required gate is `rust/verify.sh`
(determinism, peak RSS, growth plateau, incremental-vs-cold soak).

```bash
cd rust && ./verify.sh
```

---

# `dev map` performance

The mapping engine is measured by `verify.sh` and by the kernel's own `--json`
timings, not by a Python harness. Where `run_bench.py` asks whether the gated
loop produces better code, this asks how long `devmap` takes and where the
time goes.

A second of avoidable overhead on the map is paid hundreds of times a day —
rebuilt by hooks, by the watcher, and by hand.

## Running it

```bash
cd rust
./verify.sh
cargo run --release -p devmap-cli -- --db /tmp/scratch.sqlite --progress never --json build ..
```

`--json` prints per-phase timings. `verify.sh` is the bound that must hold; a
scratch `--db` is required so a benchmark cannot mutate the working index.

The staged numbers below (`cold` / `warm` / `touch` / `manifest` / `e2e`) were
taken by a retired Python harness (`map_bench.py`) and are retained as the
2026-09-02 optimization record. They are not a live command.

| stage | what it times |
|---|---|
| `cold` | full index of every discovered file into an empty store |
| `warm` | incremental build that finds no changed file (the watcher's common tick) |
| `touch` | incremental build after one file's content changes (the agent's common case) |
| `manifest` | writing `repo_map.json` + `code_graph.json` from the store |
| `e2e` | the real `dev map` command, Python wrapper included |

Two design choices worth knowing:

- **Minimum, not mean.** Wall-clock latency has a hard floor and an unbounded
  tail — background load can only make a run slower — so the mean measures the
  machine's mood while the minimum measures the code. Median and max are
  recorded alongside so a pathological tail stays visible.
- **A scratch store, always.** Every stage writes to a temporary database, never
  the target repository's `.devcouncil/`. A benchmark that mutates the tree it
  measures is not repeatable, and this one runs against real working repos.

For a finer breakdown than the five stages, run the kernel directly — each phase
reports its own cost, and the persist phase is split into its four parts:

```bash
rust/target/release/devmap --progress always build .
```

## Results: 2026-09-02 optimization pass

Baseline `20260902T164552Z`, final `20260902T181600Z`, on DevCouncil itself
(1,308 tracked / 994 code files, 41.8 MB), 5 repeats, Apple Silicon.

| stage | before | after | change |
|---|---|---|---|
| `cold` | 3.37s | 2.13s | **−36.8%** (295 → 466 files/s) |
| `warm` | 230ms | 196ms | −14.7% |
| `touch` | 2.04s | 1.31s | **−35.7%** |
| `manifest` | 520ms | 367ms | −29.5% |
| `e2e` (`dev map`) | 2.44s | 1.04s | **−57.5%** |
| `code_graph.json` | 26.21 MB | 20.59 MB | −21.5% |

Compare like with like: a run taken while a background daemon was re-indexing
measured `cold` at 2.38s rather than 2.13s. `e2e` is the stable stage (±2%);
the kernel stages carry a few percent of spread on a busy machine. On a
4,321-file repository the same build runs at 359 files/s and the graph drops
from 104.5 MB to 84.3 MB.

What changed, each found by measurement rather than review:

1. **The Python wrapper was most of `dev map`.** `stamp_freshness` read each
   finished artifact back, parsed it, set three scalars and re-serialized the
   whole thing — 1.68s of a 2.72s command, nearly all of it re-encoding a 26 MB
   graph the kernel had just encoded. `devmap manifest` now takes the three
   digests as flags and writes them itself.
2. **`mcp` was imported on every `dev` command.** `cli.commands.lease` →
   `execution.lease_ops` → `integrations.mcp.util` imported `mcp.types` at
   module scope for a symbol only one function needs at runtime. 208ms of a
   490ms CLI import, paid by commands that never speak MCP.
3. **A full `VACUUM` ran on nearly every build.** Pruning a generation pushes
   the freelist over the 5% threshold every time, so the store paid an
   O(database) whole-file rewrite — 937ms, 28% of an incremental build — to
   reclaim a few percent. Now `PRAGMA incremental_vacuum`, which costs what the
   waste costs (2ms). The reclaim decision also had to be moved after a WAL
   checkpoint: it was reading the pre-prune freelist and declining to reclaim
   while a third of the file was free.
4. **The store ran on SQLite's defaults.** `synchronous=FULL` fsynced every
   commit of a *derived* index, and a 2 MiB page cache could not hold the
   working set of a bulk generation write.
5. **The writer recompiled its SQL per row.** ~73,000 edge inserts and ~146,000
   path-id lookups, each compiling its statement afresh. Now `prepare_cached`
   plus a per-transaction path-id memo: `persist:write` 1180ms → 613ms.
6. **Resolution was the last serial CPU phase.** Extraction was parallel;
   resolution walked files one at a time, and it does *not* shrink on an
   incremental build (it deliberately covers the whole tree so liveness and
   community detection mean the same thing on both paths). Now parallel, with
   `parallel_determinism.rs` proving 1-thread and 8-thread results are identical
   field-by-field — verified end-to-end too: the artifacts are byte-identical
   under `RAYON_NUM_THREADS` of 1, 2 and 8.
7. **The release profile did not exist.** Default `codegen-units = 16` and no
   LTO, for a binary built rarely and run constantly.
8. **`code_graph.json` was pretty-printed.** 23% of a 27 MB machine-only
   artifact was indentation no reader sees; `repo_map.json`, which agents do
   open, stays indented.

### The trade in (3), stated plainly

Incremental vacuum does not compact the file the way a full `VACUUM` did. Over
15 consecutive builds the DevCouncil store settles at 295 MB against ~197 MB of
live data and stays there — free pages are reused by the next generation rather
than returned and immediately re-allocated. Checked on a 4,321-file repository
too: 1,155 MB, unmoved across 8 builds. The cost is a bounded ~50% space
overhead; the bound is what makes it acceptable, and it is asserted by
`new_stores_use_incremental_auto_vacuum_and_reclaim_without_a_full_rewrite`.

## Measuring a small change on a busy machine

Comparing a later run against a JSON written at some earlier time,
and that comparison is only as good as the machine's state in between. On a
loaded developer machine it is not good at all: three consecutive `cold` runs of
one unchanged binary measured 4.35 s, 2.96 s and 5.20 s here, and the same
unchanged code read "+143.9% vs baseline" purely because a background indexer
for an unrelated repository was saturating the CPU. Waiting for the load to
settle is not a reliable option either — an interactive machine has other work.

For a change expected to cost single-digit percent, measure a **ratio** rather
than an absolute, by interleaving:

1. Build two release binaries that differ only in the change under test. Keep
   both aside (`cp target/release/devmap …`), because the next `cargo build`
   overwrites one of them.
2. **Verify they actually behave differently** before timing anything. A build
   that silently reused a stale artifact, or a source edit that did not take,
   produces two identical binaries and a beautiful null result. Assert on an
   observable: here, the with-signatures binary wrote 11,514 signed symbols and
   the without wrote 0.
3. Alternate A, B, A, B … for several rounds against the same repository, each
   round rebuilding a scratch store from cold. Both arms then absorb the same
   load excursions.
4. Report minimums *and* medians, and say so when the difference is smaller than
   the spread.

The last point is the one that matters. In the clone-signature measurement the
arm doing *more* work came out 6.2% faster on minimums — not a speedup, but the
correct conclusion that the cost is below this machine's noise floor. Reporting
"-6.2%" as a win, or picking the run that showed "+143.9%" as a regression,
would both have been fabrications drawn from the same data.
