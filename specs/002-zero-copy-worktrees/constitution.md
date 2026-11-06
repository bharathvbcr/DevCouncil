# Constitution & Invariants: 002 - Zero-Copy Agentic Worktrees

1. **Zero-Index-Trust Verification**: `devcouncil verify` MUST NEVER trust a pre-existing `<git_dir>/index` (`SKIP_WORKTREE` or `assume-unchanged` bits). Sparse checkouts are verified from `HEAD` + cone prefix rules + disk `Lstat` + `git cat-file --batch`.
2. **Schema-29 Wire Parity**: `CURRENT_SCHEMA_VERSION` MUST remain `29` across `DevCouncil`, `GitPulse` vendored crates, `Manvi`, and `Jarvis`. Additive columns (`seeded_repo_root`) MUST degrade gracefully on read-only pre-migration databases.
3. **Fail-Closed Security Boundaries**:
   - Forged `.git` gitlink files and symlinked `.git` entries MUST be rejected before opening or seeding any store.
   - Symlinks escaping the repository root or dangling outside `tmpIdx` in sparse checkouts MUST fail verification.
   - Unknown or I/O-capable proc-macros MUST hash raw `CARGO_MANIFEST_DIR` in Stage 2.
4. **Immutable Snapshot Discipline**: `?immutable=1` MUST ONLY be used after verifying regular-file metadata with zero write permission bits (`mode & 0o222 == 0`).
5. **Single Canonical Owner**: `DevCouncil` owns store seeding, `StackedStore`, and Go `dc/devmap` client transport; downstream consumers (`GitPulse`, `Manvi`) delegate rather than maintaining drifting reimplementations.
