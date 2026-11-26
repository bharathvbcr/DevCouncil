# Implementation Plan: 003 - Stack Boundaries and Non-Overlap

> **Status (2026-11-26)**: Canonical ownership matrix (`contracts/ownership-matrix.json`) and initial architectural boundaries (`docs/STACK.md`) are ratified; `dev integrate` delegation (`backend/go_orchestrator/devcouncil/integrate/integrate.go` -> `rust/devmap-cli/src/integrate.rs`), `devmap` 11-tool parity checking, and read-only `contracts/` mirroring are shipped; four cross-repo overlap closures (`T-006`..`T-009`) remain tracked engineering work.

## 1. Architectural Topology

```
+-----------------------------------------------------------------------------------+
|                               Waylode (Desktop ADE)                               |
|  Svelte 5 UI · Git Client · Worktree & CoW Engine · Collision Radar · PTY Daemon  |
|  - DevMap subset (11 tools via vendored crates, parity-checked)                   |
|  - Policy hooks -> `manvi serve` (fail-closed `guard_command`)                    |
|  - Connect UI -> (target) `devmap integrate`                                      |
+------------------------------------+------------------------------+---------------+
                                     | NDJSON / stdio               | crates +
                                     v                              | schema parity
+----------------------------------------------------------------+  |
|                         Manvi (Agent Harness)                  |  |
|  Turn Loop · Providers · Subagents · TUI · `manvi serve`       |  |
|  - Enforces DevCouncil `policy`/`gate`/`grants` + `memguard`   |  |
|  - Wraps `devmap mcp` stdio (`dc/devmap/warm.go`)              |  |
|  - (Target) `work.verify.task` -> `dcverify`                   |  |
|  - (Target) imports canonical shared `devcouncil_*` Go tools   |  |
+------------------------------------+---------------------------+  |
                                     | Go pkg / CLI / SQLite        |
                                     v                              v
+-----------------------------------------------------------------------------------+
|                      DevCouncil (Canonical Component & Schema Core)               |
|  - `devmap-*` (21 MCP tools, Schema 29, extract 102, `devmap integrate`)          |
|  - `dc-store` (`SCHEMA_VERSION` 11, `WORKBENCH_SCHEMA` 17)                        |
|  - `dc-verify` / `dcverify`, `dc-proc`, `dc-grep`, `dc-redact`, `dc-glob`, Gusset |
|  - Go `policy` / `gate` / `grants`, 8 shared `devcouncil_*` tools, `contracts/`   |
|  - `devcouncil pr-gate`, local `devmap federate`, `devmap cache` CLI (100% Free)   |
+-----------------------------------------------------------------------------------+
                                     ^ (optional hosted relay / R2 cache / org PR gate)
                                     v
+-----------------------------------------------------------------------------------+
|               Waylode Labs (`gitpulse-relay`, `FCL-1.0-MIT`, Opt-In)              |
|  - `free` (inbox) · `pro` (Labs Geek) · `team` (Geek Squad) · `enterprise`        |
|  - Hosted private PR Gate · R2 `devmap cache` · Org federation · `policy_bundles` |
+-----------------------------------------------------------------------------------+
```

## 2. Verified Seams & Convergence Plan

1. **Canonical Contract & Ownership Registry (`DevCouncil`)**:
   - Maintain [`contracts/ownership-matrix.json`](contracts/ownership-matrix.json) as the cross-repo source of truth alongside `backend/contracts/` schemas and `rust/devmap-serve/src/mcp.rs::TOOL_NAMES`.
2. **Host Wiring Convergence (`devmap integrate`)**:
   - `rust/devmap-cli/src/integrate.rs` (host table writer), `rust/devmap-cli/src/skills.rs`, and `rust/devmap-cli/src/agent_install.rs` remain the sole writer of MCP registrations, project guides, and `.agents/`.
   - `backend/go_orchestrator/devcouncil/integrate/integrate.go` already delegates via `--servers-stdin`; Waylode's `"src-tauri/src/agent_connect.rs"` will route through `devmap integrate` (`T-008`).
3. **Verification Rigor Convergence (`dcverify`)**:
   - `rust/dc-verify/` and `backend/go_orchestrator/devcouncil/verify/orchestrate.go` execute AST stub/skip checks, LCOV/Go coverage normalization, and diff verification; Manvi's `"manvi/serve/verify.go"` (`work.verify.task`) will delegate verification execution to `dcverify` while preserving async claim management (`T-006`).
4. **Shared `devcouncil_*` Tool Convergence**:
   - `backend/go_orchestrator/devcouncil/registry.go` (`toolSpecs()`, 8 shared tools) exports schemas and dispatch; Manvi `"manvi/devcouncil/tools.go"` will import those 8 definitions instead of keeping a duplicate schema table (`T-007`).
5. **Skill Separation**:
   - `backend/go_orchestrator/devcouncil/skills/library/` and `rust/devmap-cli/skills/` ship DevCouncil/DevMap guidance only (`scripts/sync-agent-defs.mjs` + `rust/devmap-cli/tests/shipped_skills_name_only_real_devmap_surface.rs`); sibling product skills live exclusively in their owning repositories (`T-009`).
6. **Waylode Primary vs. Waylode Labs Edition Separation**:
   - All local DevCouncil engines (`devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `dc-glob`, Go `policy`/`gate`/`grants`, `devcouncil pr-gate` CLI, local `devmap federate`, and `devmap cache pack|unpack|status`), plus BYOK user-compute paths, remain 100% free and sign-in-free (`MIT OR Apache-2.0`).
   - `Waylode Labs` (`gitpulse-relay`, private `GitPulse-labs`, `FCL-1.0-MIT`) supplies only hosted/shared extensions (`Labs Geek` / `pro`, `Labs Geek Squad` / `team`, `Enterprise` / `enterprise`): private-repo hosted `DevCouncil PR Gate`, remote R2 `devmap cache push|pull`, and Space/org cross-repo federation, keeping zero billing or account state inside `DevCouncil`.

## 3. Constitution Alignment Check

| Article | Compliance | Notes |
|---|---|---|
| **I: Library-First** | PASSED | Ownership rules govern crate/package seams (`devmap-*`, `dc-store`, `dc-verify`, Go `policy`/`gate`/`grants`) prior to IPC wrappers. |
| **II: CLI First-Class** | PASSED | Validated via `devcouncil spec validate` and `devmap doc-drift --json`. |
| **IV: Fail-Closed Gates** | PASSED | Cross-repo parity checks (`check-schema-parity.mjs`, `shipped_skills`, Go contract tests) fail closed on drift. |
| **V: Single Source of Truth** | PASSED | Core objective of this spec: eliminate all four verified duplicate implementations across repos. |
| **VII: Simplicity Limit** | PASSED | Introduces zero new runtime modules, crates, or external dependencies; consolidates existing seams in place. |

## 4. Complexity Tracking

| Potential Violation / Deviation | Justification | Mitigation / Retirement Plan |
|---|---|---|
| Waylode re-implements 11 `devmap_*` MCP read handlers in `"src-tauri/src/mcp/devmap_parity.rs"` instead of vendoring `devmap-serve` | `devmap-serve` pulls `hyper` and HTTP server dependencies unsuitable for Waylode's desktop stdio binary and read-only in-process `codeintel` cache. | Keep `"devmap_parity.rs"` strictly read-only against vendored `devmap-query`/`devmap-store`, and gate every release with Waylode's `check-schema-parity.mjs` against `rust/devmap-serve/src/mcp.rs::TOOL_NAMES`. |
| Dual-prefix legacy MCP identifiers (`gitpulse-mcp`, `gitpulse_*`, `.gitpulse/`) coexist with `waylode-*` | Existing user installations, IDE configs, and worktrees require non-breaking upgrade paths. | Default MCP catalog serves `waylode_*` only; `gitpulse_*` requires explicit `WAYLODE_MCP_EXPOSE_LEGACY=1`; prose docs use `Waylode`. |
| Manvi temporarily keeps local schemas for the 8 shared `devcouncil_*` tools and host-plane `work.verify.task` | Historical separation during the Phase 7 Go rewrite left Manvi carrying standalone definitions. | Tracked for in-place convergence in `T-006` and `T-007` with golden-envelope parity tests. |
