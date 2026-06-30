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
    --model google/gemini-2.5-flash \         # cheap, reliable planner
    --executor claude \
    --out benchmarks/results
```

Useful flags:
- `--tasks median,chunk` — run a subset.
- `--arms A,B,C` — include the elaborated-prompt control.
- `--repeats 3` — repeat each task to measure variance (agents are stochastic).
- `--timeout 360` — per-arm wall-clock cap.
- `--keep-workspaces` — keep the temp repos for inspection.

Arm-B acceptance-check tuning (how DevCouncil proves each acceptance criterion):
- `--ac-samples N` — generate `N` *independent* per-criterion checks and decide by
  **majority vote** (default `1`). A criterion is proven only when a strict majority
  pass; an all-fail is a real defect (blocks); a split is inconclusive (non-blocking).
  `>1` outvotes a single mis-generated check — the cause of false `blocked` verdicts.
  Local sampling is cost-free, so raise it (e.g. `3`) when using `--monitor-model`.
- `--ac-repair-attempts N` — when a compiled check *fails to run* (wrong import, broken
  one-liner) feed the error back and regenerate the command up to `N` times (default
  `1`). Rescues the under-credited `incomplete`; can never weaken the gate, since a
  check that never ran proves nothing.
- `--ac-per-criterion` — compile **one** acceptance check per model call instead of
  batching every criterion into a single prompt. A weak/local monitor batching N criteria
  into one JSON routinely omits or mis-attributes some (a false `incomplete`); a focused
  single-criterion prompt is far more reliable. Costs N× the calls — cheap on a local
  monitor. Compounds with `--ac-samples`.

For a weak/local reviewer the high-leverage combination is `--ac-samples 3
--ac-repair-attempts 2 --ac-per-criterion` alongside `--monitor-model <ollama-tag>`.

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
