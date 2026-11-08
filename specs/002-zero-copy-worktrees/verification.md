# Verification Plan: 002 - Zero-Copy Agentic Worktrees

## Adversarial Break-It Matrix
1. **Gitlink Forgery (`F3b`, Critique 1)**: Create an unregistered directory with `.git` containing `gitdir: <main>/.git/worktrees/fake` or `<main>/.git` and assert `same_registered_repo`, `mcp::open_for_call`, and `devmap seed` reject it.
2. **Ambiguous Worktree MCP Leak (`F3a`, Critique 2)**: Configure `mcp_roots = [main]` with `client_cwd = unindexed_linked_wt` (and `mcp_roots = [main, unindexed_linked_wt]`), omit `repo_path`, and assert refusal; verify `roots_win_when_cwd_has_no_store` with unrelated scratch `cwd` still passes.
3. **Concurrent Hook Storm (`F2`)**: Run `briefs_agree_with_status_across_many_worktrees_under_concurrency` 20 iterations back-to-back and `an_unindexed_worktree_is_told_how_to_index_itself_on_session_start_only`.
4. **Seeded Rebind + Move (`F6`, `F8`, Critique 3)**: Seed a lane, verify zero `generations` table page modification, move the worktree (`git worktree move`), rebuild, and assert `latest_repo_root` reflects the moved path.
5. **Live WAL Contention (`F7`, Critique 5)**: Hold an active WAL reader/writer while calling `stage_seed`, verifying pinned `WAL_READ_LOCK` `clonefile` path when checkpointed and clean fallback when uncheckpointable.
6. **Prefix Collision & Binary Guard (`F5`, Critique 6)**: Rewrite `OUT_DIR` paths when anchor/worktree paths share prefixes (`lane-1` vs `lane-10`) and contain Mach-O (`0xFEEDFACF`) binaries, asserting binaries and `lane-10` strings are untouched and `mtime` is preserved.
7. **Sparse Checkout Escape & Tampered Index (`F11`, Critique 4)**: Tamper with `<git_dir>/index` `SKIP_WORKTREE` bits, add an unmaterialized out-of-cone file and an escaping/broken symlink, and assert `WorkingTreeSubject` catches tamper and symlink escape while hashing legitimate cone + `HEAD` files.
8. **StackedStore Masking & Lease GC (`P2`)**: Publish `0444` base, edit/delete symbols in stacked lane, verify `WorktreeOverlay` masks deleted base symbols and serves new Tree-sitter edges, and verify `gc_unused_bases` retains leased bases and deletes unleased ones.
9. **Proc-Macro Poisoning (`F14`, `OQ4`)**: Test Stage-2 cache key with `serde_derive` (pinned hash -> normalized sentinel) vs `sqlx_macros` / unknown `.dylib` / `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), ...))` (must include raw `CARGO_MANIFEST_DIR`).
10. **Multi-Lane Harness (`F16`, `P3.3`)**: Execute concurrent `Manvi` runs on sibling worktrees of one repo and invoke `workspace: "worktree"` subagent coordination.
