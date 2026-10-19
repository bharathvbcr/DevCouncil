# Branch consolidation — 2026-10-16 pass 3

Local only; nothing was pushed. `main` went from `7f97d468` to `b2c8997a` by fast-forward onto two `--no-ff` merges. Seven branches were deleted, all with `git branch -d`; no `-D` was used. Twenty-two local branches remain. No conflicts came up, so none were resolved. Nothing was committed on anyone's behalf.

The previous report in this file (2026-10-14 pass 2) is now `BRANCH_REPORT-2026-10-14-pass2.md`.

## What the checks said

- **`gitpulse_insights` / `gitpulse_collision_risk` timed out at 60 s**, three times and twice respectively, at load 10–55. There is no GitPulse collision facet for this pass. Live sessions were taken from the process table and `lsof` cwds instead. That is a recorded gap, not a clean result.
- **The DevMap MCP did not connect.** It failed with ENOENT on a test-binary path under a deleted worktree. Ordering did not depend on it, because the two merged branches touch one file each and the files are disjoint.
- **Live sessions:**
  - pid 27036 (Claude, cwd the main checkout) had just finished a worktree cleanup and was idle.
  - pid 29561 is this session.
  - pid 36664, which holds the locks on eight agent worktrees, is **gone**; its last transcript event was 17:51 CDT. Its locks are stale, but it left uncommitted work (below).
  - The desktop sessions 56829 and 57016 from the last report are gone.
- **The main checkout holds another agent's in-progress work:** 35 entries, including a staged `rust/gusset-engine/check-pin.sh` and edits to `dc-store/src/schema.rs`, `liveness.rs`, `dc-regress/*` and `devmap-serve/*`. It was not committed, stashed or touched. Its staged and unstaged diffs and untracked list hash identically before and after the fast-forward (`tmp/bc-snap-before.txt` = `tmp/bc-snap-after.txt`).
- **Remotes were out of scope.** `origin/main` is `e79c2610`, about 110 commits behind. There are no release branches.

## Inventory (at the start of the pass, main = 7f97d468)

Ahead and behind are counted against main. Every author is BHARATH CHANDRA VADDARAM. "Cherry" is `git cherry main <b>`: `+` means a patch not on main, `-` means a patch-equivalent commit is on main.

| Branch | Ahead | Behind | Last commit | Merged | Cherry | Worktree (dirty?) | Task | Class → outcome |
|---|---|---|---|---|---|---|---|---|
| accuracy/record-2026-10-15 | 0 | 60 | 10-15 13:09 `651e039b` | yes | — | agent-ac36dd3b (4 untracked `zz_*.rs`) | ft-7327fd12 held for review | merged; dirty worktree → **kept** |
| accuracy/record-2026-10-15-swift-r4 | 0 | 18 | 10-15 22:57 `91db2b1c` | yes | — | agent-aecfb297 (clean) | ft-8ca270ad **in_progress** | in use → **kept** |
| accuracy/record-2026-10-16-swift-r5 | 0 | 3 | 10-16 15:07 `88902fe4` | yes | — | agent-afffbf81 (MM ×5, blobs = main HEAD) | ft-8ca270ad in_progress | in use → **kept** |
| claude/cranky-pare-0866b9 | 0 | 109 | 10-12 15:46 `a90b17fd` | yes | — | none | none | merged → **deleted** |
| claude/silly-morse-7c6a7b | 0 | 108 | 10-13 22:44 `e79c2610` | yes | — | none | none | merged → **deleted** |
| lane/classify-parse-tests | 0 | 10 | 10-16 10:28 `c257908b` | yes | — | agent-a9361147 (2 staged; blobs = main HEAD, unconfirmed by Fable) | ft-c1e2cec7 review | merged; dirty worktree → **kept** |
| lane/classify-parse-tests-c | 0 | 9 | 10-16 12:23 `a5272c60` | yes | — | none | ft-c1e2cec7 review | merged → **deleted** |
| lane/degraded-status | 0 | 11 | 10-16 08:51 `d40fac25` | yes | — | agent-a33eb595 (clean) | ft-47124ed7 in_progress | in use → **kept** |
| lane/swift-recall-r5 | 0 | 12 | 10-16 10:29 `5b496dd6` | yes | — | none | ft-8ca270ad in_progress | in use → **kept** |
| worktree-agent-a654b9c7bef5bc53d | 0 | 108 | 10-13 22:44 `e79c2610` | yes | — | agent-a654b9c7 (untracked `swift_recall_r5_attack.rs`, differs from main's copy) | none | merged; dirty worktree → **kept** |
| audit/contract-skip-guard | 1 | 61 | 10-15 10:10 `ca30d273` | no | `+1` | none | ft-2b9adeb0 review | mergeable → **merged, deleted** |
| finish/confident-tier | 2 | 15 | 10-16 08:35 `356d3676` | no | `+1` | none | ft-7327fd12 held | mergeable → **merged, deleted** |
| lane/confident-tier-attack | 1 | 48 | 10-15 15:21 `b30c8b39` | no | `+1` | none | — | contained in finish/confident-tier → **deleted after merge** |
| lane/degraded-status-attack | 1 | 1 | 10-16 08:07 `2a22903f` | no | `-1` (= `d5628d19`) | none | ft-47124ed7 in_progress | superseded, in-progress task → **kept** |
| lane/swift-recall-r5-attack | 1 | 1 | 10-16 10:08 `c24fe674` | no | `-1` (= `10bd7c73`) | none | ft-8ca270ad in_progress | superseded, in-progress task → **kept** |
| finish/rust-accuracy | 3 | 15 | 10-16 09:01 `ed1b44e0` | no | `+2` | none | ft-f9665721 review | **blocked** |
| gitpulse/l-rust-devmap-rust-accuracy-to-its-core-e7f8ff3f | 2 | 61 | 10-15 00:12 `fc63f951` | no | `+2` | none | ft-f9665721 | contained in finish/rust-accuracy → kept |
| finish/ts-interface-dispatch | 3 | 15 | 10-16 09:00 `5f4709df` | no | `+2` | none | ft-e3f7d1b1 review | **blocked** |
| gitpulse/devmap-has-no-interface-protocol-trait-d-c631ed26 ("S5") | 1 | 61 | 10-16 08:29 `93b187b2` | no | `+1` | none | ft-e3f7d1b1 | contained → kept |
| finish/go-accuracy | 2 | 15 | 10-16 08:55 `cd384ded` | no | `+1` | none | ft-e221de3c review | **blocked** |
| gitpulse/l-go-devmap-go-accuracy-to-its-core-rece-c75b63d0 | 1 | 61 | 10-16 08:29 `dbf072bd` | no | `+1` | none | ft-e221de3c | contained → kept |
| finish/swift-recall | 8 | 15 | 10-16 09:31 `536bb98b` | no | `+5` | none | ft-8ca270ad in_progress | in use → **kept** |
| lane/swift-recall-attack | 6 | 29 | 10-15 21:11 `9e1acc9e` | no | `+2 -2` | none | ft-8ca270ad | contained in finish/swift-recall → kept |
| lane/swift-recall-fixes | 2 | 29 | 10-15 21:11 `e8f79596` | no | `+1 -1` | none | ft-8ca270ad | contained → kept |
| integration/stale-2026-10-16 | 26 | 26 | 10-16 09:48 `04b26f48` | no | `+13` | none | union of the above | **blocked** |
| finish/workbench | 4 | 123 | 10-16 09:15 `d74379fb` | no | `+3` | none | ft-aeb4a53f backlog | **blocked** |
| lane/workbench | 2 | 123 | 10-16 08:29 `d88688b5` | no | `+2` | none | ft-aeb4a53f | contained in finish/workbench → kept |
| main | 0 | 0 | 10-16 15:10 `7f97d468` | — | — | main checkout, 35 entries of another agent's work | — | kept |

Fable (read-only) reviewed this classification. It agreed with every class and spot-checked the cherry, containment, merge-tree and schema claims. It added two corrections, both folded in above:
- The `liveness.rs` overlap with `finish/rust-accuracy` is disjoint (rustfmt-only at ~L1407 against `is_never_dead_candidate` at ~L25). The real blocker is the schema collision.
- Fable could not confirm the "staged blobs = main HEAD" claim for `agent-a9361147`, so `lane/classify-parse-tests` stays kept.

## Merged

| Merge commit | Branch (tip) | Content | Conflicts |
|---|---|---|---|
| `417f70f4` | audit/contract-skip-guard (`ca30d273`), ft-2b9adeb0 | `.github/workflows/analysis-plane.yml` +26: fail the job when the flag and state-dir contract tests skip | none |
| `b2c8997a` | finish/confident-tier (`356d3676`) | `rust/devmap-analyze/src/dead_tier.rs` +15: attack probes for the confident-branch predicate (0.89 boundary) | none |

Both merges were made with `--no-ff` in a clean sibling worktree (`../DevCouncil-pass3`, now removed). main was then fast-forwarded with `git merge --ff-only`, because the main checkout held another agent's staged and unstaged work. Neither merged path was dirty there.

Fable's note for the owner: the dead_tier probe records the 0.89 decimal-versus-f32 mismatch as a "documented hazard". ft-7327fd12 is held for review, so look at it there. The workflow guard only proves itself when GitHub CI runs it; `ci:local` lints it but does not execute it.

### Verification of `b2c8997a` (load 13–55)

**`npm run ci:local` is red at step 4, "release identity (tag path)":**

```
FAIL: tag v0.2.4 names 6b889fd765ff but HEAD is b2c8997a0689
```

This predates the merge: main `7f97d468` is not at that tag either, and it is the "ci:local stops at the tag check" item on ft-7cb2d308. Steps 1–3 passed. I ran the later steps myself, on the same commit:

| Step | Result |
|---|---|
| workflow lint | OK, 8 files, including `analysis-plane.yml` |
| npm pack, npm runtime smoke | pass |
| gofmt / gusset pin | **red against the shared `../gusset`**: it is at `320c56c2`, CI pins `984209b5`. Rerun against a scratch gusset clone at the pin: ok |
| gusset engine, go host tests, go orchestrator tests | pass (scratch layout, gusset at the pin) |
| `verify.sh --quick` 1/8 fmt | **red**: rustfmt drift in 23 files. All predate the merge; neither merged file is among them |
| 2/8 clippy `-D warnings` | **red**: 1 file, `dc-proc/src/lib.rs`, predates the merge. 0 hits in `dead_tier.rs` |
| 3/8 `cargo test --workspace --no-fail-fast` | 5,383 pass, 6 fail, 22 ignored. See the next table |
| `devmap-analyze --lib dead_tier` | 8/8 pass, including the merged probes |

| Failing test | What it is |
|---|---|
| `dc-verify json_contract` ×2 | I hadn't built the host binary, which `verify.sh` does. With it built: 2/2 pass |
| `devmap-extract every_child_process_in_the_kernel_goes_through_the_runner` | Known failure from before this pass (BUILD_REPORT Part 5) |
| `devmap-cli over_limit…a_watched_file_growing_past_the_limit_settles_without_retries` | Known flake. Interleaved A/B: fails on main `7f97d468` 2/2, on `b2c8997a` 1/2 |
| `devmap-cli global_excludes…a_path_named_in_core_excludes_file…` | Passes when rerun alone (load) |
| `devmap-serve watcher…sustained_filtered_noise…` | Passes when rerun alone (load) |

The only Rust change is a `#[cfg(test)]` block inside `devmap-analyze`, which dependent crates never compile. No failure above can come from it.

**The final `ci:local` in the main checkout was not run.** That checkout carries 35 uncommitted entries from another agent, so a run there would test their work, not `main`. `main` is `b2c8997a`, the exact commit verified above.

## Deleted

All seven were reachable from main at deletion and deleted with `git branch -d`. None had a worktree.

| Branch | Tip | Backup ref |
|---|---|---|
| claude/cranky-pare-0866b9 | `a90b17fd` | `refs/backup/2026-10-16-pass3/claude/cranky-pare-0866b9` |
| claude/silly-morse-7c6a7b | `e79c2610` | `refs/backup/2026-10-16-pass3/claude/silly-morse-7c6a7b` |
| lane/classify-parse-tests-c | `a5272c60` | `refs/backup/2026-10-16-pass3/lane/classify-parse-tests-c` |
| audit/contract-skip-guard | `ca30d273` | `refs/backup/2026-10-16-pass3/audit/contract-skip-guard` |
| finish/confident-tier | `356d3676` | `refs/backup/2026-10-16-pass3/finish/confident-tier` |
| lane/confident-tier-attack | `b30c8b39` | `refs/backup/2026-10-16-pass3/lane/confident-tier-attack` |
| merge/pass3-2026-10-16 (this pass's staging branch) | `b2c8997a` | `refs/backup/2026-10-16-pass3/merge/pass3-2026-10-16` |

Worktrees removed with `git worktree remove`: this session's own `../DevCouncil-pass3` and the scratch CI worktree. No other worktree was removed.

## Backups

- There are 29 refs under `refs/backup/2026-10-16-pass3/`: all 28 branch tips at the start of the pass, including `main` at `7f97d468`, plus the staging branch.
- Bundle: `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-16-pass3.bundle` (31 MB). `git bundle verify` reports it okay, with complete history. It contains all 28 branches and their backup refs.

## Kept, and why

- **main.** Its checkout also holds another agent's uncommitted work.
- **In use (task in_progress):**
  - accuracy/record-2026-10-15-swift-r4, accuracy/record-2026-10-16-swift-r5, lane/swift-recall-r5, finish/swift-recall, lane/swift-recall-attack, lane/swift-recall-fixes, lane/swift-recall-r5-attack are bound to ft-8ca270ad.
  - lane/degraded-status and lane/degraded-status-attack are bound to ft-47124ed7.
  - The merged ones lose nothing if deleted later. They are kept because GitPulse could not be asked whether task cards reference their branch names.
- **Merged, but the worktree has unique untracked files:**
  - accuracy/record-2026-10-15 (`zz_budget`, `zz_candidate_dump`, `zz_determinism`, `zz_guard` probes)
  - worktree-agent-a654b9c7bef5bc53d (`swift_recall_r5_attack.rs`, which differs from main's copy)
  - lane/classify-parse-tests (stale index that Fable could not confirm)
- **Contained in a blocked parent:** the three `gitpulse/*` lane snapshots and lane/workbench. Delete them after the parent lands.

## Blocked, and what each needs

- **integration/stale-2026-10-16** (`04b26f48`, the rollup of the finish/* lanes):
  - your choice on the strict corpus-labelling rule that session f16a9227 asked about (label the ~30 rows / soften it to "unmeasured" / land with `record.json` unchanged);
  - "Commit S5" (`c631ed26`) and the L-Go decision (BUILD_REPORT owner decisions 2 and 10);
  - then main merged into it again: it is now 28 behind, and its extraction schema is v92 against main's v90.
- **finish/rust-accuracy:** it bumps `EXTRACTION_SCHEMA_VERSION` 89→90, but main is already at 90 from Swift r5. It needs 91 and the vendor pin bump ft-3018ab76 (owner decision 10).
- **finish/ts-interface-dispatch, finish/go-accuracy:** the S5 and L-Go decisions, the same as above.
- **finish/workbench:** a cross-app dc-store schema change (schema 10 / workbench 14) that must reach GitPulse, Manvi and Jarvis in the same change. main also carries uncommitted `schema.rs` edits from session 36664.

## Final round (2026-10-16 evening)

**Where things ended:**
- main is `9c8b5fa6`. Three branches remain: `main`, `finish/workbench` and `lane/workbench`. The last two are kept by owner decision, as a separate cross-app schema change.
- One worktree remains: the main checkout.
- `git fsck --no-dangling` exits 0.

**The landing.** devcouncil-1e landed it with `git merge --ff-only land/2026-10-16`, `b2c8997a` → `9c8b5fa6`. The landing branch is a `--no-ff` merge of:
- `integration/stale-2026-10-16` (`dba76e68`): the finish/* lanes plus the softened labelling rule, which the owner approved in that session;
- `orphan/finish` (`c67c1b6f`): session 36664's leftover work, finished as 10 commits;
- a clippy fix (`9c8b5fa6`).

Before moving main, it saved the main checkout's 28 uncommitted paths in `stash@{0}`, with the message "pre-land 2026-10-16: main-checkout work of session 36664 …". The checkout now holds only the untracked `*_REPORT*.md` files.

**Verification (this session, scratch worktree, gusset clone at pin `984209b5`):**

`dba76e68`:
- every ci:local step after the tag check passed;
- fmt drift went from 23 files to 20, and the one changed file was no worse;
- `cargo test --workspace`: 5409 pass, 6 fail. Every failure was A/B'd and also fails on main or passes when run alone.

`6ecfd136`:
- the source-scan test passes now;
- fmt drift is down to 10 files;
- the policy, gate, gatescfg and correction Go packages pass, including the 64 KiB JSON-operand bound;
- `cargo test --workspace`: 5426 pass, 2 fail. Both are load flakes that also fail on main.
- Clippy found 3 new lints plus 1 older one. The `dc-proc` error had been hiding them, because a crate that fails stops every crate depending on it from compiling.

`9c8b5fa6`:
- `cargo clippy --workspace --all-targets -D warnings` exits 0, the first clean run;
- the touched files pass rustfmt;
- devmap-analyze and devmap-query: 1166 pass, 0 fail.

**Deleted this round.** Each was reachable from main, deleted with `-d`, and has a backup ref under `refs/backup/2026-10-16-pass3/`. The exception is integration, whose ref is under `…-pass3-post/` because it moved after the first backup.
- `finish/go-accuracy`, `finish/rust-accuracy`, `finish/swift-recall`, `finish/ts-interface-dispatch`
- the three `gitpulse/*` lane snapshots
- `integration/stale-2026-10-16`, `land/2026-10-16`, `orphan/finish`
- `lane/swift-recall-attack`, `lane/swift-recall-fixes`
- `worktree-agent-a0f04921…`, `worktree-agent-a354175f…`, `worktree-agent-af5d5ab5…`

**One force-delete.** `orphan/main-checkout-2026-10-16` (`3218d0b7`), the raw snapshot, was deleted with `-D`. Its 4 files that differ from main are all finishing edits, and main has the finished versions:
- the policy test now checks the 64 KiB bound directly;
- `liveness.rs` gains the macro and dispatch handling;
- the `integrate.rs` control flow is simplified with the same behaviour;
- the stress test has the clippy fix.

The snapshot is also intact in `stash@{0}`.

**Also removed:**
- `land/final-2026-10-16`, my own short-lived branch, deleted with `-D`. Its tip was `6ecfd136`, which is on main.
- This session's scratch CI worktree.

**Lost, and not recoverable from git.** The r5 Swift label files lived in an ignored `tmp/` inside a removed worktree, and `git worktree remove` deletes ignored files. A bundle carries only commits. So the corpus record on main is the one from `83cd6e38` and was not re-measured.

## Other things you should know

- **Ownership after this pass.** Session devcouncil-1e (pid 27036) owns `integration/stale-2026-10-16`. It will bring that branch up to date with main and land it. It asked that no one else merge the `finish/*` lanes or the integration branch. It plans to delete `lane/swift-recall-r5`, `lane/degraded-status-attack` and `lane/swift-recall-r5-attack`. All three have backup refs from this pass.
- **What that session did after this report.** It removed all nine of session 36664's worktrees. It deleted these merged branches with `-d`: accuracy/record-2026-10-15, accuracy/record-2026-10-15-swift-r4, accuracy/record-2026-10-16-swift-r5, lane/classify-parse-tests, lane/degraded-status, lane/swift-recall-r5 and worktree-agent-a654b9c7bef5bc53d. It kept lane/degraded-status-attack and lane/swift-recall-r5-attack until you approve `-D`. Every one has a ref under `refs/backup/2026-10-16-pass3/`. The untracked `swift_recall_r5_attack.rs` in agent-a654b9c7, which differed from main's copy, went with that worktree unless that session saved it.
- **Follow-up (owner: "commit and merge all branches and delete stale branches").** I deleted lane/degraded-status-attack (`2a22903f`) and lane/swift-recall-r5-attack (`c24fe674`) with `-D`. Each is patch-equivalent to a commit on main (`d5628d19` and `10bd7c73`) by `git cherry`, and each has a backup ref under `refs/backup/2026-10-16-pass3/`. I left the main-checkout commit and the integration and orphan merges alone, because devcouncil-1e's agents hold those worktrees live. That session holds integration at `828b6378`, with main merged in, and has snapshotted the orphan work as `orphan/main-checkout-2026-10-16` `3218d0b7`.

- **My mistake, already reverted.** Setting fsmonitor off for my worktree with `git -C … config core.fsmonitor false` wrote to the shared `.git/config`, because a plain `git config` writes there even with `extensions.worktreeConfig` set. For about a minute, every worktree had `core.fsmonitor=false`. I restored it to `true` and moved my setting to `config.worktree`.
- **Session 36664's worktrees.** Its eight locked worktrees and the unlocked `agent-a32aecdc` (detached at `7f97d468`, clean) are still there. The process is gone, so removing them would need `--force` past its stale lock. That is your call, not part of this pass.
- **Scratch clone.** A gusset clone at the pin remains in this session's scratchpad (`…/scratchpad/ci/gusset`).

## Restore a deleted branch

```sh
git branch <name> refs/backup/2026-10-16-pass3/<name>
# e.g.
git branch claude/cranky-pare-0866b9 refs/backup/2026-10-16-pass3/claude/cranky-pare-0866b9
git branch claude/silly-morse-7c6a7b refs/backup/2026-10-16-pass3/claude/silly-morse-7c6a7b
git branch lane/classify-parse-tests-c refs/backup/2026-10-16-pass3/lane/classify-parse-tests-c
git branch audit/contract-skip-guard refs/backup/2026-10-16-pass3/audit/contract-skip-guard
git branch finish/confident-tier refs/backup/2026-10-16-pass3/finish/confident-tier
git branch lane/confident-tier-attack refs/backup/2026-10-16-pass3/lane/confident-tier-attack
# from the bundle, if the refs are gone:
git fetch /Users/bharath/Code/devtools/DevCouncil-branches-2026-10-16-pass3.bundle <name>:<name>
# Undo this pass's two merges on main (not pushed). Do this only if nothing has been built on b2c8997a.
# --keep moves main back and refuses rather than overwrite the checkout's uncommitted work.
git reset --keep refs/backup/2026-10-16-pass3/main
```

## Verification after the pass

- `git fsck --no-dangling` exits 0 with no output.
- Dangling objects (81 commits, 72 trees, 22 tags, 9 blobs) were left alone.
- pid 27036 is still alive with its cwd on `main`, which still exists. `gitpulse_insights` timed out again, so this check is from the process table, not GitPulse.
