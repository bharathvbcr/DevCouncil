# The Claude Code Hero Loop

DevCouncil's flagship integration is an **autonomous closed loop** with Claude Code over
MCP: the agent checks out a task, implements it, asks DevCouncil to verify, receives a
typed list of next actions, repairs, and re-verifies — **without a human pasting prompts
or test output back and forth.** Evidence, not model confidence, decides when the work is
done.

This is the one path DevCouncil certifies end to end. Other coding CLIs are supported (see
[coding-cli-integration.md](coding-cli-integration.md)), but the loop below is the one to
reach for first.

## Certified path (Stable)

| Agent | OS | Transport | Status |
| --- | --- | --- | --- |
| **Claude Code** | macOS, Linux | MCP (`devcouncil_*` tools) + optional hooks | **Certified / Stable** |
| **Claude Code** | macOS, Linux | Slash commands (`/devcouncil:*`) shelling to MCP | **Certified / Stable** |
| **Claude Code** | macOS, Linux | Subagent `devcouncil-implementer` | **Certified / Stable** |

Golden coverage: `tests/unit/test_mcp_closed_loop.py` and `tests/unit/test_hero_loop_golden.py`.

### Deterministic self-repair (`dev go`)

Stable repair contract (no LLM required): correction manifest from blocking gaps + next-actions; bounded re-runs (`execution.max_repair_attempts`); stop on unchanged blocking-gap fingerprint; optional LLM `RepairService` when a provider key is configured (Preview). See `tests/unit/test_go_repair_loop.py`.

### Lease contract (long runs)

| Failure | Code | Recovery |
| --- | --- | --- |
| TTL not yet expired | — | `devcouncil_renew_lease` before `expires_at` |
| TTL expired | `lease_expired` | `devcouncil_checkout_task` again |
| Wrong token / no lease | `invalid_lease` | Checkout with correct `client_id` |
| Another agent holds task | `lease_held_by_other` | `devcouncil_next_task` or wait |

### Best-effort adapters (Preview)

Codex, Antigravity, Cursor Agent, Grok, OpenCode, Warp/Aider/Copilot/others reuse the same verifier and next-actions contract but are not certified for the full MCP closed loop. Gemini CLI is **deprecated** (use Antigravity). Prefer the Claude Code MCP path for production agent loops; confirm wiring with `dev integrate check`.

Large multi-agent goals with dependency DAGs should use **`dev campaign`** (Director → Coordinator → Worker pool + Reviewer QC), not the retired feudal-theme naming.

## The loop

```
checkout_task ─▶ (agent implements) ─▶ verify_task ─▶ passed? ─▶ release_task
      ▲                                     │
      │                                     ▼ blocking gaps
      └──────────── self-repair ◀──── next_actions (typed)
```

1. **`devcouncil_checkout_task`** — the agent acquires a task lease and gets back the
   scoped prompt, planned files, allowed commands, expected tests, and (when present)
   semantic context. One agent owns the task at a time.
2. **The agent implements** the change inside the declared file scope.
3. **`devcouncil_verify_task`** — DevCouncil runs the *deterministic* verifier: planned-file
   compliance, orphan-diff detection, dependency/secret scanning, acceptance evidence, and
   the **diff↔coverage gate** (below). It returns `passed`, `blocking_gaps`, and
   `next_actions`.
4. **`next_actions`** — a typed, machine-routable contract the agent acts on directly.
   No prose parsing:

   ```json
   {
     "gap_id": "GAP-TASK-001-DIFFCOV-ab12-001",
     "gap_type": "diff_not_exercised",
     "category": "add_test",
     "severity": "high",
     "blocking": true,
     "action": "Add or extend a test that executes the changed lines (src/calc.py:42), then re-verify.",
     "file": "src/calc.py",
     "line": 42,
     "missing_evidence": "Verification commands passed but exercised 1/6 changed line(s).",
     "suggested_command": "python -m pytest tests/test_calc.py -q"
   }
   ```

   Categories the agent can branch on: `fix_code`, `add_test`, `fix_verification`, `scope`,
   `security`, `review`, `plan`.
5. **Self-repair and re-verify** — the agent resolves each action and calls
   `devcouncil_verify_task` again. The loop continues until `passed` is true.
6. **`devcouncil_release_task`** — the lease is released.

The agent never needs a human in the inner loop. A human reviews the final evidence report
(`dev report`) — the durable Requirement→Task→Diff→Evidence artifact — not the chat
history.

## The diff↔coverage gate

A green test suite is only acceptance evidence if it actually ran the lines the diff
changed. The verifier runs the task's test command under coverage and intersects the
executed lines with the diff hunks. A passing suite that never imports the changed module,
never calls the new function, or only exercises an unrelated branch is reported as
`diff_not_exercised` — the new logic was not proven.

This is deliberately **false-positive-safe** (see [security.md](security.md) for the wider
discipline):

- It only produces a signal when it has reliable data: a parseable diff, the target repo's
  own coverage tooling, and changed *executable* lines to measure. Otherwise it degrades
  silently and the verifier behaves as before — it never blocks correct work for lack of
  measurement.
- It is **signal-first**: by default the gap is non-blocking and informational. Teams opt
  into blocking with `verification.diff_coverage.enforce: true` (and an optional
  `min_ratio`).

```yaml
# .devcouncil/config.yaml
verification:
  diff_coverage:
    measure: true     # record diff coverage as evidence whenever tooling is present
    enforce: false    # promote an unexercised diff to a *blocking* gap
    min_ratio: 0.0    # 0.0 = "at least one changed line exercised"; higher demands more
```

It currently measures **Python** (via the target repo's `coverage.py`), including inline
`python -c "..."` acceptance checks. It assumes tests run against the **source tree**
(the normal setup for a repo under active development — editable install or `src` on the
path); a suite that exercises an installed *copy* of the package instead may under-report.
This is one more reason enforcement is opt-in.

On **hard** tasks, `verification.rigor.enforce_coverage_on_hard` (default `true`) promotes
this gate to blocking even when `diff_coverage.enforce` is `false` — see [Anti-laziness
rigor](#anti-laziness-rigor) below.

## Anti-laziness rigor

Coding agents routinely stub, undersize diffs, or claim "done" before tests actually prove
the work. DevCouncil's **rigor layer** catches those patterns deterministically (no extra
LLM calls for stub/effort detection) and scales strictness by **task difficulty**:

| Difficulty | Default behavior |
|---|---|
| `easy` / `normal` | Stub/effort/coarse-proof findings are **advisory** — surfaced in gaps and `next_actions` but non-blocking |
| `hard` | Same gates **block** verification; diff coverage is enforced; repair budget widens |

Tasks are classified as `easy` / `normal` / `hard` by a deterministic scorer
(`devcouncil.verification.difficulty`) from planned scope, acceptance-criteria count, and
keywords. Planners and humans can override with `Task.difficulty`.

**Verifier gates (on added diff lines only):**

- **Stub/TODO detection** (`stub_detected`): placeholders, `NotImplementedError`, skipped
  tests, assert-free tests, TODO/FIXME markers. Intentional scaffolding requires the task
  description to mention "scaffolding" and the line to carry `devcouncil: allow-stub`.
- **Effort heuristics** (`suspicious_effort`): undersized diff vs planned scope,
  comment-only diffs, net test deletion in files referenced by `expected_tests`.
- **Coarse acceptance proof** (`coarse_acceptance_proof`): a criterion "proven" only because
  a generic passing command ran, not a per-criterion check — blocking on hard tasks.

**Hard-task escalation** also injects a compact **Rigor** section into the executor prompt,
adds `extra_repair_attempts_on_hard` to the `dev go` repair budget, and (opt-in) lets a
**critical** implementation-reviewer finding block when
`reviewer_required_on_hard: true`.

```yaml
# .devcouncil/config.yaml
verification:
  rigor:
    enabled: true
    stub_detection: hard           # never | hard | always
    effort_heuristics: hard
    coarse_acceptance_proof: hard  # block coarse AC proof on hard tasks
    enforce_coverage_on_hard: true
    reviewer_required_on_hard: false  # opt-in: critical review findings block
    extra_repair_attempts_on_hard: 1
    min_added_lines_per_planned_file: 5
    acceptance_samples_on_hard: 2   # self-consistency voting on hard tasks
```

Repair runs carry a **correction manifest** with prior diff, failing output, attempt
history, stub findings, and non-negotiable **repair rules** (never weaken tests, never
stub around a gap). Tune thresholds from evidence with `dev report rigor`.

## Setup

```bash
# One-shot: MCP server, assistive hooks, slash commands, subagents, output style, skills, statusline.
dev integrate claude --apply

# Optional: add the blocking write-gate for autonomous runs.
dev integrate claude --apply --write-gate

# Confirm the wiring.
dev integrate check
```

Then, inside Claude Code, drive the loop with the `devcouncil_*` MCP tools, or let an
automated executor run it:

```bash
dev e2e "Describe the implementation goal" --executor claude
```

## Anthropic advisor tool (Claude Code only)

Pair a faster main model with a stronger advisor that Claude consults mid-task (planning,
stuck loops, completion checks). This is **not** live review, the planning council, or
`opusplan` — it is Anthropic's server-side advisor tool on Claude Code / the Anthropic API.

**Requirements:** Claude Code ≥ 2.1.98 (Fable main/advisor needs ≥ 2.1.170), Anthropic API
(not Bedrock/Vertex/Foundry), compatible main/advisor pairing. Recommended: `sonnet` main +
`opus` advisor. DevCouncil soft-filters clear mismatches only; Claude Code validates the
full versioned pairing matrix at launch.

**When not to use:** skip advisor for mechanical one-line fixes, pure lookup/grep turns, or
when you are on Bedrock/Vertex/Foundry (Claude Code ignores `--advisor` there — DevCouncil
soft-skips attach). Prefer live review / verification for evidence gates, not the advisor.

Enable via profile config:

```yaml
# .devcouncil/config.yaml
integrations:
  cli_agents:
    profiles:
      default:
        model: sonnet
        advisor_model: opus
```

| Path | How advisor enables |
|---|---|
| `dev run/go/e2e --executor claude` | `--advisor` on every spawn (including `--resume` repairs) |
| `dev run --executor claude-sdk` | SDK `extra_args={"advisor": ...}` |
| Interactive MCP hero loop | `advisorModel` written by `dev integrate claude` when the default profile sets a pairing-safe `advisor_model` |

Repair/`--resume` runs treat the correction manifest as authoritative over prior session
or prior advisor advice. Soft pairing preflight skips clearly bad pairs so Claude does
not hard-exit and burn the repair budget. Set `CLAUDE_CODE_DISABLE_ADVISOR_TOOL=1` to
disable the tool entirely (Claude still accepts `--advisor` / `advisorModel` but ignores them).

See [coding-cli-integration.md](coding-cli-integration.md) for more detail, including the
unified **stop gate** (claim checks + optional active-task verify on Claude/Codex Stop hooks).

## The lite on-ramp

Before committing to the full planning council, you can taste the evidence gate on whatever
is already in your working tree — no LLM, no provider keys:

```bash
# Verify the current diff against an inline requirement, with diff coverage.
dev check --verify --goal "reset tokens are single-use" --test "python -m pytest tests/test_auth.py -q"

# Make the diff-coverage gate blocking for this check.
dev check --verify --test "python -m pytest -q" --enforce-coverage
```

`dev check --verify` runs the same deterministic verifier as the hero loop and prints the
verdict plus the next-actions contract (`--json` for machine consumption). Once you trust
the gate here, `dev plan` graduates you to the full Requirement→Task→Diff→Evidence graph.
