# dev map → Rust: Clean-Room Rewrite Plan

**Status:** Phase 6 hybrid. `dev map` **already executes the Rust kernel** —
`rust-port/target/release/devmap`, with `src/devcouncil/devmap_engine.py` as the seam. Python
`indexing/` + `codeintel/` are still present and still own the surfaces not yet migrated, but
they are no longer what a `dev map` build runs. See [STATUS.md](STATUS.md) for what is actually
migrated and [PHASE1_CONTRACT.md](PHASE1_CONTRACT.md) for token budgets and §4.5 scoping.
**Audit basis:** DevCouncil @ `98f23e3`, 769 source files, macOS arm64, CPython 3.12.13, 2026-08-11.
**Kernel measurements (§2.1, §3 `K` findings, §3 `G` gaps):** 2026-09-02, DevCouncil @ 994 code
files and scholarlm @ 3,674, same host. Everything outside those three sections profiles the
pre-port Python incumbent.
**Rendered version:** [PLAN.html](PLAN.html) — now genuinely generated from this file by
`tools/render/render_plan.sh` (pandoc; styling preserved in `tools/render/plan_head.html`).
**Re-run it after editing this file.** Until 2026-09-02 the claim here was false: PLAN.html was
a hand-maintained 2026-08-12 snapshot with 8 sections against this file's 11 and headings with
no counterpart here, drifting further with every edit.

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

**Read §2.1 before acting on anything in this section.** Everything from here to
§2.1 profiles the **Python incumbent** at the 2026-08-11 audit basis — 769 files,
15.4 s. It is the case *for* the port, not a description of what `dev map` does
today. `dev map` has run the Rust kernel since the Phase 6 hybrid pass, so a
reader who takes "cold build 15.4 s" as current is out by an order of magnitude
and will optimise a tier that no longer runs.

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

### 2.1 Rust kernel — measured 2026-09-02

The figures above are the incumbent. These are the shipped kernel, measured with
`benchmarks/map_bench.py` against a scratch store (the real `.devcouncil/` is never
touched), reporting the **minimum** of N runs rather than the mean: the minimum is the
run least contaminated by other load, and a mean on a laptop mostly measures what else
was scheduled.

| Stage | DevCouncil · 994 files · 41.8 MB | scholarlm · 3,674 files · 37.9 MB |
|---|---:|---:|
| `cold` — full index into an empty store | 2.13 s | 10.22 s |
| `warm` — no-op rebuild, nothing changed | 0.196 s | 0.900 s |
| `touch` — one file edited, incremental | 1.31 s | 7.63 s |
| `manifest` — write `repo_map` + `code_graph` | 0.367 s | 1.26 s |
| `e2e` — `dev map` end to end via the Python seam | 1.04 s | not measured |
| `repo_map.json` | 310,925 B | 1,236,709 B |
| `code_graph.json` | 21,067,574 B | 84,276,217 B |

The DevCouncil `repo_map.json` figure is post-`G6`: it was 418,688 B when §2.1 was first
written and 310,925 B after the compaction, a 25.7% reduction achieved without changing the
schema any consumer reads. The scholarlm column predates that work and has not been re-measured.

Against the pre-pass baseline on DevCouncil: **e2e −57.5%**, cold −36.8%, touch −35.7%,
manifest −29.5%, warm −14.7%. The e2e figure is much larger than the cold figure because
roughly 70% of `dev map`'s wall time was Python wrapper overhead around the kernel, not
kernel work — the seam, not the engine, was the dominant cost.

**Where a cold build actually goes** (scholarlm, `--json` breakdown, post-K8):

| Phase | Quiet host · 13.44 s | Loaded host · 20.11 s |
|---|---:|---:|
| scan + extract | 7.25 s · 54% | 8.30 s · 41% |
| resolve | 3.29 s · 24% | 5.76 s · 29% |
| persist (total) | 2.65 s · 20% | 5.62 s · 28% |
| ├─ `persist:write` | 2.50 s | 5.37 s |
| ├─ `persist:prune_extractions` | 0.13 s | 0.23 s |
| ├─ `persist:vacuum` | 0.02 s | 0.03 s |
| └─ `persist:prune_generations` | 0.04 ms | 0.2 ms |
| analyze | 0.24 s | 0.42 s |

**Extraction dominates, then resolution, then the store write** — the one ordering that holds
across both. The right-hand column was taken while a concurrent build occupied the machine; it
is included rather than discarded because it shows which conclusions are load-sensitive (the
exact shares are; the ordering is not). Sub-phase seconds are *included* in their parent, not
additional to it: on the loaded run they sum to 5.624 s against a parent of 5.624 s.

This contradicts what a profiler would have reported before K8 was fixed. The instrumentation
attributed each phase's cost to the *next* phase's label, so the same 13.44 s run "showed"
resolution at 54% and **extraction at 42 nanoseconds**. The four `persist:*` sub-phases were
always measured correctly; only the four top-level stages were shifted.

**Two caveats on this subsection.** `touch` is an incremental build and still costs 60%
of a cold one at 3,674 files, because resolution is deliberately global on every changed
build — narrowing it was tried and broke liveness and community detection, so the cost is
a known consequence of a correctness decision, not an unexamined one. That is B3/SC2, and
it is unresolved. Second, the `.devcouncil/codeintel/index.sqlite` in this repository is
at `user_version=2` while the current binary writes `user_version=11`; the live store is
nine schema versions stale, so it is not a valid sample of current behaviour and no figure
above is drawn from it.

---

## 3. Findings as specification

> **2026-09-02, second pass.** Thirty-one further defects were found and fixed
> against the *shipped* kernel and its Python seam — `K1`(a–h), `K2`, `K3`,
> `K4`, `K5`, `K6`, `K7`, `K12`, `K13` in the store/build/drain, `S1`–`S10` in
> serve/query, and nine in the seam. They are recorded, with the failing-first
> test for each and the live before/after numbers, in the root
> `IMPROVEMENTS.md` → "Dev Map kernel audit, second pass (2026-09-02)" and
> summarised in `STATUS.md` → "Kernel audit, second pass". This section keeps
> the original 47 findings and `K1`–`K8` / `G1`–`G8`; it is not restated there.

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

### Gortex-derived gaps — reviewed 2026-09-02

A third reference ([zzet/gortex](https://github.com/zzet/gortex)) was reviewed against this
kernel. Unlike the GitNexus/CodeGraph items above, most of these are **capabilities the
kernel lacks**, not defects. Recorded so the list is explicit rather than implied by absence.

| ID | Capability | State |
|---|---|---|
| G1 | Breadth beyond linked grammars | **partly closed** — tier-2 recovery (K2/K3) serves grammarless languages; K1 leaves the nodeless case open |
| G2 | Lexical + semantic retrieval that actually ranks | **closed** — TF-IDF replaced the random-projection hash ranker (K7), and now lives in the kernel as `devmap-query/src/semantic.rs`, computed per query rather than stored |
| G3 | Clone / duplicate detection | **in progress** — `devmap-extract/src/clonesig.rs`, owned by a separate session as of this writing |
| G4 | Speculative edit preview — answer "what breaks if I change this" without writing | **closed** — `devmap preview` / `dev map preview`; `preview_writes_nothing` asserts no generation is committed |
| G5 | Notebook (`.ipynb`) extraction | open |
| G6 | Compact binary wire format for the graph export | **closed by measurement, not by adopting a binary format** — `repo_map.json` −25.7% (418,688 → 310,925 B) by dropping pretty-printing and a constant empty `summary` on 1,306 file entries. A columnar/interned form reaching 72% was measured and **declined**: it changes the `files`/`dependents` shape CLAUDE.md documents to agents as the navigation contract |
| G7 | Savings accounting — report tokens saved vs. reading the files | **closed** — `devmap savings`, every figure bytes÷4 and labelled as such, counterfactual named, unreadable indexed files counted separately rather than folded in as zero |
| G8 | Cross-repository graph | **declined, restated** — the multi-repo registry was already declined above as a product change. Gortex does not change that judgement; noted so a future reader does not re-open it as an oversight |

G6 is the one with a measured cost attached: at 84,276,217 B the scholarlm graph export is
~21M tokens, which is not an artifact any agent can open. That is the same class of problem
as T1/T2 — it is bounded today only by being opt-in, not by being small.

### Kernel findings — 2026-09-02

The 39 items above were derived from the **Python incumbent**. These eight were found in
the **Rust kernel itself**, by measuring it rather than by reading it, during the
performance-and-gap pass. They are numbered `K` to keep that provenance visible: a `B`
finding is a defect the rewrite must not reproduce, a `K` finding is one the rewrite
introduced or inherited and now owns.

| ID | Property | Evidence | Status |
|---|---|---|---|
| K1 | Every discovered file has at least one addressable node | `File` node was pushed only on the tree-sitter path; a grammarless file yielded **zero** nodes, so it could not be an edge target | closed |
| K2 | A source language with no grammar is still discovered | `is_indexable_source` admits only what `detect_language` names, so 19 `.proto` and 17 `.ps1` were dropped before extraction ran | closed |
| K3 | Pattern recovery never runs on prose or data formats | tier 2 attributed `ReasoningBank`/`TrajectoryOutcome` to the `.md` files that *described* them: 40 markdown files, 457 invented symbols | closed |
| K4 | A symbol larger than the token budget still returns a hit | search returned **zero** results for large-symbol matches rather than a truncated one | closed |
| K5 | The reclaim decision reads committed state | `freelist_count` was read before the WAL checkpoint, reporting 0 free pages while 33% of the store was free | closed |
| K6 | A daemon never serves a superseded binary | a rebuilt kernel left the running daemon answering from the old code | closed |
| K7 | An exact term match ranks first | hash-projection embeddings put the correct symbol at rank 28 or absent on all 5 probes; TF-IDF puts it at rank 1 | closed — but **re-sited**: the Python module carrying the fix was deleted in `079bc50` and the ranking now lives in `devmap-query/src/semantic.rs`, stateless at query time |
| K8 | A phase profiler attributes cost to the phase that incurred it | `stage()` recorded time-since-previous-announcement against the *next* label: extraction's 7.25 s printed as "resolving", extraction as 42 ns | closed |

**K1 was the one that mattered most, and it is now closed.** K2 and K3 make tier-2 recovery
correct *when it fires*, but a grammarless file the scanner finds nothing in — 8 of the 17
`.ps1` on scholarlm, and every `.md`, `.json`, `.yaml` in any tree — was still recorded in
`generation_files` and absent from the graph. Recovering declarations was only half the gap;
the other half was that the file itself was not addressable.

`unavailable_extraction` now emits the `File` node unconditionally, for prose and data
formats too: being unable to recover a file's declarations is not a reason to deny that the
file exists, and a Markdown document something links to still has to be a valid edge target.
The change is safe against the dead-code surface **by construction rather than by tuning** —
`File` nodes are already exempt in `liveness.rs` (both the Go duplicate-identity count and
the dead-symbol sweep) and are skipped as `Contains` sources in the resolver, so this widens
what the graph can address without adding one dead-symbol candidate.

Verified at both scales: on a 4-file probe every file carries exactly one `File` node and the
Markdown file still contributes **zero** declaration symbols; on scholarlm, **4,089 files, 0
with zero nodes, 0 missing a `File` node**, where 380 `Failed` files previously contributed
nothing at all. A second test pins the thing that would make this a false win — the `File`
node must not move `parse_outcome`, so a file with no recoverable declarations still reports
`Failed` and only a real recovery reports `Fallback`.

**K3 is a regression this pass introduced and then closed**, and it is worth recording as
such rather than as a clean win: tier-2 recovery was added, and it immediately did the
exact thing the module's own documentation warns against — "a missed declaration costs a
search hit, while an invented one puts a symbol in the graph that does not exist, and the
graph is what other tools reason about." The guard now excludes markdown, html, css, json,
yaml, toml, config and text, as a **denylist** rather than an allowlist: the tier exists to
serve languages nobody enumerated, so its default must be to scan.

**K8 is closed and verified by execution.** Stages now close against their own label and
sub-phases nest under their parent. Three properties were checked on a live build rather
than asserted: the stages sum to the reported total (20.108 s of 20.110 s, a 2 ms tail),
the four `persist:*` sub-phases sum exactly to their parent stage (5.624 s of 5.624 s, so
nesting does not double-count), and extraction is labelled as extraction rather than
reporting 42 ns. A stage still running when the breakdown is requested is emitted with
`"open": true` and its elapsed-so-far, so the stages can never silently fail to account for
the total — Class A (§3.1) applied to the profiler itself.

### 3.1 Failure classes — what recurs, and what would close it

The 47 findings above are listed by *subsystem*. Grouped by **failure shape** instead, they
collapse into five classes, and four of the five are still open as classes even though every
individual instance is closed. This is the distinction between fixing a case and hardening:
a class stays open until something structurally prevents the next instance, and every class
below produced a *new* instance in the Rust kernel after the Python instance was fixed.

Each class states the gate that would close it, and **all five are now implemented**:
Class B in `devmap-cli/tests/coverage_invariants.rs` (its assertions must walk the filesystem,
so it stands alone), Classes A, C and D in `devmap-cli/tests/failure_class_gates.rs`, and
Class E as unit tests beside `ProgressReporter` in `devmap-cli/src/main.rs`, which is where
the type lives.

The gates are stated over **types and outcomes**, not over specific inputs. "Search finds
`foo`" pins a case; "an operation that declined to run is distinguishable from one that ran
and found nothing" pins the shape, and the shape is what kept recurring. Each was checked
against a mutation or a revert rather than merely observed to pass — a gate nobody has seen
go red is a gate nobody has tested.

---

**Class A — a check that could not run reports as one that ran and passed.**

| Instance | What reported success |
|---|---|
| `N4` | a `graph/intel.py:118` timeout returned the same shape as a completed analysis |
| `K4` | search returned **zero** hits for a matching query; both budget gates stayed green |
| `K5` | `vacuum_if_needed` read `freelist_count` pre-checkpoint, saw 0 free pages on a store that was 33% garbage, and reported a 0 ms vacuum as success |

The instances look unrelated — an analysis timeout, a query budget, a storage reclaim — which
is exactly why fixing them one at a time did not close the class. In all three, the failure
path and the success path are **indistinguishable in the return value**, so no caller and no
gate can tell them apart. `K4` is the sharpest: both budget properties were satisfied
(nothing exceeded budget, every truncation was reported) by returning nothing at all.

*Gate that closes it:* every operation that can decline to run returns an outcome that
distinguishes **ran-and-found-nothing** from **could-not-run**, and no gate asserts a
threshold on a number that has a not-computed state without first asserting it was computed.

**Implemented** — `failure_class_gates.rs::class_a_declined_is_not_success`, three assertions:
a declined vacuum is distinguishable from a completed one *and* carries the counters it was
decided from; work dropped against a cap is counted rather than yielding a prefix presented as
a set; a file discovery refused is recorded with a reason instead of vanishing between the
walker and the extractor. The first also asserts the enum can express "ran" at all, because a
single-variant outcome would satisfy the distinction while meaning nothing.

Still auditable mechanically for new code: any function returning a bare collection, count or
`bool` where a timeout, budget exhaustion, cap or skip is reachable is a candidate.

---

**Class B — the pipeline's own view is used to measure the pipeline's coverage.**

| Instance | What was invisible |
|---|---|
| `K2` | 19 `.proto` + 17 `.ps1` dropped at *discovery*, so they never reached the extractor to be counted as missing |
| `K1` | grammarless files got **no node at all** — recorded in `generation_files`, absent from the graph (closed; gated at 0 nodeless files) |
| `B10`/`SC5` | size assumptions calibrated against what the store held, not what the tree contained |

A coverage number computed from the index can only describe files the index already admits.
`K2` is the proof: "35 languages parse correctly" was true *and* two whole languages were
missing from every graph. No parity gate stated over extractor input could ever have seen it.

*Gate that closes it:* every coverage and completeness claim is stated over **on-disk truth**,
never over the indexer's own inventory, and the two counts are reported together so a
divergence is visible rather than inferable. This is how the 19/19 and 17/17 figures in §2.1
were confirmed, and it is the only reason `K2` was found at all.

**Implemented** — `devmap-cli/tests/coverage_invariants.rs`, the only one of the five gates
that exists as code today. Three assertions over a fixture tree walked from the *filesystem*
rather than through the walker: discovery admits every file it claims to handle (K2), every
extracted file contributes at least one node (K1), and — because Class B and Class C pull in
opposite directions — addressability is **not** bought by declaration-scanning prose (K3).
That third assertion matters: the cheapest way to make "every file has a node" true everywhere
is to scan everything, which is exactly how 457 symbols were read out of fenced code blocks.
All three fail against the pre-fix tree.

---

**Class C — invented data presented in the same shape as derived data.**

| Instance | What was fabricated |
|---|---|
| `K3` | 457 graph symbols read out of fenced code blocks in markdown design documents |
| `K7` | random-projection hash vectors ranked noise as semantic similarity |
| `N10` | a regex shim presented as a query language |
| `D10`-adjacent | an empty `indexed_hash` reading like a computed digest |

The graph is what other tools reason about, so a fabricated symbol is not a cosmetic defect —
it is an assertion that code exists which does not. `K3` is the cautionary instance because
the module that produced it *documents this exact risk in its own header* and did it anyway:
recovery was pointed at prose, and precision-over-recall was stated but not enforced.

*Gate that closes it:* every derived artifact carries machine-readable provenance for **how**
it was derived, and consumers can filter on it.

**Implemented** — `failure_class_gates.rs::class_c_provenance_is_machine_readable`. Both
directions are asserted deliberately: a parsed file must report `TreeSitter`/`Clean` and a
pattern-matched one `RegexFallback`/`Fallback`. Pinning only the fallback side would let
"label everything `RegexFallback`" pass while destroying the distinction. A second assertion
pins the subtler half — a pattern-matched symbol must not claim `is_exported`, because
exportedness is a language rule the line scanner never evaluated, and asserting it would put
a public-API claim into the graph on the strength of a regex.

---

**Class D — a stale component serves as if fresh.**

| Instance | What was stale |
|---|---|
| `K6` | a rebuilt binary left the daemon answering from superseded code |
| `B5` | a commit/rebase/stash changed what the index should hold while touching no watched file |
| §2.1 | the working store is at `user_version=2` against a binary writing `user_version=11` |
| `D3` | `writer.lock` naming a dead pid |

*Gate that closes it:* every long-lived component validates the identity of what it depends
on — binary, schema, HEAD — at each service boundary rather than at startup only, and
refuses rather than degrades on mismatch.

**Implemented for the schema boundary** — `failure_class_gates.rs::class_d_identity_is_validated`
asserts a store stamped newer than `CURRENT_SCHEMA_VERSION` is *refused*, not opened and
misread. Fail-closed is the only safe direction: a forward-compatible read of an unknown
schema silently misreads rows, and a graph that is confidently wrong is worse than one that is
unavailable. The binary boundary is covered by K6's own regression. **The HEAD boundary (B5)
is not gated**, and the working store's nine-version drift is the standing evidence that
schema drift goes unnoticed in practice even though it is now refused at open.

---

**Class E — a measurement is attributed to the wrong thing.**

| Instance | What it misled about |
|---|---|
| `K8` | phase costs recorded against the *next* stage's label: extraction's 7.25 s printed as "resolving" |
| §2 | incumbent Python figures readable as current kernel figures |
| `main.rs` | a comment calling persistence "the largest phase of a build", inferred from `K8`'s output |

This class is the most corrosive because its output is *plausible*. `K8` produced a profile
that named a real phase, with a real duration, summing to the real total — and pointed at the
wrong one. Everything downstream that consumed it inherited the error, including a code
comment that is still wrong today.

*Gate that closes it:* every reported measurement carries the corpus, date and tier it was
taken from, and a derived figure states its derivation. §2.1 and the appendix now do this;
§2 has a pointer rather than a restatement, which is weaker but does not require re-measuring
the incumbent.

**Implemented for the profiler** — unit tests beside `ProgressReporter` in
`devmap-cli/src/main.rs`. Three properties a consumer needs before trusting a breakdown, none
of which held before K8: the stage that slept is the stage that reports the time; sub-phases
nest inside their parent and are included in its total rather than listed beside it, so no
consumer can double-count; and the stages account for the whole build, which is precisely the
check a 42-nanosecond extraction would have failed. A fourth test pins Class A *applied to the
profiler* — an unfinished stage is emitted with `"open": true` rather than omitted, because
omitting it would let the breakdown silently fail to account for the total.

Verified by mutation: forcing a stage to record no time of its own turns the attribution test
red while the other four stay green.

---

**Why this section exists.** Four of these five classes produced a fresh instance in the Rust
port *after* the Python instance had been found, fixed and written into this plan as a
property. Class A produced three instances across three unrelated subsystems. A plan that
lists 47 properties but does not name the shapes they share will keep catching instances one
at a time, which is what happened here — the six defects closed in the 2026-09-02 pass were
all found by measurement, and none of them by the acceptance tests written to prevent their
class. The gates above are the correction: 14 assertions across five classes, stated over
shapes rather than cases.

**What they do not cover.** A gate proves the shape is held *at the points it inspects*. Class
A's mechanical audit — every function returning a bare collection or count where a decline is
reachable — has not been run across the workspace; Class D does not cover the HEAD boundary
(B5); Class E covers the build profiler and not every reported measurement. These are gates
against recurrence at the known surfaces, not proof of universal absence.

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

**Added 2026-09-02 — this phase owns K1, K2 and K3, and extraction is the largest phase of
a cold build** (7.25 s of 13.44 s, 54%, §2.1). Two gate clauses are missing and should be
added before the phase can close:

- *Every discovered file is addressable.* K1, **closed**: the `File` node was emitted only on
  the tree-sitter path, so a grammarless file was recorded in `generation_files` and had no
  node at all. "35 languages parse" does not imply "every indexed file exists in the graph",
  and only the second is what a consumer depends on. Now gated at 0 nodeless files on a
  4,089-file corpus; keep the gate, because the failure was invisible to every parity measure
  that existed.
- *Discovery and extraction are gated together.* K2 was a discovery defect that looked like
  an extraction gap: `is_indexable_source` admits only extensions `detect_language` names, so
  files never reached the extractor to be counted as missing. A parity gate that measures
  only what arrives at the extractor cannot see this class at all — it must be stated over
  files **on disk**, which is how the 19/19 `.proto` and 17/17 `.ps1` figures were confirmed.

Tier-2 pattern recovery (`devmap-extract/src/fallback.rs`) is *not* a substitute for a
grammar and its output is labelled `ParseOutcome::Fallback` / `ExtractionEngine::RegexFallback`
throughout so no consumer can mistake a pattern-matched symbol for a parsed one. K3 is the
standing warning about what happens when that tier is pointed at prose.

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

**Added 2026-09-02 — K5 and K6, and one gate that cannot be measured as written.**

- K5 makes the freelist gate above **unmeasurable in its current form**. `vacuum_if_needed`
  read `freelist_count` *before* checkpointing the WAL, so it saw 0 free pages while 33% of
  the store was reclaimable, and reported a 0 ms vacuum as success. A gate reading the same
  pre-checkpoint counter would have passed on a store that was a third garbage. The
  checkpoint now runs first; the gate should state that it is measured post-checkpoint,
  because the number means nothing otherwise. This is the `N4` property — a check that could
  not run must not report as one that passed — applied to the store's own accounting.
- K6: a rebuilt binary left the running daemon serving the old code, so every query answered
  from a superseded kernel with nothing to indicate it. The daemon now compares its
  executable identity at each tick and retires itself. Add to the gate: **a rebuild retires
  the running daemon**.
- `persist:write` is 19% of a cold build on scholarlm and, on DevCouncil, *larger on an
  incremental build than on a cold one* (2,448 ms vs 1,490 ms) — backwards on its face, and
  the direct consequence of B3 being unresolved. The four `persist:*` sub-phases exist
  precisely so this is attributable; see §2.1. These four were unaffected by K8 and are the
  one part of the old breakdown that can be trusted as recorded.

  **Stale claim to correct when the extract crate is free:** the comment above
  `progress.timed("persist:write", …)` in `devmap-cli/src/main.rs` states that persistence is
  "the largest phase of a build". That was inferred from the pre-K8 attribution and is false —
  extraction is, at 54%. The two millisecond figures in the same comment are sound; only the
  superlative is not.
- The store in this repository is at `user_version=2` against a binary writing
  `user_version=11`. Whatever the intended migration path is, nine versions of drift on the
  primary development store means it is not exercised, and no gate currently notices.

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

**Added 2026-09-02 — K4: "no response exceeds its budget" was satisfied by returning
nothing.** The budget is a hard contract (`test_search_never_exceeds_hard_token_budget`
pins it, correctly), and a symbol whose source span alone exceeded the whole budget was
therefore dropped — search returned **zero** hits for a query that matched. Both halves of
the gate were green: no budget was exceeded, and every truncation that occurred was
reported. Neither noticed that the answer was empty.

The fix caps the span rather than the result (`cap_source_span`, char-boundary safe) and
reports the omission as `source_span_omitted_bytes`. The gate needs a third clause:
**a query that matches returns at least one hit, whatever the size of the match.** This is
the `N4` shape again — an empty result and a no-match result must not be indistinguishable.

Also relevant to `T2` and `G6`: the opt-in export is 84,276,217 B on a 3,674-file corpus
(§2.1), roughly 21M tokens. Opt-in bounds *who pays*, not *how much*.

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

## 9. Distribution surfaces — MCP and plugin packaging

Added 2026-09-02. §1–§8 plan the *engine*; this section plans the two surfaces the engine is
consumed through. Both moved under the port, and neither appears in the phase ledger.

**A naming correction first, because it changes what has to be done.** There is no "MCP 2.0"
protocol version — the specification is date-versioned, and the current revision is
**2026-07-28**. What this repository pins is the *Python SDK* major version:
`pyproject.toml:18` reads `mcp>=2.0.0,<3`. Those are different things with different risk:
the SDK constraint admits any 2.x, and 2.x is the line that carries the 2026-07-28 protocol,
so **this repository has already accepted the largest breaking revision in MCP's history
through a version range, without a compatibility statement anywhere in the tree.** Likewise
"Plugin 1.0" resolves to two distinct standards — Claude Code's own plugin format and the
separate **Agent Plugins 1.0** coalition spec, which Claude Code did *not* adopt. §9.2 treats
them separately.

### 9.1 MCP 2026-07-28 — accepted exposure

The revision is described by its authors as the largest since launch. Three changes are
breaking, and DevCouncil's server (`src/devcouncil/integrations/mcp/server.py`) is exposed to
all three by virtue of the open `<3` bound.

| Change | Nature | DevCouncil exposure |
|---|---|---|
| **Stateless core** — `initialize`/`initialized` handshake and `Mcp-Session-Id` removed; each request self-describes protocol version, client identity and capabilities in `_meta` | breaking | Server is `stdio_server` only, so no session header today; the risk is the *inverse* — a stateless core plus `Store`-backed handles means per-request store opens unless a handle is minted explicitly |
| **Multi Round-Trip Requests** replace server-initiated `elicitation/create`, `sampling/createMessage`, `roots/list`; server returns `resultType: "input_required"`, client retries with `inputResponses` | breaking | **Not used** — verified by search over the whole MCP surface. No migration required |
| **Header-based routing** — `Mcp-Method` and `Mcp-Name` headers required on streamable requests | breaking | Not applicable while stdio-only; becomes mandatory the moment a remote transport is offered |
| **Cacheable list results** — `ttlMs` and `cacheScope` on `tools/list`, `prompts/list`, `resources/list`, `resources/read` | additive | **The largest token win available and it is free.** `tools/list` is re-fetched per session and this repository ships 18 map/graph tools; see R1/T1 |
| **Authorization hardening** — RFC 9207 issuer validation, `application_type` at registration, credentials bound to issuer; Dynamic Client Registration deprecated for Client ID Metadata Documents | breaking for remote | Not applicable to stdio; a hard prerequisite for any hosted deployment |
| **Extensions framework** | additive | Formalises Tasks / MCP Apps; no action |
| Roots, Sampling, Logging; DCR; legacy HTTP+SSE transport | deprecated, 12-month minimum offramp | **None in use** — verified by search: no elicitation, sampling, roots or MCP `logging/setLevel` call site exists (the `logger.info` calls are Python stdlib logging, a different facility). Deprecation exposure is zero |

**Actions, in cost order.**

1. **Pin the protocol, not just the SDK.** Record the protocol revision the server is written
   against and assert it at startup. An open `<3` range means a patch upgrade can change the
   wire contract with no signal — the Class D shape (§3.1) applied to a dependency instead of
   a daemon.
2. **Adopt `ttlMs` / `cacheScope` on `tools/list`.** Additive, no migration, and it attacks
   the same cost T1 and R1 attack: tool definitions are paid for on every request whether or
   not they are called.
3. **Write the compatibility statement.** Which revision the server targets, and the verified
   fact that it uses none of the deprecated features — so a future reader does not re-derive
   it under deadline. Absent today.
4. **Do not offer a remote transport until 1–3 land**, because header routing and the
   authorization changes are prerequisites, not enhancements.

### 9.2 Plugin packaging

Two standards, and the difference matters. **Claude Code's format** is a `.claude-plugin/plugin.json`
manifest — optional, with auto-discovery of `skills/`, `commands/`, `agents/`, `hooks/hooks.json`,
`.mcp.json`, `.lsp.json` — plus `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}` interpolation,
a `userConfig` schema with a `sensitive` flag, declared `dependencies`, and four installation
scopes (`user`, `project`, `local`, `managed`). **Agent Plugins 1.0** is a separate coalition
specification; Claude Code did not adopt it, though plugins authored to it can install.

DevCouncil ships neither. It has the components a plugin packages — an MCP server, skills under
`.claude/skills/`, hooks — distributed by repository checkout instead.

This bears directly on **SC21** in [STATUS.md](STATUS.md), recorded as "the binary being
unreachable outside this repository" and marked closed. It is closed for the *binary*; the
packaging problem it is an instance of is not. A plugin manifest is the mechanism that makes
the engine installable, and three of its features map onto open items here:

- `${CLAUDE_PLUGIN_DATA}` is a per-installation writable directory — the correct home for a
  store that currently lives at a repository-relative `.devcouncil/` path.
- `userConfig` with `sensitive: true` is the declared way to take a token without it reaching
  a config file, which is what the repository's own no-secrets rule requires.
- Declared `dependencies` and lockfile-driven install (`bun.lock` / `package-lock.json`, no
  lifecycle scripts) is a supply-chain posture the current checkout distribution has no
  answer to at all.

**Recommendation:** target Claude Code's format, and treat Agent Plugins 1.0 as a portability
question to revisit rather than a second target to maintain. Deciding otherwise means
maintaining two manifests for one artifact.

### 9.3 The assert-as-guard fragility

Not a defect — stated precisely because it would be easy to overstate. `call_tool` gates
database-backed tools with a membership test against `_DB_REQUIRED_TOOLS`, then narrows the
type at each branch with `assert db is not None`. **The list is currently complete: 26 entries
against 24 assert-guarded branches, zero gaps** (the two extras use `db` outside an assert
branch). So there is no live bug.

The fragility is that nothing *enforces* that completeness. A tool added to a branch but not
to the list is caught only by the assert — and `assert` is removed under `python -O`, so the
same mistake is a clean rejection in development and an `AttributeError` from inside a handler
in an optimised deployment. There are 91 asserts under `src/devcouncil/`, 24 in this file
alone. This is Class A (§3.1) in its purest form: **a check that cannot run must not be the
thing correctness rests on.** The fix is to derive the list from the branches, or to fail
closed on an unlisted tool, rather than to add a 25th assert.

---

## 10. Worst-case scenarios

> Execution order and the consolidated open-work register live in
> [AGENT_PLAN.md](AGENT_PLAN.md#consolidated-open-work-register-2026-09-02). This section owns
> the scenarios; that register owns what is left to do about them.

Added 2026-09-02. §8 lists risks — things that may go wrong and cost schedule. This section
lists the scenarios that would make the port *worse than not doing it*, with the property that
would prevent each. They are chosen because the 2026-09-02 pass demonstrated the mechanism for
five of the seven, at smaller scale.

**W1 — The graph asserts code that does not exist, and an agent acts on it.**
Demonstrated: tier-2 recovery attributed 457 symbols to Markdown design documents (K3), Go and
TypeScript types that were *described* rather than defined. An agent asked to modify
`ReasoningBank` would have been sent to a design document. Worst case is this at cutover scale
with no prose guard and no provenance labelling, in a graph nobody yet distrusts.
*Prevented by:* Class C — provenance on every derived artifact, enforced rather than
conventional.

**W2 — Cutover silently drops a language, and nobody notices for months.**
Demonstrated: 19 `.proto` and 17 `.ps1` files were invisible to the graph while extraction
parity was green (K2), because discovery dropped them before the extractor could count them
missing. Worst case is the same failure spanning a language someone depends on, past the point
where the Python incumbent is still available for comparison.
*Prevented by:* Class B, now implemented — coverage stated over the filesystem, never over the
index's own inventory.

**W3 — A green gate certifies an unexamined system.**
Demonstrated twice. Search returned zero hits for a matching query with both budget gates green
(K4); the freelist gate read a pre-checkpoint counter that reported 0 free pages on a store
that was 33% garbage (K5). Worst case: the two-week shadow soak passes because its checks
degrade to no-ops under exactly the load it exists to test, and cutover proceeds on it.
*Prevented by:* Class A — an operation that declines to run must not return what success
returns. **No gate for this exists yet; it is the highest-value unbuilt item in this plan.**

**W4 — Deletion reconciliation is deferred and the graph accumulates ghosts.**
Not demonstrated; already stated in §8 as "B3 and N2 must ship together". The worst case is
sharper than slowness: a deleted file that stays in the graph makes `dead` and `trace` wrong in
the direction that *invents* work, and the failure grows monotonically with time since cutover.
*Prevented by:* shipping B3 and N2 as one work item, which §8 already requires and the phase
ledger does not yet enforce.

**W5 — A dependency range changes the wire contract without a signal.**
Not demonstrated here, but the exposure is live: `mcp>=2.0.0,<3` has already accepted the
2026-07-28 breaking revision (§9.1). Worst case is a transitive upgrade during cutover week,
diagnosed as a port regression because the port is what changed most visibly.
*Prevented by:* §9.1 action 1 — pin and assert the protocol revision, not just the SDK range.

**W6 — Two writers, one tree, and the evidence is not reproducible.**
Demonstrated during this very session: a concurrent session held the workspace, the suite
failed in `store_hardening` and passed in isolation seconds later, and one run failed to
*build* when shared `target/` artifacts vanished mid-run. Worst case at cutover is a parity
failure that cannot be reproduced, attributed to the port, and burns days.
*Prevented by:* parity and soak runs taking an exclusive checkout, and every recorded run
carrying the tree state it was taken against. Neither is required today.

**W7 — The measurement that justifies cutover is attributed to the wrong thing.**
Demonstrated: the phase profiler reported extraction's 7.25 s beside the word "resolving" and
extraction itself as 42 ns (K8), and a code comment inferred from that output asserted the
wrong phase was the build's largest — plausible, internally consistent, and wrong.
*Prevented by:* Class E — every measurement carries corpus, date and tier; every derived figure
states its derivation.

**The pattern across all seven** is that none is a crash. Each produces a system that runs,
answers, and reports success while being wrong — which is why §3.1's classes, not §3's
instance list, are what the remaining hardening budget should buy.

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

### Kernel measurements (§2.1) — 2026-09-02

Everything above profiles the Python incumbent. The kernel figures come from a staged
harness instead, because the stages have different fixes and a single wall-clock number
hides which one moved:

```bash
# Staged benchmark: cold / warm / touch / manifest / e2e, min-of-N, scratch store.
python benchmarks/map_bench.py --repo /path/to/any/tree --repeat 5
python benchmarks/map_bench.py --repo . --baseline benchmarks/results/map/<earlier>.json

# Per-phase breakdown of one build, straight from the kernel.
devmap --db /tmp/scratch.sqlite --progress never --json build /path/to/tree | jq .timings
```

Two properties of the harness are load-bearing rather than incidental:

- **Minimum of N, not mean.** The minimum is the run least contaminated by other load; a
  mean on a laptop substantially measures what else was scheduled.
- **A scratch store, always.** Every stage writes to a temporary database. The real
  `.devcouncil/` is never touched, so a benchmark cannot corrupt the working index and a
  stale working index cannot flatter a benchmark — which it did once during this pass, when
  a leftover store made a cold build appear *faster* than a warm one.

Results land in `benchmarks/results/map/<timestamp>.{json,md}`; `--baseline` diffs against
an earlier run. Corpus sizes are recorded in each JSON, so a figure can be checked against
the tree it came from rather than trusted.
