# devmap Hardening — Review of Landed Work

Review date: 2026-09-06
Reviewed against: `DEVMAP_HARDENING.md`
State at review: `CURRENT_SCHEMA_VERSION = 17`, `EXTRACTION_SCHEMA_VERSION = "33"`

Static review only — the workspace sandbox was unavailable, so nothing below was executed.
Every claim is sourced to a file and line. Findings that depend on runtime behavior are marked.

> **Historical paths (2026-10-07).** The Python tree this review cites (`src/devcouncil/…`,
> `graph_cmd.py`, `map.py`, `codeintel.py`, `indexing/graph/schema.py`, `liveness_ratchet.py`,
> `dead_symbols.py`, `DevMapClient`) was deleted with Phase 7 (`3286db5e`, 2026-09-10); no `src/`
> tree remains, so every Python surface named below is a record of what existed at review time,
> not a live consumer. `rust-port/crates/` became `rust/` in `ee595607` (2026-09-11), so a cited
> `db.rs`/`liveness.rs` path now lives under `rust/<crate>/src/`, and the line numbers below are
> the 2026-09-06 ones. `benchmarks/map_bench.py` does still exist.

---

## Plan status

**Status as of 2026-10-07** (code at `3658417c`): the table below is the dated record of
2026-09-06 and is left as written. Closed rows are marked individually below. Of the 19 rows
re-checked against source in this pass (R1–R7, R10–R13, R15, M1–M4, M6, M7, M9): **16 closed,
3 partly closed (R10, M3, M4), 0 open.** R8, R9, R14, M5 and M8 were not re-checked here and carry
no mark. Commits naming them exist (`1680505b`, `8239876a`, `c42faed4`), but this pass did not
verify them. Rows were checked by reading source and tests; no test was executed in this pass.

| Item | State | Note |
|---|---|---|
| W0.1 capability registry | **landed, exceeded** | 4-bit `Capabilities` bitset + 37-file probe corpus with bidirectional verification. Better than specified. |
| W0.2 charge call/import blindness | **landed, altered** | `CallBlind` / `ImportBlind` charged. The flat cap was replaced with a graded ceiling that was not in the plan — see R3. |
| W0.3 unwired import-blindness | **landed, exceeded** | Gate added *and* the underlying gap closed: 25 `imports.push` sites (was 6), `#include` handled. 245 → 120 candidates; `.rs` 119 → 2. |
| W0.4 unresolved-namesake veto | **landed** | `unresolved_namesake_names`, `liveness.rs:856`. Filter is correct. |
| W1.1 cyclic dead clusters | **landed, orphaned** | Tarjan verified correct. No consumer — see R10. |
| W1.2 heritage → Extends/Implements | **landed** | 17 grammars; `resolver.rs:2305` converts to edges. |
| W1.3 re-export chains | **landed** | R-9 correctly rescoped rather than deleted. |
| W2.1 resolution rate | **landed, exceeded** | Integer permille, gross/net split, per-language, site-dedup. |
| W2.2 epistemic field | **landed** | `with_epistemic`, `_GAP_PROSE` with permanence wording. Tool descriptions updated. |
| W2.3 `min_rung` | **partial** | Kernel only (deps/impact/trace). Not on `dead`, no MCP/Python surface — R11. |
| W2.4 empty fields | **landed** | `runtime_proven_live` retired; `unreachable_files` now fed by clusters. |
| W3.1 liveness ratchet | **landed** | Rebased on `DevMapClient.manifest()`; `LIVENESS_SCHEMA_VERSION = 2`. |
| W3.2 `dead_symbols.py` | **landed** | Fail-open closed; `unknown` distinct from `unreached`. |
| W3.3 wiring dedup | not verified | Out of scope for this pass. |
| W4.1–W4.3 measurement | not started | Still the highest-leverage unclaimed item. |

---

## R1 — Migration from any store at v5–v14 fails hard

**Severity: blocker.** Existing installations cannot open their store.

**Closed** `65ad91b3` (2026-09-06). The v14 and v15 rungs no longer call `validate_schema`
(`rust/devmap-store/src/db.rs:3037-3063`); only the end-of-chain call remains (`:3234`). Pinned by
`rust/devmap-store/tests/migration_ladder.rs:462` `every_rung_from_five_up_migrates_to_the_current_schema`,
which opens a seeded store reduced to every rung from 5 up.

`db.rs:1715-1727` — the `version == 14` block stamps `PRAGMA user_version = 15` and then calls
`Self::validate_schema(&tx)?`. `validate_schema` asserts the **current** schema, and
`REQUIRED_SCHEMA` demands `generation_edges.candidate_total` (`db.rs:1020`) — a column that does
not exist until `MIGRATION_V15_TO_V16`. Any store stamped 5 through 14 fails `Store::open` with
`required column generation_edges.candidate_total is missing`.

Stores at ≤4 survive only incidentally: the `version == 3` and `version == 4` blocks re-run
`CREATE_SCHEMA_V3` (`db.rs:1559`, `:1567`), which creates the table at current shape when absent.

The ladder documents the opposite rule in nine places — "No mid-chain validation: `validate_schema`
asserts the *current* schema" (`db.rs:1599-1601`, `:1615`, `:1629`, `:1642`, `:1657`, `:1668`,
`:1679`, `:1698`, `:1710`). The v15 and v16 blocks violate it. Only the v17 call is in the right
place.

**Fix.** Delete the `validate_schema` calls at `db.rs:1724` and `:1735`. Keep `:1757`.

**Test gap.** Nothing migrates a real v6–v14 store. `test_s2_migration_v3_to_v4`
(`tests/store_hardening.rs:413`) builds a v3 store with no `generation_edges` at all; the v4→v5 case
(`:447`) likewise; v5→v6 (`:479`) reconstructs from a *current* store; and
`a_v16_store_migrates_to_the_payload_split_and_keeps_its_rows` (`:2507`) starts at 16 and skips the
broken block. Add a test that stamps a realistic store back to 14 and opens it. The v17 doc comment
(`schema.rs:632-636`) already names this blindness class — "No test migrated a real v16 store, so
1,842 of them passed over it" — and it has now produced a second instance.

---

## R2 — `MIGRATION_V16_TO_V17` drops an index without recreating its replacement

**Severity: blocker (silent performance).**

**Closed** `65ad91b3` (2026-09-06). `MIGRATION_V16_TO_V17` now creates
`idx_file_payloads_cache_identity` (`rust/devmap-store/src/schema.rs:673-674`), and
`validate_schema` now checks every index parsed from the DDL (`db.rs:2691`). Pinned by
`migration_ladder.rs:580` `a_migrated_store_carries_the_same_schema_as_a_fresh_one` and `:504`
`a_store_missing_a_declared_index_two_rungs_down_heals_at_open`.

The migration runs `DROP INDEX IF EXISTS idx_generation_files_cache_identity` (`schema.rs:684`) but
creates only two of the three payload indexes that `CREATE_SCHEMA_V3` has. Missing:

```
idx_file_payloads_cache_identity ON file_payloads(content_hash, language, grammar_version, analyzer_version)
```

(`schema.rs:73-74`), whose own comment says the extraction-cache fallback "asks by content identity
alone… so it needs its own index". The query is `db.rs:5636-5648`. The surviving unique index
(`idx_file_payloads_identity`) leads with `file_id` and cannot serve it, so every migrated store
full-scans the payload table on each cache miss — the exact cost v13 was introduced to remove.

Fresh stores are fine. Only migrated ones are affected, and `validate_schema` checks columns, never
indexes, so nothing reports it.

**Fix.** Add the `CREATE INDEX IF NOT EXISTS` to `MIGRATION_V16_TO_V17`. Consider extending
`validate_schema` to cover indexes named in `CREATE_SCHEMA_V3`.

---

## R3 — The graded coverage cap re-elevates the defect it was built to prevent

**Severity: high, design.**

**Closed** `65ad91b3` (2026-09-06), by a variant of option 3 below rather than by options 1 or 2.
Weighting was rejected because a discovery-refused file has no `Extraction` to weigh. A degraded
corpus is instead charged at least `MIN_CHARGED_BLIND_SHARE = 0.05`
(`rust/devmap-analyze/src/liveness.rs:940`, rationale `:891-939`), so Q-1 now scores 0.663 (mid-`inferred`)
instead of 0.89. `HIGHEST_DEGRADED_CONFIDENCE` is still 0.89 (`:949`). Pinned by
`tests/coverage_cap_at_repository_scale.rs:130` `a_lost_caller_in_a_large_corpus_is_graded_not_nearly_confident`
and `:169`.

`liveness.rs:484-498`:

```rust
pub fn cap(&self, confidence: f32) -> f32 {
    if self.is_complete() { return confidence; }
    let Some(blind_share) = self.blind_share() else {
        return confidence.min(COVERAGE_LOSS_CONFIDENCE_CAP);
    };
    let ceiling = (1.0 - blind_share)
        .powi(COVERAGE_CEILING_EXPONENT)
        .clamp(COVERAGE_LOSS_CONFIDENCE_CAP, HIGHEST_DEGRADED_CONFIDENCE);
    confidence.min(ceiling)
}
```

with `COVERAGE_CEILING_EXPONENT = 8` (`:577`), `HIGHEST_DEGRADED_CONFIDENCE = 0.89` (`:586`).

**What is right.** The absolute rule is enforced structurally, not asserted: the clamp's upper bound
is 0.89, the combinator is `min`, and `confidence_millis` renders 0.89 as 890 — below
`EXTRACTED_FLOOR_MILLIS` (900, `code_graph.rs:57`). No degraded scan can reach `extracted` on any
path. The doc comment's reasoning about 0.8995 rounding up is correct and worth keeping.

**What is wrong.** Q-1 — `lib.py::helper` whose only caller sits in a file over `MAX_SOURCE_BYTES`
(`extraction_coverage_liveness.rs:9-13`) — previously scored 0.35 / `ambiguous`. Run the same shape
in a 1,000-file repository: `blind_share = 0.001`, `0.999^8 = 0.992`, clamped to **0.89** →
`inferred`. The kernel now publishes, one tier below the act-on threshold, a claim that is
*literally false*: the caller exists and was never read. The safety margin fell from 0.55 to a
single tier boundary plus the documentation convention in `CLAUDE.md:16`.

**The denominator measures the wrong thing.** `blind_share = blind / (blind + files_with_call_extraction)`
(`:431-441`) is a ratio over **file counts**. But discovery refuses files precisely for exceeding
`MAX_SOURCE_BYTES` (`db.rs:249`), and parse failures cluster on the largest files. The class of file
that goes blind is systematically the class holding the most call sites. One 4 MB generated client
in a 1,000-file repo is 0.1% by file count and plausibly 15% of the corpus's call edges; the formula
prices it at 0.1%. Nothing weights by bytes, symbols, or call-site count.
`the_ceiling_depends_on_the_ratio_and_not_on_the_corpus_size` pins the ratio property but not the
right ratio.

**Options.**
1. Weight `blind_share` by extracted symbol count or file bytes rather than file count.
2. Keep the curve but lower `HIGHEST_DEGRADED_CONFIDENCE` so a degraded scan tops out inside
   `ambiguous` rather than at the top of `inferred`.
3. Accept it explicitly — record in the doc comment that Q-1 now returns `inferred` and that the
   guarantee is "never `extracted`", not "never confident".

Whichever is chosen, R6 must be fixed alongside, or no test can observe the decision.

---

## R4 — A wholly call-blind file receives the corpus-wide ceiling

**Severity: high.**

**Closed** `65ad91b3` (2026-09-06), by the second fix below. `cap_call_blind_file[_for]`
(`rust/devmap-analyze/src/liveness.rs:757-765`) applies `.min(COVERAGE_LOSS_CONFIDENCE_CAP)`, and
the cascade uses it for symbols in call-blind files (`:1817`). Pinned by
`tests/coverage_cap_at_repository_scale.rs:316`
`a_symbol_in_a_call_blind_file_is_capped_by_its_own_file_not_the_corpus` (99 readable files and 1 `.tf` file).

`file_is_call_blind` (`liveness.rs:1147`) does not exempt the file. Contrast `is_parse_failed`
(`:1134`), which wholesale-exempts `Failed | Fallback | Skipped`. A call-blind file's symbols fall
through to cascade branch 3 and receive `coverage.cap(0.9)` computed from the **corpus-wide** ratio.
For that file the correct local blind share is 1.0.

In a 99-Python / 1-Terraform corpus, a `.tf` symbol publishes at 0.89 / `inferred` carrying
`CALL_BLIND_REASON` — "this language has no call extractor in this build" priced as a 1% risk.

This does not bite today only by accident: the two linked call-blind grammars are `hcl` (marks every
symbol `is_exported`, `treesitter.rs:3362`) and `cfml` (produces no declarations, pinned at
`extraction_coverage_liveness.rs:359`); `cobol` and `vb` are unlinked. That is an unstated
cross-crate dependency of exactly the kind `dead_clusters.rs:136-138` calls out elsewhere.

**Fix.** Either exempt call-blind files the way parse-failed files are exempted, or apply
`.min(COVERAGE_LOSS_CONFIDENCE_CAP)` to symbols whose own file is blind, regardless of the corpus
ratio. Blindness that is a permanent property of the language should not be priced as a transient
corpus hole — the two already have distinct reason strings, and the confidence should follow.

---

## R5 — Cluster exemptions do not propagate; clusters can propose deleting public API

**Severity: high.**

**Closed** `65ad91b3` (2026-09-06). The exemptions are hoisted into `symbol_exemption_index`
(`rust/devmap-analyze/src/liveness.rs:1512`), and `externally_reachable_symbols` seeds from
`liveness::exempt_symbol_names` (`dead_clusters.rs:364-368`). Pinned by
`tests/exemptions_reach_both_passes.rs:267` `no_exempt_symbol_becomes_a_dead_cluster_by_calling_its_neighbour`,
`:345`, and `:379`, which checks that the exemption is file-scoped.

`externally_reachable_symbols` (`dead_clusters.rs:297-320`) seeds only from `symbol.is_exported` and
wiring annotations. It misses every exemption the single-symbol pass computes:

- `c_header_exports` (`liveness.rs:1302`)
- `go_interface_exemptions` (`:1291`)
- `heritage_exemption` (`:1281`)
- `exported_owners` / "Member of an exported type" (`:1325`)
- `go_build_variants` (`:1331`)

Concrete case: `__all__ += ["MyClass"]` with `MyClass.a()` and `MyClass.b()` mutually recursive. The
single-symbol pass exempts both as public API. The cluster pass reports the component at
`DEAD_CLUSTER_CONFIDENCE = 0.5` with the reason "reached by nothing outside the component." Same
shape for two mutually recursive C functions declared in a shared header.

**This is a live proposal to delete public API.** No test covers it —
`an_exported_member_keeps_the_cluster_alive` exercises only `is_exported`.

**Fix.** Hoist the exemption computation ahead of both passes and seed
`externally_reachable_symbols` from the same set. The single-symbol pass already computes everything
needed; it just runs after.

---

## R6 — The coverage regression suite is structurally blind to the new grading

**Severity: high (test integrity).**

**Closed** `65ad91b3` (2026-09-06). `rust/devmap-analyze/tests/coverage_cap_at_repository_scale.rs`
adds 100-file fixtures that assert the graded value, not the floor, for these cases:
- a lost caller (`:130`)
- a discovery-refused caller (`:169`)
- a pattern-recovered file (`:190`)
- a call-blind file (`:316`)
- clusters (`:432`, `:479`)

Not re-run in this pass. The claim that the fixtures fail against the pre-fix kernel is the
commit's, and was not reproduced here.

Every fixture in the pre-existing coverage suite is 2–5 files, so `blind_share` lands at 20–50% —
past the 12.3% crossover where the ceiling is already clamped to the floor. The tests therefore pass
for the *old* reason and cannot observe the curve:

- `a_call_blind_corpus_cannot_reach_the_confident_tier` (`extraction_coverage_liveness.rs:374`) uses
  a **pure** Terraform corpus (100% blind → floor). Add 99 readable files and the assertion
  `confidence <= COVERAGE_LOSS_CONFIDENCE_CAP` fails at 0.89.
- `a_lost_caller_is_not_confident_evidence_of_death` (`:137`) — two-file fixture, 50% blind.
- `a_pattern_recovered_file_is_lost_call_coverage…` (`:252`) — same.
- `coverage_loss_caps_a_cluster_finding` (`dead_clusters.rs:178`) — two-file fixture. In a 100-file
  corpus with one blind file, `cap_cluster(0.5, 3) = min(0.5, 0.89) = 0.5`, i.e. the cap does
  nothing. The test name claims degraded corpora cap cluster verdicts; the realistic case is
  uncapped.

The code acknowledges this at `liveness.rs:572-576`. **No test anywhere asserts what a lost-caller
finding scores in a large corpus** — which is the only case R3 is about.

**Fix.** Add a large-corpus fixture (≥100 files, 1 blind) to each of the four, asserting the actual
graded value rather than the floor.

---

## R7 — `a_cluster_ceiling_is_stricter_than_a_single_symbol_ceiling` contradicts its own doc

**Severity: medium (test integrity).**

**Closed** `65ad91b3` (2026-09-06). Both fixes apply. The doc now states that the two ceilings
coincide at both clamps (`rust/devmap-analyze/tests/coverage_cap_is_graded.rs:320-339`). The test
now asserts strict separation, `cluster < single - 1e-6`, at a 5%-blind share inside the band, and
keeps `<=` only as the weak half (`:341-393`).

Doc (`coverage_cap_is_graded.rs:305`): the two "must never coincide at the same blind share, or
`cap_cluster` is decorative." Assertion (`:315`): `cluster <= single`.

At `blindness(1, 199)`, `single = 0.89` and `cap_cluster(0.9, 2) = cap_cluster(0.9, 5) = 0.89` —
they **do** coincide, because both clamp at the same upper bound. The test passes on `<=` while the
stated property is violated for small clusters. Either assert strict inequality below the clamp, or
correct the doc to say the compounding only separates them above a blind share the clamp does not
mask.

---

## R8 — Dead clusters reach no consumer

**Severity: high (orphaned work).**

`dead_clusters.rs` is ~500 lines with 5 test files. Its output reaches exactly two artifacts:
`manifest.rs:512-513` and `code_graph.rs:915-916`. It reaches **no query path**:

- `StoreQueryEngine::dead_symbols` (`devmap-query/src/engine.rs:1163-1198`) returns
  `Response<DeadSymbolReport>` from `store.dead_page` only.
- Rust `devmap dead` (`main.rs:3324` → `emit_dead`) renders no clusters.
- IPC `Dead` (`protocol.rs:683`) carries none, so `DevMapClient.dead_symbols` cannot see them.
- The MCP envelope (`codeintel.py:783-812`) has no `dead_clusters` key.
- Python `dev graph dead` (`graph_cmd.py:1237-1263`) builds entries from `rust_dead["dead_code"]` —
  clusters never referenced.
- `code_graph.json` carries them, but `CodeGraph` (`indexing/graph/schema.py:87-94`) has no
  `dead_clusters` field, so pydantic drops them at load.

`grep dead_clusters src/devcouncil` returns zero hits. The only Python reader in the repo is
`benchmarks/map_bench.py:693`.

The highest-recall feature in this round is visible only to a benchmark script.

**Fix.** Add `dead_clusters` to the `Dead` IPC response and the MCP envelope, add the field to
`CodeGraph`, and render clusters in `dev graph dead` as one line per cluster with a member sample.
Update both tool descriptions — neither mentions clusters today, correctly, because neither tool
returns them.

---

## R9 — `min_rung` stops at the kernel

**Severity: medium.**

Present on **deps, impact, trace** only: CLI flags `main.rs:568,585,620`, parsed at `:3231,3256,3294`,
validated by `check_rung` (`:2517,2532`); IPC `protocol.rs:118,130,142` with `rung_floor` /
`parsed_min_rung` at `:292-304`.

Not on `dead` — `IpcCommand::Dead { budget }` (`protocol.rs:389,683`), `Commands::Dead { budget }`
(`main.rs:3324`). And no Python or MCP surface exposes it at all: zero hits in `src/devcouncil`.

W2.3's point was that agents should be able to ask for deterministic-only edges. They still cannot.

---

## R10 — Notebooks are declared capability-`NONE` while extracting calls and imports

**Severity: medium (live flag/behavior disagreement).**

**Partly closed.**
- **Fixed in `e4ca3def` (2026-09-06).** `Extraction::capabilities()` resolves a notebook from
  `ExtractionEngine::Notebook { kernel_language }` (`rust/devmap-extract/src/model.rs:1543-1604`).
  The four charge sites named below now ask the extraction rather than the language string:
  `liveness.rs:1305` (the gap charge), `:1377` (`files_with_call_extraction`), and
  `devmap-query/src/code_graph.rs:638` (`excluded_import_blind`). Pinned by
  `rust/devmap-analyze/tests/a_notebook_is_not_call_blind.rs:76`, `:113`, `:139` and `:167`.
- **Still open (inferred from source, not executed).** The per-language coverage join that
  landed later (`8f2e7225`, 2026-09-11) brings back the string form. `extraction_coverage` keys
  `blind_by_language` / `covered_by_language` by `ext.language` (`liveness.rs:1361`, `:1382`),
  which is `"notebook"`. `language_can_reference` then asks
  `capabilities_for_language("notebook")` (`:816`), which is `NONE`, and `language_family`
  (`:826`) has no notebook arm. So a notebook's coverage and holes never reach Python
  findings. No test covers a broken notebook beside a Python symbol it calls.

`NON_REGISTRY_CAPABILITIES` lists `("notebook", Capabilities::NONE)` with the comment "its
capabilities are that grammar's, resolved per file rather than declared here"
(`languages.rs:672-674`). **Nothing resolves it per file.** `capabilities_for_language` takes only a
`&str`, and `Extraction::language` for an `.ipynb` stays `"notebook"` (`notebook.rs:140`) — the
kernel name goes into `ExtractionEngine::Notebook { kernel_language }`, never into `language`.

But `notebook.rs:372` sets `extraction.imports` and `:388` sets `extraction.calls`, and
`grammar_read_this_file()` returns **true** for notebooks (`model.rs:1076-1084`).

Consequences for every clean `.ipynb`: charged both `CallBlind` and `ImportBlind` with the false
reason "`notebook` has no call extractor in this build" (`liveness.rs:952-966`); dropped from
`files_with_call_extraction` (`:1003`); every symbol becomes `file_is_call_blind` (`:1147`); and
`unwired_candidates` increments `excluded_import_blind` (`code_graph.rs:506`).

Invisible to CI because there is no `.ipynb` probe and `every_registry_language_has_a_probe`
iterates `LANGUAGE_SPECS` only — the eleven `NON_REGISTRY_CAPABILITIES` rows other than `shell` and
`sql` are unverified.

**Fix.** Either resolve notebook capability from `ExtractionEngine::Notebook { kernel_language }` at
the charge site, or extend the probe corpus to `NON_REGISTRY_CAPABILITIES` and let the existing
bidirectional test catch it.

---

## R11 — Provenance claims in the new code repeat the defect it replaced

**Severity: medium (documentation integrity).**

**Closed.** Each half was fixed in its own commit:
- **Derivation: `1680505b` (2026-09-06).** `extracts_imports` asks the dispatcher's `match` arms
  (`rust/devmap-extract/src/langimports/mod.rs:98-140`). The const's doc now says it is
  hand-written and checked against the dispatcher (`:142-157`).
- **Filename: `0c718b11` (2026-09-06).** Both provenance lines (`mod.rs:27`, `:64`) cite
  `tests/language_capabilities.rs`. `rg -uu language_import_capabilities rust/` returns nothing.

Pinned by `rust/devmap-extract/tests/language_capabilities.rs:441`
`the_import_dispatcher_and_the_registry_agree`.

Two instances, both in `langimports/mod.rs`:

- **Lines 24-25 and 57-61** state that the declines and the bidirectional agreement are "pinned by
  `language_import_capabilities.rs`". That file does not exist. The assertions actually live in
  `tests/language_capabilities.rs`. This is precisely what the new code condemns
  `CALL_EXTRACTION_LANGUAGES` for — claiming a reader that did not exist — reintroduced in the
  documentation of its replacement.
- **Lines 98-107** say `IMPORT_EXTRACTION_LANGUAGES` is "derived from the dispatcher above by the
  test that reads it, never hand-maintained." It is a hand-written `const &[&str]`, and its only
  reader (`language_capabilities.rs:303-358`) compares it against `LANGUAGE_SPECS` — one hand-written
  list against another. Nothing parses the `match lang` arms. A dispatcher arm added for a grammar
  with no registry row, or removed while the const stays, is caught only incidentally by the probe
  corpus.

**Fix.** Correct the test filename. Either downgrade the second claim to what it does, or make it
true by deriving the list from the dispatcher.

---

## R12 — Probe-corpus verification proves less than it appears to

**Severity: medium.**

**Closed** `e4ca3def` (2026-09-06).
- **Escape 1.** Every `NON_REGISTRY_CAPABILITIES` row now has a probe
  (`rust/devmap-extract/tests/language_capabilities.rs:265` `every_declared_language_has_a_probe`,
  `rust/testdata/capabilities/probe.ipynb` among them).
- **Escape 2.** The Svelte, Vue and Astro probes declare `class Widget extends BaseWidget` inside
  their script blocks. Liquid's observable-but-clear Heritage bit is a recorded decision in
  `UNDER_CLAIMED` (`:140`), observed through `probe_heritage.liquid`. A stale entry fails
  (`:224-250`).

`tests/language_capabilities.rs:102-135` is genuinely good: it compares declared against observed in
both directions, and `every_registry_language_has_a_probe` (`:143`) closes the vacuity hole. Two
escapes remain:

1. **Coverage is `LANGUAGE_SPECS`-only** — see R10.
2. **A cleared bit is a property of the probe, not the extractor.** `probe.liquid` has no class, so
   no Heritage is observed and Liquid's `HERITAGE` bit is clear. But Liquid embeds `javascript`,
   `embedded.rs:799-802` absorbs inner references, and `heritage.rs:215-217` handles JS
   `class_declaration`. A `.liquid` file containing `<script>class A extends B {}</script>` produces
   Heritage the registry says is impossible. The test proves `declared == observed-on-this-corpus`,
   not `declared == what the extractor does`.

**Fix.** Make probes for the four template languages exercise their embedded script paths.

---

## R13 — `Capability::Heritage` and `Capability::References` have no production readers

**Severity: low.**

**Closed** `e4ca3def` (2026-09-06), by publishing the two bits rather than wiring them into
override reasoning, which they still do not affect. `LanguageResolution::blind_to` lists every
clear bit (`rust/devmap-analyze/src/resolution_rate.rs:286-294`), and `devmap build` prints it
(`rust/devmap-cli/src/main.rs:8665-8683`). Pinned by `rust/devmap-analyze/tests/resolution_rate.rs:285`
`every_capability_bit_reaches_the_per_language_readout`, which iterates over `Capability::ALL`.

Grep across all crates: only `Calls` (liveness ×4, `resolution_rate.rs:241`) and `Imports`
(`code_graph.rs:506`, `liveness.rs:960`) are consulted. Two of the four bits are declared,
maintained, and tested, and read by nobody — half the registry sits in the state the old constant was
condemned for, minus the wrongness. Either wire them (a heritage-blind language should arguably
affect override reasoning) or note in the doc comment that they exist for completeness.

---

## R14 — The CLI under-reports unwired exclusions threefold

**Severity: low.**

`src/devcouncil/cli/commands/map.py:133-159` reads only `excluded_coverage_loss` and prints
"N files excluded from unwired: extraction coverage loss." On this repository that is 5, while
`excluded_import_blind` is 15 (`STATUS.md:5035-5036`). The field the v33 work created is the one the
human-facing output ignores.

---

## R15 — Schema gate omits v17's new base tables

**Severity: low.**

**Closed** `65ad91b3` (2026-09-06). `REQUIRED_SCHEMA` (`rust/devmap-store/src/db.rs:2074`) now
names `file_payloads` (`:2133`) and `generation_file_rows` (`:2147`). The other direction of
the drift test now iterates `sqlite_master`, not the list, so a missing table fails:
`db.rs:9895` `the_schema_gate_names_every_relation_the_current_schema_creates`.

`REQUIRED_SCHEMA` (`db.rs:971-1093`) has no entry for `file_payloads` or `generation_file_rows`.
They are validated only transitively through the `generation_files` view. The S-8 drift test
(`db.rs:6190-6216`) iterates `REQUIRED_SCHEMA` and checks that every *actual* column is *required* —
so a new **table** missing from the gate is invisible to it by construction.

---

## Minor observations

| # | Observation | Location |
|---|---|---|
| M1 | `not_parsed_files` is in `blind_files()` but not `is_complete()`, so 500 vendored bundles alone leave the corpus "complete", and adding one `.proto` suddenly makes it ~50% blind. It is also absent from `degraded_reason()`, so the printed reason will size the hole differently from what `cap` charged. **Closed** `65ad91b3` (2026-09-06). `not_parsed_files` was removed from the cap's numerator, not added to `is_complete()`. `is_complete()` is now `blind_files() == 0` (`rust/devmap-analyze/src/liveness.rs:464`), `blind_files` leaves out `not_parsed_files` (`:534-570`), and `degraded_reason` names the same four counters (`:502`). Pinned by `tests/extraction_coverage_liveness.rs:576` `a_skipped_file_is_counted_apart_and_does_not_degrade_the_corpus`. | `liveness.rs:348`, `:368`, `:385` |
| M2 | The ladder flattens for `blind_share ∈ [10.8%, 12.3%]`: branch 1/2 (`cap(0.4)`) and branch 3 (`cap(0.9)`) return identical numbers — the collapse the grading exists to undo, relocated to a narrow window. `the_cap_is_monotone_at_every_blind_share` checks non-decreasing, not separation. **Closed as accepted** `65ad91b3` (2026-09-06). The window is bounded and pinned, not removed, because removing it would mean replacing `min` with a multiplicative degrade. Pinned by `rust/devmap-analyze/tests/coverage_cap_is_graded.rs:414` `the_ladder_flattens_only_inside_a_narrow_stated_window`, which requires the window to start at about 10.8% and to be contiguous. | `liveness.rs:484` |
| M3 | `lib.rs:109-117` caps `cluster.confidence` but leaves `cluster.reason` saying "reached by nothing outside the component" with no coverage caveat. The single-symbol path swaps in `COVERAGE_LOSS_REASON` for exactly this. **Closed in code, not pinned.** Since `65ad91b3` (2026-09-06), a degraded corpus appends a coverage caveat to `cluster.reason` (`rust/devmap-analyze/src/lib.rs:126-138`). No test asserts the caveat: rg for its text over `devmap-analyze/tests`, `devmap-query` and `devmap-cli` finds only the source. **Remaining:** a test. | `lib.rs:109` |
| M4 | `unwired_candidates` skips edges from `TestFile` sources but not `Vendored` or `GeneratedFile`, so a vendored blob importing a module marks it wired. Pre-existing; the widened `is_wiring_evidence` enlarges the surface. **Closed in code, not pinned.** Since `1680505b` (2026-09-06), `non_authoring_importers` covers `TestFile`, `Vendored` and `GeneratedFile` sources (`rust/devmap-query/src/code_graph.rs:495-506`, applied at `:532`). No test was found in which a vendored or generated importer leaves a module unwired; the commit added no such test, and rg over `devmap-query/tests` and `devmap-cli/tests` found none. **Remaining:** a test. | `code_graph.rs:413` |
| M5 | `only_source_files_are_charged_as_import_blind` asserts a bare `excluded == 2` on a five-file fixture — it verifies a count, not that the two files are the C# and Swift ones its message names. | `unwired_excludes_prose_on_its_own_grounds.rs:152` |
| M6 | `candidate_total` is written (`db.rs:2919`, `:2947`) and read only by `tools/fanout.sql` / `.sh`. Defensible for an offline metric, but no compiled code fails if the write regresses. **Closed** `7dfa20f4` (2026-09-06). A compiled test now fails if the write regresses: `rust/devmap-cli/tests/test_fanout_metric.rs:139` builds a real store, runs `rust/tools/fanout.sql`, and asserts `candidates == 13` (the sum of `candidate_total`, `:236`) and `pre_v16 == 0` (`:240`). The write has one owner, `rust/devmap-store/src/edge_index.rs:112` `ambiguous_candidate_total`. | — |
| M7 | `MIGRATION_V16_TO_V17` creates the view without `IF NOT EXISTS` while `CREATE_SCHEMA_V3` uses it. Harmless given the `already_split` probe, but asymmetric. **Closed** `65ad91b3` (2026-09-06). The view is now `CREATE VIEW IF NOT EXISTS generation_files` (`rust/devmap-store/src/schema.rs:711`). | `schema.rs:687`, `:93` |
| M8 | `liveness_ratchet.py` comments instruct bumping `wiring.LIVENESS_SCAN_VERSION` in step with `LIVENESS_SCHEMA_VERSION`. Nothing tests the coupling; worth confirming it moved. | `liveness_ratchet.py:27` |
| M9 | `DEAD_CLUSTER_MAX_NODES` is checked as `>= MAX` before push, so the real limit is exactly 400,000. Benign. `cap_cluster(x, 0)` treats a zero-member cluster as one member. **Closed** `1680505b` (2026-09-06). Both bounds are now stated: "exactly this many, inclusive" (`rust/devmap-analyze/src/dead_clusters.rs:153-167`), and the clamp's lower bound of 1 (`liveness.rs:786-794`). They are pinned by `tests/coverage_cap_is_graded.rs:488` `the_cluster_ceiling_is_total_over_the_sizes_its_type_permits` and `tests/graded_cap_under_hostile_input.rs:256` `an_oversized_graph_refuses_out_loud` (MAX+1). | `dead_clusters.rs:340` |

Checked and cleared: `#[serde(default)]` coverage on `AnalysisSummary` is complete (six fields), so
old generations deserialize; Python consumers tolerate the new payload keys (pydantic ignores
extras, `codeintel.py:763` spreads `**item`); `WiringKind::TargetRoot` is deliberately excluded from
`is_file_exempt`, pinned by `cargo_target_roots_are_wired.rs:87-103`; `is_entry_root` has a single
owner; `capabilities` is invisible to the frozen-registry parity test by construction
(`test_phase2_hardening.rs:21-28` deserializes five named fields); NaN cannot reach `.clamp`, and
`graded_cap_under_hostile_input.rs:88` asserts the fail-closed behavior rather than relying on it.

---

## What is strong

The Tarjan implementation (`dead_clusters.rs:228-288`) is correct against the textbook form,
including self-loops and the 400k refusal, with an explicit `call_stack` rather than recursion.

The capability registry's **bidirectional** verification is the part that makes it durable — a
missing flag and a missing extractor both fail, which is what the old constant lacked and why it
rotted. The `every_registry_language_has_a_probe` vacuity guard is the right instinct.

`resolution_rate.rs` is careful in four separate ways: integer permille rather than float, `Option`
for "never attempted" so a missing extractor is distinguishable from silence, site-counting via
`Arc::as_ptr` dedup so ambiguous fan-outs don't inflate the denominator, and `LocalBinding`
deliberately *kept* in the denominator while `Builtin`/`HostGlobal`/`External` are excluded.

The v32 and v33 cache rationale paragraphs state exactly what a stale row would resurrect —
"answers 'nothing imports this file' with the full confidence of a fresh extraction… it would look
examined and be blind." That convention is worth defending.

`dead_symbols.py` emitting a `quality_gate_failed` gap instead of `return []`, and keeping `unknown`
distinct from `unreached`, closes the fail-open hole properly rather than papering it.

W0.3 landed as a real fix rather than a gate: 245 → 120 unwired candidates, `.rs` from 119 → 2, with
19 new import extractors and `#include` handled.

---

## Suggested order

1. **R1, R2** — migration blockers. Both are small diffs; R1 is three deleted lines. Ship first and
   alone.
2. **R6** — fix the tests before touching R3, so the decision is observable.
3. **R3, R4** — the graded cap. R4 is the clear bug; R3 is a judgment call that should be recorded
   in the doc comment whichever way it goes.
4. **R5** — hoist exemptions ahead of both passes.
5. **R8** — surface clusters, or the recall work stays invisible.
6. **R10, R12** — extend the probe corpus; both are fixed by the same change.
7. **R9, R11, R13, R14, R15** and the minor list.
8. **W4.1** — the labelled corpus. Still unstarted, still the only unclaimed position in the field,
   and R6 shows why: without ground truth, a test suite can pass for the wrong reason through an
   entire design change.
