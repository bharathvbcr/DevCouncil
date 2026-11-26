# Feature: 004 - Modular Engines and Policy Core

> **Status (2026-11-26)**: Canonical Owner Repo: `DevCouncil`. All listed Rust binaries/crates (`devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `dc-glob`, `dc-proc`, `dc-evidence`, `gusset-engine`) and Go packages (`policy`, `gate`, `grants`, `devcouncil/spec`, `devcouncil/workflow`, `devcouncil/prgate`, `devcouncil/mcp`) are implemented on `main` under `MIT OR Apache-2.0` (100% free & open source with zero sign-in or billing logic; `Waylode Primary`); this spec codifies module boundaries, non-goals, and internal hardening follow-ups.

## 1. Problem Statement & Motivation

`DevCouncil` is a standalone, modular engine and schema core — not a monolithic agent TUI or desktop client. Clear specification of every exported engine (`devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `dc-glob`, `dc-proc`, `policy`/`gate`/`grants`, `devcouncil` spec/workflow/MCP) prevents downstream hosts (`Manvi` and `Waylode`) from reimplementing core algorithms while also stopping host concerns (LLM turn loops, desktop UI, git worktree cloning) from leaking back into `DevCouncil`.

## 2. Canonical Ownership & Non-Overlap Boundary

### Ownership Partition Table

| Module / Engine | Owns (Canonical `DevCouncil` Implementation) | Delegates To | Non-Goals / Forbidden Overlaps |
|---|---|---|---|
| **DevMap Engine & CLI** (`rust/devmap-{extract,resolve,analyze,store,query,serve,cli}`) | Multi-language AST extraction (`EXTRACTION_SCHEMA_VERSION = "102"`), 11-tier resolution, SQLite+CSR store (`CURRENT_SCHEMA_VERSION = 29`), `StackedStore` delta overlays, 21 MCP tools (`rust/devmap-serve/src/mcp.rs`), `devmap integrate` host writer, local `devmap federate`, `devmap cache` CLI | **Waylode** (`Primary`, `src-tauri/vendored/`) consumes read-only crates & 11-tool `devmap_parity`; **Manvi** (`backend/go_orchestrator/dc/devmap/warm.go`) calls stdio `devmap mcp`; **Waylode Labs** hosts remote R2 `devmap cache push\|pull` & org cross-repo federation | Never parses git worktree creation or CoW target cloning; never stores host UI preferences, billing state, or user credentials |
| **Task & Workbench Store** (`rust/dc-store/`, `dcstore`) | `SCHEMA_VERSION = 11` task/lease/scope/ledger SQLite engine (`rust/dc-store/src/lib.rs`) and `WORKBENCH_SCHEMA = 17` (`rust/dc-store/src/workbench/mod.rs`), `dcstore` CLI (`rust/dc-store/src/bin/dcstore.rs`), Go client (`backend/go_orchestrator/dc/store/`) | **Manvi** `work.*` RPCs operate as workbench host client; **Waylode** renders board state read-only | Never stores Markdown brief note formatting (`tasks/file_tasks.rs` in Waylode) or LLM chat histories |
| **Verification & Evidence** (`rust/dc-verify/`, `rust/dc-evidence/`, `dcverify`) | AST stub/skip/assertion gates (`rust/dc-verify/src/stub_ast.rs`), coverage normalization (`rust/dc-verify/src/coverage.rs`), bounded rigor (`rust/dc-verify/src/rigor.rs`), sparse cone verification (`backend/go_orchestrator/devcouncil/verify/sparse_subject.go`), `dcverify` binary | **Manvi** schedules async `work.verify.task` host runs (delegating rigor checks per [`specs/003`](../003-stack-boundaries-and-non-overlap/spec.md)) | Never runs model inference loops (`Manvi` owns secondary critic orchestration) |
| **Lexical Search, Redact & Subprocess Isolation** (`dc-grep`, `dc-redact`, `dc-glob`, `dc-proc`, `gusset-engine`) | `dcgrep` lexical/git search (`rust/dc-grep/src/lib.rs`), `dcredact` secret scrubbing (`rust/dc-redact/src/lib.rs`), `dc-glob` secret/protected patterns (`rust/dc-glob/src/secret_paths.rs`), `dc-proc` process group isolation & reaping (`rust/dc-proc/src/lib.rs`), `gusset-engine` C-ABI firewall | **Manvi** embeds `dc-proc`/`dc-redact`/`dc-glob` via Go/symlinked crates; **Waylode** vendors `dc-glob`, `dc-proc`, `dc-redact` | Never spawns OS terminals (`waylode-ptyd`) or manages agent TUI panes |
| **Policy Decision, Gate & Grants Engine** (`backend/go_orchestrator/policy`, `gate`, `grants`) | File & command policy evaluation (`policy/command.go`, `command_programoperand.go`, `command_secretread.go`, `file.go`, `matcher.go`), `gate.Gate` fail-closed mode control (`gate/gate.go`), persistent grants (`grants/grants.go`) | **Manvi** (`manvi/serve/policy.go`) runtime-enforces rules with `memguard` and `PostureDevCouncil` / `host-scope` (plus optional Labs Enterprise `policy_bundles` via host seam); **Waylode** triggers hook checks via `guard_command` | Never ships OS sandbox drivers, subscription checks, or GUI grant prompts |
| **DevCouncil CLI, Spec, Workflow, PR Gate & MCP Server** (`backend/go_orchestrator/cmd/devcouncil`, `devcouncil/`) | 7-layer spec parser/validator/exporter (`devcouncil/spec/`), DAG workflow runner (`devcouncil/workflow/`), `devcouncil_verify_task` & 7 sibling MCP tools (`devcouncil/registry.go`), `devcouncil pr-gate` CLI (free local + public OSS PR Gate), `devcouncil` CLI | **Manvi** executes multi-turn agents against those task leases; **Waylode Labs** runs optional hosted `DevCouncil PR Gate` for private repos | Retired Python `ModelRouter` and `src/` app remain removed; Go host never adds model providers, sign-in, or billing logic |

### Verified Module & Boundary Ledger (2026-11-26)

| Domain | Verified Source Seams | Cross-Repo Boundary Contract |
|---|---|---|
| `devmap` Extraction & Storage | `rust/devmap-extract/src/cache.rs`, `rust/devmap-store/src/schema.rs`, `rust/devmap-store/src/seed.rs`, `rust/devmap-query/src/stacked.rs`, `rust/devmap-cli/src/installation.rs`, `rust/devmap-cli/src/integrate.rs` | Authoritative `CURRENT_SCHEMA_VERSION = 29`, `EXTRACTION_SCHEMA_VERSION = "102"`, FIFO-safe host reads, Codex preflight checks |
| `dc-verify` & `dc-evidence` | `rust/dc-verify/src/{lib,rigor,stub_ast,coverage,bounded,substance}.rs`, `rust/dc-evidence/src/{lib,lean_tcb,strict_json,types}.rs` | Pin-free deterministic rigor & TCB checks consumed by `devcouncil verify` and `devcouncil_verify_task` |
| `dc-grep` / `dc-redact` / `dc-glob` / `dc-proc` | `rust/dc-grep/src/{lib,index,lexical,git}.rs`, `rust/dc-redact/src/lib.rs`, `rust/dc-glob/src/{lib,secret_paths,test_paths}.rs`, `rust/dc-proc/src/{lib,cli,diskio,lean_kernel}.rs` | Shared Rust primitives; Waylode vendors them (`src-tauri/vendored/`), Manvi symlinks `crates/dc-*` |
| Go `policy` / `gate` / `grants` | `backend/go_orchestrator/policy/{command,command_programoperand,command_secretread,decision,file,git_worktree,matcher}.go`, `gate/gate.go`, `grants/grants.go` | Pure decision rules & literal-path `sed`/`awk` `secret_read` judgment exported for Manvi & CLI callers |
| Go `devcouncil` CLI & Spec/Workflow | `backend/go_orchestrator/cmd/devcouncil/{main,spec,gate,search,tool,review,workflow,task}.go`, `devcouncil/{registry,lease}.go`, `devcouncil/spec/`, `devcouncil/workflow/` | Single owner of `devcouncil_*` task/lease/verify MCP tools and `.devcouncil/state.sqlite` orchestration |

## 3. User Stories & Acceptance Scenarios

### US1: Standalone Composable CLI & Crate Interfaces (Priority: P1)
As a consuming host (`Manvi` or `Waylode`) or CI pipeline,
I want every `DevCouncil` engine (`devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `devcouncil`) to operate deterministically from library and CLI entrypoints without requiring a desktop GUI or LLM runtime,
So that hosts compose only the modules they need via documented contracts.

- **Scenario 1: Library & JSON CLI Symmetry**
  - **Given** an initialized repository with `.devcouncil/` and `.devmap/` stores
  - **When** callers invoke `devmap <cmd> --json`, `dcstore`, `dcverify`, `dcgrep`, or `devcouncil spec validate <id> --strict --json`
  - **Then** every engine writes deterministic JSON on stdout with fail-closed exit codes and without spawning an LLM turn loop or GUI process.
- **Scenario 2: Read-Side Crate Re-Use Without Server Bloat**
  - **Given** `Waylode` needs in-process code-graph queries via vendored `devmap-query` and `devmap-store`
  - **When** `Waylode` vendors `rust/devmap-{extract,resolve,analyze,store,query}` and `rust/dc-{evidence,glob,proc,redact,store}`
  - **Then** the read-only crates compile cleanly without pulling `devmap-serve`'s HTTP stack or `devcouncil`'s Go CLI runtime.

### US2: Deterministic Policy Decision & Fail-Closed Gate Semantics (Priority: P1)
As a policy consumer (`devcouncil`, `manvi serve`, or `waylode-hook`),
I want `backend/go_orchestrator/policy`, `gate`, and `grants` to serve as the single canonical decision engine for file writes, secret paths, and shell command operands,
So that `command.secret_read`, `sed`/`awk` literal-path rules, and protected paths behave identically everywhere.

- **Scenario 1: Program-Operand & Secret-Read Judgment**
  - **Given** an agent command containing literal or shell-piped `sed` / `awk` operands targeting protected or secret files
  - **When** `policy` evaluates the command through `backend/go_orchestrator/policy/command_programoperand.go` and `command_secretread.go`
  - **Then** prohibited operands return deterministic rule decisions consumed identically by `devcouncil_policy_check_write`, `Manvi` `manvi/serve/policy.go`, and `Waylode` hooks.
- **Scenario 2: Dynamic Gate Config Reload Without Stale Allowance**
  - **Given** `.devcouncil/config.yaml` transitions to broken or malformed YAML while `devcouncil mcp serve` is running
  - **When** `OpenGate` checks `gatescfg.ConfigKey` inside `backend/go_orchestrator/devcouncil/registry.go`
  - **Then** `devcouncil_policy_check_write` fails closed with `not_initialized` instead of falling back to the prior gate posture or `"off"`.

### US3: Zero-Leakage Boundary With Host Workloads & Internal Hardening (Priority: P2)
As a `DevCouncil` maintainer,
I want clear module contracts in [`contracts/module-surface.schema.json`](contracts/module-surface.schema.json) and tracked closure of remaining internal hygiene/tooling items,
So that stale Python references never creep back in and internal gaps (`delta.sqlite` writer extraction stamp, shared Go tool export) are tracked cleanly.

- **Scenario 1: Extraction Version Awareness on Stacked Deltas**
  - **Given** stacked worktrees retain `delta.sqlite` rows across an `EXTRACTION_SCHEMA_VERSION` bump (documented limitation `ft-444eeede` in [`specs/002-zero-copy-worktrees/verification.md`](../002-zero-copy-worktrees/verification.md))
  - **When** delta rows record their writer's extraction version (`T-007`)
  - **Then** stale stacked delta rows invalidate automatically alongside warm SQLite generations without manual `devmap seed --sync-delta` resets.

## 4. Success Metrics & Testability Criteria

- **SC-001**: `devcouncil spec validate 004-modular-engines-and-policy --strict --json --project-root /Users/bharath/Code/devtools/DevCouncil` returns `valid: true` and `findings: null` (0 blocking, 0 advisory).
- **SC-002**: `devmap doc-drift --json specs/004-modular-engines-and-policy` reports `findings: []` (`total: 0`).
- **SC-003**: `node scripts/sync-agent-defs.mjs --check`, `node --test scripts/sync-agent-defs.test.mjs`, and `cargo test --manifest-path rust/Cargo.toml -p devmap-cli --test it -- shipped_skills` pass with 0 drift.
- **SC-004**: [`contracts/module-surface.schema.json`](contracts/module-surface.schema.json) parses cleanly as JSON and inventories all `17` Rust workspace crates plus `10` Go packages with explicit `explicit_non_goals`.

## 5. Edge Cases, Exclusions & Open Questions

- **Python Tooling Boundary**: Although the legacy Python application under `src/` is retired, Python scripts remain strictly for benchmarks (`benchmarks/`), test fixtures/contract generators (`backend/contracts/tools/generate_verdict_cases.py`), and Antigravity helper hooks; no product runtime engine depends on Python.
- **Zero Sign-In & Edition Boundary**: All local `DevCouncil` engines (`devmap`, `dcstore`, `dcverify`, `dcgrep`, `dcredact`, `devcouncil pr-gate`, local `devmap federate`, `devmap cache pack|unpack|status`) and public OSS PR gates stay 100% free and open source (`MIT OR Apache-2.0`, zero sign-in/billing code). Optional hosted services (`private-repo DevCouncil PR Gate`, R2 remote `devmap cache push|pull`, org cross-repo federation) live in `Waylode Labs` (`gitpulse-relay`); `devmap cache` reads `GITPULSE_RELAY_TOKEN` and `GITPULSE_RELAY_URL` strictly from the user's environment without embedding billing logic in `DevCouncil`.
- **Standalone Subprocess Contracts**: `dcverify` and `dcstore` execute primarily as deterministic subprocesses behind `rust/dc-proc/` isolation rather than requiring Gusset FFI everywhere (`docs/gusset-candidates.md`); Gusset FFI applies selectively where in-process Go <-> Rust benchmarks justify crossing overhead.
- **Exclusion**: Non-owned host features (LLM provider drivers, agent turn loops, Tauri UI components, `waylode-ptyd`, relay billing/auth servers) are explicitly excluded and governed by sibling `Manvi` and `Waylode` specifications.
