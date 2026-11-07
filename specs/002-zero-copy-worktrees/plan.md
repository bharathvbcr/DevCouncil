# Technical Plan: 002 - Zero-Copy Agentic Worktrees & DevMap Hardening

## 1. Architectural Overview
This plan eliminates all 17 verified worktree/code-intelligence failure modes (`F1`–`F17`) across `DevCouncil` (Rust `devmap-*` + Go `verify` & `dc/devmap`), `GitPulse` (Tauri engine `cow_clone`, `codeintel`, `gitpulse-worktree`), and `Manvi` (Go `devcouncil` navigation & subagent worktree coordinator, `serve/managed.go`).

## 2. Phase Topology & Component Interactions

### Phase 0: Correctness, Routing & Poisoning Stop-Bleed
1. **Bidirectional Worktree Identity (`devmap-extract`)**:
   - Implement `registered_common_git_dir`, `main_checkout_of`, and `same_registered_repo(a, b)` verifying `<common>/worktrees/<lane>/gitdir` canonicalizes back to `<wt>/.git` and rejecting symlinked `.git` files.
2. **MCP Routing Hardening (`devmap-serve`)**:
   - `mcp::open_for_call`: Permit `same_registered_repo(&attr.root, &pin)` when `--root` is pinned, lazily seeding linked worktrees from an indexed main checkout when needed.
   - `RootResolveInput::resolve`: Asymmetric resolution — refuse multi-worktree `mcp_roots` when `repo_path` is omitted, and refuse single-store `mcp_roots = [R]` when `client_cwd` is inside a distinct linked worktree or nested checkout under `R`.
3. **Hook & Build Staging Atomicity (`devmap-cli`)**:
   - Include `--manifest` in `hook::build_args`; trigger `detach_seeded_build` on `PostToolUse` for unindexed linked worktrees (preserving `PreToolUse` silence contract).
   - Keep `build --seed-from` staged until `lane`'s generation commits so concurrent `SessionStart` hooks never observe `main`'s stale symbol count at `cli.db()`.
4. **Stop-Bleed & Warm Navigation (`GitPulse`, `Manvi`, `dc/devmap`)**:
   - Retire `GitPulse`'s legacy `overlay.json`, `base_store_ref.json` writer, `CasAstStore`, and regex `extract_fallback_ast` in favor of canonical `StackedStore`, refuse legacy `base_store_ref.json` on read naming `devmap seed --stacked --from <main>`, add `Manvi/.ignore` for upstream symlinked `/crates/dc-*`, and add a warm `devmap mcp` stdio client with hung-server restart and full graph verbs in Go `dc/devmap`.

### Phase 1: `<15ms` `devmap seed`, Staged Lane Build (`F-g`), Nested Cache Cloner & Sparse `verify`
1. **Schema-29 Compatible `seed_rebind` (`devmap-store`)**:
   - Record `seed_rebind.seeded_repo_root` alongside `pending_state.repo_root` during `rebind_staged` without rewriting historical `generations` overflow pages; honor `seed_rebind` only while `pending_state.repo_root` agrees (`0d285d0b`), and normalize `generations` + clear `seed_rebind` when a generation commits or restamps on a seeded store.
2. **Live-WAL `TRUNCATE` Heal, Pinned `WAL_READ_LOCK`, & Build-Before-Place (`seed.rs`)**:
   - Checkpoint via `busy_timeout = 20` + `TRUNCATE`; if busy, `PASSIVE` + `BEGIN DEFERRED; SELECT 1 FROM sqlite_master LIMIT 1;` + verify `(busy == 0 && log == checkpointed)` + `clonefile` while holding read mark.
   - Standalone `devmap seed` (`F-g`): under target `WriterLock`, stage donor store + CSR, verify search index health, run silent incremental build + `HEAD` stamp on the staged store when the lane's tree diverged, settle WAL, and atomically rename `devmap.sqlite` last so readers immediately see the lane's edits and `HEAD`.
3. **Nested Hybrid Cache Cloner (`GitPulse` `cow_clone.rs`)**:
   - Discover nested Cargo `target/` (`depth <= 3`) + `dist`/`frontend/dist`; `libc::clonefile` `{deps, build, .fingerprint}`; boundary-checked `build/*/{output, root-output, out/**}` path rewrite with `(atime, mtime)` restoration; `tmutil addexclusion` + async teardown.
4. **Zero-Index-Trust Sparse `WorkingTreeSubject` (`verify/subject.go`)**:
   - Require `core.sparseCheckoutCone=true`, parse `<git_dir>/info/sparse-checkout`, `git read-tree HEAD` into scratch index, mark `SKIP_WORKTREE` only on out-of-cone `ENOENT` entries, `git add --sparse --all`, stream unmaterialized `HEAD` blobs via `git cat-file --batch` keyed by `(oid, mode)`, and validate symlink targets per entry lexically + against `tmpIdx`.

### Phase 2: Glean-Style `StackedStore`, Zero-Write-on-Read, & Legacy Overlay Retirement
1. **Immutable Shared Base Snapshots (`devmap-store` & `devmap-query`)**:
   - Refuse dirty donors (`BLOCK 2`) and publish `chmod 0444` snapshots under `<common_git_dir>/devmap/bases/gen-<G>-<head12>-<src8>/{base.sqlite, base.csr, manifest.json}` (`BLOCK 1`) with `stack_lease.json` lease tracking and `.lock`-serialized `gc_unused_bases` (`F-b`).
   - Wire `devmap_query::overlay::{WorktreeOverlay, RoaringBitmap}` into `StackedStore`: strictly read-only `StackedStore::open` / `open_if_leased` for MCP, CLI `ReadStore`, and `status` (zero write-on-read; `is_fresh` never forced true when `dirty_files > 0`), and explicit `WriterLock`-guarded `sync_worktree` / `materialize_seeded_store` (`F-d`) on write commands (`devmap seed --stacked`, `devmap seed --sync-delta`).
2. **Re-Vendor & Retire Legacy `worktree_overlay.rs` (`GitPulse` `P2.3`)**:
   - Re-vendor all 11 `devmap-*` / `dc-*` crates via `scripts/vendor-crates.mjs`, migrate `GitPulse` codeintel, `agent_bus`, `cow_clone`, `live.rs`, and `gitpulse_worktree_overlay_sync` completely onto canonical `StackedStore`, and delete `overlay.json`, `base_store_ref.json` writer, `CasAstStore`, and regex `extract_fallback_ast`.

### Phase 3: Hardened Compiler CAS, CoW Source Checkout & Multi-Lane `Manvi`
1. **Proc-Macro Allowlist (`gitpulse-worktree/src/build_cache.rs`)**:
   - `.dylib`/`.so`/`.dll` SHA-256 + crate-name Pure-AST allowlist + lexical guard forbidding `env!("CARGO_MANIFEST_DIR")`, `option_env!("CARGO_MANIFEST_DIR")`, and `include*!` before normalizing `CARGO_MANIFEST_DIR`.
2. **Zero-Copy Source Materialization (`GitPulse` `worktree.rs`)**:
   - `git worktree add --no-checkout` + APFS `clonefile` of clean tracked files from anchor + `git read-tree HEAD` + selective `git checkout-index -f` for dirty/diverged anchor paths + `git update-index --refresh`.
3. **Multi-Lane `Manvi` Coordinator (`subagent_tools.go` & `managed.go`)**:
   - Wire `WorkspaceCoordinator` for `workspace: "worktree"` provisioning zero-copy worktrees + `devmap seed` and verify concurrent sibling-worktree execution without `ErrHarnessBusy`.

## 3. Complexity Tracking
- **Article VII: Simplicity Limit**: Unifies worktree code-intelligence onto one canonical owner (`DevCouncil` `devmap-store` / `devmap-query`), retiring `GitPulse`'s parallel regex overlay implementation while adding zero external Cargo, Go, or npm dependencies.
