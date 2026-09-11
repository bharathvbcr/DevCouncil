# Kernel robustness audit — 2026-09-05

**Preserved from a session scratchpad.** This report was produced by an audit
pass over the Rust kernel and existed only under `/private/tmp`, which does not
survive the session. It is checked in because roughly a third of its findings
are still open and nobody should have to re-derive them.

53 defects from ~486 candidates examined; 6 critical; 7 proven end-to-end
against the shipped binary. Each row carries file:line, the exact failing input,
the wrong output it produced, the evidence status, and a fix sketch.

Two parts of it are as valuable as the findings and are easy to miss:

* Every section ends with a **"checked and CLEAN — do not redo"** paragraph
  naming what was examined and found sound (R4 determinism, panic and bounds
  sites, builtin tables, FTS5 escaping). Read those before re-auditing anything.
* Every section carries a **denominators table** — candidates examined versus
  real defects — so the coverage claim is a ratio rather than an assertion.

**Status as of 2026-09-05.** Closed and verified: E-1, E-2 (process-aborting
panics); E-3, E-4, E-5, E-6, E-7 (capped reads); Q-1, Q-2, Q-5 (coverage loss
reaching no output); Q-3, Q-4, Q-7; S-1, S-3, S-6, S-9, S-10; S-2, S-4, S-5,
S-8, S-11. **S-7 was falsified** — rusqlite 0.31 already sets a 5 s busy timeout
on every connection, so the wait the audit called missing was present; the call
was made explicit anyway rather than left to a dependency default.

**Status as of 2026-09-05, evening** (three fix lanes plus the reconcile pass;
see STATUS.md's dated 2026-09-05 sections). Closed with red-first tests:

* **K-A2** — a discovery refusal is now coverage loss with one owner:
  `devmap_extract::DiscoverySkipReason::is_refusal` (`devmap-extract/src/model.rs:385`)
  decides what counts, `devmap_analyze::liveness::DiscoveryCoverage` charges it,
  and the count leaves the kernel as `discovery_refused_files`, kept apart from
  parse failures, on the code graph and both `build --json` shapes. Pinned by
  `devmap-analyze/tests/discovery_refusals_are_coverage_loss.rs`,
  `devmap-serve/tests/daemon_discovery_refusals.rs` and
  `devmap-cli/tests/discovery_refusal_is_coverage_loss.rs`.
* **K-A4** — `search`'s `total`/`hidden`/`truncated` are counted inside the
  caller's own transaction (`Store::generation_counts_locked`,
  `devmap-store/src/db.rs:3072`), so they cannot straddle two generations.
* **K-A6** — freshness has one owner, `devmap_serve::index_is_fresh` /
  `freshness_degraded_reason` (`devmap-serve/src/protocol.rs:404,416`), and a
  store holding zero generations is not fresh on any surface.
* **K-B1** — `MAX_TOKEN_BUDGET` now bounds the *work*: a search page is
  `budget_page_size(budget).min(SEARCH_PAGE_MAX)` (200) and each hit reads only
  the prefix its span needs (`read_source_prefix`), not the whole file.
* **K-B2** — the notify→thread channel is a bounded `sync_channel`
  (`WATCH_QUEUE_CAPACITY`) and `DebounceBuffer.pending` is capped at
  `MAX_DEBOUNCE_PATHS`.
* **K-B3** — `read_is_stable` takes the two `modified()` options explicitly and
  a filesystem that cannot report them fails loudly instead of degrading to a
  length-only check.
* **K-B4** — `claim_of`'s linear scan is a `HashMap` index built once per drain
  (`devmap-serve/src/daemon.rs:659`).
* **E-8** — `recover_lock` (`devmap-extract/src/lib.rs:158`, crate-private —
  both ends of the prune-ledger mutex go through it) takes the inner guard of a
  poisoned mutex, so a poisoned prune ledger can no longer silently empty
  `DiscoveryReport::skipped_paths`. Pinned by
  `devmap-extract/src/lib.rs::prune_ledger_tests::a_poisoned_prune_ledger_is_recovered_rather_than_silently_dropped`.
* **Q-12** — the per-edge `format!("{:?}", edge.edge_kind)` is gone; edge kinds
  are interned once per generation (`devmap_store::edge_kind_from_stored`,
  `GraphIndex::kind_label`).
* Every **R-\*** row (`devmap-resolve/tests/audit_regressions.rs`).

Verified already closed by earlier commits and pinned with tests: **K-A1**,
**K-A3**, **K-A5**, **Q-6**, **Q-8**, **Q-10**. **Q-11** is not a defect and must not
be re-raised: `pdg.rs` (`devmap-analyze/src/pdg.rs`) is the Phase 7.1 adjunct
port in progress (AGENT_PLAN.md §7.1, STATUS.md "PDG kernel"), unwired by plan
rather than dead, and stays.

**Where these fixes live.** Two lines carried the same audit concurrently. The
implementations above are the ones on the reconcile branch
`claude/devmap-reconcile-1a2151`, which is `main` plus the round-1 work ported
onto it; K-A2, K-A4, K-A6, K-B2 and K-B3 are `main`'s own implementations kept
in the merge, K-B1, E-8 and Q-12 are the ported ones, and K-B4 was written
independently on both lines (`main`'s implementation kept). Where both lines
had an implementation, exactly one survives — see STATUS.md, "Port of the 1a2151
round-1 work onto main (2026-09-05)", for which side each piece came from.

Check `STATUS.md` for anything closed after this header was written; where this
report and the code disagree, the code wins.

---

# DevMap Rust kernel — robustness audit

Scope audited: `devmap-extract`, `devmap-resolve`, `devmap-analyze`, `devmap-store`,
`devmap-query`, and `devmap-serve`'s `daemon.rs` / `watcher.rs` / `protocol.rs`.
Out of scope by instruction: `devmap-serve/src/mcp.rs`, `mcp_http.rs`.

Working tree: `/Users/bharath/Code/devtools/DevCouncil/.claude/worktrees/dev-map-hardening-mcp-0b79fc`
Binary used for the runnable proofs: `rust-port/target/debug/devmap` (verified newer than every
`crates/**/src/*.rs`; only `crates/devmap-serve/tests/mcp_concurrency.rs` is newer).

Every claim is labelled **VERIFIED** (I executed it, or I read that exact line),
**INFERRED**, or **UNVERIFIED**. Nothing inferred is presented as verified.

---

## Result: 53 real defects from ~486 candidates examined

By crate (each finding counted once):

| Crate | Candidates examined | Real |
|---|---|---|
| devmap-serve (daemon/watcher/protocol) + cross-cutting freshness | 61 | 10 |
| devmap-resolve | 62 | 12 |
| devmap-store | ~90 | 11 |
| devmap-query + devmap-analyze | ~180 | 12 |
| devmap-extract | ~93 | 8 |
| **Total** | **~486** | **53** |

By category (categories overlap — one finding can be both Class A and R7, so this column sums
higher than 53):

| Category | Candidates | Real |
|---|---|---|
| Class A (a check that could not run reporting as one that passed) | 116 | **25** |
| R5 confidence honesty | 21 | 4 |
| R6 failure ≠ emptiness | 10 | 1 |
| R7 counts / rank-before-truncate | 52 | 9 |
| R4 determinism | ~70 | **0 live** (3 latent) |
| Store integrity | 9 | 3 |
| Concurrency / daemon lifecycle | 11 | 2 |
| Resource bounds + reachable panics | ~230 | 12 (**3 process-aborting**) |
| Regex / query injection | 4 | **0** |

Severity: **6 critical** (K-A1, K-A2, S-1, Q-1, E-1, E-2), 8 high, 18 medium, 21 low.
Of the 25 Class A findings, **7 were proven end-to-end against the shipped `devmap` binary**
rather than argued from source.

## The five highest-value findings

Ranked. Each is written up in full below or in the per-crate sections; IDs are stable.

1. **Coverage loss reaches no output at all** — `K-A2` + `Q-1` + `Q-2`, converging from two
   independent directions (discovery refusal and parse failure). `devmap dead` proposes deleting a
   live symbol at the **top** confidence tier with `resolution: "Available"`, `truncated: false`,
   while `graph_degraded: false` and `is_fresh: true`. Proven end-to-end through the real CLI.
2. **`--min-confidence nan` turns a filter that cannot evaluate into "no calls are affected"** —
   `S-1`. `callers_of` is the one of three surfaces missing the guard that already exists.
   `devmap deps` refuses the same input loudly; `devmap preview` lies.
3. **Two one-line source files abort `devmap build`** — `E-1` (exit 134, stack overflow) and
   `E-2` (exit 101, slice panic on ordinary JavaScript). Both reproduced through the shipped binary.
4. **One malformed glob in any `.gitignore` silently freezes the incremental index** — `K-A1`,
   while every freshness surface keeps reporting `is_fresh: true`.
5. **The OS says "I dropped events, rescan"; the watcher discards it and never self-heals** —
   `K-A3`.

Next tier, all Class A: `Q-3`/`Q-4` (a panic and a `.ok()` that erases a real deletion in
`preview`), `Q-7` (a depth-capped walk returning `truncated: false`), `K-A4` (`search`'s counts
straddle two generations), `R-1`/`R-2` (name-only resolutions stamped `DETERMINISTIC`, and a
coincidence rung outranking explicit imports), `S-3` (a failed `stat` deleting queued work).

---

### K-A1 — one malformed glob in any `.gitignore` silently freezes the incremental index, and every freshness surface still reports `is_fresh: true`
**Severity: critical (Class A).**
`devmap-extract/src/lib.rs:249-251`, `devmap-serve/src/watcher.rs:63 / 313-319`,
`devmap-store/src/db.rs:2627`, `devmap-serve/src/protocol.rs:326`.

`add_ignore_rules` treats **any** return from `ignore::gitignore::GitignoreBuilder::add`
as fatal:

```rust
// devmap-extract/src/lib.rs:249
if let Some(error) = builder.add(rules) {
    anyhow::bail!("cannot parse ignore rules {rules:?}: {error}");
}
```

`add()` returns *partial* errors — one bad glob line, with every other line still added.
The error then propagates `is_gitignored` → `IgnoreVerdictCache::is_ignored` (watcher.rs:63,
`?`) → `admitted_watch_path` → the watcher's own catch:

```rust
// devmap-serve/src/watcher.rs:315
match admitted_watch_path(&root, &path, &mut ignore_cache) {
    Ok(path) => path,
    Err(error) => { warn!("failed to evaluate watcher path {path:?}: {error}"); None }
}
```

`None` means *not a path to index*, which is exactly what a genuinely-ignored path returns.
So a check that **could not run** produces the same value as a check that ran and said "ignore".
The verdict cache is only written on success, so this repeats on every event, forever.

**Runnable proof (VERIFIED).** Scratch crate at `scratchpad/ignoreprobe`, run against
`ignore 0.4`:

```
unclosed-class   add() -> None
unclosed-class   WalkBuilder -> 2 entries, 0 errors
bad-range        add() -> Some(".gitignore: line 2: error parsing glob '[z-a]': invalid range; 'z' > 'a'")
bad-range        WalkBuilder -> 2 entries, 0 errors
valid            add() -> None
```

`[z-a]` is a plausible typo that `git` itself tolerates. Note the second line of each pair:
`WalkBuilder` — which the **cold `devmap build`** path uses — walks the same tree with
**0 errors**. So the cold build indexes everything and the watcher drops everything, which
also refutes the doc comment at `devmap-extract/src/lib.rs:152-157` ("Evaluate Git ignore rules
the same way `WalkBuilder` does … or incremental generations admit files the next cold build drops").
The divergence is the opposite of the one that comment guards against, and it is silent.

**Concrete failure.** Repo has `.gitignore` line `[z-a]`. Developer edits `src/auth.py`.
Watcher event arrives → path dropped with a `warn!` nobody reads → `pending_paths` stays empty
→ `Store::status` sets `degraded_reason = None` (db.rs:2627 sets it *only* for quarantined
pending paths) → IPC `status` answers `"is_fresh": status.pending_count == 0` = **true**
(protocol.rs:326) → `devmap_client.py:1055` `is_map_stale() = not is_fresh or pending_count > 0`
= **False** → `codeintel.py:213` `graph_degraded = bool(degraded_reason)` = **False**.
An agent asking "is the map fresh?" is told **yes** about an index frozen at the last cold build.

**Fix sketch.** Two changes, both needed. (a) In `add_ignore_rules`, keep the partially-built
matcher and record the bad-line diagnostic instead of `bail!` — match `WalkBuilder`'s tolerance,
which the comment already promises. (b) In `watcher.rs:315`, fail *open*: an ignore verdict that
could not be computed must enqueue the path (a redundant re-extract costs time; a dropped one
costs correctness) — the same reasoning `daemon.rs:611-634 head_differs_from_last_generation`
already applies ("Fails *safe*, not quiet").

---

### K-A2 — files refused by discovery, and files whose parse failed, are invisible in `status`, `repo_map.json` and `code_graph.json`; `graph_degraded` is `false`
**Severity: high (Class A).**
`devmap-query/src/manifest.rs:232-236`, `devmap-query/src/code_graph.rs:300-310 / 567-576`,
`devmap-store/src/db.rs:2627-2648`, `devmap-serve/src/daemon.rs:378-401`, `devmap-cli/src/main.rs:1126-1136`.

`graph_degraded` was fixed once (manifest.rs:682-714 pins the regression), but it is derived
**only** from `AnalysisStatus`:

```rust
// devmap-query/src/manifest.rs:232
let (graph_degraded, graph_degraded_reason) = match &analysis.status {
    AnalysisStatus::Ok      => (false, String::new()),
    AnalysisStatus::Partial { reason } => (true, format!("partial: {reason}")),
    AnalysisStatus::Timeout { reason } => (true, format!("timeout: {reason}")),
};
```

Nothing folds in the **coverage** half: discovery refusals (`Oversized` / `Unreadable` /
`NonUtf8Path`) or `ParseOutcome::Failed`. `GraphProvenance` (code_graph.rs:255-271) counts
`files_without_readable_source` and `regex_fallback_files` — but the first counts only files
*already in the generation* whose source cannot be re-read at render time, and there is **no
counter for `ParseOutcome::Failed` at all** (`rg 'ParseOutcome::Failed' devmap-query/src/` returns
nothing). Discovery refusals never enter `extractions`, so they are invisible everywhere.
The daemon's connect-time sweep logs them and explicitly does *not* queue them
(daemon.rs:378-401), and `devmap build` prints them to stderr only (main.rs:1126-1136).

**Runnable proof A (VERIFIED).** Repo of 3 files: `small.py`, a 2.4 MB `huge.py`
(> `MAX_SOURCE_BYTES` = 1 MiB), and a `chmod 000` `unreadable.py`.

```
$ devmap build …
  discovery refused 2 file(s) — these are absent from the graph:
    huge.py: Oversized { bytes: 2400000, limit: 1048576 }
    unreadable.py: Unreadable { reason: "Permission denied (os error 13)" }
  Files indexed: 1

$ devmap --json status
{"degraded_reason":null,"is_fresh":true,"quarantined_count":0,"node_count":2,"edge_count":1,…}
```

`repo_map.json`: `graph_degraded = False`, `graph_degraded_reason = ''`.

**Runnable proof B (VERIFIED).** Repo of `small.py` + `broken.ipynb` containing
`{ this is not valid json at all` → `ParseOutcome::Failed { "notebook is not valid JSON" }`
(notebook.rs:149-158).

```
$ devmap --json status
{"degraded_reason":null,"is_fresh":true,…,"node_count":3}

repo_map.json:   graph_degraded = False | reason = ''
repo_map.json:   files = [{"kind":"code","language":"notebook","path":"broken.ipynb"}, {"…":"small.py"}]
code_graph.json: meta.devmap_rust = {"analysis_status":"ok","files_without_readable_source":0,
                                     "regex_fallback_files":0, …}
code_graph.json: node for broken.ipynb = {"kind":"file","name":"broken.ipynb", …}   # zero symbols
```

The failed file is listed as an ordinary `kind: "code"` entry, indistinguishable from a file
that parsed cleanly and genuinely declares nothing.

**Runnable proof C — the end-to-end wrong answer (VERIFIED).** `lib.py` defines `helper()`;
`app.py` is 1,162,056 bytes (just over the 1 MiB ceiling) and contains
`from lib import helper` / `return helper()`. It is `app.py`'s *only* caller.

```
$ devmap build …
  discovery refused 1 file(s) — these are absent from the graph:
  Files indexed: 1

$ devmap --json dead
{"hidden":0,"items":[{"confidence":0.8999999761581421,"exemption_reason":null,
  "file_path":"lib.py","is_exempt":false,"symbol_name":"helper"}],
 "resolution":"Available","shown":1,"tokens_used":30,"total":1,"truncated":false}

$ devmap --json status
{"degraded_reason":null,"is_fresh":true,…}
```

`helper` is reported **dead at confidence 0.9**, with `resolution: "Available"`,
`truncated: false`, `hidden: 0` — the reachability check could not read the one file that calls
it, and the answer is byte-identical in shape and confidence to a check that examined everything.
A developer or agent acting on this deletes a live symbol.

**Concrete failure in the wild.** Any real repo with a vendored ≥1 MiB `parser.c` (the repo's own
comments report a 30 MB one, daemon.rs:388-395). Meanwhile `map_is_stale`'s own fail-closed branch
(`repo_mapper.py:1980  if bool(repo_map.get("graph_degraded")): return True`) cannot fire,
because `graph_degraded` is `False` — the same dead-branch shape the earlier fix removed,
reintroduced by omission rather than by a literal.

**Fix sketch.** Persist a per-generation coverage record (`refused_paths`, `failed_parse_paths`,
with counts) at generation write; fold it into `StoreStatus::degraded_reason` and into
`graph_degraded` / a `meta.devmap_rust.parse_failed_files` counter, carrying **both** numbers
(shown sample + true total) as `quarantined_paths` already does.

---

### K-A3 — the OS says "I dropped events, rescan"; the watcher throws that away and keeps reporting fresh
**Severity: high (Class A).** `devmap-serve/src/watcher.rs:296-323, 328`.

```rust
// watcher.rs:296
let admitted: Vec<String> = if matches!(
    event.kind,
    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
) { … } else {
    Vec::new()      // <-- everything else, silently
};
```

Both backends signal event loss as `EventKind::Other` with `Flag::Rescan`, which lands in that
`else` (VERIFIED by reading the vendored crate):

- macOS: `~/.cargo/registry/src/*/notify-6.1.1/src/fsevent.rs:114-123` —
  `if flags.contains(StreamFlags::MUST_SCAN_SUBDIRS) { Event::new(EventKind::Other).set_flag(Flag::Rescan) }`,
  info `"rescan: kernel dropped"` / `"rescan: user dropped"`.
- Linux: `.../notify-6.1.1/src/inotify.rs:208-209` — `EventMask::Q_OVERFLOW` → same event.

`rg 'need_rescan|Flag::Rescan|EventKind::Other|rescan' rust-port/` returns **no** hits in the
kernel — the signal is not handled anywhere. Separately, `watcher.rs:328`
`Ok(Err(e)) => warn!("Watch error: {:?}", e)` swallows `notify::ErrorKind::MaxFilesWatch`
("OS file watch limit reached", error.rs:34/108), which means whole subtrees are no longer watched.

**Concrete failure.** `git checkout` of a large branch, or a `cargo build` storm, overflows the
FSEvents/inotify queue. The kernel tells the daemon to rescan. The daemon discards the message,
`pending_count` stays 0, `status` answers `is_fresh: true`, and every later query answers from a
generation that is missing an arbitrary, unknowable subset of the edits.

**And it never self-heals.** `reconcile_connect_time` — the only full stat-and-hash sweep — is
called exactly once, at `daemon.rs:829` (VERIFIED: `rg reconcile_connect_time` finds one non-test
call site). The 300 s maintenance task does `checkpoint_wal` + `vacuum_if_needed` only. So the
gap persists until the daemon restarts, and the idle bound cannot end it either: a repository
busy enough to overflow the event queue is a repository that never looks idle
(the same reasoning the binary-retirement comment at daemon.rs:962-966 already spells out).

**Fix sketch.** On `event.need_rescan()` (or any `Watch error`), enqueue the repo root
(`db.rs:174` already documents the root as "a whole-tree rescan the drain expands") **and** set a
persisted `watcher_degraded` reason that `Store::status` reports until the next successful
full sweep. Do not let a lost-coverage watcher be indistinguishable from a quiet one.

---

### K-A4 — `search` overwrites `total` from a second, independently-locked query; the R7 numbers are not computed by the truncation function
**Severity: medium-high (Class A / R7).** `devmap-query/src/engine.rs:65-99`,
`devmap-store/src/db.rs:2729-2799`.

```rust
let total = self.store.count_search_symbols(&req.query)?;                       // lock #1, generation N
let rows  = self.store.search_symbols(&req.query, budget_page_size(budget))?;    // lock #2, generation N or N+1
…
let mut response = budget_take(hits, req.token_budget, search_hit_tokens);
response.total     = total;                              // clobbers budget_take's own total
response.hidden    = total.saturating_sub(response.shown);
response.truncated = response.hidden > 0;
```

`Store` is a single `Mutex<Connection>` (db.rs:290) and each method independently re-resolves
`latest_generation_id_locked` (db.rs:2734, 2778) — there is no pinned read transaction. The
codebase already measured and fixed exactly this for the composed surface
(`engine.rs:158-190`: "A composed answer must come from one generation … measured at 25 of 62
composed answers under contention"), and its comment concedes `impact`/`deps` "straddled the
same way". Under the daemon, `search` is one exchange sold as one answer, and the daemon commits
a generation on every watcher batch.

**Concrete failure (index grew mid-query).** Gen N has 5 hits for `"handler"`; a drain commits
gen N+1 with 50 before the second call. `total = 5`, `rows = 50`, budget shows 30 →
`hidden = 5.saturating_sub(30) = 0` → `truncated = **false**`, with 20 hits dropped. The response
also violates its own documented invariant (`model.rs:63-64` "clients enforce
`shown + hidden == total`"), so the Python client raises `DevMapClientError`
(`devmap_client.py:694`) — loud, but wrong-shaped.
**The quieter direction is worse:** gen shrinks 50 → 5. `total = 50`, `shown = 5`,
`hidden = 45`, `truncated = true` — the invariant *holds*, no client raises, and the caller is
told "45 more, raise your budget" when all 5 that exist were shown.

Secondary, independent of the race: `count_search_symbols` counts
`nodes_fts ⋈ nodes_fts_map` while `search_symbols` additionally requires
`JOIN generation_nodes … JOIN paths` — the denominator is computed by a *different* query than
the numerator, so any FTS row without a live node/path row inflates `total` and `hidden`.
(INFERRED — I read both statements; I did not construct a store with an orphaned FTS row.)

**Fix sketch.** Take one read transaction (or one `lock_conn`) for both statements and pin
`generation_id` explicitly, or apply the `neighbors` before/after generation check to `search`,
`impact`, `trace`, `deps`, `dead` and `clones` and surface a straddle via `walk_incomplete`.
Better: have the store return `(rows, total)` from one call so `budget_take` owns all four numbers.

---

### K-A5 — two of the daemon's six exit paths skip the endpoint release the code documents as mandatory
**Severity: medium.** `devmap-serve/src/daemon.rs:26-35, 972, 987` (vs. 904, 912, 927, 1018).

`release_ipc_endpoint`'s own doc states the requirement:

> `abort()` only schedules the task's future to be dropped, and the socket file is removed by that
> drop (`UnixIpcServer::drop`). Returning from `run_loop` without awaiting leaves a window in which
> the process can exit with the endpoint still on disk — which is the same stale-socket state a
> `kill -9` leaves, arrived at through the orderly path.

Exit inventory (VERIFIED by `rg -n 'return Ok\(\(\)\)|release_ipc_endpoint' daemon.rs`):

| line | reason | releases? |
|---|---|---|
| 890 | IPC task ended on its own | n/a (future already complete) |
| 904 | SIGTERM / SIGINT | yes |
| 912 | `request_shutdown` | yes |
| 927 | tree or store vanished | yes |
| **972** | **binary replaced on disk** | **no** |
| **987** | **idle retirement** | **no** |
| 1018 | drain failed + vanished | yes |

Idle retirement is the *routine* exit (`DEFAULT_MAX_IDLE_SECS = 1800`), so the untreated path is
the common one. A retiring daemon that has not yet dropped its listener answers
`probe_endpoint_liveness` (protocol.rs:756-767) with `Some(true)`, and the next client's daemon
**refuses to start**: `anyhow::bail!("devmap IPC endpoint is already active at {path:?}")`
(protocol.rs:790). Compounding: the drain is `.await`ed *inside* the `ticker.tick()` select arm
(daemon.rs:993-999) with no timeout, so a long drain blocks the signal arms too — a SIGTERM
during a full-tree resync is not serviced until the resync completes, and a supervisor's
follow-up SIGKILL then leaves the socket for real. (INFERRED for the SIGKILL sequence;
VERIFIED for the select structure and the missing calls.)

**Fix sketch.** Call `release_ipc_endpoint(&mut ipc_task).await` on both paths (one line each), or
restructure so every `return` from `run_loop` goes through one release helper. Separately, bound
the drain with `tokio::time::timeout` so shutdown arms stay live.

---

### S-1 — `--min-confidence nan` makes `preview` claim "no calls from other files are affected"
**Severity: critical (Class A).** `devmap-store/src/db.rs:2944-2977`.

`callers_of` is the **only** confidence-filtered edge query that does not call
`checked_min_confidence` — the guard exists (db.rs:565-574, "min_confidence must be a number; got
NaN, which no confidence comparison can evaluate") and is applied at db.rs:2999 and 3047. rusqlite
binds `f32::NAN` as `Value::Real` → `sqlite3_bind_double` → SQLite stores **NULL** → the predicate
`CAST(ROUND(e.confidence*1000) AS INTEGER) >= CAST(ROUND(?3*1000) AS INTEGER)` is NULL for every row
→ `Ok(vec![])`. The `is_finite()` check exists only in `devmap-serve`'s `validate_request`
(protocol.rs:299); the CLI arg is a bare `f32` (main.rs:435 → 1774 → engine.rs:844).

**Runnable proof (VERIFIED).** `lib.py::helper` with two deterministic callers, `c1.py` and `c2.py`:

```
$ devmap preview --file lib.py --content new.py --min-confidence 0.0     # control
  affects  c1.py::a  ->  lib.py::helper  (1.00)
  affects  c2.py::b  ->  lib.py::helper  (1.00)

$ devmap preview --file lib.py --content new.py --min-confidence nan     # attack
  no calls from other files are affected
  2 further call edge(s) fell below the confidence floor and are not listed
    (usually a bare method name matching many definitions); pass --min-confidence 0 to see them
```

JSON: `broken_callers: {"items":[],"shown":0,"hidden":0,"total":0,"truncated":false,
"resolution":"Available"}`. The explanatory line actively misdirects — it blames "a bare method name
matching many definitions" for two edges at confidence **1.00**.

**The contrast that proves the guard works and one surface was missed:**

```
$ devmap deps c1.py --min-confidence nan
Error: Invalid parameter name: min_confidence must be a number; got NaN,
       which no confidence comparison can evaluate
```

Same input, same store, two entry points: one refuses, one lies. No test in
`store_hardening.rs`, `kernel_defects.rs` or `test_fault_injection.rs` touches `callers_of`.

**Fix sketch.** `let min_confidence = checked_min_confidence(min_confidence)?;` as the first line of
`callers_of`, matching db.rs:2999 — then validate `is_finite()` and `0.0..=1.0` at the **CLI**
boundary too, not only in `validate_request`.

---

### E-1 / E-2 — two one-line source files abort `devmap build`
**Severity: critical.** `devmap-extract/src/treesitter.rs:4314` / `:4437`, and `:2370`.

Both reproduced by me through the shipped `target/debug/devmap` binary, not just in a unit harness:

```
# E-2: ordinary JavaScript. `text.find('{')` and `text.find('}')` taken independently.
$ cat util.js
export const isClose = (c) => c === '}' || c === '{';
$ devmap build ./repo
thread 'main' panicked at crates/devmap-extract/src/treesitter.rs:2370:42:
byte range starts at 51 but ends at 37
exit=101
```

```
# E-1: a Go pointer receiver 10,000 levels deep (10 KB file, far under MAX_SOURCE_BYTES = 1 MiB).
$ devmap build ./repo
thread '<unknown>' has overflowed its stack
fatal runtime error: stack overflow, aborting
exit=134
```

E-2 is the more alarming of the two because the input is *valid, idiomatic* JavaScript — any file
with a `}` literal before a `{` literal in an import/export statement. Bracket-matching utilities,
regex constants and template parsers all contain it. Under `extract_all` the panic unwinds out of a
rayon `par_iter` and takes the whole build with it; the release profile is `panic = "abort"`.
`rust_type_name`/`go_type_name` are the only two of six sibling type walkers without the
`depth > 16` guard the other four carry.

Aftermath (VERIFIED, and a defect in its own right — see **K-A6**): the store the crashed build left
behind has no generation, and `devmap status` reports it as `{"generation_id":null,"node_count":0,
"is_fresh":true,"degraded_reason":null}`.

---

## Full findings table (devmap-serve + cross-cutting; my own scope)

| ID | Sev | Crate | File:line | Defect (one sentence) | Failure: input → wrong output | Evidence | Fix sketch |
|---|---|---|---|---|---|---|---|
| K-A1 | critical | extract + serve | `devmap-extract/src/lib.rs:249`; `devmap-serve/src/watcher.rs:63,315` | A partial `GitignoreBuilder::add` error is fatal, and the watcher turns that error into "not indexable", so an unevaluable ignore verdict is identical to "ignored". | `.gitignore` line `[z-a]`; edit `src/auth.py` → path never enqueued, `status` = `is_fresh:true`, `graph_degraded:false`, cold build unaffected so the divergence is invisible. | VERIFIED (ran `ignoreprobe`; read both lines) | Keep the partial matcher + record a diagnostic; fail *open* in the watcher (enqueue on error). |
| K-A2 | **critical** | query + store + serve | `manifest.rs:232`; `code_graph.rs:300-310,567-576`; `db.rs:2627`; `daemon.rs:378-401` | Discovery refusals and `ParseOutcome::Failed` are recorded nowhere durable, so `graph_degraded`, `degraded_reason` and `is_fresh` all report a complete index — and `dead` reports a live symbol dead at confidence 0.9. | `app.py` (1.16 MB, over the ceiling) is `helper`'s only caller → refused → `devmap dead` returns `helper` `{"confidence":0.9,"resolution":"Available","truncated":false,"hidden":0}`; `status` returns `{"degraded_reason":null,"is_fresh":true}`. | VERIFIED (three runnable proofs, incl. the end-to-end wrong answer) | Persist a coverage record per generation; fold into `degraded_reason` + `graph_degraded` + a `parse_failed_files` counter, carrying shown *and* total; `dead` must report `walk_incomplete` when coverage is partial. |
| K-A3 | high | serve | `watcher.rs:296-323, 328` | `EventKind::Other` + `Flag::Rescan` (kernel dropped events) and `notify` `Watch error`s are discarded, so lost watcher coverage is indistinguishable from a quiet tree. | inotify `Q_OVERFLOW` / FSEvents `MUST_SCAN_SUBDIRS` during a `git checkout` → arbitrary edits never indexed, `is_fresh:true` forever. | VERIFIED (read `watcher.rs`; read `notify-6.1.1/src/fsevent.rs:114`, `inotify.rs:208`, `error.rs:34`; `rg rescan` finds nothing in `rust-port/`) | On `need_rescan()`/watch error enqueue the root and set a persisted `watcher_degraded` reason until the next clean sweep. |
| K-A4 | med-high | query + store | `engine.rs:65-99`; `db.rs:2729,2773` | `search` overwrites `budget_take`'s `total` with a second, separately-locked count, so `total`/`hidden`/`truncated` are not computed by the truncation function and can straddle two generations. | Gen shrinks 50→5 mid-query → `{total:50, shown:5, hidden:45, truncated:true}` when all 5 were shown; gen grows 5→50 → `truncated:false` with 20 hits dropped. | VERIFIED (read both statements + the overwrite); INFERRED for the orphan-FTS-row denominator drift | One read transaction / pinned generation for both statements, or return `(rows,total)` from a single store call. |
| K-A5 | med | serve | `daemon.rs:972, 987` (vs 904/912/927/1018) | Binary-change and idle retirement return without `release_ipc_endpoint`, leaving the stale-socket window the helper's own doc says it exists to close. | Idle retirement (the routine exit) → next client's `bind` hits `probe_endpoint_liveness == Some(true)` → `"devmap IPC endpoint is already active"` and the daemon refuses to start. | VERIFIED (exit inventory by `rg`) | Await `release_ipc_endpoint` on both paths; route every `return` through one release helper. |
| K-B1 | med | serve + query | `protocol.rs:47` (`MAX_TOKEN_BUDGET=100_000`), `engine.rs:1914`, `engine.rs:1949-1970` | `MAX_TOKEN_BUDGET` bounds the *response*, not the *work*: `budget_page_size(100_000) = 5_001`, and `hit_from_stored` does one unbounded `fs::read_to_string` per row, with no cancellation check in `search`'s loop. | `{"cmd":"search","query":"a","budget":100000}` → up to 5,001 whole files read and allocated per request; ×64 concurrent connections (`MAX_CONCURRENT_CONNECTIONS`); a `query_timeout` answer does not stop the reads because `search` never consults `Cancel`. | VERIFIED (read `budget_page_size`, `hit_from_stored`, and `search` — no `cancel` reference in `engine.rs:56-99`) | Cap the SQL page independently of the budget; check `Cancel` per row; cap the per-file read. |
| K-B2 | med | serve | `watcher.rs:274` (`std::sync::mpsc::channel()`), `watcher.rs:107-150` | The notify→thread channel is unbounded and `DebounceBuffer.pending` is an uncapped `BTreeSet<String>` held for up to `MAX_DEBOUNCE_HOLD` (10 s). | `git checkout` across a 200k-file tree → both grow without limit for 10 s of churn before any flush; memory is bounded only by how fast the tree changes. | VERIFIED (read the lines) | Bounded channel with a drop-and-mark-rescan policy; cap `pending` and, on overflow, replace it with the root sentinel + a degraded marker. |
| K-B3 | low-med | serve | `daemon.rs:113` | `read_stable_source`'s mtime comparison is `before.modified().ok() == after.modified().ok()`, so a platform/filesystem where `modified()` errors compares `None == None` and the check silently degrades to length-only. | fs without mtime support → a same-length in-place edit passes the "stabilized" check and a torn read is admitted as a clean one. | VERIFIED (read the line); INFERRED that any supported target actually errors | Treat `Err` from `modified()` as "not stable" (or require `Some(_)` on both sides). |
| K-B4 | low | serve | `daemon.rs:597-604` | `claim_of` is a linear `claims.iter().find()` called once per batch entry — O(n²) with `DEFAULT_DRAIN_BATCH_LIMIT = 8192` (~34 M string compares per drain). | Not a correctness defect; a 50k-path branch switch pays it on every batch. | VERIFIED (read the closure and its two call sites) | Build a `BTreeMap<&str, PendingClaim>` once before the loop. |
| K-A6 | med (Class A) | serve + cli | `protocol.rs:326`; `devmap-cli/src/main.rs:1942` | `is_fresh` is `pending_count == 0` and nothing consults `latest_generation`, so a store with **zero generations** reports fresh. The CLI has a "no devmap store at this path" branch (main.rs:1895) for a *missing file* — a present-but-empty store falls through to the normal path. | After the `devmap build` that E-2 crashed (exit 101), the store file exists with no generation: `devmap --json status` → `{"generation_id":null,"node_count":0,"edge_count":0,"is_fresh":true,"degraded_reason":null}`. `devmap_client.py:1055` `is_map_stale() = not is_fresh or pending_count > 0` → **False**, and `codeintel.py:643` renders `state: "fresh"`. Nothing is indexed and the stack says the map is current. | **VERIFIED (ran it; output above)** | `is_fresh` must be `latest_generation.is_some() && pending_count == 0`; a store with no generation needs a `degraded_reason`. |

**Clean in my scope (checked, no defect found)** — so the next agent need not redo these:
`protocol.rs` frame reading (`read_frame` per-read timeout + overall `REQUEST_DEADLINE`, size cap
with `saturating_add`, half-close handling) — the dribbling-peer test at `protocol.rs:930` passes
and pins it; `validate_request` (every command's text/budget/depth/`min_confidence` bound; `Neighbors`
per-element length check; `Clones` kind asked of the parser rather than re-listed; `min_confidence`
`is_finite()` guard); `dispatch_with_timeout` (timeout trips `Cancel`, `PathOutsideRepoRoot`
downcast to `invalid_parameters`); `accept_error_backoff` (grows, caps, pinned by test);
`MAX_CONCURRENT_CONNECTIONS` semaphore backpressure; `lock_ipc_endpoint` (`WouldBlock` vs
`TryLockError::Error` correctly *not* collapsed — this is a model of the Class A rule);
`probe_endpoint_liveness` (`None` treated as active); `UnixIpcServer::drop` unlink ordering;
0700 dir / 0600 socket; `ShutdownSignals` installed before the loop; `vanished_reason`
(returns `None` when unanswerable rather than guessing); `head_differs_from_last_generation`
(fails safe on both read errors); `drain_pending_batch` attempt accounting (charged per failed
path after the loop, acknowledged only after a durable generation);
`collect_pending_path` traversal/escape guards; `is_git_ref_event` narrowness and `.lock` exclusion;
`DebounceBuffer` empty-batch handling (does not reset either clock);
`IgnoreVerdictCache` stamp-based invalidation. Only two `expect()` outside `#[cfg(test)]` in the
whole of `devmap-serve/src` (`protocol.rs:62/68` activity mutex, `daemon.rs:603` claim lookup);
both are locally provable and `panic = "abort"` is set for release.

---

## Candidates examined vs. real — my own scope (`devmap-serve` daemon/watcher/protocol + cross-cutting freshness)

| Category | Candidates examined | Real |
|---|---|---|
| Class A (error→benign value, uncomputed field, cap without flag) | 25 | 5 (K-A1, K-A2, K-A3, K-A4, K-A6) |
| Concurrency / daemon lifecycle | 11 | 2 (K-A5, K-B3) |
| Resource bounds & reachable panics | 14 | 3 (K-B1, K-B2, K-B4) |
| R7 counts | 6 | 1 (K-A4) |
| R4 determinism (serve only) | 5 ordered-collection sites; **zero `HashMap`/`HashSet` exist** in `daemon.rs`, `watcher.rs`, `protocol.rs`, `lib.rs` (verified by `rg`) | 0 |
| **Total** | **61** | **11** |

Denominator detail: the 24 Class A candidates were the 41 `unwrap_or_default()`/`unwrap_or(false)`/
`unwrap_or(0)` sites from a workspace-wide `rg`, narrowed to the 24 that reach a freshness,
coverage or count field; plus every `if let Ok(..)`/`match … Err =>` in `daemon.rs` and `watcher.rs`.
`devmap-extract/src/lib.rs:163` (`matches_ignore(...).unwrap_or(false)`) was examined and is
**not** a defect — `None` there means "no rule matched", and treating that as not-ignored is
fail-open in the safe direction.

---

## Refuted / stale documentation (findings in their own right)

- `devmap-extract/src/lib.rs:152-157` claims `is_gitignored` evaluates rules "the same way
  `WalkBuilder` does for a cold build … or incremental generations admit files the next cold build
  drops." **Refuted by measurement** (K-A1): on a `.gitignore` with one bad glob, `WalkBuilder`
  returns 0 errors and walks everything while `is_gitignored` returns `Err`. The divergence runs
  in the opposite direction to the one the comment guards, and is silent.
- `devmap-query/src/engine.rs:165-167` states `impact`/`deps` "straddled the same way" as
  `neighbors` did before its fix. Still true, and `search` (K-A4) additionally *derives* its
  R7 numbers across the straddle. The comment documents a known gap that has not been closed.
- `devmap-serve/src/daemon.rs:26-35` documents `release_ipc_endpoint` as mandatory on every
  orderly return; two of six returns do not call it (K-A5). Doc and code disagree.

---

## Per-crate audits (delegated, read-only; folded in below)

Paths below are all under
`/Users/bharath/Code/devtools/DevCouncil/.claude/worktrees/dev-map-hardening-mcp-0b79fc/rust-port/crates/`.

### devmap-resolve

Proof harness: a standalone crate at `scratchpad/resolve/` path-depending on
`devmap-resolve`/`devmap-extract` with `default-features = false`, building `Extraction` values by
hand and running 8 probes. Its target dir is in the scratchpad; `rust-port/target` was not written.

| ID | Sev | File:line | Defect | Failure: input → wrong output | Evidence | Fix sketch |
|---|---|---|---|---|---|---|
| R-1 | high (R5) | `devmap-resolve/src/resolver.rs:1620` | `reference_edge` stamps **every** `References` edge `Confidence::DETERMINISTIC`, including the bare-name `UniqueGlobal` rung (1573-1583) — the same evidence the call ladder rates `HIGH`. | `app.py: def use(r: Record)` with no import, `models.py: class Record` → `References … conf=1.0 res=UniqueGlobal`, while the identical evidence as a *call* gets `conf=0.9`. A `min_confidence=1.0` query keeps the fabricated reference and drops the honest call (`db.rs:2969` filters on the rounded f32). `ReferenceKind::Heritage` (1475-1478) is on the same path, so `class B(A)` by name alone is also 1.0. | VERIFIED (ran probe 1) | Give `reference_edge` a confidence derived from the `Resolution` variant: `HIGH` for `UniqueGlobal`, `DETERMINISTIC` only for `SameFile`/`ImportScoped`/`ReceiverType`. |
| R-2 | high (R5) | `devmap-resolve/src/resolver.rs:739-745`, used at `753-758` | The `literal_type` sub-rung resolves a method call whenever the receiver *expression string* equals a type name declaring that method — no import, no binding, no same-file check — at `DETERMINISTIC`, and it sits in **rung 1, ahead of the import rungs 2a/2b**, so a spelling coincidence outranks explicit import evidence. | `u.py: from real import parser; def go(): parser.parse()` plus an unrelated `other.py: class parser: def parse(self)` → `Calls u.py::go -> other.py::parser.parse conf=1.0 res=ReceiverType`. Most likely in Go (lowercase package handle vs. lowercase struct: `client.Do()` with any `type client struct`) and lowercase-class Python. | VERIFIED (ran probe 5) | Move `literal_type` below rungs 2a/2b and require corroboration (type declared in this file, or bound by an import here). |
| R-3 | med (R5) | `devmap-resolve/src/resolver.rs:836-851` (same shape at `1490-1509`) | Rung 2c counts bare-name matches in `file_symbols` with **no kind filter**, so a bare call binds to an *instance method* of a same-file class at `DETERMINISTIC`. | `a.py: class C: def run(self)` + `def invoke(): run()` → `Calls a.py::invoke -> a.py::C.run conf=1.0` for code that raises `NameError` at runtime; `C.run` is thereby given a caller and shielded from the dead-code pass. The existing guard `duplicate_same_file_methods_do_not_become_a_deterministic_bare_call` uses **two** classes so the count is 2 and it abstains — the single-method case is the uncovered gap. | VERIFIED (ran probe 2) | For a bare callee (receiver `None`), restrict the same-file match to symbols whose `parent_symbol` is the file; keep the method match only under `receiver_is_self`. |
| R-4 | med (Class A) | `devmap-resolve/src/resolver.rs:977-1049` | A route whose handler cannot be bound produces **no edge and no record**, so "handler ambiguous / not indexed" is byte-identical to "route has an anonymous handler". | `@app.get('/items')` naming `handler`, with `h1.py` and `h2.py` both defining `handler` → zero `HandlesRoute` edges, zero unresolved records; identical output when the name is absent entirely. `HandlesRoute` is what tells liveness a handler is reached from outside the call graph, so **both handlers are then reported dead** with nothing saying the route was checked and failed. | VERIFIED (ran probes 3a/3b) | Emit an unresolved record for a route whose handler did not bind, carrying the candidate count so ambiguity ≠ absence. |
| R-5 | med (Class A) | `devmap-resolve/src/resolver.rs:964-974`, `1587` | `resolve_name_reference` returning `None` drops the reference silently, while an unresolvable **call** in the same scope is recorded in `unresolved` — violating the rule stated verbatim at `model.rs:216-222` ("R5 forbids silence"). | `def use(r: Missing): alsoMissing()` → one unresolved record for the call, nothing for the type reference. `devmap-cli/src/main.rs:1388-1434` prints `resolution.unresolved` as the R5 completeness ledger, so the ledger **overstates** how much of the reference graph was attributed — a partial denominator presented as a total. | VERIFIED (ran probe 4) | Record unresolved references alongside calls (a `kind` discriminator, or a parallel vec) so the ledger covers both edge families. |
| R-6 | med (Class A) | `devmap-resolve/src/resolver.rs:446-450`, `462-471`, consumed at `245-252` | An import whose specifier is **relative** (`.helpers`, `./util`, `super::x`) but whose target is not indexed lands in `external_imports`, so `classify_unresolved` labels it `External` — the tier documented as "demonstrably comes from outside the corpus" and printed as not-worth-acting-on. | `pkg/app.py: from .helpers import thing; def use(): thing()` with `pkg/helpers.py` present but unindexed (gitignored / over the size cap / generated) → `class=external`. A relative specifier is intra-repository by construction, so an **index gap is laundered into the "expected" bucket**. The missing `Imports` edge is likewise unrecorded (682-707 emits only on success). | VERIFIED (ran probe 7) | Reject `./`, `../`, `.`, `self::`, `super::`, `crate::` prefixes before writing `external_imports`; route them to a distinct `UnindexedLocal` class. |
| R-7 | med (bounds) | `devmap-resolve/src/resolver.rs:889-901`, `910-924` | One ambiguous call site fans out to one edge per candidate with **no cap and no truncation flag**, each edge carrying the full candidate list. | One call to `New()` against 200 same-named declarations → **200 `Calls` edges from one call site**, each with a 200-element `candidates` vector; every one is a persisted `generation_edges` row. `model.rs:107-116` records this measured at 72.6 M pairs / 8.5 GiB of a 10.45 GiB peak — the `Arc` fixed allocations, not the edge count. `STATUS.md:955` still lists collapsing it as open (SC4). | VERIFIED (ran probe 8) | Cap the fan-out (e.g. 16) and carry `candidate_total` + `truncated` on `AmbiguousGlobal` — both numbers, never a capped sample as coverage. |
| R-8 | med (structural) | `devmap-resolve/src/model.rs:98-119`; live site `devmap-query/src/engine.rs:1591-1618` | **`Resolution` is "the only way to make an edge" by convention only.** `ResolvedEdge` has all-public fields, no constructor, derives `Deserialize`; `confidence` and `resolution` are independent and `resolution` is `Option`. `Confidence(pub f32)` accepts any `f32`. Nothing prevents `AmbiguousGlobal` + `DETERMINISTIC`, or `resolution: None` + `1.0`. | Not hypothetical: `stored_edge_to_resolved` is a **production** site constructing `ResolvedEdge { resolution: None, confidence: Confidence(edge.confidence) }` straight from a DB row, unvalidated, no NaN check. Compounding — `ResolvedEdge::resolution` has **zero production readers**: `generation_edges` has no `resolution` column (`db.rs:2118`), so the R5 evidence carrier is discarded at the storage boundary and the bare `f32` is all that survives. That is precisely what makes R-1's overclaim consequential. | VERIFIED (read both sites; workspace-wide grep for readers) | Private fields + `Resolution`-keyed constructors deriving confidence from the variant; a distinct `StoredEdge` type for the read path rather than `ResolvedEdge` with a hole. |
| R-9 | low (Class A) | `devmap-resolve/src/resolver.rs:1141` | `reexport_chains: BTreeMap::new()` is hardcoded empty on every return and **no code path ever writes it** — there is no re-export/alias-chain following at all. | A consumer reading `reexport_chains` gets "no re-export chains in this repo" where the truth is "never computed". `parallel_determinism.rs:147` asserts two empty maps are equal — tautological. LOW only because no production reader exists yet. | VERIFIED (read; grepped for writers) | Delete the field and its tautological assertion, or populate it. Do not ship a computed-looking constant. |
| R-10 | low | `devmap-resolve/src/resolver.rs:321-336` | `index_extractions` documents itself as resetting rebuild state and clears twelve fields — but **not** `go_modules` (set separately at 120-122). | A caller reusing a `Resolver` without re-calling `index_go_modules` resolves Go imports against stale module prefixes and `replace` directives, at `DETERMINISTIC`. Latent: `main.rs:1263-1265` currently calls both on a fresh `Resolver`. | VERIFIED (read) | Clear `go_modules` in the same reset. |
| R-11 | low (R4, latent) | `devmap-resolve/src/resolver.rs:891-894` | `symbol_index` values are `Vec` in **input-slice order**, and that order flows into `AmbiguousGlobal { candidates }`, a field of every emitted edge and a key in the sort comparator (1113-1115) and dedup predicate (1125). | Same 4-file corpus with the slice reversed → candidate lists `[x,y,z]` vs `[z,y,x]` on all three edges (`IDENTICAL = false`). Latent, not live: the edge *list* is stable (the sort covers four string keys first) and the payload is unpersisted with no reader — but the crate's own R4 comment at 1084 says emission "must not depend on input iteration order". | VERIFIED (ran probe 6) | `candidates.sort()` before constructing `AmbiguousGlobal`. |
| R-12 | low (stale doc) | `devmap-resolve/src/resolver.rs:580-584`; `STATUS.md:402` | `resolve_subset`'s `only` parameter has **no production caller** (only `resolve_all(…, None)` at 571; `main.rs:1267` calls `resolve_all`), and `main.rs:1234-1253` explains the narrowing was unsound and was reverted. | `STATUS.md:402` still claims "editing one file resolved **13 edges instead of 172,046**" — refuted by `main.rs:1267`. | VERIFIED (read both) | Retire `only` or restore its caller; correct `STATUS.md:402` either way. |

**devmap-resolve denominators**

| Category | Candidates examined | Real |
|---|---|---|
| R5 confidence honesty | 19 `Resolution` construction sites (every non-test site in resolver.rs: 636, 657, 700, 754, 774, 802, 846, 883, 898, 952, 988, 1001, 1025, 1075, 1440, 1457, 1502, 1521, 1578) + 2 enforcement questions | 4 (R-1, R-2, R-3, R-8) |
| Class A | 14 | 6 (R-4, R-5, R-6, R-9, R-10, R-12) |
| R4 determinism | 10 ordering sites — **zero `HashMap`/`HashSet` in the crate** | 0 live, 1 latent (R-11) |
| Panics + bounds | 17 (10 direct-index, 5 byte-slice, 2 loop) + **0 numeric casts**, **0 recursive functions** | 1 (R-7) |
| **Total** | **62** | **12** |

**devmap-resolve — checked and CLEAN** (do not redo): no float→int cast exists in the crate
(`rg ' as (i64|u32|usize|i32|u64|f64|f32|u8)'` → zero hits in resolver.rs and builtins.rs); the crate
never computes a score, only assigns the five `Confidence` constants, and the known
`(NaN * 1000.0).round() as i64` shape lives in `devmap-extract/src/model.rs:442-447` where it is
**already guarded** (`if !value.is_finite() { return 0 }` + `.clamp(0.0, 1000.0)`). All ten
`[0]`/`.pop().unwrap()` sites (750, 870, 896, 984, 1021, 1243, 1382, 1434, 1597) sit behind a
`len()` or `first()`-is-`Some` check, read individually; all byte-slicing (1292, 1337, 1676) is
guarded by a preceding `starts_with`/`==`/ASCII-dot count so every index is a char boundary; no
`expect`/`panic!`/`unreachable!`/`todo!` outside `#[cfg(test)]`. **No recursion anywhere**, so no
missing depth cap and no missing cycle guard (which is *why* R-9's `reexport_chains` is empty).
`resolve_go_import`'s ambiguity bail (1379) is unreachable rather than a hole — after the
`retain` on `best_len`, two distinct qualifying dirs would have to be the same string (nearly
reported, withdrawn). The builtin tables fail safe: `is_builtin` returns `false` for
`CStyle`/`Scala`/`Lua`/`R`/`Dart`/`Generic` and `host_global_environment` returns `None` for every
non-JsTs family, pushing calls into `Unresolved` (over-report, not exempt), and
`every_builtin_table_is_sorted_and_unique` guards `binary_search`'s silent-liar mode. **G5 is
honoured on the `Calls` path**: the multi-candidate rung emits `SPECULATIVE` + `AmbiguousGlobal`,
never picks a winner, never claims `HIGH` (confirmed by reading and by probes 6/8).
Receiver-key poisoning, Go export visibility, the Python stdlib guard, cross-family isolation,
same-file duplicate abstention and `lookup_in_package`'s two-file abstention all behave as
documented, each with a live regression test. `edges.sort_by` + `dedup_by` is a total order over
all eight fields.

### devmap-store

`cargo check -p devmap-store` passes (exit 0), so the code read is the code that compiles.
All experiments in the scratchpad; nothing in the repo modified.

| ID | Sev | File:line | Defect | Failure: input → wrong output | Evidence | Fix sketch |
|---|---|---|---|---|---|---|
| S-1 | **critical (Class A)** | `devmap-store/src/db.rs:2944-2977` | `callers_of` is the **one** confidence-filtered edge query that does not call `checked_min_confidence` (present at 2999 and 3047), so a NaN threshold makes SQLite's comparison unevaluatable and the caller is told "nothing calls this" by a filter that never ran. | See the promoted write-up above — proven end-to-end against the real binary. rusqlite binds `f32::NAN` as `Value::Real` → `sqlite3_bind_double` → stored as **NULL** → `CAST(ROUND(e.confidence*1000) AS INTEGER) >= CAST(ROUND(?3*1000) AS INTEGER)` is NULL for every row → `Ok(vec![])`. The finiteness check exists **only** in `devmap-serve`'s `validate_request` (protocol.rs:299); the CLI arg is a bare `f32` (main.rs:435 → 1774 → engine.rs:844). No test in any of the three test files touches `callers_of`. | **VERIFIED** (read the missing guard; ran `select ? is null` with a bound NaN → `1`; ran the exact predicate with a NaN parameter → 0 of 2 rows a `0.0` threshold matches; ran the CLI end-to-end) | `let min_confidence = checked_min_confidence(min_confidence)?;` as the first line, matching db.rs:2999. Then validate `is_finite()` + `0.0..=1.0` at the **CLI** boundary too, not only in `validate_request`. |
| S-2 | med (R7) | `devmap-store/src/db.rs:2267-2275`, written at `2282-2302` | `build_history.parse_failed` and `languages_covered` are computed over the `extractions` **input slice**, while `files`/`symbols`/`edges` on the same row are `COUNT(*)` over the whole generation — a partial numerator against a whole-generation denominator, in the one table whose purpose is the trend. | `daemon.rs:701` passes `if full_rebuild { &extractions } else { &fresh }`. A one-line edit in a 1,310-file, 12-language repo with 16 parse failures writes `files: 1310, symbols: ~48000` beside `parse_failed: 0, languages_covered: 1`. `devmap history` shows the repo losing 11 languages and fixing every parse failure on each incremental build, then regaining them on the next cold build. The CLI passes the whole tree (main.rs:1319), so the two writers disagree about what the columns mean. | **VERIFIED** (read db.rs:2267-2275 and daemon.rs:695-703; the only guard, `build_history_separates_confident_ambiguous_and_unmeasured_values`, uses `GenerationWriteOpts::default()` = full rewrite, so the differential case is untested) | Derive both from the generation's own rows inside the tx: `SELECT COUNT(DISTINCT language) FROM generation_files WHERE generation_id = ?1`, plus a `parse_failed` flag column counted the same way. |
| S-3 | med (Class A) | `devmap-store/src/db.rs:204-219`, acted on at `1455-1459` | `classify_pending_entry` treats **every** `symlink_metadata` error as "the file is absent", so a stat that *could not run* deletes a queued path exactly as a stat that ran and found nothing. | Watcher queues a new `src/new.py`; `reconcile_pending_paths` (run by `devmap build`, main.rs:1072, and `devmap repair --pending`, main.rs:2056) stats it and gets ELOOP/EACCES/EIO rather than ENOENT (symlink loop in a parent, a parent that lost `+x`, a stale NFS handle) → not in the latest generation → `Err("no longer exists under the root…")` → the row is **deleted** and reported as garbage; the file is never indexed until an unrelated whole-tree build, and `status` reports fresh. Same shape one level up: `root.canonicalize().ok()` at db.rs:136-138 turns a canonicalize failure into "outside the repository root". | **VERIFIED** (`Err(_) =>` at 204 discards the errno; `Err(reason) => deletes.push(...)` at 1456; both reconcile tests, kernel_defects.rs:364 and 421, exercise only genuine ENOENT) | Match `ErrorKind::NotFound` for the absent branch; any other errno is transient — keep the row so it is retried and eventually quarantined, which is *visible*. |
| S-4 | med | `devmap-store/src/db.rs:3611-3620` | `prune_extraction_cache`'s eviction predicate cannot delete a cache row whose grammar/analyzer identity no longer matches any generation, while it *does* delete the row that matches — so after an extraction-schema bump the table retains only unusable rows and grows monotonically. | File at content hash H, python, cached under `(g1,a1)`. Extractor version bumps; rebuild re-extracts (content unchanged → hash still H) and writes `(H,python,g2,a2)`. Prune: clause 2 deletes `(H,python,g2,a2)` as a generation duplicate; clause 1 spares `(H,python,g1,a1)` because `(H,python)` *is* in `generation_files`. Nothing can serve or evict it. Each bump adds a full copy of every payload — the crate's own docs measure one copy at 198 MiB of a 525 MiB store. Contradicts the method's own doc ("Eviction is by reachability… bounds the cache to the retained working set"). | **VERIFIED** (ran the exact DELETE against a two-row fixture in `sqlite3`: before 2 → after 1, survivor `111\|python\|g1\|a1`) | Add a third disjunct: delete when `(content_hash, language, grammar_version, analyzer_version)` is absent from `generation_files`' non-NULL identity set — evict on full identity, not on `(hash, language)`. |
| S-5 | low (Class A) | `devmap-store/src/db.rs:3673` | `Ok(payload.and_then(\|json\| serde_json::from_str(&json).ok()))` maps an **unreadable stored payload** to a cache *miss*, while every other JSON read in the file (2842, 2883, 2921, 2564) errors loudly and names the path. | Corrupted cache row → silent permanent re-extraction of that file on every build, and the corruption signal is lost. Self-healing (re-extraction overwrites the row), so the cost is slowdown plus a lost signal rather than a wrong answer. | VERIFIED (read) | Propagate the parse error with the identity, as `latest_extractions` does. |
| S-6 | low (Class A) | `devmap-store/src/db.rs:1146-1176` | `lock_writer_at` matches `Err(_busy)` from `File::try_lock`, conflating `TryLockError::WouldBlock` (contention) with `TryLockError::Error(io::Error)` (the check failed) — the same collapse `lock_ipc_endpoint` (protocol.rs:698-717) explicitly refuses to make. | On a filesystem without `flock` (ENOLCK/EOPNOTSUPP), every build polls the full 60 s then fails with "another devmap writer holds … (pid unknown)" — a definite claim about a process that does not exist, from a check that never ran. | VERIFIED (signature + code read) | Match `WouldBlock` for the retry path; propagate `TryLockError::Error` immediately. |
| S-7 | low | `devmap-store/src/db.rs:857-868` | `stored_schema_version` opens its connection directly and never passes through `configure_connection`, making it the one connection in the crate with **no `busy_timeout`**. | A lock conflict fails instantly instead of waiting the 5 s every other connection waits. (Consequence for `devmap status` during a `VACUUM` is INFERRED; the missing pragma is VERIFIED.) | VERIFIED (read) / INFERRED (consequence) | `conn.busy_timeout(Duration::from_secs(5))?` before the `PRAGMA user_version` read. |
| S-8 | low | `devmap-store/src/db.rs:678-791` | `validate_schema`'s `REQUIRED` omits `generation_files.grammar_version`/`analyzer_version` (v8) and `generation_unresolved.classification`/`receiver` (v10/v11), so the gate is narrower than the schema it claims to assert. | A store stamped 12 without those columns opens clean and fails at the first *write* instead of at the gate. Not reachable through the current migration chain. | VERIFIED (read) | Add the four columns to `REQUIRED`, or derive `REQUIRED` from the migration DDL. |
| S-9 | low | `devmap-store/src/db.rs:2387` | `latest_unresolved` binds `params![limit as i64]`: `usize::MAX as i64` is `-1`, and SQLite treats a negative LIMIT as **unbounded**, so a "limit" above `i64::MAX` silently means no limit. | An unbounded result set where the caller asked for a cap. `get_pending_paths_limited` (db.rs:1598) clamps correctly with `limit.min(i64::MAX as usize)`; this one does not. | VERIFIED (read) | Clamp with `limit.min(i64::MAX as usize)` as db.rs:1598 does. |
| S-10 | low | `devmap-store/src/db.rs:126` | `canonical_pending_entry` applies `raw.replace('\\', "/")` unconditionally, so a legal Unix filename containing a backslash is rewritten to a different path. | `a\b.py` → `a/b.py` → fails classification → dropped from the pending queue; the file is never indexed and `status` reports fresh. | VERIFIED (read) | Apply the separator normalisation only on Windows (`#[cfg(windows)]`). |
| S-11 | low | `devmap-store/src/db.rs:2721-2722` vs `2757-2759` | Two readers of the same columns disagree on a corrupt span: `search_symbols` errors (`IntegralValueOutOfRange`) while `all_symbols` silently clamps with `.max(0)` (as does `latest_clone_candidates`, 3188-3189). | The same corrupt row makes `search` fail loudly and `all_symbols` publish a `0..0` span as if it were real. One fails closed, one lies. | VERIFIED (read) | Pick one policy — preferably the loud one — and share a single row-decoder. |

**devmap-store denominators**

| Category | Candidates examined | Real |
|---|---|---|
| Store integrity | 9 areas (10 migration steps, fresh-create completeness, `validate_schema` coverage, 3 MATCH sites, generation atomicity, busy/WAL pragmas per connection, non-atomic file writes, cache eviction reachability, prune atomicity) | 3 (S-4, S-7, S-8) |
| Class A | 21 sites (db.rs 82, 83, 134, 137-138, 658, 667, 1151-1154, 1165-1168, 1314, 1750, 2280, 3056, 3073, 3326, 3378, 3398, 3673, 3745, the 3 confidence surfaces, the stat classifier, the writer lock) | 4 (S-1, S-3, S-5, S-6) |
| R7 | 7 truncating read surfaces + 3 `build_history` metric derivations | 1 (S-2) |
| R4 | ~20 SELECTs feeding returned Vecs + every HashMap/HashSet on the write path | 0 reaching an output artifact today (3 latent) |
| Panics + bounds | 2 non-test unwraps, 9 unbounded result sets, 5 `format!`-built SQL sites, ~15 `as` casts, 19 `prepare_cached` sites | 3 (S-9, S-10, S-11) |
| **Total** | **~90** | **11** |

**devmap-store — checked and CLEAN** (do not redo): **FTS5 escaping is correct at all three MATCH
sites** (db.rs:2674, 2738, 2781) — 17 hostile inputs (`"`, `-`, `*`, `^`, `.`, `(`, `\`, `NEAR`,
`alpha OR beta`, `name:alpha`, `""`, whitespace-only, `alpha"`, `(alpha`, `alpha*`) run through the
exact `query.replace('"',"\"\"")` + `"\"{}\"*"` construction against a real fts5 table: **zero
errors**, hostile input stays data, legitimate queries still match, zero-token phrases return 0 rows
rather than erroring. **Migration atomicity holds**: all 10 steps put DDL and the `user_version`
bump in the same `Immediate` transaction, and `user_version` was *proved* transactional
(`BEGIN IMMEDIATE; CREATE TABLE t; PRAGMA user_version=6; ROLLBACK` → version back to 5, table gone);
the re-read of `user_version` inside the write lock (db.rs:885) handles the create race.
**No half-written generation is observable**: `save_generation_with_metadata` writes the
`generations` row, every child table, `build_history` and the retention delete in one `Immediate`
transaction committed at db.rs:2311 — not in-process (store mutex) and not cross-process (WAL
snapshot isolation). **`busy_timeout` (5 s) and WAL** are set per connection in
`configure_connection`/`enable_wal` for both real openers; `enable_wal` bounds-retries the one
statement SQLite does not route through the busy handler and propagates the final failure;
`checkpoint_wal` sets a bounded 250 ms timeout, restores the previous one, and reports both errors.
**No `SQLITE_BUSY` is anywhere mapped to an empty result.** **No non-atomic file writes**: the store
writes only through SQLite; the sole plain-file write is the best-effort pid inside the already-held
writer lock. **Reachable panics**: the only two non-test `unwrap()`s (db.rs:51-52) take pipes
`Stdio::piped()` guarantees are `Some`; `stdout_reader.join().unwrap_or_default()` (82-83) fails
closed — an empty HEAD is rejected by the 7-64 hex-digit validation at db.rs:92. **No SQL
injection**: all 5 `format!`-built statements use compile-time constants; `callers_of`'s dynamic SQL
is `?` placeholders only. **Prepared-statement cache bounded**: 19 sites, ~11 distinct statements,
under rusqlite's default LRU capacity of 16. **R7-compliant surfaces**:
`search_symbols`/`count_search_symbols` (the store side), `status.quarantined_paths` vs
`quarantined_count`, `latest_clone_candidates`' `unsigned` denominator, `VacuumOutcome.pages_freed`
vs `requested`. **Latent R4 (no consumer depends on the order today)**: `latest_edges_for_test`
(2521, no ORDER BY — both consumers sort or take `.len()`), `list_generation_paths` (3229,
`SELECT DISTINCT` with no ORDER BY — consumers use `contains`/`len`/sum), `clear_pending_superseded`'s
returned `cleared` Vec (1541). One order-dependent value *does* reach output: the "for example {}"
path in the unreplaceable-payload refusal (db.rs:1849-1860) is drawn from an unordered scan, so the
same failure names a different file run to run.

### devmap-query + devmap-analyze

| ID | Sev | File:line | Defect | Failure: input → wrong output | Evidence | Fix sketch |
|---|---|---|---|---|---|---|
| Q-1 | **critical (Class A)** | `devmap-analyze/src/liveness.rs:418-444`, enabled by `devmap-analyze/src/lib.rs:39-42` | A parse-failed file contributes zero call/import edges, and **nothing downgrades findings about the other files those lost edges pointed at** — `AnalysisStatus` is derived only from clustering convergence, so corpus-level extraction loss never reaches any output. | `lib.py` defines `helper()`; `app.py` imports and calls it. Flip `app.py` to `ParseOutcome::Failed`, change nothing else → `dead_symbol_candidates = ["lib.py::helper"]`, `code_graph dead_code = [{"confidence":"extracted","reason":"no inbound call edges and not exported"}]`, `graph_degraded = false`, `analysis_status = "ok"`. `"extracted"` is the **top** confidence tier (`confidence_millis(0.9) >= 900`, code_graph.rs:144). A check that could not run yields a maximum-confidence proposal to delete working code. `analyze_liveness` already applies exactly this reasoning to a parse-failed file's *own* symbols (X6, liveness.rs:294-305: *"Nothing calls it is only evidence when calls were looked for"*) — the cross-file half is missing, unlike the dedicated joins Go interfaces (SC6a) and C headers each got. | **VERIFIED (executed)**. Independently corroborated by proof C above, which reaches the same wrong answer from the *discovery-refusal* direction through the real CLI. | Have `analyze_liveness` return a count of `Failed`/`Fallback` files; `analyze()` folds a non-zero count into `AnalysisStatus::Partial` (which already drives `graph_degraded` and `analysis_status`) and caps non-exempt confidence at the ambiguous tier while it is non-zero. |
| Q-2 | high (Class A) | `devmap-query/src/code_graph.rs:210-252` (`unwired_candidates`) | The filter exempts `TestFile \| Vendored \| GeneratedFile \| ReExportPackage \| Launcher` and entry roots, but **not** parse-failed/fallback files. | In the Q-1 fixture `lib.py` — genuinely imported — is reported unwired, and `liveness_meta.unwired` says `{"shown":2,"total":2,"truncated":false}`: honest counts over a dishonest population. Also reaches `repo_map.json` (manifest.rs:238-243). | VERIFIED | Add `ParseOutcome::Failed{..} \| ParseOutcome::Fallback{..}` to the exclusion and carry the excluded count beside `liveness_meta.unwired`. |
| Q-3 | high (panic) | `devmap-query/src/engine.rs:1883, 1888` | `byte_span_to_line_range` does `source[..start]` / `source[..end]` on a `&str`, which **panics** on a non-char-boundary index; offsets are stored spans while `source` is read fresh from disk at generation time (`code_graph.rs:304 → 342`). | Add one emoji before an indexed symbol's end offset, then run `dev map manifest` → `panicked at engine.rs:1888: end byte index 25 is not a char boundary; it is inside '🦀'`. Release profile is `panic = "abort"`. The identical computation in `devmap_extract::model::Span::line_range` (model.rs:330-342) is **already hardened** via `source.as_bytes()[..offset]`, and `hit_from_stored` (engine.rs:1978) uses that safe one — two copies of one behaviour, one safe. | VERIFIED (reproduced) | `source.as_bytes()[..start]` in both lines, or delete `byte_span_to_line_range` and call `Span::line_range`, the canonical owner. |
| Q-4 | high (Class A) | `devmap-query/src/engine.rs:728-733` (`preview`) | `std::fs::read_to_string(&resolved).ok()` collapses "file does not exist" and "file exists but could not be read" (non-UTF-8, EACCES, EISDIR) into the same `None`; `compared_against` is documented at `model.rs:181-183` as *`nothing` (no such file, so every symbol is an addition)*. | Same edit, same file: readable `mod.py` → `compared_against:"disk"`, `symbols:["beta:Removed"]`. Non-UTF-8 `mod.py` → `compared_against:"nothing"`, `symbols:["mod.py:Added","alpha:Added"]`, `degraded_reason:None`, `delta_available:true`. The genuine removal of `beta` **disappears** and the report returns a clean bill of health. `preview_containment.rs` covers refusal paths but not this one. | VERIFIED (executed both) | Keep the `io::Error`; on `Err` set `delta_available:false` (or at minimum `degraded_reason`) and a third `compared_against` value — never reuse `"nothing"`. |
| Q-5 | med (R7) | `devmap-query/src/manifest.rs:183-189` | `dead_symbol_candidates` truncates at 200 **without ranking**, and strips confidence. | 255 non-exempt findings — 250 at `confidence 0.4 / "only_ambiguous_callers"` first, then 5 at `0.9` → the 200 emitted are **all** the 0.4 ones; `contains any 0.9 finding? false`. Counts are honest (`{"shown":200,"total":255,"truncated":true}`); the cut is not. Root cause: the CLI feeds `store.latest_analysis()` (main.rs:1808 — a JSON blob preserving `analyze()`'s extraction order) rather than `latest_dead_symbols()`, whose SQL already sorts `is_exempt, confidence DESC, …` (db.rs:3116). Consumed by `subsystem_map.dead_symbol_candidates_of`, `wiring.py`, `map_viz.py` and the verify `dead_symbol` gate. | VERIFIED (executed) | Sort by `(confidence DESC, file_path, symbol_name)` before `.take`, and emit `{id, confidence}` objects as `code_graph.json`'s `dead_code` already does. |
| Q-6 | med (R7) | `devmap-query/src/manifest.rs:115, 126` | Two truncating surfaces report no `{shown,total,truncated}` at all. | 30 `*PLAN.md` files → `important_files` shows 15; 40 communities → `subsystems` shows 20; the only keys mentioning either are the lists themselves, and `liveness_meta` covers only `dead_symbol`/`entry_roots`/`unwired`. `generate_manifest:28-36` then pops further to fit a byte budget, also silently, and `consumer_manifest_json:208-230` drops more entries uncounted. `entry_roots` **is** disclosed — so the pattern exists in this exact function and two peers were left out. | VERIFIED (executed) | Add `liveness_meta.subsystems` / `.important_files` `{shown,total,truncated}` computed by the truncation, plus a `subsystems_dropped` counter for the filter. |
| Q-7 | med (Class A) | `devmap-query/src/engine.rs:1782+1793, 1822+1833` | `QueryEngine::impact`/`trace` **discard `walk.stop`**. | Depth-2 walk over a 4-hop chain: the kernel computes `depth_capped = true` with *"the walk did not complete: stopped at depth 2; the result is a lower bound, not the full blast radius"* — and the response returns `truncated:false, total:2, walk_incomplete:None`. This is the literal "depth-capped walk presented as a complete blast radius". `StoreQueryEngine::traverse` does it correctly at engine.rs:446. `QueryEngine` is only *constructed* in tests today but is re-exported as public API at `lib.rs:25`. Same functions also walk the unfiltered edge set and apply `min_confidence` afterwards, listing only the surviving edge with `truncated:false` and no count of what the floor excluded. | VERIFIED (executed) | Set `response.walk_incomplete = walk.stop.reason(max_depth, 5000)` in both; filter edges before the walk as the store engine does. |
| Q-8 | med (Class A) | `devmap-query/src/engine.rs:450-466` | `dead_symbols` carries no completeness denominator: `Response<DeadSymbolReport>` with `resolution: Available`, surfacing neither `AnalysisSummary::status` nor `unresolved_calls`. | `unresolved_calls = 4242` appears nowhere in `repo_map.json`; the only consumers are CLI status (main.rs:1409, 1428). The field's own doc (`devmap-analyze/src/model.rs:28-33`) says it exists precisely so a reader *"can tell 'nothing calls this' apart from 'we could not work out what this calls'"* — and the surface that most needs it does not carry it. | VERIFIED | Include `unresolved_calls` and `AnalysisStatus` in the dead-symbols response. |
| Q-9 | low | `devmap-query/Cargo.toml:18-20` | `cargo check -p devmap-query --no-default-features` **fails** (`devmap-store/src/db.rs:2462: cannot find 'cache' in 'devmap_extract'`), refuting the feature's own doc ("Off, this crate builds without tree-sitter and answers questions about a persisted map"). | The losing branch of the flag is unbuildable configuration. | VERIFIED (ran it) | Fix the feature gating or delete the feature. |
| Q-10 | low | `devmap-query/src/engine.rs:65-95` | The SQL page is cut by `ORDER BY bm25(...) LIMIT` (db.rs:2751) and then re-scored by an exact/prefix/other function, so the response's `score` is not the key the truncation used. | An exact match can fall outside a 101-row bm25 page. Counts stay honest (`total` from `count_search_symbols`), so this is ordering fidelity, not a count lie. See K-A4 for the count half. | INFERRED | Rank in SQL with the same function, or fetch a wider page before re-ranking. |
| Q-11 | low (dead code + bounds) | `devmap-analyze/src/pdg.rs` (whole module) | The entire module is **unwired** (`build_function_pdg`, `FunctionPdgInput`, `PdgStatement` have no reference anywhere in `rust-port/crates` outside pdg.rs and its own tests), and it carries unbounded mutual recursion over attacker-shaped `Deserialize` input (`build_sequence:203`, `validate_statements:485`), two fixpoint `loop`s with no ceiling and no cancellation (369, 402), and `BTreeMap` `Index` panics (374, 385, 406, 426, 440) safe only by construction. | Not reachable today; it is a loaded gun for whoever wires it. | VERIFIED (grepped for callers) | Delete it, or add depth caps + iteration ceilings before it gains a caller. |
| Q-12 | low (perf) | `devmap-query/src/engine.rs:1837-1865` | `traversed_resolution_edges` allocates `format!("{:?}", edge.edge_kind)` plus two `String` clones **per edge in the whole generation** inside the filter closure; `edge_kind_name` (engine.rs:1240) exists for exactly this and is used only by `shortest_path`. Keying on `(source,target,kind)` also emits untraversed edges sharing that triple. | Allocation churn proportional to generation size on every call. | VERIFIED | Use `edge_kind_name`; key on edge identity. |

**devmap-query + devmap-analyze denominators**

| Category | Candidates examined | Real |
|---|---|---|
| Class A | 28 | 6 (Q-1, Q-2, Q-4, Q-5 partial, Q-7, Q-8) |
| R7 (counts + ranking) | 26 truncating surfaces | 3 real + 1 low (Q-5, Q-6 ×2, Q-10) |
| R4 determinism | 25 `HashMap`/`HashSet` lines / 9 distinct iteration sites | 0 real, 2 theoretical notes |
| Panics + resource bounds | 97 `.unwrap()` lines (1 non-test), 25 `as` casts, 7 recursion/fixpoint sites, all slicing sites | 1 real panic (Q-3) + 2 bound notes (Q-11, Q-12) |
| Regex / query injection | 4 sinks (escape.rs, query_match.rs, FTS `MATCH`, glob) | **0** |
| **Total** | **~180** | **12** |

**Determinism notes (no defect claimed):** `semantic.rs:151, 178-181` sum `f32` over `HashMap`
iteration order, so absolute scores can differ in the last bits between processes — within one call
the order is fixed for every document, so ties are preserved. `clones.rs:187/198` iterate `HashMap`s
but sort on `(min_nodes desc, members.len() desc, signature asc)` afterwards — total unless an
`exact` and a `structural` signature collide on `u64`.

**devmap-query + devmap-analyze — checked and CLEAN** (do not redo): **`escape.rs`** —
`html_escape` and `json_script_escape` correct; the latter neutralises `</script` via `<` and
round-trips through `JSON.parse`. **No regex anywhere in either crate.** **`query_match.rs`** — user
query text never becomes a regex or a glob; matching is exact/segment-boundary with per-branch
mutation-killing tests. **`traversal.rs`** — every one of the five decline paths sets a
`TraversalStop` field (`starts_dropped`, `depth_capped`, `node_capped`, `edges_unrecorded`), the
"leaf at the cap is not a decline" distinction is right, ranking precedes the edge budget, and the
reason reaches `Response::walk_incomplete` on the store engine (pinned by
`neighbors_adversarial.rs::the_default_depth_marks_its_walk_incomplete_rather_than_truncated`).
**`shortest_path` (engine.rs:1326-1465)** — `PathSearch::NoPath` vs
`Exhausted{depth_capped,node_capped,visited}` is a genuine three-way distinction that
`trace_between` renders (328-352); `max_depth == 0 \|\| max_nodes == 0` handled up front.
**`clustering.rs`** — `louvain` returns a real `converged` flag reachable from tests via
parameterised ceilings; all-`BTreeMap`/`BTreeSet`; cohesion guards `denominator > 0.0` (also
excluding NaN) then clamps; the `unwrap_or(0.0)` at 208 is the correct "no incident weight".
**Float→int casts** — `confidence_millis` (devmap-extract/model.rs:181-186) rejects non-finite and
clamps to `0..=1000`; **the known `(NaN * 1000.0).round() as i64` shape is gone**. `snapshots.rs:46-47`,
`engine.rs:1926-1928, 2018-2020, 2030` all use `try_from`/`saturating_*`.
**`cap_source_span` (2017-2034)** walks back to a char boundary and reports
`source_span_omitted_bytes`. **`contained_repo_path` (1516-1573)** — `..` refused outright,
absolute paths must be under the root, canonical form re-checked when the path exists, and
`canonicalize` failure is a refusal not a pass; covered by `preview_containment.rs`.
**`is_foreign_code_graph`/`is_foreign_repo_map`** return `true` (refuse) on an unparseable artifact —
fail-closed. **`Workspace::load`** — malformed registry is an error, not an empty workspace;
future version refused; `update` holds an `flock` across read-modify-write.
**`write_atomic` (artifacts.rs:22-66)** — per-writer unique temp name, `sync_all` before rename,
temp cleaned on every non-`Ok(true)` path. **R7-compliant surfaces** (true pre-truncation total,
ranked before cutting): `build_dependents` (manifest.rs:424-451), `clones.rs:273-274`
(`members_omitted`), `snapshots.rs`, `workspace_search` (engine.rs:901-995), `atomic_budget_take`,
and `StoreQueryEngine::{search_semantic, dependencies, clones, trace_between}`.
**`code_graph.rs` provenance** — `duplicate_node_ids_dropped`, `edge_endpoints_without_node`,
`files_without_readable_source`, `regex_fallback_files`, `dead_code_without_node`, and the
*conditional* `unavailable.{indexed_hash,content_fingerprint}` markers (513-531) are all computed,
not asserted; the `unwrap_or_default()` at 548/553 is correctly paired with them.
**Non-test panics** — zero `.unwrap()`/`.expect()`/`panic!` outside `#[cfg(test)]` in either crate;
the single `unreachable!` (engine.rs:195) is provably unreachable.

---

### devmap-extract

Reproductions in `scratchpad/extract/probe2` (a scratch crate depending on `devmap-extract` by path,
with its own `CARGO_TARGET_DIR`). I re-verified findings E-1 and E-2 myself against the **production
`devmap build` binary** — see the promoted write-ups above.

| ID | Sev | File:line | Defect | Failure: input → wrong output | Evidence | Fix sketch |
|---|---|---|---|---|---|---|
| E-1 | **critical (panic)** | `devmap-extract/src/treesitter.rs:4314` (`rust_type_name`), `:4437` (`go_type_name`) | Both recurse once per `reference_type`/`pointer_type`/`generic_type` wrapper with **no depth parameter**, unlike the four sibling type walkers beside them (`go_type_qualifier:4391`, `rust_type_qualifier:4420`, `go_composite_literal_type:757`, `split_call_target_inner:4015`), each of which caps at 16. | See the promoted write-up: `devmap build` on a 10 KB Go file → `fatal runtime error: stack overflow, aborting`, **exit 134**. Thresholds measured: ~10,000 wrappers on a 2 MiB rayon worker stack, ~60,000 on the 8 MiB main thread — far under `MAX_SOURCE_BYTES` (1 MiB). Controls at n=20,000 that do **not** abort: deep parens in Rust/Python/JS, deep Rust reference *expressions*, deep C pointer declarators. `tests/stress_hardening.rs:18` tests this exact shape but only to depth 200 (fits the stack) and asserts on the *qualifier* pair, which is bounded, not the *name* pair. | **VERIFIED (I reproduced it through `devmap build`)** | Give both the `depth: usize` parameter and `if depth > 16 { return None; }` guard their siblings carry; raise the stress test's depth above the stack limit (e.g. 50,000). |
| E-2 | **critical (panic)** | `devmap-extract/src/treesitter.rs:2370` | `let inner = &text[idx1 + 1..idx2];` where `idx1 = text.find('{')` and `idx2 = text.find('}')` are taken **independently** — nothing establishes `idx1 < idx2`. | See the promoted write-up: `devmap build` on `export const isClose = (c) => c === '}' \|\| c === '{';` → `panicked at treesitter.rs:2370:42: byte range starts at 51 but ends at 37`, **exit 101**. Also verified panicking: `export const MAP = ['}', '{'];`, `import x from "}{";`, and `export const RE = /\}\{/;` (`.ts`). Reached from the `"javascript" \| "typescript" \| "tsx"` arm of `extract_node`; under `extract_all` it unwinds out of a rayon `par_iter` and kills the build. `tests/adversarial_corpus.rs` never combines a valid export statement with a `}` preceding a `{`. | **VERIFIED (I reproduced it through `devmap build`)** | `let Some(idx2) = text[idx1+1..].find('}').map(\|o\| idx1+1+o) else { … }` — search for the closer only after the opener. |
| E-3 | high (Class A / R7) | `devmap-extract/src/notebook.rs:190-195` (and `:309-315`) | Notebook cell-cap truncation is recorded **only in `diagnostics`**, which `for_durable_store` erases; the stored outcome reads `Clean`. | A 6,000-cell Python notebook yields `symbols=5001, parse_outcome=Clean, diagnostics=["notebook has 6000 cells; only the first 5000 were read"]`. After `for_durable_store()` — the payload written to `generation_files.extraction_json` and the extract cache — `diagnostics=[]`, outcome still `Clean`. `fn_5999` is absent and nothing records that 1,000 cells were never read. `cache_admits(Clean)` is `true`, so it is admitted **under a real content hash**: a capped sample presented as complete coverage, permanently. This is exactly what `EXTRACTION_SCHEMA_VERSION` v29 (`cache.rs:150-156`) fixed for the *fallback* path by moving the count into the `ParseOutcome::Fallback` reason — the case was fixed, the class was not. | VERIFIED (executed) | Carry the cell cap and unlocatable count in the `ParseOutcome` (a `Partial`/`Fallback` reason naming `{shown, total, dropped}`), as `unavailable_extraction` at `treesitter.rs:1112-1126` now does. |
| E-4 | high (Class A / R7) | `devmap-extract/src/fallback.rs:216` | `if trimmed_len == 0 \|\| trimmed_len > MAX_LINE_BYTES \|\| is_comment(line) { continue; }` — `FallbackScan::truncated` counts only symbol-cap overflow (239-241), so a line skipped **for length** increments nothing and the reason string reports a complete set. | A `.proto` with `message Short {}`, a 3,000-byte `message LongXXX… {}` line, and `message Tail {}` → `names = ["p.proto","Short","Tail"]` with `ParseOutcome::Fallback { reason: "…2 declaration(s) recovered by pattern" }` and empty diagnostics. A caller reads "2 of 2" where the file declares 3. Generated `.proto`/`.ps1` files routinely carry long lines. | VERIFIED (executed) | Add `skipped_long_lines: usize` to `FallbackScan`, increment at 216, fold into the `Fallback` reason alongside `truncated`. |
| E-5 | high (Class A) | `devmap-extract/src/treesitter.rs:3196` | `source.get(start..end).unwrap_or_default()` — `end` is `min(declarator_start, start+256)`; when that offset is not a char boundary `.get()` yields `None`, the whole head becomes `""`, and `head_has_word`/`head.contains(…)` answer `false` for **everything**. | `__global__ /* <200 em-dashes> */ void kern(int* p) {}` in a `.cu` file → `wiring = []`; the identical file with 400 ASCII padding chars → `wiring = [RuntimeEntryPoint "CUDA kernel launched by name from host code"]`. The kernel loses its entry-point exemption and becomes a dead-code candidate. Likewise `__attribute__((visibility("default"))) /* <200 em-dashes> */ int f(void)` → `is_exported = false`; short head → `true`. A genuinely exported C symbol is reported private. | VERIFIED (executed both) | Walk `end` down to the nearest char boundary before slicing, as `clamp_receiver` (`langcalls/scope.rs:102`) already does. |
| E-6 | med (Class A / R6, latent) | `devmap-extract/src/treesitter.rs:285`, reaching `:468` | `ParseAttempt::NoTree` falls through to `unavailable_extraction` rather than `refused_extraction` — the variant that exists specifically to name "the parser returned no tree for a reason it did not name" is the one not routed to the refusal path. `Budget` and `GrammarLoadFailed` are routed correctly. | The doc on `refused_extraction` (963-969) names this exact hazard: a tree-sitter ABI break would "downgrade every file of a language to regex fallback, be cache-admitted, exempt them all from dead-code analysis, and leave the build green." If any linked grammar returns `None` for a non-budget reason, every file of that language is emitted as `RegexFallback` with the **false** reason `"no linked tree-sitter grammar for rust; N declaration(s) recovered by pattern"`, and `cache_admits(Fallback)` is `true` so the poisoned rows persist under real content hashes. | code path **VERIFIED** by reading; **reachability UNVERIFIED** (could not construct a `NoTree` without editing the crate) | `ParseAttempt::NoTree => return refused_extraction(path, lang, source, format!("grammar {grammar} returned no tree for {lang} and named no reason"))`. |
| E-7 | low | `devmap-extract/src/notebook.rs:268` | `.find(\|cell\| cell.code.contains(declaration))` — when `declaration` is `""`, `contains("")` is unconditionally `true`, so the symbol takes **cell 0's** span and `unlocatable` is not incremented, contradicting the module doc ("dropped and counted — never emitted with a guessed span"). The parallel call path at `:359` *does* guard with `!line.is_empty()`. | A guessed span presented as a located one. | asymmetry **VERIFIED** by reading; reachability **INFERRED** (needs a zero-width or whitespace-leading span) | `.find(\|cell\| !declaration.is_empty() && cell.code.contains(declaration))`, matching 359. |
| E-8 | low | `devmap-extract/src/lib.rs:426` | `if let Ok(pruned) = pruned.lock()` — a poisoned mutex silently drops every pruned `CACHEDIR.TAG` subtree from `DiscoveryReport::skipped_paths`, so the report reads as if nothing was skipped. | Low: the `filter_entry` closure has no panicking operation today. | VERIFIED (read) | `pruned.lock().unwrap_or_else(\|e\| e.into_inner())`. |

**devmap-extract denominators**

| Category | Candidates examined | Real |
|---|---|---|
| Class A | 28 | 5 (E-3, E-4, E-5, E-6, E-7) |
| R6 (failure ≠ emptiness; cache admission) | 10 | 1 (E-6) |
| R7 (truncating surfaces) | 10 | 3 (E-3, E-4, and E-5 counted under Class A) |
| R4 determinism | 10 | **0** |
| Panics + resource bounds | ~35 | 2 (E-1, E-2) — both process-aborting |
| **Total** | **~93** | **8** |

**devmap-extract — checked and CLEAN** (do not redo): **R4 is genuinely clean** — only 4
hash-container sites exist in the whole crate (`treesitter.rs:4931` `SCOPE_LOCALS`, `:5036`
`collect_non_symbol_locals` — consumed only by `.contains()` at 5091 and `entry.extend(...)` into a
`BTreeSet` at 5177; `fallback.rs:209 seen`, insert-only; `lib.rs:100 verdict`, get/insert only).
`exports`/`references` explicitly sorted (403, 410); `go_interface_methods`/`go_method_params`
sorted (1689, 1696); `scope_locals` is `BTreeMap`-derived; `symbols`/`imports`/`calls`/`wiring` come
from a deterministic pre-order worklist. **Empirically verified**: extracting `fallback.rs` twice on
one thread and once on another produced byte-identical serialized `Extraction`s.
**Panics**: all 37 `.unwrap()` calls are inside `#[cfg(test)]` (checked against each file's
`cfg(test)` line); `treesitter.rs`, `notebook.rs`, `wiring.rs`, `clonesig.rs`, `languages.rs` contain
zero non-test ones. The only non-test `.expect()` is `fallback.rs:85` on a compile-time-constant
regex. Every other direct slice site is boundary-safe: `treesitter.rs:845, 860, 2127, 2351` (indices
from `find`/`match_indices`/ASCII prefix lengths), `fallback.rs:219`, `langcalls/scope.rs:105`
(explicit `is_char_boundary` loop), `gomod.rs:71`; `get_node_text` (5403) and `preceding_token` (674)
fail closed. **Recursion is otherwise bounded**: `split_call_target_inner`,
`go_composite_literal_type`, `go_type_qualifier`, `rust_type_qualifier` cap at 16;
`c_declarator_name_node` and `split_qualified_identifier` use `for _ in 0..16`; macro probing caps at
`MAX_MACRO_DEPTH = 4`; `walk_tree`, `parse_outcome_of`, `collect_non_symbol_locals`,
`collect_scope_locals`, `collect_parameter_names`, `go_method_sets`, `signature_of`,
`declaration_hash_of` all use explicit worklists. **Resource bounds**: `MAX_SOURCE_BYTES = 1 MiB`
enforced and reported as `DiscoverySkipReason::Oversized`; NUL-byte refusal works (`a\0b\0c\n` →
`Failed` with the offset); COBOL/unsafe grammars refused before parsing; the parse budget (5 s) and
walk deadline both return `refused_extraction` rather than a truncated tree (nested-JS at
n=500…8000 all returned `Failed` in ~5.2-5.4 s); `ignore::WalkBuilder` never sets `follow_links`, so
symlink loops are not followed; empty, 1-byte, 200,000-token minified single-line JS,
20,000-deep Python parens, malformed-JSON notebook and kernel-less notebook all returned honest
results. **Casts**: every non-test `as` checked — `confidence_millis` (model.rs:183) guards
`!is_finite()` and clamps; `Span::line_range` (model.rs:339) clamps and `min(u32::MAX)`; `clonesig`
uses `saturating_add`. **Cache admission (R6 boundary) holds**: `cache_admits` (cache.rs:424) rejects
`Failed`, and both workspace insert sites check it (`devmap-store/src/extract_cache.rs:60`,
`devmap-store/src/db.rs:3682`); `grammar_version_for` returns a real per-language identity for all
32 linked grammars. **Pre-flagged candidates that are NOT defects**: `treesitter.rs:251`
(`contains(&0)` already proved the offset exists), `:1661`/`:1677` (a Go method with no `parameters`
field genuinely has 0), `:2347` (empty `mod_spec` filtered by `!mod_spec.is_empty()`), `:3037`
(HCL block type — `hcl_block_address` already required child 0 to be an identifier), `:4054`/`:4058`
(empty callee names filtered at 3920), `:4142`, `:4430` (dead default — `split` on a non-empty `str`
always yields `Some`), `frameworks.rs:150`/`:189` (`""` is a distinguishable sentinel; no symbol is
named `""`), `notebook.rs:356` (guarded by `!line.is_empty()`), `gomod.rs:25` (`""` = collect root,
documented), and **`lib.rs:163`** — `matches_ignore` returns `Option<bool>` where `None` means "no
rule expressed an opinion", so `unwrap_or(false)` is correct semantics. *(Note: genuine `.gitignore`
parse errors do propagate as `Err` — which is exactly what makes K-A1 above a defect at the
**watcher's** catch site, not here.)*
**Hypothesis tested and refuted**: that the post-walk passes (`collect_scope_locals`,
`stamp_signatures`, `go_method_sets`, `extract_framework_routes`) escape the walk deadline and blow
up superlinearly — measured on nested-JS at n=500…8000, the walk hits the 5 s budget first and
returns `Failed` every time, while a flat 262 KB file completes in 147 ms. The deadline gap is real
in code but no exploiting input was constructed.
