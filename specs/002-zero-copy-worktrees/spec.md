# Feature: 002 - Zero-Copy Agentic Worktrees & DevMap Hardening

## 1. Problem Statement & Motivation
Multi-agent development creates short-lived Git worktrees across repositories (`DevCouncil`, `GitPulse`, `Manvi`), but provisioning and querying those lanes suffers from 17 verified failure modes (`F1`–`F17`): split-brain `base_store_ref.json` overlays invisible to `devmap`, MCP routing leaks/refusals across linked worktrees, regex fallback AST poisoning of anchor CAS stores, `8.03 MiB` SQLite overflow page rewrites during `rebind_staged`, `~272 MiB` `VACUUM INTO` penalties on live WALs, missed nested Cargo `target/` workspaces, sparse-checkout verification refusals, proc-macro `CARGO_MANIFEST_DIR` thrash/hazards, and `68–90 MiB` private source extent inflation on `git worktree add`.

## 2. User Stories & Acceptance Scenarios

### US1: Instant Canonical DevMap Seeding & Safe MCP Routing (Priority: P1)
As an autonomous coding agent operating in a linked worktree,
I want `devmap` and `devmap mcp` to seed or stack my worktree's code graph in `<15ms` without leaking another worktree's graph or refusing valid sibling worktrees,
So that graph navigation (`explore`, `impact`, `affected`) is immediately query-ready and honest from the first tool call.

**Acceptance Scenarios**:
- **Scenario 1: Bidirectional Registered Worktree Verification**
  - **Given** a `devmap mcp` server pinned with `--root <MAIN>` and a registered linked worktree `<WT>` of `<MAIN>`
  - **When** an agent calls a tool with `repo_path: "<WT>"` or forging an unregistered `.git` gitlink pointing to `<MAIN>/.git`
  - **Then** registered `<WT>` succeeds using `<WT>`'s store while the forged `.git` checkout is hard-refused.
- **Scenario 2: Asymmetric Unpinned Root Resolution**
  - **Given** an unpinned `devmap mcp` server with `mcp_roots = [<MAIN>]` where `<MAIN>` is indexed and `client_cwd` is inside an unindexed linked worktree `<WT>`
  - **When** a tool call omits `repo_path`
  - **Then** `RootResolveInput::resolve` refuses ambiguity instead of silently serving `<MAIN>`'s stale graph.
- **Scenario 3: Overflow-Free Rebind, Staged Lane Build (`F-g`), & Live-WAL CoW Seed**
  - **Given** an anchor `devmap.sqlite` with a large `generations.analysis_json` payload and non-empty `-wal`, and a linked worktree `<WT>` with its own committed or working edits
  - **When** `devmap seed` provisions `<WT>`
  - **Then** `stage_seed_healing_wal` checkpoints/clones under pinned `WAL_READ_LOCK`, `rebind_staged` writes `seed_rebind` + `pending_state.repo_root` without rewriting historical `generations` overflow pages, any diverged `<WT>` tree runs an incremental build + `HEAD` stamp on the staged store before atomic placement, and no partial store is placed on failure.

### US2: Glean-Style StackedStore, Zero-Write-on-Read, & Overlay Poisoning Elimination (Priority: P1)
As an agent editing files inside a worktree lane,
I want explicit delta sync commands to update a Tree-sitter worktree delta layered over an immutable `0444` shared base snapshot while all read paths (`devmap mcp`, CLI read commands, `status`, `hook`) stay strictly zero-write-on-read,
So that anchor stores are never poisoned by regex ASTs, read paths never mutate `delta.sqlite` or `stack_lease.json`, and legacy `base_store_ref.json` pointers fail closed naming `devmap seed --stacked --from <main>`.

**Acceptance Scenarios**:
- **Scenario 1: Immutable Base Snapshot (`gen-<G>-<head12>-<src8>`) & Lease GC**
  - **Given** a clean, committed donor generation `G` in `<common_git_dir>/devmap/bases/gen-<G>-<head12>-<src8>/` with `0444` `base.sqlite` and `base.csr`
  - **When** worktree lanes hold `stack_lease.json` leases and an older unleased base exists
  - **Then** `publish_base_snapshot` refuses dirty donors, `StackedStore::open` verifies `(len, mtime_ns, ctime_ns, ino, dev, mode == 0444)` via `fstat` before opening `base.sqlite` read-only, and `gc_unused_bases` prunes only unleased generations under `.lock`.
- **Scenario 2: Zero-Write-on-Read Tree-Sitter Delta & RoaringBitmap Tombstoning**
  - **Given** a stacked worktree lane synced via `devmap seed --stacked` or `devmap seed --sync-delta` where a source file modifies functions and deletes a symbol
  - **When** CLI or MCP read tools query symbols, callers, dependencies, dead code, or doc drift (even with subsequent unsynced scratch edits on disk)
  - **Then** base facts for synced `FileId`s are masked via `WorktreeOverlay` `RoaringBitmap` bitsets and merged with `delta.sqlite` symbols/edges while leaving `delta.sqlite`, `stack_lease.json`, and `base.sqlite` byte-for-byte untouched.

### US3: Nested Hybrid Build Cache & Zero-Copy Source Materialization (Priority: P2)
As a worktree orchestrator (`GitPulse` and `Manvi`),
I want new agent worktrees to CoW-clone clean tracked sources and nested Cargo/`dist` caches with `mtime`-preserving `OUT_DIR` path rewriting and safe proc-macro caching,
So that lane creation finishes in `~150ms` with near-zero private APFS block allocation and warm Cargo builds.

**Acceptance Scenarios**:
- **Scenario 1: Nested Cargo Workspace & Boundary-Safe Path Rewrite**
  - **Given** an anchor repo with nested `rust/target` or `src-tauri/target` containing `build/*/out/paths.rs` and Mach-O binaries alongside sibling lanes `lane-1` and `lane-10`
  - **When** `cow_clone` clones `{deps, build, .fingerprint}` into `lane-1`
  - **Then** exact path prefixes are rewritten without corrupting `lane-10` or binary files, and original `(atime, mtime)` timestamps are restored.
- **Scenario 2: Pure-AST Proc-Macro Allowlist Guard**
  - **Given** `gitpulse-rustc` Stage-2 compilation of a crate invoking proc-macros
  - **When** all loaded proc-macro `.dylib` hashes match the Pure-AST allowlist and source tokens contain no `env!("CARGO_MANIFEST_DIR")` or `include*!`
  - **Then** `CARGO_MANIFEST_DIR` normalizes to `COMPILE_ONLY_MANIFEST_DIR_SENTINEL`; otherwise raw `CARGO_MANIFEST_DIR` is hashed fail-closed.

### US4: Zero-Index-Trust Sparse Checkout Verification & Multi-Lane Harness (Priority: P2)
As a verification gate (`devcouncil verify`) and multi-agent host (`Manvi`),
I want cone sparse checkouts to verify deterministically against `HEAD` and sibling worktree runs to execute in parallel,
So that sparse monorepo lanes pass strict safety gates and subagents can isolate work via `workspace: "worktree"`.

**Acceptance Scenarios**:
- **Scenario 1: Zero-Index-Trust Sparse Subject**
  - **Given** a worktree with `core.sparseCheckout=true` and `core.sparseCheckoutCone=true` containing both unmaterialized out-of-cone `HEAD` blobs and a dangling/escaping symlink
  - **When** `WorkingTreeSubject` runs verification
  - **Then** out-of-cone `ENOENT` blobs stream from `git cat-file --batch` without trusting `<git_dir>/index`, and escaping or broken symlinks fail verification deterministically.
- **Scenario 2: Parallel Multi-Lane Subagent Execution**
  - **Given** two concurrent `Manvi` runs targeting distinct worktrees of the same repository
  - **When** both enter `managed.go` checkout gates and spawn `workspace: "worktree"` subagents
  - **Then** neither blocks with `ErrHarnessBusy` and each subagent receives a seeded isolated worktree.

## 3. Success Metrics & Testability Criteria
- **SC-001**: `devmap seed` rebinds `seeded_repo_root` in `<15ms` writing `<=32 KiB` (zero `generations` overflow page rewrites).
- **SC-002**: `hook_conformance` (`briefs_agree_with_status_across_many_worktrees_under_concurrency` and `an_unindexed_worktree_is_told_how_to_index_itself_on_session_start_only`) passes 20/20 consecutive runs with zero flakes.
- **SC-003**: `Manvi` `devmap status --json` reports `discovery_refused.total == 0` while `TestCratePinsMatchDevCouncil` passes.
- **SC-004**: Forged `.git` gitlink files pointing at another repo's `.git` are 100% rejected by `same_registered_repo`, `mcp::open_for_call`, and `devmap seed`.
- **SC-005**: `GitPulse/scripts/check-schema-parity.mjs` and `check-vendor-schema.mjs` pass at `CURRENT_SCHEMA_VERSION = 29`.
- **SC-006**: Zero regressions across existing Rust and Go test suites in `DevCouncil`, `GitPulse`, and `Manvi`.

## 4. Edge Cases & Exclusions
- **Edge Case**: Pre-existing schema-29 SQLite stores opened read-only (`?mode=ro`) before `seed_rebind` DDL runs must dynamically fall back to `generations.repo_root` without `no such table` errors.
- **Edge Case**: `seed -> git worktree move -> devmap build` (including by an older writer) honors `seed_rebind` only while `pending_state.repo_root` agrees with it, and clears stale `seed_rebind` on the next generation commit or restamp.
- **Exclusion**: Non-cone (`core.sparseCheckoutCone=false`) arbitrary gitignore-pattern sparse checkouts remain refused fail-closed in `WorkingTreeSubject`.
- **Exclusion**: Cargo `build.build-dir` sharing across concurrent worktrees is explicitly excluded due to Cargo `1.99.0` cross-branch `.rlib` corruption (`F12`).
