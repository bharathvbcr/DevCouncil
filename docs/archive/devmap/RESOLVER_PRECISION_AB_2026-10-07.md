# Resolver precision A/B — 2026-10-07

Branch `claude/devmap-resolver-precision`, measured against its merge base
`4cee6c50`. This covers three things: cross-crate Rust calls, calls on typed
receivers, and a precision corpus with enough cases to catch either one
regressing.

## Method

Each corpus is built twice with `devmap --db <isolated store> build --full`
and `DEVMAP_AUTOSPAWN=0`. One build uses a release binary from the merge base
and the other uses one from the branch head. Both builds read the same frozen
tree. The comparison covers three things: the dead list (`devmap --json dead`),
the per-language resolution counts (`devmap --json status`), and the set of
current edges (`edge_rows WHERE valid_to IS NULL`).

- **DevCouncil**: this worktree. It contains the code under change, so it is
  the corpus most likely to agree with the change.
- **MarkDev** (`20dec7a8`, clean tree, Swift/Rust/Python): an independent
  corpus. Nothing in this branch was written against it.

## Final: merge base vs branch head

| | DevCouncil base | DevCouncil head | MarkDev base | MarkDev head |
|---|---:|---:|---:|---:|
| dead rows | 36 | 25 | 5 | 5 |
| edges | 53,060 | 54,423 | 35,417 | 35,480 |
| edges lost / gained | | 3 / 1,363 | | 0 / 63 |
| Rust resolved / unresolved | 28,230 / 119,768 | 29,576 / 118,145 | 2,254 / 10,118 | 2,317 / 10,050 |
| Go resolved / unresolved | 6,563 / 29,449 | 6,568 / 29,443 | — | — |
| Python resolved / unresolved | 1,645 / 6,502 | 1,657 / 6,499 | 1,067 / 4,311 | 1,067 / 4,311 |
| Swift resolved / unresolved | — | — | 17,905 / 39,145 | 17,905 / 39,145 |

**The dead rows that left the list (DevCouncil, 11 rows):**

- **Bound by a typed receiver (9).**
  - `Priority.valid`, `Source.valid` and `VerificationMethod.valid` are Go
    methods on a `var w T` value.
  - `Collected.merge` and `Collected.can_skip` are reached through a
    `Mutex` lock guard.
  - `MirroredHeaders.check`, plus `PathRanks.len`, `PathRanks.path_of` and
    `PathRanks.rank_of`, are reached through Rust locals typed from a binder
    or a function header.
- **Duplicates (2).** `PinnedDir.inspect_child` and `ShutdownSignals.recv`
  were each reported once per `cfg` variant. Each is now one row. Both
  methods are still on the list once.

No row was added to either dead list. MarkDev's five rows are the same before
and after.

**The 3 lost edges were misbindings, now corrected.** `enhancements::mutate`
was bound twice from `workbench/automation.rs` to the parent `mod.rs::mutate`,
and `briefs::get` was bound from `workbench/runs.rs` to `mod.rs::get`. Each
call now binds to the module the path names: `enhancements.rs` and
`briefs.rs`.

## Per-step deltas (DevCouncil / MarkDev)

These are taken from intermediate stores during development. Each row is
measured against the row before it, so the absolute counts differ from the
final table, whose tree also contains the later test files.

| Step | Dead | Edges lost | Edges gained | Note |
|---|---|---:|---:|---|
| Rust cross-crate paths | unchanged / unchanged | 3 / 0 | 512 / 34 | Rust unresolved 118,633 → 118,051 and 10,118 → 10,083 |
| Go `var w T`, cfg dedupe | 36 → 31 / unchanged | 0 / 0 | 58 / 0 | 3 Go `valid` rows, 2 duplicates |
| Rust binder: lock guards, loop variables | 31 → 30 / unchanged | 1 / 0 | 17 / 0 | `can_skip`; the lost edge was in code this step changed |
| Rust function headers: `T::f()?`, closure parameters | 30 → 25 / unchanged | 0 / 0 | 815 / 28 | `merge`, `MirroredHeaders.check`, `PathRanks.*` |
| Python parameter annotations | unchanged / unchanged | 0 / 0 | 3 / 0 | |
| Python module-scope re-export hop | unchanged / unchanged | 0 / 0 | 10 / 0 | |

## Real-corpus resolution baseline

The resolution baseline for MarkDev is measured separately by
`rust/devmap-resolve/tests/real_corpus_resolution.rs` (`#[ignore]`, needs
`DEVMAP_MARKDEV_ROOT`). That test uses the labelled sample in
`rust/testdata/realcorpus/markdev_resolution_sample.json`. The numbers below
are copied from the sample's `baseline`.

**How the sample was drawn.** The sample has 40 call sites. Each comes from a
caller that makes exactly one call to that name. It is stratified by language
(Swift, Rust, Python) and by whether the resolver bound the site: 8+8, 7+7
and 5+5. The labels were made by reading the source without seeing what the
resolver answered. On that sample:

- **Precision:** 19/20 (950‰).
- **Recall:** 19/21 (904‰).
- **Abstentions:** 18 sites are external calls, and the resolver correctly
  bound none of them.

These are numbers for this sample, not estimates for the whole corpus.

**What the errors are:**

- **The wrong binding.** An XCTest `XCTAssertTrue` call was bound to a
  `private` function at file scope in another test file. The `UniqueGlobal`
  rung does not honour Swift's file-private visibility. This is a Swift gap
  that predates this branch.
- **The two misses.** Both are Swift method calls whose receiver is typed
  only by a return type: one is a chained call, the other is a helper's
  return.

**Confirmed by breaking a fix.** Disabling `type_from_rust_header` loses
`vault.note(..)` (`let mut vault = Vault::open(&root)`) and fails the floor:
recall drops to 857‰.

## Named callers (`devmap impact`)

The brief named three callers. `devmap impact --depth 1` was run against the
two final stores above. On the head store it reports each caller as an
`ImportScoped` Calls edge. On the merge-base store it reports none of them:

- `session.rs::build_report` → `session_log.rs::read_live`
- `dcgrep.rs::index` → `dc_grep::build_index`, declared in `index.rs`
- dc-verify `classify_scope` → `dc_glob::matches`
