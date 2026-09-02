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

### Defect found and fixed while building clone detection### Defect found and fixed while building clone detection

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
