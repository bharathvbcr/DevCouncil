# DevCouncil Improvement Backlog

Prioritized findings from a full-codebase review (July 2026). File refs verified against source.

## Status after implementation session (2026-07-05)

**Shipped:** #1 (git timeouts in verifier + shared `utils/proc.py`), #2 (`utils/fsio.py` atomic writes applied to 11 state-writing modules), #4 (20 silent excepts now logged, 7 upgraded to warnings; verifier's own were already logging), #5 partially (new storage-roundtrip, gating, and utils test suites; note tests/unit already had flat per-module files — "zero tests" was overstated for storage), #7 (coverage config in pyproject + CI wired to `coverage run -m pytest`), #9 (checkpoint ref moved to `domain/checkpoint_refs.py`, cycle workaround removed), #12 partially (council was already parallel via `asyncio.gather` — finding was stale; the remaining serial loop in `live/reviewer.py` sampling is now gathered), #13 (sha256-keyed parse cache at `.devcouncil/cache/repo_map_parse.json`), #14 partially (mcp SDK import made lazy; broader lazification blocked by test monkeypatch targets), #16 as `dev check --watch` (poll-based re-run loop), #17 (`verification.retry_flaky`, on by default, single re-run with `[flaky: passed on retry 2/2]` tag), #18 (`dev report --evidence-json PATH`), #19 (`telemetry.cost_budget_usd` + `dev cost budget`, warn-only), #20 (`dev doctor` status-doc drift check). Also fixed: `hook_policy.py` read a nonexistent `execution.global_allowed_commands` config key — the hook gate's allowlist was always empty; it now derives from `config.commands.{test,lint,typecheck}` like the run path.

## Status after optional follow-ups (2026-07-06, post-loop)

**Shipped:**
- **#8 (god-module trim, final)** — command runner → `verification/command_runner.py` (~132 lines); git diff/changed-files/committed-task-diff → `git_diff_fallback.py`. `verifier.py` 615→387 lines. `_verification_env` now delegates to shared `utils/subprocess_env.clean_subprocess_env` (dedupes 60-line copy).
- **`dev map` refresh** — regenerated after extractions.
- **Mypy (touched modules):** clean on `command_runner`, `git_diff_fallback`, `verifier`, `coverage_measurement`. Full-repo mypy: 75 pre-existing errors in 36 files (unchanged scope; not introduced by this session).
- **Full suite green:** **1385 passed / 0 failed**.

**Backlog: FULLY CLOSED.** All P0–P3 items (#1–#20) shipped or corrected/deferred with documented rationale. No substantive optional work remains; further `verifier.py` trim would fragment thin delegation wrappers without meaningful cohesion gain.

## Status after loop session (2026-07-06, tick 10 — final polish)

**Shipped this tick:**
- **Regression fixes (tick-9 JSON migration fallout)** — restored missing `dump_json` imports in `cli/commands/map.py`, `plan.py`, `trace.py` (26 failures → 0).
- **Test/telemetry alignment** — `read_cost_records` / `group_cost` now honor `DEVCOUNCIL_LOG_DIR` (matches `_log_model_call` write path); per-test ledger isolation in companion/ollama cost tests; logs tests unset override when asserting project-relative paths; verify JSON tests parse `stdout` not merged `output`; `cost show` test invokes `show` subcommand.
- **Lint** — ruff clean on touched modules; MCP server re-exports `_CLI_OUTPUT_LIMIT`, `_CLI_TIMEOUT_SECONDS`, `_allowed_next_tools` via module-level aliases; mypy clean on verifier + server + cost.
- **Full suite green:** **1385 passed / 0 failed** (was 1359 passed / 26 failed at tick-10 start).

**Backlog (#6, #8, #10, #11, #15): complete.** Residual optional work addressed in post-loop session (verifier trim, mypy spot-check, `dev map` refresh).

**Loop status: STOP.** Backlog fully closed.

## Status after loop session (2026-07-06, tick 9)

**Shipped this tick:**
- **#8 (god-module trim, continued)** — command-malformation analysis → `verification/command_malformation.py` (~150 lines). `verifier.py` 793→615 lines.
- **#10 (MCP service layer, completed)** — remaining DB-direct MCP tools now route through CLI: `verify-leased`, `scope update`, `evidence-append`/`evidence-list`, `policy-check`, `record-command`, `run-cmd`, `next-task`, `handoff-leased`. Shared service: `execution/task_gate_ops.py`; CLI surface: `cli/commands/task_gate.py`.
- **#11 (JSON persistence, migration)** — MCP `trace.py` tail_trace uses `json_text`; `task.py` get_task uses `dump_json`. Residual `json.dumps` confined to `json_persist`, MCP util, LLM/dashboard/integration emitters (intentional or low-priority).
- **Tests:** rollback e2e fixed (checkpoint before checkout ordering); `test_mcp_verify_persists` updated for CLI subprocess routing; re-exported `_allowed_next_tools` from server. Targeted MCP/verifier: 98 passed.

**Backlog (#6, #8, #10, #11, #15): substantively complete.** Residual: `verifier.py` still 615 lines (command runner/git helpers remain), full-suite green (~13 pre-existing unrelated failures), full-repo `mypy`, optional `dev map` refresh.

## Status after loop session (2026-07-06, tick 8)

**Shipped this tick:**
- **#8 (god-module trim, continued)** — `server.py` 1142→299 lines (thin dispatch). Extracted: `handlers/tool_specs.py` (~620 lines), `handlers/prompts.py`, `handlers/cli_gate.py`, `handlers/router_cache.py`.
- **#10 (MCP service layer, continued)** — checkout/lease/write MCP tools now route through CLI (`dev checkout`, `dev release`, `dev lease list/renew`, `dev write`, `dev apply-patch --json`). Shared services: `execution/lease_ops.py`, `execution/gated_write.py`. Added `parse_cli_json()` for non-zero exit codes with JSON stdout.
- **#11 (JSON persistence, migration)** — migrated 18 CLI modules to `dump_json()`: `tasks`, `watch`, `show`, `verify`, `check`, `lsp`, `export`, `requirements`, `prompt`, `handoff`, `evidence`, `ast`, `map`, `report`, `semantic`, `campaign`, `watch_fs`, `cost`.
- **Tests:** full suite 1372 passed / 13 failed (12 pre-existing + 1 rollback e2e flake; improved from 14 failures). Ruff clean on touched files.

**Still open:** #8 `verifier.py` still ~793 lines, #10 route remaining DB-direct MCP tools (verify/scope/evidence/policy/next_task/handoff/run_command), #11 migrate remaining CLI JSON sites (~15 modules: hook/plan/trace/watch partial), full-suite green (13 unrelated failures), full-repo `mypy`.

## Status after loop session (2026-07-06, tick 7)

**Shipped this tick:**
- **#8 (god-module trim, continued)** — diff-coverage instrumentation → `verification/coverage_measurement.py` (~290 lines). `verifier.py` 1016→794 lines. Knowledge discovery → `knowledge/resource_discovery.py` (breaks provenance↔MCP import cycle).
- **#10 (MCP service layer, continued)** — `devcouncil_get_task_provenance`, `list_resources`, and `read_resource` now route through CLI (`dev provenance --json`, `dev resource list/read`). Shared services: `reporting/task_provenance.py`, `reporting/mcp_resources.py`.
- **#11 (JSON persistence, migration)** — added `dump_json()` to `utils/json_persist.py`; migrated `storage/repositories.py` (22 sites), `storage/native.py` (3), MCP-routed CLI (`gaps`, `status`, provenance/resource).
- **`dev map` refresh** — regenerated after extractions.
- **Tests:** 81 targeted passed; full suite 1371 passed / 14 failed (12 pre-existing unrelated: cli_logs, companion_trace_cost, verify_json, etc.; 2 fixed: mcp_resources unknown-uri, storage history_json spacing). Ruff + mypy clean on touched files.

**Still open:** #8 `server.py` still ~1142 lines (orchestration handlers remain inline), #10 route remaining DB-direct MCP tools (checkout/lease/write paths), #11 migrate remaining CLI JSON sites (~35 modules, mostly watch/hook/plan), full-suite green (14 unrelated failures), full-repo `mypy`.

## Status after loop session (2026-07-06, tick 6)

**Shipped this tick:**
- **#8 (god-module trim, continued)** — git/diff fallback helpers → `verification/git_diff_fallback.py` (~182 lines); MCP inline handlers → `handlers/runs.py`, `handlers/wiki.py`, `handlers/knowledge.py`, `handlers/graph.py`. Shared service helpers: `knowledge/wiki_read.py`, `knowledge/knowledge_select.py`. `verifier.py` 1166→1016 lines; `server.py` 1296→1142 lines.
- **#10 (MCP service layer, continued)** — `devcouncil_list_agent_runs`, `get_run`, `wiki_page`, `select_knowledge`, `graph_context` now route through CLI (`dev runs list/show --json`, `dev wiki read --json`, `dev okf select --json`, `dev graph-context --json`). Extended `runs list --json` with `total`; added `wiki read`, `okf select`, `graph-context` commands.
- **#11 (JSON persistence, migration)** — 18 modules migrated to `utils/json_persist.py`: `run_trace`, `prompt_builder`, `rigor_analytics`, `transcripts`, `signals`, `correction_manifest`, `repo_mapper`, `semantic_index`, `check`, `dashboard`, `cards`, `coding_cli` (4 reads), `wiki`, `checkout`, `prompt_enhancer`, `gepa_agent`, `semantic_diff`, `runs`.
- **`dev map` refresh** — regenerated after handler extractions.
- **Tests:** 95 passed (verifier 40, MCP server/closed-loop/wiki-runs/knowledge/companion-runs 53, json_persist 2). Ruff clean on touched files.

**Still open:** #8 further trim (`verifier.py` still ~1016 with coverage measurement helpers; `get_task_provenance` still DB-direct), #10 route provenance/read_resource paths, #11 migrate remaining hand-rolled JSON (mostly CLI output modules + `storage/`), full-suite `mypy`/`pytest`.

## Status after loop session (2026-07-06, tick 5)

**Shipped this tick:**
- **#8 (god-module trim, continued)** — MCP inline handlers → `handlers/task.py` (get_task, get_prompt, prepare_execution), `handlers/policy.py` (policy_check_write, record_command), `handlers/trace.py` (tail_trace, run_timeline, run_supervise). `server.py` 1432→1296 lines; `verifier.py` unchanged at 1166 (git/diff fallback helpers deferred).
- **#10 (MCP service layer, continued)** — `devcouncil_get_gaps`, `get_next_actions`, `list_tasks`, `get_task`, `get_prompt`, `prepare_execution` now route through CLI (`dev gaps --task-id`, `--next-actions`, `dev tasks --json --status/--limit/--offset`, `dev show`, `dev prompt`). Extended `gaps` and `tasks` CLIs with MCP-compatible flags.
- **#11 (JSON persistence, migration)** — 14 modules migrated to `utils/json_persist.py`: `llm/cache`, `telemetry/tracker`, `app/run_context`, `planning/prompt_enhancer_service`, `cli/commands/baseline`, `cli/commands/map`, `knowledge/wiki`, `executors/coding_cli` (5 sites), `integrations/clients/common`, `integrations/gitnexus`, `integrations/graphify`, `optimization/gepa_agent`, `optimization/skillopt`.
- **Tests:** 46 passed (MCP server/contract/closed-loop/wiki-runs/resumable-gaps, json_persist, gaps/tasks CLI). Ruff clean on touched files; mypy on new handlers matches existing db `object` typing pattern.

**Still open:** #8 further trim (`verifier.py` git/diff fallback helpers; server still ~1296 with list_agent_runs/wiki/select_knowledge inline), #10 route remaining inline MCP tools (list_agent_runs, wiki, graph_context, provenance partial), #11 migrate remaining ~55 hand-rolled JSON call sites, full-suite `mypy`/`pytest`, `dev map` refresh.

## Status after loop session (2026-07-06, tick 4)

**Shipped this tick:**
- **#8 (god-module trim, continued)** — `verify_task` orchestration body → `verification/verify_orchestration.py` (~346 lines); MCP inline handlers → `handlers/read.py`, `handlers/run.py`, `handlers/next_task.py`, `handlers/handoff.py`; `is_secret_path` moved to `mcp/util.py`. `verifier.py` 1528→1164 lines; `server.py` 1670→1432 lines.
- **#10 (MCP service layer, continued)** — `devcouncil_status` and `devcouncil_report` now route through `run_cli_command` (`dev status --json`, `dev report`) instead of direct DB/report-builder calls; MCP text format preserved.
- **#11 (JSON persistence, migration)** — `execution/handoff.py`, `live/cards.py`, `indexing/semantic_index.py` now use `utils/json_persist.py`.
- **`dev map` refresh** — regenerated after handler extractions.
- **Tests:** 75 passed (verifier 40, MCP server 35 incl. status/report/cli, closed-loop 5, json_persist 2). Ruff clean on touched files; mypy clean on new modules (pre-existing db type ignores remain in handlers).

**Still open:** #8 further trim (verifier still ~1164 lines with git/diff helpers; server still ~1432 with get_task/prompt/record_command inline), #10 route remaining DB-direct MCP tools (get_gaps/list_tasks/get_task still direct DB; gaps could use `dev gaps`), #11 migrate remaining ~70 hand-rolled JSON call sites, full-suite `mypy`/`pytest`.

## Status after loop session (2026-07-06, tick 3)

**Shipped this tick:**
- **#8 (god-module trim, continued)** — semantic diff → `verification/checks/semantic_diff.py`; verify setup/finalize → `verification/verify_setup.py`; MCP scope/evidence/git → `handlers/scope.py`, `handlers/evidence.py`, `handlers/git.py`. `verifier.py` 1808→1528 lines; `server.py` 1865→1670 lines.
- **#10 (MCP service layer, continued)** — `devcouncil_cli` allowed roots expanded: `gaps`, `doctor`, `cost`, `check`, `export`, `requirements`, `runs`, `logs`, `watch`, `go`.
- **#11 (JSON persistence, migration)** — `checkpoints.py`, `live/signals.py`, `correction_manifest.py`, `cli/commands/export.py` now use `utils/json_persist.py`.
- **`dev map` refresh** — regenerated after handler extractions.
- **Tests:** 89 passed (verifier 40, companion MCP 17, MCP checkout 8, closed-loop 5, json_persist 2, export 2, correction_manifest 15). Ruff clean on touched files.

**Superseded by tick 4** — see tick 4 section above.

## Status after loop session (2026-07-06, tick 2)

**Shipped this tick:**
- **#8 (god-module trim, partial)** — extracted compiled-acceptance orchestration to `verification/checks/compiled_acceptance.py`, AC evidence mapping to `verification/checks/acceptance_evidence.py`, live-review MCP tools to `integrations/mcp/handlers/live.py`. `verifier.py` 2093→1808 lines; `server.py` 1941→1865 lines.
- **Hero-loop rollback e2e** — `test_hero_loop_rollback_after_passing_verify` in `test_mcp_closed_loop.py` (checkout → checkpoint → write → verify pass → rollback → verify blocked → release).
- **#10 (MCP service layer, partial)** — `rollback` added to `devcouncil_cli` allowed roots so agents can invoke `dev rollback` through the gated CLI path.
- **#11 (JSON persistence, seed)** — new `utils/json_persist.py` (`write_json`, `write_model_json`, `read_json`, `read_model_json`) on top of `fsio.atomic_write_*`; 2 unit tests.
- **Tests:** 45 passed (`test_mcp_closed_loop.py` 5, `test_verifier.py` 40, `test_json_persist.py` 2). Ruff clean on touched files.

**Still open:** #8 further trim (semantic diff, verify_task setup/finalize still in monolith; server inline handlers for scope/evidence/git), #10 route more MCP tools through CLI service layer, #11 migrate call sites to `json_persist`, `dev map` refresh, full-suite `ruff`/`mypy`.

## Status after loop session (2026-07-06)

**Shipped this tick:**
- **#6 (hero-loop e2e)** — `tests/unit/test_mcp_closed_loop.py` exercises checkout → verify (blocked) → write → verify (pass) → release over MCP; added `test_hero_loop_repair_after_failing_evidence` for the repair leg (wrong code → blocked → fix → pass). Setup now writes minimal `config.yaml` and uses `sys.executable` for runnable evidence commands.
- **#15 (git batching)** — new `utils/git_snapshot.py` (`GitWorktreeSnapshot.capture`: `rev-parse` + `status --porcelain -z` + `diff HEAD` batched once per `verify_task`); wired into `verifier.py` with ignore-filter fix so `.devcouncil/*` state does not false-pass the empty-diff guard.
- **Tests:** `tests/unit/utils/test_git_snapshot.py` (2 tests). Verified: `test_mcp_closed_loop.py` (4), `test_verifier.py` (40), `test_git_snapshot.py` (2) — 44 passed.

**Still open:** #8 god-module trim (orchestration still in monoliths), #10 MCP service layer, #11 JSON serializer module, hero-loop rollback e2e, `dev map` refresh, full-suite `ruff`/`mypy`.

**Corrected findings:** #3 was wrong — `llm/router.py` already implements bounded exponential backoff with a dedicated 429 budget honoring Retry-After. Dropped.

**Previously deferred, now shipped:** #6, #15 (see loop session below).

**Deferred:** #8 partially (per-gate checks + MCP handlers extracted; `verifier.py`/`server.py` still large orchestrators), #10 (MCP interface layer — route through CLI service layer), #11 (full JSON-persistence centralization; `fsio.py` is the seed). Rollback leg of hero-loop e2e still untested.

**Required follow-up:** run full `uv run pytest tests/unit`, `ruff check`, `mypy`, and `dev map` before commit.

## P0 — Reliability

1. **Add timeouts to git subprocess calls in `verification/verifier.py`.** 14+ `subprocess.run/check_output` calls (lines 129–782) run unguarded; a hung git process hangs verification indefinitely. `executors/coding_cli.py` already does this correctly — extract a shared `run_git()` helper with timeout + consistent error handling and reuse it across the 29 files that invoke subprocess independently.
2. **Atomic file writes.** 24 `write_text()` calls across 15 files (checkpoints, handoff, semantic_index, okf, verifier) rewrite state files non-atomically — a crash mid-write corrupts state. Use tempfile + `os.replace()` via one shared util.
3. **Retry on LLM rate limits.** `llm/provider.py` parses `Retry-After` but never retries. Add bounded exponential backoff — this is the difference between a flaky and a dependable `dev go` loop.
4. **Stop swallowing exceptions.** 27 silent `except Exception: pass` blocks (7 in verifier.py alone, 5 in cli/commands/hook.py). At minimum log with context; several likely mask real gate failures — which undermines the product's core promise.

## P1 — Testing

5. **Cover the untested subsystems.** `storage/`, `gating/`, `reporting/`, `optimization/` have zero tests; `execution/` has 2 tests for 14 modules. Gating and storage are the trust core of the product — start there.
6. **Add an e2e test for the hero loop.** No integration test exercises plan → run → verify → repair → rollback end to end. One pytest that drives the manual executor against a fixture repo would catch whole classes of regressions.
7. **Turn on coverage measurement.** coverage.py is in dev deps but never invoked; CI runs ruff/mypy but no coverage gate. Ironic given the product ships a diff↔coverage gate — dogfood it.

## P2 — Architecture

8. **Split the god-modules.** `verifier.py` (2059 lines, `verify_task` ≈ 729 lines, 9 instance caches) and `integrations/mcp/server.py` (1790 lines, `call_tool` ≈ 754 lines) mix orchestration, policy, and I/O. Extract per-gate check classes from the verifier and per-tool handlers from the MCP server; this also unblocks finding 5.
9. **Break the verification↔execution import cycle** (hardcoded git ref workaround at verifier.py:168). Move shared bits into `domain/`.
10. **Give MCP server an interface layer.** It imports 20+ internal modules directly; route through the same service layer the CLI uses so both surfaces stay in sync.
11. **Centralize JSON persistence.** ~80 files hand-roll `.model_dump()`/`json.dumps()`; a single serializer module makes schema migrations feasible (and is where atomic writes from #2 live).

## P3 — Performance

12. **Parallelize council/planning LLM calls.** Debate roles are queried sequentially; running them concurrently saves 5–10s per plan.
13. **Cache parsed ASTs in `indexing/repo_mapper.py`.** Full re-parse per invocation costs 2–8s on medium repos; key cache by file hash.
14. **Lazy-import heavy deps in `cli/`** so `dev status` and other quick commands don't pay tree-sitter/SDK import cost.
15. **Batch git invocations** in indexing/verification (single `git status --porcelain -z` + `git diff` pass instead of per-file calls).

## Strategic product areas 4–8 (2026-07-08)

Codebase gap analysis against the next product bets. Complements (does not reopen) the closed P0–P3 backlog above.

### 4. CI/team evidence as first-class product

**Exists:** `EvidenceExportGenerator` (`reporting/evidence_export.py`) via `dev report --evidence-json`; `GitHubCheckGenerator` fails only when `blocking_gaps > 0` (`reporting/github_check.py`, `integrations/github.py`); PR/MR markdown via `integrations/pr_comments.py`; `fail_on_blocking` on report/status; doctor lists Checks/Comments as preview.

**Missing:** No PR-auto pipeline (scaffold CI in `repo/ci_scaffold.py` is lint/typecheck/test only); no HTML artifact; no install-free AC→evidence reviewer summary beyond raw JSON; Check/comment not in `.github/workflows/ci.yml`.

**Next:**
1. Extend `src/devcouncil/repo/ci_scaffold.py` (+ optional `.github/workflows/devcouncil-evidence.yml`) to run verify → `--evidence-json` → `actions/upload-artifact` → optional `--github` / `--github-pr-comment`.
2. Add HTML renderer next to `evidence_export.py` (AC table + evidence links) for artifact preview.
3. Keep Check conclusion blocking-only (already true); document advisory gaps as annotations/comment sections only.
4. Smoke-test with env: `GITHUB_TOKEN`, `GITHUB_REPOSITORY`, `GITHUB_SHA`, `GITHUB_PR_NUMBER`.

### 5. Incremental / smart verification

**Exists:** `dev check --watch` full re-run on mtime (`cli/commands/check.py`); changed-file gates (coverage, orphan, planned files); git snapshot batching; repo-map AST parse cache.

**Missing:** Path→gate selection; content-hash cache of green results; sub-second sidecar (watch still re-runs the full gate).

**Next:**
1. Add `verification/gate_selector.py` mapping changed paths → gates/commands (lint packages, dirty-module coverage).
2. Persist `.devcouncil/cache/gate_results.json` keyed by file content hashes; skip green unchanged gates.
3. Teach `--watch` to use selector + cache; keep full verify for release/`dev go`.
4. Benchmark iterative edit → verdict; target sub-second when only cached gates apply.

**Implemented (2026-07-08):**
- **`verification/gate_selector.py`** — pure `select_gates(changed_files, {kind: [cmd]})` mapping. Drops a command whose stack (python vs js/ts, inferred from the resolved tool through `python -m`/`npm run`/`poetry run` wrappers) has no changed file; narrows broad-target linters/type-checkers (`ruff check .`, `mypy src`, `black --check src`, `eslint .`) to the touched files (subcommand-aware, e.g. `ruff check`), leaving `pytest`, explicit paths, and shell-operator commands untouched. Each `GateSpec` carries the `inputs` that key its cache.
- **`verification/gate_cache.py`** — `GateResultCache` at `.devcouncil/cache/gate_results.json`, keyed by SHA-256 over the command string + byte content of the gate's inputs (missing files hash to a stable sentinel so create/delete invalidates). Only *passing* gates authorize a skip; failures always re-run. Atomic writes; a corrupt/absent cache degrades to "nothing cached".
- **`verification/incremental_check.py`** — `run_incremental_gates(...)` ties selector + cache: selects, skips cached-green, runs the rest via an injectable `runner` seam, records, persists. Returns a compact `IncrementalResult` (ran/cached/skipped, per-gate timing).
- **`cli/commands/check.py`** — `dev check --watch` now runs the incremental gate (one shared cache across iterations) and prints `PASS/FAIL — N run, M cached (Xms)`; falls back to the full evidence gate when no stack-relevant command applies to the change. The full `verify_task` / `dev go` path is unchanged and never consults this cache.
- **Timing:** on a 2-gate change (`ruff check` + `mypy`), cold run ≈ 500 ms; a subsequent save that leaves those inputs byte-identical is served from cache in ≈ 0.4 ms (~1000× faster; no subprocess spawned).
- **Tests:** `tests/unit/test_gate_selector.py` (10), `test_gate_cache.py` (8), `test_incremental_check.py` (7) — 25 tests.

### 6. Native executor out of Experimental

**Exists:** Preview `NativeAgent` (`executors/native/agent.py`) via `TaskRunner` + `PromptBuilder` + correction-manifest prefix; doctor tier Experimental.

**Missing:** Not on MCP lease/`gated_write` + HookPolicy path; no shared `next_actions` repair contract with MCP verify; no coding-CLI timeout/sandbox parity.

**Next:**
1. Route native `apply_patch`/`write_file` through `execution/gated_write.py` (same policy as MCP).
2. After verify, inject `verification.next_actions.split_next_actions` into the native loop (mirror MCP closed-loop).
3. Honor `execution.command_timeout` + optional `verification/sandbox.py` docker/nix like coding-CLI profiles.
4. Promote doctor tier only after e2e parity with `tests/unit/test_mcp_closed_loop.py`.

**Implemented (2026-07-08):** All four shipped.
- `NativeAgent` now acquires a task lease (`lease_ops.checkout_task_payload`) at the start of its loop and routes every write through `execution/gated_write.py` — `apply_patch` → `apply_patch_payload`, the path+content fallback → `write_file_payload` — so native writes hit the same lease + scope + `HookPolicy` gate as MCP (no more direct `TaskRunner` writes bypassing it). Lease is released in a `finally`.
- Closed loop: on `finish`, the agent verifies through `task_gate_ops.verify_task_payload` (the exact surface MCP's `verify-leased` shells into, run via `asyncio.to_thread` to avoid nesting event loops). Blocking gaps are formatted from the shared `split_next_actions` blocking/advisory arrays and fed back into the message loop for bounded self-repair (`MAX_VERIFY_ROUNDS`), so repair guidance is byte-identical to MCP.
- Sandbox/timeout parity: `run_command` already honors `execution.command_timeout` via `TaskRunner`; `sandbox="docker"|"nix"` routes verification through `verification/sandbox.py` (`get_sandbox`), whose per-command ceiling reuses the same `command_timeout` knob.
- Doctor tier promoted **Experimental → Preview** (not Stable — the LLM loop itself is still preview quality; the *safety path* is now certified). Gated on `tests/unit/test_native_closed_loop.py`, which mirrors `test_mcp_closed_loop.py` (checkout→gated write→verify BLOCKED→next-actions repair→re-verify PASS), plus out-of-scope rejection, `command_timeout`, and docker/nix sandbox routing. `ruff check src tests` clean; native + MCP + doctor-maturity + go-repair subsets pass.

### 7. Knowledge graph depth

**Exists:** Blast radius / call sites / subsystem neighbors in `execution/prompt_builder.py`; optional `integrations/code_review_graph.py` + `dev graph-context`; orphan/planned-file gates; wiki incremental update + `dev wiki install-action`.

**Missing:** No architecture-drift gate for mapped subsystem boundaries; wiki not a verify post-step for large refactors; impact text weak when map/graph absent.

**Next:**
1. Always inject dependents + neighbors ("changing X touches Y") from `repo_map.json` even without code-review-graph.
2. Add `verification/checks/subsystem_boundary.py`: block/advisory when edits cross non-neighbor areas without plan coverage.
3. After verify, if change spans N+ subsystems or M+ files, run `dev wiki update` (or flag stale wiki pages).
4. Optionally treat stale repo map as blocking for hard-difficulty tasks.

**Implemented (2026-07-08):**
- **`indexing/subsystem_map.py`** — shared, pure helpers over a loaded `repo_map.json` (`area_for_path` longest-prefix + `files[].area` fallback, `neighbors_for_area`, `are_neighbors`, `dependents_of`, `areas_touched`, `cross_boundary_pairs`, `impact_targets`), used by both the prompt builder and the boundary gate so they agree on the area graph.
- **`execution/prompt_builder.py`** — new always-on **`_impact_section`** ("Impact (changing X touches Y)") injected from `repo_map.json` dependents + subsystem neighbors, **independent of** the optional code-review-graph CLI (present on the common keyless path). High-priority/short segment ordered right after structural context; new files are marked "no importers yet". Complements the existing detailed `_dependents_section`/`_call_sites_section`.
- **`verification/checks/subsystem_boundary.py`** — advisory `detect_subsystem_boundary_gaps(...)`: flags a change that edits two subsystems the map does NOT consider neighbors when the crossing was not declared by the task plan (both areas in `planned_files`). Emits `architecture_drift` gaps, **non-blocking by default** (`verification.subsystem_boundary.blocking` to enforce), degrades to a no-op without a `subsystems`/`neighbors` map. Wired into `verify_orchestration` alongside the semantic-diff/dependency checks (gated by `verification.subsystem_boundary.enabled`).
- **`verification/wiki_refresh.py`** — post-verify `evaluate_wiki_refresh(...)`: when a verified change spans ≥ `min_subsystems` areas OR ≥ `min_files` files it flags the stale wiki pages a refresh would rewrite (cheap, no model calls) or, with `verification.wiki_refresh.auto_update`, runs `dev wiki update --no-llm`. Wired as a best-effort, non-blocking post-step in `verify_orchestration`.
- **Config:** added `SubsystemBoundaryConfig` (`enabled`, `blocking`) and `WikiRefreshConfig` (`enabled`, `min_subsystems`, `min_files`, `auto_update`) under `verification`.
- **Tests:** `tests/unit/test_subsystem_map.py` (7), `test_subsystem_boundary.py` (6), `test_prompt_impact.py` (4), `test_wiki_refresh.py` (6) — 23 tests.

### 8. Type hygiene / dogfood

**Exists:** CI mypy + `coverage run -m pytest` + `coverage report`; `check_status_doc_drift` in `cli/commands/doctor.py` (5 Stable areas); historical ~75 mypy errors / 36 files.

**Missing:** No `fail_under` coverage floor; narrow status-doc mapping; mypy 1.20.2 currently INTERNAL ERROR locally (re-count after pin/fix).

**Next:**
1. Unblock mypy (pin or upgrade), then burn down full-repo errors to green.
2. Add `[tool.coverage.report] fail_under = …` and fail CI on breach (start low, raise).
3. Expand `STATUS_DOC_UNIT_TEST_DIRS` (gating, execution, indexing, reporting, executors) as areas claim Stable.
4. Doctor rows for "mypy green" and "coverage floor configured".

**Session checks (2026-07-08):** `ruff check src tests` clean; `mypy src` INTERNAL ERROR on 1.20.2 (prior count ~75 stands until re-measured).

### Implemented (2026-07-08) — Area 4: CI/team evidence

- **`reporting/evidence_html.py`** + `dev report --evidence-html PATH`: self-contained HTML AC→evidence table with task/diff links; advisory vs blocking gaps documented in-page.
- **`repo/ci_scaffold.py`**: `render_evidence_workflow()` / `scaffold_evidence_ci()` emit `.github/workflows/devcouncil-evidence.yml` (verify → JSON/HTML artifacts → `upload-artifact` → optional `--github` / `--github-pr-comment` with `GITHUB_TOKEN`, `GITHUB_REPOSITORY`, `GITHUB_SHA`, `GITHUB_PR_NUMBER`).
- **`dev scaffold-ci --evidence`**: writes the evidence workflow alongside the starter CI workflow.
- **`dev report --github`**: prefers `GITHUB_SHA` when set (Actions-friendly).
- GitHub Check conclusion remains blocking-only (`GitHubCheckGenerator` unchanged).

### Implemented (2026-07-08) — Area 8: Type hygiene / dogfood

- **mypy**: runs cleanly (no INTERNAL ERROR on current dev deps); reduced from ~82 to 42 errors (remaining in llm/provider, executors, semantic_layer, etc.).
- **`[tool.coverage.report] fail_under = 18`** in pyproject.toml; CI `coverage report` now enforces the floor.
- **`STATUS_DOC_UNIT_TEST_DIRS`** expanded (gating, executors, execution, reporting, indexing) with flat-test prefix map for subsystems without dedicated dirs.
- **`dev doctor`** rows: **Coverage floor** and **mypy green**.

## Feature ideas

16. **`dev verify --watch` / incremental verification** — re-run only gates affected by changed files; makes the sidecar loop feel instant.
17. **Flaky-evidence detection** — re-run failed evidence once before blocking; distinguish "test is flaky" from "change is wrong" in next-actions.
18. **Team/CI evidence sharing** — export the requirement→task→diff→evidence graph as a PR artifact so reviewers see the trail without installing DevCouncil (extends existing PR-comment integration).
19. **Cost budgets** — telemetry already tracks per-call cost; add `dev cost budget` that warns/blocks a run when a plan or repair loop exceeds a spend cap.
20. **Doc/status drift check** — project-status.md says storage is "covered by unit tests" but tests/unit has none for storage/; a `dev doctor` check could keep status claims honest.

## Log-audit session (2026-07-06)

Findings from auditing `.devcouncil/logs/` and `benchmarks/results/`; all shipped this session.

**Shipped:**
- **`dev go` final report crashed with `AttributeError: 'OptionInfo' object has no attribute 'expanduser'`** — `go.py` called the Typer command `report()` directly, omitting `evidence_json`/`fail_on_blocking`, so raw `OptionInfo` defaults leaked in. Every arm-B bench run (20260706T182554Z) exited 1 AFTER completing its work, mislabeling 4/4 and 5/5 tasks "incomplete" (verdict calibration read 0%). Fixed at the call site; `report()` also normalizes OptionInfo defaults defensively. Swept all other direct command calls (`run`, `approve`, `verify`) — they pass every parameter; no other instance.
- **Test-fixture noise polluted real telemetry** — 298/302 entries in `model_calls.jsonl` and most ERROR/WARNING lines in `devcouncil.log` were test artifacts (TASK-900, fake 403s). New `DEVCOUNCIL_LOG_DIR` override honored by `configure_logging`/`set_log_dir`/`_log_model_call`; session-scoped conftest fixture sets it. `dev logs`/`dev doctor` resolve through the same helper. `traces.jsonl` deliberately NOT redirected: it is per-project state with project-root-keyed readers.
- **~16% of devcouncil.log was the same "Logging configured" DEBUG line** — now announced only when the (console level, file path) config actually changes.
- **`model_calls.jsonl` records unattributable** — `run_id` was None on 100% of records; `latency_ms`/`provider` only populated by Ollama. Added a ContextVar (`telemetry/context.py`) set in `Orchestrator.start_run` with router fallback, and latency/provider tagging to OpenRouter/Doubleword/VertexAI.
- **Bench harness marched through sweeps with a non-starting executor** — 2026-07-03 run scored arm B 0/N on 11 tasks (~8s each, $0) because `claude-agent-sdk` wasn't installed; the failure text matched no infra pattern. Added "agent sdk is not installed" / "not found on path" / "unknown agent profile" to `_EXECUTOR_INFRA_PATTERNS` and `_NONRETRYABLE_INFRA_PATTERNS`.
- **Session/notification traces carried no identifying payload** — hooks now record `session_id` (start/end), end `reason`, and put the notification message in the summary. Note: 34 starts vs 19 ends is Claude Code not firing SessionEnd on crash; consumers must treat unpaired starts as open sessions.

**Verification status:** all changed files py_compile clean; override/dedupe/ContextVar/pattern/ledger logic unit-verified standalone. Full pytest not run (sandbox lacks Python ≥3.12) — run `uv run pytest tests/unit` locally, especially `test_logging_setup.py`, `test_llm_router.py`, `test_cli_logs.py`.

## Subprocess-timeout migration (2026-07-06, follow-up to backlog #1)

Completed the "reuse across the codebase" half of backlog #1. Migrated all remaining unguarded subprocess calls (34 flagged, 2 were false positives/by-design):
- **`run_git`/`git_output` adoption:** go.py (all 8 git calls incl. the end-of-run auto-commit and squash path), check.py, report.py, run_trace.py, context_builder.py, repo_mapper.py, mcp/util.py, orphan_diff.py.
- **Explicit timeouts (env=/input=/DEVNULL semantics preserved):** checkpoints.py (all 9 calls; snapshot `git add` gets 2× GIT_TIMEOUT), clean_git.py, fetch.py (`git clone` 600s → RuntimeError on expiry), sandbox.py (docker/nix commands bounded by `execution.command_timeout` via new `_run_sandboxed`, timeout → rc 124; `uv --version` 10s).
- **Left alone:** task_runner.py (already had timeout — scanner false positive), coding_cli.py Popen (streamed child with its own lifecycle timeouts).
Post-migration scan: zero subprocess calls without a timeout remain in src/. All files compile; run_git prefix/failure semantics and _run_sandboxed's 124-on-timeout verified standalone. Full pytest still pending locally (Python ≥3.12).

## Local-monitor safety guardrails (2026-07-06)

Motivated by the 2026-07-03 `local_monitor_*` calibration probes (Ornith-35B): `samples=1` rubber-stamped 1/6 buggy criteria as passing (both single-shot runs); `samples=3` + `per_criterion=true` caught 6/6 with zero false passes. Auto-resolution already picks safe local defaults; the gap was that unsafe states could arise SILENTLY:
- **Explicit unsafe overrides now warn (config still honored):** `AcceptanceCheckConfig.unsafe_override_warnings()` flags `samples<3` / `per_criterion=false` on a local monitor; `ReviewerCheckConfig` likewise flags single-shot voting on a local reviewer. Logged from `verify_setup.resolve_verify_context` and `live/reviewer._samples`. Cloud monitors are unaffected (single-shot is their intended default).
- **Silent config-failure fallback now warns:** `resolve_verify_context`'s broad `except` used to drop to `samples=1, per_criterion=False` with no signal — the exact unsafe mode if the monitor is local. It now logs why and what that implies.
- Unit tests added to `test_local_llm_calibration.py` (warn only on local + explicit unsafe; auto and safe-explicit never warn).
Note: Ollama context truncation was checked and is already handled (adaptive num_ctx, 16k default, 64k cap in `OllamaProvider`).

## Local-monitor guardrails, second pass (2026-07-06)

- **`warn_once()` in `telemetry/logging_setup.py`:** the new unsafe-config warnings fire from per-task (`resolve_verify_context`) and per-review (`reviewer._samples`) paths — a 20-task run would have printed 20 identical lines, recreating the log-spam problem this session started by fixing. Process-level dedupe by message.
- **`dev doctor` row (`check_local_monitor_sampling`):** surfaces the same unsafe overrides at setup time (Risky rows), and when the monitor/reviewer IS local with safe settings, prints one OK row showing the resolved ensembling (samples/repairs/per_criterion/votes). Cloud configs add no rows. Never raises.
- Verified standalone: warn_once dedupe, and doctor row logic across local-unsafe (3 Risky), local-auto (1 OK), and cloud (0 rows) cases.

---

## `dev map` engine: performance pass and gortex gap review (2026-09-02)

Goal: benchmark the mapping engine against [zzet/gortex](https://github.com/zzet/gortex),
find and harden the bottlenecks, and close reachable gaps. Every number below is
measured on this repository (1,308 tracked / 994 code files) unless stated, with
a scaling check on a 4,321-file repository. Harness: `benchmarks/map_bench.py`;
raw results in `benchmarks/results/map/`.

### Results

| stage | before | after | change |
|---|---|---|---|
| `cold` build | 3.37s | 2.13s | −36.8% (295 → 466 files/s) |
| `warm` (no change) | 230ms | 196ms | −14.7% |
| `touch` (one file changed) | 2.04s | 1.31s | −35.7% |
| `manifest` | 520ms | 367ms | −29.5% |
| **`dev map` end-to-end** | **2.44s** | **1.04s** | **−57.5%** |
| `code_graph.json` | 26.21 MB | 20.59 MB | −21.5% |

Minimum of 5 repeats, baseline `20260902T164552Z` vs `20260902T181600Z`. Run-to-
run spread on an otherwise-busy machine is a few percent on every stage except
`e2e`, which is stable to ±2%; a run taken while a background daemon was
re-indexing measured `cold` at 2.38s rather than 2.13s, so compare like with
like. Scaling check on a 4,321-file repository: 359 files/s cold, and
`code_graph.json` 104.5 MB → 84.3 MB.

Verified: 668 Rust tests, 4,114 Python tests, 0 failures; `cargo fmt --check`
and `cargo clippy` clean on every changed crate. Artifacts are byte-identical
under `RAYON_NUM_THREADS` of 1, 2 and 8.

### Defects found (each was silent)

1. **Search returned nothing for any query whose top match was a large symbol.**
   A hit costs `source_span.len() / 4 + 20` tokens — the symbol's whole body —
   against a 2,000-token budget, so one 8 KB function was unreturnable. The
   response said `{total: 1, shown: 0, truncated: true}`, which reads as "too
   many results" rather than "the one result did not fit", and `DevMapClient`
   logged it as a kernel failure and fell back to the Python path on every such
   query. `devmap search "resolve calls"` returned zero of its one match.
   Fixed by capping the span to the budget and reporting the omission in a new
   `source_span_omitted_bytes`, so a capped span is never passed off as the
   verbatim body R2 promises. The hard-budget contract is unchanged.
2. **The store reclaim decided on stale accounting.** `vacuum_if_needed` read
   `PRAGMA freelist_count` before the WAL was checkpointed, so it saw the state
   *before* the two prunes that run immediately above it — declining to reclaim,
   in 0 ms, while a third of the file was free. Eight consecutive builds
   declined while the store sat pinned at 295 MB.
3. **A rebuilt kernel kept being served by the old daemon.** The IPC handshake
   checks `PROTOCOL_VERSION`, a wire-format number that does not move on a
   rebuild, so a daemon started before `cargo build` served pre-fix answers for
   its full 30-minute idle bound. Found the hard way while fixing (1): the
   rebuilt binary returned the match and the daemon in front of it did not.
   The daemon now retires when its own executable's (size, mtime) changes —
   identity, not a version string, for the reason `find_engine_binary` already
   documents: every build here reports `devmap 0.1.0`.

   Unlike the idle bound, this deliberately does *not* wait for the pending
   queue to drain. The first attempt copied that guard and the retirement then
   never fired on this repository, which always carries pending paths — and
   draining under a superseded binary is the very outcome the check exists to
   prevent. The queue is persisted in the store, so the next client's daemon
   picks up the same paths and drains them with the new kernel. Verified
   end-to-end: retires ~5s after the binary changes, with pending work present.

### Bottlenecks removed

Ranked by measured share of wall time when found:

- **Python re-serialized the whole graph to stamp three fields** — 1.68s of a
  2.72s `dev map`. `devmap manifest` now takes `--generated-head`,
  `--indexed-hash` and `--content-fingerprint` and writes them itself; the
  read-modify-write survives only as the fallback for a kernel too old for the
  flags, because an unstamped map reads permanently stale to `map_is_stale`.
- **A full `VACUUM` ran on nearly every build** — 937ms, 28% of an incremental
  build, to reclaim a few percent of the file. Now `PRAGMA incremental_vacuum`
  (2ms). Legacy `auto_vacuum=NONE` stores convert on the vacuum they were
  already going to pay for.
- **The writer recompiled its SQL per row** — ~73,000 edge inserts and ~146,000
  path-id lookups. `prepare_cached` plus a per-transaction path-id memo:
  `persist:write` 1180ms → 613ms.
- **`mcp` was imported by every `dev` command** — 208ms of a 490ms CLI import,
  via `cli.commands.lease` → `execution.lease_ops` → `integrations.mcp.util`,
  for a symbol only one function needs at runtime.
- **SQLite defaults** — `synchronous=FULL` fsyncing every commit of a *derived*
  index, and a 2 MiB page cache for a bulk generation write.
- **Resolution was the last serial CPU phase** — and it does not shrink on an
  incremental build, since it deliberately covers the whole tree so liveness and
  community detection mean the same thing on both paths. Now parallel.
- **No `[profile.release]`** — default `codegen-units = 16`, no LTO, for a
  binary built rarely and run on every agent turn.
- **`code_graph.json` was pretty-printed** — 23% whitespace in a machine-only
  artifact. `repo_map.json`, which agents do open, stays indented.

Observability: `devmap build` now reports per-stage timings, with the persist
phase split into write / prune / prune / reclaim, and the reclaim decision
printed with the freelist ratio it saw — because "declined" and "reclaimed
nothing" leave an identical file behind.

### Gaps vs gortex — status

All eight are closed. They are listed in the order they were reviewed, not the
order they were built, so the numbering matches the original gap review above.

Two of the eight were closed by *deleting* rather than adding — the Python
embedding index (#3) and the constant `summary` field (#7) — and one, #7, was
deliberately closed short of what was measurable, because the remaining 46%
could only be had by breaking the interface the agent guides document.

1. **Language breadth.** ~~Absent~~ **Closed.** Tier-2 declaration recovery
   (`devmap-extract/src/fallback.rs`): a file whose language has no linked
   grammar now contributes pattern-matched declarations instead of nothing,
   stamped `ExtractionEngine::RegexFallback` / `ParseOutcome::Fallback` so a
   consumer can tell a matched symbol from a parsed one. `.proto` and `.ps1`
   were being indexed as `generic` and contributing zero symbols; on the
   scholarlm corpus the fallback recovers 19 protobuf and 9 PowerShell files.
   Prose and data formats are excluded by `NON_DECLARATIVE_LANGUAGES` — an
   early version invented `ReasoningBank` and `HyDEService` out of fenced code
   blocks in Markdown design docs. Fallback files are exempted from the
   dead-code sweep, because they emit no calls and every symbol in them would
   otherwise read as callerless.

5. **Clone detection.** ~~Absent~~ **Closed.** `devmap clones` / `dev map
   clones`, reporting duplicated symbol bodies in two tiers: `exact` (the same
   code modulo formatting and comments) and `structural` (the same shape under
   renaming, callables only). On this repository: 388 groups — 112 exact, 276
   structural — over 11,495 signed symbols. Findings are real, e.g. `_repo_map`
   duplicated across two test files, `modifier_nodes` shared by
   `langdecl/kotlin.rs` and `langdecl/swift.rs`, `collect_args` shared by
   `dc-grep` and `dc-verify`, and four near-identical integration functions in
   `cli/commands/integrate.py` (`grok`/`opencode`, `antigravity`/`aider`).

   Design notes worth keeping:
   - **Signatures come from the parse tree, not the text.** A text-based
     comment stripper collapses `a = "//foo"` and `a = "//bar"` to the same
     prefix and reports two unrelated functions as identical. Comments are
     grammar-declared nodes and whitespace is not in the tree at all, so the
     tree-based hash is comment- and format-immune by construction.
   - **Stamped at extraction, not analysis.** Extraction caches strip
     `source_code`, so an analysis-time fingerprint would see bodies only for
     files edited in that build.
   - **Stored per symbol, grouped on demand.** Three nullable columns on
     `generation_nodes` (schema v12), not a persisted group table: a group is a
     join over facts, and persisting it would store a truncated derived view
     that goes stale. Cost is +1.31% store size (169.17 → 171.39 MB).
   - **The size floor is measured.** `MIN_SIGNATURE_NODES = 32` sits in the gap
     between accessors (Python getter 15, TypeScript 20, Go 23, Rust 24, Python
     one-line delegate 25) and real bodies (Python guard-and-call 36, Go error
     wrap 43, Rust three-line 52, Go three-line 60). A floor of 24 would admit
     the Go and Rust getters.
   - **Coverage travels with the report.** `signed_symbols` / `unsigned_symbols`
     are printed even when nothing is found, because an empty list says the same
     thing for a clean tree and for a tree nothing was examined in.
   - **Build cost is below the noise floor.** Measured by interleaved A/B — two
     release binaries differing only in whether stamping runs, alternating over
     7 rounds so both arms see the same machine load. Minimums: 2.238 s with
     stamping, 2.387 s without. The stamping arm came out *faster*, which is not
     a speedup but the honest reading that the difference is smaller than
     run-to-run variance on a loaded machine (±6%). A straight
     `map_bench.py --baseline` comparison on the same machine read "+143.9%
     cold" for the same code, which was load, not regression: three consecutive
     runs of one unchanged binary gave 4.35 s, 2.96 s and 5.20 s. Verify the two
     binaries actually differ before trusting any A/B — here, 11,514 signed
     symbols against 0.

4. **Speculative execution.** ~~Absent~~ **Closed.** `devmap preview` /
   `dev map preview`: extract a candidate buffer in memory, diff it against the
   file on disk, and report symbols added, removed and re-declared plus the
   calls from other files that a removal or re-declaration would affect. Nothing
   is written and no generation is committed (`preview_writes_nothing` asserts
   both).

   Three findings that shaped it, each one a wrong answer first:

   - **A buffer that does not parse yields no symbols**, so a naive diff reports
     every symbol in the file as removed and every caller as breaking — the most
     alarming output the tool can produce, from a half-typed edit, which is
     exactly when an agent would be asking. A failed parse now returns
     `delta_available: false` and no symbols; a partial parse reports the delta
     with an explicit warning that a symbol inside an error region will look
     removed.
   - **`generation_edges.target_symbol` holds qualified names** (`path::Name`).
     Matching callers on the bare `symbol_name` returned zero rows and printed
     "no calls from other files are affected" — a plausible-looking answer that
     meant the feature's more valuable half had never run.
   - **`ExtractedSymbol.signature` is populated by one grammar.** 80 of ~2,300
     sampled symbols, all Go; null for Python, Rust and TypeScript. Comparing it
     made every signature change in those languages look like a body change. The
     declaration/body split now comes from a `declaration_hash` computed off the
     parse tree with the `body` field child excluded — in memory only, never
     persisted, because the one consumer extracts both sides in the same
     process.

   Two accuracy decisions worth keeping:

   - **Callers are filtered by resolver confidence.** The 50,533 call edges sit
     in three tiers — 1.0 (19,134), 0.9 (4,800) and 0.2 (26,599) — and the 0.2
     tier is name-only attribution. Every `dict.get(...)` in the tree resolves to
     `LLMCache.get`, giving that one method 921 edges, none of them real.
     `PREVIEW_CALLER_MIN_CONFIDENCE = 0.5` sits in the empty band between the
     tiers. Excluded edges are *counted* in `ambiguous_callers`, not dropped, so
     "no callers affected" cannot quietly mean "none we would vouch for".
   - **The diff is against disk, not the index.** The user is editing the file
     that is on disk; diffing against a generation built from an older commit
     would report their own already-saved work as part of the candidate change.
     The index is still consulted, but only for the caller graph, where being a
     generation behind is a stated property rather than a wrong diff. Paths
     resolve against the generation's `repo_root`, since the CLI and the daemon
     have different working directories.

2. **Cross-repo / multi-repo graph.** ~~Absent~~ **Closed.** `devmap workspace`
   — a registry (`.devcouncil/workspace.json`), federated search across every
   registered repository, and cross-repository link candidates.

   What it deliberately does *not* do is join repositories on symbol names. Two
   repositories both declaring `New`, `Client` or `get` is the normal case, not
   a dependency, and asserting edges from it would manufacture them at the scale
   the resolver already records at 0.2 confidence *within* one repository. The
   only cross-repository relation asserted is "repository A imports a module
   repository B declares", evidenced by B's `go.mod` module path or a top-level
   Python package, with the evidence string carried on every candidate. Verified
   on a two-repo fixture: `svca example.com/libb/store -> libb (go.mod declares
   module example.com/libb)`, and `shared_symbol_names_alone_do_not_make_a_link`
   pins the negative.

   Prefix matching is on segment boundaries, not `starts_with`: `manvibench` is
   not an import of `manvi`. Repositories that cannot be queried are named in
   the response — a federated answer assembled from three of five repositories
   is not a federated answer, and the reader has no other way to know.

3. **Vector search in the kernel.** ~~Outside the kernel~~ **Closed.**
   `devmap search --semantic` / `dev map search --semantic`, TF-IDF over symbol
   names in `devmap-query/src/semantic.rs`.

   Nothing is stored. The vocabulary, document frequencies and vectors are all
   derivable from `generation_nodes`, so precomputing them would put a second
   copy of a derived fact in the database — one to rebuild in step with the
   symbols, and stale whenever it was not. That is precisely the machinery the
   Python implementation carried: a `symbol_embedding_idf` table, a build step,
   a generation stamp, and a `stale_rows_skipped` counter for when they
   disagreed. Computing instead costs one pass over ~14,500 names; the whole
   query, including opening the store, measures 40–50 ms.

   The 398-line Python implementation, its build wiring in three places, its
   config flag and its tests are **removed**, not left beside the new one.
   `--semantic` no longer silently falls back to prefix matching when no index
   exists — it reports that it cannot answer, because returning keyword results
   under a flag promising similarity ranking is a different answer, not a
   degraded one.

   One real bug found by its own test: the tokenizer split `LLMCache` into the
   single term `llmcache`, so a query for "llm cache" could not match it. An
   acronym run needs a boundary *before its last capital* when a lowercase
   follows. With that fixed, `LLMCache` ranks first (0.979) for "llm cache";
   before, a `cache.py` file node did.

   A second defect surfaced while measuring it: `cap_source_span` capped a hit
   at the *whole* budget, so a single `File` symbol — whose span is its entire
   file — crowded out every other result. A 4,000-token search returned 2 hits
   and reported 512 withheld. Capped at a quarter of the budget, the same query
   returns 7. That fix applies to keyword search too.

6. **Notebooks.** ~~Absent~~ **Closed.** `.ipynb` files are indexed:
   `devmap-extract/src/notebook.rs` reconstructs code cells, parses them as one
   buffer so a symbol defined in cell 3 resolves against a call in cell 7, and
   relocates every resulting span back into the raw file.

   The extractor was written in a concurrent session and left unregistered — it
   did not compile into the crate. Wiring it in surfaced four instances of one
   defect: the synthetic filename used for the parse (`nb.ipynb.py`) leaking
   into `qualified_name`, `caller_symbol`, `parent_symbol`, and the call spans.
   The `parent_symbol` one was the quiet one — the resolver emits a second
   `Contains` edge only when a symbol's parent differs from its file, so every
   notebook symbol gained a containment edge from a file that does not exist.
   A fifth: relocation rebuilt qualified names from `symbol.name`, flattening
   `Holder.method` to `method` while the call attributed to it kept the dotted
   form, so every method declared in a notebook was recorded under a name
   nothing referred to. `no_synthetic_filename_survives_into_the_extraction`
   asserts on the substring rather than the known fields, so the next field
   added gets the guarantee without anyone remembering to extend the test.

   Verified end to end: spans index the raw `.ipynb` bytes (`raw[309:330]` is
   exactly `def load_frame(path):`), prose in Markdown cells contributes no
   symbols, and a cross-cell call resolves.

7. **Compact wire format.** ~~JSON~~ **Closed**, and by measurement rather than
   by adopting a binary format. `repo_map.json` — the artifact the agent guides
   instruct an agent to open before searching — went from 411.4 KB to 303.7 KB,
   a 26.2% reduction, or about 27,600 tokens, matching gortex's GCX1 claim of
   −27% without changing the schema a single consumer reads.

   Two causes: the file was still pretty-printed (22%), and every one of its
   1,306 file entries carried `"summary": ""`, a constant nothing read. The
   *subsystem* summaries are kept, because `map_viz.py` indexes them directly
   and would raise on their absence.

   The deeper compaction was measured and **not** taken: columnar file rows plus
   an interned path table reach 72% (105,329 tokens to 29,333). It changes the
   shape of `files` and `dependents`, which CLAUDE.md documents to agents as the
   navigation contract. Saving tokens by breaking the interface agents are told
   to use is not a saving.

8. **Token-savings accounting.** ~~Absent~~ **Closed.** `devmap savings`, with
   the counterfactual named rather than implied.

   Every figure is bytes ÷ 4 and says so — a tokenizer count depends on the
   model reading it, and a precise-looking number derived from a divisor is
   fabricated precision. The comparison is deliberately conservative: the
   alternative is charged only for reading the files the map *already named*,
   not for the grep that would have been needed to find them, so the reported
   figure is a floor. Indexed files that cannot be read are counted separately
   rather than folded in as zero bytes, which would shrink the corpus and
   flatter the map. And when the files are cheaper than the query, it says so —
   a savings report that can only ever report a saving is advertising.

### Defect found and fixed while building clone detection

`--kind` / `--min-nodes` were filtering the report *after* the token budget had
already cut it, then re-taking the budget over the survivors. Because
re-budgeting a short list leaves `hidden` at zero, the result came back labelled
complete: `--kind exact --min-nodes 100 --budget 900` reported "2 groups, not
truncated" where the true answer was 29. Filtering now happens before the
budget, inside `StoreQueryEngine::clones`, and the helper that made the wrong
order possible was deleted rather than documented.
`clone_filters_apply_before_the_token_budget` pins it, and fails against the
pre-fix ordering.

### Known flake, not fixed

`protocol::tests::a_second_binder_is_refused_while_the_first_holds_the_endpoint`
fails intermittently under heavy CPU load — it releases an advisory `flock` and
rebinds immediately, assuming the release has taken effect. It failed twice
while a full pytest run saturated the machine and passed 52/52 across five
consecutive clean runs. Pre-existing and not a product bug: `flock` is released
by the OS on process exit, so the leftover PID-named `.lock` files in TMPDIR are
harmless. Left alone deliberately — retiming a locking test could mask a real
regression, and the honest report is worth more than a green run.

## Dev Map kernel audit, second pass (2026-09-02)

Scope: everything between `dev map` and the store — the Python seam, the kernel's
store/build/drain, the daemon and query surfaces — audited against the gortex
feature set, with every fix carrying a test that failed first and the whole thing
run against this repository's live 700 MB store. Nothing here is committed.

### The defect that mattered most: two engines, one artifact

The Rust kernel built the map, but the retired Python engine still ran on every
verify, checkout, `dev plan`, `dev init`, `dev map init|ingest|sync`, MCP
`graph_ingest`, the post-tool-use hook, and — through the `SyncCoordinator` the
MCP server started in its lifespan — on every file edit. Each of those rewrote
`repo_map.json` and `code_graph.json` from a generation built at an older HEAD,
after the kernel had written them. The last writer won, silently.

Now `indexing.map_artifacts.refresh_map_artifacts` is the only path to the
artifacts and it builds through the kernel; there is no fallback (it raises
`DevMapEngineError`; verify/checkout catch it). The Python store
`.devcouncil/codeintel/index.sqlite` is a read cache that `load_code_graph`
fills from the kernel's JSON. The Python watcher, incremental sync, isolated-build
worker and hook refresher are deleted; the MCP server warms the kernel daemon
instead of running its own.

### Python seam — defects fixed (each with a failing-first test)

| # | Defect | How it showed | Fix |
|---|--------|---------------|-----|
| 1 | Stale binary chosen: the release build was older than the debug build and the uv tool fell through to `~/.cargo/bin/devmap`; each refused the schema-12 store with a bare version number | `dev map` exit 1 | `find_engine_binary`: `DEVMAP_BINARY` override, else the newest *capable* build (capability probed via `manifest --help`, because every build prints the same version) across `<repo>/rust-port/target`, `<package>/rust-port/target`, PATH; the refusal names the binary, its build time, the store's schema and the rebuild command |
| 2 | A status probe auto-spawned a daemon that reconciled the tree and committed its own generation seconds after the CLI build | generation 1 → 3 after one build; 288 daemons left behind by one test run | `DevMapClient(autospawn=False)` for probes; `DEVMAP_AUTOSPAWN=0` process-wide; the test suite sets it |
| 3 | Client and kernel derived different socket paths (SHA-256 `/tmp/devmap-<hex>.sock` vs FNV-1a `<tmp>/devmap-<hex>/ipc.sock`, root hashed as spelled) | two daemons per repository | one formula, owned by the kernel (`devmap serve --print-socket-path`), mirrored in `devmap_client.default_socket_path`, pinned by FNV vectors and a parity test |
| 4 | The guides a build writes made the map stale one second later | three-file repository stale on a fresh map | rebuild after a guide changes (two files, incremental); a repository without guides commits two generations on its first build, explicitly pinned |
| 5 | `dev map --watch` polled | timer loop | watchdog events, 0.5 s debounce, 30 s poll as the safety net; a 2,000-file burst is one rebuild |
| 6 | `dev map query` measured edges for every definition matching a name | unbounded fan-out | exact matches first; edges for the first five; the rest say why they were not measured |
| 7 | `--no-liveness` / `--lsp-refs` accepted and ignored | silent | rejected (exit 2) |
| 8 | Status/doctor described the retired engine | `dev map status` | `devmap_health`: binary + build time, store schema vs kernel schema, free pages, WAL, kernel freshness with stuck paths named, daemon, who wrote each artifact; doctor verdicts with fixes, exit 1 on critical |
| 9 | Map artifacts could be stamped over a failed build | `graph_degraded` lean maps | fail closed: a failed or timed-out kernel build leaves the prior artifacts byte-identical |

### Kernel — store/build/drain (K) and serve/query (S)

Two Opus agents, one per crate group; 27 new store/CLI tests, 10 serve fixes.
`cargo test --workspace`: 828 passed after their passes.

- **K1 pending queue.** 51,136 rows on the live store (799 consecutive full rebuilds at the old 64-row batch), 188 quarantined including the *old* checkout path, directories and `README.md`. Canonical enqueue with escape refusal, structural reconcile at every build, refusals are not work, per-path attempt accounting (a failing path no longer charges its batch mates), `devmap repair --pending`, status names the stuck paths, batch bound 64 → 8192.
- **K2 reclaim.** Checkpoint result discarded, then (found live) `execute_batch` steps `PRAGMA incremental_vacuum(N)` exactly once, so one page per build: 66% → 68% free across four builds of a 701 MB file. Reproduced on a copy: unstepped → −1 page; 65,536 rows stepped → 702 MB → 429 MB in 4.5 s. Fixed by stepping every row and reporting pages freed.
- **K3** schema refusals name the store, versions and remedy; `--version` prints the schema; status never migrates. **K4** `--full`. **K5** prose/data formats are not parse failures. **K6** default `--db` is the seam's store. **K12** poisoned mutex is an error. **K13** advisory writer lock names the holder instead of "database is locked"; generation writes are `BEGIN IMMEDIATE`.
- **Directory rows survive a build** (found live: 918 `target-*/debug/.fingerprint/<crate>-<hash>` directories after two full builds, status NOT FRESH forever). A whole-tree build now supersedes every row queued before it started.
- **Cache directories indexed** (found live: 1,041 of 2,363 files in generation 779 were cargo `.fingerprint/*.json` under untracked, unignored `target-serve`/`target-store`). Directories carrying a `CACHEDIR.TAG` are skipped by discovery, affected builds and the watcher.
- **S1** preview path traversal (`../secrets.py` answered). **S2/S4/S6** budgets honoured (snapshots charged a flat 50 tokens; workspace search reported partial totals as complete; semantic search read 500 files to show 11). **S3** scoped trace O(V×E) → 2.67 s to 0.08 s. **S5** `workspace.json` atomic + flock. **S7** a timed-out query is cancelled, not abandoned on a pool thread. **S8** SIGTERM/SIGINT release socket and lock. **S9** socket path from the canonical root. **S10** a daemon whose repository or store vanished exits.

### Field results (this repository, rebuilt release kernel)

| Measure | Before | After first build | After follow-ups |
|---|---|---|---|
| pending rows | 47,095 | 918 (directories) | 0 |
| quarantined | 188 | 0 | 0 |
| files indexed | 2,363 | 2,363 (1,041 cargo output) | 1,318 |
| store | 669 MB, 66% free | 669 MB, 68% free | 209 MB, 0% free |
| `dev map`, changed tree | refused (schema) | 10 s | 4 s |
| `dev map`, unchanged tree | refused (schema) | 3 s, no new generation | 1 s, no new generation |
| doctor | critical | healthy, 2 warnings | healthy |

Kernel verification after the follow-ups: `cargo fmt --all --check` clean,
`cargo clippy --workspace --all-targets -- -D warnings` clean, `cargo test
--workspace` 835 passed / 0 failed (60 new kernel tests in all). Python stress
suite (8 concurrent builds, SIGKILL mid-persist, hostile tree, 2,000-file burst,
silent daemon, budget contract) 7 passed; the client's socket-path parity test
passes against `devmap serve --print-socket-path`.

### Tests deleted, and why

`test_codeintel_sync.py` lost the tests of the Python incremental sync, the
change-set probe and the lease-contention lean map — they tested code that no
longer exists and encoded the two-writer behaviour this pass removes. Their
invariants that still apply (fail closed, keep the prior artifacts, reuse a
generation on an unchanged tree) are re-expressed against the kernel in the
same file. The isolated-build tests in `test_graph_build_control.py` and
`test_coverage_wave7.py` went with `run_isolated_full_build`.

### Toward a gortex-class tool

Closed: daemon lifecycle and endpoint identity, transactional store with repair,
agent-facing budgets, actionable status. Next: serve `graph_query` / `trace` /
`context` from the kernel rather than the Python cache; tiered map reads for
token economy; ship `devmap` with the tool install. Gaps: grammar breadth (30
linked vs gortex's 257 claim), an embedding index, cross-repository contracts,
diff-scoped review.

### `RepoMapper.map_repo` and the Python graph builder are gone (2026-09-02, evening)

`RepoMapper.map_repo` built a Python `CodeGraph` (`indexing.graph.build.build_code_graph`)
and wrote `.devcouncil/graph/code_graph.json` — the retired engine, and a second writer of an
artifact the Rust kernel owns. Two signals said it had no production callers (`rg -uu` over
`src/` found no `.map_repo(` call; the only remaining `map_repo` the kernel resolves is the
unrelated `dev map` CLI command), and `refresh_map_artifacts` had stopped calling it. The
Python-writer retirement agent had already removed the method (−948 lines) and
`build_code_graph` (−485) before it was stopped; this pass finished the job so the tree is
coherent again:

- **Deleted** `indexing/graph/resolve.py` (1,020 lines) and `indexing/graph/extract_ts.py`
  (1,205): no importer anywhere in `src/` once the builder was gone (`rg -ln` empty, both
  with and without ignore rules).
- **The extraction cluster went too, one layer deeper than the first pass expected.**
  `extract_python.py` was left standing on the grounds that the generic extractor and the
  semantic resolver still imported `FileExtraction` — true, but those two importers were
  themselves reachable only from `assemble_graph`: `enrich_semantic_edges` had exactly one
  call site (`graph/build.py`), and `extract_generic` exactly one (`extract_ts.py`). With the
  builder gone the whole cluster was unreachable, so it went as one piece —
  `codeintel/resolution/` (`semantic.py`, `abstract_state.py`, `frameworks/{base,di,events,routes}.py`,
  1,485 lines), `codeintel/languages/generic_extractor.py` (236) and
  `indexing/graph/extract_python.py` (307). Deleting a leaf because its only caller is dead
  is only safe when you check the *caller's* callers too; stopping at the first live-looking
  import would have left 2,028 lines of unreachable code behind a plausible-sounding reason.
- **Two modules were only half dead, and the half that lives is a verification gate.**
  `graph/liveness.py` (1,124 → 226) and `graph/cache.py` (329 → 77) both looked like pure
  builder dependencies. They are not: `RepoMapper.liveness_snapshot` → `_compute_liveness` →
  `file_liveness` feeds the **liveness ratchet**, and the Python/JS import-edge passes behind
  the same snapshot read and write the `modules`/`specs` parse cache. Deleting either file
  outright — which a file-level reachability pass recommends — would have silently disarmed a
  verify gate. Only the extraction halves went (`symbol_reachability_dead`,
  `token_dead_from_shards`, `build_liveness_shard`, `extract_cached`,
  `extraction_{to,from}_cache_entry`, …); `file_liveness`, `confidence_at_least` and the
  parse cache stayed. `PARSE_CACHE_VERSION` is deliberately *not* bumped: the surviving
  `modules`/`specs` entries are byte-identical in meaning, and a bump would discard every
  warm cache to drop keys nothing reads.
- **`graph/build.py`** (941 → 523) kept what reads and exports a kernel-built graph
  (`load_code_graph`, `write_code_graph`, the slim/compact/stub export tiers, the PDG layer)
  and lost what built one (`build_code_graph`, `assemble_graph`, `extract_all`,
  `extract_paths`, `_build_liveness_shards`, `_area_fn_for`, `_PENDING_ANALYSIS_SHARDS`).
  `content_fingerprint` stays — `RepoMapper.map_is_stale` is its caller — but the sibling
  `_git_head` / `_files_fingerprint` / `_code_files` went with `assemble_graph`.
- **Deleted** `RepoMapper._primary_code_files` and `detect_languages`: reachable only from
  `map_repo`. Every other method is still reached from `map_is_stale`, `get_git_files`,
  `dependents_for` (verify's wiring check), `liveness_snapshot` (the liveness ratchet),
  `_ripgrep_search` and `_scan_dependency_risks` (the seam's enrichment) — checked with an
  AST reachability pass, not by eye.
- **Fixed** two breakages the interrupted retirement had left: `indexing.graph.__init__`
  re-exported the removed `build_code_graph` (so `import devcouncil.indexing.graph` failed,
  taking `load_code_graph`, `query_symbol` and MCP with it), and
  `devmap_engine.compute_freshness` imported `_files_fingerprint` from the removed builder.
  The freshness digests now come from `RepoMapper._files_fingerprint` /
  `_content_fingerprint` — the checker's own methods, so writer and checker cannot drift.
- **Tests.** Fixture-style uses of `map_repo` moved to `tests/unit/support_maps.py`:
  `stamped_repo_map` / `write_stamped_map` (a kernel-shaped map whose stamps match the tree,
  for the stale-map gate, the hook refresher and the verify-time refresh), `stub_kernel` (for
  the seam's goal ranking and opt-in SCA), and `dependents_view` (the live
  `RepoMapper.dependents_for`, so the JS/TS/Go/Rust import-edge claims in
  `test_companion_import_edges`, `test_jsts_resolution`, `test_ts_imports`, `test_parse_cache`
  and `test_review_fixes` keep being tested at the owner verify actually uses).
  Tests whose *subject* was the deleted engine were removed: `test_map_liveness`,
  `test_graph_dead_code`, `test_code_graph`, `test_graph_incremental`, `test_map_verify_parity`
  (both sides now read the same kernel generation), `test_repo_mapper_generic`,
  `test_extract_ts_ast`, and individual tests in `test_repo_mapper_helpers`,
  `test_phase0_liveness_audit`, `test_primary_stack_map_languages`,
  `test_swift_map_language_fix`, `test_ts_imports`, `test_jsts_resolution` and
  `test_coverage_wave5` that named a retired helper. The pruning was mechanical: a test was
  removed only when it failed *because* it named a symbol that exists in `HEAD` and not in the
  working tree (175 such names), iterated to a fixed point.
- **Kernel coverage of the deleted claims** (test names in `rust-port/crates/*/tests`):
  entry roots / unwired / dead-code tiers (`entry_root_paths`, `unwired_candidates`,
  `unwired_discounts_test_only_importers_and_spares_entry_roots`,
  `a_genuinely_uncalled_private_function_in_the_same_file_is_still_confident`,
  `python_dunder_all_*`, `test_x20_g14_decorators`, `is_wiring_decorator`); JS/TS resolution
  (`tsx_relative_imports_use_the_jsts_resolution_ladder`,
  `re_exports_carry_their_source_module_only_when_they_have_one`,
  `ordinary_javascript_export_forms_are_unchanged`); Go packages (`go_package_*`,
  `test_g20_go_package_star_topology`); Rust `use` (`names_in_a_rust_use_list_are_not_uses`);
  Python re-exports (`a_reexporting_package_still_binds_to_its_init`,
  `a_python_reexport_alias_resolves_the_import_that_names_it`).
  **Gaps found, not covered by a kernel test today** (record them before adding the
  behaviour back): tsconfig `extends` / project-reference path mapping; generic subsystem
  inference for non-DevCouncil trees; the primary-stack ordering of `languages`; comment
  stripping not skewing dead-symbol detection (moot with tree-sitter, but unpinned).
- **Route registrations were already gone before the delete.** `api_routes.map_routes` reads
  a `registers` edge for a route's middleware/registration list, and only
  `codeintel/resolution/frameworks/routes.py` ever produced one — the kernel's
  `edge_kind_label` has no `registers` case. So `registrations` has been empty on every
  kernel-written graph since the kernel became the only writer; deleting the producer changed
  nothing and only made the absence visible. Recorded rather than papered over: the consumer
  branch stays, so a future kernel edge kind fills it instead of being re-invented.
- **What the kernel does not yet write, found by converting the tests** (each pinned as a
  strict `xfail` in `tests/`, so the day the kernel fills one the suite says so instead of
  staying quietly green). `rust-port/crates/devmap-query/src/manifest.rs:224-229` writes
  `frameworks`, `package_managers`, `test_commands` and `candidate_files` as constant `[]`,
  and leaves `lsp: {}` / `processes: []` empty; the Python writer detected uv/npm from
  lockfiles and the LSP languages, and `knowledge/wiki.py:151,253` plus
  `mcp/handlers/map.py:212` still render those fields, so they are silently empty rather
  than absent. Every one of the 14 subsystems has `role_files: {}` with no `neighbors` or
  `handoff_paths` — the fields steps 5-6 of the generated agent guide tell agents to
  navigate by, so the guide currently promises what the manifest does not fill. Every file
  is `kind: "code"` (1,298 of 1,298, `README.md` included) where the Python writer
  classified doc/config/test; no `src/` consumer branches on `kind`, so that one is
  degraded rather than broken. No route metadata reaches `code_graph.json` (the
  `HandlesRoute` edges are in the store but not the export), which is what leaves
  `indexing/graph/api_routes.py` — the route map, shape check, api impact, `dev map routes`
  and the MCP `route_map` — without input. And `dev map query <leaf> --json` reports
  `callees_unavailable: "… is not indexed"` for a symbol that *is* indexed and simply has
  no callees, which is a wording bug that reads as a data-loss bug. Subsystem granularity
  also changed (14 whole-repo areas against the old per-package split under
  `src/devcouncil/`); that is the kernel's design, not a gap.
- **Two config knobs with no reader are gone, and a removed key no longer goes quiet.**
  `IndexingConfig.repo_map_dependents_cap` had exactly one reader, `repo_mapper.py:153`,
  which went with `build_dependents`; the kernel writes `dependents` now and does not read
  the cap. `repo_map_unwired_cap` beside it had no reader at all, and none at `HEAD` either
  — `unwired_candidates` has always been bounded by `repo_map_liveness_cap`. Both were
  validated and then ignored, which is what this codebase already refuses for CLI flags
  (`docs/code-graph.md`: "a flag that is accepted and ignored is worse than one that is
  rejected").
  The first pass left them, on the reasoning that their bounds were an acceptance-criterion
  surface. That reasoning was wrong and checking it was cheap: **AC-2.1** reads *"the
  repo_mapper.py file has configurable upper bounds for map size that are significantly
  higher than current limits"* — it names no field, it is satisfied by
  `repo_map_liveness_cap`, which is read, and it exists only in a July 2026 plan run under
  `.devcouncil/runs/`, not in the live ledger. The only thing tying it to
  `repo_map_unwired_cap` was one agent's interpretation, preserved in two leftover scratch
  scripts (`tests/unit/_patch_unwired_cap.py`, which *edits* `config.py` through a lease
  token, and `_patch_coverage_omit.py`, which asserts the field's bounds and prints
  "AC-2.1 pass"). Both scripts went with the fields, as did
  `_attach_repo_map_unwired_cap_bounds`, ~40 lines of pydantic-v2 `FieldInfo` surgery whose
  entire purpose was to make `hasattr(IndexingConfig, "repo_map_unwired_cap")` true for
  that check.
  Removing a key is only half the job, because these models take pydantic's default
  `extra="ignore"`: a config still setting `repo_map_dependents_cap` would have gone from
  validated-and-ignored to silently dropped, which is the same defect one layer down. So
  `load_config` now warns through `RETIRED_CONFIG_KEYS`, beside the existing
  `_migrate_verify_on_post_task` seam — a named home for every future removal rather than a
  one-off. Pinned by `test_a_config_that_still_sets_a_retired_cap_is_told_so`.
- **Total: 6,786 lines out of `src/`** — 4,259 in deleted files, 2,527 trimmed from
  `repo_mapper.py` (3,178 → 2,219), `liveness.py` (1,124 → 226), `build.py` (941 → 523) and
  `cache.py` (329 → 77).
- **A four-space prefix stopped meaning what it used to, and corrupted `--json`.**
  `devmap_engine.build_map` re-prints the kernel's discovery refusals to stderr — a refused
  file is absent from the graph, so swallowing that line makes "not in this repository"
  indistinguishable from "refused by the indexer". It selected them with
  `"discovery refused" in line or line.startswith("    ")`. That held until the build began
  running with `--progress always`: the kernel indents a progress line with **six** spaces
  (`main.rs:195,234,253`) and a refusal continuation with exactly **four**
  (`main.rs:1061,1064`), so the prefix test began matching the whole progress stream.
  `CliRunner` folds stderr into `result.output`, so every `dev … --json` command that
  triggers a map refresh emitted JSON followed by progress text — 7 tests across
  `test_cli_check_gate`, `test_cli_commands`, `test_cli_lease`, `test_cli_status` and
  `test_rigor_analytics`. Real terminals were unaffected (stderr is a separate stream), but
  any agent running `dev … --json 2>&1` got unparseable output. The block is now tracked by
  state (`iter_refusal_lines`) instead of by prefix.
  **The same predicate had a second, quieter victim.** `_is_note` fed the run record's
  `notes`, which is tail-capped at 40 — so once progress lines matched, they did not merely
  add noise, they *evicted the refusals the record exists to preserve*, while
  `stderr_tail` beside it already kept the raw tail. Both call sites now share one stateful
  classifier; a 200-line progress flood no longer pushes a refusal out of `notes`.
- **Verified, not assumed.** `ruff check src/` clean; all 400+ `devcouncil.*` modules import
  (a `pkgutil.walk_packages` sweep, since a deleted method is invisible to a linter); and on
  this repository `liveness_snapshot` still returns 20 entry roots / 18 unwired / 32
  unreachable / 10 dead symbols / 1,268 indexed symbols, `import_edges_for` 5,743 edges,
  `dependents_for` 469 keys — i.e. the ratchet's inputs survived. End to end, `dev map` wrote
  both `repo_map.json` and `code_graph.json` through the kernel and reported 20 entry roots
  and 179 dead symbols.

## Dev Map hardening, MCP/hooks/plugins integration, and adversarial pass (2026-09-04)

Branch `claude/dev-map-hardening-perf-1c392c`. One coordinator on the Rust kernel plus four
parallel Opus agents on disjoint file sets (MCP failure-honesty, MCP boundary/protocol, hooks,
and a read-only `devmap-extract` audit). Every fix ships a test **verified red against the
pre-fix tree**; where the honest shape required a new type, the red was reproduced a second time
by neutering the new logic, so the test pins behaviour and not the type's existence.

**Gates.** Rust workspace **849 passed / 0 failed**; `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets` clean; release binary rebuilt. Python suite results are
recorded with the individual lanes.

The full kernel record — every defect, its failing test, and the measurements — is
`rust-port/STATUS.md` → the three 2026-09-04 sections. Highlights:

### Correctness the agent-facing surfaces were getting wrong

- `repo_map.json` asserted `graph_degraded: false` unconditionally, which made
  `RepoMapper.map_is_stale`'s fail-closed branch **dead code** — `--if-stale`, `watch` and
  `verify` accepted maps built from a partition that never converged.
- `unwired_candidates` was a hardcoded `[]` in `repo_map.json` while `code_graph.json` computed
  it from the same inputs in the same build.
- `trace` reported *"no indexed path from X to Y"* when it had merely hit `--depth`; `impact`
  reported `truncated: false` on a walk that stopped early, on essentially every call.
- A public class was reported dead at **0.90 confidence in 21 languages**, because
  `generic_is_exported` read the declaration's whole subtree text and any `private ` inside a
  member decided the enclosing type's visibility.
- The MCP map/graph family returned confident empty successes for kernel-down, store-locked,
  mid-build and timeout; `graph_query`/`graph_trace` returned `ok: true` carrying an `error`.
- `devcouncil_liveness` reported "no dead code, analysis reliable" when nothing had run.

### Security

`projectPath` escaped the server's root on 16 MCP tools — reads, the debugger, and one **write**
(`devcouncil_code_sync` → `refresh_map_artifacts` into an arbitrary directory). Now contained at
one canonical resolver; `additionalProperties: false` applied across all 73 tool schemas after an
AST pass confirmed `create_planned_files` was the only undeclared key a handler read.

### Liveness and robustness

- The IPC daemon failed to start **~30% of runs under load** (`devmap-serve --lib` failed 6 of
  14) because `lock_ipc_endpoint` treated a transient `WouldBlock` as permanent, and reported an
  unusable lock file as *"owned by another live daemon"*. Fixed and measured: **0 failures in 15
  runs**.
- **The Dev Map had never refreshed inside a git worktree** — `--project-root` was baked to the
  main checkout and the worktree's own paths were dropped as a nested checkout. Live in this very
  checkout, not theoretical.
- A new adversarial sweep (35 language specs × 14 hostile inputs) found a **4 KB C++ file that
  took 199 seconds to extract**. The first diagnosis — a pathological parse — was wrong:
  instrumenting showed the parse completing in **2.98 ms**, with all the cost after it. Bounded
  by a single budget covering parse *and* walk, returning an explicit refusal rather than a
  truncated symbol set.
- **A six-byte COBOL file containing NUL bytes hangs the indexer forever.** NUL is now refused at
  the boundary for every grammar; the BOM variant is not coverable in-process. Recorded as a
  decision in `rust-port/STATUS.md`, with the sweep excluding COBOL *loudly* (printed, and the
  coverage assertion carries `covered + excluded == total`).

### Performance, measured

- `devcouncil_graph_context`: **688.5 ms → 0.83 ms** by deleting a subprocess that re-entered
  Python to call a function the handler already had three lines below as its fallback.
- `tools/list` shipped `ttlMs: 0` on every request; now a 300 s hint, justified by verifying the
  tool list is a pure function of the code and that no `list_changed` notification is ever sent.
- A repository containing the hostile `.cbl` file builds in **0.026 s** instead of never.
- Characterised but **not landed**: `impact`/`trace` cost ~100 ms flat regardless of `--depth`
  because `Store::latest_edges` rescans all 71,598 edges per request. A generation-keyed cache is
  the fix; it is held pending the repository's open decision on a peak-memory gate rather than
  landed without an RSS measurement.

### Integration

Hooks brought to the current 33-event specification (from ~9), adding `PostToolBatch` batching,
`FileChanged` + `SessionStart.watchPaths` for branch-switch refresh, and `CwdChanged` /
`DirectoryAdded`; `WorktreeCreate` was **declined** with a reason (registering it replaces Claude
Code's default worktree behaviour and any non-zero exit aborts creation). The plugin bundle now
passes `claude plugin validate --strict` for both the plugin and marketplace manifests — it did
not before. Hook timeouts in the plugin generator were **milliseconds where Claude Code expects
seconds** (10000 → 2.8 hours, 150000 → 41.7 hours); both generators now share one constant.

### Ledger corrections — three documented claims refuted by measurement

1. `dev map --pdg` losing `meta.map_engine` and tripping `doctor`'s `foreign_writer`: **does not
   reproduce.**
2. SC34's "roughly 26 of 35 languages have no call graph": the real figure is **12 of 35**.
3. "Plugin packaging has no owner": `build_plugin_bundle` already existed; what was missing was
   that nobody had ever run the validator against it.

### Follow-up in the same session: both open decisions closed, and the read path moved to the kernel

The three items left open above were closed rather than handed on.

**1. COBOL — closed by measuring what the grammar was worth.** It was framed as a human decision
between dropping the language, replacing the grammar, or moving extraction into a killable
subprocess. That framing was wrong: on a realistic COBOL program the grammar parsed `Clean` and
yielded **only the File node** — zero declarations, zero calls — and the bounded fallback scanner
recovers nothing either. There was no trade-off. `cobol` is now in `UNSAFE_GRAMMARS` and refused
before any parser sees the source, with a reason a maintainer cannot miss. Consequently the
adversarial sweep's exclusion list is **empty**: all 35 specs × 14 hostile inputs run in 6.3 s
with COBOL back under test exercising the refusal.

**2. The edge cache — closed by taking the measurement instead of citing its absence.** Withheld
above pending an RSS number. Measured, A/B on one settled store (gen 799, 14,189 nodes, 71,598
edges), same harness, warm daemon: `impact` median **96.2 ms → 38.5 ms (−60%)** for **+19.4 MB
(+4%)** of steady-state RSS. The memory intuition was backwards — the uncached daemon already
allocated a fresh 71,598-edge vector per query and did not return it (531.6 → 801.4 MB over 26
queries), so the cache replaces an unbounded series of transient allocations with one bounded
retained one.

Its invariant test was **first written vacuous and caught as such**: both generations of the
original fixture had 13 edges, so the length assertion passed with the generation key
deliberately disabled. The fixture now makes the second generation strictly larger, and the test
is red with the key off.

**3. `graph_query` / `graph_trace` now ask the Rust kernel first**, falling back to Python only
when the kernel declines — and naming which engine answered, because the two do not always
agree. Measured against a settled store over the CLI transport:

| tool | before | after | |
|---|---|---|---|
| `graph_trace` | 1014.6 ms | **379.2 ms** | −63% |
| `graph_query` | 1099.1 ms | **503.1 ms** | −54%, and `callees` now answered |

**`graph_query` did not improve from the routing alone, and the reason mattered.** Its kernel
path is a *composition* — one `search` plus `impact` + `deps` for each of the first five
definitions, so 9-11 kernel calls — and `DevMapClient` falls back to a `devmap` subprocess
whenever no daemon socket is live, making each one a process spawn. Rewriting the analysis in
Rust could not touch that; only removing the fan-out could.

So the fan-out was removed: a composed `neighbors` command answering both directions for several
targets in one exchange, implemented once in `StoreQueryEngine::neighbors` and exposed on both
transports (`IpcCommand::Neighbors`, `devmap neighbors`). Measured same-process, same store, same
binary, min-of-5, with the batch disabled to reproduce the old path exactly: **828.3 ms →
359.9 ms, 2.30x**, with **byte-identical payloads**.

That measurement then exposed a gap worth more than the speed: `callees` never carried anything.
For a symbol-shaped target the outbound side was always `Unavailable`, because `dependencies`
resolves a file path and a symbol id is not one — honest, and useless. `neighbors` now picks the
query by target shape (`latest_file`, the store's own notion of a file, not a `::` sniff): files
keep `dependencies`, symbols get the symbol-scoped forward traversal. Answering that field costs
real work, so the shipped end-to-end number is **1398.8 ms → 503.1 ms, 2.78x**, while taking definitions with a real
callee list from **0 to 4 of 6**. The equivalence is the load-bearing
assertion, not the timing — a faster query that answers something different is a regression with
a good benchmark — and it is verified red by swapping the two directions. The fan-out bound is
refused rather than trimmed (verified red against a trimming implementation), and a `devmap`
binary predating the command degrades to the slower per-target path rather than failing.

Two seams worth recording. First, `graph_trace`'s answers **change**: Python's BFS is undirected
over five edge kinds, the kernel's is directed over resolved edges, and the kernel is correct —
on a real probe Python reported a path between two functions through a shared test module where
the kernel correctly reported none. Second, the first version of this wiring passed `query=` where
the callee reads `kwargs["name_or_path"]`, which would have raised on every call while every
mocked test stayed green; `test_handlers_call_the_kernel_with_the_kwargs_it_actually_declares`
drives the real function to pin the keyword contract, and is red against that bug.

## `explore` and `affected_tests` move into the kernel; `codeintel/query/` deleted (2026-09-05)

Closes next step 4 of the 2026-09-02 handoff for the two MCP surfaces agents actually
call. `src/devcouncil/codeintel/query/` is gone (−364 lines); the kernel gained
`StoreQueryEngine::explore` and `::affected_tests`, a `devmap explore` / `devmap affected`
CLI pair, and the `explore` / `affected` IPC commands.

**The Python path was not merely slower — it could not answer.** `CodeIntelQueryEngine`
reads `.devcouncil/codeintel/index.sqlite`, and nothing has written that store since the
Python writer was retired on 2026-09-02. Measured on a worktree with a freshly built map:

```
$ python -c "CodeIntelQueryEngine(Path('.')).explore('budget_take', limit=3)"
FileNotFoundError no code-intelligence index; run `dev map init`
```

`devcouncil_code_explore` and `devcouncil_code_affected_tests` therefore answered
`codeintel_not_initialized` against a repository whose map was current. The store only
reappears as a *side effect* of `load_code_graph`, which imports `code_graph.json` under a
writer lease from tools an agent reads as read-only — so whether these tools worked
depended on whether some unrelated Python graph surface had run first.

**Measured, this repository (14,330 nodes / 74,061 edges), same query, CLI transport
with no daemon** — the worst transport the kernel uses, since each call pays a process
spawn:

| Surface | Python cold | Python warm | Rust cold | Rust warm | Speedup (warm) |
|---|---:|---:|---:|---:|---:|
| `explore budget_take` | 2.054 s | 1.163–1.470 s | 0.437 s | 0.274–0.279 s | **4.4×** |
| `affected budget_take` | 0.914 s | 0.902–0.918 s | 0.184 s | 0.124–0.129 s | **7.2×** |

Peak RSS: Python 185.7 MB in-process; Rust 22.9 MB in-process plus a 151 MB subprocess.
The wall-clock win is real; the memory win is not, on the subprocess transport.

**Equivalence, not just speed.** `affected_tests('budget_take')` returns the *identical*
11 test files from both engines — despite the stricter test-path predicate (Q5) — so the
paths the new rule drops were genuinely not tests on this corpus.

**Contract repairs shipped with the move** (`rust-port/DIVERGENCES.md` Q1–Q8): ranking
before truncation with an index-wide total; a snippet that could not be read reported as
unread rather than empty; per-direction edge counts that stay exact whatever the budget
buys; a blast radius banded by distance with measured confidence and an explicit
`walk_incomplete`; nearest-first affected tests with named unmatched targets.

**Retired, and not replaced:** `CodeIntelQueryEngine.dead`'s `runtime_proven_live`
suppression. Both dead-code surfaces (MCP `devcouncil_code_dead`, `dev graph dead`) had
already moved to the kernel, so it had no production caller — but the kernel does not
consult runtime observations, so a symbol a debug session proved live is still listed as a
dead candidate. The observation *merge* survives: it moved to
`CodeIntelService.load_with_runtime_observations`, which `run_cypher` reads.

**Still Python, and out of scope here** — surveyed with `file:line` evidence and left
alone because none has a kernel equivalent: MCP `devcouncil_graph_impact`, `route_map`,
`shape_check`, `api_impact`, `pdg_query`, `explain`, `graph_cypher`; CLI `check`,
`process`, `export`, `routes`, `shape-check`, `api-impact`, `explain`, `pdg`, `html`,
`view`; and the two surfaces that load the graph without any graph command being typed —
`execution/prompt_builder.py:573` (every task prompt build) and `knowledge/wiki.py:179`.
`indexing/graph/build.py`'s `load_code_graph` and the Python cache therefore stay.

## Dev Map hardening and performance pass (2026-09-05)

Six workstreams on disjoint file partitions. Every fix below ships with a test that was run
against the unmodified code and observed to fail; where a test already existed and encoded
the defect, its expectation was corrected with the reason recorded at the assertion site
rather than the test weakened.

**Gates at the end of the pass:** `cargo test --workspace` **1,003 passed / 0 failed**
(835 at the start); `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings`
clean; `go build`, `go vet`, `go test ./...` and `go test ./... -race` green across all
7 packages; Python unit suite green.

### Seam: `dev map` was running a 5.8x slower kernel, silently

`find_engine_binary` ranked capable candidates by mtime. Any `cargo test` writes
`rust-port/target/debug/devmap`, so every subsequent `dev map` selected the **unoptimized**
build. Measured, interleaved, same store and same argv:

| kernel `manifest` | run 1 | run 2 | run 3 |
|---|---|---|---|
| `target/debug` | 6.25 s | 6.36 s | 6.82 s |
| `target/release` | 0.54 s | 1.10 s | 1.79 s |

A ~0.9 s `dev map` became 8.5 s for anyone who had run the suite.

The docstring said age was a proxy for "built after the schema bump". The binary can be
*asked* instead: `devmap --db <path with no store> status` reports
`expected_schema_version`, exits 0, and creates nothing (verified). Selection is now
**schema, then the optimized build, then age**, degrading to the previous newest-wins rule
when no candidate can report a schema — preferring `release` on no evidence would resurrect
the bug the age rule was added to fix. Selecting a debug kernel logs a warning naming the
cost, once per selection.

Two of the four new tests fail behaviourally against the pre-fix code, and the second red is
its own finding: with the release build *newer*, the old rule chose it over a debug build
carrying a **higher** schema. The proxy was wrong in both directions. Probe cost: 51.4 ms
cold for 3 candidates, 0.4-0.6 ms memoised, against ~5.7 s saved. `dev map` end to end:
8.48-19.24 s before, **2.28-2.85 s** after, at comparable machine load.

### Extraction: embedded `<script>` blocks were a total blackout

`LanguageSpec.embedded` was declared and read by nothing. A `.svelte` / `.vue` / `.astro` /
`.liquid` file was parsed by its outer grammar only, and every one of those grammars hands
the script body back as a single opaque leaf. Measured before, on all four: **1 symbol** (the
`File` node), **0 imports**, **0 calls**, reported as `ParseOutcome::Clean` —
indistinguishable from a file that genuinely declares nothing.

`devmap-extract/src/embedded.rs` locates the regions, routes each to the language the
registry permits, and shifts every span into the outer file's byte coordinates — verified by
slicing the outer file with every emitted span, including after multi-byte text. 11 of 14 new
tests fail against the pre-fix code.

Measured on a real external Svelte project (GitPulse, 826 files): svelte **86 symbols -> 650**,
**0 calls -> 1,611**, with correct qualified names (`src/App.svelte::loadCoverageViewer`).

Vue additionally permits `tsx` (X38) because Vue's own compiler accepts
`<script lang="tsx">`. Svelte and Astro deliberately do not get it: Svelte's template is not
JSX and an Astro `<script>` is plain JS/TS.

### Extraction: the TS/JS import arm read function bodies as binding lists

The `"import_statement" | "export_statement"` arm found its bindings with `text.find('{')`
over the *whole statement text*. For `export function helper(n) { if (n > 0) { return n; }
return 0; }` that takes the function body's brace and the `if` block's close:

    IMPORT module="" names=["if"]
    IMPORT module="" names=["return", "n", "0);"]
    IMPORT module="" names=["NAME"]        <- from a correct `export { NAME as ALIAS };`

Neither `""` as a module nor `if` as an imported name is a value a valid program can produce,
so every consumer joining on them gets an endpoint no resolver can bind — the SC26/SC32
whole-expression shape, in the import emitter instead of the call emitter.

Now read off the parse tree (`export_clause` / `named_imports` specifiers), with the import
push gated on the statement actually naming a module. Six tests, four red against the pre-fix
code. `each_grammar_extracts_its_exact_imports_and_exports` had **pinned the defect** — it
expected `("", ["x"], ["x"], None)`. Expectation corrected with the reason at the assertion
site, and its own guard strengthened: the loop already refused an empty binding *name* and
was one field short of the class it was written for.

### Extraction: six languages gained a call graph, two were declined

`erlang`, `pascal`, `solidity`, `shell`, `sql` and `nix` had a linked grammar and no
`calls.push` site, so `impact`, `trace`, dead-code and the PDG answered from an empty call
graph with nothing separating "no callers" from "callers were never extracted" (SC34). Each
now has a module under `langcalls/`, callers attributed through
`langcalls::scope::enclosing_emitted_symbol`, and **orphaned call edges = 0**.

Two were **declined, and pinned by tests that fail if someone adds them**:

* `hcl` — Terraform/OpenTofu has no user-defined function syntax. All five `function_call`
  nodes in a realistic `.tf` name built-ins, so no edge could ever resolve. The real graph
  there is `local.x` / `var.y` / `module.m.out`, already emitted as references.
* `cfml` — script-syntax `.cfc` parses to `(program (component_file (cf_component_content)))`
  and `<cfscript>` to one opaque node. Script syntax is the dominant modern dialect, so
  shipping this would claim coverage while seeing nothing in most real files.

A stated "this language has no call graph, and here is why" is worth more than a fabricated
one. Found in passing and fixed: `generic_symbol_kind` mapped tree-sitter-erlang's `module`
node to `SymbolKind::Module`, but that node is the module *half* of `fun other:f/2`, so
`t.erl` emitted a phantom `t.erl::other` declaration.

### Resolution: four languages fell to `Generic` and produced a confident wrong edge

`LangFamily::from_lang` had no arm for `svelte`/`vue`/`astro`/`liquid`. Harmless while they
contributed zero calls; not harmless once embedded extraction landed. Measured against the
real binary:
`Widget.svelte::renderWidget --Calls 0.9--> Vault.sol::Vault.helperOnlyInSolidity` — a Svelte
function bound to a Solidity contract method — while the same file's call to a real
`helpers.ts` export produced **no edge at all**. Isolated A/B on this repository: **-9
fabricated cross-family edges, +0 lost**. `LangFamily::admits` now makes `Generic` inert, so
the class is closed structurally rather than by four string additions.

### Storage, resolution and analysis: ten defects under adversarial load

| attack | what broke | after |
|---|---|---|
| NaN confidence threshold | `callers_of` answered `Ok(Some(0))` — "nothing calls this", from a filter that never ran | all three siblings refuse |
| 40k-name caller batch | `too many SQL variables`, naming neither caller nor limit | chunked at 512, **not capped** — a dropped name reads as "nothing depends on this" |
| NUL byte in a search query | `unterminated string`: SQLite hands MATCH to FTS5 as a C string. The escape existed in **three drifted copies** | one `fts_match_query`, refuses NUL by name |
| open a `user_version=2` store | the file was **mutated before being refused** — `enable_wal` ran ahead of `migrate` | `schema_is_migratable()` consulted before any write; file byte-identical after refusal |
| 40k-segment `use crate::a::a::…` | **124.468 s** for one import in one file | 5.7 s for three prefixes |
| 200k-deep PDG statement tree | **stack overflow, SIGABRT** — not an error a `Result` function may produce | refused by name at depth 256 |
| 64-node walk over a 500k-edge graph | `adj` deep-cloned every edge *before* `max_nodes` was consulted | 661.7 -> 104.8 B/edge |
| query plan of the SC8 cache fallback | `SCAN generation_files` — no index on the cache identity, and the cache is drained to 0 rows every build, so **every** file fell through the scan on **every** build | schema v12->v13; real binary, 8,001 files, no-op build **2.619 s -> 0.724 s** |

Attacks that found nothing, with the tests kept: racing openers, a store replaced under an
open handle, a writer `SIGKILL`ed mid-transaction, hostile identifiers round-tripping, a
20k-deep `super::` chain, path traversal as a JS import, determinism across runs.

Three places were fixed as a *shape* rather than an instance: `fts_match_query` replaced
three drifted copies of the escape; `LangFamily::admits` replaced `*candidate_family ==
family` in four places; `schema_is_migratable` became the single owner of "can this binary
touch this store", consulted by both `open` and `migrate`.

### Artifact: an interned encoding for `code_graph.json` (X39 / PLAN.md §3 `G6`)

Measured by the Rust encoder on this repository (1,331 files):

| | verbose | interned | |
|---|---|---|---|
| size | 21,186,034 B | 5,001,998 B | **-76.4%** |
| `json.loads`, median of 5 | 85.2 ms | 46.2 ms | -46% |

`source` + `target` alone were 52.6% of the verbose file: 14,324 distinct endpoint strings
written 147,726 times. Opt-in via `devmap manifest --compact-graph-output`; the verbose
artifact stays canonical and is written either way.

Both encodings render from one `build_code_graph_value` traversal, so a dropped field is
impossible by construction, and `decode_compact(encode_compact(v)) == v` is the test —
red-demonstrated by making the encoder skip one column. A table whose rows are not uniformly
shaped is carried through verbatim and named in `verbatim_tables` rather than interned
against a spec taken from the first row.

**It does not make the graph agent-readable.** 5.3M tokens to 1.25M is still unopenable. It
buys bytes, parse time and disk churn; `devmap search` / `impact` / `trace` remain the way to
read the graph. Its first consumer landed the same day: `backend/go_orchestrator/repomap`
reads both wires from one `Load`, and on a 4,499-file corpus that is 405.0 ms and 242.6 MiB
against 114.9 ms and 112.0 MiB for the identical `Map`.

### Provenance: one name, two counts

`edge_endpoints_without_node` counts **edges** with a dangling endpoint — the contract
`repomap.go` decodes into `OrphanEndpoints`, documented there correctly. The key's *name*
reads as a count of endpoints, and on this repository the two differ by more than 2x: 360
edges naming 167 distinct absent identities. Renaming would break the consumer that already
reads it right, so `distinct_edge_endpoints_without_node` travels beside it. The ratio is the
diagnostic: many distinct means files the index never read, few means one symbol the
extractor failed on.

### Go: ten defects at the artifact boundary

`repomap.Load` did `os.ReadFile` with **no bound** — the only unbounded payload in a package
that bounds all six others (67 MB transient for 0.6 MB retained on the real artifact).
`DisagreementsWith(0, 0)` returned `nil`, "checked and agree", when **neither comparison
ran**. Two bounds were off by one and discarded a *complete* answer (`room > len(p)` for a
stream that exactly filled the cap). `Manifest` reported success when the producer exited 0
having written an unparseable artifact. `AreaForPath` on an unindexed path rescanned every
file per ancestor level: **29,968 -> 190 ns/op**.

`go test ./... -race` now runs in CI. It is not a speculative gate: the concurrency work in
this pass landed a data race in `Provenance()` that plain `go test` was green on, and the
detector was the only thing that saw it. One test had to be desensitised first — a 150 ms
probe timeout raced the OS's ability to fork `/bin/sh` under whole-module load, passing 10 of
10 in isolation and failing when all seven packages ran at once.

### Python: importing six exception classes loaded SQLAlchemy

`devcouncil/app/errors.py` is six `class X(Exception): pass` and imports nothing — 163 us to
execute. Importing it cost **304 ms** over a bare interpreter, because
`devcouncil/app/__init__.py` eagerly reached `Orchestrator` -> `storage.db` -> SQLAlchemy and
SQLModel: 284.8 ms of ORM an exception class has no use for. Now PEP 562 lazy: **304 ms ->
4.5 ms**. The facade is kept rather than deleted — one static caller is weak evidence for
deletion and none at all for reflective paths. `TYPE_CHECKING` preserves the names for mypy,
which does not run `__getattr__`.

### Tests: 39 assertions conflated stdout with stderr

`json.loads(result.output)` appeared at 39 call sites across 14 files. Click's `result.output`
is stdout **and** stderr combined, so each of those asserted "the `--json` surface emits JSON
*and nothing logs a warning*" — and a warning wrongly written to **stdout** would have been
indistinguishable from one correctly written to stderr. All 39 now read `result.stdout`,
which is both narrower and stricter. Found because a new, correct warning broke two of them.

### What was measured and deliberately not changed

* **Ambiguous-edge sort** carries an O(candidates) tie-break (`format!("{:?}", …)` over the
  whole candidate list): 4-5x for 2x candidates, 5.4% of a cold build. That comparator defines
  the total order edge ordinals come from, and the last change to it was validated with an
  edge-ordinal digest over 4,742 files. Bounded by a regression test instead.
* **`extraction_json` is 60% of the store** (64.2 MB of 106 MB at one generation) and it is
  **the only copy** — `extraction_cache` holds 0 rows after every build, so deleting it makes
  every incremental build a full re-parse. Not redundancy; it *is* the cache.
* **Store growth is bounded and flat**: 110.6 MB at one generation -> 222.0 MB, then dead flat
  over 25 rounds (plateau spread 0.1%), ratio 2.01x = `GENERATION_RETENTION`. Freelist 0.0%
  after every one of 25 builds.
* **`dev map` and `devmap build` do not leak free pages.** A 306 MB / 33%-free store was
  observed once during this session and **could not be reproduced**: 25 consecutive
  `devmap build` rounds and 6 `dev map` cycles with real file churn both hold the freelist at
  0. Recorded as observed-once, not as a defect.

### Python: `dev <anything>` paid for all 72 commands

`cli/main.py` eagerly imported ~50 command modules and registered 75 commands at import
time, so `dev version` loaded the ORM, the MCP handlers and the graph adapters before
printing a string. Registration is now deferred through a `TyperGroup` subclass that resolves
a command the first time Click asks for it by name; `list_commands` still enumerates all 72,
so `--help`, completion and `dev <typo>` suggestions are unchanged. `hook.py` and `map.py`
additionally moved their `storage.db` / `CodeReviewGraphAdapter` imports to their call sites.

Measured on this machine, same interpreter, minimum of three `-X importtime` runs:
**392 ms -> 21 ms** for `import devcouncil.cli.main`. That baseline is conservative — it is
HEAD's eager `main.py` measured against a tree that *already* has the lazy
`app/__init__.py` facade, so the pre-pass figure was higher.

`tests/unit/test_cli_lazy_commands.py` pins all three properties: the 72-command surface is
compared name-by-name against a list captured before the change, the group resolves each
command on demand, and `dev version` completes without `sqlalchemy` or `devcouncil.storage.db`
entering `sys.modules`. The `dev hook` case is deliberately *not* asserted — `active_task_id`
opens the database as its first act, so the ORM is genuinely required there.

### `preview` built 1,836 rows to report one integer

`ambiguous_callers` is "and M more the floor excluded". It came from
`callers_of(&at_risk, path, 0.0)?.len()` — the same query the confident list had just run,
re-run at floor 0.0, fully materialised into `StoredEdge` (six `String` allocations a row),
and reduced to a `usize`. On this repository the busiest symbol has 918 callers against an
average of 9 over 5,673 symbols, so previewing a file that declares a hot symbol built
~1,836 rows and kept none.

`Store::count_callers_of` issues `SELECT COUNT(*)` over the identical `WHERE` clause, with
the same `checked_min_confidence` guard, the same `BTreeSet` de-duplication and the same
`MAX_CALLER_BATCH` chunking. Because a count and a listing that drift are indistinguishable
from a correct answer, the test pins them *against each other* rather than against a
hand-computed number, across four floors and a name list larger than one chunk. Removing the
`sp.path <> ?2` filter from the count alone makes it fail (40 vs 0), which is the drift it
exists to catch.

### The most frequent build reported no timings

A no-source-change build is what a watcher does on almost every tick, and `--json` answered
it with a hand-written format string: `{"unchanged":true,"files":…,"generation":…,
"reclaim":…}`. No `timings` key of any kind — so the one build shape a profiler most wants
to look at was the one it could not see, and "the warm path is fast" was an assertion nobody
could check from the tool's own output. The branch is not free: it hashes every file in the
tree to *prove* nothing changed, and it runs the reclaim decision. Both are already timed
stages; only the reporting was missing. It now goes through `emit_json` with
`progress.timings_json()`, like every other build result.

### Benchmarks after the merge (2026-09-05, `6f7716f`)

`benchmarks/map_bench.py --repeat 7`, release binary, this repository (1,053 code files).
The machine was **not quiet** — load averaged 10-14 with other agent sessions running — so
`touch` (91%), `manifest` (143%) and `impact` (85%) exceeded the harness's spread gate and
their minima are not comparable to a quiet baseline. Reported rather than dropped, because a
missing row and a noisy row are different things.

| stage | min | median | spread | peak RSS | throughput |
|---|---|---|---|---|---|
| `cold` | 2.53s | 2.65s | 31% | 592 MiB | 417 files/s |
| `warm` | 264ms | 269ms | 24% | 150 MiB | — |
| `e2e` | 1.24s | 1.29s | 15% | — | — |
| `search` | 10ms | 11ms | 12% | 13 MiB | — |
| `path` | 101ms | 125ms | 30% | 119 MiB | — |
| `dead` | 16ms | 17ms | 16% | 19 MiB | — |

**The synthetic sweep is the comparable measurement**, because the corpus is fixed and the
stage is the one a watcher actually runs. `--synthetic N --repeat 5`, minimum reported:

| files | cold | warm (no-op) | touch | manifest | throughput | `code_graph.json` |
|---|---|---|---|---|---|---|
| 2,001 | 657ms | 110ms | 412ms | 219ms | 3,047 files/s | 11.05 MB |
| 8,001 | 3.37s | 444ms | 1.79s | 720ms | 2,375 files/s | 44.49 MB |

**4.0x the no-op build time for 4.0x the files** — linear in corpus size, which is the shape
the v13 cache-identity index was added to produce. Cold is 5.1x over the same pair and
throughput falls only 22%. The prior note in `schema.rs` claimed "~2.9x"; that figure did not
follow from its own table and has been replaced with this one.

**Store growth is bounded, re-confirmed.** Ten rounds with one file's content changed between
builds, so every round is a real new generation: 128.3 MB -> 257.7 MB = **2.01x**, exactly
`GENERATION_RETENTION`, with a **0.0% spread over the second half** and a **0.0% freelist**
after the last build (0.1% peak), read from the file rather than from the vacuum's own report.
This is the second independent measurement failing to reproduce the 306 MB / 33%-free store
observed once earlier; it stays recorded as observed-once, not as a defect.

### Still open

* ~~`traverse_graph` rebuilds its index per query.~~ **Closed 2026-09-05** (`1dcbc69`) — the
  index is now built once per *generation* and cached on the `Store`
  (`devmap-store/src/edge_index.rs`, `Store::edge_index`), not once per question; the
  in-memory and store-backed walks are the one implementation, `traverse_graph_indexed`, and
  `traverse_graph` is the thin single-use wrapper for callers holding a loose edge slice.
  Measured p50 on two settled corpora: `impact` 18.62 ms -> 947 µs (15,080 nodes / 74,729
  edges) and 73.06 ms -> 2.82 ms (41,276 / 271,508); `status` 774 µs -> 13.6 µs and
  2.689 ms -> 12.8 µs. See STATUS.md → "Port of the 1a2151 round-1 work onto main (2026-09-05)".
* ~~`devmap --version` reports `schema 13` while the artifact declares `schema_version: 2`.~~
  **Closed 2026-09-05** (`d6ac670`) — the line reads `devmap 0.1.0 (store schema 13, code
  graph schema 2)`, naming both numbers instead of one. The store number stays first on
  purpose: `devmap_health.binary_info` takes the first digit group after the word `schema`,
  and that is the number that decides whether a kernel can open a store.
* ~~Nothing reads the interned `code_graph` encoding.~~ **Closed 2026-09-05** —
  `backend/go_orchestrator/repomap` reads both wires from one `Load`, dispatching on the shape
  of each table so `build` is unchanged and the two encodings cannot answer differently.
  Measured on a 4,499-file corpus, both artifacts written from one generation: 85,001,360 B ->
  17,104,715 B, `Load` 405.0 ms -> 114.9 ms (3.5x), 242.6 MiB -> 112.0 MiB allocated, 1,291,942
  -> 79,699 allocations. The verbose wire pays +3,009 B and +41 allocations per load for the
  shape dispatch. Equivalence is asserted against the **real** producer
  (`dc/devmap/interop_test.go::TestTheLiveCompactGraphBuildsTheSameMap`, which CI already
  fails on if it skips), not against a fixture. See STATUS.md -> "Go: the compact graph has a
  reader".
* `repomap` and `devmap.Client` have no caller **in this repository** — no `main` package, and
  the only importer is a test. They are not dead: MANVI holds its own copies at
  `manvi/repomap` and `manvi/dc/devmap` with five production call sites, and the copy here is
  the upstream — 809 lines against MANVI's 584, and the only one carrying `MaxGraphBytes`,
  `SchemaDeclared`, `DistinctOrphanEndpoints` and the interned decoder. The register entry is
  therefore "port the accumulated fixes across", not "delete". Two proofs and the Chesterton's
  fence are in STATUS.md.
* **New, and load-bearing:** `devmap-query/src/manifest.rs:302` writes `"neighbors": []` as a
  literal, and `src/devcouncil/indexing/subsystem_map.py::are_neighbors` reads that field. The
  "allow a write into a neighbouring subsystem" rung in `execution/policy_engine.py:628` can
  therefore never fire, and `verification/checks/subsystem_boundary.py` flags every cross-area
  change as drift. Measured: 0 of this repository's 16 subsystems and 0 of the scholarlm map's
  12 carry a non-empty `neighbors`. See STATUS.md for the two candidate owners.

## Python typing: mypy to zero (2026-09-05)

CI runs `uv run --python 3.12 mypy src` as a hard gate (`.github/workflows/ci.yml`), and the
tree had drifted to **44 errors across 23 files**. They were not 44 problems. They were six
shapes, each with an owner, and the count is what a per-site suppression pass would have
hidden. `mypy src` now reports `Success: no issues found in 387 source files`; `ruff check
src tests` is clean; no `# type: ignore` was added, and nothing was widened to `Any`.

**No behaviour was changed anywhere.** Five of the six classes were purely a type the code
had outgrown. The sixth turned out to close a real fail-open, described below.

### The six classes, and where each was fixed

1. **`Tool(inputSchema=...)` / `Resource(mimeType=...)` — 18 sites.** `0a6b97d` had already
   moved `handlers/tool_specs.py` off the camelCase wire aliases (mypy rejects them; the SDK
   accepts both via `populate_by_name`), but the three handlers that build their own literals
   were left behind. All now use the field names, so the package has one spelling rather than
   two that happen to agree. Serialization is `by_alias`, so the wire is untouched — pinned by
   `test_mcp_contract.py::test_every_advertised_tool_serialises_its_schema_as_inputschema`
   and its resource twin, which are discriminating: the same assertion over
   `model_dump(by_alias=False)` names all 73 tools.

2. **Task-status strings into a `Literal` — 7 sites, one expression.** Every one was
   `task.status = verification_task_status(...)`, and that producer returned a bare `str`. It
   now declares the three statuses it can actually produce, and `Task.status` names its
   vocabulary as `domain.task.TaskStatus`. One of the seven was in another lane's file
   (`cli/commands/hook.py`); fixing the owner closed it without touching it.
   `campaign/orchestrator.py` was the same shape one layer out — `TaskOutcome.status` was a
   `str` with its vocabulary written as a trailing comment, now `CampaignStatus`. Not a logic
   defect: "failed" vs "blocked" is the real distinction between an executor that produced
   nothing and a gate that refused, and `_coordinator_rollup` already counts them together.

3. **Gate mode — 4 errors, 12 copies, and a fail-open.** Eleven modules had written the
   resolution out twelve times, in one of two hand-rolled shapes. `gating.policy` already owned `GateMode` and
   all three consumers, so it now owns the resolution: `gate_mode(project_root)` and
   `gate_mode_of(config)`, the first defined in terms of the second, both narrowing at the
   boundary. That narrowing is a real hardening. `apply_gate_enforcement` branches on
   `mode == "enforce"` and demotes every non-safety blocker otherwise, so **any value it did
   not recognise read as "relax the gates"** — with `gates.mode = "Off"` a blocking gap came
   back `blocking=False`. Unreachable through a validated config, reachable through the two
   sites that read an already-loaded config object. `cli/commands/status.py` and
   `cli/commands/report.py` keep their direct reads: they deliberately have no fallback, and
   routing them through the owner would turn a broken config from a visible error into a
   silent `enforce`.

4. **`HookDecision` vs `PolicyDecision` — 2 seams.** Not merged: `HookDecision` is narrower on
   purpose, because a hook-synthesised verdict ("hook_gate mode=off allows this") fired no
   rule, and `PolicyDecision` derives `severity` from `rule` with an unknown rule failing
   closed to "hard" — so giving such a verdict a `PolicyDecision` would put a fabricated
   severity into the vocabulary `contracts/verdict.schema.json` shares with Manvi. The seam is
   named instead: `policy_engine.Decision`, a protocol of the three members a gate reads, and
   `PolicyDecision` gains the `allowed` property `HookDecision` already had.
   `is_command_allowed` keeps its asymmetry (`action == "allow"` on one branch, `.allowed` on
   the other) — those are different predicates, and reconciling them is a policy decision.

5. **Nine optionals.** Every one already survived a `None` at runtime, so each was fixed by
   making the code say what it does. Three were a local that meant two things (a 300-line
   function in `integrations/check.py` used `status` for a config row and, 200 lines later,
   for hook-file integrity); two were an annotation the value had outgrown; two were one
   imprecise signature (`effective_live_review` returns `None` only for the `None` it was
   given — now two overloads); and the last two were one duplicated unwrap. `dev map dead`
   merges rows from the Rust kernel (confidence is whatever JSON held, possibly absent) with
   entries from the Python graph (a `Confidence` enum) and unwrapped that inline in the filter
   and again in the printer, right next to `confidence_at_least`, which already owned the same
   unwrap. Now `liveness.confidence_label`, used by all three, verified byte-identical to both
   old expressions across enum / str / "" / None / int.

6. **The lazy CLI group.** `LazyCommandGroup.list_commands` and `.get_command` were annotated
   `typer.Context`; Click passes a `click.Context` and Typer's is a subclass, so the overrides
   narrowed a parameter the caller never promised — a Liskov violation on two supertypes at
   once. `get_command` also bound its local from the first branch (`get_group` returns a
   `TyperGroup`), so the leaf from the other branch did not fit; both are `click.Command`.
   `_LAZY_COMMANDS`'s `kind` is now `Literal["typer", "command"]`, so a typo in one of the 75
   rows is a type error rather than a command that silently resolves to nothing.

### What the tests pin

Five tests, each written against the unmodified code and watched fail:

| test | pre-fix failure |
|---|---|
| `test_verifier.py::test_verification_task_status_only_produces_task_statuses` | `assert set() == {'blocked', 'done', 'verified'}` (`get_args(str)` is empty) |
| `test_gate_policy.py::test_a_posture_outside_the_vocabulary_does_not_disable_enforcement` | `assert [False] == [True]` — the fail-open above |
| `test_task_policy_engine.py::test_both_engines_agree_on_what_allowed_means` | `AttributeError: 'PolicyDecision' object has no attribute 'allowed'` |
| `test_mcp_contract.py` (2 tests) | contract pins, not bug repros: labelled as such |
| `test_liveness_confidence_hardening.py::test_confidence_label_reads_both_producers_the_same_way` | `ImportError` — the shared owner did not exist |

### Still open

* ~~`[tool.mypy]` sets neither `disallow_untyped_defs` nor `check_untyped_defs`, so mypy
  skipped the bodies of every untyped function.~~ **Half closed 2026-09-05** (`203879e`) —
  `check_untyped_defs` was measured at two errors and is now on (lease de-duplication's
  untyped lists; the MCP freshness closure calling `.status()` on a `DevMapClient | None`).
  `disallow_untyped_defs` was measured at **337 errors in 100 files** and stays a decision for
  the maintainer; `tests/` is still unchecked.

## Session guard: live siblings and divergent branches (2026-09-05)

Twice in one day this repository was worked on by two agent sessions at once and neither knew.
The first time, one session edited the main checkout and committed to `main` while another
worked in a linked worktree on a branch; both merged the same unmerged branch and both fixed
the same audit findings with different code, which then had to be reconciled by hand. The
second time, a previous day's branch simply sat unmerged with nothing at session start to say
so. In both cases the information was sitting in `git worktree list` and `git branch
--no-merged` the whole time — nobody was asked.

`devcouncil/utils/git_siblings.py` asks. It answers three questions about the repository root,
each independently so one failure cannot silence the others:

* **live siblings** — every checkout in `git worktree list --porcelain` other than this one
  that has uncommitted changes, or that something touched inside the activity window
  (30 minutes; `DEVCOUNCIL_SIBLING_WINDOW_MINUTES`), reported with path, branch and age;
* **divergent branches** — local branches under the watched prefixes (`claude/`;
  `DEVCOUNCIL_SIBLING_BRANCH_PREFIXES`) that are ahead of the default branch, with the count
  and the age of the tip;
* **behind** — commits on the default branch (`main`, else `master`;
  `DEVCOUNCIL_DEFAULT_BRANCH`) that this checkout does not have.

The answer reaches a session in the one place a session already looks first: the
`Continuity — …` clause of the SessionStart status line. `_with_continuity` in `hook.py` is now
the only thing that writes that sentence, so the new clauses fold into the existing ones rather
than starting a second one. `dev map doctor` carries the same finding as a non-critical
`siblings` check.

**Honesty.** A probe that could not run reports *why* — `could not check: …` in the hint,
`ok: null` in the doctor — and never the "none" a probe that ran and found nothing reports.
A directory that is not a repository is an *answer*, not a failure, and `proc.git_repo_state`
already owned that distinction. Every listing that hits a cap says so (`partial: only the
first 20 matching branches were counted`).

**Cost.** SessionStart is the only caller: UserPromptSubmit, SubagentStart and the statusline
keep the cheap `_status_line`, and PostToolUse — the hook that fires most — never sees it.
Three tiers, cheapest decisive signal first: the mtimes of a checkout's git metadata (stat
calls, no subprocess) decide most checkouts; `git status --no-optional-locks` runs only for a
checkout that has been quiet; the tracked-file walk only after that, and the last two start
together rather than in sequence. `git` costs ~15 ms per spawn on this machine before it does
any work, so the three top-level probes are issued at once.

Measured on this repository, and the number depends on how many checkouts tier 1 can decide,
so both ends are here: with two other checkouts, both recently active (tier 1 decides them, no
subprocess per sibling), **p50 45 ms / p95 65 ms**; with three, two of them quiet and clean so
they pay `git status` *and* the tracked-file walk, **p50 103 ms / p95 118 ms**. Forcing the
window to zero so every tier runs for every sibling: p50 137 ms. Computing the same answer
serially, before the tiering and the concurrency, was 146 ms for the two-sibling case and
177 ms for the all-tiers case. So the cheap case got 3x cheaper and the expensive case is the
one to watch: it scales with the number of *quiet* checkouts, not with how many there are.

`--no-optional-locks` is not incidental: a plain `git status` refreshes and rewrites the index
it reads, and that index belongs to the other session's working tree. A guard must observe the
tree it warns about, never perturb it.

Per-call timeouts do not bound the guard on their own — eight checkouts on a stuck filesystem
are sixteen git calls, four workers deep, at five seconds each. `_Budget` is one shared
deadline every git call takes a slice of, so the whole guard is bounded at 8 s: a probe that
starts after the budget is gone reports `session guard budget exhausted (8s)` instead of the
empty answer a probe that ran and found nothing would give, and whatever was answered inside
the budget is still reported.

### The neighbour rule was answering from a field the kernel stubs (2026-09-05)

Found by the Go/`repomap` review (`rust-port/STATUS.md`, "Left open, for the Python and Rust
lanes") and fixed on the Python side here. `devmap-query/src/manifest.rs:302` writes
`"neighbors": []` as a literal for every subsystem, and the kernel has been the only map
writer since the Python one was retired on 2026-09-02, so the field is always empty.
`indexing/subsystem_map.are_neighbors` read it and returned `False` — reporting a measured
non-adjacency for a relation nothing ever measured. **Measured on this repository's map: 16
subsystems, 0 with a non-empty `neighbors`, no `meta`; every distinct pair answered `False`.**

Two consumers were acting on that:

* `execution/policy_engine.py` denied every unplanned cross-subsystem write with "not a
  declared neighbor". The neighbour rung one line above — allow a write into a *neighbouring*
  subsystem of a planned file — could never fire.
* `verification/checks/subsystem_boundary.py` raised an `architecture_drift` gap for every
  cross-area change, because `cross_boundary_pairs` read "no declared neighbours" as "not
  adjacent".

`are_neighbors` is now a tri-state — `True` / `False` / `None` for unknown — which is the
shape and the reasoning `is_entry_root` two functions below already used for a capped list: a
negative is only evidence when the producer was in a position to give one. `neighbors_established`
decides, from two independent positive claims: a non-empty `neighbors` list anywhere (a map
that names one adjacency demonstrably computed them), or the producer's own marker. Positive
answers need no evidence and stay definite, so every existing test kept its exact assertions.

The consumers were changed to match, and neither loosens anything:

* The policy engine **still denies** — an unmeasured relation is not permission, and no write
  that was refused before is allowed now — but the reason says the relation "is not established
  by this map" rather than sending an agent to widen its scope over a fact nothing established.
* The boundary gate reports a new `architecture_check_unavailable` gap (low, never blocking)
  instead of returning `[]`, which would have been its "ran and found nothing". It stays
  non-blocking even when the gate is configured `blocking=True`: that flag is about an
  undeclared crossing, and halting the loop over the map writer's missing feature would stop
  every repository the kernel maps rather than the ones with a boundary problem.

The two MCP surfaces that forward the field were the third and fourth readers, and the full
suite found them: `devcouncil_impact` emitted `cross_boundary_pairs: []` whether the pairs were
checked or not, and `devcouncil_map`'s subsystem detail forwarded a raw `neighbors: []`. Both
now carry the companion fact — `cross_boundary_checked` and `neighbors_computed` — the same
shape as `is_entry_root` and `walk_incomplete` already in that handler. Both keys are additive.
`test_impact_cross_boundary_force` had encoded the old semantics in its fixture ("force a
cross-boundary by removing neighbor links"); its assertion is unchanged and its map now carries
the producer marker, so the empty lists are an answer rather than a field nobody computed.

**The two lanes converged on the key.** The Rust writer's fix landed on this branch as
`696e1b8` while this was being written, and it emits exactly the marker this reader had been
told to assume: `meta.devmap_rust.neighbors_computed`. Its counts, though, brought a second
Class A case with them. Each area's `neighbors` list is capped
(`liveness_meta.subsystems.neighbors_truncated`, with `neighbors_shown`/`neighbors_total`), and
couplings whose endpoint could not be placed in any area are counted separately
(`neighbors_endpoints_unresolved`) — so on such a map a *negative* is a capped sample answering
as a complete one, the same trap `is_entry_root` two functions below already documents.
`are_neighbors` now degrades the negative to unknown when the map makes either claim. The
positive is untouched in both cases: a listed neighbour is listed whatever the cap dropped.

That distinction has one owner, because otherwise it grows a second: `can_rule_out_adjacency`
is the question every consumer of a *negative* is really asking (computed **and** complete),
and the boundary gate and `cross_boundary_checked` both ask it. `neighbors_established` stays
as the narrower provenance question — did the producer compute the field at all — which is
what `devcouncil_map`'s `neighbors_computed` row reports and what the kernel's own marker
means. Asking only the narrow one at a consumer would have pushed the same defect one level
up: a map that derived neighbours and capped them would have reported a clean check off a
relation it holds two entries of.
