# Research & Module Audit: 004 - Modular Engines and Policy Core

## 1. Crate & Package Verification Summary

Verified against `DevCouncil` `main` (`2026-11-26`):
1. **Rust Workspace (`rust/`)**:
   - DevMap stack: `devmap-extract` (Tree-sitter extraction, schema `"102"`), `devmap-resolve` (multi-language call/reference resolution), `devmap-analyze` (liveness, PDG, clones), `devmap-store` (SQLite schema `29`, CSR, seeding, base snapshots), `devmap-query` (`StackedStore`, semantic ranking, `devmap_doc_drift`), `devmap-serve` (21 MCP tools in `rust/devmap-serve/src/mcp.rs`), and `devmap-cli` (`devmap` CLI, `devmap integrate`, `devmap hook`, `devmap cache`).
   - Core state, rigor & safety crates: `dc-store` (`SCHEMA_VERSION = 11`, `WORKBENCH_SCHEMA = 17`), `dc-verify` (`stub_ast.rs`, `coverage.rs`, `rigor.rs`), `dc-evidence` (`lean_tcb.rs`, `strict_json.rs`), `dc-grep` (lexical/git search), `dc-redact` (credential redaction), `dc-glob` (`secret_paths.rs`, `test_paths.rs`), `dc-proc` (bounded subprocess execution & reaping), `dc-regress` / `dc-regress-store` (golden/regression tooling), `gusset-engine` (C-ABI firewall).
2. **Go Orchestrator (`backend/go_orchestrator/`)**:
   - Policy & Gates: `policy/` (`command.go`, `command_programoperand.go`, `command_secretread.go`, `file.go`, `matcher.go`, `decision.go`), `gate/gate.go`, `grants/grants.go`, `fnmatch/`.
   - Orchestration & Specs: `devcouncil/spec/` (7-layer parser, validator, task export), `devcouncil/workflow/` (DAG engine & persistence), `devcouncil/verify/` (`orchestrate.go`, `sparse_subject.go`), `devcouncil/integrate/` (delegating to `devmap integrate`), `devcouncil/mcp/` & `devcouncil/registry.go` (8 MCP tools), `cmd/devcouncil/` CLI.

## 2. Alternatives Considered

| Option | Pros | Cons | Decision |
|---|---|---|---|
| **A. Rewrite Go `policy`/`gate`/`grants` in Rust and remove Go packages** | Single language across `devmap` and policy | Breaks direct Go package imports in `Manvi` (`manvi/serve/policy.go`) and forces costly FFI crossings on microsecond path decisions (`docs/gusset-candidates.md`). | **Rejected** |
| **B. Vendor `devmap-serve` into Waylode (`GitPulse`)** | Eliminates the 11-tool wrapper table | Pulls `hyper` HTTP transport stack into desktop binary and breaks multi-repo `repo_path` isolation guarantees (`src-tauri/src/mcp/devmap_parity.rs` comment). | **Rejected** |
| **C. Keep explicit library/CLI boundaries with parity checks & shared `contracts/`** | Lets `Manvi` import Go policy directly and `Waylode` vendor read-side Rust crates cleanly while sharing single wire schemas. | Requires tracking `delta.sqlite` extract stamp (`T-007`) and shared `devcouncil_*` Go tool export (`T-006`). | **Accepted** |
