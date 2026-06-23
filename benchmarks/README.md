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
  trust metric: *when DevCouncil says done, is it done?*
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

Output: a `results/<timestamp>.json` (raw per-run data) and a printed Markdown
summary table.

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
