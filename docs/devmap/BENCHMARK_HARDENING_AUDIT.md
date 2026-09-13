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

Remaining work includes typed-receiver propagation, package-level function
variables/closures, platform-conditioned target selection, SQL dialect/client
include contracts, a PowerShell parser, anonymous-scope negative binding coverage, Go callback type annotations and
factory-result inference, and source-wide ground truth. Five
selected caller pairs or ten synthetic definition cases cannot close these
gaps. Evidence/confidence scoring and incomplete-answer disclosures remain
separate from the number of returned matches.

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

## Remaining acceptance gates

| Work still needed | Evidence required before claiming it closed |
|---|---|
| Binding coverage and private Rust values | A format-level distinction between unavailable facts and examined unbound sites; cold, cached and unchanged-upgrade cases for private file-level values and anonymous scopes. Existing public/export behavior must remain separate. |
| SQL and PowerShell | Dialect/source-checked fixtures and explicit parser/provider availability. SQL object references need a defined relationship to source-file imports; fallback extraction cannot be relabeled complete. No new parser dependency was authorized or added. |
| Latency optimization | Measure the remaining teardown owners and candidate extraction/resolution/persistence work, then prove invalidation and result equivalence before narrowing work. The current measurements do not prove removable work. |
| Broader comparison | Counterbalanced runs with more repetitions, matched actual provider capabilities and independent caller/definition ground truth. Expand persistent mutation convergence beyond the current separate DevMap soak. |
| Durability and portability | Controlled crash/power-loss scenarios, multiple filesystems and native operating-system runs. The local kill/restart test does not simulate physical power failure. |
