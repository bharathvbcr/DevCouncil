# Branch consolidation, 2026-10-19 pass 7

This pass ran locally and pushed nothing. Main stays at `9558b8a1`; pid 136 landed the overnight wave itself, the last merge at 01:45.

- **Committed:** nothing. Every worktree was clean.
- **Merged:** nothing left to merge. Every lane of the wave was already on main when the pass started: lane/build-relative-root, lane/blast-literal-pathspecs, lane/budget-refusal-answers and lane/blast-blob-batch. The only unmerged branches are superseded, owner-kept, or the scratch probe attack2/timing-base.
- **Backups:** `refs/backup/2026-10-19-pass7/*` (46 branches) and `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-19-pass7.bundle` (verified).
- **Deleted with `-d`, worktree removed:** lane/blast-literal-pathspecs, lane/build-relative-root, lane/blast-blob-batch. Also 4 orphaned `worktree-agent-*` pointers.
- **Worktrees removed, branch left for your `-D`:** attack/literal-pathspecs, attack/budget-refusal, attack/relative-root, all cherry `−`. Also attack3/py-followup.
- **attack3/py-followup worktree:** removed with your approval. Its nested scratch worktree `tmp/base` had 7 dirty files; they and their diff are archived in `tmp/wt-archive/agent-ac9f-base.{tgz,diff,files}` before `git worktree remove --force` on the nested worktree only. The parent's ignored files are in `tmp/wt-archive/agent-ac9f4211914703da9.tgz`.
- **Held:** blast-radius timed out on `git branch -D` a third time. By your choice, you will run the deletion of the 20 superseded branches yourself.
- **Kept, by your choice:** the 7 worktrees pid 136 has locked, until it exits: lane/split-resolver-treesitter, lane/budget-refusal-answers, lane/corpus-runner-refusals, lane/mcp-status-degraded (merged); attack/swift-properties, attack4/py-wave8, attack/mcp-status (superseded). a5de's `tmp/dump-*/go.labels.json` must be archived before that one is removed.
- **Also kept:** integration/wave7 (pid 136's integration branch), claude/literals-substring-b3546e (pid 16139 is in it), finish/workbench and lane/workbench (your decision).
- **Verification:** `git fsck --connectivity-only` printed nothing (rc 0). `gitpulse_insights` returned every facet `ok`: 9 worktrees, 0 dirty, 0 overlaps, 3 live sessions (136, 46486, 16139), each with its worktree intact.

---

# Branch consolidation, 2026-10-18 pass 6 (after "commit and merge all changes, delete old and stale branches")

This pass ran locally and pushed nothing. Main went from `732f5311` to `1c4866d2`.

## Committed
- `76d9c680` docs: the 11 untracked audit, branch and build reports in the main checkout. A scan for credential shapes found none.
- Nothing else was dirty that could be committed:
  - the only other dirty tree is `agent-ac9f…/tmp/base`, a detached scratch worktree;
  - its 7 files are attack3 probes that match main blob for blob, plus 2 untracked scratch probes;
  - it has no branch, so it was left alone.

## Merged
| Branch | Merge commit | Conflicts | Review | Verification |
|---|---|---|---|---|
| accuracy/rustlabel (`b0d060c2`) | `1c4866d2` (`--no-ff`, built on a staging worktree, then main fast-forwarded) | none in text. One semantic fix: `Cargo.lock` entry `devmap-rustlabel` 0.2.4 → 0.2.5 (main's workspace version), because the auto-merged lock failed `--locked` | Opus deep review: **merge with follow-ups**, no blocker. No new third-party dependency, `publish`/`dist` false, existing `lsp.rs` callers unchanged | `fmt --check` ok. Tests: devmap-rustlabel + devmap-resolve **1510 pass / 0 fail**; `real_corpus_accuracy` **89 / 0**. Clippy: 0 lints in `tools/rustlabel` or `lsp.rs` |

- **Clippy `-D warnings` is red on main's own drift,** in files this merge does not touch:
  - `devmap-extract/src/treesitter/bindings.rs:255` (type_complexity, from the split commit `0ce3b70a`);
  - `devmap-resolve/src/resolver.rs:436` and `:571` (type_complexity);
  - `devmap-resolve/src/resolver/pytypes.rs:1049` (question_mark).

  All four are pid 136's wave code, so I left them for that session to fix.
- **Not run:** full `ci:local` (its Go/npm/workflow steps need the sibling gusset layout, and load was 60–110), and the `#[ignore]`d rustlabel golden test (needs rust-analyzer).
- **Review follow-ups** were folded into ft-b812a97c (rev 5, stays in review):
  - partial runs exit 0 without saying so;
  - the golden test runs nowhere in CI;
  - `namer.rs` `target_at` does not check the symbol's name;
  - two README corrections.
- **Not merged: `lane/build-relative-root` + `attack/relative-root`.** pid 136's attack round on it is still running, and your rule says no merge before the attack round closes.

## Deleted
Every branch below has a ref under `refs/backup/2026-10-18-pass6/` and is in `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-18-pass6.bundle` (66 branches, verified).
- **Deleted with `-d` (merged), worktree removed:** lane/deps-outbound, lane/verdict-generation-pin, lane/py-residue, attack2/freshness-close, lane/py-attack3-fixes, lane/traverse-freshness, lane/swift-properties, lane/search-partial-parse, lane/go-qualifier-leak, lane/go-type-use-dead, accuracy/rustlabel, merge/rustlabel-2026-10-18 (staging).
- **Deleted with `-d`:** 20 `worktree-agent-<id>` harness pointers whose worktrees were gone. 6 remain, for live worktrees.
- **Worktrees removed:** 24. Each was unlocked, tracked-clean, had no process cwd inside and no nested worktree.
  - Every removal used `git worktree remove` without `--force`.
  - Ignored non-build content (`tmp/`, logs) was archived first to the main checkout's `tmp/wt-archive/<agent-id>.tgz`.
  - Target directories and `.devcouncil/` indexes were not archived; both can be rebuilt.
  - The only accuracy labels found outside tracked source (`tmp/dump-*/go.labels.json`) are in a5de, which is locked and kept.

## Held: needs your approval
The `blast-radius` mod held `git branch -D` twice and timed out with no answer, so these 16 proven-superseded branches still exist. Their worktrees are already removed.
- attack5/py-close, attack/deps-outbound, attack/py-residue, attack/corpus-refusals, attack/verdict-pin, attack2/swift-close, attack/go-type-use, attack2/search-partial-close, attack/search-partial, attack2/py-residue-close, attack/go-qualifier, attack/traverse-freshness, attack2/deps-close
- attack5/base-run, attack/base-timing (no worktree)
- attack2/timing-base, a scratch probe not on main; the 5-line difference is recorded in pass 5

To delete them, run `git branch -D` with that list, or approve the prompt.

## Kept
- **pid 136's locked worktrees,** with their branches: lane/split-resolver-treesitter (merged), attack/swift-properties, attack4/py-wave8, lane/corpus-runner-refusals (merged), attack/mcp-status, lane/mcp-status-degraded (merged).
- **The live wave:** lane/build-relative-root, attack/relative-root, integration/wave7.
- **attack3/py-followup:** its worktree holds a nested dirty scratch worktree.
- **claude/literals-substring-b3546e:** live session pid 16139 is in it.
- **finish/workbench, lane/workbench:** your decision; a cross-app schema change.
- **`tmp/rrm`:** a detached worktree another session made.

## Verification
- `git fsck --connectivity-only` exited 0 with no output.
- The final `gitpulse_insights` returned every facet `ok`: 13 worktrees scanned, 0 overlapping files, 3 live sessions (136 and 46486 in the main checkout, 16139). Every live session's worktree and branch is intact.

## Restore
```sh
git branch <name> refs/backup/2026-10-18-pass6/<name>
git worktree add .claude/worktrees/<agent-id> <name>   # then: tar -xzf tmp/wt-archive/<agent-id>.tgz -C .claude/worktrees/<agent-id>
```

---

# Branch consolidation, 2026-10-18 pass 5

This pass ran locally and pushed nothing. **This session merged no branches, deleted no branches, removed no worktrees and committed nothing.** It made only two kinds of change: backup refs and a bundle.

During the pass, live session pid 136 landed two lanes on main by itself:
- 21:44 → 22:02: `4d6e9aa0`, a fast-forward of `lane/split-resolver-treesitter`;
- 22:09: `e01e6f0d`, a merge of `lane/go-type-use-dead`.

That makes main `79c721f5` → `e01e6f0d`. This session reviewed neither landing.

The previous report (2026-10-17 pass 4) is now `BRANCH_REPORT-2026-10-17-pass4.md`.

## What the checks said

- **GitPulse could not answer.** `gitpulse_insights` and `gitpulse_collision_risk` ran four times between 21:56 and 22:19. Every facet returned `ok=false`: "git worktree deferred under load … spawn rate limit", with load at 40–48. Those results are not read as clean. Sessions, worktrees and dirty state came from `ps`, `lsof`, `git worktree list --porcelain` and a per-worktree `git status` scan (`tmp/bc-dirty.sh`, which prints each exit code).
- **One of my own checks failed without saying so.** The first inventory (`tmp/bc-inv2.sh`) reported `dirty=0` everywhere because `timeout` is not installed here. The rerun without it is the dirty data used below.
- **DevMap is stale.** It reports "source tree differs from indexed generation" after the split landed. Fable's `devmap_impact` on `lsp.rs` came back `walk_incomplete`: 21 direct dependents, none in a live lane. Treat that as a lower bound.
- **`blast-radius` held `git branch -D`.** It flagged "3 commits would be reachable from nowhere" (it does not count `refs/backup/*`), and nobody answered within 10 minutes, so the command did not run. Neither branch was deleted.
- **Fable reviewed the classification** (agent, read-only) and agreed with it. It added three corrections, all folded in below:
  - main moved while the draft was being written;
  - pid 136 opened a new lane, `lane/go-qualifier-leak`;
  - `attack5/base-run` is not a subset of `attack5/py-close`.

## Live sessions

| pid | Session | Where | What it holds |
|---|---|---|---|
| 136 | Claude, started 10-17 17:09, actively merging | cwd = main checkout | the wave; locks on agent worktrees a170, a389, a5d2, a5de, ac796, aeb67, aff1c, ae815 (new) |
| 16139 | Claude desktop | `.claude/worktrees/literals-substring-b3546e` | that worktree |
| — | this session | main checkout | — |

## Inventory

All authors are BHARATH CHANDRA VADDARAM, so the author column is dropped. "Cherry" is `git cherry main <b>` at main `79c721f5`: `+` means the commit is not on main, `−` means a patch-equivalent commit is. Every branch's tip is in `refs/backup/2026-10-18-pass5/`.

| Branch | Ahead/behind | Last commit | Merged | Cherry | Worktree (dirty?) | Live session | Task | Class → outcome |
|---|---|---|---|---|---|---|---|---|
| accuracy/rustlabel | 1/99 | 10-17 00:52 `b0d060c2` | no | 1+ | abac9499 (clean, unlocked) | — | ft-b812a97c review | mergeable → **kept, flagged** |
| attack/go-type-use | 3/0 → moved to `cb4c4494` | 10-18 21:50 | no | 3+ | a389 (1 dirty; nested tmp/base 2 dirty; locked) | 136 | ft-19604c23 in_progress | **in use** |
| lane/go-type-use-dead | 2/0 | 10-18 21:46 | landed by 136 at 22:09 | — | af6da532 (clean) | 136 | ft-19604c23 in_progress | **in use** |
| lane/split-resolver-treesitter | 2/0 | 10-18 21:33 | landed by 136 at 22:02 | — | a170 (locked, process cwd) | 136 | ft-90cef8de in_progress | **in use** |
| lane/go-qualifier-leak (new) | 0/0 | = `4d6e9aa0` | yes | — | ae815e19 (locked) | 136 | — | **in use**; post-backup ref |
| integration/wave7 | = main | 10-18 22:09 | yes | — | none | 136's integration branch | — | **in use** |
| claude/literals-substring-b3546e | 0/35 | 10-18 13:16 | yes | — | literals-substring (clean) | 16139 | — | merged; **in use** |
| attack/mcp-status | 1/36 | 10-18 13:23 | no | 1− | aeb67 (clean, **locked**) | 136 | — | superseded; **in use** |
| attack/swift-properties | 3/58 | 10-17 23:48 | no | 3− | a5d2 (clean, **locked**) | 136 | — | superseded; **in use** |
| attack4/py-wave8 | 5/62 | 10-17 22:43 | no | 3− 2+ | a5de (clean, **locked**) | 136 | — | superseded; **in use** |
| lane/corpus-runner-refusals | 0/37 | 10-18 12:21 | yes | — | ac796 (clean, **locked**) | 136 | — | merged; **in use** |
| lane/mcp-status-degraded | 0/34 | 10-18 13:33 | yes | — | aff1c (clean, **locked**) | 136 | — | merged; **in use** |
| attack3/py-followup | 1/62 | 10-17 20:10 | no | 1+ | ac9f (clean; nested tmp/base **7 dirty**) | 136's finished agent | ft-572bb69d review | superseded → **kept** |
| attack5/py-close | 8/62 | 10-18 03:11 | no | 5− 3+ | a0069 (clean) | 136's finished agent | ft-572bb69d review | superseded → **kept** |
| attack5/base-run | 7/62 | 10-18 03:11 `8067cb93` | no | 5− 2+ | none | — | ft-572bb69d review | superseded → **delete held by blast-radius** |
| attack/base-timing | 1/20 | 10-18 18:38 `5614e2a6` | no | 1− | none | — | — | superseded → **delete held by blast-radius** |
| attack2/timing-base | 1/20 | 10-18 19:13 `8ae90aea` | no | 1+ | none | — | — | ambiguous scratch → **kept, flagged** |
| attack/corpus-refusals, attack/deps-outbound, attack/py-residue, attack/search-partial, attack/traverse-freshness, attack/verdict-pin, attack2/deps-close, attack2/py-residue-close, attack2/search-partial-close, attack2/swift-close | 1–2 ahead | 10-18 09:53–19:00 | no | all − | each in an unlocked, clean agent worktree | 136's finished agents | — | superseded → **kept** |
| attack2/freshness-close, lane/deps-outbound, lane/py-attack3-fixes, lane/py-residue, lane/search-partial-parse, lane/swift-properties, lane/traverse-freshness, lane/verdict-generation-pin | 0 ahead | 10-18 10:00–21:01 | yes | — | each in an unlocked, clean agent worktree | 136's finished agents | — | merged → **kept** |
| finish/workbench | 4/157 | 10-16 09:15 | no | 3+ | none | — | ft-aeb4a53f backlog | **owner-kept** |
| lane/workbench | 2/265 | 10-16 08:29 | no | 2+ | none | — | ft-4966b304 backlog | **owner-kept** |
| worktree-agent-<id> ×22 | 0 ahead | 21 at `1ae3bc15`, 1 new | yes | — | none | 136's harness base pointers | — | merged → **kept** |

There are no release branches. The remote was not in scope and nothing on it was touched.

## Proof that the superseded branches are superseded

- **Python heritage chain** (attack3/py-followup, attack4/py-wave8, attack5/base-run, attack5/py-close). These are the pre-rebase copies of work that is on main.
  - `git range-diff 103272ea~1..5ec100d2 e2983396~1..b7d088d6` pairs 7 of 8 commits with their copies on main: `103272ea→e2983396`, `87d3d5cf→502a3d6f`, `042133bf=477309d9`, `d5a86ce5=007efe83`, `bf936983=c4487088`, `5de18fd1→3dccc0b5`, `064d4bce=b7d088d6`. The 8th, `5ec100d2`, is cherry `−`.
  - The three `!` deltas are a Cargo.toml context line, the v97 `cache.rs` paragraph (moved into `3dccc0b5`; main `cache.rs:770`, `EXTRACTION_SCHEMA_VERSION = "97"` at `:798`, so the freeze holds), and a treesitter.rs hunk shift.
  - Fable checked line by line. Of 1,526 distinct non-comment lines the chain added, 10 are not verbatim on main. Every identifier in those 10 exists on main in the split submodules, so they were moved or reformatted, not lost.
- **All-`−` attack branches.** Every commit has a patch-equivalent on main (`git cherry`).
- **agent-ac9f `tmp/base` dirty files.** These are not new work:
  - `python_heritage_attack3.rs` is identical to main's blob;
  - `python_field_super_attack3.rs` is identical to the `e2983396` blob;
  - Cargo.toml and it.rs only wire up test targets;
  - `dump_nonpy.rs` and `probe_base.rs` are untracked scratch.

## Merged, conflicts

- **Merged by this session: none.**
  - `accuracy/rustlabel` would merge cleanly (`merge-tree`: +5,046/−19 over 23 files, no overlap with the live lanes). Fable ruled against merging it now, for four reasons:
    - your rule "no shallow review" applies to it;
    - ft-b812a97c is still in review;
    - it would be a third lane running alongside pid 136's two;
    - its `real_corpus_accuracy.rs` hunk predates 6 harness rewrites on main.
- **Conflicts resolved: none.** 136's two landings were its own, and this session did not review them.

## Deleted

Nothing was deleted. Both deletions below were held by `blast-radius` and need your approval:

| Branch | Tip | Backup ref |
|---|---|---|
| attack5/base-run | `8067cb93` | `refs/backup/2026-10-18-pass5/attack5/base-run` |
| attack/base-timing | `5614e2a6` | `refs/backup/2026-10-18-pass5/attack/base-timing` |

## Why branches were kept

- **In use.** The lanes, the integration branch, the worktree-agent pointers and every branch in a locked worktree belong to pid 136. It is merging right now (it landed twice during this pass), and it can resume a finished subagent in that subagent's worktree.
- **Merged or superseded branches in unlocked agent worktrees.** Deleting the branch means removing its worktree. `git worktree remove` silently deletes ignored content, and several of these worktrees hold ignored `tmp/`, `target-*` and `.devcouncil/` (`tmp/bc-ignored.sh` output). Wait until pid 136 exits.
- **accuracy/rustlabel** is kept and flagged, as above.
- **attack2/timing-base** is a scratch timing probe that is not on main. It differs from main's `search_partial_close_attack.rs` by 5 assertion lines (cap 64 vs 16). It is kept and flagged; it can be deleted when pid 136 exits.
- **finish/workbench and lane/workbench** stay under your pass-3 decision: a cross-app dc-store schema change. `stash@{0}` is related to them.

## Blocked, and what unblocks it

| What | Needs |
|---|---|
| `git branch -D attack5/base-run attack/base-timing` | your approval in the blast-radius prompt |
| accuracy/rustlabel | a deep review, ci:local at low load, and no more than 2 lanes running |
| 8 merged + 14 superseded branches and their worktrees, worktree-agent-* ×22 | pid 136 exits; then survey and move ignored `tmp/` content, `git worktree remove`, and `branch -d` (merged) or `-D` (superseded) |

## Backups

- `refs/backup/2026-10-18-pass5/<branch>`: all 58 branch tips at 21:59, with main at `79c721f5`.
- `refs/backup/2026-10-18-pass5-post/`: refs that moved afterwards, as of 22:17:
  - `main` and `integration/wave7` at `e01e6f0d`;
  - `attack/go-type-use` at `cb4c4494`;
  - `lane/go-type-use-dead` at `cb7c54a4`;
  - `lane/go-qualifier-leak` at `4d6e9aa0`.
- Bundle: `/Users/bharath/Code/devtools/DevCouncil-branches-2026-10-18-pass5.bundle` (33.5 MB, 116 heads = 58 branches + 58 backup refs). `git bundle verify` says it "records a complete history".

## Verification

- `git fsck --connectivity-only --no-dangling` printed no errors.
- **Final ci:local was not run.** This session changed no code on main; the two commits that moved main were pid 136's. Running it would have shared the main checkout and cargo target with that live session at load 33–48, and your rule treats tests run under that load as no evidence.
- The final `gitpulse_insights` (22:19) was deferred on every facet again. No branch checked out by a live session was touched; I confirmed that from `git worktree list`.

## Restore commands

```sh
git branch <name> refs/backup/2026-10-18-pass5/<name>
# from the bundle, in any clone:
git fetch /Users/bharath/Code/devtools/DevCouncil-branches-2026-10-18-pass5.bundle 'refs/heads/<name>:refs/heads/<name>'
```
