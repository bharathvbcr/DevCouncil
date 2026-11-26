# Research & Overlap Audit: 003 - Stack Boundaries and Non-Overlap

## 1. Evidence Methodology

All boundaries and overlap entries in [`spec.md`](spec.md) and [`contracts/ownership-matrix.json`](contracts/ownership-matrix.json) were verified on `2026-11-26` via DevMap symbol search (`devmap search` across `DevCouncil`, `Manvi`, and `GitPulse`) and direct source inspection. Historical notes in [`docs/STACK.md`](../../docs/STACK.md) were cross-checked against live source files before recording any path or symbol.

## 2. Verified Findings Across the Stack

1. **`devmap` MCP Surface Parity (`21` upstream vs `11` Waylode vs `3` Manvi)**:
   - Upstream `rust/devmap-serve/src/mcp.rs` declares `TOOLS` (`TOOL_NAMES`) containing `21` tools: `devmap_status`, `devmap_search`, `devmap_ask`, `devmap_ask_evidence`, `devmap_dependencies`, `devmap_impact`, `devmap_trace`, `devmap_neighbors`, `devmap_dead_symbols`, `devmap_skeleton`, `devmap_clones`, `devmap_preview`, `devmap_explore`, `devmap_literals`, `devmap_doc_drift`, `devmap_affected_tests`, `devmap_suspects`, `devmap_blast`, `devmap_routes`, `devmap_api_impact`, and `devmap_cypher`.
   - Waylode deliberately omits `devmap-serve` (to avoid `hyper`/HTTP transport deps) and serves a `11`-tool subset in `"src-tauri/src/mcp/devmap_parity.rs"` (`devmap_status`, `devmap_search`, `devmap_dependencies`, `devmap_impact`, `devmap_trace`, `devmap_neighbors`, `devmap_dead_symbols`, `devmap_clones`, `devmap_preview`, `devmap_explore`, `devmap_affected_tests`), enforced against upstream `TOOL_NAMES` by `"scripts/check-schema-parity.mjs"`.
   - Manvi spawns `devmap mcp` over stdio via `backend/go_orchestrator/dc/devmap/warm.go` (`"manvi/serve/devmap.go"`) and exposes 3 simplified harness-named graph tools (`devcouncil_graph_query`, `devcouncil_code_dead`, `devcouncil_graph_context`) in `"manvi/devcouncil/navigate.go"`.
2. **Verification Rigor Overlap (`dcverify` vs `work.verify.task`)**:
   - `DevCouncil` owns deterministic rigor verification (`rust/dc-verify/` and `backend/go_orchestrator/devcouncil/verify/orchestrate.go`, invoked by `devcouncil_verify_task` in `backend/go_orchestrator/devcouncil/registry.go`).
   - `Manvi` `"manvi/serve/verify.go"` implements `work.verify.task` (using its "VerifyTaskParams" struct) as a host-plane long-running job that invokes tests, DevMap impact, and a read-only critic directly. Keeping job lifecycle in `Manvi` while delegating diff/stub/coverage gates to `dcverify` closes this duplication.
3. **Shared `devcouncil_*` Tool Overlap (`registry.go` vs `manvi/devcouncil/tools.go`)**:
   - `DevCouncil`'s MCP registry (`backend/go_orchestrator/devcouncil/registry.go`) publishes `8` canonical tools (`devcouncil_get_diff`, `devcouncil_checkout_task`, `devcouncil_renew_lease`, `devcouncil_release_task`, `devcouncil_next_task`, `devcouncil_verify_task`, `devcouncil_get_gaps`, `devcouncil_policy_check_write`). `registry.go` explicitly comments: *"It does not import Manvi packages."*
   - `Manvi` `"manvi/devcouncil/tools.go"` independently defines tool schemas for those shared task/policy names alongside its harness-only tools (`devcouncil_read_file`, `devcouncil_write_file`, `devcouncil_patch_file`, `devcouncil_delete_file`, `devcouncil_exec_command`, `devcouncil_list_dir`, `devcouncil_find_files`, `devcouncil_grep`, `devcouncil_search_tools`, `devcouncil_spawn_subagents`, `devcouncil_request_override`, `devcouncil_get_next_actions`, `devcouncil_get_task`, plus "artifact_tools.go", "question_tools.go", "skill_tools.go", and "manvi/tools/tools.go"). Importing DevCouncil's shared tool specs eliminates schema drift.
4. **Host-Wiring Convergence (`devmap integrate` vs `dev integrate` vs Waylode Connect)**:
   - `backend/go_orchestrator/devcouncil/integrate/integrate.go` confirms `dev integrate` already delegates host table writes, MCP configs, and `.agents/` installation to `devmap integrate --servers-stdin` (`rust/devmap-cli/src/integrate.rs`) and `devmap agents install` (`rust/devmap-cli/src/agent_install.rs`), keeping only the Cursor rule and retired-host hints (`gemini`, `aider`).
   - Waylode Connect (`"src-tauri/src/agent_connect.rs"`, `"src-tauri/src/workbench/agy_plugin.rs"`, `"src-tauri/src/workbench/claude_hook.rs"`) still performs independent host detection and settings merging, creating the one remaining host-wiring overlap.
5. **Primary vs. Labs Edition Boundary (`devcouncil pr-gate`, `devmap cache`, `devmap federate`)**:
   - Verified in `backend/go_orchestrator/cmd/devcouncil/prgate.go`, `rust/devmap-cli/src/commands/cache.rs`, and `rust/devmap-cli/src/commands/federate.rs` (`rust/devmap-query/src/federate.rs`): DevCouncil ships local `devcouncil pr-gate`, local `devmap federate status|impact`, and `devmap cache` (`GITPULSE_RELAY_TOKEN` read strictly from env via mode-0600 temp curl config) with zero billing or sign-in gates inside `DevCouncil`.
   - `Waylode Labs` (`pro` `Labs Geek`, `team` `Labs Geek Squad`, `enterprise`) layers hosted infrastructure outside `DevCouncil` (`gitpulse-relay`): private-repo hosted `DevCouncil PR Gate`, managed R2 remote cache hosting for `devmap cache push|pull`, and org-wide Space federation. Public OSS repos and local BYOK compute stay free (`requires_sign_in: false`, `paywalled: false`).

## 3. Alternatives Considered

| Option | Pros | Cons | Decision |
|---|---|---|---|
| **A. Merge all 3 repos into a single monorepo binary** | Eliminates FFI/crate vendoring overhead | Violates independent release cadence of Tauri desktop ADE, Go agent CLI/serve, and standalone `devmap` CLI; forces `hyper` and GUI deps onto `devmap`. | **Rejected** |
| **B. Keep independent parallel tables (`devmap_parity`, Manvi `tools.go`, Waylode Connect) with manual checklists** | Zero short-term refactoring | Proven source of drift (already caused `GAP-P7-NEXT-TOOLS-DRIFT` and mismatched tool-count claims in `docs/STACK.md`). | **Rejected** |
| **C. Single canonical owner per capability + automated parity gates + in-place convergence of overlaps** | Preserves modular repo boundaries (`DevCouncil` -> `Manvi` -> `Waylode`) while closing duplicated logic at clean Go/Rust/CLI seams. | Requires explicit `contracts/` mirroring and scheduled closure tasks (`T-006`..`T-009`). | **Accepted** |
