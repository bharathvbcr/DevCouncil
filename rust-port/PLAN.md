# dev map → Rust: Clean-Room Rewrite Plan

**Status:** active scaffold. Workspace crates exist under `rust-port/crates/`; Python
`indexing/` + `codeintel/` remain the live system until Phase 6 cutover. See
[PHASE1_CONTRACT.md](PHASE1_CONTRACT.md) for token budgets and §4.5 scoping decisions.
**Audit basis:** DevCouncil @ `98f23e3`, 769 source files, macOS arm64, CPython 3.12.13, 2026-08-11.
**Rendered version:** [PLAN.html](PLAN.html) (same content, styled).

A standalone Rust implementation of the code-intelligence subsystem — new binary, new store,
new agent-facing contract. Not a PyO3 extension and not a function-by-function port. The 32
audit findings and 7 reference-derived enhancements below are the **acceptance
specification**: the new system is designed so the defects cannot occur, and each item becomes
a test rather than a patch.

§4 is the counterweight — an inventory of behaviour that already works and must survive. A
clean-room rewrite loses undocumented capability silently, and that inventory is what makes
the loss detectable.

---

## 1. Scope being replaced

| Component | LOC | Role |
|---|---:|---|
| `src/devcouncil/indexing/` | 17,929 | Repo map, graph build, resolution, liveness, viz |
| `src/devcouncil/codeintel/` | 8,405 | SQLite store, generations, incremental sync, languages |
| `src/devcouncil/cli/commands/map.py` | 503 | CLI entry point |
| **Production total** | **26,837** | 29% of the 93,676-LOC `src` tree |
| Associated tests | 16,534 | Behaviour that must survive |

**37 files outside those directories import them** — 11 CLI commands, 5 MCP handlers,
11 verification checks, plus `execution/policy_engine.py`, `knowledge/wiki.py`, and the
OKF bundle writer. Each is a contract the rewrite must honour.

## 2. Measurements

All figures measured, not estimated.

### Cold build — 769 files, 15.4s

| Phase | Cumulative | Share | Nature |
|---|---:|---:|---|
| `enrich_semantic_edges` | 5.81s | 38% | Python resolution |
| `resolve_calls` | 4.07s | 26% | Python symbol resolution |
| `_build_liveness_shards` | 1.88s | 12% | Python reachability |
| `extract_all` (parsing) | 0.66s | 4% | stdlib `ast` + tree-sitter |
| `sqlite3.execute` | 0.42s | 3% | Native SQLite |

Already-native components are ~7% of the build; tree-sitter is not in the top 22 hot
functions. Accidental overhead in the Python tier: **1,085,746** `Path` constructions
(1,412/file), **5,827,369** `re.finditer` calls, **176,348** route-matcher and **176,298**
DI-matcher invocations (~229 redundant sweeps per file each), **247,115** uncached
`_lang_family` calls.

### Benchmark harness

| Metric | fast · 256 | heavy · 10,000 | Current ratchet |
|---|---:|---:|---:|
| Cold build | 1.22s | 59.9s | 300s |
| Single-file resync | 0.18s | **7.55s** | 60s |
| Peak RSS | 103 MB | 714 MB | 2 GB |
| Query p95 — search (FTS) | 0.57 ms | 0.78 ms | 50 ms |
| Query p95 — explore | 36.8 ms | **754 ms** | 5,000 ms |
| Query p95 — dead | 44.2 ms | **751 ms** | 5,000 ms |

Ratchets sit far above actuals and cannot catch regression.

### Storage — live index, 769 source files

| Object | Size | Share | Rows |
|---|---:|---:|---:|
| `extraction_cache` (~18 KB/row, 64 MB cap) | 56.2 MB | 34.5% | 2,972 |
| `generation_edges` + 4 indexes (~490 B/row) | 52.7 MB | 32.3% | 107,136 |
| `edge_payloads` + autoindex | 21.8 MB | 13.4% | 53,568 |
| `generation_nodes` + 3 indexes | 10.8 MB | 6.6% | 23,332 |
| `nodes_fts` (all shadow tables) | 6.7 MB | 4.1% | 23,332 |
| Freelist (never reclaimed) | 42.2 MB | 20% | 10,312 pg |
| **File on disk** | **208 MB** | | 270 KB per source file |

### Token cost of agent-facing surfaces

Byte counts at 4 chars/token — an estimate, but the magnitudes are not in question.

| Artifact | Size | Tokens |
|---|---:|---:|
| `.devcouncil/repo_map.json` | 514,625 B | **~128,656** |
| ├─ `dependents` (406 entries) | 255,376 B | ~63,844 (50%) |
| ├─ `files` (853 entries) | 142,049 B | ~35,512 (28%) |
| ├─ `subsystems` (24 entries) | 26,480 B | ~6,620 |
| └─ everything else | ~6,200 B | ~1,550 |
| `.devcouncil/graph/code_graph.json` | 17,454,216 B | **~4,363,554** |

CLAUDE.md step 1 directs every agent to open `repo_map.json`. That single instruction is the
largest token expenditure in the system, and **78% of it is two lookup tables** shipped whole
to answer point queries.

---

## 3. Findings as specification

32 findings plus 7 reference-derived enhancements — 39 items. Each is a property the rewrite must hold,
with the acceptance test that proves it.

### Correctness and concurrency

| ID | Property | Source | Acceptance |
|---|---|---|---|
| B1 | Queued work survives process death | `sync/coordinator.py:270` | kill −9 mid-build, work replays |
| B2 | Back-pressure, never silent discard, on lease contention | `build_control.py:447` | two writers, zero lost edits |
| B3 | Write cost ∝ change size, not repo size | heavy `write_stats` | 1-file edit writes <100 rows at 10k |
| B4 | Readers never blocked by a writer | `sync/lease.py` | query p95 unchanged during build |
| B5 | Staleness self-heals without an unrelated trigger | `service.py:151` | commit-then-idle leaves index fresh |
| N1 | Queries never deserialise the whole graph | `query/engine.py:16` | explore p95 <50 ms at 10k |
| N2 | Deletions and renames explicit, not implied | `incremental.py` — absent | deleted file absent next build |
| N4 | A check that could not run never reports as one that passed | `graph/intel.py:118` | timeout surfaces `status` |

### Storage

| ID | Property | Source | Acceptance |
|---|---|---|---|
| B6 | Generation prune is a bounded delete, not a full reindex | `sqlite.py:1326` | prune cost ⊥ live row count |
| B7 | Cache admission is O(1), not a table scan | `sqlite.py:1673` | cold-build insert cost linear |
| B8 | Eviction follows liveness, not age | `sqlite.py:1677` | live files never evicted first |
| B9 | No redundant column in any primary key | `sqlite.py:503` | schema review |
| B10 | Paths interned; no repeated text in hot tables | `sqlite.py:503` | index <60 MB for this repo |
| B11 | Free space reclaimed on a schedule | `sqlite.py:1165` | freelist <5% after 1,000 builds |
| D2 | One schema, no rollback-compat shadow tables | `sqlite.py:643` | schema review |

### Compute and analysis quality

| ID | Property | Source | Acceptance |
|---|---|---|---|
| B12 | Extraction saturates available cores | `build.py:162` | near-linear core scaling |
| B13 | No repeated path parsing in hot loops | 1.09M `Path` calls | profile shows no string-path work |
| B14 | One tree pass per file, not many regex sweeps | `routes.py:77`, `di.py:37` | profile shows one traversal |
| B15 | Language resolved once per file, not per edge | `resolve.py:81` | profile review |
| N5 | Leiden clustering — connected communities guaranteed | `intel.py:114` | no disconnected community |
| N6 | Receiver types tracked from constructors, not guessed by name | `resolve.py:764` | resolution-precision ratchet |
| N7 | Retrieval fuses lexical and semantic ranking | `sqlite.py:1464` | RRF over BM25 + vectors |
| N9 | Pools sized from real cores and RAM, no fixed ceiling | `workers.py:221` | 12-core host uses 12 cores |

### Interface and artifacts

| ID | Property | Source | Acceptance |
|---|---|---|---|
| T1 | No agent-facing artifact exceeds its token budget | 514 KB repo map | manifest ≤2k tokens |
| T2 | Machine interchange lives outside the context directory | 17.4 MB export | opt-in, not default |
| T3 | Every response token-budgeted, reports truncation | `handlers/map.py:235` | all carry `{shown,total}` |
| T4 | Budget discipline generalised from `prompt_builder` | `prompt_builder.py:281` | applied to all surfaces |
| N3 | No dual-write of a second full serialisation | `build.py:650` | one canonical store |
| N8 | Ingestion owned by a long-lived watcher | `graph_cmd.py:425` | freshness holds, hooks off |
| N10 | Query surface honest about its capability | `cypher.py:1-20` | no regex posing as a query language |
| D1 | One extractor per language, tree-sitter throughout | `cache.py:308` | no stdlib-`ast` path |
| D3 | Lock status reports the true holder | `lease.py:17` | status never names a dead pid |

### Reference-derived enhancements

Adopted from GitNexus and CodeGraph. These are additions rather than defects — nothing in the
current system is wrong, but each is a capability it lacks.

| ID | Enhancement | Source | Acceptance |
|---|---|---|---|
| R1 | One primary query tool; the rest unlisted by default | CodeGraph | ≤3 listed tools replace today's 18 |
| R2 | Results carry verbatim source, grouped by file | CodeGraph | symbol query needs no follow-up read |
| R3 | Embedding index bounded by node count | GitNexus | peak RSS <1 GB at 10k files |
| R4 | WAL auto-checkpoint tuned for build churn | GitNexus | freelist <5% without manual vacuum |
| R5 | Call paths include dynamic-dispatch hops | CodeGraph | interface-dispatch path resolves in corpus |
| R6 | Communities carry cohesion scores | GitNexus | every community reports cohesion |
| R7 | Targeted repair without a full rebuild | GitNexus | `devmap repair --fts` rebuilds index only |

**Declined:** multi-repo global registry and connection pooling (GitNexus). DevCouncil is
single-repo by design; adopting these changes the product, not the implementation.

**Already present, not adopted from elsewhere** — see §4. tsconfig path aliases, re-export
chains and a portable fallback extractor exist in DevCouncil today, in some cases more
thoroughly than in either reference.

### Notes on two findings that are not bugs

- **D2** — the six 0-row legacy tables are deliberate: `sqlite.py:643` keeps them "present for
  rollback-compatible schema inspection" after the v1→v2 migration. The rewrite implements v2
  semantics only; this is a decision, not a defect.
- **D3** — `writer.lock` naming a dead pid is cosmetic. `flock` is the authority and the OS
  releases it on process death.

---

## 4. Capabilities to preserve

A clean-room rewrite loses undocumented behaviour silently. Golden-corpus parity only catches
a regression if the corpus exercises the capability — so each row below carries the corpus
requirement that makes it detectable. **This inventory is incomplete**: it was compiled from
the 49% of the subsystem the audit opened plus targeted probes into `repo_mapper.py` and
`wiring.py`. Completing it is a Phase 1 deliverable.

### 4.1 Import resolution, per language

| Capability | Source | Corpus must contain |
|---|---|---|
| Python module-suffix index + `__init__.py` ancestry | `repo_mapper.py:1070,1114` | nested packages, namespace pkgs |
| JS/TS relative resolution + extension/index probing | `repo_mapper.py:1310,1332` | `./x`, `../y`, dir-index imports |
| tsconfig/jsconfig path aliases **across `extends` + project references** | `repo_mapper.py:1340-1344` | multi-level `extends`, project refs |
| Monorepo package.json workspace → source dir mapping | `repo_mapper.py:1551` | workspace with 2+ packages |
| JS re-export chain following | `repo_mapper.py:1636`, `resolve.py:236` | `export { X } from './y'` chains |
| Named-binding imports (`import { X as Y }`) | `resolve.py:583` | aliased named imports |
| Go module prefix from `go.mod` | `repo_mapper.py:1726` | go.mod with non-trivial module path |
| Rust crate root + `mod.rs`/`name.rs` probing | `repo_mapper.py:1802,1814` | nested modules both spellings |

### 4.2 Liveness exemptions — the false-positive guard

`wiring.py` is the layer that stops dead-code detection reporting entry points as dead. Losing
any row here does not break a build; it makes `dev map dead` untrustworthy, which is worse.

| Capability | Source | Corpus must contain |
|---|---|---|
| `pyproject.toml` script targets | `wiring.py:829` | console_scripts entry point |
| `package.json` entry + script targets | `wiring.py:917,424` | bin/main/scripts entries |
| `Cargo.toml` binary targets | `wiring.py:796` | `[[bin]]` target |
| Launcher files + module-form reference keys | `wiring.py:376,387` | a launcher referencing a module |
| Re-export-only `__init__.py` recognition | `wiring.py:277` | package re-exporting submodules |
| Python `__all__` and re-export names | `wiring.py:451,470` | `__all__` with indirect exports |
| Generated-file detection (path + header sniff) | `wiring.py:253,258` | a file with a codegen header |
| Vendored path detection | `wiring.py:181` | a vendored third-party dir |
| Test path detection | `wiring.py:113` | tests in a non-standard location |
| Decorator-based wiring (`is_wiring_decorated`) | `wiring.py:706` | registration decorators |
| Structural convention exemptions | `wiring.py:728` | conventionally-wired file |

### 4.3 Framework recognition

| Capability | Source | Corpus must contain |
|---|---|---|
| Route extraction: django, rails, spring, aspnet, laravel, nest, axum, play, drupal | `frameworks/routes.py` | one app per framework family |
| DI/provider patterns | `frameworks/di.py` | provider registration |
| Event handler patterns | `frameworks/events.py` | event subscriber |

### 4.4 Subsystem inference and artifacts

| Capability | Source | Corpus must contain |
|---|---|---|
| Source-root detection | `repo_mapper.py:1037` | `src/`-style and flat layouts |
| Area/role classification, role_files buckets | `repo_mapper.py:916,990` | multi-role subsystem |
| Subsystem index (hardcoded + generic paths) | `repo_mapper.py:937,946` | repo matching neither shape |
| `handoff_paths` / `neighbors` cross-subsystem flow | repo_map schema | subsystems that call each other |

### 4.5 Unaudited — inventory incomplete

No capability inventory yet exists for: PDG/taint/CFG (872 LOC), LSP integration (775),
debug/DAP runtime observations (1,207), visualization and export (2,591), semantic index and
AST matcher (449). Each needs the same treatment before its fate — port, keep in Python, or
drop — can be decided.

## 5. Target architecture

A single Rust binary owning indexing, storage, watching and the agent interface. CodeGraph
validates the shape (Rust kernel + tree-sitter compiled in + plain SQLite/FTS5); the storage
engine does not need replacing, since SQLite is 3% of the current build.

```
devmap serve     // long-lived: watcher + index + MCP over stdio/socket
devmap build     // one-shot index, for CI
devmap query …   // CLI queries, token-budgeted like the MCP surface
devmap status    // freshness, generation, lock holder
```

```
rust-port/crates/
  devmap-extract/   // tree-sitter, one module per language, rayon
  devmap-resolve/   // symbols, calls, receiver types, imports
  devmap-analyze/   // liveness, dead code, Leiden clustering, PDG
  devmap-store/     // rusqlite, v3 schema, generations
  devmap-query/     // token-budgeted responses
  devmap-serve/     // watcher, daemon, MCP
  devmap-cli/       // binary entry point
```

The 37 Python consumers do not import a shim — they talk to the daemon over a versioned local
socket. A thin client of ~400 LOC replaces 26,837 and contains no analysis logic.

Making the daemon the sole ingestion path turns three findings into structural impossibilities
rather than fixes: **B1** cannot occur when the queue lives in a process that does not exit,
**N8** is satisfied by construction, and **B4** dissolves because the daemon holds the write
connection while readers use WAL snapshots.

### Why Rust over Go

Tree-sitter's canonical binding is Rust; `rusqlite` is mature; `rayon` makes parallel
extraction trivial. Go's main advantage — simple cross-compilation — is forfeited the moment
tree-sitter pulls in cgo, and cgo taxes every node visit.

### Reference architectures

| Dimension | dev map (today) | GitNexus | CodeGraph |
|---|---|---|---|
| Core language | Python | TypeScript / WASM | **Rust kernel** |
| Storage | SQLite + FTS5 | LadybugDB (ex-Kuzu) | **SQLite + FTS5** |
| Languages | **35** | 14 | 20 |
| Parallelism | serial; pool capped at 4 | cores−1, max 16 | sized from cores + RAM |
| Ingestion trigger | per-tool-use hook | git-diff on re-run | **OS file events, 2s debounce** |
| Incremental cost | scales with *repo* size | changed files only | **scales with change size** |
| Search ranking | BM25 only | **BM25 + vector + RRF** | FTS5 |
| Clustering | Louvain, skipped at 15s | **Leiden + cohesion** | — |
| Query surface | many tools + regex Cypher | 17 tools + real Cypher | **one primary tool** |

Deliberately **not** imported: the embedded graph database. It buys real Cypher (N10) at the
cost of replacing a layer that is not the bottleneck.

---

## 6. Phases

Six phases, 34–46 weeks, one engineer. Estimates are **inferred**, not derived from comparable
work in this repository. No phase before cutover ships user-visible value — that is the cost of
the clean-room choice.

### Phase 1 — Freeze the contract · 4–5 weeks
Capture what the current system produces before replacing it: golden corpus across all 35
languages, canonical graph snapshots from the Python implementation, and the 37 consumer
contracts written down — especially the verification checks, whose graph-shape assumptions
exist nowhere but their assertions. Specify token budgets now so the new surface is designed
against them rather than retrofitted.

**Complete the §4 capabilities inventory**, then build the corpus *from* it. The inventory is
currently drawn from the 49% of the subsystem the audit opened; the unaudited areas in §4.5
have no inventory at all. Every row in §4 names a corpus requirement, and a capability whose
requirement is absent from the corpus is a capability parity cannot protect. This is the phase
where an unwritten behaviour becomes a test or becomes a future regression.

**Gate:** §4 inventory complete for all 26,837 LOC; corpus satisfies every stated requirement;
golden graphs for 35 languages; every consumer contract has a signature and a test; token
budgets specified per surface; scoping decision recorded for each §4.5 area (port / keep in
Python / drop).

### Phase 2 — Extraction · 6–10 weeks
Tree-sitter for all 35 languages including Python, resolving **D1** by construction. Parallel by
default with `rayon`, pools sized from real cores (**B12**, **N9**). Grammars link into the
binary, so the spawn-based crash isolation, 30s per-file timeout and quarantine/respawn logic
have no analogue to port — a panicking grammar unwinds one task.

**Gate:** extraction parity against golden corpus, all 35 languages; near-linear core scaling;
Python-extraction divergences enumerated and individually accepted.

### Phase 3 — Resolution and analysis · 10–14 weeks
Largest and hardest — 76% of current build time, global passes with subtle ordering semantics.
**B13**–**B15** have no counterpart in a design built on interned `FileId`s and a single tree
traversal. Fold in quality work while the code is open: Leiden with cohesion scores (**N5**,
**R6**), constructor-inferred receiver types (**N6**), dynamic-dispatch hops in call paths
(**R5**), explicit status on timed-out analysis (**N4**). **R5** builds directly on **N6** —
once a receiver's type is tracked, an interface-dispatch hop is resolvable rather than a dead
end — so they are one work item, not two.

**Gate:** full-graph parity; cold build for this repo under 2s; resolution precision ≥ Python
baseline; every community reports cohesion; interface-dispatch path resolves in corpus.

### Phase 4 — Store and daemon · 5–7 weeks
v3 schema: interned paths (**B10**), clean primary keys (**B9**), indexable FTS pruning
(**B6**), O(1) cache admission and liveness-ordered eviction (**B7**, **B8**), scheduled vacuum
(**B11**), one schema with no rollback shadow (**D2**), WAL auto-checkpoint tuned for build
churn (**R4**). Differential membership writes *with* deletion reconciliation — **B3 and N2 are
one work item and must never be separated**. Daemon owns the watcher and a durable queue, and
exposes targeted repair so a corrupt FTS index does not force a full rebuild (**R7**).

The durable queue is what converts lease contention from data loss into back-pressure
(**B2**) — with work persisted, a writer that cannot acquire the lease has deferred, not
dropped, so the timeout stops being a correctness parameter. Status surfaces report the lease
holder from the lock itself rather than stale advisory metadata (**D3**).

**The watcher must observe git HEAD, not only the working tree** (**B5**). A commit, branch
switch, rebase or stash changes what the index should contain while touching no watched file —
`.git/HEAD` and `.git/refs` need their own watch, or the clean-room design reproduces exactly
the staleness that motivated the rewrite.

**Gate:** 1-file edit writes <100 rows at 10k files; kill −9 mid-build replays; index <60 MB for
this repo; freelist <5% without manual vacuum; `devmap repair --fts` rebuilds the search index
alone; freshness holds across a 30-minute multi-session burst; **`git commit` then idle leaves
the index fresh**; a lease timeout under load loses zero edits; status never names a dead pid.

### Phase 5 — Token-budgeted query surface · 5–6 weeks
Graph stays resident in the daemon, so **N1**'s 1,639 ms reload has no analogue. Every response
takes a budget and reports truncation. The 129k-token repo map becomes a ≤2k-token manifest
plus queries; the 17.4 MB export becomes opt-in and moves out of the context directory
(**T1**–**T3**, **N3**). Add RRF over BM25 and vectors (**N7**), bounded by a node cap so the
embedding index cannot grow unbounded (**R3**); either implement real graph queries or retire
the Cypher shim honestly (**N10**).

Collapse today's 18 map/graph MCP tools to one primary plus a small unlisted set (**R1**), and
have results carry verbatim source grouped by file (**R2**). These two are the largest
remaining token wins and they compound: tool definitions cost context on every request whether
or not they are called, and a result without source forces a follow-up read that costs another
round-trip.

**Gate:** explore p95 <50 ms at 10k files; no response exceeds its budget; every truncation
reports `{shown, total}`; manifest ≤2k tokens; ≤3 listed tools; a symbol query answers without
a follow-up file read; peak RSS <1 GB at 10k files.

### Phase 6 — Cutover and deletion · ~4 weeks
Point the 37 consumers at the client, run both systems in parallel against live work until the
parity harness is quiet, then delete `indexing/` and `codeintel/`. Deleting *is* the phase — a
rewrite that lands beside what it replaces doubles the surface permanently.

**Gate:** all 16,534 LOC of existing tests green against Rust; 26,837 LOC removed; no Python
analysis code remains; binaries published for every supported platform.

---

## 7. Implementation guide

### 7.1 Token budget as a protocol primitive — T1–T4

Put the budget in the request type so it cannot be forgotten; make truncation a value rather
than an absence. This generalises what `prompt_builder.py:281-319` already does correctly for
file bodies.

```rust
pub struct Request<Q> {
    pub query: Q,
    pub token_budget: u32,   // required — no default that hides cost
    pub min_confidence: Confidence,
}

pub struct Response<T> {
    pub items: Vec<T>,
    pub shown: u32,
    pub total: u32,          // what a complete answer would have held
    pub truncated: bool,
    pub tokens_used: u32,
}
```

Two rules make this load-bearing. **Rank before truncating**, so the budget keeps the most
relevant rows rather than the first ones. And **carry both numbers always** — a capped sample
presented as complete coverage is how "no dead code found" comes to mean "we stopped at 200".

### 7.2 Replace the repo map with a manifest plus queries — T1

78% of the map is `dependents` (63,844 tokens) and `files` (35,512) — lookup tables shipped
whole to answer point questions. Neither belongs in a file an agent reads.

```
Manifest (~2k tokens total):
  subsystems (names + entry points only)   ~800 tok
  important_files                          ~140 tok
  entry_roots, languages, test_commands     ~80 tok
  freshness: head, fingerprint, generation   ~30 tok
  -- dependents, files: removed. queried instead.

Replaced by:
  devmap where <symbol>            // one answer, not 853 file entries
  devmap deps <path> --budget N    // ranked, truncation reported
  devmap impact <symbol>           // precomputed blast radius + confidence
```

CLAUDE.md changes with it: "open repo_map.json first" becomes "query the map". ~65× reduction
for one line of documentation.

### 7.3 Collapse the tool surface, return source with results — R1, R2

Today the MCP surface exposes 18 map/graph tools. Tool definitions are serialised into *every*
request whether or not they are called, so an unused tool is a standing tax on every turn.
CodeGraph lists one primary tool and leaves the rest unlisted but callable.

```
Listed (≤3):
  devmap_explore   // symbols + call paths + impact + verbatim source, budgeted
  devmap_search    // ranked lookup, budgeted
  devmap_status    // freshness, generation

Unlisted but callable: dead, routes, cypher/paths, affected-tests,
                       impact, trace, process, api-impact, …
```

R2 is the other half. A result that gives a location forces the agent to issue a file read,
which costs a round-trip and usually pulls in far more of the file than the answer needed.
Returning the relevant spans inline, grouped by file, makes the query self-contained:

```rust
pub struct SymbolHit {
    pub id: SymbolId,
    pub path: FileId,
    pub span: (u32, u32),
    pub source: String,      // R2: the lines themselves, budget-trimmed
    pub flows: Vec<FlowRef>, // §7.4: which processes it participates in
}
```

Both interact with the budget from §7.1: source is the first thing trimmed when a response
approaches its limit, and the trim is reported rather than silent.

### 7.4 Precompute so one call is enough — GitNexus's central idea

The deepest token saving is avoided round-trips, not compression. An agent that issues four
queries and reassembles edges spends far more context than one receiving a structured answer.
Store entry-point-to-leaf call flows at build time and let a symbol query return the flows it
participates in with step indices — not edges the caller must walk. `min_confidence` is a
request parameter with a conservative default, since speculative edges cost context and
mislead.

### 7.5 Differential writes with deletion reconciliation — B3 + N2

This is the 7.55s. Each generation currently re-materialises all 59,880 membership rows.

```
1. carry forward  rows from prev generation whose source file is not in :affected
2. delete         rows whose source file is in :affected
3. insert         freshly resolved rows for :affected
4. reconcile      any file present in the previous generation but absent from
                  `git ls-files` is a deletion — add it to :affected even though
                  nothing on disk changed
```

**Step 4 is why B3 and N2 cannot ship separately.** Deletion is currently handled implicitly
*by* the full rewrite; make writes differential without it and deleted files persist as live
nodes forever. Renames are a delete plus an add — correct, though it loses rename identity,
which nothing downstream consumes today.

### 7.6 Schema v3 — B6, B9, B10, D2

```sql
CREATE TABLE paths (
    id   INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
);

CREATE TABLE generation_edges (
    generation_id  INTEGER NOT NULL,
    ordinal        INTEGER NOT NULL,
    payload_hash   TEXT NOT NULL,
    source_file_id INTEGER NOT NULL REFERENCES paths(id),
    target_file_id INTEGER NOT NULL REFERENCES paths(id),
    PRIMARY KEY (generation_id, ordinal)   -- B9: payload_hash was redundant
) WITHOUT ROWID;

-- FTS holds text only; membership is indexable, so prune is a bounded delete
CREATE VIRTUAL TABLE nodes_fts USING fts5(
    name, qualified_name, path, tokenize='unicode61'
);
CREATE TABLE nodes_fts_map (
    rowid_ref     INTEGER NOT NULL,
    generation_id INTEGER NOT NULL,
    PRIMARY KEY (generation_id, rowid_ref)
) WITHOUT ROWID;

CREATE TABLE pending_paths (      -- B1: survives process death
    path       TEXT PRIMARY KEY,
    queued_at  REAL NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
```

Pending rows are deleted only after the generation commits, so a crash mid-build replays rather
than drops. In the core, `FileId(u32)` replaces every string path in hot loops; the per-edge
language classification (**B15**) collapses into a field resolved once at intern time.

### 7.7 Parallel extraction — B12, N9

```rust
use rayon::prelude::*;

pub fn extract_all(files: &[FileRef]) -> Vec<(FileId, Extraction)> {
    files
        .par_iter()
        .filter_map(|f| {
            if let Some(hit) = cache::get(f.content_hash) {
                return Some((f.id, hit));
            }
            let tree = parser_for(f.language).parse(&f.source)?;
            Some((f.id, extract(&tree, &f.source)))
        })
        .collect()
}
```

Size the pool from real cores rather than the current `min(4, cpu_count()//2)`. CodeGraph sizes
analysis caches from free RAM too, which matters at 10k files where Python peaks at 714 MB.

### 7.8 Parity harness — gates every phase after 1

Structural comparison, not textual — ordinals and iteration order differ legitimately between
implementations. Normalise both sides to sorted sets keyed on stable identity, and run per
language so a divergence names the grammar responsible.

```
nodes: sorted (id, kind, path, line, name, exported)
edges: sorted (source, target, kind, confidence)
dead:  sorted (id, confidence)
```

Confidence belongs in the comparison: a rewrite finding the same edges at different confidence
changes what the verification gates decide. Expect real divergences from **D1** — the gate is
that each is enumerated and accepted deliberately, not that the count is zero.

### 7.9 Consumer client and distribution

The Python client is a transport, not a port: request/response types, a socket, and auto-spawn
of `devmap serve` if not running. ~400 LOC with no analysis logic. Keeping it that thin is what
makes the phase-6 deletion possible.

Distribution ships the binary per platform — macOS arm64/x86_64, manylinux x86_64/aarch64,
Windows x86_64 — built in CI on tag. Because grammars link into the binary, this **replaces**
the existing `devcouncil-codeintel-grammars` companion package rather than sitting alongside
it; plan that deprecation deliberately.

---

## 8. Risks

- **No value until cutover.** 34–46 weeks before anything improves for a user; staleness and
  query latency persist throughout. Inherent to the clean-room choice, not a flaw in the plan.
- **The estimate is a lower bound.** It was derived from the 49% of the subsystem the audit
  opened. The unread half contains the two largest files (`repo_mapper.py`, `wiring.py`) and
  three concerns with no home in the architecture — see §4.5.
- **The §4 inventory is the thing most likely to be skipped, and most costly to skip.**
  Capability loss from a clean-room rewrite is silent by construction: nothing fails, the graph
  is merely poorer. The `wiring.py` exemption rules are the sharpest case — losing one does not
  break a build, it makes `dev map dead` report entry points as dead, and a dead-code report
  nobody trusts is worse than none.
- **The 35-language matrix is the schedule**, not the Rust — 2.5× GitNexus, 1.75× CodeGraph,
  each needing parity evidence.
- **D1 guarantees divergence.** Moving Python onto tree-sitter will change extractions. The
  gate is enumerated-and-accepted, not zero-diff, and someone must make those calls.
- **B3 and N2 must ship together.** Differential writes without deletion reconciliation leave
  deleted files in the graph permanently — worse than the slowness being fixed.
- **37 contracts, mostly unwritten.** Phase 1 exists to find them before phase 6 depends on
  them.
- **Token work is separable.** T1–T3 are generated by Python today and could ship in days —
  129k tokens reduced to ~2k without waiting for the rewrite. Holding them until phase 5 is a
  deliberate choice worth revisiting if the schedule slips.

---

## Appendix — how the measurements were taken

```bash
# Cold-build profile
python -c "import cProfile; from devcouncil.indexing.graph.build import build_code_graph; ..."

# Benchmark harness (existing, in-repo)
python -m pytest tests/performance/test_codeintel_benchmark.py
# heavy profile via tests/performance/benchmark_harness.run_benchmark(profile="heavy")

# Storage breakdown
sqlite3 .devcouncil/codeintel/index.sqlite \
  "SELECT name, SUM(pgsize) FROM dbstat GROUP BY name ORDER BY 2 DESC"

# Query cost
python -c "from devcouncil.codeintel.service import get_codeintel_service; ..."  # times .load()

# Token cost
wc -c .devcouncil/repo_map.json .devcouncil/graph/code_graph.json   # ÷ 4 for tokens
```
