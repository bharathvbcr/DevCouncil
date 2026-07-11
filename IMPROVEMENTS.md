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
