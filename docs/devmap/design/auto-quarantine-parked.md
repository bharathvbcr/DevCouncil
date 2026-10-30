# Auto-quarantine of a damaged store: parked

Status: parked (Fable ruling, 2026-10-30). Lane `damaged-store-recovery` is cut to
detect and refuse. This note is what a resume starts from.
Cards: `ft-6bcae99cd8aea8538b677a3660c18cbe` (task_id
`store-ownership-first-and-quarantine-resume`: F3 and the resume), `ft-3b886056`
(F2, the daemon), `ft-72758783` (the damaged `unresolved_rows` page).

## What was tried, and what remains

`devmap build` used to move a damaged store aside (`<name>.corrupt-<unix
seconds>`, with its `-wal` and `-shm`) and build again from source. The mover
(`Store::quarantine_damaged`, its naming logic, `Store::quick_check`, the
`SQLITE_CORRUPT` retry in `build`, and the `store_recovery` key of the build
JSON) was deleted, not disabled. It is in the history of this branch at
`0f254876` (and the attack round that broke it is `attack/dsr` @ `8da36922`).

What stays, and is all `build` does with a damaged store now:

- `status` reports `query_ready: false` and `search_index_damaged: true` with
  the probe's finding as `degraded_reason`;
- `build` reads the owner and then the search index before it writes anything,
  and refuses with the store's path as given, the finding, and the one thing to
  do: move or delete it and run `devmap build` again. Any `SQLITE_CORRUPT` the
  build dies on is reported in the same shape, naming the store;
- there is no `store_recovery` key: nothing is recovered, so there is nothing to
  report.

## F1: the BLOCK that parked it

Verbatim from the round: a pre-v20 store has no `pending_state`, so its owner
lives only in `generations.repo_root`. Smash its generations page. The v20
migration rolls back, `refuse_if_owned_by_another_live_worktree` returns Ok
("predates the recorded owner"), and `quarantine_damaged` moves another live
worktree's store aside. Base refuses and moves nothing.

Test: `attack_dsr_quarantine_ownership::a_live_worktrees_store_from_before_pending_state_is_not_set_aside_by_another_worktree`.
It passes on the cut, because nothing moves: B is refused and A's store is
identical byte for byte and inode for inode.

## F3: `Store::open` migrates before any ownership check

Pre-existing on base `dbd0193a`. `Store::open` migrates a store before any
ownership check runs. Worktree B runs `build --db <A's v26 store>`; B's build is
refused for ownership, but A's store is already at v27 when the refusal comes.
A refused command has mutated another worktree's data.

Test: `attack_dsr_quarantine_ownership::a_build_refused_for_ownership_has_not_migrated_the_other_worktrees_store`
(`#[ignore]`, card `ft-6bcae99cd8aea8538b677a3660c18cbe`). Output on base and on
this branch: `B's refused build migrated A's store  left: 27  right: 26`. This
lane's reordering does not reach `Store::open`: it puts the ownership read and
the integrity probe before `bind_repo_root`, the only write `build` makes after
open, but the migration inside `open` is still first.

## The rule: ownership first

No write to a store (migration, index heal, digest backfill, quarantine) before
ownership is proven from a read that survives the damage being checked for, and
unprovable means refuse. It applies to every open path, including the v28
heal-on-open and digest backfill, whose landing must check its open order against
it.

Ownership sources, and what each does not survive:

| Source | Schemas | Lost when |
|---|---|---|
| `generations.repo_root` | 7 onward | the `generations` page is damaged |
| `pending_state.repo_root` | 20 onward (copied from `generations` by the v19 to v20 migration) | the `pending_state` page is damaged, or the migration that creates it rolls back |

A store older than 20 has only the first, and a damaged page of either source
leaves nothing to read: the check that asked only the newer source and read its
absence as "unowned" is F1. No single source survives damage to its own page, so
"could not read the owner" has to be a refusal. That is what the cut does.

Order of operations on open and build once the rule is met: prove ownership
(a read that does not migrate), then the integrity probe, then any mutation. Today
`build` does the last two in that order after `Store::open`; F3 is the first.

## Resuming

Prerequisites: F3 (`ft-6bcae99cd8aea8538b677a3660c18cbe`), and F2, the daemon
comparing the store's file (device and inode) and not its name
(`ft-3b886056`, test `daemon::tests::a_daemon_whose_store_was_replaced_at_the_same_path_exits`,
`#[ignore]`), because a set-aside followed by a fresh store at the same path is
exactly what the daemon does not see.

Acceptance: every test of `attack/dsr` passes on the resumed head, and one
independent round finds 0 BLOCKs in "moves a store that is not ours". Stop at one
BLOCK in that class.

Not yet probed: the `-wal` and `-shm` rename window, and a cross-process
quarantine-name collision (two builds choosing the same `.corrupt-<seconds>`).

Separately, `ft-72758783`: a smashed `unresolved_rows` page leaves `status` ready
and `build` saying "unchanged". It is outside this seam: the probe covers the
search index, and a probe of a table that size is a cost decision.
