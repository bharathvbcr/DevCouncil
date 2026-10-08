# DevMap benchmark and hardening audit — 2026-09-12

This audit reproduces the one-file latency gap, explains the preserved store's
live footprint, fixes demonstrated retention and attribution defects, and
strengthens the benchmark and stress checks. **It does not establish a speedup,
complete attribution, language parity, or universal robustness.**

Work is isolated on `codex/devmap-evidence-hardening-20260912`, based on
`ee07c183b127e7291c38f993f4d71ca75f1121dc`. The canonical checkout at
`/Users/bharath/Code/devtools/DevCouncil` had concurrent uncommitted work and was
not modified. The supplied `/Users/bharath/Code/DevCouncil` path was absent.
Three agents independently investigated profiling, storage, and attribution;
the root lane audited the comparative harness and ran integration checks.

## Findings and implemented corrections

| Finding | Evidence and correction | Scope |
|---|---|---|
| The update gap is reproducible | Seven alternating-order, exact-symbol-checked edits: clean DevMap median 719.3 ms, CodeGraph 314.4 ms. Native phase medians identify substantial extraction, resolution and persistence work. Added nested spans to the existing progress owner, including failed operations and skipped-work controls. | Observability; no algorithmic speedup claimed. |
| The cold footprint is predominantly live data | 138.844 MiB, zero free pages, one generation, no stale cache/retry rows. Extraction payloads 74.109 MiB plus attribution rows 45.391 MiB account for about 86%. Full vacuum on a copy recovered 1.125 MiB. | A representation cost, not proof of unnecessary records. |
| Unique renames leaked interned paths | An unmodified store retained 64 paths after 64 generations although only two generations remained. The existing prune transaction now removes paths only after every persisted reference disappears. | Preserves retention, edge endpoints, pinned snapshots and rollback. |
| Builtin/prelude names could hide explicit imports or generic bindings | Six call/type regression cases reproduced incorrect explanations. Binding origin now precedes builtin/prelude classification; Rust module-path classification requires the language and a real namespace-qualified form. | Conservative classification; no guessed edges or higher confidence. |
| Unchanged stores could preserve old classifications | A real CLI fixture seeded the previous cache identity and incorrect ledger. Schema identities 49→50→51 force refresh; separate unchanged-store fixtures retain both historical upgrades and compare with a cold build. | Store schema remains 20; no database-format migration. |
| Captured values could borrow a global, import or another parameter's type | Positive use-site binding facts now reach call and member-reference classification. One receiver-type origin helper preserves the declaring scope and prevents sibling type borrowing; imported/Swift typed controls remain. Missing facts retain conservative fallback evidence. | No extraction representation change or guessed dispatch edges. |
| Coverage wording overstated what classification explains | Query warnings now enumerate excluded classes and say those classifications do not prove complete source coverage. | Warnings and partial/fallback outcomes remain visible. |
| Soak queries could pass without observing an edit | The old helper accepted eleven adversarial receipt cases; short edit/restore timing could outrun watcher debounce. Both interfaces now require the exact name/file, verified removal, and complete available receipts. Daemon convergence is bounded. | Forty standalone and forty daemon cycles qualified the main change; a later availability check received its own regression and two-cycle live smoke. |
| Comparative evidence could accept invalid outputs or lose provenance | The new runner qualifies exact definition projections, preserves failures/raw responses, verifies source and executable hashes, separates startup, and bounds processes and MCP frames. Reused utility fixes validate version execution and SHA-256 identity. | Synthetic definitions are a limited comparison contract; native capabilities differ. |

Detailed evidence, source references and retained pre-fix failures are in
[UPDATE_PROFILE_AUDIT.md](UPDATE_PROFILE_AUDIT.md),
[STORE_FOOTPRINT_AUDIT.md](STORE_FOOTPRINT_AUDIT.md), and
[ATTRIBUTION_AUDIT.md](ATTRIBUTION_AUDIT.md).

## Latency interpretation

**Verified:** the preserved original raw output already contained phase timings.
The earlier report overlooked those records. The original medians were
scan/extract 199.4 ms, resolve 220.8 ms, analysis 68.9 ms and persistence 255.4 ms.
They support phase-level triage but do not establish which work is removable.

The new clean baseline used direct process timing on 1,186 source files with
before/after hash equality. Its 719.3/314.4-ms medians use a different measurement
protocol from the old 817.4/376.5-ms figures and must not be merged with them.
The frozen v50 candidate's seven-edit median was **802.8 ms**, with native
total 743.4 ms: cached extraction 110.2 ms, global resolution 217.0 ms, analysis
66.9 ms, and persistence 259.5 ms. Nested spans are already included in parent
spans; separate medians are not additive.

**Inferred:** cached extraction, repository-wide resolution and persistence are
the main optimization candidates in this workload. Writer waiting and affected
write-set construction are small. Narrowing global resolution requires a proved
dependency/invalidation contract; removing attribution data changes the query
contract. Neither was done speculatively.

An independently measured 51.7-ms interval follows JSON emission. Source ordering
rules out writer-lock release and repeated progress completion as its cause.
Build-owned collections and the SQLite connection remain to be destroyed;
their individual cleanup costs are **unverified**. Timing those owners is the
next focused probe before considering a cleanup optimization.

**Superseded 2026-10-07:** the owners were timed and the interval removed; see
"Build-cost follow-up (2026-10-07)" below.

## Source attribution and extractor limits

**Verified:** the historical ledger contains 162,590 physical entries, of which
91,784 remained after explanatory classifications. Thirteen additional
source-checked samples and adversarial language fixtures exposed real
misclassifications. They are exploratory selections, not a random sample or a
full recall estimate. The corrected v50 candidate leaves **92,904** unresolved sites
on the same restored historical corpus. The increase reflects more conservative
classification; it is not a demonstrated loss of correct caller edges.

All eight SQL files have partial parse outcomes, with 23 recorded error ranges.
A source script containing `CHECK(json_valid(body))` executes successfully in
SQLite while the parser reports an error, establishing a real grammar gap.
SQL object references are not automatically source-file imports. One PowerShell
file still uses a pattern fallback because no linked grammar exists. Those
warnings remain. No dependency was added.

**Reproduced 2026-10-08** (`rust/devmap-extract/tests/sql_check_constraint_parse.rs`):
the 23 error ranges are exactly 13 column `CHECK`s whose expression calls a
function (`CHECK(json_valid(body))`; `CHECK(revision>0)` parses cleanly) plus
10 SQLite-only statements the linked grammar does not know — trigger bodies
(`BEGIN … END`, `RAISE(ABORT, …)`) and `CREATE VIRTUAL TABLE … USING fts5(…)`.
A function-call `CHECK` is confined: the error is the `CHECK(...)` text and no
declaration is lost. The SQLite-dialect ranges are not: of the 25 tables and
views the files write, `work_items_fts` (the FTS5 table) and `work_events` (the
table written after a trigger the parser could not close) are not extracted.
Indexes and triggers are not symbols in this extractor at all. The test pins
both counts and both lost names, so a grammar change in either direction fails
it. Repairing either gap is a grammar change; no parser dependency is
authorised, so they stay recorded here.

**SQL imports — decision 2026-10-08: `sql` stays `blind_to: ["imports"]`.**
`devmap status` already states it per file (`import_blind`, reason "`sql` has
no import extractor in this build"), and the resolution rate lists `imports` in
the language's `blind_to`. A defined object-reference ↔ file relationship would
need a table → declaring-file index and an extractor for the references a
query makes (`FROM`, `JOIN`, `INSERT INTO`, `REFERENCES`, trigger targets);
the files that matter here are loaded by Rust through `include_str!`, which no
SQL-side import could express either. Not built.

**PowerShell — decision 2026-10-08: pattern recovery stays, labelled as such.**
A PowerShell grammar is a new dependency and needs the owner's approval; none
was given. `scripts/install.ps1` is listed under `pattern_recovered` in
`devmap status` ("no linked tree-sitter grammar for powershell; 2
declaration(s) recovered by pattern"), its extraction carries
`ParseOutcome::Fallback`, and DIVERGENCES.md X34/X37 record the tier. A grammar
would buy calls and imports for `.ps1`; one installer script does not justify
the dependency today.

Remaining work includes typed-receiver propagation, package-level function
variables/closures, platform-conditioned target selection, SQL dialect/client
include contracts, a PowerShell parser, anonymous-scope negative binding coverage, Go callback type annotations and
factory-result inference, and source-wide ground truth. Five
selected caller pairs or ten synthetic definition cases cannot close these
gaps. Evidence/confidence scoring and incomplete-answer disclosures remain
separate from the number of returned matches.

Package-level function variables, closures and platform-conditioned targets
were reproduced on 2026-10-08 with one fixture each
(`rust/devmap-resolve/tests/function_values_and_platform_targets.rs`); what is
resolved and what is a known limit is recorded in DIVERGENCES.md, "Known
limits: function values and platform targets".

### JavaScript resolution: classified, two classes fixed (2026-10-08)

**Why the 1,705 JavaScript sites went unresolved** (generation 4144, this
repository; 16 JS files: scripts, test probes and fixtures):

| Class | Sites | Cause |
|---|---|---|
| `uninferred_receiver` | 837 | 827 name a member no JS/TS file declares — `includes` 77, `push` 67, `length` 51, `slice` 34, … (476 calls, 351 property reads); 10 have a JS/TS namesake (`seen.add`, `child.stdin.write`, …) |
| `external` | 616 | `node:` imports and `node:test` / `node:assert` (`it`, `equal`, `join` on an imported `path`) |
| `host_global` | 198 | `process.*`, `console.*`, `JSON.*` |
| `builtin` | 39 | language builtins |
| `no_namesake` | 10 | all 10 are `node:fs` / `node:child_process` / `node:url` functions bound by a destructured CommonJS `require` the extractor did not read |
| `local_binding` | 3 | `resolve` of an enclosing `new Promise((resolve) => …)` |
| `unresolved` | 2 | `resolve` from a nested callback; `original` in a probe |

Net was `resolved / (resolved + unresolved − explained)` = 196 / 1,038 = 188‰:
the 827 namesake-free members sat in the denominator although no edge could
ever bind them. Two classes were fixable, and both are fixed with tests that
fail on the old kernel:

1. **A member no symbol of the family declares** is now `no_namesake`
   (`a_receiver_member_nothing_declares.rs`), the evidence the bare-name ladder
   already used. Classification only: no edge moves.
2. **CommonJS `require`** binds what it declares (`commonjs_require_bindings.rs`):
   `const { a, b: c } = require(m)` and `const x = require(m)`. On a local module
   this adds edges; on this repository every `require` names a Node builtin, so
   it moved 30 rows to `external` and added no edge.

**Before / after, `devmap build --full` + `devmap status --json`**, on frozen
snapshots (`git archive`): DevCouncil at 58bcc215, MarkDev at 20dec7a. Before
is the main build c3907784, after is 58bcc215; each ran on its own copy.

| Corpus / language | gross ‰ before → after | net ‰ before → after | resolved | unresolved | explained before → after |
|---|---|---|---|---|---|
| DevCouncil total | 198 → 198 | 308 → 517 | 40,731 | 163,981 | 72,642 → 125,983 |
| javascript | 103 → 103 | 188 → 928 | 196 | 1,705 | 863 → 1,690 |
| rust | 202 → 202 | 283 → 476 | 31,420 | 124,002 | 44,715 → 89,521 |
| go | 189 → 189 | 460 → 699 | 7,100 | 30,307 | 21,998 → 27,261 |
| python | 203 → 203 | 383 → 859 | 1,657 | 6,499 | 3,831 → 6,228 |
| c | 287 → 287 | 490 → 557 | 184 | 455 | 264 → 309 |
| cpp | 93 → 93 | 478 → 550 | 11 | 107 | 95 → 98 |
| shell / sql / html | unchanged | 655 / 0 / 1000, unchanged | | | |
| MarkDev total | 285 → 285 | 510 → 770 | 21,453 | 53,724 | 33,183 → 47,324 |
| swift | 313 → 313 | 607 → 772 | 17,905 | 39,145 | 27,598 → 33,864 |
| rust | 188 → 188 | 267 → 687 | 2,335 | 10,028 | 3,633 → 8,969 |
| python | 198 → 198 | 292 → 949 | 1,067 | 4,311 | 1,731 → 4,254 |
| javascript | 0 → 0 | 0 → no rate | 0 | 22 | 6 → 22 |

**Read the gross column.** Edges are identical on both corpora (57,505 and
35,498), gross is unchanged in every language, and the dead lists are
identical row for row (27 and 5). Every net gain is the reclassification: a
site that could never bind leaves the denominator. MarkDev's 22 JavaScript
sites now all have no namesake, so its net rate has no denominator — the same
shape `cfml_app` has in `resolution_baseline.json`. Labelled precision is
unchanged: `labelled_corpus_precision.rs` reports precision 1.000 on every
fixture with claims and dead recall 10 / 10. The residual JavaScript
`uninferred_receiver` rows (10) are the real type-inference limit.

## Clean release provenance

The clean baseline binary is retained under `.devcouncil/audit/baseline/`, built
with `cargo build --release --locked -p devmap-cli` from `ee07c183b127`.
SHA-256: `cce843d25f48323c90bf92079ee2b7be79af5b1bd5a63c4661acc5aa53c8bddd`.

The comparative campaign and its v50 profile use a separately committed, clean
source snapshot `eed3872f0b8e750aae5e0cf9c6df65610b80d1a5` in an ignored local
clone. Binary SHA-256:
`afff0036ffa5e1d281e9d6852a743533b03dc4b303545532e1485f602f443e3e`.
Its exact patch from the base, compiler/Cargo versions, lockfile hash, Git status
and build command are preserved in `.devcouncil/audit/candidate-final-source.*`
and `.devcouncil/audit/candidate-final/`. The patch SHA-256 is
`2040fec439deece540595c2b8b28682f73f2c1c0217b1630be43f3412eb55428`.
The later qualified snapshot `659ed7d4e1fd` retained identical production Rust
source. Subsequent captured-binding classification and v51 cache invalidation
change production code; their final verification is recorded separately below.
The earlier comparison is not attributed to the newer executable.

These are reproducible source/build inputs and preserved exact executables.
Bit-identical rebuilds were not established: build metadata includes time.
The old dirty binary's missing source patch remains unrecovered. This new
provenance does not retroactively qualify the old executable.

## Expanded comparison contract

`benchmarks/competition_bench.py` creates separate clones at explicit commits:
DevCouncil `ee07c183b127`, Manvi `d8acc2e0186c`, and MarkDev `1e1cb66fa4d8`.
It checks all tracked regular-file hashes across tools and after execution;
the corpus accounting explicitly excludes symlinks. Ten additional fixtures
cover Python, Go, Rust, JavaScript, TypeScript, C, C++, Java, Ruby and Swift.
Each native response is preserved; the common projection is exact definition
name and repository-relative path. It does not equate native node/edge counts.

Per tool/repository, the runner records one cold invocation, three unchanged
builds, ten language searches plus three repeated Python searches through both
standalone and persistent MCP, and three repetitions each of edit, rename,
delete and empty-file restoration. Every mutation starts from a checked state,
and the update must succeed before its result can qualify. Unknown, unavailable,
capped or malformed results cannot establish absence. Invocation success and
semantic success are carried independently; failed cells remain in denominators.

MCP startup is separate from established-session query latency. Comparative
mutation timings use each tool's ordinary standalone refresh command; this is
not a comparison of persistent watcher convergence. DevMap's watcher behavior
is separately covered by the daemon soak. Timing native refresh invocations
also does not mean they perform equivalent amounts of internal work.

CodeGraph 1.6.0 and CBM 0.10.8 use their installed, inspected command contracts.
No hosted credentials are passed and no providers are installed; isolated cache
and configuration directories are used. **Provider parity remains unverified:**
the tools' bundled local extractors and language-server availability differ.
Graphify, GitNexus and Gortex are not rerun in this expanded campaign. Their old
figures remain historical observations, including the separately inspected
Graphify store.

Raw campaign artifacts live in `.devcouncil/audit/competition-final/`, with
configuration, executable identities, per-clone source manifests, MCP schemas,
native outputs, measurements, all checks and descriptive summaries. Frozen
runner sources and their hashes are retained in
`.devcouncil/audit/competition-runner/` and its adjacent manifest. The rejected
pilot remains in `competition-pilot/`: its DevMap MCP path mismatch was corrected
before the final campaign and its failed cells are not qualified evidence.

### Completed comparison

**Verified:** all nine repository/tool combinations completed: **777 recorded
operations, zero failed invocations, 342/342 semantic checks passed**, and nine
original-source preservation checks passed. The 342 checks comprise 180 language
queries, 54 repeated Python queries, and 108 mutation outcomes. An independent
post-run check also verified every original and fixture file: 1,196 files per
DevCouncil clone, 750 per Manvi clone, and 323 per MarkDev clone, with no mismatch.

Executable hashes were unchanged. Because CodeGraph's executable is a launcher,
the full underlying bundle was independently checked: **940 files totaling
289,548,902 bytes** had identical hashes before and after. No preserved stdout,
stderr or MCP frame log exceeded 16 MiB; the largest was 58,897 bytes. These
checks are retained in `.devcouncil/audit/competition-independent-verification.json`
and `codegraph-bundle-{before,after}.json`.

The following are median milliseconds, **three observations per cell**. The
query columns use the same Python definition-search name/path contract.
Established-session MCP timing excludes separately recorded startup.

| Repository | Tool | Edit | Rename | Delete | Restore | CLI query | MCP query |
|---|---|---:|---:|---:|---:|---:|---:|
| DevCouncil | DevMap | 757.37 | 1009.62 | 1034.27 | 828.29 | 12.34 | 0.95 |
| DevCouncil | CodeGraph | 342.55 | 333.87 | 250.70 | 347.65 | 113.87 | 0.70 |
| DevCouncil | CBM | 8301.07 | 8304.84 | 6728.01 | 7921.25 | 4146.46 | 15.98 |
| Manvi | DevMap | 453.41 | 561.34 | 565.53 | 466.79 | 11.99 | 0.82 |
| Manvi | CodeGraph | 315.45 | 310.38 | 248.04 | 314.31 | 107.55 | 0.46 |
| Manvi | CBM | 7024.98 | 7147.57 | 6361.02 | 7375.63 | 4026.31 | 14.20 |
| MarkDev | DevMap | 438.67 | 622.07 | 655.05 | 524.28 | 14.35 | 1.32 |
| MarkDev | CodeGraph | 368.24 | 371.09 | 265.69 | 368.58 | 130.09 | 0.43 |
| MarkDev | CBM | 6998.28 | 6661.28 | 6185.76 | 6689.17 | 4447.69 | 14.90 |

**Verified within this protocol:** CodeGraph's edit median is lower in all three
repositories, and persistent queries substantially reduce process overhead.
**Unverified:** a general product ranking or statistically significant difference
between sub-millisecond queries. Tool order is fixed, not counterbalanced; CBM's
documented `full` mode includes its own LSP/similarity/semantic work. These
observations do not equate provider work or supersede each tool's coverage limits.

After the campaign, four adversarial failures reproduced gaps in output-budget
handling: fast oversized stdout, fast oversized stderr, a stream of individually
small MCP notifications, and oversized MCP stderr before an otherwise successful
response. The corrected runner checks final subprocess output sizes, aggregate
MCP request/session bytes and stderr while waiting. Logs can overshoot between
polls, but oversized output cannot qualify as success. The campaign used the
preserved earlier runner; its independently verified small outputs are unaffected.

## Final v51 release qualification

**Verified:** the delivered executable comes from clean local commit
`962ea5f07921ea925e3b70821c5487472c8b57ea`, with the exact patch from the original
base preserved as `.devcouncil/audit/delivery-source.patch` (SHA-256
`5498fe82a51870f8ef319e5a7861631d8e942ccf6fe81bf6e6706dc79e070ee6`).
The executable `.devcouncil/audit/delivery-release/devmap` has SHA-256
`d6628af04241e3e12853ce279ed1ba60481364e8ca8140ec8cd12ec59cba65db`.
It was built with `cargo build --release --locked --offline -p devmap-cli`;
the storm integration target was then compiled with `--no-run` before recording
the binary hash. Full commands, clean status, compiler/Cargo versions, lockfile
hash and native version are in `delivery-release/provenance.json`. Both the
workspace and frozen snapshot contain the same delivered code/test bytes.
The resolver's separate `--no-default-features --all-targets` run passed
**21 tests**, with no failures or ignored tests.

The final seven-edit profile, run after compilation and before other stress
work, measured **804.0 ms** median process time (771.0–865.6 ms), **745.6 ms**
native total, scan/extract 206.3 ms, resolution 214.6 ms, analysis 73.0 ms and
persistence 244.2 ms. Relevant nested medians were extraction 113.6 ms, resolver
index 40.1 ms, unresolved-row writes 119.4 ms, generation pruning 40.1 ms and
vacuum 8.5 ms. The post-JSON interval was 51.4 ms. These are seven observations;
nested phases overlap their parents and separate medians do not add up.
This final-only follow-up was not interleaved with CodeGraph or the earlier
DevMap build and establishes no speedup or regression against either.
Raw records and source hashes are in `update-profile/delivery-v51/`.

A fresh DevMap-only campaign then repeated the same three repository pins,
ten language fixtures, query interfaces and edit scenarios with the final
runner and executable. **261 operations completed, zero failed invocations,
114/114 semantic checks passed**, and all three original-source preservation
checks passed. Independent post-run checks verified all original and fixture
bytes: 1,196 DevCouncil files, 750 Manvi files and 323 MarkDev files, with no
mismatch. The 114 checks comprise 60 language queries, 18 repeated queries and
36 mutation outcomes. Earlier competitor measurements remain pinned to their
original campaign; they were not rerun or folded into these measurements.

| Final DevMap corpus | Edit | Rename | Delete | Restore | CLI query | MCP query |
|---|---:|---:|---:|---:|---:|---:|
| DevCouncil | 778.57 | 1013.16 | 1069.68 | 846.48 | 12.92 | 1.07 |
| Manvi | 463.99 | 590.33 | 591.76 | 452.59 | 13.42 | 1.11 |
| MarkDev | 432.28 | 569.36 | 561.87 | 478.02 | 13.60 | 0.95 |

Values are median milliseconds, three observations per cell; established MCP
queries exclude startup. Full outputs, limits and exact source checks are in
`.devcouncil/audit/delivery-campaign/`; the runner source hashes and wrapper
that limits this follow-up to DevMap are in `delivery-runner/`.

On the restored historical corpus, final v51 retains **162,590 physical ledger
entries**, with **96,233 remaining after explanatory classifications**. The
same build reports 10,193 nodes, 33,789 edges, zero confidence mismatches and a
fresh, query-ready store. The higher unresolved count preserves uncertainty;
it does not establish full recall or adjudicate every changed classification.
The historical 91,784 and intermediate v50 92,904 counts remain distinct.
The first ad hoc final tally, 95,215, used an incorrect exclusion set and was
withdrawn: native policy excludes no-namesake sites and retains local-binding
sites. The corrected 96,233 comes directly from the native warning and its
66,357 explained-site count; the correction is retained in the evidence JSON.

An independent comparison confirmed identical values for all 10,193 nodes and
33,789 edges between the v50 and v51 restored corpora, including edge confidence,
resolution and candidate counts. Every ledger identity/reason also matched;
3,329 classifications changed from external to uninferred receiver. Exact
source checks of the first Go cases found absent binding-specific type facts;
previous external explanations borrowed sibling annotations. Explicitly typed
controls kept their external origin. These source-selected cases do not
adjudicate all 3,329 transitions. This checks
preservation of graph evidence, not correctness of every explanation. Exact
hashes and the first eight changed sites (an explicitly limited selection) are
in `update-profile/delivery-v51/graph-and-classification-comparison.json`.

## Verification

**Verified:** `bash rust/verify.sh` completed successfully. Formatting and strict
workspace Clippy passed; the workspace test run reported **3,058 passed, zero
failed, three ignored across 279 result groups**. Its subsequent release worker
probe deliberately panicked, then reported `worker panic returned; cleanup ran;
subsequent worker completed`. The panic is an intentional negative control,
not an unreported failed verification step.

| Required gate | Observed result |
|---|---|
| Determinism | Two independent cold graphs had identical digests. |
| Self-build time | 2,364 ms, below the existing 10-second gate. |
| Cold store budget | 140 MiB for 868 indexed files, below its 149-MiB gate. |
| Peak memory | 620 MiB, below both the absolute 2-GiB and derived 759-MiB budgets. |
| Ambiguity-memory probe | 758,784 milli-bytes/candidate below 1,200,000; above-cap memory delta 88% of at-cap, below 200%. |
| Growth | Two retained generations; fifth store size 1,638,400 bytes; plateau and size gates passed. |
| Incremental equivalence | Five restore cycles passed; this short gate explicitly does not assert growth. |

The final run completed in 547.5 seconds and is preserved under
`.devcouncil/audit/delivery-verification/`, including raw stdout/stderr, command
result, and independently summed test counts. Its ambiguity probe used 5,000
synthetic sites with 16 and 64 candidates; it explicitly did not run a production
corpus. The earlier 3,041-test qualification remains in its original log.

The verifier's final line is `ALL GATES GREEN`, but its optional broad mutation
sweep is skipped by default and is not counted as passed. Scoped mutation
evidence is recorded separately. Of the three default ignored tests, the real
`claude plugin validate --strict --json` integration was subsequently invoked
explicitly and passed. `concurrent_prune_writer_child` is a subprocess helper
invoked by the passing cross-process retention test. The long storm test is
also invoked separately, with its own scope and outcomes below.

The benchmark's **24 Python tests passed**, including actual failed processes,
timeouts, orphan cleanup, invalid result schemas, hidden/unavailable results,
source changes, error-frame preservation, and output-budget failures. A fresh
MCP search through the final reader passed against DevMap, CodeGraph and CBM.
The soak's **10 receipt tests and eight real-process shutdown tests passed**.
The final helper also passed a two-cycle real daemon integration check, with
unchanged source hashes, exact insertion/removal, availability/freshness,
restored digest and a graceful zero exit. Its peak-memory receipt was retained.
The earlier 40+40-cycle endurance evidence and its precise harness revisions
remain in the profile report; these short follow-ups do not invent a new
plateau measurement.

The final no-default-features store check passed, including all available
targets. The new retention test is correctly gated on `parse`; no production
dependency or feature default was changed.

Earlier integration failures were retained and corrected, not omitted:

- Clippy: `this if let can be collapsed into the outer if let` in the new type
  branch. The pattern was narrowed directly, preserving behavior.
- Test classification: `tests/retention_churn.rs is neither declared
  required-features = ["parse"] ... nor listed in FEATURE_OFF_SAFE`. The test
  requires parsing and now declares that requirement.
- Exact cache-version assertion: `left: "50"`, `right: "49"`. At that stage both current
  identity assertions were advanced to 50. The final contract is 51, with both
  historical v49 and v50 upgrade fixtures retained. The final workspace run
  passed afterward.

Logs are under `.devcouncil/audit/full-verify{,-final,-qualified,-complete}.log`,
with `full-workspace-test-counts.json`, `store-feature-off.log`,
`claude-strict-validation.log`, `benchmark-bounds-{prefix,final}.log`,
`final-mcp-bounds-smoke/`, and the source-qualified `update-soak/` artifacts.
The product Rust changes add **224 lines and remove 143**; most other additions
are evidence, benchmark orchestration and executable regression cases.

## Adversarial and stress scope

**Verified:** a scoped mutation run changed only `Resolver::classify_unresolved`.
All 33 planned mutations completed: 28 were caught, four survived, and one did
not compile because `UnresolvedClass` has no `Default` implementation. The
unviable mutation was not runtime-tested. Source regressions were then added
for all four survivors; replay caught all four, with no timeout or unexecuted
case. The original outcome and the replay remain separate in
`.devcouncil/audit/attribution-mutations/summary.json`; this is not a claim of
whole-resolver mutation coverage. The new cases distinguish Python imports
from Swift SDK imports, typed Swift prelude values from local types, captured
TypeScript values from globals, and Rust `self` values from module paths.

An additional source probe exposed a remaining extraction gap: a private Rust
file-level `static std: usize` can be classified as an external namespace when
used as `std.count_ones()`. The extractor omits private file-level bindings and
the call representation collapses dot and namespace forms. Correcting that
requires an explicit file-scope binding/namespace contract; assigning a guessed
binding or weakening private-export tests would hide the gap. This remains
unresolved and limits attribution claims.

A second conservative limitation remains: an anonymous callback parameter can
leave a scope-level shadowing fact that hides a later use of the real global.
An absent precise binding row cannot distinguish an examined unbound site from
an unavailable fact in older or constructed extraction payloads. The correction
uses positive binding evidence; it does not turn absent evidence into proof of
a global. Resolving that remaining false gap requires a binding-coverage
contract, and the source probe is retained.

The first explicit 12-cycle daemon storm passed, but inspection found that its
fixture silently ignored filesystem errors and renamed entries while walking a
live directory iterator. The 10,000-file rename actually renamed 5,077 entries
once, 4,707 twice and 216 three times. Four fixture regressions failed before
the correction and passed afterward. The fixture now checks every storm
filesystem operation and snapshots entries before renaming; eight successive
shapes verify the exact intended file sets. The original successful run remains
evidence for its actual workload, not the corrected workload.

**Verified on the final clean v51 executable:** the corrected storm completed
all **12 cycles** in 156.62 seconds (157.13 seconds including the recorded
invocation). Every cycle had a positive RSS sample and quiesced queue. After a
three-cycle warm-up, RSS half-means were 125,904 and 101,104 KiB, a 19.7% decrease.
These are samples across restarted processes and different storm shapes, not
proof of a continuously running daemon's memory plateau. Final node/edge counts
and complete symbol membership matched a cold build; endpoint cleanup passed.
The final queue observation reported 1,602 nodes and 1,101 edges. Source commit,
fixture, lockfile and release hashes stayed unchanged. Exact rows and guards
are in `.devcouncil/audit/storm12-qualified/` and the store report.

The storm permits forced termination and does not verify every signal receipt;
it cannot establish graceful shutdown at every cycle. The separate final
strict-harness two-cycle daemon smoke passed with exact insertion/removal,
availability/freshness, restored digest, graceful exit zero, unchanged fixture
and input hashes, and a retained 30,621,696-byte peak-RSS receipt. It ran the
same clean release; its older IPC probe is separately identified by hash.
Artifacts are in `.devcouncil/audit/delivery-daemon-smoke/`. This is a smoke test,
not another 40-cycle endurance or plateau measurement.

The soak shutdown checks use real subprocesses to verify startup failure,
reparented daemons, bounded forced cleanup and unrelated-process preservation.
The helper observes both its launcher and owned process group, including the
interval before session creation. A daemon forced to exit or lacking an exit
receipt cannot qualify as a successful soak. No authorization or provider
access was widened.

## Final worktree state

The required manifest build completed in the isolated worktree. Its final
status reports generation 2, 10,333 nodes, 34,229 edges, zero confidence mismatches,
zero pending items, fresh source/analyzer state and query readiness. These are
worktree counts, distinct from the pinned historical comparison corpus. Raw
build/status output and snapshot verification are in
`.devcouncil/audit/delivery-worktree/`.

All delivered code and test bytes match clean snapshot `962ea5f07921`; final
`git diff --check` passed. Changes remain uncommitted in this isolated worktree
and were not merged into the concurrently edited canonical checkout. The clean
release snapshots are local commits in the ignored audit clone, with exact
patches and executables retained. No new dependency or provider was installed.

## Interpretation boundaries

The audit is local to this macOS host, pinned repositories, installed tool
versions and bounded fixtures. Three repetitions and one cold sample per cell
do not support population-wide rankings. Background machine activity and OS
caches are not controlled; there are no confidence intervals. Definition
presence, mutation visibility, source attribution and complete caller recall
are different questions. Every comparison should retain those distinctions.

The demonstrated fixes have adversarial regressions and concrete local gates.
Physical power-loss durability, other operating systems/filesystems, indefinitely
stalled readers, hosted-provider behavior and every language construct remain
unverified. Passing a finite stress test cannot establish complete confidence
in all future workloads.

## Build-cost follow-up (2026-10-07)

Task `ft-fb65f8b052aa02ed607803b7c81c3302`. Every number below was taken on a
macOS arm64 host (18 cores) that other sessions were loading: the 1-minute load
average is recorded per sample and ranged 18–45. Every comparison is therefore
interleaved, counterbalanced, and reported as n/min/median/max; a difference
inside the median spread is reported as no difference. Raw records, binary
hashes and patches are in
`benchmarks/results/competition/20261007-build-cost/`.

### Per-edit latency against CodeGraph (reproduced)

`benchmarks/competition_bench.py --mode edit-latency --repeat 18`. The corpus was
DevCouncil at `c3907784`, with a fresh clone per arm. DevMap ran at HEAD
(`43d89cb1…`) and CodeGraph at the pinned v1.6.0. A third arm, DevMap B, carried
the teardown change below. Rounds follow a rotated Latin square, so each arm
held each position six times, and the executed order equalled the plan in all
18 rounds. A sample is timed only when the post-edit search found the edit, and
all 108 were.

| Arm | Edit min / median / max (ms) | Restore min / median / max (ms) |
|---|---:|---:|
| DevMap HEAD | 1282 / 1419 / 4701 | 1217 / 1448 / 3928 |
| DevMap B (teardown change) | 1044 / 1413 / 2505 | 1088 / 1434 / 2694 |
| CodeGraph v1.6.0 | 623 / 786 / 1715 | 644 / 759 / 2203 |

The gap holds. A DevMap edit costs about 1.8x a CodeGraph sync at the median,
in line with the earlier 2.3x (802.8 vs ~340 ms), which was measured on a quiet
host and must not be merged with these numbers. On whole-edit time,
DevMap B is indistinguishable from HEAD at the median. The focused A/B below is
what resolves the teardown.

The caller check in the same run used the seeded ground truth
(`benchmarks/ground_truth.py`): 48 cross-file caller-to-callee pairs across
Python, Go, Rust and TypeScript, written by the generator rather than any tool.
DevMap, DevMap B and CodeGraph each scored 48/48. Names in that fixture are
unique, so the set proves the scorer works. It does not yet distinguish one
tool from another.

### The interval after the JSON line (attributed and removed)

A probe patch (`patches/teardown-probe.patch`, never committed) timestamped
each owner's drop after the JSON line, over nine one-file edits on a frozen
DevCouncil tree. Min and median of the steps:

| Owner | min | median |
|---|---:|---:|
| `ResolutionResult` | 48 ms | 149 ms |
| `Vec<Extraction>` | 14 ms | 45 ms |
| `Resolver` (symbol/type indexes) | 12 ms | 37 ms |
| `Store` (SQLite close, WAL checkpoint) | 1 ms | 15 ms |
| runtime shutdown + process exit | 14 ms | 17 ms |
| `AnalysisSummary`, discovery report, manifest, progress, presentation | <0.5 ms each | |

The interval was deallocation: freeing heap graphs one allocation at a time
just before exit. The four heap owners hold no file, lock, thread or effectful
`Drop`, so `devmap build` now forgets them once its output is written, and the
OS reclaims them at exit (`rust/devmap-cli/src/main.rs`, end of the build arm).
The store still drops normally, because closing it releases its locks and
checkpoints the WAL.

`benchmarks/teardown_ab.py` ran 20 ABBA rounds of one-file incremental builds
with both binaries on one store, timing the JSON line to the process exit:

| Binary | JSON line → exit, min / median / max (ms) | Whole build, min / median (ms) |
|---|---:|---:|
| HEAD | 75.6 / 85.7 / 220.0 | 1193 / 1417 |
| B (teardown change) | 11.9 / 15.1 / 44.7 | 1188 / 1466 |

Teardown no longer bounds the interval. The 12–15 ms that remain are the store
close plus runtime shutdown and exit, and they are treated as irreducible here.
Whole-build time does not move outside the noise. The saving is about 70 ms for
a caller that waits on the exit. A caller that reads the line saves nothing.

### Cold-build cost of `idx_unresolved_rows_callee`

`Store::open` re-creates every declared index on each writable open, so a
dropped index cannot survive into a CLI build. The B binary is therefore HEAD
with the index removed from its schema (`patches/no-callee-index.patch`).
`benchmarks/cold_build_ab.py` ran 12 ABBA rounds of `devmap build --full` into a
new store. Both arms built identical counts (1,310 files, 15,885 symbols,
57,193 edges, 163,126 unresolved calls), and after every build the index was
checked present in A and absent in B.

| | with index | without |
|---|---:|---:|
| `persist:write` unresolved part, min / median | 378 / 505 ms | 263 / 445 ms |
| `persist:write`, min / median | 906 / 1353 ms | 814 / 1238 ms |
| whole cold build, min / median | 2.89 / 4.94 s | 2.93 / 4.78 s |
| store | 151,650,304 B | 148,815,872 B |

The index costs about 60–115 ms of a cold persist and 2.83 MB of store. That
is inside whole-build noise, against 5.0 ms saved per `impact` call at p50. The
same figures are recorded beside the index in `schema.rs` (`MIGRATION_V23_TO_V24`).

### B3

Recorded in `DIVERGENCES.md` (B3) rather than here. The range and payload
relations are differential, at 5 rows for a one-file edit at any corpus size.
The `generation_id`-keyed relations are re-written in full: 6n + 1 rows on the
fixture, and 40,156 for DevCouncil's generation 4131.

## Remaining acceptance gates

| Work still needed | Evidence required before claiming it closed |
|---|---|
| Binding coverage and private Rust values | A format-level distinction between unavailable facts and examined unbound sites; cold, cached and unchanged-upgrade cases for private file-level values and anonymous scopes. Existing public/export behavior must remain separate. |
| SQL and PowerShell | Dialect/source-checked fixtures and explicit parser/provider availability. SQL object references need a defined relationship to source-file imports; fallback extraction cannot be relabeled complete. No new parser dependency was authorized or added. **2026-10-08:** the `CHECK(json_valid(body))` gap and the SQLite-dialect gap are reproduced and pinned (`sql_check_constraint_parse.rs`); `sql` stays explicitly import-blind and PowerShell stays pattern-recovered, both by recorded decision (see "Source attribution and extractor limits"). |
| Latency optimization | **Teardown closed 2026-10-07** (below). Still open: candidate extraction/resolution/persistence work. Nothing was narrowed, so no invalidation contract or equivalence test was owed; any narrowing still needs both, on DevCouncil and MarkDev. |
| Broader comparison | **Partly closed 2026-10-07** (below): counterbalanced, interleaved per-edit rounds with recorded order, position and load, plus a generator-derived caller ground truth. Still open: a ground truth that discriminates between tools (the seeded set has unique names and every arm scores 48/48), more than one edit shape, CBM (no verified interface), and mutation convergence beyond the DevMap soak. |
| Durability and portability | **Partly closed 2026-10-07** — see "Crash consistency" below: process death mid-persist and a lost un-fsynced WAL tail are tested on macOS/APFS. Still open: torn or reordered writes below the filesystem, physical power loss, other filesystems and native runs on other operating systems. |

### Crash consistency (2026-10-07)

`rust/devmap-cli/tests/crash_consistency.rs` holds two tests.

- `a_build_killed_mid_persist_leaves_a_whole_generation` builds a 1,200-file
  corpus, rewrites every file, starts `devmap build`, and sends `SIGKILL` 0, 2,
  5, 10, 20, 40, 80 or 160 ms after the build prints `[4/5] persisting`. Every
  round checks five things. The child died by the signal. `Store::open`
  succeeds and `PRAGMA integrity_check` is `ok`. The latest generation is
  either the previous one or the new one, and it holds all 1,200 source files.
  The next build succeeds without operator action. After the last round, the
  recovered store's nodes and edges equal a cold `--full` build of the same
  tree. On the debug build all eight kills landed before the commit; the
  previous generation survived intact each time.
- `a_lost_wal_tail_loses_whole_generations_never_half_of_one` simulates losing
  the WAL tail that `synchronous = NORMAL` does not fsync per commit. Generation
  1 is checkpointed. Generation 2 lives only in the WAL: 771,208 bytes in 47
  frames of 16 KiB. The test copies the main file and every WAL prefix. The cuts
  are no WAL, the header alone, each of the 47 frame boundaries, and a cut
  through the middle of the committing frame, 50 in all. Each copy opens and
  passes `integrity_check`. A prefix that includes the commit frame recovers
  generation 2. Any shorter prefix recovers generation 1, which still holds
  every file. Only the final frame commits, so a generation is one transaction.
  Measured: 49 cuts recovered generation 1 and 1 recovered generation 2.

The assertions were checked against deliberate breaks, each made and then
reverted. Flipping one payload byte of a committed WAL frame left the full
prefix at generation 1, and the test failed. Halving the main database file
failed `Store::open` with `database disk image is malformed` in both tests.

**Not proven:**
- Whether a kill landed inside the write transaction or in the persist
  stage's setup before it. Both leave the previous generation, and both are
  recorded as "before the commit".
- Torn or partially written pages in the main database file.
- Writes reordered by the device or the filesystem.
- A lying disk cache.
- A crash during a checkpoint, which writes the main file.

Those need fault injection below the filesystem, such as a VFS shim or a
block-device simulator. This suite has neither.

| Platform | Status |
|---|---|
| macOS 27.0.1 arm64, APFS (`diskutil info /`) | run |
| Linux ext4 / xfs / btrfs | not run |
| Windows NTFS | not run |
| Network filesystems (NFS, SMB) | not run |
| Physical power loss | not run |
