# The Claude Code Hero Loop

This is an opt-in strict workflow. It describes `gates.mode=enforce` and does
not make tasks, leases, or verification mandatory for ordinary work when the
project uses `advisory` or `off`.

DevCouncil's flagship integration is an **autonomous closed loop** with Claude Code over
MCP: the agent checks out a task, implements it, asks DevCouncil to verify, receives a
typed list of next actions, repairs, and re-verifies — **without a human pasting prompts
or test output back and forth.** Evidence, not model confidence, decides when the work is
done. These are **modules** (lease, verify, MCP). **Manvi** wraps the same components for
multi-agent campaigns. A host can take this loop without Manvi, or take Manvi without
this loop.

Cursor and Claude can run this loop over the eight-tool host MCP. It is **Preview**, not a certified slash/hook/subagent install. Other names in `integrate.Hosts` are stub receipts (see
[coding-cli-integration.md](coding-cli-integration.md)).

## Certified path (Preview)

| Agent | OS | Transport | Status |
| --- | --- | --- | --- |
| **Claude Code** | macOS, Linux | MCP via `.mcp.json` (`devcouncil_*` eight-tool host) | **Preview** — integrate writes MCP only |
| **Cursor** | macOS, Linux | MCP via `.cursor/mcp.json` + `.cursor/rules/devcouncil.mdc` | **Preview** — no hooks.json |
| **Codex** | macOS, Linux | comment-only `.codex/config.toml` | **Stub** |

`devcouncil integrate claude` does **not** install slash commands, a plugin, PreToolUse hooks, a statusline, or a `devcouncil-implementer` subagent. Those were the Python installer. Coverage: Go MCP tests (`backend/go_orchestrator/devcouncil/mcp`) and `dcverify` golden fixtures. Those `dcverify` fixtures cover the Rust binary; host MCP `devcouncil_verify_task` currently runs Go `verify.Run()` and does not spawn `dcverify` (TASK-P7-1). Manvi `runRigor` is the path that execs it.

### Deterministic self-repair

Stable repair contract (no LLM required): correction manifest from blocking gaps + typed `next_actions`; bounded re-runs; stop on unchanged blocking-gap fingerprint.

### Lease contract (long runs)

| Failure | Code | Recovery |
| --- | --- | --- |
| TTL not yet expired | — | `devcouncil_renew_lease` before `expires_at` |
| TTL expired | `lease_expired` | `devcouncil_checkout_task` again |
| Wrong token / no lease | `invalid_lease` | Checkout with correct `client_id` |
| Another agent holds task | `lease_held_by_other` | `devcouncil_next_task` or wait |

### Best-effort adapters (Preview)

Cursor Agent, Codex, and other CLIs can still *call* the same eight host MCP tools if you point them at `devcouncil mcp` by hand. `integrate` does not configure Antigravity / OpenCode / Warp / Aider / Gemini (stub receipt). Prefer Cursor or Claude MCP for the loop; confirm wiring with `devcouncil integrate cursor|claude --check`.

Large multi-agent goals with dependency DAGs are orchestrated by **Manvi**, which wraps these same DevCouncil components rather than replacing them.

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

The agent never needs a human in the inner loop. A human reviews the final evidence (`devcouncil verify TASK_ID --json` / MCP `devcouncil_get_gaps`) — not the chat history. There is no `dev report` command (unknown, exit 2).

## The diff↔coverage gate

This gate lives in **`dcverify`**. Manvi `runRigor` execs it. Go `verify.Run()` / MCP `devcouncil_verify_task` do not (TASK-P7-1); they always record `"coverage profile not supplied"`.

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
stub around a gap). There is no `dev report rigor` command.

## Setup

```bash
# Writes .mcp.json (devcouncil + devmap). No slash commands, hooks, or statusline.
devcouncil integrate claude --apply

# Confirm the wiring.
devcouncil integrate claude --check
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

## CLI Verification Gateway

You can run the same deterministic evidence gate against current changes via the CLI — no LLM or external provider keys needed:

```bash
# Verify the task diff against planned scope and rigor gates
devcouncil verify TASK-001

# Emit structured JSON for scripting and CI pipelines
devcouncil verify TASK-001 --json

# --sandbox is recorded on the report; docker/nix do not isolate today
devcouncil verify TASK-001 --sandbox local
```

`devcouncil verify` runs Go `verify.Run()` (planned-file / orphan / expected tests) and prints the verdict along with typed `next_actions`. It does not spawn `dcverify`. Stub / secret / coverage rigor is in that binary; Manvi calls it. See [TODO.md](TODO.md) TASK-P7-1.
