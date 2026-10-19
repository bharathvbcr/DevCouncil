# Branch consolidation — 2026-10-14 pass 2

Local only. Nothing was pushed. `main` is still `e7a70d204c5e1ba10e84cbe4b0b204efcedd2f82`, 47 commits ahead of `origin/main`.

This pass merged nothing, resolved no conflicts, committed nothing, and deleted nothing. Fable reviewed the classification and agreed: every remaining local branch stays. The earlier merge of the `audit2/*` branches is recorded in `BRANCH_REPORT-2026-10-14.md`. The 2026-10-13 report that used to live in this file is now `BRANCH_REPORT-2026-10-13.md`.

## What the checks said

`gitpulse_insights` and `gitpulse_collision_risk` both returned `ok: true` on every facet. Five worktrees scanned, zero unscanned, zero overlapping dirty files, zero shared-worktree files. Claude was observed (2 sessions). Codex was observed (0 sessions). No GitPulse task is `in_progress`. No worktree has a bound `task_id`.

DevMap's index is stale (`is_fresh: false`, generation 4480, stored extraction payload obsolete), so it was not used to order a merge. That gap is `GAP-BRANCH-PASS2-STALE-INDEX`.

| Worktree | Branch | Dirty | Live session |
| --- | --- | --- | --- |
| `/Users/bharath/Code/devtools/DevCouncil` | `main` | 32 files at the start of this pass (25 unstaged, 1 staged, 6 untracked). This report adds one preserved copy, so the count is now 33. | No Claude or Codex session. This Grok session's cwd is here and did not create the dirt. Other Grok sessions are in other repositories. |
| `/Users/bharath/Code/devtools/DevCouncil-l3-workbench` | `lane/workbench` | 28 | none (no process has this cwd) |
| `/Users/bharath/Code/devtools/DevCouncil-l4-policy` | `lane/policy` | 1 untracked file | none |
| `.../.claude/worktrees/silly-morse-7c6a7b` | `claude/silly-morse-7c6a7b` | 0 | Claude desktop, pid 56829, idle |
| `.../.claude/worktrees/cranky-pare-0866b9` | `claude/cranky-pare-0866b9` | 0 | Claude desktop, pid 57016, idle |

A second insights call after the backup and `ci:local` still shows those two Claude sessions on those two branches. Neither lost its branch.

There are no release branches. Remotes were out of scope. `origin/dependabot/github_actions/actions/upload-artifact-7` was left alone.

## Inventory

Ahead and behind are `git rev-list --left-right --count main...branch` (behind, then ahead). Every last commit is by BHARATH CHANDRA VADDARAM. "Merged" is `git branch --merged main`. "Cherry" is `git cherry -v main <branch>` (`+` is a patch not already on main).

| Branch | Behind | Ahead | Last commit | Merged | Cherry | Worktree | Task | Class |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `main` | 0 | 0 | 2026-10-14 16:14 `-0500`, `e7a70d20`, Merge audit2/partial-parse into main | — | — | this checkout, dirty | none bound | keep |
| `claude/silly-morse-7c6a7b` | 47 | 0 | 2026-10-13 22:44 `-0500`, `e79c2610` | yes | empty | clean, live pid 56829 | none bound | in use |
| `claude/cranky-pare-0866b9` | 48 | 0 | 2026-10-12 15:46 `-0700`, `a90b17fd` | yes | empty | clean, live pid 57016 | none bound | in use |
| `lane/policy` | 62 | 0 | 2026-10-12 09:08 `-0700`, `d78cd9de` | yes | empty | dirty: untracked `backend/go_orchestrator/gate/zz_probe_test.go` | none bound | in use |
| `lane/workbench` | 62 | 1 | 2026-10-12 10:12 `-0700`, `765db331` | no | `+ 765db331` | dirty, 28 files, no live process | no lease. Related card `ft-aeb4a53f` is **backlog** (inferred from the commit and the card title, not from a binding) | keep, blocked |

Fable (`claude --model fable -p`, read-only) agreed with all five classes and with merging nothing. Anything that was not a clean, idle, fully-contained branch stayed keep.

## Backups (this pass, before any delete)

Nothing was deleted, and the backups were still written first.

Refs, each pointing at the tip above:

- `refs/backup/2026-10-14-pass2/main` → `e7a70d20`
- `refs/backup/2026-10-14-pass2/claude/silly-morse-7c6a7b` → `e79c2610`
- `refs/backup/2026-10-14-pass2/claude/cranky-pare-0866b9` → `a90b17fd`
- `refs/backup/2026-10-14-pass2/lane/policy` → `d78cd9de`
- `refs/backup/2026-10-14-pass2/lane/workbench` → `765db331`

Bundle: `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-14-pass2.bundle` (31 MB). `git bundle verify` says the bundle is okay and records a complete history. It contains those five branch tips and the five backup refs, including current `main` at `e7a70d20`.

The older bundle `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-14.bundle` records `main` at `d371a620`, which is an ancestor of `e7a70d20`. Use the pass2 bundle for the current tips. The `refs/backup/2026-10-14-consolidate/` refs from the earlier pass were not pruned.

## Branches merged

None in this pass. No merge commit was created. `git merge-tree --write-tree --name-only main lane/workbench` exits 0 and prints tree `18cdc96bfa464b2562f18306c240e6b7243f649b` with no conflict names, so the committed workbench tip would merge cleanly into committed `main`. It was not merged. See Blocked.

## Conflicts

None. No merge was started, so nothing was resolved with ours or theirs.

## Branches deleted

None in this pass. `git branch -d` and `git branch -D` were not used. No worktree was removed.

The earlier pass already deleted the `audit2/*` branches and some `worktree-agent-*` placeholders. Their backup refs were still present when this pass listed `refs/backup/2026-10-14-consolidate/`. Restore commands for those are below. This pass did not re-check that each of those commits is an ancestor of `main`; the earlier report says they are, and `main` has since moved forward with the two merge commits `1e8d082e` and `e7a70d20`.

## Branches kept

| Branch | Reason |
| --- | --- |
| `main` | Integration branch. Its 32 pre-existing dirty files, including `rust/dc-store/src/schema.rs` and untracked `rust/dc-store/tests/retired_tables.rs`, belong to another session. They were not committed, stashed, or reset. The staged mode change on `rust/gusset-engine/check-pin.sh` was left staged. |
| `claude/silly-morse-7c6a7b` | Already merged (cherry empty). A live Claude session has it checked out. Deleting it would drop that session's branch. |
| `claude/cranky-pare-0866b9` | Same: already merged, live Claude session. |
| `lane/policy` | Commits are merged (cherry empty). The worktree is dirty with a 7-line `TestZZProbe` that only logs `EvaluateCommand`. That probe does not belong on `main`, so it was not committed. `git worktree remove` was not used, because the worktree is dirty. The branch stays while it is checked out there. |
| `lane/workbench` | One real unmerged commit (`git cherry` is `+`, not a patch-id duplicate). The worktree has 28 uncommitted files and no live process. Fable said not to commit them: the card is backlog, the Go half (`doctor.go`, `task.go`, `tasks.go`, `gatescfg/workbench.go`) is unfinished, and committing it would make the branch look done. |
| `origin/dependabot/github_actions/actions/upload-artifact-7` | Remote-only. This pass was local. Not deleted. |

## Blocked

`lane/workbench` at `765db331`.

The commit bumps `SCHEMA_VERSION` from 9 to 10 (`rust/dc-store/src/lib.rs`) and `WORKBENCH_SCHEMA` from 13 to 14 (`rust/dc-store/src/workbench/mod.rs`). Landing that on `main` is a schema change. The standing rule is that GitPulse, Manvi, and Jarvis pick up the new number in the same piece of work, and that a pin bump is asked for first. Neither has happened. The card `ft-aeb4a53f` (backlog) also says to sequence this after `ft-7cb2d308`, which is still in review.

A merge in this checkout would also have to overwrite or conflict with the other session's uncommitted `schema.rs`. That file drops eight unused tables (`assumptions`, `critique_findings`, `project_state`, `shell_sessions`, `shell_command_events`, `file_change_events`, `semantic_diffs`, `correction_manifests`). The workbench commit edits the same file for a different migration. Those two edits were not mixed.

Needed before a later merge:

1. The session that owns `main`'s uncommitted `schema.rs` commits it or discards it.
2. You say the schema bump may land, and the consumer pins are updated in GitPulse, Manvi, and Jarvis.
3. `ft-7cb2d308` is no longer the blocker the workbench card names.
4. The 28 dirty files on `lane/workbench` are either committed by their author or explicitly left out of the merge.

`ci:local` is also red on current `main`, for a reason that is not this branch. See Verify.

## Verify

`npm run ci:local` in this checkout (dirty `main`, so the steps that ran saw the other session's files):

- script tests: 62 passed, 0 failed
- `go test` of `devcouncil/release` and `cmd/dcrelease`: ok
- `dcrelease` build: ok
- release identity: `OK: product version 0.2.4 is consistent`
- **stopped** at release identity (tag path), exit 1: `tag v0.2.4 names 6b889fd765ff but HEAD is e7a70d204c5e`

The script exits there. Workflow lint, npm pack, gofmt, the Go host tests, `go test ./...`, and `rust/verify.sh --quick` did not run. Retagging `v0.2.4` is a release decision (`ft-7cb2d308`). This pass did not move the tag.

`git fsck --no-progress` exited 0. Its output is dangling commits, trees, blobs, and tags only. No missing or corrupt objects.

Insights afterwards: the same two live Claude sessions, on the same branches, zero collisions.

## Restore

Nothing from this pass needs restoring. If a later delete removes one of today's branches:

```
git branch <name> refs/backup/2026-10-14-pass2/<name>
```

Or from the bundle, outside a checkout that already has the branch:

```
git fetch /Users/bharath/Code/devtools/DevCouncil-branches-2026-10-14-pass2.bundle <name>:<name>
```

Branches the earlier pass already deleted, refs verified still present:

```
git branch audit2/accuracy-gates        refs/backup/2026-10-14-consolidate/audit2/accuracy-gates
git branch audit2/backslash-readers     refs/backup/2026-10-14-consolidate/audit2/backslash-readers
git branch audit2/component-install     refs/backup/2026-10-14-consolidate/audit2/component-install
git branch audit2/config-trust          refs/backup/2026-10-14-consolidate/audit2/config-trust
git branch audit2/consumers-env         refs/backup/2026-10-14-consolidate/audit2/consumers-env
git branch audit2/daemon-backslash      refs/backup/2026-10-14-consolidate/audit2/daemon-backslash
git branch audit2/daemon-followup       refs/backup/2026-10-14-consolidate/audit2/daemon-followup
git branch audit2/daemon-write          refs/backup/2026-10-14-consolidate/audit2/daemon-write
git branch audit2/disk-writes           refs/backup/2026-10-14-consolidate/audit2/disk-writes
git branch audit2/extract-robustness    refs/backup/2026-10-14-consolidate/audit2/extract-robustness
git branch audit2/gap-txn-immediate     refs/backup/2026-10-14-consolidate/audit2/gap-txn-immediate
git branch audit2/glob-poison           refs/backup/2026-10-14-consolidate/audit2/glob-poison
git branch audit2/go-windows-part1      refs/backup/2026-10-14-consolidate/audit2/go-windows-part1
git branch audit2/grants-ttl            refs/backup/2026-10-14-consolidate/audit2/grants-ttl
git branch audit2/integration           refs/backup/2026-10-14-consolidate/audit2/integration
git branch audit2/lsp-counters          refs/backup/2026-10-14-consolidate/audit2/lsp-counters
git branch audit2/lsp-pass              refs/backup/2026-10-14-consolidate/audit2/lsp-pass
git branch audit2/manifests             refs/backup/2026-10-14-consolidate/audit2/manifests
git branch audit2/mcp-lifecycle         refs/backup/2026-10-14-consolidate/audit2/mcp-lifecycle
git branch audit2/partial-parse         refs/backup/2026-10-14-consolidate/audit2/partial-parse
git branch audit2/pdg-taint             refs/backup/2026-10-14-consolidate/audit2/pdg-taint
git branch audit2/query-cancel          refs/backup/2026-10-14-consolidate/audit2/query-cancel
git branch audit2/residue               refs/backup/2026-10-14-consolidate/audit2/residue
git branch audit2/store-lifecycle       refs/backup/2026-10-14-consolidate/audit2/store-lifecycle
```

Those commits are also inside the pass2 bundle only if they are ancestors of one of the five current tips. `e7a70d20` contains the merged `audit2` history. The older bundle lists the pre-delete branch tips by name. Prefer `refs/backup/2026-10-14-consolidate/` when restoring one of those names.

# Branch consolidation — 2026-10-15 pass 3

Local only. Nothing was pushed. `main` is still `e7a70d204c5e1ba10e84cbe4b0b204efcedd2f82`. This pass merged nothing, committed nothing, and deleted nothing. The pass 2 text above is unchanged.

## What the checks said

`gitpulse_insights` and `gitpulse_collision_risk` both returned `ok: true` on every facet. Nine worktrees scanned, zero unscanned, zero failed. `sessions_ok` true. Claude was observed (2 idle sessions, pids 57016 and 56829). Codex was observed (0). No GitPulse task is `in_progress`.

One overlapping dirty file: `rust/devmap-analyze/src/liveness.rs`, shared by this checkout (`main`) and the interface worktree. `shared_worktree_files` is 0.

DevMap `devmap_status` on this checkout: generation 4500, `analyzer_freshness` true, `is_fresh` true, `degraded_reason` null, `repository.root` `/Users/bharath/Code/devtools/DevCouncil`. GitPulse codeintel on the same store and generation: `analyzer_freshness` false, `is_fresh` false, reason "stored extraction payload is obsolete". That disagreement is `GAP-BRANCH-PASS3-FRESHNESS`. The index was not used to choose a merge.

| Worktree | Branch | Dirty | Live session |
| --- | --- | --- | --- |
| this checkout | `main` | 34 (25 unstaged, 1 staged, 8 untracked) | none |
| `DevCouncil-l3-workbench` | `lane/workbench` | 28 | none |
| `DevCouncil-l4-policy` | `lane/policy` | 1 | none |
| `.claude/worktrees/cranky-pare-0866b9` | `claude/cranky-pare-0866b9` | 0 | Claude desktop, pid 57016, idle |
| `.claude/worktrees/silly-morse-7c6a7b` | `claude/silly-morse-7c6a7b` | 0 | Claude desktop, pid 56829, idle |
| interface `…c631ed26` | `gitpulse/devmap-has-no-interface-protocol-trait-d-c631ed26` | 20 | none |
| L-Go `…79a53535` | `gitpulse/l-go-devmap-go-accuracy-to-its-core-rece-79a53535` | 0 | none |
| L-Go `…c75b63d0` | `gitpulse/l-go-devmap-go-accuracy-to-its-core-rece-c75b63d0` | 11 | none |
| L-Rust `…e7f8ff3f` | `gitpulse/l-rust-devmap-rust-accuracy-to-its-core-e7f8ff3f` | 0 | none |

## Inventory

Ahead and behind are `git rev-list --left-right --count main...branch` (behind, then ahead). "Merged" is `git branch --merged main`. "Cherry" is `git cherry -v main <branch>`.

| Branch | Behind | Ahead | Tip | Merged | Cherry | Class |
| --- | --- | --- | --- | --- | --- | --- |
| `main` | 0 | 0 | `e7a70d20` | — | — | keep |
| `claude/silly-morse-7c6a7b` | 47 | 0 | `e79c2610` | yes | empty | in use |
| `claude/cranky-pare-0866b9` | 48 | 0 | `a90b17fd` | yes | empty | in use |
| `lane/policy` | 62 | 0 | `d78cd9de` | yes | empty | in use, dirty |
| `gitpulse/devmap-has-no-interface-protocol-trait-d-c631ed26` | 0 | 0 | `e7a70d20` | yes | empty | in use, 20 dirty |
| `gitpulse/l-go-…-79a53535` | 0 | 0 | `e7a70d20` | yes | empty | checked out, clean, no unique commits |
| `gitpulse/l-go-…-c75b63d0` | 0 | 0 | `e7a70d20` | yes | empty | in use, 11 dirty |
| `gitpulse/l-rust-…-e7f8ff3f` | 0 | 2 | `fc63f951` | no | `+ d7b83bdf`, `+ fc63f951` | keep, blocked |
| `lane/workbench` | 62 | 1 | `765db331` | no | `+ 765db331` | keep, blocked |

`git merge-tree --write-tree --name-only main gitpulse/l-rust-…-e7f8ff3f` exits 0, tree `fab392b3c41845c4596132661fd9afe1a51f0d5b`, no conflict names. `git merge-tree --write-tree --name-only main lane/workbench` exits 0, tree `18cdc96bfa464b2562f18306c240e6b7243f649b`, no conflict names. A clean committed merge-tree is not permission to merge either branch. See Blocked.

The three audit2 tips pass 2 left blocked are ancestors of `main` now. `git rev-list --left-right --count` is ahead 0, and `git cherry -v main <tip>` is empty:

- `audit2/disk-writes` `bac12dcd`
- `audit2/store-lifecycle` `6897dc9d`
- `audit2/daemon-backslash` `d9154f29`
- `audit2/partial-parse` `2d9ee4d4` (already recorded as merge `e7a70d20`)

`refs/backup/2026-10-13/audit2/manifests` (`86e41976`, subject "WIP manifests") is not an ancestor of `main` and `git cherry -v main 86e41976` is `+`. Its successor `fc984740` is an ancestor of `main`, and the files that commit introduced (`cargo_manifests_collect.rs`, `kotlin-flavors/edges.tsv`, `rust-cargopaths/edges.tsv`) are on `main`. The WIP was not merged again.

The 32 review SHAs named in `BUILD_REPORT.md` (`c93af111` through `1c5e440a`) are missing objects in this checkout. There is no branch to merge.

## Branches merged

None.

## Conflicts

None. No merge was started.

## Branches deleted

None. No worktree was removed. No backup ref was written, because nothing was deleted.

## Blocked

`gitpulse/l-rust-devmap-rust-accuracy-to-its-core-e7f8ff3f` at `fc63f951`, parent chain `fc63f951` → `d7b83bdf` → `e7a70d20`. Card `ft-f9665721`. The worktree is clean and has no live session. The two commits delete the three Rust rows from `known_failing.tsv` (`rs.c04`, `rs.c24b`, `rs.c26e`) and edit 14 files.

A merge in this checkout would overwrite the other session's uncommitted `rust/devmap-analyze/src/liveness.rs`. That dirty hunk is a wrap inside `unconfirmed_dispatch_reason` (3 insertions, 4 deletions at the old line 1406). The branch's hunk is different: it adds `SymbolKind::Macro` to `is_never_dead_candidate`. Those edits were not mixed, and the dirty file was not stashed or reset.

The interface worktree, checked out at `e7a70d20` with no live session, holds uncommitted edits to 7 of the 14 files the branch changes:

- `docs/devmap/DIVERGENCES.md`
- `rust/devmap-analyze/src/liveness.rs` (+15/−6 there, against +3/−4 on main and the Macro hunk on the branch)
- `rust/devmap-extract/src/cache.rs`
- `rust/devmap-extract/src/treesitter.rs` (+59/−5 there, +297 on the branch)
- `rust/devmap-extract/tests/embedded_scripts.rs`
- `rust/devmap-extract/tests/extract_bindings.rs`
- `rust/devmap-resolve/src/resolver.rs` (+131/−17 there, +172 on the branch)

`lane/workbench` at `765db331` stays blocked for the reason in pass 2. The commit message records dc-store schema 10 and workbench schema 14. The worktree still has 28 dirty files. The schema-migration stop was not lifted. `main`'s uncommitted `schema.rs` was not mixed with it.

Dirty trees that sit on `e7a70d20` were not committed: the interface worktree (20 files), the L-Go worktree `…c75b63d0` (11 files, including untracked `tools/accuracy/**` and `go.labels.json`), and `lane/policy`'s untracked `zz_probe_test.go`.

## Verify

No merge landed, so `npm run ci:local` was not run again. Pass 2 already recorded it stopping at the `v0.2.4` tag check. The L-Rust commits were not built or tested in this pass. The uncommitted stress test on main was left as wave 11 recorded it.
