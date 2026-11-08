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
   - Remove regex fallback writes to `anchor_cas_store`, make CSR loading lazy, upgrade legacy `base_store_ref.json`, add `Manvi/.devmapignore` for upstream symlinked `crates/dc-*`, and add a warm `devmap mcp` stdio client in Go `dc/devmap`.

### Phase 1: `<15ms` `devmap seed`, Nested Cache Cloner & Sparse `verify`
1. **Schema-29 Compatible `seeded_repo_root` (`devmap-store`)**:
   - Add `seeded_repo_root TEXT` to `pending_state` DDL + idempotent migration; `latest_generation_repo_root` and `latest_repo_root` query `COALESCE(pending_state.seeded_repo_root, generations.repo_root)` when column exists; clear on `save_generation` and `RepoRootOwner::Gone`.
2. **Live-WAL `TRUNCATE` Heal + Pinned `WAL_READ_LOCK` (`seed.rs`)**:
   - Checkpoint via `busy_timeout = 20` + `TRUNCATE`; if busy, `PASSIVE` + `BEGIN DEFERRED; SELECT 1 FROM sqlite_master LIMIT 1;` + verify `(busy == 0 && log == checkpointed)` + `clonefile` while holding read mark.
   - Standalone `devmap seed` CLI cloning `{repo_map.json, code_graph.json, compact_code_graph.json, devmap.sqlite.csr, resolution-reuse-1/}`.
3. **Nested Hybrid Cache Cloner (`GitPulse` `cow_clone.rs`)**:
   - Discover nested Cargo `target/` (`depth <= 3`) + `dist`/`frontend/dist`; `libc::clonefile` `{deps, build, .fingerprint}`; boundary-checked `build/*/{output, root-output, out/**}` path rewrite with `(atime, mtime)` restoration; `tmutil addexclusion` + async teardown.
4. **Zero-Index-Trust Sparse `WorkingTreeSubject` (`verify/subject.go`)**:
   - Require `core.sparseCheckoutCone=true`, parse `<git_dir>/info/sparse-checkout`, `git read-tree HEAD` into scratch index, mark `SKIP_WORKTREE` only on out-of-cone `ENOENT` entries, `git add --sparse --all`, stream unmaterialized `HEAD` blobs via `git cat-file --batch`, and validate symlink targets lexically + against `tmpIdx`.

### Phase 2: Glean-Style `StackedStore` & Overlay Retirement
1. **Immutable Shared Base Snapshots (`devmap-store` & `devmap-query`)**:
   - Publish `chmod 0444` snapshots under `<common_git_dir>/devmap/bases/gen-<G>/{base.sqlite, base.csr, manifest.json}` with `stack_lease.json` lease tracking and `gc_unused_bases`.
   - Wire `devmap_query::overlay::{WorktreeOverlay, RoaringBitmap}` into `StackedStore` (`?mode=ro&immutable=1` verified `0444` base + Tree-sitter `delta.sqlite` + auto-rebase above `250` dirty files or `16 MiB`).
2. **Re-Vendor & Retire Split-Brain (`GitPulse`)**:
   - Re-vendor `devmap-*` crates via `scripts/vendor-crates.mjs`, migrate `GitPulse` codeintel onto canonical seeded/stacked stores, and retire `base_store_ref.json` split-brain.

### Phase 3: Hardened Compiler CAS, CoW Source Checkout & Multi-Lane `Manvi`
1. **Proc-Macro Allowlist (`gitpulse-worktree/src/build_cache.rs`)**:
   - `.dylib`/`.so`/`.dll` SHA-256 + crate-name Pure-AST allowlist + lexical guard forbidding `env!("CARGO_MANIFEST_DIR")`, `option_env!("CARGO_MANIFEST_DIR")`, and `include*!` before normalizing `CARGO_MANIFEST_DIR`.
2. **Zero-Copy Source Materialization (`GitPulse` `worktree.rs`)**:
   - `git worktree add --no-checkout` + APFS `clonefile` of clean tracked files from anchor + `git read-tree HEAD` + selective `git checkout-index -f` for dirty/diverged anchor paths + `git update-index --refresh`.
3. **Multi-Lane `Manvi` Coordinator (`subagent_tools.go` & `managed.go`)**:
   - Wire `WorkspaceCoordinator` for `workspace: "worktree"` provisioning zero-copy worktrees + `devmap seed` and verify concurrent sibling-worktree execution without `ErrHarnessBusy`.

## 3. Complexity Tracking
- **Article VII: Simplicity Limit**: Unifies worktree code-intelligence onto one canonical owner (`DevCouncil` `devmap-store` / `devmap-query`), retiring `GitPulse`'s parallel regex overlay implementation while adding zero external Cargo, Go, or npm dependencies.
