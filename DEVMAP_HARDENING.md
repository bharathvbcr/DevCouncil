# devmap Hardening Plan

17 work orders across 5 phases. Every item below is an edit point, not a direction.

The kernel's resolution discipline is the strongest of the four tools surveyed (devmap, gortex,
GitNexus, trace-mcp) — and it is undermined by three places where the analysis layer reads a
structural blind spot as evidence.

---

## Correction to the earlier comparison

An earlier note claimed 12 of 35 languages emit zero calls. That figure came from
`rust-port/STATUS.md:111-119` and is **stale**. The real count is **4**: VB.NET, CFML, COBOL,
Terraform/HCL.

- Svelte, Vue, Astro, Liquid were fixed by `embedded::merge_embedded_scripts` in
  `EXTRACTION_SCHEMA_VERSION` 31.
- Pascal, Erlang, Nix, Solidity gained `langcalls` modules.

Two of the remaining four are already caught: COBOL is refused by `UNSAFE_GRAMMARS` and VB.NET
falls to `scan_declarations`, so both land in `ParseOutcome::Failed` or `Fallback` and get charged
to coverage. **Only CFML and HCL parse `Clean` with zero calls and escape the cap.**

The real breakage is one layer over. **Imports have almost no coverage at all** — 24 of 35
languages emit zero import edges, including every C-family language (`#include` has no handler
anywhere) and all of Java, C#, Ruby, PHP, Swift, Kotlin, Scala, Dart. And `unwired_candidates` is
computed entirely from inbound `Imports` edges. That is **W0.3**, and it is the headline.

---

## Scoreboard

| Phase | Count | Theme |
|---|---:|---|
| P0 | 4 | The map states things it cannot know |
| P1 | 3 | The map is silent where it should speak |
| P2 | 4 | The map knows but doesn't say |
| P3 | 3 | Documented consumer breakage |
| P4 | 3 | Measurement you don't have |

---

# P0 — The map states things it cannot know

*Gates every later phase.*

Each of these produces a confident verdict from an absence the kernel created itself. A structural
blind spot and a genuine negative are currently indistinguishable in the output, at the `extracted`
tier the tool descriptions tell agents to act on.

## W0.1 — Make per-language capability a declared, tested fact

`blocker` · size M

**Why.** `CALL_EXTRACTION_LANGUAGES` claims in its own doc comment to be "read by the coverage
report so 'this language has no call graph' is a stated fact rather than an indistinguishable
zero." No coverage report exists. The constant has **zero production readers** — every reference is
under `tests/` — and it is wrong by omission, listing 17 languages while silently excluding the 10
handled by `treesitter.rs` match arms plus the 4 C-family languages.

**Evidence.**
- `devmap-extract/src/langcalls/mod.rs:90-97`
- `devmap-extract/src/treesitter.rs:2241-3369` (`extract_node`)
- `devmap-extract/src/languages.rs:87-96` (`LanguageSpec`)
- `rust-port/STATUS.md:1186-1191`

**Change.**
- Add `capabilities: Capabilities` to `LanguageSpec` — a bitset over
  `CALLS | IMPORTS | REFERENCES | HERITAGE`. There is no existing field to reuse; this is new.
- Delete `CALL_EXTRACTION_LANGUAGES` and repoint its 9 test readers at the registry.
- Budget the parity contract: `LANGUAGE_SPECS` derives `Serialize` and is zipped pairwise against a
  frozen Python golden. Adding a field touches `testdata/golden/language_specs.json` and
  `test_phase2_hardening.rs:47`.

**Test.** The registry must be *derived and verified*, never hand-maintained — that is exactly how
the current constant rotted. Add a test that runs every fixture under `testdata/golden/languages/`
and asserts the observed non-empty vectors match the declared bitset **in both directions**. A
language that gains a `langcalls` module and forgets the flag must fail CI.

**Done when.** Every language's capability is one lookup, and a capability that drifts from behavior
breaks the build.

## W0.2 — Charge call-blindness and import-blindness to `ExtractionCoverage`

`blocker` · size S

**Why.** `extraction_coverage` folds `extraction_gaps`, which increments only on `ParseFailed` and
`PatternRecovered`. A CFML or HCL file parses `Clean`, emits zero calls, and
`coverage.is_complete()` stays true — so `cap(0.9)` passes 0.9 through untouched and every top-level
symbol in the file is reported at the `extracted` tier.

**Evidence.**
- `devmap-analyze/src/liveness.rs:392-422` (`extraction_gaps`)
- `devmap-analyze/src/liveness.rs:431-440` (`extraction_coverage`)
- `devmap-analyze/src/liveness.rs:325-331` (`cap`)
- `devmap-analyze/src/liveness.rs:724` (the 0.9 branch)

**Change.**
- Add `ExtractionGap::CallBlind` and `ExtractionGap::ImportBlind`, driven by the W0.1 registry, not
  by a second hand-written list.
- Add matching counters to `ExtractionCoverage`; fold them into `files_without_call_extraction()`.
- Give call-blindness its own reason string. `COVERAGE_LOSS_REASON` says "call extraction did not
  cover every file" — for CFML the truth is "this language has no call extractor," which is a
  different fact and a permanent one.
- Leave `COVERAGE_LOSS_CONFIDENCE_CAP` at 0.35. The mechanism is right; only its trigger set was
  incomplete.

**Test.** Extend `extraction_coverage_liveness.rs` — whose thesis is already "nothing calls it only
counts corpus-wide when calls were looked for" — with a CFML and an HCL fixture. No
`extracted`-tier report may name a symbol in a call-blind file.

**Done when.** A pure-CFML or pure-Terraform repo yields zero `extracted` dead findings, and says
why.

## W0.3 — `unwired_candidates` is import-blind for 24 of 35 languages

`critical` · size M

**Why.** This is the largest false-positive surface in the kernel. `unwired_candidates` asks whether
a file has an inbound `Imports` edge from a non-test file. But there are only **five**
`imports.push` sites in the entire extractor — Python, JS/TS/TSX, Rust `use`, Go `import_spec`, plus
the embedded-script merge. `langcalls/*` and `langdecl/*` do not even take an `imports` parameter,
and `#include` has no handler at all.

Consequence: in a Java, C++, Ruby, Swift or C# repo, *every* non-entry-root, non-exempt file is an
unwired candidate. These files parse `Clean`, so the `excluded_coverage_loss` escape does not fire.
The list caps at 200, so the agent sees 200 confidently-wrong filenames.

**Evidence.**
- `devmap-query/src/code_graph.rs:228-288` (`unwired_candidates`)
- `devmap-extract/src/treesitter.rs:2360, 2624, 2685, 2972, 3182` (all five import sites)
- `devmap-extract/src/embedded.rs:776`
- `devmap-query/src/manifest.rs:17` (`UNWIRED_CANDIDATE_CAP`)

**Change — two moves; ship the first alone if needed.**

1. **Stop the bleeding.** Gate `unwired_candidates` on the W0.1 `IMPORTS` capability: an
   import-blind file is excluded and counted into `excluded_coverage_loss`, which
   `liveness_meta.unwired` already carries to consumers. Small diff, removes the whole class of
   falsehood.
2. **Then close the gap.** Add import extraction for the C family (`preproc_include`) and the top
   JVM/CLR languages. C-family `#include` is the highest-value single addition — it is the only one
   that also unlocks header-to-implementation topology, which `c_header_exported_names` currently
   approximates corpus-wide by name.

**Test.** A Java-only and a C++-only golden fixture. Before the gate: assert the fixture produces
unwired candidates (pinning the bug). After: assert zero candidates and a non-zero
`excluded_coverage_loss`. Keep both assertions — the second is the regression guard once imports
land.

**Done when.** No file is called unwired in a language where the kernel never looked for an import.

## W0.4 — Veto a dead verdict when an unresolved call site names the symbol

`high` · size S · *from gortex*

**Why.** You keep a six-tier ledger of every call site the resolver could not bind, and the
dead-code pass never reads it. If an unresolved site names `foo`, then "nothing calls `foo`" is a
statement about your resolver, not about the code. gortex does exactly this join and documents it as
a false-positive suppressor.

The plumbing is already done: `analyze_liveness_with_coverage` takes `&ResolutionResult`, and
`ResolutionResult.unresolved` is right there. **Nothing needs threading.** The only mentions of
`unresolved` in all of `devmap-analyze/src` are a `.len()` and two empty test fixtures.

**Evidence.**
- `devmap-resolve/src/model.rs:629-649` (`ResolutionResult`)
- `devmap-resolve/src/model.rs:611-627` (`UnresolvedReference`)
- `devmap-analyze/src/liveness.rs:461-465`
- `devmap-analyze/src/lib.rs:79`

**Change.**
- Pre-pass over `resolution.unresolved` building `HashSet<&str>` of `callee_name`, filtered to
  `UnresolvedKind::Call | Reference` and to classes that could be a real miss — exclude `Builtin`
  and `HostGlobal`, keep `UninferredReceiver`, `LocalBinding`, `Unresolved`.
- In the cascade, demote the 0.9 branch to the 0.4 `only_ambiguous_callers` tier when the set
  contains the symbol's short name, with its own reason:
  `"an unresolved call site names this symbol"`.
- **Document the imprecision honestly.** `UnresolvedReference` carries no `target_file`, so the join
  is name-only and corpus-wide — the same trade `c_header_exported_names` already makes. Say so in
  the reason string, not just a code comment.

**Test.** Add a shape to `liveness_honesty_over_the_hard_shapes.rs`: a call through an uninferrable
receiver to a uniquely-named method. No `extracted` report may name it. Guard the inverse too — a
symbol whose only namesake is a `Builtin`-class miss must still be reportable, or the veto swallows
everything.

**Done when.** The unattributed tier stops being a silent source of confident dead findings, and the
defect ledger earns its keep twice.

---

# P1 — The map is silent where it should speak

*Recall, not precision. Safe after P0.*

## W1.1 — Detect cyclic dead clusters

`high` · size L

**Why.** Liveness is a one-hop inbound-edge join, never transitive. An abandoned subsystem whose
functions call each other has an inbound edge on every symbol, so the kernel reports **zero** of it.
This is the classic dead-code case and the single largest recall hole. trace-mcp catches it with a
reachability BFS; the liveness path here has no traversal at all.

**Evidence.**
- `devmap-analyze/src/liveness.rs:479-537` (flat edge pass)
- `devmap-analyze/src/traversal.rs:313` (`traverse_graph_indexed` — a real BFS with
  `AdjacencyIndex`, depth/node caps, `TraversalStop`; reuse it)
- No SCC / Tarjan / Kosaraju exists anywhere in either crate. That part is genuinely new code.

**Change.**
- Build the call subgraph from resolved edges only — deterministic and `UniqueGlobal` rungs,
  excluding `AmbiguousGlobal`. An ambiguous edge must not keep a cluster alive.
- Tarjan for SCCs, then mark any SCC whose only inbound edges come from inside itself. Seed
  exemption from the existing entry-root and wiring machinery, so a cluster containing a
  `ScriptEntry` or `RuntimeEntryPoint` is live.
- Report as a new shape, not a flood: `{cluster_id, members[], size, reason}`. A 40-symbol dead
  cluster is one finding, not 40 — otherwise it blows past `DEAD_CANDIDATE_CAP` and pushes real
  single-symbol findings out of the ranked list.
- Confidence ceiling of `inferred` (0.4–0.89). A cluster verdict depends on the whole graph being
  complete, which after P0 you will be able to state precisely — and usually it will not be.

**Test.** Fixture with three mutually-recursive functions reachable from nothing, and a second with
the same shape reachable from `main`. Plus an adversarial case: a 10,000-node cycle, to confirm the
traversal caps hold and the pathology that cost 199 s on nested braces does not recur here.

**Done when.** An abandoned internally-cohesive subsystem is one `inferred` finding instead of
silence.

## W1.2 — Produce `ReferenceKind::Heritage`, then `Extends` / `Implements` edges

`high` · size M

**Why.** A method reached only through its base type is invisible, so every such override is a
candidate `extracted` false positive. Both gortex and GitNexus emit inheritance edges; devmap
declares them and never produces them.

The consumer side is **already written**. `ReferenceKind::Heritage` exists and
`resolver.rs:1906-1908` reads it to set `prefer_types`. The variant is dead only on the producer
side. Superclass names already arrive — as `ReferenceKind::Name`, indistinguishable from any other
identifier. `EdgeKind::Extends` and `Implements` likewise exist with no producer; every reference to
them is a label map or string parser.

**Evidence.**
- `devmap-extract/src/model.rs:722-739` (`ReferenceKind`)
- `devmap-resolve/src/resolver.rs:1906-1908` (consumer already handles it)
- `devmap-extract/src/model.rs:142-143` (`EdgeKind::Extends` / `Implements`)
- `devmap-extract/src/langdecl/swift.rs:263-281` (a heritage reader already exists, emitting only a
  `WiringAnnotation`)
- `devmap-extract/src/treesitter.rs:3703-3704` (ObjC: superclass falls through to
  `maybe_push_name_reference`)

**Change.**
- Tag heritage positions per language: `class_heritage` / `extends_clause` (JS/TS), `superclass` /
  `base_clause`, `inheritance_specifier` (Swift), `delegation_specifier` (Kotlin), `impl_item` trait
  path (Rust), ObjC's non-first container identifier.
- Carry the declaring type on the reference so the edge has both endpoints.
- Emit `Extends` / `Implements` through the existing resolution ladder — no new confidence rules, and
  an ambiguous supertype resolves to nothing as usual.
- In liveness, treat an override as reached when its declaring type implements an interface whose
  method is called. This generalizes the Go interface pre-pass, which today matches on name + arity
  only and produces a wiring exemption rather than an edge.

**Test.** Per-language fixture: base class with a virtual method, derived override, call through the
base type. Assert an `Extends` edge exists and the override is not reported dead. Extend the golden
`edges.json` for each affected fixture.

**Done when.** Polymorphic dispatch stops reading as death, and two declared edge kinds stop being
decorative.

## W1.3 — Compute re-export chains for JS/TS

`medium` · size S

**Why.** `reexport_chains` is documented as "Always empty. Nothing computes this," and
`audit_regressions.rs:766` pins the emptiness as intentional under R-9. In a barrel-heavy TypeScript
repo a symbol consumed only through `index.ts` has no direct inbound edge. trace-mcp treats barrel
re-export as one of four core dead-code signals precisely because of this.

**Evidence.**
- `devmap-resolve/src/model.rs:645` (`reexport_chains`)
- `devmap-resolve/src/resolver.rs:1499` (initialized, never written)
- `devmap-resolve/tests/audit_regressions.rs:766` (R-9 pin)
- `devmap-extract/src/model.rs:771-777` (`ExtractedExport.module_specifier`)

**Change.**
- No extraction work needed for JS/TS: `module_specifier` is already populated for
  `export { a } from './m'`, with a test pinning that it is `Some` only when a source module exists.
- Compute at `resolver.rs:1499` by joining re-exports against the target file's exports. Cap chain
  depth and refuse on cycles.
- In liveness, a symbol reached through a resolved chain counts as called at the chain's weakest
  rung.
- **Retire R-9 deliberately.** Rewrite `reexport_chains_are_never_computed` into a scope assertion —
  chains exist for languages with export capability, empty elsewhere — rather than deleting the
  guard.

**Done when.** A barrel-file TypeScript project stops reporting its own public API as dead.

---

# P2 — The map knows but doesn't say

*Independent of P0/P1 — parallelizable.* This is where the surveyed tools are ahead, and the
cheapest ground to take. devmap computes more honesty internally than any of them and publishes
less of it.

## W2.1 — Compute and persist a resolution rate

`high` · size S

**Why.** Every number is in the unresolved ledger and nothing divides. There is no
`resolution_rate` / `resolved_pct` anywhere in the Rust kernel or the Python surface — grep returns
a SQLite freelist ratio and a task-completion percentage, nothing else. gortex ships per-language
parity baselines fenced in CI; devmap ships raw counts and leaves arithmetic to a reader who will
not do it.

Deeper payoff: with a per-language rate, dead-code confidence becomes *derived* rather than a
hardcoded 0.9 — and W0.2's whole bug class would have surfaced automatically as a language sitting
at 0.0.

**Evidence.**
- `devmap-cli/src/main.rs:2368-2383` (tally loop — six counters over `resolution.unresolved`)
- `devmap-cli/src/main.rs:2396-2424` (JSON emit)
- `devmap-analyze/src/model.rs:45-93` (`AnalysisSummary`)

**Change.**
- Insert at `main.rs:2383` — after the tally, before `build_manifest_payload`. All six counters and
  `analysis.total_edges` are already in scope.
- Report two numbers: **gross** (resolved / resolved + all unresolved) and **net** (excluding
  `Builtin`, `HostGlobal`, `External` — the misses that are not defects). The net figure is the one
  worth ratcheting.
- Break down by language; that is what makes it actionable.
- Add to `AnalysisSummary` behind `#[serde(default)]` so old `analysis_json` still deserializes.
  Mirror into the human CLI output below `main.rs:2439`.

**Done when.** `dev map` ends with a per-language resolution rate, and a language at zero is visible
without reading source.

## W2.2 — Publish an `epistemic` field on every liveness answer

`high` · size S · *from GitNexus*

**Why.** GitNexus stamps every impact answer `exact` or `lower-bound` with prose `boundaries` and
`causes`. devmap computes strictly more of this — coverage loss, the six-tier defect ledger,
ambiguity fan-out truncation — and exposes none of it as a single readable verdict.

**Evidence.**
- `src/devcouncil/integrations/mcp/handlers/codeintel.py:774-792` (envelope)
- `src/devcouncil/integrations/mcp/handlers/codeintel.py:105-115` (tool spec)
- `src/devcouncil/integrations/mcp/handlers/tool_specs.py:458-490`

**Change.**
- Add `epistemic: "exact" | "lower_bound"` plus `boundaries: []` to the `_client_envelope` payload
  for `devcouncil_code_dead` and `devcouncil_liveness`.
- Populate boundaries from what already exists: coverage loss counts, call/import-blind language
  names from W0.1, `excluded_coverage_loss`, unattributed-tier count, ambiguous fan-out truncation.
- Update both tool descriptions. The `devcouncil_liveness` text already carries manual caveats —
  "If entry_roots are empty or unreachable_unreliable is true, ignore unreachable_files" — which is
  exactly the reasoning this field automates. Replace the prose caveat with the structured field.

**Done when.** An agent can tell from the response alone whether the answer is complete, without
being told to remember a rule.

## W2.3 — Expose the resolution rung as a query parameter

`medium` · size S · *from gortex*

**Why.** The devmap ladder is better than gortex's — five rungs with abstention, receiver poisoning,
and honesty invariants asserting each rung claims only what its evidence entitles. Theirs is
queryable (`min_tier: lsp`) and this one is not. A caller who wants only deterministic edges for a
refactor cannot ask for them.

**Change.**
- Add `min_rung: "deterministic" | "high" | "speculative"` to the impact, callers and trace queries,
  defaulting to current behavior so nothing shifts under existing callers.
- Filter on persisted milliconfidence, reusing the `confidence_millis` comparison rather than float
  compare — the same reason `EXTRACTED_FLOOR_MILLIS` exists.
- Return the rung histogram alongside results so a caller sees what filtering cost them.

**Done when.** The ladder already built is usable from the outside.

## W2.4 — Resolve two permanently-empty fields

`hygiene` · size S

**Why.** Two fields ship in every response and are always empty. `unreachable_files` is hardcoded
`[]` with `liveness_unreachable_unreliable: true` unconditionally, so four Python consumers
permanently suppress it. `runtime_proven_live` is hardcoded `[]` at four sites and nothing in the
repo populates it.

**Evidence.**
- `devmap-query/src/manifest.rs:459-465`
- `src/devcouncil/integrations/mcp/handlers/codeintel.py:734, 769, 780, 803`
- `src/devcouncil/integrations/mcp/handlers/map.py:765-775`

**Change.**
- `unreachable_files`: W1.1 gives real traversal. Either populate it from the SCC pass, or remove
  the key and its four consumer suppressions. Do not leave it as a third state.
- `runtime_proven_live`: the DAP tracing subsystem exists but the fingerprint join was never wired.
  Either wire it or drop the field. Shipping an always-empty "runtime proven" key next to static
  findings implies a capability that does not exist.

**Done when.** Every key in the envelope can be non-empty for some input.

---

# P3 — Documented consumer breakage

`rust-port/CONSUMERS.md:152,157` marks two verification checks as "PRIVATE INTERNAL REACH … WILL
BREAK AT CUTOVER." The cutover has happened — `map_artifacts.py` declares one writer and
`MAP_ENGINE = "devmap-rust"` — so these are live inconsistencies, not future ones.

## W3.1 — The liveness ratchet baselines against the retired Python scanner

`correctness` · size M

**Why.** `snapshot_liveness_baseline` calls `RepoMapper.liveness_snapshot()`, which runs
`_compute_liveness` → Python `file_liveness` and the regex token scanner `_dead_symbol_candidates`.
Everything else in the system reads the Rust kernel. So the CI ratchet compares a Python baseline to
a Python current, on a code path nothing else uses — and both differ from what `dev map dead`
reports.

Worse, the unreachable half is dead by construction: the diff is skipped whenever either side is
unreliable, and the kernel writes `unreliable: true` unconditionally. That branch has not fired
since cutover.

**Evidence.**
- `src/devcouncil/verification/checks/liveness_ratchet.py:297-355` (baseline), `:148-156` (skip)
- `src/devcouncil/indexing/repo_mapper.py:1836-1930`
- `src/devcouncil/indexing/map_artifacts.py:7-13, 39`
- `devmap-query/src/manifest.rs:459-465`

**Change.**
- Rebase the snapshot on `DevMapClient` — the same `dead_symbols(budget=…)` path the CLI and MCP
  handler use.
- Bump `LIVENESS_SCHEMA_VERSION` (1) and `LIVENESS_SCAN_VERSION` (4) together; the symbol half
  already refuses a mismatched `scan_version`, so old baselines invalidate cleanly rather than
  comparing across engines.
- Ratchet on the W2.1 net resolution rate as well as candidate counts — a rate drop is the earliest
  signal that extraction regressed, and it is the metric gortex fences in CI.
- Drop the unreachable half or re-enable it against W1.1 output. Do not leave a branch that cannot
  execute.

**Done when.** The ratchet measures the engine that ships, and a regression in it means what a user
would see.

## W3.2 — Give `dead_symbols.py` a supported client path

`medium` · size S

**Why.** The diff-scoped gate reaches the kernel through `DevMapClient.impact(path, depth=1)` and
falls back to parsing `code_graph.json` directly via `symbol_has_non_test_inbound`. When both fail
it sets `graph_confirmed = False` and records `"graph-confirmation:unavailable"` — but still emits
the gap, and the whole body degrades to `return []` on any exception. A check that can silently
return nothing is not a gate.

**Evidence.**
- `src/devcouncil/verification/checks/dead_symbols.py:328-344, 353-356, 392-400, 413-415`
- `rust-port/CONSUMERS.md:152`

**Change.**
- Add a first-class "is this symbol reached" query to `DevMapClient` instead of inferring it from
  `impact` plus a JSON fallback.
- Make graph-confirmation failure *visible*: on `graph_confirmed = False`, downgrade to non-blocking
  and say so, rather than emitting an unconfirmed gap at the same weight as a confirmed one.
- Narrow the bare exception guard so a kernel outage is reported, not swallowed.

**Done when.** The gate cannot fail open without saying it failed.

## W3.3 — Collapse the duplicated wiring heuristics

`debt` · size M

**Why.** `src/devcouncil/indexing/wiring.py` and `devmap-extract/src/wiring.rs` implement
overlapping entry-root, test-path and exemption rules in two languages. Duplicated logic, not dead
logic — five Python modules still import it — so the two can and will disagree about whether a file
is an entry root.

The Python side holds one thing the kernel lacks: dynamic-import clearing (`importlib`,
`import('./App')`, `new Worker(new URL(…))`, `package.json` scripts) and the `ALLOW_UNWIRED` escape
hatch. That is real false-positive protection worth keeping.

**Evidence.**
- `src/devcouncil/indexing/wiring.py:46, 1357-1380, 1509-1694`
- `devmap-extract/src/wiring.rs:443`
- `src/devcouncil/indexing/graph/liveness.py:1-7` (docstring already announces the retirement)

**Change.**
- Port `dynamic_import_keys` and the `ALLOW_UNWIRED` marker into `wiring.rs` as a new `WiringKind`
  or annotation detail — the kernel is where unwired is decided.
- Then retire `graph/liveness.py` and `repo_mapper._dead_symbol_candidates`.
- Keep the Python `confidence_label` / `confidence_at_least` helpers — three live callers, pure tier
  arithmetic.

**Done when.** One implementation decides what "wired" means.

---

# P4 — Measurement you don't have

*Start W4.1 during P0 — it scores P0.*

Accuracy today is asserted through determinism digests, golden identity fixtures, mutation testing
at 86% kill rate, and structural honesty invariants. That is more rigor than any of the three
competitors — **and none of it is a precision or recall number.** Neither gortex nor GitNexus nor
trace-mcp publishes one either. This is open ground.

## W4.1 — A labelled corpus with ground-truth liveness

`high` · size L

**Why.** The 40 golden fixtures pin *identity* — same nodes, same edges — not *correctness*. The
parity harness compares against a Python-derived golden that still reports diffs and carries no
sign-off. Nothing anywhere answers "of the symbols we called dead, how many were?"

**Evidence.**
- `rust-port/testdata/golden/` — 40 fixtures × 3 files + `language_specs.json`
- `rust-port/tools/parity/parity_harness.py:1-18`
- `rust-port/STATUS.md:236` — "No parity sign-off."

**Change.**
- Extend the fixture shape with `truth.json`: `{symbol_id, live: bool, why}`, hand-labelled. Start
  with the 5 app fixtures (TS, Python, Rust, Go, Solidity) — small enough to label exhaustively,
  varied enough to be meaningful.
- Score precision and recall **per confidence tier**. The number that matters is precision at
  `extracted`: that tier's contract is "safe to act on," and any false positive in it is a contract
  violation, not a metric dip.
- Deliberately include the shapes P0 and P1 address — a CFML file, a Java file with no imports, a
  cyclic cluster, a polymorphic override, a barrel re-export. The harness should fail today and pass
  as each work order lands.
- Fence `extracted`-tier precision in CI at 1.0 and treat any drop as a build break, matching the
  strictness of the existing honesty invariants.

**Done when.** You can state a precision number for the `extracted` tier, and defend it.

## W4.2 — Per-language resolution baselines, fenced in CI

`medium` · size M

**Why.** gortex's one real accuracy discipline is `eval parity`: the share of symbol-bearing files
with at least one resolved cross-file dependent, per language, baselined and CI-fenced three ways.
It is a proxy, but it catches silent extraction regressions that identity fixtures cannot — a
grammar bump that quietly stops matching a node kind passes every golden and tanks the rate.

**Change.**
- Build on W2.1's per-language net rate. Freeze a baseline JSON alongside the golden fixtures.
- Fence a drop beyond a small tolerance as a CI failure **with the language named** — the failure
  message should say which language regressed and by how much.
- Record the current corpus figures as the initial baseline: 852,962 resolved edges and the 94.9%
  unattributed reduction are already measured, just never fenced.

**Done when.** A grammar bump that silently stops extracting calls fails CI on the language it
broke.

## W4.3 — Add a precision stage to `map_bench.py`

`low` · size S

**Why.** `benchmarks/map_bench.py` already stages `cold / warm / touch / manifest / e2e / search /
impact / path / dead / growth` with peak RSS beside wall time. The `dead` stage measures how fast
the answer arrives, never whether it is right.

**Change.** Extend the `dead` stage to emit W4.1 precision and recall alongside latency, and the
W2.1 resolution rate alongside edge counts. One command should report both cost and correctness, so
a change that buys speed by dropping edges is visible in the same table.

**Done when.** Speed and accuracy regress into the same report.

---

# Version budget

Several work orders invalidate caches or migrate schema. Batch them so the corpus rebuilds once
rather than five times, and so a single `v32` rationale paragraph covers the whole extraction
change.

| Version | Now | After | Forced by | Footgun |
|---|---:|---:|---|---|
| `EXTRACTION_SCHEMA_VERSION` | 31 | 32 | W0.1, W1.2, W0.3 (imports) | Convention is a rationale paragraph per bump in `cache.rs`. One paragraph, one bump, all three changes. |
| `CURRENT_SCHEMA_VERSION` | 15 | 16 | W2.1 (rate columns) | The fresh-create path at `db.rs:1338-1358` stamps the version directly and never runs the migration chain — new tables must be added **there too**, or a fresh clone diverges from a migrated one. |
| `LIVENESS_SCHEMA_VERSION` | 1 | 2 | W3.1 | Bump with `LIVENESS_SCAN_VERSION` (4→5) or old Python baselines silently compare across engines. |
| Golden fixtures | 121 | 126+ | W0.1, W1.2, W4.1 | `language_specs.json` is a pairwise parity contract against a frozen Python registry; `truth.json` is a new third file per fixture. |
| Pinned regression R-9 | pinned | rewritten | W1.3 | `reexport_chains_are_never_computed` pins emptiness as intentional. Rewrite as a scope assertion; do not delete. |

Migration recipe for `CURRENT_SCHEMA_VERSION` 15→16 (from `db.rs:1369-1531`):

1. Add `MIGRATION_V15_TO_V16` const in `schema.rs` next to `MIGRATION_V14_TO_V15`.
2. Bump `CURRENT_SCHEMA_VERSION` to 16.
3. Add `if version == 15 { … }` in `db.rs`, stamping the **literal** 16, not the constant.
4. Move `Self::validate_schema(&tx)?` from the v14 block into the new last block.
5. Add the new table/column to the fresh-create path at `db.rs:1338-1358` as well.
6. Guard any `ADD COLUMN` with `Self::has_column(&tx, table, col)?`.
7. Extend `validate_schema` (column list example at `db.rs:948`).

---

# Sequencing

| # | Ships together | Why this grouping |
|---:|---|---|
| 1 | W0.1 → W0.2 → W0.3 gate | W0.2 and W0.3 both consume the capability registry. The W0.3 gate alone stops the largest falsehood without waiting on import extraction. |
| 2 | W0.4 + W2.1 | Both read `ResolutionResult.unresolved`, both small, neither touches extraction. Ship together and the ledger pays for itself twice in one diff. |
| 3 | W4.1 (start early) | The labelled corpus should be written to **fail** against today's kernel. It is the scoreboard for everything above, so it wants to exist before P0 lands, not after. |
| 4 | W2.2 + W2.3 + W2.4 | All envelope and query surface. One MCP schema change, one round of tool-description edits, one consumer update. |
| 5 | W0.3 imports + W1.2 heritage | Both are per-language extractor work against the same grammars, sharing the `v32` bump and one corpus rebuild. |
| 6 | W1.1 → W2.4 `unreachable_files` | The SCC pass is what makes the always-empty field answerable. Resolve them as one decision. |
| 7 | W3.1 → W3.2 → W3.3, then W4.2, W4.3 | Consumer cutover after the kernel stabilizes; baselines last, so they freeze the improved numbers rather than the current ones. |

---

# The through-line

Every P0 item is the same failure in a different place: **a structural absence read as an observed
negative.** No calls extracted, no imports extracted, no resolution achieved — all three currently
render as "nothing references this," at the tier that means "safe to delete."

The fix is not more analysis. It is making the kernel's own coverage a first-class input to its
verdicts, which is a thing nearly built already: `COVERAGE_LOSS_CONFIDENCE_CAP` is exactly this
mechanism, applied to two of the five ways coverage can be lost. P0 finishes it. P2 publishes it.
P4 scores it.

That combination — a confidence number derived from measured coverage, with a precision figure
defending it — is something none of gortex, GitNexus or trace-mcp currently offers.
