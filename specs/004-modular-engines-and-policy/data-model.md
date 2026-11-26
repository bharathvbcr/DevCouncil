# Data Model: 004 - Modular Engines and Policy Core

## 1. Persistent Stores & Formats Owned by `DevCouncil`

| Store / Artifact | Location | Schema Constant & Owner | Purpose |
|---|---|---|---|
| Code Graph Database | `.devmap/codeintel/devmap.sqlite` (or `.devcouncil/codeintel/devmap.sqlite`) | `CURRENT_SCHEMA_VERSION = 29` (`rust/devmap-store/src/schema.rs`) | Symbols, references, edges, FTS, CSR shards, generations, unresolved classifications |
| Extraction Cache Payload | Inside `devmap.sqlite` (`file_payloads`, `extraction_cache`) | `EXTRACTION_SCHEMA_VERSION = "102"` (`rust/devmap-extract/src/cache.rs`) | Language extraction payloads keyed by `(file, digest, analyzer)` |
| Stacked Worktree Delta | `<wt_store>/delta.sqlite` + `<common_git_dir>/devmap/bases/gen-*/` + `stack_lease.json` | `rust/devmap-store/src/delta.rs`, `rust/devmap-query/src/stacked.rs` | Immutable `0444` base snapshots + RoaringBitmap-masked per-worktree AST deltas |
| Task & Lease State | `.devcouncil/state.sqlite` | `SCHEMA_VERSION = 11`, `WORKBENCH_SCHEMA = 17` (`rust/dc-store/src/lib.rs`, `workbench/mod.rs`) | Specs/tasks, active lease mutex (`task_leases`), scope envelopes, verification findings |
| Policy & Gate Config | `.devcouncil/config.yaml`, grants DB/state | `backend/go_orchestrator/devcouncil/gatescfg/`, `gate/`, `grants/` | Fail-closed gate mode (`enforce` / `advisory` / `off`), allowlists, grants |
| Agent Ledger | `.devcouncil/ledger.sqlite` | `rust/dc-store/src/` (`constitution.md` Article IX) | Append-only lease, verification, and session event audit log |

## 2. Module Surface Inventory Schema (`contracts/module-surface.schema.json`)

Every module record in [`contracts/module-surface.schema.json`](contracts/module-surface.schema.json) asserts:
- `crates`: all 16 product crates in `rust/` (`devmap-*`, `dc-*`, `gusset-engine`) with binary entrypoints, consumers (`DevCouncil`, `Manvi`, `Waylode`), and explicit non-goals.
- `go_packages`: core Go packages under `backend/go_orchestrator/` (`policy`, `gate`, `grants`, `dc`, `devcouncil/{spec,workflow,verify,integrate,mcp}`) with external import expectations.
