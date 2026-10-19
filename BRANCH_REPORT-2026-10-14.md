# Branch consolidation — 2026-10-14

Local only; nothing pushed. `main` @ `d371a620`. `BRANCH_REPORT.md` beside this file belongs to another session and was not touched.

Defaults agreed before starting: dirty worktrees with uncommitted work from another session (main, lane/workbench, lane/policy) count as **in use** and were not committed; the per-merge gate is the touched crates'/packages' tests, with `ci:local` run once at the end; local branches only.

## Summary

- **Merged into main this session:** 19 `audit2/*` branches. They went through an integration branch, `audit2/integration`, which was fast-forwarded into main (`e79c2610..d371a620`).
- **Mergeable now:** none. The 4 remaining `audit2/*` branches merge cleanly (`git merge-tree --write-tree`), but git refuses them because main's working tree has uncommitted edits in two files they touch.
- **Deleted:**
  - the 19 merged `audit2/*` branches, plus `audit2/integration`;
  - their 20 worktrees, removed with `git worktree remove` and never `--force`;
  - 19 `worktree-agent-*` placeholder branches (fully merged, created by the agent-worktree tool): 15 in the earlier cleanup, 4 in this pass.
- **Superseded or abandoned:** none. Every unmerged commit shows as `+` in `git cherry main <branch>`, meaning it is real work not already on main.
- **Kept:** main, the 4 blocked `audit2/*` branches, `claude/cranky-pare-0866b9`, `claude/silly-morse-7c6a7b`, `lane/policy`, `lane/workbench`.

## Inventory (main @ d371a620, before deletions in this pass)

GitPulse Insights reported:
- 9 of 9 worktrees scanned, 0 collisions.
- 3 live Claude sessions:
  - pid 3121, this session, in the main checkout;
  - pid 57016 (idle desktop) in `cranky-pare-0866b9`;
  - pid 56829 (idle desktop) in `silly-morse-7c6a7b`.

| branch | ahead | behind | last commit | merged | worktree | dirty | `git cherry` +/− | task (status) | class |
|---|---|---|---|---|---|---|---|---|---|
| audit2/disk-writes | 1 | 40 | 2026-10-14 BHARATH CHANDRA VADDARAM, bac12dcd | no | .claude/worktrees/agent-aee4ff91453c92f64 | 0 | +1/−0 | ft-02f43146 (review) | mergeable, **blocked** |
| audit2/store-lifecycle | 2 | 40 | 2026-10-14, 6897dc9d | no | agent-a8a1b52601cba078f | 0 | +2/−0 | ft-c13da1b7 (review) | mergeable, **blocked** (stacked on disk-writes) |
| audit2/daemon-backslash | 3 | 40 | 2026-10-14, d9154f29 | no | agent-acf89db45f450b0cd | 0 | +3/−0 | ft-359e6751 (review) | mergeable, **blocked** (stacked on store-lifecycle) |
| audit2/partial-parse | 1 | 42 | 2026-10-14, 2d9ee4d4 | no | agent-af0dbcd782f62925d | 0 | +1/−0 | ft-cc6c12cb (review) | mergeable, **blocked** |
| claude/cranky-pare-0866b9 | 0 | 42 | 2026-10-12, a90b17fd | yes | .claude/worktrees/cranky-pare-0866b9 | 0 | 0 | — | in use (live session) |
| claude/silly-morse-7c6a7b | 0 | 41 | 2026-10-13, e79c2610 | yes | .claude/worktrees/silly-morse-7c6a7b | 0 | 0 | — | in use (live session) |
| lane/policy | 0 | 56 | 2026-10-12, d78cd9de | yes | ../DevCouncil-l4-policy | 1 (untracked `gate/zz_probe_test.go`) | 0 | — | in use (dirty worktree) |
| lane/workbench | 1 | 56 | 2026-10-12, 765db331 | no | ../DevCouncil-l3-workbench | 28 | +1/−0 | ft-aeb4a53f (backlog), ft-4966b304 (backlog, "IN-FLIGHT lane/workbench only") | in use (dirty, in-flight) |
| worktree-agent-a8a1b526…, -acf89db4…, -aee4ff91…, -af0dbcd7… | 0 | 41–56 | e79c2610 / d78cd9de | yes | none | — | 0 | — | merged → deleted |

All commits are by BHARATH CHANDRA VADDARAM. Fable reviewed this classification, agreed with it, and changed two things:
- the `audit2` branches are "ready, blocked only by main's working tree";
- do not rebase them (see Blocked).

## Backups

- **Refs:** `refs/backup/2026-10-14-consolidate/<branch>` for every branch, covering:
  - the 13 that existed at the start of this pass, including main @ d371a620;
  - the 19 merged `audit2/*` tips, taken from each merge commit's second parent;
  - `audit2/integration`.
- **Bundle:** `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-14.bundle` (32 MB). It contains every branch. `git bundle verify` reports "The bundle records a complete history." It was written before the last 20 refs above were added, but every commit those refs name is reachable from main, so the bundle has them.
- **Older refs:** `refs/backup/2026-10-13/audit2/*`, created automatically by another tool, are kept. Two of them, `accuracy-gates` (a90b17fd) and `manifests` (86e41976), point at older commits than the merged tips. Use the `2026-10-14-consolidate` refs.

## Branches merged

Each was merged with `git merge --no-ff` into `audit2/integration` (based on e79c2610) in Fable's order, then main was fast-forwarded to `d371a620`.

| branch | merge commit | branch tip | card |
|---|---|---|---|
| audit2/gap-txn-immediate | cc287ac9 | 48df05a2 | ft-a4b3e5f0 |
| audit2/pdg-taint | ad1ac216 | cc8545c8 | ft-08e1753c |
| audit2/residue | 0a098216 | 2f9c887d | ft-81c3e16f |
| audit2/accuracy-gates | 85353f85 | 3aea7839 | ft-4cd88dbe |
| audit2/extract-robustness | 07e8d7c3 | 87823824 | ft-359e6751 |
| audit2/backslash-readers | 0bf68ea9 | be862eaf | ft-359e6751 |
| audit2/lsp-pass | 5b73cc9c | 62c6438f | ft-2e510bf3 |
| audit2/glob-poison | 8c90d97d | 06ca5f7b | ft-7661cacb |
| audit2/lsp-counters | 8afcc8c9 | 2106bfb1 | ft-2e510bf3 / ft-359e6751 |
| audit2/manifests | 1f1f5149 | fc984740 | ft-837a6f05 |
| audit2/daemon-write | c1a2c7fb | bb01a8c2 | ft-3f82536e |
| audit2/daemon-followup | 61367e71 | eefa2f2d | ft-dff2a712 |
| audit2/config-trust | 0c7a04ed | 1955ec58 | ft-b7d531c8 |
| audit2/grants-ttl | 014970b8 | 23cdf38a | ft-fb1fe1ee |
| audit2/mcp-lifecycle | 17909500 | 1d55b10e | ft-09a91ab9 |
| audit2/consumers-env | dac28f5f | 90e37ac1 | ft-bccb7d54 |
| audit2/component-install | afbb8fd2 | 7cc534bc | ft-f8d29003 |
| audit2/go-windows-part1 | a89456d4 | 32c9ebc8 | ft-00755856 |
| audit2/query-cancel | 2b787167 | 18db3836 | ft-9ac4f289 |

Two follow-up commits on the integration branch: `1d58c6ad` (regenerated accuracy record) and `d371a620` (test classification). Every card stays in **review**: done needs Manvi verification. Each card has a note saying it is merged.

## Conflicts and how they were resolved

- **`rust/testdata/accuracy/record.json`** (accuracy-gates × manifests) was the only textual conflict, in the `fixtures` section (Kotlin and Rust rows).
  - The merge commit took the accuracy-gates side; nobody edited it by hand.
  - After all merges it was regenerated with the harness's own command, `DEVMAP_ACCURACY_BLESS=1 cargo test -p devmap-analyze --test accuracy_fixtures`, in commit `1d58c6ad`.
  - Result: Kotlin 61/61, Rust 110/113. The accuracy-gates `forbid_any_tier_rows` fields are kept.
  - Why: both sides changed generated numbers, so the correct value is whatever the merged code now produces, not either side's text.
  - Not regenerated: the `corpora`, `measurements` and `open_gates` sections. They need private pinned corpora, so they are unchanged.
- **Merged without textual conflicts:**
  - `ratchet.json` (byte-identical after regeneration);
  - `rust/devmap-resolve/src/resolver.rs` (lsp-pass, glob-poison, lsp-counters, manifests);
  - `rust/devmap-extract/tests/it.rs` (every `mod` line kept);
  - `docs/devmap/README.md`;
  - `grants/grants.go`.
- **Semantic fix, `d371a620`:** manifests added `devmap-extract/tests/cargo_manifests_collect.rs`, which the parse-feature classification test did not know about. It was confirmed to compile and pass with the `parse` feature off (4 tests), then added to `FEATURE_OFF_SAFE`. This was a gap in the manifests branch itself, not a merge resolution.
- **Review of the resolutions:** the integration agent (Opus) made and checked them. There was no separate Opus review pass: the only hand decision was the `record.json` side, which was then regenerated rather than kept.

## Verification

**Integration branch, before main moved:**
- `cargo build --workspace`: ok.
- Crates tested: devmap-resolve 721, devmap-extract 961, devmap-analyze 339, devmap-query 739, devmap-store 295, devmap-serve 390, devmap-cli 766, dc-store 294, dc-proc 42, dc-glob 23, dc-verify 152. The two failures below are the only ones.
- **Failures already on main before these branches:**
  - devmap-cli `every_integration_test_is_classified_against_the_parse_feature`: `dc-regress tests/change_paths.rs` is unclassified.
  - devmap-extract `every_child_process_in_the_kernel_goes_through_the_runner`: a direct `git` call at `devmap-serve/src/blast_cancel_tests.rs:17`.
- **Flaky test:** `hook::tests::a_session_end_that_overlaps_another_report_still_runs_its_own` failed once and passed alone and on a full rerun. No merged branch touches `hook.rs`.
- **Go:**
  - `go build ./...`, `go vet ./...` and `GOOS=windows go vet` on verify and safefile: ok.
  - `go test ./...`: 30 packages pass.
  - `dc/devmap` fails only under the scratch `go.work` the worktree needed; with `GOWORK=off` it passes.

**main @ d371a620 after the fast-forward:**
- `cargo test --manifest-path rust/gusset-engine/Cargo.toml`: 23 + 1 + 0 pass.

**`ci:local` (`npm run ci:local`), run in the main checkout:**
- **What it ran on:** the main checkout includes the other session's 31 uncommitted files. A clean sibling checkout was refused: the shell gate blocks running node outside the repo, and the in-repo alternative needed a symlink under protected `.claude/`.
- **Passed:** script tests (62/62), release gate tests, `build dcrelease`, release identity.
- **Stopped:** at `release identity (tag path)`, with `FAIL: tag v0.2.4 names 6b889fd765ff but HEAD is d371a6202a84 … Retag at HEAD, or push a different tag.`
  - This is the known failure tracked in ft-7cb2d308.
  - The Go and Rust steps come after it and **did not run** under `ci:local`. The per-crate and per-package runs above are the evidence for those.
  - Retagging is a release decision, so it is out of scope here.

**Repository and session checks:**
- `git fsck`: no errors (142 dangling objects only).
- GitPulse Insights afterwards: the same 3 live sessions on the same worktrees and branches, 0 collisions, and main's 31 dirty files unchanged. No live session lost its branch.

## Branches deleted

Every deleted branch was merged into main and has a backup ref. All were deleted with `git branch -d`; `-D` was never used.

| branch | was | backup ref |
|---|---|---|
| audit2/{gap-txn-immediate, pdg-taint, residue, accuracy-gates, extract-robustness, backslash-readers, lsp-pass, glob-poison, lsp-counters, manifests, daemon-write, daemon-followup, config-trust, grants-ttl, mcp-lifecycle, consumers-env, component-install, go-windows-part1, query-cancel} | tips in the merged table | `refs/backup/2026-10-14-consolidate/audit2/<name>` |
| audit2/integration | d371a620 (= main) | `refs/backup/2026-10-14-consolidate/audit2/integration` |
| worktree-agent-a8a1b52601cba078f, -acf89db45f450b0cd | e79c2610 | `refs/backup/2026-10-14-consolidate/worktree-agent-…` |
| worktree-agent-aee4ff91453c92f64, -af0dbcd782f62925d | d78cd9de | `refs/backup/2026-10-14-consolidate/worktree-agent-…` |
| 15 other worktree-agent-* placeholders (earlier pass) | at a90b17fd / e79c2610, all ancestors of main | no individual ref; their commits are reachable from main and are in the bundle |

The worktrees of the 20 deleted `audit2` branches were removed with `git worktree remove`; each was clean.

## Branches kept

| branch | reason |
|---|---|
| main | main |
| audit2/disk-writes, store-lifecycle, daemon-backslash, partial-parse | merge-ready but blocked (see Blocked); cards in review |
| claude/cranky-pare-0866b9, claude/silly-morse-7c6a7b | live desktop sessions; already merged, nothing to do |
| lane/policy | already merged, but its worktree holds an untracked `gate/zz_probe_test.go` that belongs to someone else. Deleting the branch would strand the worktree. |
| lane/workbench | 28 dirty files and an in-flight task. Its unmerged commit `765db331` bumps dc-store `SCHEMA_VERSION` 9→10 and `WORKBENCH_SCHEMA` 13→14, so merging it falls under the cross-app schema rule (GitPulse, Manvi, Jarvis) and needs your sign-off. |

There are no release branches.

## Blocked

The four `audit2` branches merge cleanly against committed main (`git merge-tree --write-tree` trees: disk-writes `d5165fda`, store-lifecycle `3e27a41a`, daemon-backslash `2c3987e4`, partial-parse `93782223`). Git refuses the merge only because these files are modified, uncommitted, in main's working tree:
- `rust/dc-proc/src/lib.rs` (disk-writes and the stack above it);
- `rust/devmap-analyze/src/liveness.rs` (partial-parse).

Fable compared the hunks: main's edits sit at lib.rs:659 and liveness.rs:1406, the branches' edits at lib.rs:66 and liveness.rs:2309–2455, so no textual conflict is expected.

**Needed:** the session that owns main's uncommitted edits commits them, or you authorise a stash. This pass did neither. Then:

```
git merge --no-ff audit2/daemon-backslash   # brings disk-writes and store-lifecycle with it
cargo test -p dc-proc -p devmap-serve -p devmap-store   # from rust/
git merge --no-ff audit2/partial-parse
cargo test -p devmap-analyze -p devmap-cli
```

**Follow-ups and decisions:**
- **Rebase:** Fable advised against it. It would buy no conflict resolution and would rewrite commits that cards in review reference.
- **`schema.rs` overlap:** main's uncommitted `dc-store/src/schema.rs` (retired-table removal) and lane/workbench's commit both edit the DDL / `migrate_inner` region. Re-run `git merge-tree` for lane/workbench after main's edits are committed.
- **Duplicated rule:** `audit2/daemon-backslash` carries a local copy of the backslash rule. Once it merges, replace the copy with `devmap_extract::repo_relative_key`, now on main. This is a code change, not consolidation.
- **Version bumps still awaiting sign-off:** D-14, extraction 89→90, the `mut`-binding output change, and the store schema change.

## Update: the four blocked branches merged ("merge all")

The user said "merge all". The four `audit2` branches were merged; lane/workbench was not.

**Setting main's uncommitted edits aside** (the other session's work was never committed):
- Stash 1 held `rust/dc-proc/src/lib.rs` and `rust/devmap-analyze/src/liveness.rs`. Backup ref: `refs/backup/2026-10-14-consolidate/main-dirty-stash`.
- Stash 2 held the staged mode change on `rust/gusset-engine/check-pin.sh` (100644 → 100755). Git refuses a non-fast-forward merge while anything is staged. Backup ref: `refs/backup/2026-10-14-consolidate/main-staged-stash`.

**Merges:**
- `1e8d082e`: `git merge --no-ff audit2/daemon-backslash`, which brought in disk-writes and store-lifecycle (ft-02f43146, ft-c13da1b7, ft-359e6751).
- `e7a70d20`: `git merge --no-ff audit2/partial-parse` (ft-cc6c12cb).
- Neither had conflicts.

**Restoring the edits:**
- Stash 2 was popped with `--index`, so the mode change is staged again.
- Stash 1 was popped and auto-merged with no conflict.
- Afterwards: the same diff stats (2 files, +7/−8), the same staged mode change, no stash entries left. Main still has the other session's 31 files, plus this report.

**Verification**, on a clean isolated checkout of `e7a70d20`:
- `cargo build --workspace`: ok.
- Tests passed: dc-proc 44, devmap-serve 407, devmap-store 297, devmap-analyze 340, devmap-cli 768, devmap-extract 961, devmap-query 739, devmap-soak 51.
- The only failures are the two that already exist on main.
- rustfmt is clean on the 17 merged `.rs` files. Main's existing drift remains at `liveness.rs:1406`, which is unchanged since d371a620.

**Deleted:** `audit2/disk-writes`, `audit2/store-lifecycle`, `audit2/daemon-backslash`, `audit2/partial-parse` (`git branch -d`), their worktrees and the verification worktree. Their backup refs are under `refs/backup/2026-10-14-consolidate/audit2/`. The four cards have a merge note and stay in review.

**Left** (reasons in the Branches kept table): main, `claude/cranky-pare-0866b9`, `claude/silly-morse-7c6a7b`, `lane/policy`, `lane/workbench`.
- lane/workbench is still held. Its commit `765db331` bumps dc-store `SCHEMA_VERSION` 9→10 and `WORKBENCH_SCHEMA` 13→14. Under the cross-app rule that bump must land together with the GitPulse, Manvi and Jarvis updates. Its worktree also has 28 uncommitted in-flight files.

## Update: lane/workbench test-merged, not merged

The user said "continue to merge all". lane/workbench was test-merged into e7a70d20 on a throwaway branch, `audit2/workbench-merge-test` @ e9109f54. It merged with no conflicts, but **main was not moved**.

**Results:**
- Rust builds, and dc-store passes 316 tests.
- **Go: 57 tests that pass on e7a70d20 fail after the merge**, across dc/store, devcouncil, mcp, stopgate, verify and policy.

**Why:**
- The Go store client hard-codes `SchemaVersion = 9` (`dc/store/store.go:643`) and refuses the schema-10 store: `dcstore speaks schema 10, this harness speaks 9`. That also breaks the production host.
- `Acquire` now refuses task ids that are not workbench items.
- Go tests still seed the removed `tasks` table: `no such table: tasks`.
- `backend/contracts/CHECKSUMS` is stale for `lease.schema.md`.

The Go half of this change looks like lane/workbench's 28 uncommitted files (store.go, tasks.go, lease.go, registry.go, testsupport and the failing tests), but that is not verified. Outside this repo, GitPulse, Manvi and Jarvis also need the schema 10 / workbench 14 bump.

**Cleanup:** the throwaway branch was deleted with `-D` because it was superseded: it holds only a merge of 765db331, which `lane/workbench` still has. Its worktree and placeholder branch were also removed.

**Decision needed from the user:** whether to commit lane/workbench's uncommitted Go half and merge both together, which also needs the cross-app bump and pin sign-off.

## Restore commands

```
# any branch deleted in this pass
git branch <name> refs/backup/2026-10-14-consolidate/<name>
# e.g.
git branch audit2/query-cancel refs/backup/2026-10-14-consolidate/audit2/query-cancel

# from the bundle (e.g. into a fresh clone)
git fetch /Users/bharath/Code/devtools/DevCouncil-branches-2026-10-14.bundle 'refs/heads/*:refs/remotes/bundle/*'

# list every backup
git for-each-ref refs/backup/2026-10-14-consolidate/
```
