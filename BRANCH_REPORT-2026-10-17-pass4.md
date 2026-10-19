# Branch consolidation — 2026-10-17 pass 4

Local only; nothing was pushed. **No branch was merged and no branch was deleted.** `main` stays at `9c8b5fa6`. Nothing was committed on anyone's behalf. Each non-main branch is in use by a live session, kept by an earlier owner decision, or tied to a live session's agent worktrees. Fable reviewed the classification and agreed that this session should merge nothing.

The previous report (2026-10-16 pass 3) is now `BRANCH_REPORT-2026-10-16-pass3.md`.

## What the checks said

- **`gitpulse_insights` / `gitpulse_collision_risk` answered, with every facet `ok`.** All 4 worktrees were scanned, there were 0 overlapping files, and sessions were observed for both kinds (claude 2, codex 0).
- **Live sessions:** pid 4321 (session `dae782ed`, started 10-16 23:34, cwd the main checkout, busy) and pid 36616 (this session). pid 4321 runs the three agent worktrees below.
- **The main checkout** holds only the 8 untracked `*_REPORT*.md` files, and it is shared with pid 4321. They were not committed.
- **The DevMap MCP did not connect** (ENOENT on a test-binary path). No `devmap_impact` overlap check was run, because no merge happened. This is a gap, recorded here.
- **The Bash gate** refused `git log main..$b` loops, `git -c`, and programs that take paths outside the repo. The inventory ran from `tmp/bc-inv.sh`, which is gitignored.

## Inventory (main = 9c8b5fa6)

Every author is BHARATH CHANDRA VADDARAM. "Cherry" is `git cherry main <b>`.

| Branch | Ahead | Behind | Last commit | Merged | Cherry | Worktree (dirty?) | Live session | Task (status) | Class → outcome |
|---|---|---|---|---|---|---|---|---|---|
| accuracy/rustlabel | 1 | 0 | 10-17 00:52 `b0d060c2` | no | `+1` | agent-abac9499 (clean, **locked by 4321**) | 4321 | ft-b812a97c (ready) | **in use → kept** |
| fix/devmap-py-field-type-leak | 1 | 0 | 10-17 08:00 `a4d62f42` | no | `+1` | agent-a364fba3 (clean, unlocked) | 4321's subagent, finished | ft-d69f8f97 (ready) | **in use → kept** (see below) |
| fix/devmap-reference-ladder-guard | 0 at start → 1 by the end | 0 | 10-17 08:10 `31ac1075` (committed during this pass) | at start yes (= main) | — | agent-a69280e6 (24 dirty at start, **locked by 4321**) | 4321's subagent, running | ft-0e6cc90f (ready) | **in use → kept** |
| finish/workbench | 4 | 58 | 10-16 09:15 `d74379fb` | no | `+3` | none | — | ft-aeb4a53f (backlog) | **blocked: owner decision → kept** |
| lane/workbench | 2 | 166 | 10-16 08:29 `d88688b5` | no | `+2` (both in finish/workbench) | none | — | ft-4966b304 (backlog) | contained in a blocked parent → **kept** |
| worktree-agent-a364fba39a15b0d88 | 0 | 151 | 10-13 22:44 `e79c2610` | yes | — | none | 4321's harness (`CLAUDE_BASE`) | — | merged; **kept until pid 4321 exits** |
| worktree-agent-a69280e6dfd0afddc | 0 | 151 | 10-13 22:44 `e79c2610` | yes | — | none | same | — | same |
| worktree-agent-abac9499b6826d618 | 0 | 151 | 10-13 22:44 `e79c2610` | yes | — | none | same | — | same |

Remote (not in scope; nothing touched):
- `origin/main` = `e79c2610`. Local main is **151 ahead, 0 behind**, so nothing landed since 10-13 is on the remote.
- `origin/dependabot/github_actions/actions/upload-artifact-7` (`55042f64`) is an ancestor of main. The remote branch and its PR are stale.

There are no release branches.

## Update 09:20: the P0 pair landed (by session dae782ed, not by this session)

You chose "wait, land its branch". pid 4321 then did the landing itself, before this session could:
- **main `9c8b5fa6` → `1ae3bc15`**: "Merge branch 'integration/accuracy-p0-2026-10-17'", `--no-ff`, 08:38.
- The integration branch holds two merges:
  - `dd07b6a6` merges `fix/devmap-reference-ladder-guard` (`31ac1075`);
  - `640f210f` merges `fix/devmap-py-field-type-leak` (`a4d62f42`).
- Every one of those branches is now an ancestor of main (`merge-base --is-ancestor`).
- pid 4321 resolved the conflicts in its own worktree. This session resolved none and did not review those resolutions with Opus.

**Cross-app:**
- `EXTRACTION_SCHEMA_VERSION` went 93 → 94 (`rust/devmap-extract/src/cache.rs:711`).
- GitPulse's vendored copy already reads `"94"` (`GitPulse/src-tauri/vendored/devmap-extract/src/cache.rs:711`, file content verified).
- Not verified: that the GitPulse change is committed, or that the installed GitPulse binaries were rebuilt. GitPulse's `check:schema-parity` does not compare this constant.

**Verification of `1ae3bc15`.** It ran in the scratch worktree `tmp/verify-1ae3bc15` with its own `CARGO_TARGET_DIR=tmp/verify-target`, at load 30–44:
- `cargo fmt --check`: 10 drifting files, the same count as the pass-3 baseline. None is a file the merge touched.
- `cargo clippy --workspace --all-targets -D warnings`: exit 0.
- `cargo test --workspace --no-fail-fast`: **5423 pass, 7 fail, 21 ignored.** Each failure was rerun alone:
  - `dc-redact` `repeated_credential_assignments_scale_linearly`: ok alone (timing).
  - `devmap-extract` `wiring_port_under_stress::a_file_of_nothing_but_dynamic_imports_stays_linear`: ok alone (timing).
  - `devmap-cli` `over_limit_daemon_agrees_with_build::…settles_without_retries`: ok alone.
  - `devmap-cli` `hook::tests::a_session_end_that_overlaps_another_report_still_runs_its_own`: ok alone.
  - `dc-verify` `json_contract` (2 tests): fail because there is "no host binary at …/tmp/verify-1ae3bc15/backend/go_orchestrator/bin/devcouncil". The scratch worktree is not a sibling checkout, so the Go host cannot be built there. The merge does not touch the Go host.
  - These were not A/B'd against `9c8b5fa6`. "Load flake" is inferred from the isolated passes.
- **Not run:** the Go, npm, workflow and release steps of `ci:local`. The merge changes no Go, JS, workflow or release file. These steps need the sibling checkout layout for the gusset engine.

**Still unmerged:** `accuracy/rustlabel`. When checked at 09:05, rust-analyzer was running in its worktree (pids 55432/55434, cwd `agent-abac9499/rust/target-rustlabel/fx`) and pid 4321 still held the lock. This session is watching for the lane to finish before landing it.

**Now merged but not deleted:** `fix/devmap-py-field-type-leak`, `fix/devmap-reference-ladder-guard` and `integration/accuracy-p0-2026-10-17`. Each is checked out in a pid-4321 agent worktree; the integration one is still locked and has live processes. `git branch -d` refuses a checked-out branch, and those worktrees belong to that session's harness.

## Why each branch was kept

- **fix/devmap-py-field-type-leak.** It is mechanically mergeable: it fast-forwards, and `merge-tree` is clean. Fable found that session `dae782ed` (pid 4321) owns it:
  - its subagent `a364fba3` committed it at 08:00:44;
  - the parent session's transcript says it will "review both P0 branches together … then bring you the merge decision", including diffing the `record.json`/ratchet rewrites first.

  Merging it here would pre-empt that review. Its files also overlap the ladder-guard lane on 9 paths (`resolver.rs`, `record.json`, `ratchet.json`, the accuracy TSVs).
- **accuracy/rustlabel.** Its worktree is locked by pid 4321. Task ft-b812a97c says "review and merge it", and the parent session plans to amend the commit trailer before merging, so `b0d060c2` may be rewritten.
- **fix/devmap-reference-ladder-guard.** A subagent was still building in it when the pass started. It committed `31ac1075` during the pass, which confirms the lane is live.
- **finish/workbench, lane/workbench.** Pass 3 records the owner's decision on these. They are a cross-app dc-store schema change (schema 10 / workbench 14) that must reach GitPulse, Manvi and Jarvis in the same piece of work. `stash@{0}` holds related `schema.rs` edits from session 36664; keep it.
- **worktree-agent-\*.** These are merged pointers at `e79c2610`. The Claude Code harness of pid 4321 deletes `worktree-agent-<id>` itself when it cleans up those agent worktrees. Deleting them first would only cause a best-effort warning, but nobody could confirm that a failure there does not cut the rest of the cleanup short. Waiting costs nothing.

## Merged, conflicts, deleted

- **Merged:** none. No merge commits.
- **Conflicts:** none, because nothing was merged.
- **Deleted:** none. No `-d` or `-D` was run, and no worktree was removed.
- **Phase 3 (commit pending work):** nothing was eligible. The only dirty worktrees were the ladder-guard lane, which had a live agent, and the main checkout, which is shared with live pid 4321 and holds only untracked reports.

## Backups (Phase 2)

Backup refs at the pass start, under `refs/backup/2026-10-17/`:

| Ref | SHA |
|---|---|
| main | `9c8b5fa6` |
| accuracy/rustlabel | `b0d060c2` |
| finish/workbench | `d74379fb` |
| fix/devmap-py-field-type-leak | `a4d62f42` |
| fix/devmap-reference-ladder-guard | `9c8b5fa6` (the branch has since moved to `31ac1075`) |
| lane/workbench | `d88688b5` |
| worktree-agent-a364fba39a15b0d88 | `e79c2610` |
| worktree-agent-a69280e6dfd0afddc | `e79c2610` |
| worktree-agent-abac9499b6826d618 | `e79c2610` |

Bundle: `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-17.bundle` (32.7 MB). It holds all 9 branches plus `refs/stash`. `git bundle verify` reports "complete history".

Earlier backups are kept on purpose: `refs/backup/2026-10-13/*`, `…/2026-10-14-consolidate/*` and `…/2026-10-16-pass3/*`.

Restore any branch:

```sh
git branch <name> refs/backup/2026-10-17/<name>
# or from the bundle
git fetch /Users/bharath/Code/devtools/DevCouncil-branches-2026-10-17.bundle refs/heads/<name>:refs/heads/<name>
```

## What's needed next (owner)

1. **Session `dae782ed` (pid 4321) lands its three lanes.** Fable's suggested order:
   1. `fix/devmap-py-field-type-leak` (ff);
   2. `accuracy/rustlabel` (a disjoint file set);
   3. `fix/devmap-reference-ladder-guard`, rebased onto the first two because of the 9-path overlap.

   Re-run this consolidation after that, and delete the merged branches and the `worktree-agent-*` pointers once pid 4321 has exited.
2. **finish/workbench:** the cross-app schema change across DevCouncil, GitPulse, Manvi and Jarvis.
3. **Remote:** pushing main (151 commits) and closing the stale dependabot branch are your call.

## Verification (Phase 6)

- `git fsck --no-dangling`: exit 0.
- `gitpulse_insights` re-run: both live sessions are present, all four worktrees are still on their branches, and every facet is `ok`.
- **ci:local was not run.** main did not change in this pass (`9c8b5fa6` before and after). Pass 3 verified that commit (clippy `-D warnings` clean; cargo tests 5426 pass / 2 known load flakes). A run now would compete for CPU with pid 4321's live builds. If you want a fresh run anyway, ask for it.
