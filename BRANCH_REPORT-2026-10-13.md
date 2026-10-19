# Branch consolidation report

Date of this pass: 2026-10-13. Nothing was pushed. `main` was not moved.

Live agents were committing on their own branches the whole time. The table is the last read after the deletions. A tip can move again the moment an agent commits. The commits that were deleted are still reachable from the backup refs below.

## What was not done

No branch was merged. No conflict was resolved. No other worktree's pending edits were committed. `npm run ci:local` was not run.

`main` is checked out in this worktree with about 30 uncommitted files, and a Claude session (pid 3121, busy) has this directory as its cwd. A merge would move `main` under that session. That was not done. `ci:local` would also build this dirty tree and contend for the same Cargo and Go caches that session is using, so a green or red result would not be a result for committed `main`.

The classification review was a read-only code-reader pass. This harness cannot select the Fable model. The review agreed with keeping every in-use branch and with deleting only the five refs named below. Two later `worktree-agent-*` refs were left in place because the review said to keep anything it had not named.

DevMap impact was not used to order merges. `gitpulse_insights` reported the code graph `is_fresh: false` (generation 4478, stored extraction payload obsolete). That gap is recorded as `GAP-BRANCH-CONSOLIDATION-STALE-INDEX`.

## Live sessions and worktrees

`gitpulse_insights` and `gitpulse_collision_risk` both returned `ok: true`. Final scan: 12 worktrees, 0 unscanned, `sessions_ok: true`, Claude observed, Codex observed with 0 sessions. Overlapping dirty files: 0. Shared worktree files: 0.

Three live sessions, all still on the branch they started this pass on:

| Worktree | Branch | Session |
| --- | --- | --- |
| `/Users/bharath/Code/devtools/DevCouncil` | `main` | Claude cli, pid 3121, busy. 30 dirty files (25 unstaged, 1 staged, 4 untracked). |
| `.../.claude/worktrees/cranky-pare-0866b9` | `claude/cranky-pare-0866b9` | Claude desktop, pid 57016, idle. Clean. |
| `.../.claude/worktrees/silly-morse-7c6a7b` | `claude/silly-morse-7c6a7b` | Claude desktop, pid 56829, idle. Clean at the final scan. |

The other nine worktrees are Claude agent checkouts. Several are git-locked. None of them lost its branch. Final collision scan still covered all 12.

`gitpulse_change_context` returned an empty `task_id` for every worktree that was asked (`main`, `lane/workbench`, `lane/policy`, `audit2/gap-txn-immediate`, `claude/silly-morse-7c6a7b`). There is no GitPulse worktree binding. The task column below is a name match to the board, marked inferred, except where a task log names the branch.

## Inventory

Ahead and behind are `git rev-list --left-right --count main...<branch>`: behind, then ahead. "Merged" means `git merge-base --is-ancestor` of the tip into `main` exited 0, or the tip is `main` itself. A `+` from `git cherry main <branch>` means the patch is not already on `main`.

Author of every local tip in this table is BHARATH CHANDRA VADDARAM.

| Branch | Behind | Ahead | Tip date | Merged | Worktree | Dirty | Live session | Task | Class |
| --- | ---: | ---: | --- | --- | --- | ---: | --- | --- | --- |
| `main` | 0 | 0 | 2026-10-12 15:46 -0700 | yes (`a90b17fd`) | this checkout | 30 | pid 3121 busy | none bound | keep, in use |
| `lane/workbench` | 14 | 1 | 2026-10-12 10:12 -0700 | no, cherry `+ 765db331` | `DevCouncil-l3-workbench` | 28 | no | inferred `ft-aeb4a53f` backlog | keep, blocked |
| `lane/policy` | 14 | 0 | 2026-10-12 09:08 -0700 | yes, ancestor `d78cd9de` | `DevCouncil-l4-policy` | 1 | no | none | keep, dirty worktree |
| `audit2/gap-txn-immediate` | 0 | 1 | 2026-10-13 22:43 -0500 | no, cherry `+ 48df05a2` | locked agent worktree, now clean | 0 | parent pid 3121 | task log names this branch: `ft-a4b3e5f0` (read as review) | keep, in use |
| `audit2/daemon-write` | 0 | 1 | 2026-10-13 22:45 -0500 | no, cherry `+ bb01a8c2` | agent worktree, clean | 0 | parent pid 3121 | inferred `ft-3f82536e` (was in_progress) | keep, in use |
| `audit2/consumers-env` | 0 | 1 | 2026-10-13 22:47 -0500 | no, cherry `+ 90e37ac1` | agent worktree, clean | 0 | parent pid 3121 | none found | keep, in use |
| `audit2/pdg-taint` | 0 | 1 | 2026-10-13 22:53 -0500 | no, cherry `+ cc8545c8` | locked, clean after the commit | 0 | parent pid 3121 | inferred `ft-08e1753c` (was in_progress) | keep, in use |
| `claude/silly-morse-7c6a7b` | 0 | 1 | 2026-10-13 22:44 -0500 | no, cherry `+ e79c2610` | Claude desktop worktree | 0 | pid 56829 idle | none bound | keep, in use |
| `audit2/extract-robustness` | 0 | 0 | 2026-10-12 15:46 -0700 | yes, tip is `main` | locked | 6 | parent pid 3121 | inferred `ft-359e6751` in_progress | keep, in use |
| `audit2/config-trust` | 0 | 0 | 2026-10-12 15:46 -0700 | yes, tip is `main` | locked | 5 | parent pid 3121 | none found | keep, in use |
| `audit2/lsp-pass` | 0 | 0 | 2026-10-12 15:46 -0700 | yes, tip is `main` | locked | 3 | parent pid 3121 | none found | keep, in use |
| `claude/cranky-pare-0866b9` | 0 | 0 | 2026-10-12 15:46 -0700 | yes, tip is `main` | Claude desktop worktree | 0 | pid 57016 idle | none | keep, in use |
| `worktree-agent-a37df40a3527d4886` | 14 | 0 | 2026-10-12 09:08 -0700 | yes, ancestor `d78cd9de` | not checked out (worktree is on `audit2/lsp-pass`) | 0 | no | none | keep |
| `worktree-agent-a75132bc1df5b468c` | 14 | 0 | 2026-10-12 09:08 -0700 | yes, ancestor `d78cd9de` | not checked out (worktree is on `audit2/config-trust`) | 0 | no | none | keep |

`main` is 14 commits ahead of `origin/main` and 0 behind. Those 14 are local merge commits, including the five Dependabot merges, and are not pushed.

Remote tips were not deleted. Each Dependabot tip is an ancestor of local `main` (`git merge-base --is-ancestor` exited 0). `origin/main` is `d78cd9de`.

| Remote | Tip | Ahead of local main | Behind |
| --- | --- | ---: | ---: |
| `origin/main` | `d78cd9de` | 0 | 14 |
| `origin/dependabot/github_actions/actions/checkout-7` | `1424b343` | 0 | ancestor |
| `origin/dependabot/github_actions/actions/setup-go-7` | `cb243908` | 0 | ancestor |
| `origin/dependabot/github_actions/actions/setup-node-7` | `9704349c` | 0 | ancestor |
| `origin/dependabot/github_actions/actions/upload-artifact-7` | `55042f64` | 0 | ancestor |
| `origin/dependabot/.../setup-gusset/actions/checkout-7` | `ec95b2e3` | 0 | ancestor |

## Branches merged

None. No merge commit was created.

## Conflicts

None. No merge was started.

## Branches deleted

All five were ancestors of `main` (`merge-base --is-ancestor` exited 0), pointed at `d78cd9de`, were not checked out, and had a backup ref. `git branch -d` deleted them. `-D` was not used. No worktree was removed. The worktrees that share these slugs are checked out on the `audit2/*` branches and were still there after the delete.

| Branch | Was | Backup ref |
| --- | --- | --- |
| `worktree-agent-a53845bde69f78c0e` | `d78cd9de` | `refs/backup/2026-10-13/worktree-agent-a53845bde69f78c0e` |
| `worktree-agent-a69e60d6adc08da5a` | `d78cd9de` | `refs/backup/2026-10-13/worktree-agent-a69e60d6adc08da5a` |
| `worktree-agent-ac826ab4336721c2e` | `d78cd9de` | `refs/backup/2026-10-13/worktree-agent-ac826ab4336721c2e` |
| `worktree-agent-aeaea602075eb898b` | `d78cd9de` | `refs/backup/2026-10-13/worktree-agent-aeaea602075eb898b` |
| `worktree-agent-af56be8bf0f376ec1` | `d78cd9de` | `refs/backup/2026-10-13/worktree-agent-af56be8bf0f376ec1` |

`d78cd9de` is still `origin/main`, `lane/policy`, and the two `worktree-agent-*` refs that were kept. The deleted tips are not dangling.

## Branches kept

| Branch | Why |
| --- | --- |
| `main` | Integration branch. Dirty, and pid 3121 is busy here. Also 14 unpushed commits. |
| `lane/workbench` | One unique commit (`765db331`, cherry `+`) plus 28 uncommitted files of the workbench task, including untracked `doctor.go`, `task.go`, `tasks.go`, and `workbench.go`. Not a live session, and the board task is backlog, but the dirty tree is unfinished work. It was not committed and not merged. |
| `lane/policy` | Commits are already on `main`. The worktree is dirty only because of untracked `backend/go_orchestrator/gate/zz_probe_test.go`, a logging probe. That file was not committed, and the worktree was not removed. |
| `audit2/gap-txn-immediate` | Unique commit `48df05a2` (`fix(dc-store): gap writers take the write lock before they read`, cherry `+`, 3 files, +284/-49). Checked out. The task log on `ft-a4b3e5f0` names this branch. |
| `audit2/daemon-write` | Unique commit `bb01a8c2` (`fix(devmap-serve): scope the post-HEAD-move drain write to the scan's hash delta`, cherry `+`). Checked out. |
| `audit2/consumers-env` | Unique commit `90e37ac1` (`fix(install): run the schema-parity check with a scrubbed environment`, cherry `+`). Checked out. Appeared during this pass. |
| `audit2/pdg-taint` | Unique commit `cc8545c8` (`fix(devmap pdg): route finally and handlers, find nested sinks, flatten elif, report what did not parse`, cherry `+`). Locked worktree. |
| `claude/silly-morse-7c6a7b` | Unique commit `e79c2610` (`fix(policy): give the read-only git argument rung its own rule`, cherry `+`). Live session pid 56829. |
| `audit2/extract-robustness` | Tip is `main`, but the locked worktree has 6 dirty files and the matching task was in progress. |
| `audit2/config-trust` | Tip is `main`. Locked worktree, 5 dirty files. Created during this pass. |
| `audit2/lsp-pass` | Tip is `main`. Locked worktree, 3 dirty files. Created during this pass. |
| `claude/cranky-pare-0866b9` | Tip is `main`. Live idle session pid 57016. |
| `worktree-agent-a37df40a3527d4886` | Ancestor of `main`, not checked out. The review said keep it. The `audit2/lsp-pass` worktree is the live checkout for this slug. |
| `worktree-agent-a75132bc1df5b468c` | Same. The `audit2/config-trust` worktree is the live checkout. |

## Branches blocked

These have commits that are not on `main`. They stay where they are until the session holding `main` is finished and this worktree is clean.

| Branch | Commit | What it changes | What has to happen first |
| --- | --- | --- | --- |
| `lane/workbench` | `765db331` plus 28 dirty files | dc-store schema, tasks, workbench scope, Go store client, docs. 26 files, +2085/-256 on the commit alone. | The dirty work belongs to that worktree. Do not commit it from here. After it is committed or dropped by its owner, merge it only once `main` is clean. It is 14 commits behind `main` and touches `schema.rs`, which is also dirty on `main`. |
| `audit2/gap-txn-immediate` | `48df05a2` | `rust/dc-store/src/lib.rs`, `records.rs`, `tests/gap_write_contention.rs` | Worktree must be released. Overlaps `lane/workbench` in dc-store. Merge with a real resolution, not `-X ours` or `-X theirs`. |
| `audit2/daemon-write` | `bb01a8c2` | `rust/devmap-serve/src/daemon.rs` and a race test | Worktree must be released. `main` also has uncommitted edits under `devmap-serve`. |
| `audit2/consumers-env` | `90e37ac1` | Go component consumer and proc env scrub | Worktree must be released. |
| `audit2/pdg-taint` | `cc8545c8` | `devmap-analyze` PDG and the PDG CLI | Locked worktree must be released. |
| `claude/silly-morse-7c6a7b` | `e79c2610` | policy read-only git arguments, verdict schema | Live session pid 56829 must be done. `main` has uncommitted policy files. |

When `main` is free, the five one-commit branches that sit directly on `a90b17fd` can be merged with `git merge --no-ff` in an order that keeps dc-store together: `audit2/gap-txn-immediate` before `lane/workbench`, because the workbench commit is the schema move and is 14 commits back. The policy, daemon, consumer, and PDG commits touch different trees from each other. Re-check `git cherry` first. Agents were still committing during this pass, so a tip above may already have another commit on it.

There is no `CONTRIBUTING` file that asks for a rebase. Recent history on `main` is merge commits, so `--no-ff` is the match.

After each merge, run `npm run ci:local` from the repository root on a clean `main`.

## Backup

Every local branch that existed when the refs were written has a ref under `refs/backup/2026-10-13/`. Where a branch moved later, the backup ref was moved to the new tip. The previous tip of those branches was `a90b17fd`, which is still `refs/backup/2026-10-13/main`.

Bundles, both outside the repo, both `git bundle verify` okay:

| File | What it contains |
| --- | --- |
| `/Users/bharath/Code/devtools/devcouncil-branch-backup-2026-10-13.bundle` (31M) | Heads at 22:47, including the five refs that were later deleted, and the remote tips. `audit2/pdg-taint` in this file is still `a90b17fd`. |
| `/Users/bharath/Code/devtools/devcouncil-branch-backup-2026-10-13-v3.bundle` (31M) | Heads at 22:54, including `cc8545c8`, `90e37ac1`, `bb01a8c2`, `48df05a2`, `e79c2610`, `765db331`. Does not name the five deleted refs. Those commits are in the first bundle and in the backup refs. |

`devcouncil-branch-backup-2026-10-13-v2.bundle` is an intermediate snapshot from 22:53, before `audit2/pdg-taint` moved to `cc8545c8`. Prefer v1 plus v3.

## Restore

For any deleted branch, from this repository:

```
git branch worktree-agent-a53845bde69f78c0e refs/backup/2026-10-13/worktree-agent-a53845bde69f78c0e
git branch worktree-agent-a69e60d6adc08da5a refs/backup/2026-10-13/worktree-agent-a69e60d6adc08da5a
git branch worktree-agent-ac826ab4336721c2e refs/backup/2026-10-13/worktree-agent-ac826ab4336721c2e
git branch worktree-agent-aeaea602075eb898b refs/backup/2026-10-13/worktree-agent-aeaea602075eb898b
git branch worktree-agent-af56be8bf0f376ec1 refs/backup/2026-10-13/worktree-agent-af56be8bf0f376ec1
```

From the first bundle, if the backup refs are gone:

```
git fetch /Users/bharath/Code/devtools/devcouncil-branch-backup-2026-10-13.bundle refs/heads/worktree-agent-a53845bde69f78c0e:refs/heads/worktree-agent-a53845bde69f78c0e
```

The same `git fetch <bundle> refs/heads/<name>:refs/heads/<name>` form works for every head in that bundle. Use the v3 bundle for `audit2/pdg-taint` at `cc8545c8` and `audit2/consumers-env` at `90e37ac1`.

## Verify

`git fsck` exited 0. It printed a long list of dangling commits, trees, blobs, and tags. Those are unreachable objects already in the database. No missing or corrupt object was reported. `d78cd9de` is still referenced.

Final `gitpulse_insights`: `ok` on worktrees, sessions, collisions, and the ledger. The three live sessions are still on `main`, `claude/cranky-pare-0866b9`, and `claude/silly-morse-7c6a7b`. All 12 worktrees still have their branches.

## Continuation

A second pass re-read the repository. Merges are still blocked. `main` is still dirty (31 files in the insights scan: the same uncommitted audit edits, plus this report). Claude pid 3121 is still in this checkout, now reported as `shell`. Pid 57016 and pid 56829 are still idle on `claude/cranky-pare-0866b9` and `claude/silly-morse-7c6a7b`. No pending work was committed. No worktree was removed. No further branch was deleted. `npm run ci:local` was not run.

### What changed under the live session

`main` is now `e79c2610` (`fix(policy): give the read-only git argument rung its own rule`). The reflog entry is `main@{0}: merge claude/silly-morse-7c6a7b: Fast-forward` from `a90b17fd`. This pass did not run that merge. `a90b17fd` is still an ancestor (`merge-base --is-ancestor` exited 0). Its explicit backup is `refs/backup/2026-10-13/main-a90b17fd`. The current tip is `refs/backup/2026-10-13/main`.

`origin/main` is also `e79c2610`. `git rev-list --left-right --count main...origin/main` is `0 0`. `refs/remotes/origin/main` reflog says `update by push` from `d78cd9de` to `e79c2610`. This pass did not push.

`gitpulse_insights` now counts 21 worktrees and 18 Claude checkouts on disk. `agents.live` still shows 3 sessions, `sessions_ok: true`. `gitpulse_collision_risk` returned `ok: true` with 0 overlapping files, and `truncated: true` with `scanned_worktrees: 16` and `unscanned_worktrees: 5`. That overlap list is incomplete, so it is not evidence that the five unscanned worktrees are clean.

DevMap is still stale (`is_fresh: false`, generation 4479, source tree differs from the indexed generation, extraction payload obsolete).

### Unique commits still off main

Every row below is `git cherry -v main <branch>` printing `+`. Behind counts include the fast-forward of `e79c2610`, so a branch that used to sit on `a90b17fd` is now 1 behind.

| Branch | Behind | Ahead | Cherry | Worktree | Dirty | Class |
| --- | ---: | ---: | --- | --- | ---: | --- |
| `lane/workbench` | 15 | 1 | `+ 765db331` | `DevCouncil-l3-workbench` | 28 | keep. Uncommitted workbench files still there. |
| `audit2/gap-txn-immediate` | 1 | 1 | `+ 48df05a2` | agent worktree | 0 | keep. Checked out. |
| `audit2/daemon-write` | 1 | 1 | `+ bb01a8c2` | agent worktree | 0 | keep. Same patch is also on the two branches below. |
| `audit2/daemon-followup` | 1 | 2 | `+ bb01a8c2`, `+ eefa2f2d` | agent worktree | 0 | keep. Tip is the same commit as `audit2/disk-writes`. |
| `audit2/disk-writes` | 1 | 2 | `+ bb01a8c2`, `+ eefa2f2d` | agent worktree | 3 | keep. Dirty, so the tip is not the whole change. |
| `audit2/extract-robustness` | 1 | 1 | `+ 87823824` | agent worktree | 0 | keep. Checked out. |
| `audit2/pdg-taint` | 1 | 1 | `+ cc8545c8` | agent worktree | 0 | keep. Checked out. |
| `audit2/lsp-pass` | 1 | 1 | `+ 62c6438f` | agent worktree | 0 | keep. Checked out. |
| `audit2/config-trust` | 1 | 1 | `+ 1955ec58` | agent worktree | 0 | keep. That patch is also the first commit on `audit2/grants-ttl`. |
| `audit2/grants-ttl` | 1 | 2 | `+ 1955ec58`, `+ 23cdf38a` | agent worktree | 0 | keep. Checked out. |
| `audit2/consumers-env` | 1 | 1 | `+ 90e37ac1` | agent worktree | 0 | keep. Checked out. |
| `audit2/component-install` | 1 | 1 | `+ 7cc534bc` | agent worktree | 0 | keep. Checked out. |
| `audit2/mcp-lifecycle` | 1 | 1 | `+ 1d55b10e` | agent worktree | 0 | keep. Checked out. |
| `audit2/residue` | 1 | 1 | `+ 2f9c887d` | agent worktree | 0 | keep. Checked out. |
| `audit2/manifests` | 1 | 1 | `+ 86e41976` subject `WIP manifests` | agent worktree | 5 | keep. The subject says the commit is unfinished, and the worktree is dirty. |
| `audit2/accuracy-gates` | 1 | 0 | tip `a90b17fd`, already on `main` | agent worktree | 18 | keep. Dirty in-progress tree. |
| `audit2/partial-parse` | 1 | 0 | tip `a90b17fd`, already on `main` | agent worktree | 3 | keep. Dirty. |
| `claude/cranky-pare-0866b9` | 1 | 0 | tip `a90b17fd`, already on `main` | desktop worktree | 0 | keep. Idle session pid 57016. |
| `claude/silly-morse-7c6a7b` | 0 | 0 | tip is `main` | desktop worktree | 0 | keep. Idle session pid 56829. The fast-forward put this commit on `main`. |
| `lane/policy` | 15 | 0 | ancestor `d78cd9de` | `DevCouncil-l4-policy` | 1 | keep. Untracked probe still there. |

Eleven `worktree-agent-*` refs still point at `d78cd9de` and are not checked out. They were backed up and left in place. The parent session is still creating worktrees of that shape, and the collision scan left 5 worktrees unscanned.

When pid 3121 is gone and this worktree is clean, merge the stacked tips once, not the branches they already contain: `audit2/daemon-followup` (or `audit2/disk-writes`, same tip `eefa2f2d`) covers `audit2/daemon-write`; `audit2/grants-ttl` covers `audit2/config-trust`. Re-run `git cherry` first. Skip `audit2/manifests` while its subject is `WIP` or its worktree is dirty. `lane/workbench` is still the schema move and is 15 commits behind.

### Backup added on this pass

`refs/backup/2026-10-13/` now names every local branch tip from the list above, plus `main-a90b17fd`.

`/Users/bharath/Code/devtools/devcouncil-branch-backup-2026-10-13-v4.bundle` (31M) verifies okay and lists these 38 refs, including `main` and `origin/main` at `e79c2610`. Prefer this file for the commits that landed after v3. v1 is still the bundle that names the five deleted `worktree-agent-*` refs.
