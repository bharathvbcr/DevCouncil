# Implementation Plan: 004 - Modular Engines and Policy Core

> **Status (2026-11-26)**: Core Rust engines (`rust/` workspace) and Go orchestration/policy packages (`backend/go_orchestrator/`) are implemented on `main`. This plan documents the canonical crate/package layering and remaining internal hardening work (`T-006`..`T-008`).

## 1. Architecture & Dependency Layering

```
Layer 3: CLI & Server Entrypoints (No domain logic ownership)
  - `rust/devmap-cli/` (`devmap`)       - `rust/devmap-serve/` (`devmap mcp/serve`, 21 tools)
  - `rust/dc-store/src/bin/dcstore.rs`  - `rust/dc-verify/src/bin/dcverify.rs`
  - `backend/go_orchestrator/cmd/devcouncil/` (`devcouncil` CLI + MCP)

Layer 2: Orchestration, Policy & Spec Engines (Go + Rust Query)
  - `backend/go_orchestrator/policy/` · `gate/` · `grants/`
  - `backend/go_orchestrator/devcouncil/{registry,lease,spec,workflow,verify,integrate}/`
  - `rust/devmap-query/` (`StackedStore`, `WorktreeOverlay`, `devmap_doc_drift`)
  - `rust/devmap-analyze/` (liveness, PDG, resolution rate)

Layer 1: Core Extraction, Storage & Primitive Libraries (Vendored/Linked)
  - `rust/devmap-extract/` (`EXTRACTION_SCHEMA_VERSION = "102"`)
  - `rust/devmap-resolve/`
  - `rust/devmap-store/` (`CURRENT_SCHEMA_VERSION = 29`, CSR, `seed.rs`)
  - `rust/dc-store/` (`SCHEMA_VERSION = 11`, `WORKBENCH_SCHEMA = 17`)
  - `rust/dc-verify/` · `rust/dc-evidence/` · `rust/dc-grep/` · `rust/dc-redact/` · `rust/dc-glob/` · `rust/dc-proc/` · `rust/gusset-engine/`
```

## 2. Internal Seam Guarantees & Pending Hardening

1. **Zero-Cycle MCP & Policy Registry**:
   - `backend/go_orchestrator/devcouncil/registry.go` (`toolSpecs()`) serves as the single definition source for DevCouncil's 8 shared MCP tools (`allowed_next_tools_test.go` verifies agreement with `devcouncil/verify/`).
   - Follow-up `T-006` extracts the shared 8-tool specification slice so external Go consumers (`Manvi`) import it directly without import cycles or duplicate schemas.
2. **Stacked Delta Extraction Hygiene**:
   - `rust/devmap-store/src/delta.rs` and `rust/devmap-query/src/stacked.rs` layer per-worktree edits over immutable `0444` bases; follow-up `T-007` adds `EXTRACTION_SCHEMA_VERSION` stamping to `delta.sqlite` (`ft-444eeede`).
3. **Host Wiring Single-Path Enforcement**:
   - `rust/devmap-cli/src/integrate.rs`, `skills.rs`, and `agent_install.rs` enforce fail-closed writes (`installation.rs` FIFO-safe reads and `integrate.rs` Codex preflight).
   - `backend/go_orchestrator/devcouncil/integrate/integrate.go` passes server definitions via `--servers-stdin`; follow-up `T-008` exposes a structured JSON plan/apply helper for desktop hosts (`Waylode Connect`).
4. **Zero-Paywall Local Core vs. Optional Hosted Labs Relay (`gitpulse-relay`)**:
   - `backend/go_orchestrator/cmd/devcouncil/prgate.go`, `rust/devmap-cli/src/commands/federate.rs`, and `rust/devmap-cli/src/commands/cache.rs` contain zero sign-in, license, or billing checks (`MIT OR Apache-2.0`).
   - Hosted extensions (`private-repo DevCouncil PR Gate`, managed R2 `devmap cache` storage, org Space federation) sit behind external `Waylode Labs` relay endpoints (`GITPULSE_RELAY_URL` / `GITPULSE_RELAY_TOKEN`), keeping local CLI & public OSS CI workflows completely free.

## 3. Constitution Alignment Check

| Article | Compliance | Notes |
|---|---|---|
| **I: Library-First** | PASSED | Every binary (`devmap`, `dcstore`, `dcverify`, `devcouncil`) wraps library crates/packages. |
| **II: CLI First-Class** | PASSED | All engines expose deterministic `--json` CLI interfaces before MCP/UI exposure. |
| **III: Test-First** | PASSED | Contract, golden-envelope, adversarial, and `shipped_skills` test suites guard each module. |
| **IV: Fail-Closed Gates** | PASSED | `gate.Gate`, `devmap_doc_drift`, FIFO checks, and schema floors refuse on missing or stale state. |
| **VII: Simplicity Limit** | PASSED | Catalogs existing implemented crates/packages with zero new external dependencies. |

## 4. Complexity Tracking

| Architectural Trade-Off | Rationale | Constraint / Boundary |
|---|---|---|
| Hybrid Go (`policy`/`gate`/`devcouncil`) + Rust (`dc-verify`/`dc-store`/`devmap-*`) codebase | Rust handles compute-heavy AST parsing, SQLite WAL/CSR graphs, and signal-safe reaping (`dc-proc`); Go provides fast CLI dispatch and policy/orchestration packages imported directly by `Manvi`. | Cross-language contracts pinned by `backend/contracts/`, `dc/store/`, and `dc/devmap/` clients (`gusset` used only where microbenchmarks justify FFI overhead per `docs/gusset-candidates.md`). |
