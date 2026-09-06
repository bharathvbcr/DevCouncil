# devmap (Rust) Status Ledger

**Active work:** Phase 6 consumer hybrid migration (2026-08-12); kernel performance and gap pass (2026-09-02)
**Verified scope:** local Rust kernel, persisted queries, differential daemon batching, IPC, and one hybrid Python consumer
**Closed 2026-08-15 (external-corpus remediation):** generation retention (SC1), peak memory (SC3), build time (SC3b), missing memory/growth gates (SC3c/SC5), dead-code entry-point false positives (SC6), Go cross-file interface exemptions (SC6a), extraction-cache growth (SC7), doubled payload storage (SC8), receiver-name collision producing confidently wrong edges and an unjoinable Go call graph (SC9), orphaned JS/TS call scopes (SC10), Rust trait identity collisions and missing trait signatures (SC11/SC6b), unresolvable Rust method calls (SC12), calls hidden in Rust macro bodies (SC13), most nested-symbol identity collisions (SC14)

**Closed 2026-08-17 (personal-corpus remediation):** phantom call edges from JSX intrinsic host elements and Go composite-literal type expressions (SC17); external/builtin classification of calls that can never resolve (SC18); CI running the gate script under the wrong shell (SC20); the binary being unreachable outside this repository (SC21); shell and SQL parsing (SC22); the hybrid consumers' Rust path never having executed (SC23); an unbuilt store reporting itself as available (SC24); receiver-type inference for library types (SC25); five call shapes recording whole expressions as callees, recovering 89,356 real edges (SC26); a size gate measuring a state production never reaches (SC27); two concurrency defects that made a cold start unreliable for any second process (SC28); Metal coverage measured and found at C++ parity, closing an item that had been recorded as unclosable (SC19); an absolute memory gate that covered no large corpus, replaced with a scale-invariant one — and the Σ N² memory model this ledger had asserted since SC3, refuted by measurement (SC29); the unresolved defect tier resolved into four evidence-backed classes (SC30); the C family given a call graph and made answerable to dead-code analysis (SC31); the whole-expression-callee mechanism SC26 had only patched shape-by-shape (SC32)

**Not complete** — the consolidated, de-duplicated list of open work with owners and blocking decisions now lives in [AGENT_PLAN.md → Consolidated open-work register](AGENT_PLAN.md#consolidated-open-work-register-2026-09-02); this line is the narrative summary it indexes: one unlinked grammar (VB.NET), the call-graph blackout across ~26 non-C languages (SC34), the unexplained SC26 per-file memory increase (SC29), full audit-property suite, the B3 write-amplification redesign and genuinely incremental resolve (SC2), nested-symbol identity collisions inside anonymous callbacks and nested types (SC14), workspace-wide mutation coverage beyond the retention surface, freshness soak, consumer cutover, deletion, and platform publication

**Closed 2026-09-02 (kernel performance and gap pass, K1–K8 in [PLAN.md](PLAN.md) §3):** discovery silently dropping grammarless source languages — 19 `.proto` and 17 `.ps1` invisible to the graph (K2); tier-2 pattern recovery inventing 457 symbols out of fenced code blocks in markdown design documents (K3, a regression this pass introduced); search returning **zero** hits when a matched symbol exceeded the token budget, with both budget gates green throughout (K4); the reclaim policy reading a pre-checkpoint freelist and reporting a 0 ms vacuum as success on a store that was 33% garbage (K5); a rebuilt binary leaving the daemon serving superseded code (K6); an embedding ranker that put the correct symbol at rank 28 or absent on all 5 probes (K7). End-to-end `dev map` is **−57.5%**, cold −36.8%, incremental −35.7% — most of it Python seam overhead rather than kernel work.

**All eight kernel findings K1–K8 are closed**, each with a regression that fails against the pre-fix code; workspace suite **717 passed / 0 failed**. **Open as *classes* rather than instances:** the five failure shapes in [PLAN.md](PLAN.md) §3.1, four of which produced a fresh instance in the Rust port after the Python instance had already been fixed and written up as an acceptance property. **All five now have closing gates in code** (required decisions 9 and 11); what remains is the workspace-wide Class A audit, the HEAD boundary for Class D, and measurements beyond the build profiler for Class E. Also open: the unbuilt gortex capabilities G4–G7. See [Performance and gap pass (2026-09-02)](#performance-and-gap-pass-2026-09-02).

**Ledger corrections (2026-08-17), each measured rather than asserted:**

- This file previously said "5 of 35 grammars linked". That has been false for some time: `extract_treesitter` has **32 grammar arms**, and of the 35 frozen `LANGUAGE_SPECS` only **VB.NET** (`grammar: "vb"`) has no grammar behind it. ArkTS and Metal are not unlinked — they deliberately reuse the `typescript` and `cpp` grammars via their spec's `grammar` field.
- The suite is **407 tests**, not 279.
- Corpus scale claims here were all derived from one 4,742-file external corpus. The port has now been run over a **12,821-file, 12-language personal corpus** (`/Users/bharath/Code`): 116,418 symbols, 853,421 edges, 70 s wall, 3.00 GiB peak RSS.
- **The SC9/SC10 joinability invariant holds at that scale: orphaned call edges = 0** across 853,421 edges, 2.7× the corpus it was fixed against.
- SC14 duplicate identities on that corpus: **741** (the 76 figure was for the smaller corpus; the defect is unchanged, the exposure is proportional).

Passing tests are local/mechanical evidence only. They are not evidence of the required two-week shadow soak, platform CI, production deployment, or full parity.

## Phase ledger

### Phase 0 — scaffold reconciliation: locally complete

- Seven-crate workspace is mapped and the execution/divergence ledgers exist.
- The old regex fallback extractor was independently verified as runtime-unwired (source search and `dev map query`) and removed; Tree-sitter failure remains explicit.

### Phase 1 — frozen contract: partially complete

- The Rust registry matches all 35 frozen Python language specifications.
- One fixture directory and three normalized golden files exist per language; the five tier fixtures are retained.
- Snapshot generation is fail-closed, atomic, runs under `PYTHONHASHSEED=0`, and removes transient fixture caches.
- Two complete regenerations produced the same 121-file SHA-256 tree digest: `c3af36d772f6212202c155435402066674f3f4c1c4285726a5198311b22d0499`.
- `CONSUMERS.md` preserves the frozen 37-file audit set; the current direct-import count is 35 because two files no longer match the original grep.
- Open: the tier fixtures do not yet exercise every PLAN §4.1–§4.3 capability and all framework families.

### Phase 2 — extraction: incomplete

- Complete extraction contract now includes engine identity, `ParseOutcome`, byte spans, exports, references, imports, calls, routes, and wiring.
- Failed/partial parsing is not promoted to fabricated clean output; failed outcomes are not cache-admitted.
- Cache identities include the Cargo package version, extraction-schema version, grammar package version, TS/TSX variant, and ABI, so new serialized semantics cannot reuse an older payload.
- Discovery now reports yielded/skipped candidates and rejects sources larger than 1 MiB.
- Cache keys are computed before parsing, cache-admission errors propagate, and the source hash is pinned to FNV-1a 64 instead of Rust's release-dependent `DefaultHasher`.
- Extraction schema v3 carries nested type-qualified method identities and auxiliary diagnostics. Framework matcher failures no longer rewrite a clean grammar parse as failed; the AST walker is iterative, and zero-argument `require()` cannot fabricate a module.
- A cache collision found by the real-repository build was fixed: identical-content files can no longer inherit another path's cached identity.
- **Grammar coverage (re-measured 2026-08-17, replacing the stale "5 of 35" claim):** `extract_treesitter` links 32 grammars. Of the 35 frozen specs, 34 reach a real grammar — ArkTS routes to `typescript` and Metal to `cpp` through their spec's `grammar` field. Only **VB.NET** (`grammar: "vb"`) has no grammar and returns `ParseOutcome::Failed`. Adding grammar dependencies still requires explicit approval under repository policy.
- **Closed (SC19, 2026-08-17): Metal's coverage was never measured, and when measured it is at C++ parity.** The open item recorded that no tree-sitter Metal grammar exists and concluded the language "cannot be closed". That is a claim about *grammar availability* standing in for a claim about *coverage*, and the two have different answers. Metal's spec routes to `tree-sitter-cpp`, so Metal files were being parsed the whole time; nobody had measured how well.

  **Measured first, against a plain-C++ control.** A realistic 111-line shader — compute kernels with `[[buffer(n)]]`/`[[thread_position_in_grid]]`, vertex/fragment stages with `[[stage_in]]`/`[[position]]`, `[[attribute(n)]]` struct members, address-space pointers, `constexpr constant` globals, templates, namespaces — parsed `Partial` with 16 ERROR nodes over 91 bytes, **2.5% of the file**, and still yielded **13 of 13 declarations with correct names and kinds**. Neutralising every Metal qualifier and re-extracting as C++ produced the *identical* symbol set. Metal's `[[...]]` attributes are C++11 attribute syntax and contribute **zero** errors, argument position included.

  **The control is what settles it.** Metal recovers no calls, no `#include` imports, no global-variable symbols, drops function prototypes, and absorbs a pointer return type into the symbol name — and **plain C++ does every one of those identically**. Those are C-family limits of the generic extractor, not Metal defects, and they are now tracked as SC31. The only Metal-specific difference in the entire measured surface was the `Partial` bit.

  **That bit was still worth closing, for two named costs**: it made the diagnostic worthless (every Metal file permanently degraded, so a genuine grammar regression is invisible against the background), and it armed `liveness`'s `overlaps_parse_error` to exempt every shader in the corpus from dead-code analysis. Three error classes were classified benign, each gated on the file being Metal — read from the registry, since `lang` is `"cpp"` for Metal and C++ alike: the qualifier displacing the type (`kernel void f`, `device float *p`), the address-space cast (`(device bfloat *)O`), and the zero-width MISSING `::` the grammar inserts for an atomic in an address space (`device atomic_float *d`, `device atomic<float> *w`). Fail-closed: the claim is bounded to a single token and the qualifier must be reachable across at most one intervening type token, so a broken region — which tree-sitter reports as one wide ERROR — is never absorbed.

  **Shader entry points now carry evidence.** `kernel`/`vertex`/`fragment` are the GPU half of a call whose caller is host code in another language, so they get `WiringKind::RuntimeEntryPoint`, the same seam as Go's `init` and Rust's `#[no_mangle]`. The generic C-family extractor has no visibility keyword to read and marks every declaration exported, so without this an entry point and a private helper were indistinguishable in the dead-code output.

  **Measured on 55 real `.metal` files (11,274 lines): 54 of 55 `Partial` with 2,584 error ranges → 0 and 0**, with the recovered declaration count **unchanged at 287** and no file's symbol set differing — the outcome is cleared without moving a single symbol — and **240 shader entry points annotated where there were none**. Parity unchanged at 20 diffs; the harness compares 5 tier fixtures and exercises no `.metal` file. `EXTRACTION_SCHEMA_VERSION` 19→20.

  **Both changes are currently latent in dead-code output** — they change the exemption *reason*, not the verdict, until SC31 was fixed. Stated here rather than claimed as a dead-code win.

  **Source rewriting was considered and rejected**: blanking qualifiers before parsing was measured to recover nothing extra, and it breaks on a variable legitimately named `thread` or `constant`. Authoring a Metal grammar is still not proposed — and is no longer what closing this depended on.

  **Known stale, not corrected here:** `testdata/golden/languages/metal/nodes.json` names the fixture's symbol `kernel` — the Python baseline took the qualifier for the function name, where the Rust port correctly names it `add`. Pinned by test rather than regenerated, since regenerating perturbs the 121-file golden digest.
- **Closed (SC29, 2026-08-17): the memory gate was absolute where the risk is scale-dependent — and the model it was reasoned from had been false since SC3 fixed it.** The RSS gate was a flat 2 GiB measured on a 1,121-file repository, so a 12,831-file corpus at 3.40–3.55 GiB had no coverage at all, and no single absolute number can bound both. Replaced with a scale-invariant budget, `files × 512 KiB + fanout_edges × 600 B`, kept **alongside** the absolute gate rather than instead of it. Green at 316 MiB against a 573 MiB budget; SC3's pre-fix regime was 2,256 KiB/file, 4.4× over, and would have been red. At 12,831 files the budget is 6,415 MiB, so the gate now says something about the corpus that previously had none.

  **The ledger's own model was wrong, and this file was the source of the error.** SC3 established `RSS ≈ base(files) + 112 B × Σ N²` and every later entry — including the SC26 memory note — reasoned from it. That model was true *before* SC3 and was falsified *by* SC3's fix: `ResolvedEdge.resolution` became `Option<Arc<Resolution>>` (`crates/devmap-resolve/src/model.rs:78`), so the N candidate lists are shared — one list per site, each edge O(1). The squaring came entirely from the per-edge clone SC3 deleted. **Measured, paired A/B against a zero-ambiguity control with identical file, symbol and call counts:** holding Σ N fixed at 500,000 while cutting Σ N² fourfold (50,000,000 → 12,500,000) moved the fan-out cost from **198 MiB to 197 MiB** — 0.5%, where the Σ N² model predicts a 4× fall. Bytes/edge is constant to 1.2% across an 8× sweep of Σ N (401.5, 406.4, 412.1, 415.2); bytes/pair moves 4× across the same points, the signature of a derived rather than causal quantity. **Post-SC3 peak memory is linear in Σ N at ~410 B/edge.** The pre-SC3 model overpredicts the probe by 24× (5,369 MiB predicted, 226 MiB measured). Reproduced independently after the agent reported it.

  **Consequence: the SC26 +30% has no established cause and is reopened as an open item.** Fan-out is 8.6 MiB of a 309 MiB peak on this repository — 2.8% — and cannot explain a 30% move. The real change is per-file base cost, ~210 → ~278 KiB/file (inferred from corpus totals; that corpus is not on this machine). The fan-out explanation previously recorded here is withdrawn.

  **The derivation is validated, not asserted.** Σ N² is recovered from the persisted store (`tools/fanout.sql`) with no resolver change, two independent ways — the target node's `name`, and the suffix of `target_symbol` — which must **agree** or the gate fails. Pinned by a hand-counted 6-file fixture (sites 5, edges 13, Σ N² 35, max N 3, exact) in `crates/devmap-cli/tests/test_fanout_metric.rs`, and by the arithmetic identity Σ N² = sites × K² on a synthetic corpus at 10⁷ scale. Documented limitation: the resolver dedups before persisting, so the store-derived figure is a **lower bound** — the fail-closed direction, since a smaller denominator inflates observed bytes/pair.

  **Nine red-demonstrations, all restored byte-identical.** Deleting one target node, corrupting one node's `name` (the two derivations disagreed: 275,647 vs 279,151), and dropping `callee` from the `GROUP BY` (the hand-counted fixture failed, `left: 4 right: 5` — proof the grouping key is not vacuous) each turned the gate red, along with six threshold and model-bound perturbations. The model check is bounded **both ways**: a measurement below 60% of prediction fails too, so a model that stops describing the code forces re-derivation instead of sitting there unable to fire.

  **A latent CI defect was found in passing:** `verify.sh`'s `peak_rss_bytes` used `/usr/bin/time -l`, which is BSD-only — GNU time requires `-v` — so step 5 would have failed on the `ubuntu-latest` gates job on first push. A `-v` fallback was added; **that path is unexecuted on this machine and remains unverified on Linux.**

- **Closed (SC30, 2026-08-17): the unresolved defect tier held four distinct classes, not one.** After SC25 the tier still carried 628 rows on this repository, undifferentiated. Sampling showed each had different, checkable evidence available, so two tiers were added and the existing tables audited against their specifications. `unresolved` **637 → 585 (−8.2%)**, edges 62,873 → 62,874 (no drop), orphaned call edges **0 → 0**, structural exclusivity clean across all 38,451 rows.

  - **`HostGlobal { environment }` (new, +25).** `setTimeout`, `clearTimeout`, `requestAnimationFrame`, `setInterval` and the rest are runtime-provided, **not** language-defined — no edition of ECMA-262 declares `setTimeout`, so folding them into `Builtin` would assert the wrong authority for a correct-looking answer. Cited to WHATWG HTML/DOM/URL/Fetch and the Node.js globals doc, carried as `"web"`/`"node"`/`"web+node"` exactly as `External` carries its module. Ranked **after** the import rung: `import { fetch } from 'node-fetch'` is file-specific evidence and outranks a global name list. Host *objects* (`console`, `document`, `window`, `process`, `globalThis`) are deliberately **not** admitted — a row naming one as a *callee* is evidence of an extraction defect and must stay in `Unresolved`.
  - **`Builtin` table audit (+23).** JS held 16 of the standard constructors; added every native error (`TypeError` was 18 of the missing rows), all typed arrays including `Float16Array`, `ArrayBuffer`/`SharedArrayBuffer`/`DataView`, `Function`, `WeakRef`, `FinalizationRegistry`. Python gained the four missing exception/warning types; Rust gained the std-root macros usable with no `use`; Go was already complete. Namespace objects (`Math`, `JSON`, `Reflect`, `Intl`) excluded — not callable, so a call naming one is a defect signal. `exit`/`quit` excluded as `site`-injected, a deployment property not a language one.
  - **`LocalBinding` (new, +4).** A bare callee the enclosing scope declares itself. Placed **first** in the ladder, ahead of `Builtin` and `External`: a parameter named `len` shadows Go's builtin and a parameter named `useState` shadows the import, so answering from a wider table would be the right label for the wrong reason. Consults only scope-keyed maps and refuses the empty/file-level scope explicitly — reusing a file-wide fallback here is the SC9 collision and the SC25 leak in a third place, and `a_local_binding_does_not_leak_between_functions_in_one_file` fails the instant the lookup falls back. **Reaches only 4 of ~50 available rows** until the extractor exports per-scope locals; the rows that motivated it (`handler`, `key`) are Rust `let`-bound closures, which the extractor emits nothing for.
  - **Minified vendor bundles (244 rows, 42% of the remainder) deliberately left in `Unresolved`.** No evidence distinguishes a minified bundle from hand-written short-named code: `.min.js` is a filename convention, and a mangled-name heuristic (`ka`, `va`, `Zi`) would exempt real unresolved calls in ordinary math and parser code. Classifying them trades a correct "possible defect" for a guess. The remedy is excluding vendor bundles at discovery, which is not this crate's concern.

  Every new tier carries a tested structural invariant, every new test was verified failing against pre-change code, and no migration was needed — `classification` is `TEXT NOT NULL DEFAULT 'unresolved'` with no `CHECK` (`crates/devmap-store/src/schema.rs:259`), verified rather than assumed.

- **Closed (SC31, 2026-08-17): the C family had no call graph and could never be reported dead.** Found while measuring Metal against a plain-C++ control (SC19) — the control failed identically, which is what proved it was not a Metal defect. Reproduced directly on a three-function file before any change.

  **Root cause 1 — no calls.** The C family has no arm in `extract_node`; it fell through to the generic `_` arm, which emits *declarations only*. Every `calls.push` site in `treesitter.rs` lives inside a language-specific arm. Fixed by extracting `call_expression`, `new_expression` and ObjC `message_expression` gated on grammar keys `c|cpp|objc|cuda` (Metal rides `cpp`), with node shapes read from each grammar's own parse tree rather than assumed. `split_call_target` gained `qualified_identifier` (recursing right, so `a::b::c()` yields callee `c`), `template_function`, `pointer_expression`, and an `argument` fallback on `field_expression` — C/C++ name the receiver `argument` where Rust names it `value`, so Rust is unaffected.

  **Root cause 2 — never dead.** Two causes, not the one first diagnosed. `generic_is_exported` falls back to `!name.starts_with('_')`, reporting **1,393 of 1,464** C-family symbols exported, so all were exempt — every one of the 1,094 `dead` rows was confidence 0.3 "Exported or exempt". Separately `callable_binding_name` reads a `name` field no C-family declaration carries (the name hangs off `declarator`), so **117 of 117** C-family `References` edges had the *file* as source — the SC9/SC10 orphan shape. Export now defaults to header evidence (`extern "C"`, `dllexport`, `visibility`), putting C on the same footing as Python, and `c_callable_identity` is the single canonical owner of C-family identity, so an out-of-line `int S::m()` and its in-class declaration agree (SC14's recommendation applied).

  **Measured, 183 first-party C-family files:** `Calls` **0 → 1,828**; `References` 117 → 999 with file-sourced ones 100% → 14%; exported 1,393/1,464 → 270/1,362; duplicate identities 32 → **3**; actionable dead findings **0 → 45**; orphaned call edges **0**. Collapsing duplicate type symbols also recovered **841 real type-reference edges** that ambiguity had been discarding — `cmark_node` had 9 symbol copies, now 1. **9,082 LibTorch/ATen headers** (the header-hazard test): `Calls` 0 → 154,107, nodes 29,012 → 22,339, confident dead findings **0 before and after** — every symbol is header-declared, so the header rule holds and manufactures no false positives. All 45 survivors were read individually: 41 are cgo `x_cgo_*` called from Go assembly, 4 were CUDA (see below), 1 is used only inside `#define` bodies.

  **Two follow-on fixes applied by the orchestrator.** `LangFamily::from` was missing `"cuda"`, so CUDA fell to `Generic` and cross-family resolution never fired — invisible until the C family had calls at all, and the sole cause of 4 of the 45 findings. And function-like macros were not symbols: once C calls are extracted, `ACTIONS(1)` is a recorded call whose `#define` target the graph never emitted, which alone put **13,630 rows** into the defect tier on this repository — 7,372 of them `ACTIONS` inside one generated `parser.c` dispatch table. `preproc_function_def` is now a `Function` symbol; object-like `preproc_def` deliberately stays out, since it is a constant and never a callee, and the test pins both directions. Defect tier **13,788 → 178** after that fix.

  **Not covered:** Objective-C is fixture-tested only — the corpus contained zero `.m`/`.mm` files, and ~1,041 `.m` files under `/Users/bharath/Code` appear to be MATLAB. `#define` bodies are still unparsed, so calls inside them are invisible (SC13's Rust-macro fix has no C counterpart). The 9,082-header build went 15 s → 90 s, but the baseline extracted zero calls so it is not like-for-like.

- **Closed (SC32, 2026-08-17): SC26 fixed five call *shapes*; the *mechanism* survived in every other arm.** `split_call_target_inner`'s catch-all returned `get_node_text(function_node, source)` — the target's own source text — and every arm that called it re-introduced its own text fallback. So the graph kept recording whole expressions as callee names wherever SC26 had not looked: Python curried calls (`app.command(name="apply-patch")(fn)` — not a decorator, as first assumed), bundled-JS IIFEs, Rust immediately-invoked closures, and the JS `new_expression` arm, which never went through the splitter at all and read the constructor's raw text.

  `split_call_target` now returns `Option` and applies two rungs — structural (`is_anonymous_callable`) then lexical (`is_callee_identity`) — with all per-arm fallbacks deleted, so there is one place a callee name is built. `is_callee_identity` deliberately admits `$` and `#` rather than reusing ASCII-only `is_user_ident`, which would drop JS private methods and non-ASCII identifiers that *do* resolve. **All seven `ExtractedCall` construction sites were audited**, not just the three reported.

  **Python re-export aliases are now symbols.** Module-level assignments were emitted then filtered by `__all__`; `src/devcouncil/domain/evidence.py` has no `__all__`, so `TestEvidence = VerificationEvidence` existed nowhere and 20 import bindings fell off the ladder. Kept only when the right-hand side is a bare or dotted name — aliases and re-exports, nothing else, since keeping every assignment adds a node per private constant. The payoff was larger than the 20 rows: `_project_root = common._project_root` had no symbol behind it, so **87 call edges** resolved by speculative fan-out instead of import evidence.

  **`Extraction::scope_locals` exported**, letting SC30's `LocalBinding` tier see untyped and closure-bound locals: **4 → 414** rows. One handed-off spec was measured wrong and rejected: widening `is_defining_name` to collect parameter names deleted **231 real `References` edges**, because pytest's `def test_x(mapper)` names a module-level fixture — that parameter is genuinely both a binding and a use. Two questions, two owners; parameter names are collected separately.

  **Measured (isolated A/B, this repository):** non-identifier callee names **216 → 0**; defect tier 585 → 118 (−79.8%); nodes +289, edges +514; 328 speculative/low-confidence edges replaced by 185 deterministic ones with **zero deterministic edges lost and zero `References` lost**, verified case by case; orphaned call edges 0; parity unmoved. The +15 new dead candidates are all module-level aliases nothing imports — true unused-re-export findings, and no previously-dead symbol became live.

- **Correction (2026-09-04): SC34's headline number was wrong, and the shape of the finding changed.**
  This entry said "roughly 26 of 35 languages have no call graph". Re-measured against the
  current tree — every `calls.push` site read, then confirmed empirically on an 18-language
  fixture — the real figure is **12 of 35**: VB.NET, Svelte, Vue, Astro, Liquid, Pascal, CFML,
  COBOL, Erlang, Solidity, Terraform/HCL and Nix, plus shell and SQL which are outside the spec
  set. A `langcalls/` module landed after this entry was written and closed eleven languages
  (Java, C#, Kotlin, Dart, PHP, Ruby, Swift, Scala, Lua, Luau, R); the ledger was never updated,
  so the number here overstated the gap by more than 2x. The definitive language table is in the
  2026-09-04 section below.

  **What did *not* improve is the part that matters.** `langcalls/mod.rs:72` defines
  `CALL_EXTRACTION_LANGUAGES` with the comment "Read by the coverage report so 'this language has
  no call graph' is a stated fact rather than an indistinguishable zero". That claim is false in
  two ways: `rg -uu` finds **no production reader** — all five call sites are in `tests/` — and the
  list omits the ten languages whose calls come from `treesitter.rs` arms rather than `langcalls/`
  (Python, JS, TS, TSX, Rust, Go, C, C++, ObjC, CUDA), so a consumer that did read it would
  conclude Python has no call graph. The constant written to carry the fact carries it nowhere.
  For the twelve, `parse_outcome` is `Clean` and `calls` is `[]` — byte-identical to a symbol with
  no callers. VB.NET is the sole exception and only by accident: it takes the `Fallback` branch,
  which `liveness.rs:338` translates into an explicit "no call extraction" exemption.

- **Open (SC34, 2026-08-17, number corrected 2026-09-04): the call-graph blackout is not limited to the C family.** SC31 fixed the C family, but its root cause was structural: `treesitter.rs` has **9 `calls.push` sites, all inside language-specific arms**, and every other language falls through to the generic arm that emits declarations only. Verified directly on a one-function-calls-another file per language: **Java, C#, Ruby, Swift and PHP each produce `Contains` edges and zero `Calls`**, though each parses cleanly and emits its symbols. Only Python, JS/TS, Go, Rust and (now) the C family have call extraction.

  This is the same failure the C family had, at 5× the scope: `impact`, `trace`, dead-code and the PDG return answers for those languages built on an empty call graph, with no signal distinguishing "no callers" from "callers were never extracted". Dead-code output is the dangerous surface — a symbol with no extracted calls looks exactly like an unused one. Closing it means a call-extraction arm per language family, measured against a control the way SC19 and SC31 were.

- **Closed (SC22, 2026-08-17): shell and SQL are parsed.** Both were reaching `extract_treesitter` already — `detect_language`'s fallback table routes `.sh`/`.bash`/`.zsh` to `"shell"` and `.sql` to `"sql"` — with no grammar arm behind either, so every such file returned `ParseOutcome::Failed` and contributed only a File node (86 shell and 19 SQL files on the personal corpus). Added `tree-sitter-bash` 0.25 and `tree-sitter-sequel` 0.3, both approved as new dependencies.

  Neither language is in the frozen Python 35, so **neither gets a `LanguageSpec`**. `LANGUAGE_SPECS` is a strict pairwise parity contract with the Python registry — `test_phase2_hardening.rs` zips it against a golden generated from `devcouncil.codeintel.languages.registry` — and adding entries there would break the migration's own baseline to gain nothing, since the fallback table already produces the grammar keys.

  SQL needed node-shape support to be worth anything: it parsed `Clean` and yielded **zero symbols**, because the generic extractor knew none of its node kinds. `generic_symbol_kind` now maps `create_table`/`create_view`/`create_materialized_view`/`create_type` to `Struct` (a relation is a named set of typed columns), `create_function`/`create_trigger` to `Function`, and `create_schema` to `Module`. Indexes, sequences, roles, databases and extensions are deliberately excluded — they name no callable and own no fields, so they would add nodes nothing can traverse to. `generic_declaration_name` gained an `object_reference` fallback, since no `create_*` node has a `name` field; it reads that reference's own `name` field so a schema-qualified `analytics.users` is identified as `users` rather than by its qualified text.

  Both extended the seams that already existed rather than adding a parallel SQL path. Verified: shell yields `helper`/`main` as functions; SQL yields `users`/`active` as structs and `tally` as a function, with the index correctly absent.
- Open: non-code files are admitted and permanently recorded as parse failures — on the personal corpus 1,091 JSON, 978 Markdown, 119 YAML, 86 shell, 67 config, 47 HTML, 34 CSS, 19 SQL and 18 TOML files, ~2,460 in all (19% of the index). No grammar exists for any of them, so every build re-records the same failures. The cost is that a *genuine* grammar regression in a real language is invisible against a constant background of ~2,460 expected failures.

- **Closed (SC17, 2026-08-17): two classes of phantom call edge are no longer emitted, and real Go constructor references are recovered.** Found by running the port over a 12,821-file personal corpus and reading the *unresolved* table instead of the passing gates — both defects were invisible to every existing test because each produced a well-formed call that simply never joined.

  **JSX intrinsic host elements.** `jsx_opening_element`/`jsx_self_closing_element` pushed an `ExtractedCall` for every tag. `<div/>` compiles to the string `"div"`, not to a binding, so that edge names a symbol that cannot exist. `div` was the 5th most common unresolved callee at 10,307 rows, `button` 2,280. The call is now gated on the JSX transform's own rule — lowercase `JSXIdentifier` is intrinsic, capitalized is a value, `JSXMemberExpression` (`<Panel.Header/>`) is always a value, `JSXNamespacedName` (`<svg:circle/>`) never is. The `ReferenceKind::JsxTag` reference is still emitted for every tag, so no knowledge is lost.

  **Go composite-literal type expressions.** The literal's whole `type` field text was recorded as the callee, so `[]*genai.Part{...}` produced a call to `[]*genai.Part` — 24,382 such rows. The type is now unwrapped through `slice_type`/`array_type`/`pointer_type`/`map_type`/`generic_type` to the named type it actually constructs, so that literal references `Part`. Fail-closed: a literal whose core is a predeclared type (`[]string{}`, `map[string]int{}`) emits nothing rather than a call to `string`. `split_call_target` gained a `qualified_type` arm so `genai.Part{}` splits into callee `Part` + receiver `genai`, matching how `genai.NewClient()` already split.

  Deliberately **not** folded into `go_type_name`, which answers a different question — "what is this value's type, for receiver dispatch". Unwrapping there would bind a `[]Foo` parameter to `Foo` and let `x.Method()` resolve against `Foo`'s method set, which is exactly the SC9 class of confidently-wrong edge.

  Measured on the personal corpus, before vs after: **unresolved calls 420,698 → 379,873 (−40,825)**, JSX intrinsic callees **15,111 → 4**, Go type-syntax callees **24,382 → 0**, and **resolved edges 852,962 → 853,421 (+459)** — the fix *recovers* real edges rather than only deleting noise. Orphaned call edges remain 0. `EXTRACTION_SCHEMA_VERSION` 15→16 so cached v15 payloads cannot resurrect either class. Caveat on the A/B: the corpus is live and moved by 4 files between runs, so small deltas are drift; the two class-specific counts are exact.

  Both regression tests were verified failing against the reverted tree, reporting exactly the corpus's phantom callees (`["div", "Widget", …]` and `["[]*Hypothesis", "map[string]*Finding", …]`).

- **Closed (SC18, 2026-08-17): a structurally-external call is no longer reported identically to a resolution failure.** `UnresolvedReference` now carries an `UnresolvedClass` of `Builtin`, `External { module }` or `Unresolved`, persisted in a new `generation_unresolved.classification` column (schema v9→v10) and reported per-tier by `devmap build`.

  Both non-default tiers are drawn from **evidence, never from what a name looks like**. `Builtin` means the language specification declares it — Go's predeclared identifiers, Python's `builtins`, Rust's prelude, ECMAScript's globals — so no indexed file can ever declare it. `External` means an import statement in that very file names the symbol and its module resolved to no indexed file; the import *is* the proof. Everything else stays `Unresolved`, so the classifier can only ever over-report a defect, never hide one.

  Two deliberate exclusions, because there is no evidence for them: **standard-library methods** (`unwrap`, `to_string`, `Fatalf` on a `*testing.T`) are library API that no import binds — an inherent method needs no `use` — and **host globals** (`setTimeout`, `fetch`, `console`) are supplied by a runtime this crate cannot observe. Both stay `Unresolved`.

  Kept off `Resolution` deliberately. `Resolution` describes how an *edge* resolved and rides on every `ResolvedEdge`, feeding the sort comparator, the dedup predicate and the determinism digest; this describes why there is *no* edge. Adding a variant there would have perturbed all three for no gain.

  Measured on the personal corpus: of 380,285 rows, **68,814 builtin (18.1%), 135,903 external (35.7%), 175,568 unattributed (46.2%)** — 54% of the ledger is now provably expected, and the actionable tier is less than half what it was. The conservatism is visible in the data: `len` classifies as builtin 20,125 times but stays `Unresolved` for the 1,811 `x.len()` *method* calls, and `unwrap`/`to_string` are never claimed. The v9→v10 migration was verified in place on a real 380k-row database, backfilling existing rows as `unresolved` — which is what they meant, since the classifier had not run on them.

  **What the cleared signal reveals:** the remaining 175,568 are overwhelmingly one class — a method call on a receiver whose type could not be inferred (`t.Fatalf`, `expect(...).toBe`, `path.write_text`, testify's `m.On(...).Return(...)`). That is a known limitation rather than a new defect, and no phantom-name bug of the SC17 kind survives in the top 25. Narrowing it is receiver-type inference for library types, which is the natural successor to this work.

- **Closed (SC25, 2026-08-17): receiver-type inference for library types, and a fourth tier that makes the residual legible.** SC18 left 175,568 rows in the tier reserved for probable defects. Reading them showed one dominant shape — a method call on a receiver whose type was never inferred — and one recoverable subset: receivers whose *declared type* comes from an import.

  Two changes. First, `UnresolvedClass::UninferredReceiver` separates "there is a receiver we could not type" (`expect(...).toBe`, `value.unwrap()`) from "a bare name matched nothing". That is a structural limitation of a syntax-directed extractor, not a defect, and merging the two is what made the defect tier unreadable. Second, a receiver typed by an external import is now classified `External`: `t.Fatalf()` where `t` is a `*testing.T` follows the chain receiver → declared type → the import that binds it → outside the corpus.

  The qualifier had to be captured to make this work. `go_type_name` and `rust_type_name` deliberately return the *bare* name — it is the dispatch key, so `pkg.Widget` must look up as `Widget` — so the module was being discarded. A parallel `TypeQualifier` reference now carries it, on the SC17 principle: same-looking data, different question, separate owner.

  **A real leak was caught by its own hardening test.** The first implementation fell back from the scoped binding to the file-wide one *per slot*. Given `TestOne(t *testing.T)` and `useTracker(t *Tracker)` in one file, `t.RecordMissing()` found no scoped qualifier — `Tracker` is unqualified — and inherited `testing` from the other function, declaring a local type's method external. That is precisely the SC9 defect in a new place. Fixed by making the scoped binding authoritative *as a unit*: if the scope says anything about a receiver, only the scope may speak for it.

  Measured on the personal corpus: unattributed **175,568 → 34,000 (−80.6%)**, external +19,303.

- **Closed (SC26, 2026-08-17): five call shapes recorded a whole expression as the callee — and fixing them recovered 89,356 real edges.** With SC18 and SC25 having cleared the expected rows out, the defect tier finally became readable, and it was full of bugs. This is the payoff the tiering was for.

  - **Rust path calls.** `Vec::new()`, `MyType::create()`, `std::fs::write()` recorded the entire path as the callee — 11,447 rows. `split_call_target` had no `scoped_identifier` arm, the same gap SC12 fixed for `field_expression`. **This was dropping real edges, not just noise:** `MyType::create` is a user-defined associated function, and the graph never had it.
  - **Awaited generic calls.** `await invoke<Raw>('x')` produced the callee `await invoke`. With a type argument present tree-sitter makes the *await expression* the call's `function` field. Plain `await invoke('x')` was always fine, which is why it hid.
  - **JSX member tags.** `<motion.div/>` emitted a call to `motion.div`; it is a member access, so the namespace belongs in the receiver where import evidence can see it.
  - **Turbofish.** `row.get::<_, String>(0)` kept its type arguments in the callee name.
  - **Immediately-invoked literals.** `defer func(){ _ = recover() }()` has no callee identity, and the text fallback turned the literal's body into a callee name.

  Measured: **edges 854,422 → 943,778 (+89,356, +10.5%)**, unattributed **34,013 → 19,300**, `await`-prefixed callees 400 → 0, path callees 11,447 → 0. **Orphaned call edges remained 0 throughout**, which is the load-bearing check: 89k new edges all join to real symbols. Parity unchanged at 20 diffs. `EXTRACTION_SCHEMA_VERSION` 17→19.

  One test changed shape rather than expectation: `a_constructed_value_binds_its_local_name_in_every_grammar` pinned the Rust constructor reference as `Worker::new`. Verified at the resolver level that this is an improvement, not a regression — `worker.go()` still dispatches to `Worker.go` at confidence 1.0 *and* `Worker::new()` now resolves to `Worker.new`, an edge that did not previously exist. That assertion is now a resolver test, which is the level that actually matters.

- **Closed (SC27, 2026-08-17): the size gate measured a state no running repository is ever in.** Found by stress-testing rather than by a failing gate. The live DevCouncil store had reached **188 MiB for 1,113 files** while every green run of `verify.sh` reported 73 MiB — and 188 MiB was *over* the 173 MiB budget the gate was checking against.

  Not bloat: `freelist_count` was 0, so the space is real data. A store retains `GENERATION_RETENTION` (2) generations and each carries its own extraction payloads, so `generation_files` alone was 87 MiB. Retention itself is sound — a repeated-build stress at repository scale plateaus flat (29 → 54 → 48 → 48 → 48 → 48 MiB, 2 generations). The defect was in the *measurement*: the self-build gate built once into a fresh database, so it compared a **cold, single-generation** size against a budget, and production never sits there.

  The first fix attempt was wrong in an instructive way: doubling `DB_SIZE_GATE_PER_FILE` to 320 KiB would have made the cold-build check 2× looser to accommodate a state it was not measuring. The constant's own pinning test caught the change, which is what that test exists for. Corrected approach — **gate both states, each against the budget that describes it**: the self-build gate keeps comparing a cold build to the per-generation 160 KiB, and the growth gate now bounds its plateaued size against `db_size_gate_steady_bytes` (per-file × retention). Nothing was loosened; a missing check was added.

  A second build in the self-build gate was also tried and abandoned: with no source changes the build is correctly a no-op, so it never reaches two generations there. The growth gate already churns a file between builds, which is why it is the right home for this.

  `DB_SIZE_GATE_RETAINED_GENERATIONS` mirrors `GENERATION_RETENTION` because `devmap-extract` cannot depend on `devmap-store`. Per the SC15 lesson about policy numbers living in two places, `retention_matches_the_store_constant` asserts they agree.

- **Closed (SC28, 2026-08-17): two concurrency defects that made a cold start unreliable for any second process.** Every gate to this point drove the binary one process at a time, so nothing had ever established that two builds racing produce a valid store. DevCouncil runs a watcher, an editor hook and a developer's own `dev map` against the same file, so this was live exposure, not a hypothetical.

  Reproduced immediately by the first concurrency test written: **four racing builds against a fresh store failed 100% of the time**, in two distinct ways.

  1. **Migration race.** `migrate` read `PRAGMA user_version` *before* opening its write transaction, so every process observed 0 on a new store. `Immediate` serialises writers but does not refresh a stale read: the winner created the schema, and the losers then reached an unconditional `ALTER TABLE generations ADD COLUMN repo_root` and died with `duplicate column name`. Fixed by re-reading the version inside the write lock and continuing down the chain from what the winner actually left; the `ADD COLUMN` is now probed like every other one.
  2. **WAL enable race.** `PRAGMA journal_mode=WAL` needs an exclusive lock and SQLite returns `SQLITE_BUSY` for it **without consulting the busy handler**, so the 5-second `busy_timeout` never applied to that one statement — it surfaced as a bare `database is locked`. Losing that race is not an error, since the winner sets WAL for everyone, so a busy result now re-reads the mode and succeeds if the database is already there, with bounded retries otherwise. The final failure is still propagated: silently falling back to rollback-journal mode would make every reader block on every write, a performance cliff nobody would trace back here.

  Verified with the release binary at higher contention than the test uses: **30 racing cold-start builds, 0 failures**, ending in `integrity_check ok`, `journal_mode wal`, schema v11. Under sustained contention — 18 real concurrent writes across 3 churn rounds — integrity and `foreign_key_check` stay clean, retention holds at 2 generations, and **orphaned call edges remain 0**, which is the check that matters: SC9/SC10 showed a store can pass every structural check while carrying edges that name symbols which do not exist.

  `crates/devmap-cli/tests/test_concurrency.rs` drives real OS processes rather than threads, because in-process threads share a connection pool and would test something easier than what actually happens. It runs under `cargo test`, so it covers Linux, macOS and Windows in CI — better reach than the Unix-only shell gates.

- **Cumulative effect of SC18 + SC25 + SC26 on the unresolved ledger:** 380,285 undifferentiated rows → **19,300 in the tier that means "possible defect"**, a 94.9% reduction, with every remaining row in an evidence-backed bucket. Verified on the corpus rather than asserted: `builtin` and `unresolved` are **100% bare calls**, `uninferred_receiver` is **100% receiver-bearing**, across all 372,498 rows.

- **Final state of the personal corpus (12,831 files, 12 languages), measured on a quiet machine:** 116,933 symbols, **944,112 edges**, 59 s wall. Two independent builds produced **identical digests for nodes, edges and the classification ledger**, so the new `TypeQualifier` references, the four-tier classification and the stored receiver are all deterministic. `integrity_check` ok, zero foreign-key violations, and **zero orphaned call edges** across 944,112 edges. Residual phantom-callee shapes: path callees 11,447 → 10, `await` callees 400 → 0, type-argument callees 558 → 130.

- **Open, and reported rather than buried: peak memory rose ~30% with SC26.** 2.5–2.7 GiB before, **3.40–3.55 GiB** after, for +10.5% edges. This is consistent with the SC3/SC4 model — memory tracks Σ N², the squared ambiguity fan-out, not the edge count — and splitting Rust path calls introduces many call sites whose bare callee (`new`, `from`, `write`) has a large candidate set. It is well inside the regime SC3 left (10.45 GiB before that fix) and the RSS gate is calibrated on a repository 11× smaller, so no gate covers this corpus. Reducing it means attacking the fan-out, which SC4 examined and deliberately declined to collapse.

- **Known and deliberately excluded from `Builtin`:** host-environment globals (`setTimeout`, `clearTimeout`, `URL`, `AbortController`, `requestAnimationFrame`) are ~1,200 of the residual. They are supplied by a browser or by Node, not by ECMAScript, so whether they exist depends on a runtime this crate cannot observe. Classifying them would mean asserting a fact not in evidence.

- **Historical statement of SC18, retained because it motivated the design:** After SC17 the unresolved table is 379,873 rows and is now dominated by names that are *not* defects: Go builtins (`len` 21,944, `append` 11,143, `make` 5,327), Go stdlib and test methods (`Equal` 14,235, `TrimSpace` 10,560, `Fatalf` 9,183, `Errorf`, `Background`, `NoError`), Rust std (`to_string`, `unwrap`, `Some`), Python builtins (`print`, `str`, `setattr`), and third-party/framework surfaces (`useState`, `toBe`, `toContain`). None can ever resolve, because no indexed file declares them. Recording them as `Unresolved` alongside genuine failures means the signal cannot be used to find resolution defects — SC17's two classes were only found by hand-reading the top-N. This needs an `External`/`Builtin` classification distinct from `Unresolved`, which is a consumer-visible contract change (`generation_unresolved` semantics and `dev map` output) and so is recorded rather than taken unilaterally.

### Phase 3 — resolution and analysis: partially complete

- Confidence-typed resolution, ambiguity preservation, named-import aliasing, Python-only stdlib guard, Go package-star topology, dead-symbol parse-error exemptions, weighted connected communities, and bounded deterministic traversal have local tests.
- Unique-global resolution is language-family scoped. Ambiguous globals preserve every candidate at speculative confidence; duplicate same-file methods do not become deterministic bare calls.
- Constructor assignment, not a later method name, owns receiver-type inference. Route handlers prove same-file, import-scoped, or unique-family ownership instead of selecting the first indexed name.
- Receiver dispatch resolves unique type-qualified methods across files and refuses ambiguous constructor types. Logical graph edges are deterministically sorted and deduplicated; Rust `self::` and `super::` module probes are covered.
- Ambiguous-only inbound calls remain visible as `only_ambiguous_callers` dead candidates at confidence 0.4 rather than being mislabeled confident dead.
- Impact no longer traverses upward through `Defines`; max depth and node caps are enforced.
- Open: full import capability inventory (tsconfig/project references/workspaces, Go module prefix, Rust module probing), dynamic dispatch, MRO, process precomputation, and full parity/precision ratchets.
- Open (SC4): ambiguity preservation materializes the full call-site × candidate cross-product — 80.2% of `Calls` edges (64.2% of all edges) on an external Go-dominant corpus. **Correction:** this ledger previously called the fan-out "the direct cause of the memory and build-time scaling in SC3". It was not. The cause was the *per-edge clone* of the candidate list, now fixed under SC3 with the fan-out left intact; collapsing the fan-out is no longer needed for memory. **Decision: do not collapse to a candidate set.** An audit of every consumer found the change would alter consumer-visible answers — `impact` on candidates 2..N would return zero inbound edges, and `src/devcouncil/verification/checks/dead_symbols.py` reads exactly that emptiness to declare a symbol dead, so `dev verify` would start reporting live Go and Rust methods as dead. It would also permanently break `tools/parity/parity_harness.py` against the Python-derived `testdata/golden/*/edges.json`, and blind the determinism digest to ~64% of resolution output. Capping fan-out is rejected outright: it manufactures false-positive dead code and violates PHASE1_CONTRACT.md:17 ("never present a capped sample as complete coverage"). The remaining cost of the fan-out is storage-side only and modest — edges are 84 MiB of a 525 MiB database.
- **Closed (SC6, 2026-08-15): runtime and harness entry points are no longer confident-dead.** Exemption was file-level only, so a symbol could only be spared by its whole file carrying a `WiringKind`. `WiringAnnotation.target_symbol` already existed as the per-symbol seam but every producer wrote the file path into it, and liveness never read it; `WiringKind::StructuralExempt` was matched but produced nowhere. The extractor also captured no attributes at all — in tree-sitter-rust `attribute_item` is a *sibling* of `function_item`, so `is_exported` (`text.starts_with("pub")`) could not see `#[test]`. Fixed by emitting per-symbol annotations from the language arms of `extract_node` with `target_symbol` set to the qualified name, adding `WiringKind::RuntimeEntryPoint`, and splitting liveness into file-scoped and symbol-scoped exemption sets. Now exempted: Rust test/bench/ctor/no_mangle/proc-macro/allocator attributes, `fn main` under `src/bin`, `examples`, `benches`, `tests`, and trait-impl methods (structurally unable to be `pub`); Go `func init()` and `func main()` in `package main` regardless of filename, plus same-file interface implementations; Python `test_*`, `pytest_*` and unittest/pytest lifecycle hooks; JS/TS class-method lifecycle hooks. A separate real bug was fixed alongside: `is_exported` for `method_definition` checked `node.parent()`, which is `class_body` and never `export_statement`, so every method of an exported class was false-dead. Measured on the external corpus: confident non-exempt dead **636 → 214**, Go `init` **4 → 0**, Rust `test_*` **29 → 0**, with `EXTRACTION_SCHEMA_VERSION` bumped 7→8 so cached v7 payloads cannot resurrect the false positives. `wiring.rs` gained 12 unit tests where it previously had none.
- **Closed (SC6a, 2026-08-15): Go interface implementations are exempted across files within a package.** Interface method specs and method arities now travel on the `Extraction` (`go_interface_methods`, `go_method_params`) and are joined in `liveness.rs` by package key — parent directory plus package clause. Extraction and analysis call one shared matcher and emit one shared reason string, so a single match cannot produce two verdicts; the old extraction-time rule was replaced, not duplicated. `EXTRACTION_SCHEMA_VERSION` bumped 8→9. The matcher verifies method kind, name, declared parameter arity, and same-package declaration; it does **not** verify parameter or result types, the receiver type, that any value is ever assigned to the interface, or embedded interfaces (`type_elem` produces no spec — a pre-existing gap). Package scope is exact rather than merely conservative: Go qualifies *unexported* interface method names by declaring package, so no outside type can satisfy them, and exported methods are already exempt via `is_exported`. Corpus-wide scope would buy nothing and exempt every same-named method everywhere. Accepted residual: a same-arity same-named method in a package that declares such an interface is exempted even if unrelated — unavoidable without a type checker, and it errs toward missing a finding rather than proposing a deletion that breaks the build.
- **Honest reading of SC6a's real-corpus impact:** on the 1,505-file Go orchestrator it rescued **nothing** from the confident-dead tier — that set is byte-identical at 35 rows before and after. Interface-reason rows went 144 → 551, but 407 of those were already exempt via `is_exported` and only had their reason sharpened. The false-positive class is real and reproduced in a fixture (the sealed-interface marker pattern), but it does not occur in this corpus. The demonstrated value here is that the change is provably non-regressive across 1,505 real files.
- **Closed (SC9, 2026-08-15): receiver-type inference no longer collides, and the Go call graph is now joinable.** Two independent defects, one root: Go callables reported the bare `file::method` as their identity in `caller_symbol`, and the receiver binding was emitted against the `method_declaration` node itself so its `enclosing_symbol` walked *past* the method to file scope and came back `None`.

  Fixes, both in `crates/devmap-extract/src/treesitter.rs`: `enclosing_callable_qualified` now derives a Go method's owner from its receiver (`go_receiver`), because a Go method is owned by a field of its own declaration rather than by an enclosing node, so the ancestor walk could never find it; and the receiver binding derives its scope from the method **body**, which stops the walk at the method and yields exactly the string a call inside it reports as `caller_symbol` — the two now join. `crates/devmap-resolve/src/resolver.rs` keys receiver bindings by `(file, enclosing_symbol, var)`, falls back to the file-wide map only when unambiguous, and **poisons any key claimed by two types** rather than letting last-write-wins hand back the loser of a race. `EXTRACTION_SCHEMA_VERSION` 9→10.

  Measured on the external corpus: confident non-exempt dead **214 → 210**, both `readList` methods correctly live, and all six calls resolving to their own type at confidence 1.0. **Orphaned call edges — edges whose `source_symbol` matches no node's `qualified_name` — fell from 74,312 to 44 (99.94%).** That second number is the larger result: 18% of all call edges were previously unjoinable to the symbol they came from, so any traversal *out of* a Go method node silently missed its calls. Parity against the Python-derived goldens is unchanged: 18 diffs before, 18 after, zero new. Regression coverage in `test_phase3_hardening.rs`, both tests verified failing against a fully reverted pre-fix tree (`uniqueToRedis` → `MemStore.readList` at 1.0; source symbol `store.go::caller` instead of `store.go::Widget.caller`).

- **Closed (SC10, 2026-08-15): the call graph is now fully joinable — zero orphaned edges.** The residual 44 were the same asymmetry as SC9 in a different place: `callable_binding_name` named call scopes for two node kinds the symbol emitter ignored. `generator_function_declaration` (`function*` / `async function*`) was simply never extracted, though it is as much a declaration as `function_declaration` and is frequently an exported API — now emitted. And a *named function expression* was named by its own internal name, which is only in scope inside itself and is deliberately not a symbol; `dns.lookup = function patchedLookup() {…}` therefore attributed its calls to `file::patchedLookup`, matching no node. Expressions are now named by the binding they are assigned to, falling through to the enclosing symbol otherwise — exactly as unnamed arrows already behaved, and consistent with the emitter, which names `const f = function inner() {…}` as `f`.

  Measured on one frozen corpus with both binaries: **orphaned call edges 44 → 0**, symbols 33,064 → 33,068 (the four generator declarations that were missing), confident non-exempt dead unchanged at 168 — no new dead-code candidates. Parity unchanged at 18 diffs, zero new. Regression test in `test_phase3_hardening.rs`, verified failing pre-fix on all three shapes (`patchedLookup`, `resolveOnce`, `streamIt`).

- Historical detail of SC9, retained because the diagnosis path matters: **receiver-type inference collided on receiver variable name, producing confidently WRONG edges.** Not merely a failure to resolve. `receiver_types` is keyed `(file_path, var_name)` at `crates/devmap-resolve/src/resolver.rs:209` with last-write-wins, and read back at `:321`. Two types in one file that share a receiver variable name — `func (s *A)` and `func (s *B)`, entirely idiomatic Go — collide on the key `file:s`, so **every** `s.method()` in the file resolves against whichever type was indexed last, and the result is stamped `Confidence::DETERMINISTIC` (1.0). A real method with real callers is then reported confidently dead while a fabricated 1.0-confidence edge points at its namesake. High-confidence edges are exactly what consumers treat as proof: `src/devcouncil/verification/checks/dead_symbols.py` reads inbound `impact` items to decide liveness.

  Reproduced in 16 lines (`RedisStore`/`MemStore`, both using receiver `s`): both `s.readList()` calls resolved to `MemStore.readList` at confidence 1.0, and `RedisStore.readList` was reported dead at 0.9 non-exempt. Confirmed in the wild at `backend/go_orchestrator/internal/wisdev/journal_shared.go` — `RedisJournalStore.readList` is called at lines 433, 444, 466 and 501 and reported confidently dead, with all four edges pointing at `InMemorySharedJournalStore.readList`. Note the earlier fixtures that used *distinct* receiver names (`s` and `m`) resolved correctly, which is why this hid: the defect needs the name collision, not merely duplicate method names.

  A first attempt scoped only the resolver side and was **measured, found to be a lateral move, and reverted** — confident dead unchanged at 17, with the false positive merely moving from one type to the other — because the scoped half never fired while `enclosing_symbol` was `None`. That reverted state is what the diagnosis above describes; the closure at the top of this section is the completed fix, which required the extractor change as well.
- **Open (SC6b, refined by reproduction): Rust trait method *signatures* are not extracted, and what looks like the signature is actually the implementation.** A defaulted trait method (`fn farewell(&self) -> String { … }`) is a `function_item` and *is* extracted; a bare signature (`fn greet(&self) -> String;`) is a `function_signature_item` and is not. The `Greeter.greet` symbol that appears in its place comes from the `impl` block, because `enclosing_type_name` resolves an `impl_item` to its **trait** rather than its type.

- **Closed (SC11 + SC6b, 2026-08-16): Rust trait identities no longer collide, and a trait's declared surface exists.** Two coupled defects, fixed together because neither could land alone. Impl methods were qualified by the **trait**, so every implementor shared one identity — a single `impl` overriding a *defaulted* method already produced `Trait.method` twice, two nodes no edge could tell apart. And a trait's bare signature (`fn greet(&self) -> String;`) is a `function_signature_item`, never extracted, so the declaration was invisible while implementations pointed at nothing; adding it while impls still took the trait name would only have added a third source of the same name.

  Impl methods are now qualified by their type (`enclosing_type_name` prefers the `impl_item`'s `type` field), and signatures are extracted and own `Trait.method`. Verified: a trait with a signature and a default, implemented by two types with one override, yields six distinct symbols and **zero duplicate identities**. The trait relationship is not lost — `rust_method_structural_reason` reads the `trait` field directly, and both the declaration and the implementation keep their structural exemption. Its reason string now distinguishes a *declared* trait method from a *defaulted* one rather than calling every signature a default. `EXTRACTION_SCHEMA_VERSION` 11→12.

  **This is a deliberate divergence from the Python incumbent**, which also names these by the trait; recorded in DIVERGENCES.md. Cost: 1 net parity diff (18 → 19; 3 new, 2 resolved), where `rust_app` reports the same dead symbol under a different name — and the symbol in question is at confidence 0.3 exempt, not a confident false positive. Corpus: 279 tests pass, confident non-exempt dead 167, orphaned call edges still 0, all gates green.

  `test_wiring_exemptions` needed strengthening rather than updating: under the new naming it kept passing while asserting only the *declaration's* exemption, which would have let the implementation silently become confidently dead. It now asserts both, and that the two do not collapse into one identity.

- **Closed (SC13, 2026-08-16): calls inside Rust macro bodies are recovered.** tree-sitter parses macro arguments as an unstructured `token_tree`, so `println!("{}", s.work())` flattened to loose identifier tokens and the call disappeared from the graph — absent, not weakly resolved. Any symbol reached only from inside a macro read as dead, and in real Rust (`println!`, `assert!`, `format!`, `vec!`) that is a large share of calls.

  The token text is now re-parsed with the real grammar rather than pattern-matched out of the flat token stream, so nesting, chained calls and closures come out correctly; the arguments are wrapped in a synthetic call to make a comma-separated list a valid expression position. Nested macros are probed through a bounded work queue (`assert_eq!(a, format!("{}", b.c()))` reaches two levels down) rather than by recursion, which keeps the thread-local probe parser borrowed for one parse at a time. **Fail-closed:** a token tree that is not a valid argument list — a declarative macro body, a pattern macro — yields nothing rather than a guess. Measured on the external corpus: **Rust call edges 1,482 → 1,801 (+319)**.

- **Closed (SC14, 2026-08-16, partially — 182 → 76 duplicate identities): nested definitions are scoped to their enclosing function.** A function, struct or class declared inside a function body was qualified `file::name`, so N same-named local definitions in one file collapsed to one node: `run` declared inside 15 different Python test functions, `struct Out` inside three different Rust functions. `scoped_qualified_name` now qualifies such a definition by the callable that contains it.

  The first attempt introduced **1,519 orphaned call edges** — the SC9/SC10 failure repeated: symbol identities moved while `enclosing_callable_qualified` still built the old flat scope string, so calls made inside nested functions named a source no node had. Making the two agree (the scope builder now recurses through the same helper) brought orphans back to **0**.

  **Remaining, and deliberately not forced:** 76 duplicates persist in two shapes. 47 come from definitions inside *anonymous* callbacks — `it('…', () => { const start = … })` — where `callable_binding_name` intentionally keeps walking past unnamed arrows so `useEffect(() => persist())` attributes to its outer named function; scoping these needs a stable identity for an anonymous callback, which position would not give. 29 come from methods of nested types, where the method takes the type's *bare* name. A fix for the second was attempted **three times and reverted three times**, each on measurement:

  1. Scoping the member's identity alone: duplicates 76 → 49, but **119 orphaned edges** — identities moved, the scope string did not.
  2. Same, after the first revert, believing the scope builder was already consistent: identical outcome.
  3. Moving the identity *and* `enclosing_callable_qualified`'s type branch together, which was supposed to be the fix for (1): duplicates 76 → 49, **29,874 orphaned edges**, suite down to 205 tests. Worse, not better.

  Three measured failures against the same sub-problem is itself the finding. Member identity and call-scope identity are derived by two independent walks (`enclosing_type_name` for the owner, `enclosing_callable_qualified` for the scope) that agree today only because both flatten to a bare type name. Making one hierarchical desynchronises them in ways that are not local to the edit — the third attempt changed both and still diverged, which means the coupling runs through `scoped_qualified_name` calling back into `enclosing_callable_qualified`. **This needs a single canonical owner for "what is this symbol's identity", derived once and consumed by both paths, rather than a fourth patch.** Recorded rather than forced; the tree is left at 76 duplicates, 0 orphans, all gates green.

- One test was updated rather than deleted: `arrow_function_calls_are_attributed_to_the_binding_name` asserted the old flat `::handleClose`. The arrow is now `Modal.handleClose`, scoped to the function declaring it — strictly more precise, and its intent (the call must not collapse onto `Modal`) still holds. The assertion now also requires the edge source to match a real symbol, so the identity and the scope cannot silently drift apart again.

- A second watcher flake was fixed, same root cause as the first: `watcher_edit_reaches_durable_generation_and_ipc_query` wrote once and polled for 8 s, so a write landing before the platform watch registered was unrecoverable. It now rewrites per round and stays quiet longer than the debounce, since retrying faster than `DEBOUNCE_SETTLE` starves the callback it is trying to trigger. Four runs under concurrent build load: 3.3–4.4 s, previously a hard timeout.


- **Closed (SC12, 2026-08-15): Rust method calls resolve, and a parameter's declared type is a receiver binding.** Two defects compounded. Rust `call_expression` handling recorded the *whole dotted chain* as the callee with `receiver_expr: None` — `s.read_list` rather than callee `read_list` on receiver `s` — because `split_call_target` had no arm for Rust's `field_expression`, so receiver-type resolution could never fire for Rust at all. And receiver typing only ever learned from constructor assignments, leaving a function that operates on a value it did not construct with no binding for its own parameter. Together these meant such a call did not resolve *weakly*; it produced **no edge at all** and vanished from the graph.

  Fixed by teaching `split_call_target` about `field_expression`, and by binding each Rust/Go parameter name to its declared type via `param_type_bindings` — scoped to the enclosing function, applying the SC9 lesson so two functions sharing a parameter name cannot collide. `EXTRACTION_SCHEMA_VERSION` 10→11.

  Measured on one frozen corpus with both binaries: **Rust call edges 953 → 1,482 (+55%)**, total edges 215,297 → 216,184, confident non-exempt dead 168 → 167, orphaned call edges 0 before and after. **Parity unchanged at 18 diffs, and all 277 workspace tests pass** — unlike SC11, this one costs nothing. Regression test verified failing pre-fix on the first assertion: the callee arrives as the whole chain.

### Phase 4 — store and daemon: partially complete

- Schema version 7 (`CURRENT_SCHEMA_VERSION`, `schema.rs`) persists file parse state, path-safe extraction payloads, nodes, edges, dead rows, analysis metadata, capped longitudinal build history, and the absolute repo root a generation was built from. (Earlier revisions of this ledger said "version 6"; v7 added `generations.repo_root` so query processes stop resolving repo-relative node paths against their own working directory.)
- Initial schema creation and v3→v4→v5→v6 migrations are transactional with `user_version` last; injected initial-schema failure rolls back prior statements. Future schemas are rejected before schema mutation.
- Queries use the durable generation and remain functional after source deletion.
- Daemon batches read only claimed paths; unclaimed changes cannot leak into a generation. Deleted paths are explicit.
- Unix IPC uses owner-only `0600` sockets; repository-scoped defaults live in a `0700` runtime directory, and overlong portable Unix paths fail before bind. Windows has a named-pipe implementation. Requests are newline-framed, versioned, size-limited, time-limited, and return structured errors.
- Retry delay grows exponentially; status reports pending, quarantined, and degraded counts.
- The live watcher filters devmap/VCS/build namespaces and repository ignore rules, caches ignore verdicts with explicit rule-event invalidation, debounces/deduplicates events, and has explicit shutdown ownership. A real watcher test proved `.devcouncil/.../index.sqlite-wal` cannot feed back into the queue.
- File rebuilds use a bounded stat-read-stat loop; startup performs a content-hash sweep for changed/new/deleted sources after the watcher starts. Real edit→durable-generation→Unix-IPC search and SIGKILL→stale-socket→restart/replay tests pass.
- Daemon cancellation aborts IPC and maintenance children and removes the Unix socket. Busy truncate checkpoints fall back to passive handling.
- One bad path is isolated from valid siblings in the same daemon batch. SQLite connections explicitly enable foreign keys and a 5-second busy timeout; the WAL reader test now uses an independent reader while a separate writer holds an uncommitted transaction.
- Real DevCouncil release build (three clean DBs): 0.7347–0.9040 s; 1,076 files, 11,358 symbols, 36,563 edges; database 50.23–50.25 MiB (under the 60 MiB gate). This is the only repository the build has ever been gated against; see the stress-test section below for behaviour on a 4.4× external corpus.
- Open: B3 is not closed. The current carry-forward schema still writes O(repository size) membership rows for a one-file edit; the 10k/<100-row gate is intentionally not claimed. Also open: migration kill-at-every-statement process matrix, 30-minute burst/poison soak, and Windows named-pipe/process CI.
- **Closed (SC1, 2026-08-15): generation retention is now bounded.** Generations were never pruned, so the database grew by O(repository size) per build forever — a regression against the Python incumbent's bounded two-generation retention. Root cause was wiring, not design: `prune_generations_except_latest` (`db.rs:1543`) was already implemented and correct but had **zero production call sites**. It is now called from both commit paths — `devmap-cli/src/main.rs` after `save_generation_with_metadata`, and `devmap-serve/src/daemon.rs` after resync — bounded by `GENERATION_RETENTION = 2` (`schema.rs`). The CLI also calls `vacuum_if_needed` so a pruned database actually shrinks; previously only the daemon's 300 s maintenance loop ever reclaimed space, so a CLI user pruned and saw no change. Two supporting fixes: the prune now reads its candidate list *inside* an `Immediate` transaction (it was a read-then-write across a `DEFERRED` boundary), and it runs an FTS5 `optimize` so deleted postings release their space instead of tombstoning. No schema change was required. Regression coverage: `devmap-cli/tests/test_generation_retention.rs` (4 tests) drives the **binary**, because the two pre-existing prune tests called the function directly and therefore could not fail when the call site was missing — which is how the defect survived.
- Open (SC2, materially improved): the incremental path is still not incremental in *design* — a one-file edit re-runs resolve, analyze and persist over the whole repository — but its cost fell 4.8× as a side effect of SC3/SC3b. Measured on the frozen 4,742-file corpus: a one-line edit went from **110.88 s to 23.11 s**, at 1.20 GiB peak. That moves `devmap serve` from unusable to tolerable at this scale without closing the underlying design gap; a genuinely incremental resolve (rebuilding only edges affected by changed files) is still the B3 redesign and is not claimed.
- **Closed (SC3, 2026-08-15): peak resident memory reduced 85.8%, with byte-identical output.** Measured A/B on one frozen 4,742-file corpus, same host: **10.45 GiB → 1.48 GiB**. Root cause was *not* what this ledger first claimed. Memory does not track edge count; it tracks **Σ N², the squared ambiguity fan-out**. A call with N same-family candidates fans out into N edges, and `resolver.rs` gave each of those N edges its own clone of the N-element candidate list — so one call site cost N² owned strings. Measured Σ N² = 72,611,980 pairs over 62,861 ambiguous call sites (max fan-out 217), at ~112–128 B/pair ≈ 8.5 GiB of the peak, held live across the whole sort. The model `RSS ≈ base(files) + 112 B × Σ N²` predicts five corpora within 1%. Fix: `ResolvedEdge.resolution` is now `Option<Arc<Resolution>>`, so the fan-out shares one allocation. `Arc<T>` delegates `Debug`, `PartialEq` and (via serde's `rc` feature) `Serialize` to `T`, so the sort comparator and dedup predicate are untouched and the result is provably unchanged — confirmed by three digests over the A/B pair: edge set, **edge ordinal order**, and dead symbols all byte-identical. Profiling also refuted three plausible-sounding suspects: extractions are 4.7% of peak, edge endpoint strings 1.2%, and persist adds 0.000 GiB; `RAYON_NUM_THREADS=1` changes peak by 0.2%, so parallelism is not a factor.
- **Closed (SC3b, 2026-08-15): build time cut 4.2×, ordering provably unchanged.** The edge sort dominated the build — 113.5 s of 130 s over ~1.06 M pre-dedup edges. The cause was evaluation strategy, not the comparison itself: the comparator built an **eight-element tuple**, and constructing a tuple evaluates every element up front, so all four `format!("{:?}", …)` calls ran on *every* comparison even though tuple comparison short-circuits at the first difference. The two costly keys sit 5th and 7th and are reached only when the four string keys ahead of them tie, and formatting a `Resolution` serializes its whole candidate list. Rewritten as `cmp(...).then_with(...)` chains, which defer each key until the preceding ones compare equal. The total order is unchanged by construction, and verified: edge-set **and edge-ordinal** digests are identical across pre-fix, `Arc`-only and post-sort builds of one frozen 4,742-file corpus.
- Cumulative effect of SC3 + SC3b on that corpus, same host: peak RSS **10.45 GiB → 1.49 GiB (−85.7%)**, build **124.90 s → 27.46 s (4.5×)**, with `4584916033973865…` edge-set and `46527f781a105157…` ordinal digests byte-identical throughout. 244 workspace tests pass.
- **Closed (SC7, 2026-08-15): the extraction cache is no longer unbounded.** `extraction_cache` is keyed by content hash and had **no `DELETE` statement anywhere in the tree**; `accessed_at` was written on every upsert and never read. Every edit to a file therefore added a row and kept the superseded one forever — measured: five edits to one file left five rows, and the table reached 198 MiB of a 525 MiB database on a 4,742-file repository. `Store::prune_extraction_cache` (`db.rs`) now drops entries no retained generation references, wired into both commit paths after the generation prune. Eviction is by **reachability, not recency**: an LRU policy over `accessed_at` would be actively wrong here, because a file untouched for months has the oldest timestamp and simultaneously holds the entry the next build needs, while the rows worth dropping are superseded versions of files being edited right now. Regression coverage: `superseded_extraction_cache_entries_are_evicted`, verified failing against a pre-fix tree that had the generation prune present, so the cache fix is isolated.
- **Closed (SC3c/SC5, 2026-08-15): the gates that would have caught SC1, SC3 and SC7 now exist.** `verify.sh` grew from 6 steps to 7. Added: a **measured peak-RSS gate** (2 GiB) — an unavailable reading now fails the step rather than passing silently, because a check that could not run must never report what a passing check reports; and a **growth gate** that builds five times into one store and fails if generations exceed the retention bound or if the database is still growing between builds 3 and 5. Every prior gate built exactly once into a fresh database, which is precisely why two unbounded-growth defects survived every green run. The size budget was recalibrated from an unsourced 80 KiB/file to a measured 160 KiB/file: cold-build cost is ~81 KiB/file on DevCouncil and ~113 KiB/file on the Go-heavy external corpus, and the old figure had gone red on DevCouncil itself (86 MiB against an 84 MiB allowance) independently of any change in this session — verified by A/B, pre-fix 90,279,936 bytes vs post-fix 90,247,168.
- **Mutation testing on the retention and reclamation code reached a 100% kill rate (19/19), from 5/20.** Progression, each step driven by a mutant that survived: 5 caught → 8 (running the *workspace* test suite instead of one crate's) → 12 (a vacuum reclamation test) → 19 (extracting the vacuum policy and deleting a dead guard). Two of those four steps changed production code, which is the point — mutation testing found a genuinely untestable predicate and a genuinely unreachable branch, not just missing assertions. Details below.
- **Mutation testing found real holes (2026-08-15).** `cargo-mutants` 27.1.0 was installed under decision 5 and run against the retention and reclamation code. First lesson is about the harness, not the code: `cargo mutants -p devmap-store` runs **only that crate's tests**, so the `devmap-cli` integration tests that actually exercise the wired build path never participated — 15 of 20 missed. Re-run with `--test-workspace=true`, the same 20 mutants score 8 caught / 12 missed, and `prune_extraction_cache -> Ok(0)` flips from missed to killed. A mutation score computed at the wrong scope understates coverage and would have sent the next reader hunting for nonexistent test gaps. Real holes it did find: `vacuum_if_needed` was **wholly replaceable with `Ok(())`** with nothing noticing, and both of its threshold comparisons were mutable in every direction — pruning only moves pages to the freelist, so a dead vacuum means a correctly-pruned database that never shrinks. Closed by `vacuum_returns_freed_pages_to_the_filesystem` (`store_hardening.rs`), which asserts page count and freelist both fall. Two findings then required production changes rather than more assertions. First, `vacuum_if_needed`'s threshold survived every attempt to test it through its side effect: below the ratio, "declined to vacuum" and "vacuumed but reclaimed nothing" produce identical page counts, so `&&`→`||` and `/`→`*`/`%` all lived. The decision is now a pure `Store::should_vacuum(freelist, pages)` beside a named `VACUUM_FREELIST_RATIO`, asserted directly — the effect and the policy are separately wrong-able and only the policy is cheaply observable. Second, the `pruned_count > 0` guard around the FTS `optimize` was **provably always true**: the early return above it leaves `gen_ids.len() > keep_generations`, so the loop always deletes at least one generation. No test could distinguish its branches because one branch is unreachable; it was deleted rather than tested. A full workspace mutation run across all crates is still **not** claimed.

- **Mutation testing extended to `devmap-analyze` (2026-08-16): 202 mutants, 133 caught, 52 missed, 13 unviable, 4 timeouts.** Survivors clustered in `traversal.rs` (13), `pdg.rs` (7) and `liveness.rs` (7). Two of those files had **no test module at all**, which is why whole functions were replaceable with a constant.

  The sharpest finding was `traversal.rs::is_symbol_node`, replaceable with both `true` and `false` and with every `&&` and `!` in it mutable — undetected. That function is the guard that stops `impact Type.method` from following file-level topology upward and returning every importer of the package: the documented `segment` flood. Always-true suppresses legitimate file-level impact; always-false reopens the flood. It now has direct table-driven coverage, plus a traversal test asserting that reverse impact from a *symbol* refuses `Imports`/`MemberOf` while the same edges are still followed for a *file* start — the guard is scoped, not a blanket suppression. Depth and node caps are asserted too.

  In `liveness.rs`: `parent_dir` was replaceable with a constant, which would make every Go file in a repository look like one package and silently disable dead-method detection language-wide. The underscore-private skip and the parse-error span overlap were both fully mutable; the overlap comparisons are now pinned as half-open at both ends, since `<`→`<=` would exempt any symbol merely *adjacent* to a syntax error and quietly suppress real findings. One clause was deleted rather than tested: `sym.name.starts_with("__")` is subsumed by the single-underscore check — dead code no mutant could kill.

  Workspace tests 279 → **289**, and re-measurement of those two files reached **81 mutants, 75 caught, 0 missed, 6 unviable — a 100% kill rate**, from 20 missed.

  Getting the last three took reading *why* they survived rather than adding assertions, and all three were fixture flaws rather than missing tests. `liveness.rs:191` is the disjunction inside `is_ambiguously_called`, a different property from the `is_called` one already covered — collapsing it loses the `only_ambiguous_callers` tier, reporting a symbol whose only callers are ambiguous as *confidently* dead, exactly the mislabelling that tier exists to prevent. `traversal.rs:87`'s priority arm was unobservable because the fixture's target names happened to sort in the same order as their priority; discriminating it needs names where alphabetical and priority order disagree. `traversal.rs:146`'s edge budget never bound because the fixture had one edge per node, so the node cap stopped the walk first — several edges onto the same targets are needed before an off-by-one in the edge budget can show at all.


- **Mutation testing extended to the query surface (2026-08-15): 289 mutants, 129 caught, 90 missed, 70 unviable — a 59% kill rate.** This is the first coverage measurement outside retention, and it found the query predicates almost entirely unpinned. The composed end-to-end tests in `query_match.rs` pass while every helper beneath them is replaceable: `path_matches -> true`, `has_source_extension -> false`, `split_qualified -> None`, and both `||` in `looks_like_path` flipped to `&&`. A composed assertion cannot pin which branch produced it — an always-true `path_matches` still yields the right answer whenever the symbol half of the conjunction is also true. `snapshots.rs::is_public_symbol` was worse: **every** per-language match arm could be deleted undetected, because the surrounding tests only used inputs where the arm and the `_` fallback happen to agree. Most serious was `manifest.rs::is_foreign_repo_map -> Ok(true)` surviving — that is the guard which stops devmap overwriting a repo map written by the frozen Python mapper, and nothing pinned either direction of it.

  Tests added, each case chosen to sit where a mutant changes the answer rather than where it does not: direct table-driven coverage of all five `query_match` predicates, per-language `is_public_symbol` cases chosen where the arm and the fallback *disagree*, and both directions of the foreign-map guard including unparseable content failing closed. Re-measured on those three files: **138 mutants, 86 caught, 42 missed** — and every mutant I targeted is now killed. `query_match.rs` has **zero** survivors, and neither `is_public_symbol` nor `is_foreign_repo_map` appears in the missed list any more.

  The remaining survivors clustered in two functions nothing asserted at all, both since covered: `manifest.rs::build_dependents`, whose whole body was replaceable and every comparison flippable while it feeds the manifest's dependency counts — a silent miscount a consumer cannot detect; and `semantic_snapshots`, where the path filter and the truncation arithmetic were both fully mutable, deciding respectively *which* file a caller gets back and whether a partial answer is labelled partial. Also closed: the `!root.is_empty()` guard in `resolve_manifest_output`, which was replaceable with `true` — an empty repo root would then join against a bare relative path and write the manifest somewhere other than intended. Workspace tests 261 → **273**, `verify.sh` green.

  **Confirmed by re-measurement: 138 mutants, 121 caught, 7 missed** — down from 42 missed. Three of the seven are **equivalent mutants**, proven so rather than assumed, and are recorded here so nobody spends effort chasing them:

  - `resolve_manifest_output`'s `!root.is_empty()` guard replaced with `true` is unkillable because `Path::new("").join(p) == p` — verified by execution. This also **corrects a claim made earlier in this session**: an empty repo root does *not* write the manifest somewhere unintended, it resolves to exactly the same path. The guard is clarity, not safety.
  - Both `semantic_snapshots` survivors sit on `truncated: resp.truncated || resp.shown < total`. `budget_take` (`engine.rs:630`) sets `truncated` on every early break, so `shown < total` implies `truncated`: the second clause can never be the deciding one. Flipping `||` to `&&` or `<` to `>` therefore cannot change the result. The clause is redundant defensive code, deliberately kept — removing it would couple `snapshots` to `budget_take`'s internals.

  That leaves **four genuine survivors**, all in manifest functions not yet targeted: `generate_manifest` (`*`→`+`), `generate_lean_manifest_json` (whole body → `String::new()`), `lean_manifest` (`==`→`!=`) and `consumer_manifest_json` (`&&`→`||`). Effective kill rate on killable mutants is 121/125. Not closed.
- **Mutation testing extended to `devmap-resolve` (2026-08-16): 212 mutants, 139 caught, 68 missed.** Every survivor is in `resolver.rs`, concentrated in import and Go-module resolution — `resolve_name_reference` (13), `lookup_in_package` (9), `resolve_go_import` (5), `resolve_import_path` (4), `apply_go_replace` (4). That is the machinery deciding *which file* an import points at, so an error there yields a confidently wrong edge rather than a missing one.

  The `apply_go_replace`/`import_local_name` surface is now **15/15 caught**. Three unasserted properties pinned: `replace` directives resolve **longest-prefix-first** (a shorter prefix winning silently reroutes every import beneath the longer one, and since both targets exist nothing errors); a local target is recognised by `./` *or* `/`, where collapsing that disjunction to `&&` treats every local replacement as a remote module path; and equal-length prefixes resolve to the **first declared**, because relaxing the comparison to `>=` makes the winner depend on module iteration order — two `go.mod` files replacing the same path would resolve differently between builds, which R4 forbids.

  Writing these caught an error in my own test rather than in the code: `apply_go_replace` returns `(original_spec, Some(replacement))` and my first assertions read the original element, so they failed against correct behaviour. Had they happened to pass they would have been vacuous.

  The resolution ladder was then covered too. `lookup_in_package` went from 9 survivors to **0**, pinning that Go same-package lookup is directory-scoped, excludes `_test.go` files, prefers the querying file, and **abstains when two files in a package declare the name** rather than picking one. `resolve_name_reference` went 13 → **4**, pinning cross-family isolation and Go export visibility.

  Two of my own tests were caught being **vacuous** while writing these, both by checking a positive control. A Python ambiguity test asserted "two candidates produce no confident edge" — true, but so did *one* candidate, because a Python bare-name reference does not resolve cross-file without an import, so the assertion could never fail. And the cross-family test asserted "no Go target" while resolving nothing at all. The first was deleted rather than shipped; the second now asserts the Python target *is* reached before asserting the Go one is not. A test whose negative assertion holds because nothing happened is worse than no test, because it reports coverage that does not exist.

  The 4 remaining `resolve_name_reference` survivors are 3 clauses of the Python builtin guard plus the `prefer_types` negation. The guard sits on the unique-global rung, and the same investigation suggests Python references reach that rung rarely or never cross-file — so those may be equivalent mutants over unreachable code rather than missing tests. **Not claimed either way**; distinguishing them needs a reachability check on that rung, which is the same class of work as the SC14 identity refactor.

- **Mutation testing extended to `devmap-serve` (2026-08-16): 130 mutants, 69 caught, 52 missed, 9 unviable.** The survivors were concentrated in guards, and several were security boundaries with no test behind them at all.

  **Path containment.** `Daemon::collect_pending_path` carries three guards — a `..` traversal check and two `starts_with(root)` checks, one for directories and one for files. Mutation deleted the `!` on both `starts_with` guards and flipped the traversal check, all undetected. Inverted, they accept precisely the paths meant to be refused: a watcher event naming `../../etc` would be indexed, and the daemon reads and stores whatever it is pointed at. Now asserted in all three directions with a positive control, so "refuses everything" cannot pass either.

  **IPC request bounds.** All three limits in `validate_request` — query length, token budget, traversal depth — had their comparisons flipped without a failure. These are the only thing bounding the work a caller can request; a comparison that never fires lets one request pin the daemon. Each is now pinned exactly on its boundary and one step past, including the independently-bounded trace destination. The limit *constants* are pinned by value too: mutation changed `4 * 1024` to `4 + 1024` and the bound tests could not notice, because they compare against the constant.

  **Torn reads.** `read_stable_source_with`'s three-way conjunction is what makes a read stable; relaxing it to `||` accepts a source whose length matches but whose mtime moved, and that content is then indexed as if it were the file on disk.

  **Ignore-rule invalidation.** `IgnoreVerdictCache::rule_stamps` was replaceable with `Ok(vec![])`. The stamps are the entire cache-invalidation signal, so constant-empty stamps compare equal forever: an edited `.gitignore` never invalidates a cached verdict and the watcher keeps applying rules the user has already changed.

  **Protocol defaults.** `default_budget` and `default_depth` were replaceable with 0 and 1 — a zero budget returns nothing for every request that omits one, and depth 1 truncates impact to direct neighbours. Both failure modes look like an empty graph rather than a misconfigured default, so they are pinned by value and through deserialization.

  Writing these caught a macOS fixture artifact worth recording: `std::env::temp_dir()` yields `/var/folders/…` while `canonicalize()` yields `/private/var/…`, so a containment positive control fails for a reason unrelated to containment. Had only the negative assertions been written they would have passed against an inverted guard.

- **Mutation testing extended to `devmap-cli` (2026-08-16): 24 mutants, 8 caught, 16 missed — a 33% kill rate, the lowest of any crate.** `main.rs` is 612 lines with no test module; the integration tests in `tests/` drive the binary and assert JSON, so every human-readable emitter could be replaced with `()` unnoticed.

  The one that matters is `emit_truncation`. Replacing it with `()` — and flipping every comparison in its condition — passed, meaning **a capped result is never reported as capped**. PHASE1_CONTRACT.md:17 is explicit that a capped sample must never be presented as complete coverage, and this line is the only thing that tells a CLI caller their answer was truncated. It was untestable as written because it printed, so the policy is now a pure `truncation_line(shown, hidden, total, truncated) -> Option<String>` that `emit_truncation` prints — the same split applied to `Store::should_vacuum`. Both halves of its condition are asserted: `truncated` alone covers a budget stop that hid nothing, `hidden > 0` covers rows dropped without the flag.

  Also closed: `split_csv` (replaceable with `vec![]`, which makes every differential build see no affected paths, and its emptiness filter invertible so only blanks survive) and `ensure_parent` (replaceable with `Ok(())`, deferring the failure to whatever writes the file and reporting a path error instead of a missing directory).

- **`devmap-extract` measured and its survivors closed (2026-08-16): 41 workspace-scoped survivors, all now killed; `model.rs` is at 58/58.** The run covered `languages.rs` (18), `frameworks.rs` (8), `model.rs` (8), `gomod.rs` (5) and `lib.rs` (2). The 41 fell into three shapes.

  *Identity collapse.* `ExtractorId::name` was replaceable with a single constant, because the only test asserted `!name().is_empty()`. That name is the language identity in the extraction cache key, the generation store, and every `language` filter in the query layer — collapsed to one value, every language shares a cache key and one language's payload is served for another's file. The same shape appeared in `grammar_version_for`, where the `rust` and `go` arms were deletable: the existing test compared only Python against JavaScript, so two of five linked grammars had no coverage and would fall to the `unavailable:` fallback. Both are now pinned by table tests that also assert **distinctness** and that every `LANGUAGE_SPECS` entry routes to a pinned extractor, so a new variant added without a name is caught. Four extractors deliberately diverge from the grammar they reuse (arkts on typescript, vbnet on vb, metal on cpp, terraform on hcl), so the mapping is pinned rather than derived.

  *Silently smaller results.* Every `detect_language` fallback arm was independently deletable, as were all six config-filename comparisons — dropping one reclassifies `tsconfig.json` as plain `json`. `MAX_SOURCE_BYTES` was mutable from 1 MiB to 2 KiB and its bound from `>` to `>=`; since an oversized file is *recorded as skipped* rather than failing, the effect is a quietly smaller map. The gate constants were the sharpest case: the existing test in `devmap-cli` reads `DB_SIZE_GATE_FLOOR` and asserts only relations **against itself**, so `60 * 1024 * 1024` mutated to `60 + 1024 + 1024` passed every relation while making the gate fire on any non-empty repo. A budget constant now has one absolute assertion, or every check of it is self-referential.

  *Wrong-but-plausible output.* `Span::line_range` — the query boundary every `file:line` passes through — was replaceable with `(0,1)`, `(1,0)` and `(1,1)`, each of which reports every symbol at the top of its file. `parse_import_bindings` would push `*` as a literal imported name. In `binding_pairs`, `!is_empty() && len == len` mutated to `||` lets `zip` truncate to the shorter side: with two imported names and one local, the second import vanishes and the first binds to the wrong local.

  **Four of my own fixes did not work, and only re-measuring found that.** After writing tests for all 41 I re-ran rather than assuming, and four gomod/model survivors were still alive: my `go.mod` tests covered the *module-prefix* guard at line 47, not the *directory* guard at line 24 (only `./go.mod`, whose parent is a bare `.`, distinguishes it); the replace-spec emptiness guard needed a **quoted-empty** half (`"" => ./x`), since an unquoted blank is caught by the `?` above it and never reaches the guard; and `binding_pairs` needed misaligned lengths, not equal ones. One survivor was closed by **deletion instead of a test**: `line.starts_with("replace (") || line == "replace ("` — the second disjunct is strictly implied by the first and therefore unreachable, so no input could distinguish the branches.

  Two survivors are recorded as **not closed, with reasons**. `is_ignored_path:468` (`&&`→`||`) is a proven equivalent mutant: the inner condition can only hold when the outer one does, since `first` is `norm`'s own first path component. And 20 survivors in `lib.rs`'s gitignore machinery and `gomod.rs`'s filesystem walk appear only under **package scope** — they are killed by the workspace integration tests (confirmed against the `--test-workspace=true` log), so they are untested at unit level, not untested. Scope was changed deliberately: `--test-workspace=true` costs ~145 s per mutant (680 mutants ≈ 28 h), package scope 1–5 s, and package scope is the **stricter** kill criterion.

  Also closed while in these files: four pure functions behind the Go interface-satisfaction exemption. `go_interface_method_matches` was replaceable with `vec![]` (no method ever exempted — reads as a tuning problem, not a bug), and its `kind == Method` filter was invertible, which is the dangerous direction: a plain function that collides with an interface method's name and arity gets exempted, so a genuinely dead function is reported as live-through-an-interface. `go_interface_exemption_reason` could be blanked, making every exemption unattributable — the reader could no longer check *which* interface supposedly reaches the method. And the `retain` in `for_durable_store` had a deletable negation that inverts it: every type annotation and name use dropped while calls are double-counted, with both shapes round-tripping cleanly through the store.

- **`treesitter.rs` measured for the first time (2026-08-16): 363 mutants, and the initial kill rate was 33% — the lowest in the workspace. Nine tests took it to 64%, closing 107 of 233 survivors.** It is the largest file in the port and the foundation every other crate reads, and until this session it had almost no tests.

  I first checked whether the low rate was a scope artifact, because it would have been an easy and wrong conclusion either way. Re-running a sample of four functions under `--test-workspace=true` left `is_user_ident` with 8 survivors, so **the integration tests do not cover this file either** — the gap is genuine at both scopes, not an artifact of package scoping.

  The survivors clustered by shape, and the dangerous direction is the same throughout: **a missing symbol, import or reference is not an error anywhere.** The file still extracts "successfully", the graph is simply smaller, and the symbols that lost their incoming edges drift toward `dead`. That is the one failure mode a code-intelligence index cannot self-report.

  - *`extract_node` (58 survivors, the single largest bucket).* Whole node-kind arms — `import_statement`, `import_spec`, `variable_declarator`, `macro_invocation`, `impl_item` — were deletable, and the export-detection conditions invertible, because no test asserted a *whole* set, only that particular symbols were present. Two golden tests now pin the exact symbol set and the exact import/export set for all four grammars with linked-and-exercised parsers, taking it to **27 survivors**. The qualified names in the symbol golden are load-bearing: `f.rs::Thing.go` vs `f.rs::Doer.go` is SC11's type-first qualification of impl items, and `f.go::Client.Run` is SC9's receiver ownership, without which Go method edges do not join at all. Both fixes were expensive and neither had a regression test pinning its output shape.
  - *Reference classification (`is_defining_name`, `is_call_callee`, `maybe_push_name_reference`, `is_user_ident`, `split_call_target` — 40 survivors, now 8).* These decide whether an identifier is a definition, an invocation, or a use. `is_call_callee` was replaceable with a constant in **both** directions and neither was caught: `false` double-counts every callee as an invocation *and* a name use, `true` erases the non-call uses liveness depends on. Each direction needed its own assertion — a duplicate-count check and a positive control that ordinary value identifiers survive.
  - *The JSX false-error suppression.* `is_benign_jsx_ampersand` exists to stop a known grammar bug marking clean TSX as `Partial`. Two of its operands were mutable, and **my first attempt to kill one did not work** — I reasoned the mutation through and wrote a test that passed under it. Applying the mutation to the source directly showed why: `&&` binds tighter than `||`, so removing the first operand turns the guard into "no braces, no angle bracket" and suppresses *any* ordinary syntax error inside JSX, not just ampersand ones. The discriminator is a plain error inside a JSX expression (`{a b}`), verified to produce exactly one error range carrying none of the four characters the guard tests.

  **Recorded as open, with reasons rather than silence.** 126 survivors remain, concentrated in `extract_node` (27), the Rust macro path (`rust_macro_calls`, `probe_macro_body` — 12), and `rust_method_structural_reason` (4). Two are argued equivalent: `is_ignored_path:468` provably, since the inner condition can only hold when the outer one does; `is_benign_jsx_ampersand:191:50` structurally, because an error node containing `<` starts a JSX element rather than sitting inside one, so it never reaches the parent walk — that is an argument, not a proof, and it is recorded as unresolved. The two `!field.is_empty()` match guards in `split_call_target` need a node whose field text is present but empty, which I could not produce from valid source.

- **`treesitter.rs` survivors driven from 233 to 50 (2026-08-16): kill rate 33% -> 86%, 307/357 killable.** Nineteen tests and three production fixes. Whole clusters are now clean: the Rust macro path (`rust_macro_calls`, `probe_macro_body`, `rust_attribute_paths`, `macro_token_body`) measures **50/50 with zero survivors**, and the exemption cluster (`rust_method_structural_reason`, `go_package_name`, `go_type_name`) is fully closed.

  Three of those tests pinned bounds that were measured rather than assumed. The macro nesting depth is one: five levels resolve and six do not, so the comparison and its `depth + 1` increment are both pinned from the observed boundary instead of from reading the constant. `probe_macro_body` is a pure string function whose eighteen possible return values were *all* substitutable, including `("xyzzy", Some("xyzzy"))` — it is how a call hidden inside `println!("{}", helper())` is recovered at all, and a fabricated return there invents call edges to symbols that do not exist.

- **B3/SC2 closed: resolution is incremental.** Phase timings on 1,610 files, measured rather than assumed: extract 2,327 ms (payload reload, already cached), index 166 ms, resolve 2,145 ms, analyze 835 ms. Resolve and analyze are **54%** of a rebuild — and on a no-change rebuild every millisecond of it reproduces a graph that is already stored.

  A build now compares the scanned tree's per-file content hashes against the committed generation and, when they match exactly, skips resolve, analyze and persist entirely. This is safe for one specific reason: identical inputs give an identical graph, which the determinism gate proves. **Measured 6.87 s -> 1.88 s (3.7x)** on a no-change rebuild — the case a watcher hits on every tick where nothing relevant changed.

  The risk is not skipping, it is skipping when something *did* change, so the test drives all four mutations: modify, add, delete, and a rename with byte-identical content — the last is the one a file-count check alone would miss, and it is why the comparison is per-path rather than a count or a set size.

  **A one-file edit narrows the store *write*, not the resolve. Corrected 2026-09-05.** The affected closure is the changed files plus every file mentioning a name whose definition moved. That is the complete dependency surface for one specific reason: the resolver has exactly two genuinely global maps, the symbol index and the type/method index, and *both are keyed by bare symbol name*. Nothing else a file resolves against is global. ~~Measured on a 1,610-file tree, editing one file resolved **13 edges instead of 172,046**.~~ That claim was **stale**: narrowing the resolve was reverted as unsound (see `main.rs:1234-1253` — liveness and community detection are global, so a subset resolve committed 433 dead-code candidates instead of 14), and `main.rs:1267` calls `resolve_all` on every build. The `resolve_subset(extractions, only)` entry point had no caller left and has been removed from `devmap-resolve` rather than left as a supported-looking option. The narrowing that survives is the store's edge partition, which still carries unaffected extractions forward.

  Two things made this safe rather than a rerun of SC16. The index is still built from every extraction — a subset index would resolve differently, which is the bug itself. And the store's edge partition had to change: carry-forward skipped edges where *either* endpoint was affected while the resolver added them on the same rule, which leaves a hole exactly where a subset resolve stops producing edges — an edge from an unaffected source into an affected target would have been skipped by both and silently vanished. Edges are now partitioned by **source file only**, so each belongs to exactly one bucket: the file whose extraction produced it.

  The closure falls back to a full resolve whenever the cheap answer is not clearly safe — no prior generation, any file added or deleted, or a closure covering half the repository. Narrowing too far is the only dangerous direction, so every uncertain case takes the slow, correct path.

  **Verified, not argued.** The incremental graph digest matched a cold build of the identical tree exactly (`e4d9f169…`); a 25-cycle soak held its digest; and `incremental_equivalence.rs` now asserts the property permanently by editing a definition two other files depend on, building incrementally, then rebuilding the same tree cold and requiring the edge sets to be equal. SC16 was "sound because of an argument" and still wrong for the life of the port, so the argument is not the evidence here — the equality is.

- **The self-build budget was re-derived, not raised to fit.** The grammar matrix going from 5 to 32 languages made the build legitimately slower (1,110 files instead of 1,088, 22 more grammars). The 5 s budget measured work the port no longer does. Quiet-run measurements after the change: 3.0 s, 3.65 s, 4.0 s. D17's ledger was A/B-measured and is *not* a factor — 4,007 ms without it against 3,650 ms with it, inside the noise — so the obvious suspect was ruled out rather than assumed. The budget is now 10 s with the rationale recorded in `verify.sh`, keeping ~2.5x headroom so the gate catches a regression rather than a busy machine.

- **Grammar coverage completed: 32 of 33 declared languages now reach a real grammar (2026-08-16).** The blocker for the last five was never the parser — it was the *Rust wrapper*. `tree-sitter-vue`, `-cobol`, `-liquid` and friends pin tree-sitter 0.20, whose `Language` is a distinct type from our 0.25, so they cannot be linked at all. The generated `parser.c` is fine: it emits ABI 14/15, which 0.25 accepts. Compiling the C directly and declaring the entry point ourselves sidesteps the stale wrapper and keeps every grammar on one runtime, with no second tree-sitter in the dependency graph.

  `vue`, `cobol` and `liquid` are vendored under `vendor/grammars/` and built by `cc` from `build.rs`; `astro` came from `tree-sitter-astro-next`, which already targets 0.25; `kotlin` and `svelte` from the maintained `-ng` forks. Wiring them surfaced a real build-system bug: the build script registered `rerun-if-changed` only for grammar directories that existed on the *previous* run, so adding a new grammar never re-triggered it and the C was silently never compiled — the link then failed with an undefined symbol. It now watches the vendor root.

  **VB.NET is the one language with no grammar anywhere** — absent from crates.io and from every upstream repository I could reach. It stays declared in the registry and reports `Unavailable` honestly rather than degrading silently. `grammar_matrix.rs` asserts the whole matrix, so a language added to the registry without a grammar now fails a test instead of quietly indexing nothing.

  Two of my own tests caught their own staleness here, which is the pattern worth noting: the grammar-identity test asserted `cobol` was unavailable, and stopped testing anything the moment cobol was vendored. It now names VB.NET, the language that genuinely has none.

- **CI exists for the first time (`.github/workflows/rust-port.yml`).** The port had *no* CI at all — every gate ran only on one developer's macOS machine, which hid both platform-specific failures and the plain case of a change never having `cargo test` run against it. The workflow builds and tests on ubuntu, macOS and Windows, runs the full `verify.sh` gate suite on Linux, and reports parity. Windows compilation cannot be verified from this machine — cross-compiling 25 C grammars to MSVC needs the MSVC toolchain — so the CI job is the verification. A source audit for ungated Unix-only APIs (`std::os::unix`, `UnixListener`, `PermissionsExt`) found **zero** outside `#[cfg(unix)]` items, so the port is expected to compile there.

- **D17 closed: the unresolved-call ledger is durable (schema v9).** The resolver has always computed unresolved calls — they are the honest denominator for "is this symbol really uncalled?" — but they lived only in memory, so nothing could ask *why* a symbol had no callers. `generation_unresolved` now records source symbol, callee and reason per generation, readable via `latest_unresolved`.

  Two things this surfaced. A fresh database stamps `CURRENT_SCHEMA_VERSION` directly and never runs the migration chain, so a table added by a migration must *also* be added to the create path — otherwise every new database is missing it while every migrated one has it. And the v7->v8 block validated the schema mid-chain, which was only correct while 8 was current; with 9 current it rejected every legitimately-migrating v8 database. The v6->v7 block already carried a comment warning about exactly this, so the hazard was documented and then repeated one migration later.

  The table is pruned with its generation. An unpruned side table is precisely the SC1 failure — the store grows by O(repository) per build forever while every visible count stays bounded — so the test asserts the prune, not just the write.

- **D10 was already closed; the ledger entry was stale.** `repo_root` is stored (schema v7) and consumed by `resolve_source_path` in the query engine. The claim that "storing/resolving a repository root remains open" no longer matched the code.

- **SC16 (CLOSED, correctness): incremental builds drifted permanently from cold builds.** Found by the new soak harness on its **first cycle**, and it is the most serious open finding in the port.

  **Reproduction** (`tools/soak.sh <tree>`): build a tree, append a function to any one file, rebuild, restore that file exactly, rebuild. The graph does not return to its original state. On a 1,610-file DevCouncil tree the restored build carries **172,161 edges against a cold build's 172,046** — 115 extra, none missing.

  **Isolated, not guessed.** Both builds see identical file sets (1,610) and produce identical node sets (21,679). Edge kinds are identical for `Contains`, `Imports`, `References`, `MemberOf` and `HandlesRoute`; **only `Calls` differs** (136,333 vs 136,218). The drift is permanent and self-sustaining: rebuilding twice more does not converge it, and clearing `extraction_cache` does not either — only a fresh database gives the cold answer. The extra edges are speculative multi-candidate fan-out (one caller to many same-named `.impact` methods), so a long-lived daemon accumulates plausible-looking wrong edges that no rebuild removes.

  **Root cause, and my first two hypotheses were both wrong.** I guessed dedup-against-references; `resolve_all` already skips call-kind references, so that was refuted by reading the code. I then guessed the soak's file edit mattered; it did not — the perturbed file was under `.cursor/`, which the walker ignores, which meant the *real* reproduction was far simpler and far worse: **building the same unchanged tree three times gives 172,046 / 172,161 / 172,161.** Build 1 is cold, build 2 takes the incremental path and diverges permanently.

  The actual cause is `index_extractions`, not `resolve_all`. Receiver-type bindings are built from `reference.assigned_to`, and `worker = Worker()` is a *Constructor-kind* reference whose `assigned_to` is the only record that `worker` is a `Worker` — `calls` has no field for it. `for_durable_store` stripped all call-kind references as "redundant with `calls`", so a reloaded extraction lost every receiver binding, receiver resolution fell back to speculative fan-out, and the incremental build emitted **more** `Calls` edges than the cold one. That is why the count went up rather than down, and why nothing converged: no later rebuild recomputes it.

  **Fix:** `for_durable_store` now retains a call-kind reference when it carries a binding, and still drops the genuinely redundant ones, so SC8's payload saving is kept. `EXTRACTION_SCHEMA_VERSION` 15 -> 16. Verified: four consecutive builds of the same tree now all produce **172,046** edges. The regression test was confirmed to fail against the pre-fix code (`left: []` vs `right: [("Worker", Some("worker"))]`), and a 40-cycle soak passes with a stable digest and growth plateauing from cycle 10.

  **The gate that would have caught it now exists.** `verify.sh` is 8 steps, not 7: step 7 runs `tools/soak.sh` and fails on incremental-vs-cold drift. Determinism compares two *cold* builds and the growth gate compares size — neither could ever have seen this.

  **This corrects how SC2 was characterised in this ledger.** SC2 was recorded as a *performance* gap ("the incremental path is still not incremental in design"). It is also a correctness gap: incremental output is not equivalent to cold output, and the difference does not wash out. Any claim that `devmap serve` can back the Python `dev map` continuously depends on this being fixed, because the daemon never does a cold build.

  **Remaining after the fix:** the incremental path is still not incremental *by design* (SC2/B3) — it re-runs resolve and analyze over the whole repository. That is now a cost problem only; the correctness half is closed and gated.

- **Replacement readiness assessed against the Python baseline, and the two blocking gaps closed (2026-08-16).** The question was whether the port can replace Python `dev map`. The integration model settles half of it: `devmap_client.py` states that "all graph analysis, extraction, resolution, and query budgeting are strictly owned by devmap (Rust)" and the Python CLI/MCP surface calls into it, so the command-count difference (13 Rust subcommands vs 38 Python ones) is not a blocker — Python keeps the surface. What mattered was engine coverage, and the parity harness returned `ok: false` on all five fixtures.

  **Grammar coverage: 5 crates -> 25.** The `solidity_app` fixture was the proof: the Python baseline emitted 3 nodes, devmap emitted **0**, because 30 of 36 declared languages had no linked grammar. Linking them exposed a hard blocker first — tree-sitter 0.24 accepts ABI 13–14, while the current grammars ship ABI 15, so *every* modern grammar failed to load. Upgrading the core to 0.25 (ABI 13–15) fixed it. **20 grammars now load and parse**, verified individually: hcl, java, csharp, php, ruby, c, cpp, objc, cuda, swift, scala, dart, pascal, lua, luau, r, cfml, erlang, solidity, nix. Four more are **unusable and recorded as such**: kotlin, svelte and vue depend on an older tree-sitter and fail to compile against 0.25 ("expected `Language`, found a different `Language`"), and tree-sitter-cobol does not resolve. Three have **no crate at all**: astro, liquid, vb.

  A generic declaration extractor covers the new grammars, designed from each grammar's own parse tree rather than assumption — C in particular puts no `name` field on `function_definition`, so the name is recovered through its declarator, and `function_declarator` is skipped when its parent is a definition so the same function is not emitted twice. Solidity now yields `Class SimpleStorage` with `Method SimpleStorage.get/set`, which is *better* than the baseline's flat `Contract.sol::get` (X30). Terraform needed its own arm: its declarations are `block` nodes the generic table cannot see, so a `.tf` file indexed with a File node and nothing else. Blocks now carry their Terraform address (X32). Measured on scholarlm: **345 HCL files that previously produced nothing now yield 2,406 symbols**.

  **Exported module-level bindings.** `export const VALUE = 42`, Go `const Limit`, Rust `pub const` produced **no symbol in any language** — invisible to the map: unsearchable, unreferenceable, neither confirmable live nor reportable dead. The baseline emits them (as kind `function`, a misclassification); devmap now emits `Variable` (X31), scoped to *exported* module-level bindings so private and function-local constants add no dead-code noise. **+3,117 Variable symbols** on scholarlm. My own symbol golden had pinned this omission as deliberate, which is how it survived — that comment is now corrected to say why the rule changed.

  **What this cost and what it caught.** The 0.25 upgrade broke one store test, which was the right outcome: its fixture used `src/legacy.java` as an "unavailable grammar" case, and Java now parses. The fixture moved to `.vb` (no crate exists) *with an explicit precondition assertion*, so it fails loudly rather than silently testing nothing if that ever changes. Two golden tests also failed and both changes were verified as intended before updating. `ts_app` parity diffs fell 6 -> 4 and the `REEXPORTED_VALUE` gap is closed; `solidity_app` went from producing nothing to producing a better-structured graph than the baseline.

  **Still not a drop-in replacement.** Remaining: Python module-level constants (Python has no export marker; the principled rule is `__all__` membership and it is not implemented), 7 languages with no usable grammar, SC2/B3 incremental design, D17, D10, no Windows CI, no soak test. The engine is now materially closer, and the two gaps that made replacement impossible for a polyglot repo are closed.

- **`resolver.rs` hardened: 86 survivors -> 24, kill rate 58% -> 88% (2026-08-16).** A whole-workspace sweep found this the weakest crate in the port, which matters more here than anywhere else: the resolver is what turns a name into an edge, so its failures are *confidently wrong* answers rather than missing ones.

  **Go import resolution is now 45/45, zero survivors** — the single largest cluster in the workspace at 33. `resolve_go_import` is a five-tier ladder (replace directive, module prefix, vendor, directory suffix, abstain) and every tier and guard was mutable, including whole-body replacement with `vec![]` and `vec!["xyzzy"]`. Closing it needed a fixture where each tier is the *only* one that can succeed, because several tiers otherwise reach the same files and mask each other: a one-component package `x/` that the suffix tier is forbidden to claim, so only the module prefix can resolve it; a nested module deliberately mapping `sub/` to `othersub/`, so a shorter prefix winning lands on a directory that does not exist; and a Python file at a *longer* path suffix than the real Go package, so relaxing the `.go` filter lets a directory with no Go code outrank it.

  Two guards turned out to be load-bearing in ways worth recording. `import "C"` is cgo's pseudo-package, and its early return is only observable when a directory of that name exists — the fixture now vendors `vendor/C/`, which the vendor tier would otherwise resolve every cgo import in the repository to. And `go_package_name_of` matters most after `for_durable_store` strips `source_code`: the extracted field is then the only record of the package, so losing it breaks package-level import edges on exactly the path a restarted daemon takes.

  Also closed: per-language import-path resolution for TypeScript, Python and Rust (quote stripping, index files, dot-counting relative imports, `crate::` segment popping); `bind_receiver`'s SC9 poisoning, including that agreement is not conflict and that poison is permanent; `import_spec_for_name`; `LangFamily::from_lang` with a distinctness check; the Python builtin guard on the call path; and same-file ambiguity abstention.

  **`resolve_all` then went 15 survivors -> 5.** Its remaining mutants needed a multi-file fixture per branch, and the branches are the graph's structural skeleton: `Contains` (the file owns every symbol, a type additionally owns its methods, and the file never contains itself), Go package membership (non-`main` only, Go only), Go import edges naming the package rather than each of its files, and the confidence tiers — a same-file callee is DETERMINISTIC, a unique global match HIGH, and a multi-candidate guess must stay SPECULATIVE, which is G5's whole point. Also closed: module-qualified calls (`helpers.do()`, `ns.doIt()`, `svc.Do()`) resolving through their import binding, and Rust `self::`/`super::` paths.

  **A gap found while closing the route-handler mutants — since fixed.** `HandlesRoute` edges could only ever form for **axum**: the fastapi/flask matcher emitted the literal handler name `decorated_handler` and the express matcher `anonymous_or_function`, neither of which matches any symbol. Routes for the two most common web frameworks in the corpus were extracted but never connected to their handlers.

- **FastAPI/Flask and Express route handlers are now named (2026-08-16, `EXTRACTION_SCHEMA_VERSION` 14 -> 15).** A Python route decorator carries no handler name of its own — the handler is the `def` it is attached to, which may sit several stacked decorators, comments and blank lines later, so the scan walks forward to it. An Express handler is the last *top-level* argument of the route call, after any middleware; finding it means tracking paren, bracket and brace depth so that a comma inside an arrow body, a nested call, an array of middleware or an options object does not end the scan early.

  **Both fail closed rather than guessing.** A decorator not attached to a function, and an arrow or anonymous function handler, yield an empty name — which resolves to nothing. The previous placeholders were worse than useless: a repository containing a symbol actually named `decorated_handler` would have had every FastAPI route bound to it. A route bound to the wrong symbol is worse than an unbound one, because it also protects that symbol from ever being reported dead.

  **Measured, and the claim is narrower than it looks.** On 3,846 real Python/JS/TS files: **HandlesRoute edges 0 -> 97**, total resolved edges +97 exactly, unresolved calls unchanged. Dead-symbol rows were **byte-identical at 265** — those handlers were already protected by SC6's separate route-wiring exemption, so this buys graph *traceability* (`trace`/`impact` can now follow a route to its handler) and not fewer false positives. The `verify.sh` determinism digest changed for the first time in this port, from `1c616c80…` to `d8c1a745…`; the change was attributed precisely by diffing the testdata graph, which gained exactly one edge — `GET /health > health:HandlesRoute` — and lost none. `frameworks.rs` measures **52/52 with zero survivors**, including all of the new depth-tracking code, and 101 Python consumer tests pass under schema v15.

  **14 survivors remain and are not claimed as equivalent:** 6 in `resolve_name_reference`, 5 in `resolve_all`, 2 in `index_extractions`, 1 in `resolve_import_path`. Two of the `resolve_all` five are the Go-visibility clause on the route path, unreachable because Go routes are not extracted at all. The `resolve_name_reference` mutants sit in the global-unique tier; I could not construct a Python name reference that reaches it, and a positive control showed a *non*-builtin name in the same shape also resolves to nothing — so asserting the builtin case is empty would have been a vacuous test passing for an unrelated reason. It was dropped rather than shipped.

- **The last 22 `treesitter.rs` survivors closed (2026-08-16): 25 -> 8, and every one of the 8 is demonstrated equivalent rather than merely argued.** Kill rate 97% (330/339 killable).

  Nine tests closed the genuine gaps: function-expression naming (`const f = () => {}` attributing its calls to `f`, the join-key shape behind SC9/SC10), the Go interface-satisfaction *wiring* (the pure matcher was tested in `model.rs`, but nothing asserted its results ever became annotations), import-specifier suppression, `alias` and `parameter` binding sites (`with … as handle`, `except … as e`, arrow and catch parameters), Rust `use`-list names, scope-bounded shadowing, and the scope-local cache reset.

  *A cache whose staleness is invisible.* `reset_scope_locals` was replaceable with nothing. Its keys are `Node::id()` — arena addresses reused after a tree is dropped — so a stale entry can be served for an unrelated scope in the next file, suppressing that file's genuine references. Address reuse is nondeterministic, so a behavioural test would be flaky; the contract is asserted directly through a `#[cfg(test)]` length accessor, with a vacuity check that the fixture really populates the cache first.

  *Two deletions, both proven before removal.* The ancestor walk inside `is_call_callee` was dead by construction — that branch is only entered when `node.parent()` **is** the call, so the first ancestor examined is always that same parent and the walk breaks immediately. Instrumentation over 4,421 files in five languages recorded zero reaches and zero second iterations. Deleted; digest unchanged. The `python_all_exports` cursor was replaced with `str::match_indices`, which cannot fail to advance: mutating the old increment did not produce a wrong answer, it **hung the extractor**, which is a worse failure than the arithmetic was guarding against.

  *A near-miss worth recording.* I very nearly deleted the `_target` clause in `is_binding_wrapper` after a scan of top-level node kinds reported that no linked grammar declares one. That scan was wrong: Python's `as_pattern_target` appears in `node-types.json` only as a *field type* of `as_pattern`, never as a top-level entry. The deletion was a real regression — `with … as handle` stopped being a binding site — and the test I had just written caught it. The clause is restored with the reason recorded in its doc comment.

  *And a false finding I nearly reported.* My first equivalence run showed the Go `selector_expression` guard changing output. Re-running with a **frozen** corpus and a trailing baseline control showed the corpus had drifted between runs (2,386 Go files down to 818); with the control in place, that mutant is identical. The same contaminated-A/B shape as the stale-database mistake earlier in this port. Every equivalence claim below was taken with a `B0 … mutants … B1` control where `B0 == B1`.

  **The eight survivors, each accounted for.** Seven are equivalent by identical node *and* edge digests over a frozen 4,422-file five-language corpus (Rust, TypeScript, TSX/JSX, Python, Go): `is_symbol_binding`'s declarator conjunction, `get_node_text`'s bounds guard (defensive and unreachable — no node ever exceeds its own source), both `split_call_target` emptiness guards, `is_call_callee`'s member-expression conjunct (also instrumentation-proven: arguments are wrapped in `arguments`/`argument_list` in every linked grammar, so a member expression directly under a call is always its callee), `extract_node`'s `idx1 * 1`, and its `"impl_item" => {}` arm against the identical `_ => {}` default. The eighth, `is_benign_jsx_ampersand`'s second operand, is equivalent over 513 real TSX/JSX files — an error node containing `<` starts a JSX element rather than sitting inside one, so it never reaches the parent walk.

- **`extract_node` closed: 25 survivors -> 2, both proven equivalent (2026-08-16).** The largest single bucket in the port, and the two that remain are not gaps.

  *Five copies of one rule.* The export test — `parent().kind() == "export_statement" || …` — was inlined at five declaration sites (const-arrow, class, enum, interface/type-alias, namespace) while a canonical `js_symbol_is_exported` helper already existed and was called from exactly one. Two copies were subtly different: the `variable_declarator` case needed `parent().parent()` because the export statement wraps the `lexical_declaration`, not the declarator. Every copy was independently mutable, so a broken one would mark a single *kind* of declaration unexported while the others stayed right — and since `is_exported` is direct evidence for dead code, that reads as "this enum is private and unused" rather than as a bug. The five copies now call the helper, which grew a `variable_declarator` case alongside its existing `method_definition` one. Output was byte-identical across all six forms before and after. One test now covers every form with a matching unexported control, so neither a constant `true` nor a constant `false` can satisfy it.

  *Unreachable code deleted with proof, not with absence of evidence.* The `composite_literal` branch carried a child-scanning fallback for when `child_by_field_name("type")` returns None. Three independent signals say it cannot: instrumented runs recorded **zero invocations across 2,386 real Go files**, zero across a synthetic file covering every literal shape (plain, addressed, sliced, mapped, nested, anonymous-struct, implicit-length-array), and tree-sitter-go's own `node-types.json` declares `composite_literal`'s `type` field **`"required": true`**. Deleted; the Go corpus rebuilt to exactly 336,224 edges and 166,269 unresolved calls, unchanged. If a future grammar relaxes the field the expression yields None and the literal is skipped — the safe direction, no fabricated call.

  *The two survivors are equivalent mutants, demonstrated.* `"impl_item" => {}` has the same empty body as its match's `_ => {}` default and no later arm matches `impl_item`; `idx1 * 1` is `idx1`, and the leading brace is trimmed by the binding parser. Rather than argue that, both were applied to the source and rebuilt against a 1,330-file Rust+TS corpus: **node and edge digests byte-identical to baseline in both cases**. The `impl_item` arm is kept despite being a no-op — its comment records why methods must not be emitted there, which is a guard against a future edit, not dead code.

- **A bug the mutation work surfaced: `__all__` was matched anywhere in the source (2026-08-16).** Writing the `python_all_exports` test showed `my__all__ = ["nope"]` exporting `nope`, and a commented-out `# __all__ = [...]` doing the same — both cases the function's own doc comment claimed were handled. `__all__` is a module's explicit public API, so a false entry marks a name as public and **suppresses a genuine dead-code finding**. Fixed with a stand-alone-token check and a comment check. The first fix introduced cross-iteration offset bookkeeping that was itself hard to test — three surviving mutants sat in it — so the scan was restructured to line-local offsets, where the arithmetic is naturally correct and there is nothing left to track between iterations. Known limit, stated in the doc comment: a `#` inside a string literal earlier on the same line would hide a real declaration.

- **Two receiver-binding gaps, found by following `assigned_to` (2026-08-16).** Probing bindings for a test showed Python, JavaScript and TypeScript recording `worker = Worker()` while Rust and Go recorded nothing.
  - *Rust:* the `call_expression` arm passed a literal `None` where the Python, TS and Go arms all passed `assignment_binding(node, source)`. `assignment_binding` was verified to return `Some("w")` for that exact shape, so the fix was one argument.
  - *Go:* `worker := NewWorker()` wraps its target in an `expression_list` even with a single target, so `simple_binding_name` saw a list and bound nothing — every Go value constructed the idiomatic way had a receiver with no type behind it. Fixed at the canonical owner by unwrapping a **single-target** list only; `value, err := New()` deliberately still binds nothing, because nothing there says which target receives the value and a wrong binding is worse than an absent one.

  **Measured on real corpora, not asserted.** The `verify.sh` determinism digest was unchanged, but that digest covers the DevCouncil Python corpus where neither path runs — it is not evidence about these fixes, and was not treated as such. A/B on 2,386 Go files (an rsync copy; the source repository was confirmed untouched, 0 dirty lines): **speculative 0.2 edges down 275, deterministic 1.0 edges up 183**, unresolved calls identical at 166,269, one fewer dead-symbol row. The improvement is *precision* — speculative fan-out collapsing into exact resolutions — which is why the edge total moved by only 229 and why edge counts alone would have hidden it. The mechanism was isolated on a controlled two-type fixture: `w.go()` goes from **0.2 to 1.0** with the same edge count.

  **What is still open, with the failed attempt recorded.** On real Rust the fix is nearly inert (+1 edge on 58 files) because the dominant `Worker::new()` idiom produces the callee name `Worker::new`, which matches no symbol, so the resolver discards the binding; only the tuple-struct form `Worker(1)` binds. I tried closing that in the resolver by falling back to the `::` prefix, **measured it, and reverted it**: it gained no edges and produced 4 *more* unresolved calls on the Rust corpus. Recorded here so the next reader does not repeat the same fix.

- **A serialized field with no readers, found while pinning imports (2026-08-16).** Probing import extraction for the golden showed Python `from .rel import thing` reporting `is_relative: false` while Rust `crate::…` reported `true`. The apparent bug turned out to be worse than a bug: `ExtractedImport::is_relative` was **written at six sites and read at none** — hardcoded `false` at five of them, genuinely computed only for Rust. Confirmed with two independent signals before touching it, per the deletion rule: no reader anywhere in the workspace, and none in the frozen Python consumers that read the extraction JSON (every Python hit for the name was `pathlib.Path.is_relative_to`). It was deleted rather than corrected — computing it properly would have added a field nobody reads — with `EXTRACTION_SCHEMA_VERSION` bumped 13 -> 14 so no cached payload carries the stale shape. The determinism digest was **unchanged** across the deletion (`1c616c80…`), which is the evidence that nothing downstream consumed it.

- **Earlier `treesitter.rs` guard coverage.** Three of its load-bearing guards were covered directly: `is_benign_jsx_ampersand`, where over-matching is the dangerous direction because genuinely broken source would report `Clean` and X6's "symbols overlapping a parse error are never dead-code evidence" rule stops firing when no error is recorded; `macro_token_body`, which bounds SC13's macro re-parsing; and Rust type-name reduction, which feeds SC12's parameter bindings — leaving a `&` or generic attached names a type no symbol has, so the call silently fails to resolve. `model.rs` (478 lines, also untested) had its confidence persistence pinned: that discrete milliconfidence space exists *because* SQLite REAL cannot round-trip `f32` 0.9, and nothing checked the conversion the whole scheme rests on. NaN now provably fails closed at 0 — a NaN confidence makes every comparison false, so the row disappears from every filtered query without an error.

  **Mutation coverage by crate:** `devmap-store` retention **19/19** · `devmap-query` **121/125 killable** (3 proven equivalent, 4 genuine survivors in manifest) · `devmap-analyze` traversal+liveness **75/75** · `devmap-resolve` **193/207 killable (93%), up from 121/207 (58%)** — 86 survivors down to 14, itemised below. `devmap-serve` **69/130** and `devmap-cli` **8/24** measured with their guard survivors since closed. `devmap-extract` **194 mutants / 41 workspace-scoped survivors all killed**, `model.rs` re-measured at **58/58 viable, zero survivors**; 1 proven equivalent and 20 integration-only-covered survivors recorded above. `treesitter.rs` **330/339 killable (97%), up from 115/348 (33%)** — every remaining survivor is individually accounted for: 8 demonstrated equivalent, 1 a detected timeout since removed.

  Scope matters when reading any of these numbers: the same crate scores differently under `-p <crate>` and `--test-workspace=true`, and neither is "the" mutation score. Package scope is stricter and ~30-140x faster; workspace scope reflects what the shipped test suite actually catches. Every figure here names the scope it was measured at.

- **Fault injection around the new destructive write path (2026-08-15).** `crates/devmap-store/tests/test_fault_injection.rs` — five cases, all passing. Retention and cache eviction now run on *every* build and *every* daemon resync, deleting rows across seven tables, so their failure modes are pinned rather than just their happy path: a generation is never left half-pruned (asserted per table, in both directions — no orphan rows, and no surviving generation stripped of its files); a **truncated database fails closed** rather than answering confidently with zero symbols, which for a code-intelligence store is the dangerous failure because a corrupt index and a clean repository look identical; cache eviction cannot be silently disabled by SQL `NOT IN` NULL semantics (one NULL key would turn the whole eviction into a no-op with no error anywhere); pruning a fresh single-generation store is harmless, which pins the ordering dependency that eviction must run *after* the generation commits; and a concurrent reader never observes an empty store while a prune and vacuum run against the same file — the real daemon shape, where IPC queries serve while resync prunes. Writing these caught a vacuous fixture: `save_generation` does not populate `extraction_cache`, so the first version of the eviction test asserted against an empty cache and would have passed no matter what the code did.
- **Python hybrid consumers re-verified after the schema-v14 bump (2026-08-16).** Deleting `is_relative` changed the extraction payload shape, so the frozen Python consumers were re-run rather than assumed unaffected: **274 tests pass** — `test_devmap_client`, `test_dead_symbols`, `test_wiring_check`, `test_stale_map` (72); `test_skills`, `test_graph_dead_code`, `test_codeintel_store` (82); `test_cli_graph`, `test_graph_cmd_command`, `test_codeintel_sync`, `test_companion_mcp` (120). The full Python suite has **not** been re-run since the v9 change below.

- **Python hybrid consumers verified against the changed Rust surface (2026-08-15).** The SC6 work bumped `EXTRACTION_SCHEMA_VERSION` to 8 and changed dead-symbol output, both of which the frozen Python consumers read. **274 focused tests pass**: `test_devmap_client`, `test_dead_symbols`, `test_wiring_check`, `test_skills` (90), plus `test_cli_graph`, `test_graph_cmd_command`, `test_graph_dead_code`, `test_codeintel_store`, `test_codeintel_sync`, `test_companion_mcp`, `test_stale_map` (184). The **full Python suite was then run to completion: 4,128 passed, 5 skipped, 3 warnings in 1,317 s** — no regression from the schema-v9 extraction change or the dead-code output changes.
- **Closed (SC8, 2026-08-15): the doubled extraction payload is gone — database down 42%.** `extraction_cache.payload_json` and `generation_files.extraction_json` held byte-identical content for the same key, so every extraction was stored twice: 153 MiB of a 374 MiB database on the external corpus, 37 MiB of 88 MiB on DevCouncil itself.

  The naive dedup would have been a silent correctness bug, and the investigation that caught it is why this took a schema change. `extraction_cache` is keyed `(content_hash, language, grammar_version, analyzer_version)` **precisely** so a payload produced by older extraction semantics can never be reused; `generation_files` recorded only the first two. Serving cache misses from it as-is would have discarded that guarantee — exactly how the SC6 entry-point false positives would come back, since fixing them required bumping `EXTRACTION_SCHEMA_VERSION`. Schema **v8** therefore adds `grammar_version` and `analyzer_version` to `generation_files` (transactional migration with the same `has_column` idempotency probe as v7), both insert paths stamp them, and carry-forward preserves them. The cache read falls back to a retained generation **only on the full four-part identity**; pre-v8 rows carry NULL and are never eligible, because absence of a recorded identity is not proof of a matching one.

  Measured on one frozen corpus with both binaries: **374 MiB → 216 MiB (−42%)**, `extraction_cache` 3,292 rows → **0**. The DevCouncil self-build gate went 88 MiB → **51 MiB**. Reuse verified end-to-end rather than assumed: a second build over unchanged sources completed in 9.07 s against 12.36 s cold with zero cache rows, so the reuse came from `generation_files`. Parity unchanged at 18 diffs; 279 workspace tests pass.

  One existing test needed updating rather than deleting: `superseded_extraction_cache_entries_are_evicted` asserted the stable file's entry survived *in `extraction_cache`*, which is now correctly evicted as redundant. It was rewritten to assert **retrievability with a full identity** instead of which table holds the row — the invariant it existed to protect, now pinned in a way that a storage change cannot make read as a regression.

- Search/deps/impact/trace/dead/manifest/snapshots read SQLite rather than rebuilding the repository.
- Missing and parse-failed targets return explicit `ResolutionAvailability::Unavailable`.
- Every implemented response reports `shown`, `hidden`, `total`, `truncated`, and `tokens_used`; the Python client rejects inconsistent counts or budgets.
- Persisted FTS search p95 at 10,000 synthetic files: 0.172 ms in the latest debug test, below the 50 ms gate.
- CLI integration builds a generation, deletes the source tree, and successfully runs search, deps, impact, trace, dead, manifest, and snapshots.
- `trace from to` now returns one deterministic bounded shortest path through SQLite. Depth/node caps apply, IPC remains backward compatible, and an insufficient token budget returns no misleading path prefix.
- JSON embedded in HTML is script-safe without corrupting JSON semantics; hostile `</script>` payloads have regression coverage.
- Generation metadata now records a real Git HEAD when available; manifests receive the persisted HEAD and real pending count instead of decorative constants. Persisted search reports `source_unavailable_reason` rather than silently presenting an empty snippet when source is absent.
- Open: N7 RRF/vector fusion, process-flow precomputation, complete artifact stamps/staleness UI, dynamic-dispatch traces, route/shape capability coverage, and durable source snippets for search after source deletion. Raw source intentionally remains absent from every generation payload.

### Phase 6 — cutover: build path cut over (2026-09-02); Python deletion interrupted — see "Kernel audit, second pass" and AGENT_PLAN.md → Handoff

- `src/devcouncil/devmap_client.py` remains transport-only; added `try_connect()` and `resolution_unavailable_reason()` (X14).
- Hybrid migrations (Rust-primary when binary+compatible DB available, else Python fallback):
  - MCP: `handlers/codeintel.py`, `handlers/map.py`, `util.py` freshness wrapper
  - CLI: `cli/commands/graph_cmd.py` query surfaces (search/query/trace/dead/impact/status)
  - Verification: `dead_symbols.py`, `stale_map.py`, `wiring.py`
- Still **37** files outside `indexing/`+`codeintel/` import Python indexing/codeintel (grep-proven); hybrids keep fallbacks because the live `.devcouncil/codeintel/index.sqlite` is the Python schema and is incompatible with Rust `devmap` store open.
- Blockers for primary cutover: shared/compatible store or dual-write; semantic_diff/acceptance_corpus/liveness_snapshot APIs not on Rust surface; adjuncts (AST/LSP/debug/PDG) Phase 7.
- Open: shadow soak, quiet weeks, core Python deletion, binary publication, platform CI.
- **Closed (SC23, 2026-08-17): the hybrid consumers' Rust path had never once executed in this repository.** Phase 6 landed seven hybrid consumers that call `DevMapClient` first and fall back to Python on `DevMapClientError`. `DevMapClient.DEFAULT_DB_PATH` was `.devcouncil/codeintel/index.sqlite` — the **Python** store (`user_version = 2`, tables `node_payloads` / `aliases` / `unresolved_references`). The Rust store is `user_version = 10` with a different table shape and correctly fails closed on it: `devmap status` against that file returns `unsupported schema version 2`.

  So every hybrid call raised, was caught, and fell through to the Python fallback — on every invocation, for every consumer. This ledger recorded the blocker as "the live index is the Python schema and is incompatible", which was accurate, but recorded the consumers as "hybrid (Rust-primary)", which was not: the primary path was unreachable, and nothing measured whether it ran.

  Fixed by pointing the client at the Rust kernel's own store, `.devcouncil/codeintel/devmap.sqlite`, so the two schemas coexist during cutover instead of contending for one path. Fail-safe when absent: `_spawn_daemon` already requires a non-empty file, so a repository that has not run `devmap build` degrades to the Python fallback exactly as before.

  **Verified end-to-end, which is the part that was previously missing:** after `devmap build` into the new path, `DevMapClient(root).status()` returns generation 1 with 12,103 nodes and 60,944 edges, and `client.impact(...)` returns 80 items with `resolution == "Available"` — the first time the Rust surface has answered a Python consumer in this repository.

  `test_start_daemon_reaps_child_when_readiness_times_out` began failing on the change: its fixture hardcoded the old path, so the daemon returned before spawning and the reap it asserts never ran. The test was not weakened — its setup now derives the path from `DEFAULT_DB_PATH`, so it cannot silently stop exercising the reap the next time the default moves.

- **Closed (SC24, 2026-08-17): `try_connect` reported an unbuilt store as available, so consumers answered from an empty index instead of falling back.** Switching the client onto a reachable store (SC23) immediately broke five Python tests, which is the value of actually running a path that had never run.

  Root cause: `try_connect` returned a live client whenever `status()` did not raise. But `devmap status` against a path with **no database creates one** and reports success — `generation_id: null`, zero nodes. And a build over a directory whose walker found no sources commits a generation with **zero nodes**. In both shapes the consumer received a confident "no callers, no dead code, no trace" drawn from an index that had never seen the repository, rather than falling back to Python. This is the same fail-open class the port has fixed repeatedly: a check that could not run reporting what a check that ran and passed reports.

  `try_connect` now requires a committed generation **and** a non-zero node count. There is no case where answering from an empty index beats falling back — if the repository really is empty, the fallback reports the same nothing.

  Verified both directions: in this repository the client returns a live handle (generation 7, 12,103 nodes, `impact` returning 80 items), and against an empty directory it returns `None`. All five tests pass without their assertions being touched.

  **Worth recording about those tests:** `test_cli_graph_dead` and `test_cli_graph_trace` write a *synthetic* `CodeGraph` into the Python store describing `src/a.py` and `src/b.py`, while creating no files on disk. A filesystem-derived engine cannot reproduce that fixture and correctly reported zero. The guard resolves it because the fixture's Rust store is genuinely empty — but the underlying mismatch remains: those fixtures assert against graph data with no basis on disk, so they can only ever exercise the Python path.

- **Closed (SC20, 2026-08-17): `verify.sh` ran under the wrong shell in CI, so three of its eight gates never executed.** `.github/workflows/rust-port.yml` invoked `zsh ./verify.sh`, and the file's header comment called it "a zsh script". Its shebang is `#!/usr/bin/env bash`. zsh does not word-split an unquoted parameter expansion, so `set -- $SIZES` made the five recorded build sizes a single word and `S3=$3` aborted under `set -u` at line 134 — taking gate 6 (growth plateau), gate 7 (incremental-vs-cold equivalence, the check that caught SC16) and gate 8 with it. The workflow is untracked and has never been pushed, so no green build was ever hiding a red gate, but the next push would have been the first to find out. Fixed by invoking `bash ./verify.sh` and dropping the now-unneeded `zsh` install. Verified: under zsh the run dies at step 6; under bash all eight run and report `ALL GATES GREEN`, with growth plateauing at 794,624 bytes across builds 3–5, 2 generations retained, and incremental equivalence OK over 5 cycles.
- **Closed (SC21, 2026-08-17): the Rust binary is now reachable outside this repository.** `DevMapClient._find_devmap_binary()` looks for `<root>/rust-port/target/{release,debug}/devmap` and otherwise falls back to `shutil.which("devmap")`, then to the bare string `"devmap"`. Since `rust-port/` exists only inside DevCouncil and nothing had ever installed the binary, every consumer outside this repository fell through to a name that does not resolve — so "apply devmap to another repository" could not work at all. `cargo install --path crates/devmap-cli` now puts `devmap` on `PATH` via `~/.cargo/bin`. The lookup order is unchanged, so a local build still wins inside this repository.

### Phase 7 — adjuncts: started, not complete

- The PDG kernel now consumes an explicit structured statement tree and builds deterministic branch/loop/try/return/raise control flow, entry/exit threading, loop-carried reaching definitions, and exact-variable parameter taint. Inputs are range/name/sink validated and outputs carry generation/content-hash identity.
- LSP remains cut from the Rust scope pending real usage evidence.
- Open: grammar extraction into the structured PDG input, durable storage/query integration, security sink classification at the extractor boundary, parity against the frozen corpus, and final Python deletion.

## Verification evidence

Latest complete commands and totals must be refreshed after every code change:

```text
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets -- --nocapture
uv run ruff check <changed Python files>
uv run pytest -q <focused Python tests>
PYTHONHASHSEED=0 snapshot double-regeneration digest
release-mode DevCouncil cold-build/size benchmark
```

Latest results (2026-08-12):

- Focused Phase-6 consumer tests (devmap_client + MCP codeintel/map + graph CLI + dead/wiring/verify-map): **176+ passed** in this session (client 77 / map+graph suites 99).

- Rust: **166 passed, 0 failed, 0 ignored**; `cargo fmt --check` and clippy with `-D warnings` passed. `verify.sh` also passed a deterministic double build (`528f9bb1ab5e4fec02a4514827aabf867d75e8dfc2315bcc72666b583d0ce846`) and a release DevCouncil self-build at **2.656 s / 52 MiB**.
- Rust instrumented coverage: **88.88% lines**, **86.64% regions**, **85.25% functions**; all instrumented tests passed. Component line coverage includes PDG 96.47%, resolver 91.56%, daemon 88.14%, watcher 89.63%, protocol 78.48%, query engine 81.24%, and store 87.61%.
- Python full suite: **4,122 passed, 5 skipped, 3 warnings** in 872.66 s. This completed before the final two strict-JSON adapter tests were added; the final focused adapter suite is **23 passed**, Ruff clean, and mypy clean for `devmap_client.py`.
- Current Phase 6 hybrid-consumer selection: **150 passed**; focused Ruff is clean and focused `devmap_client.py` mypy is clean. The snapshot tool's four tests pass, and two full regenerations produced the identical digest above.
- Repository-wide Ruff still reports one pre-existing unused import in the frozen Python indexing tree. Repository-wide mypy reported 93 errors before the two adapter-local errors were fixed; the remaining unrelated baseline was not rerun or claimed clean.
- DevCouncil `dev verify TASK-RP-6 --json` returned `ok: true` with no gaps, but explicitly skipped quality verification because `gates.mode=off`; it is not counted as independent verification evidence.

Current post-audit verification (2026-08-12, after the evidence above):

- Hardened `./verify.sh` passed format, workspace clippy with `-D warnings`, **171 listed Rust tests**, semantic graph determinism, and the optimized DevCouncil self-build gate. The final measured self-build was **2,646 ms / 52 MiB**. Mutation testing was explicitly skipped because `cargo-mutants` is not installed and the script no longer installs dependencies implicitly.
- The verifier itself now rejects unknown flags, uses the real v6 generation-edge schema for a path/symbol/confidence digest, invokes global CLI options in the correct position, and uses a macOS-safe nanosecond clock fallback.
- The five-stage build progress reporter is available as global `--progress auto|always|never`; it writes only to stderr and preserves JSON stdout.
- Schema v6 and `devmap history --last N` record real Git identity, file/symbol/edge counts, honest confident-versus-ambiguous dead counts, parse/language coverage, database size, and nullable end-to-persistence duration. JSON exposes deltas; retention is capped at 500. Manifest trend embedding remains open.
- Integrity worklist reconciliation: D1–D17 all have implementations and focused regression coverage in the current tree. D10 (repository-root storage and resolution) and D17 (persisted unresolved-call ledger, schema v9) are both closed as of 2026-08-16. T1–T9 now have falsifiable assertions; T1 was strengthened after the full verifier run and passed in isolation.
- A clean post-fix Python integration snapshot passed **4,128 tests**, with **5 skipped** and **3 warnings**, in **1,051.55 s**. The warnings were two existing Gemini deprecation notices and one pre-existing unawaited `AsyncMock` runtime warning.
- The canonical root `dev map` completed in **0.65 s** and wrote a full repository map/code graph. Running the legacy Python mapper from inside `rust-port/` is intentionally incompatible with its Rust schema-v6 `.devcouncil/codeintel/index.sqlite` (the installed Python mapper supports schema 2); it fails closed and emits a degraded lean map rather than downgrading either database.

## External-corpus stress test (2026-08-15, ScholarLM)

First run of the release binary against a repository other than `testdata/` and DevCouncil.
Corpus: ScholarLM at `539633c` — 4,731 indexed files across 16 languages, Go-dominant
(2,357 Go, 878 TypeScript, 510 TSX, 342 Python, 179 JavaScript, 23 Rust), with 110,049
`node_modules` files and 11,413 `.bench-cache` files present as untracked bulk. Seven builds
plus one DevCouncil baseline, same release binary and host (darwin arm64). The corpus was not
modified: incremental tests ran against an rsync copy of its 5,025 tracked files and its
working tree was clean afterward.

**Correctness held; resource behaviour did not.** Determinism, discovery, parsing, adversarial
robustness and query latency all passed at 4.4× the largest previously tested repository. The
build is usable as a batch indexer at this scale and is not viable as the live watcher it is
designed to become.

Scaling, same binary and host:

| Measure | DevCouncil | ScholarLM | Ratio |
| --- | ---: | ---: | ---: |
| Files | 1,084 | 4,731 | 4.4× |
| Symbols | 11,612 | 46,167 | 4.0× |
| Edges | 58,328 | 410,387 | 7.0× |
| Build time | 3.21 s | 114.39 s | 35.6× |
| Peak RSS | 335 MiB | 10.45 GiB | 32.0× |
| Database | 85 MiB | 525 MiB | 6.2× |

4.4× the files costs 32× the memory and 36× the time — approximately O(n^2.3) in file count.
Memory per file rises from 317 KiB to 2.3 MiB.

### Post-remediation re-stress (2026-08-15)

Same host, one frozen 4,742-file copy of the corpus, all fixes applied. The scaling
premise above was wrong: memory tracked neither files nor edges but **Σ N²**, the squared
ambiguity fan-out (SC3).

| Measure | Before | After | Change |
| --- | ---: | ---: | ---: |
| Peak RSS | 10.45 GiB | **1.41 GiB** | −86.5% |
| Cold build | 124.90 s | **25.85 s** | 4.8× faster |
| One-line-edit rebuild | 110.88 s | **23.11 s** | 4.8× faster |
| Database after 5 builds | 1,507 MiB, unbounded | **834 MiB, plateaued** | bounded |
| Confident non-exempt dead | 636 | **214** | −66% false positives |
| Go `func init()` false-dead | 4 | **0** | closed |
| Rust `#[tokio::test]` false-dead | 29 | **0** | closed |
| Edges / symbols | 410,468 / 46,166 | 410,468 / 46,166 | unchanged |
| Edge-set + ordinal digest | `4584916…` / `46527f7…` | identical | unchanged |

Graph output is byte-identical through the SC3 and SC3b changes — verified by edge-set
*and* edge-ordinal digests, not by reasoning about the types. Dead-symbol counts change
only where SC6 corrected a false positive.

Findings, most severe first:

- **SC1 — generations are never pruned (critical, new).** Four consecutive one-line edits grew
  the database 525 → 852 → 1,180 → 1,507 MiB, +327 MiB each, with all four generations fully
  retained. The Python incumbent on the same repository is at generation **437** retaining
  **2**, steady at 1.41 GiB. Twenty edits projects to ~7 GB and a hundred to ~33 GB. This is a
  regression against the system being replaced and is not described by the B3 ledger entry.
- **SC2 — incremental rebuild saves 5% (critical, known as B3, quantified at scale).** Appending
  one comment line to one Go file and rebuilding took **110.88 s** against **116.74 s** cold.
  Exactly one of 4,742 files changed content hash; generation 2 nonetheless wrote 4,742 file
  rows + 410,571 edges + 46,167 nodes = **461,480 rows**. The extraction cache does work — user
  CPU falls from 135 s to 109 s because parsing is reused — but resolve, analyze and persist all
  re-run over the whole repository.
- **SC3 — peak memory 10.45 GiB (critical, new).** Reproducible across four runs
  (10.26–10.46 GiB) against 335 MiB for the 1,084-file baseline. A 16 GB developer machine will
  swap and an 8 GB CI runner will be killed. Memory tracks edge count, so SC4 is the cheapest
  lever.
- **SC4 — speculative edges are 80% of the graph (high, new).** `Calls` edges by confidence:
  0.2 → **263,639** (80.2%), 1.0 → 38,986 (11.9%), 0.9 → 26,109 (7.9%). Six distinct `.Run`
  methods carry 2,702 inbound edges **each**; fourteen `.Error` methods carry 22,885 edges
  total. Removing the speculative tier leaves 146,748 edges = **31 edges/file**, against the
  DevCouncil baseline's 34 — so the entire superlinear cost is this one tier. Collapsing an
  ambiguous call to a single edge carrying a candidate set would cut the graph ~64% without
  losing ambiguity information.
- **SC5 — database size gate is miscalibrated (medium, new).** 525 MiB actual against a
  4,731 × 80 KiB = 369 MiB scaled allowance: the `verify.sh` gate **fails** on this corpus,
  though `verify.sh` never runs it. Average extraction payload is 228.6 KB/file for Rust and
  57.0 KB/file for Go, well above the per-file assumption. Space is `generation_files` 204 MiB +
  `extraction_cache` 198 MiB + edges 84 MiB — the payload is stored twice. The gate's absolute
  premise still holds: 525 MiB is 2.75× smaller than the Python index's 1.41 GiB.
- **SC6 — runtime and harness entry points reported as confidently dead (medium, new).** 33 of
  636 confident non-exempt dead symbols (5.2%) are invoked, just never by an explicit call: 4 Go
  `func init()` (`telemetry/logger.go:81`, `telemetry/safego_wiring.go:15`,
  `wisdev/autonomous.go:53`) and 29 Rust `#[tokio::test]` functions across four
  `rust_gateway` sources. Root cause is at `crates/devmap-analyze/src/liveness.rs:63`:
  exemption requires the whole file to carry `WiringKind::TestFile` or similar. Go `*_test.go`
  files match and are clean — 9,172 exempted, zero false positives — but Rust's inline
  `#[cfg(test)] mod tests` lives inside production source, so the file cannot be exempted and
  its tests fall through. `init` has no rule at all. Both need symbol-level exemptions keyed on
  attribute and language semantics.

Passed, with evidence:

- **Determinism** — two clean builds produced the identical edge digest
  `543d6bcb1f7e1c48e77433f25fcc6b442d8f83ee1a77426dd80d67971b9800b9` across 410,387 edges, 11×
  the edge count the gate normally exercises; symbol, edge and dead counts matched exactly.
- **Discovery and ignore rules** — zero indexed paths from `node_modules`, `.gopath` or
  `.bench-cache` against 73 GB of untracked bulk.
- **Parsing** — all 4,289 files in linked grammars parsed clean with zero failures. The 442
  failures are exactly the unlinked-grammar languages (markdown, json, shell, sql, yaml, config,
  hcl, html, css, toml), matching the Phase 2 open blocker.
- **Adversarial inputs** — a 1,100,000-byte source was rejected at the 1 MiB cap while its
  sibling indexed; 40 KB of random bytes named `.py` was skipped; syntactically broken Go was
  marked `Partial` and never promoted to clean; 4,000-deep nested parentheses parsed without
  stack overflow (iterative walker confirmed); a symlink loop did not hang.
- **Query latency** — 25–53 ms for search, status and dead over the full 410k-edge graph;
  budget enforcement correct (a broad query reported 1 of 23,807 shown).
- **Fail-closed guards** — `manifest` refused to overwrite the corpus's Python-generated
  `repo_map.json` without `--force`.

Indicative parity against the live Python index (`.devcouncil/codeintel/index.sqlite`,
generation 437, built 2026-08-14). **Not a controlled parity run** — the Python index was built
against a possibly-drifted tree and the two systems model speculative edges differently:

| Measure | Python | Rust | Delta |
| --- | ---: | ---: | ---: |
| Files indexed | 4,245 | 4,289 | +1.0% |
| Symbols | 49,815 | 46,167 | −7.3% |
| Edges | 604,397 | 410,387 | −32.1% |
| Dead symbols reported | 563 | 636 | +13.0% |
| Database on disk | 1.41 GiB | 525 MiB | −63.7% |
| Generations retained | 2 of 437 | all | unbounded |

File counts agree closely, which suggests discovery is aligned. The symbol and edge gaps are
unexplained and warrant a controlled comparison before any cutover decision.

Not run and not claimed: `devmap serve` under sustained load on this corpus (SC2 makes the
per-edit cost ~111 s, so the daemon result is derived from the build measurement rather than
measured directly), multi-repository soak, and any non-darwin host.

## Performance and gap pass (2026-09-02)

A pass over the shipped kernel for bottlenecks and for gaps against a third reference
([zzet/gortex](https://github.com/zzet/gortex)). Eight kernel findings, numbered `K1`–`K8` in
[PLAN.md](PLAN.md) §3; measurement method in §2.1 and the appendix. Every figure below is from
`benchmarks/map_bench.py` against a **scratch** store — the working `.devcouncil/` was not
touched — reporting the minimum of N runs.

### Results

| Stage | DevCouncil · 994 files | scholarlm · 3,674 files | vs. baseline (DevCouncil) |
|---|---:|---:|---:|
| `cold` | 2.13 s | 10.22 s | −36.8% |
| `warm` | 0.196 s | 0.900 s | −14.7% |
| `touch` | 1.31 s | 7.63 s | −35.7% |
| `manifest` | 0.367 s | 1.26 s | −29.5% |
| `e2e` (`dev map`, full) | 1.04 s | — | **−57.5%** |

`e2e` improved far more than `cold` because roughly 70% of `dev map`'s wall time was Python
wrapper overhead around the kernel rather than kernel work. **The seam was the bottleneck, not
the engine** — which is not what this ledger would have predicted, and is the single most
useful thing the pass found.

Cold-build phase split (scholarlm, 13.44 s): extract **7.25 s / 54%**, resolve 3.29 s / 24%,
`persist:write` 2.50 s / 19%, analyze 0.24 s / 1.8%.

### Defects found

Closed, each with a regression that fails against the pre-fix code:

- **K2** — `.proto` and `.ps1` were dropped at *discovery*, not extraction: `is_indexable_source`
  admits only extensions `detect_language` names. 19 `.proto` and 17 `.ps1`/`.psm1`/`.psd1` were
  invisible to the graph on scholarlm. Now 19/19 and 17/17 discovered (9 `.ps1` recover
  declarations, 8 genuinely declare none). Verified against on-disk `git ls-files` counts, not
  against the indexer's own view — the indexer's view is what was wrong.
- **K3** — tier-2 pattern recovery was attributing Go and TypeScript types written inside
  *fenced code blocks in design documents* to the `.md` files describing them: 40 markdown
  files, 457 symbols that exist nowhere. Now 28 fallback files, all real source
  (19 protobuf + 9 powershell), 390 symbols. This was a regression introduced by this pass.
- **K4** — search returned **zero** hits when a matching symbol's source span exceeded the
  whole token budget. Both budget gates stayed green throughout; neither could distinguish an
  empty result from a no-match result.
- **K5** — `vacuum_if_needed` read `freelist_count` before checkpointing the WAL, so it saw 0
  free pages while 33% of the store was reclaimable and reported a 0 ms vacuum as success.
- **K6** — a rebuilt binary left the running daemon answering from superseded code.
- **K7** — the embedding ranker was measurably broken: random-projection hash vectors put the
  correct symbol at rank 28 or absent on all 5 probes. Replaced with TF-IDF; rank 1 on all 5.
  **Superseded 2026-09-02 by `079bc50`, and this entry described a fix that no longer exists.**
  The Python fix landed in `indexing/graph/embeddings.py` as a *stored* index — an IDF table, a
  build step, a generation stamp, a model tag and a `stale_rows_skipped` counter for when they
  disagreed. That module has been deleted and the ranking ported into the kernel as
  `devmap-query/src/semantic.rs`, computed at query time over `generation_nodes` with nothing
  stored. The defect and the measurement stand; the *mechanism* described above does not, and
  the stored-index machinery it depended on is gone rather than fixed.

Also closed, after this section was first written:

- **K1 — grammarless files had no graph node at all.** Not "no symbols": *no node*. The `File`
  node was pushed only on the tree-sitter path, so a file with no linked grammar was recorded
  in `generation_files` and absent from the graph — not an edge target, not returnable by a
  file-level query. K2 and K3 make tier-2 recovery correct when it fires; they did not close
  this. Two doc comments in `fallback.rs` and `treesitter.rs` asserted the opposite ("a `File`
  node and nothing else"); both were corrected, then corrected again once the fix landed.
  `unavailable_extraction` now emits the node unconditionally, prose and data formats
  included. Safe against dead-code analysis by construction — `File` nodes are already exempt
  in `liveness.rs` and skipped as `Contains` sources — so no dead-symbol candidate was added.
  **Verified: 4,089 files on scholarlm, 0 with zero nodes, 0 missing a `File` node**, against
  380 `Failed` files that previously contributed nothing.
- *(closed 2026-09-02, after this section was first written)* **K8 — the phase profiler
  misattributed every measurement by one position**, recording time-since-previous-announcement
  against the *next* stage's label. On the 13.44 s scholarlm build it printed extraction's
  7.25 s beside the word "resolving" and extraction itself as **42 ns**. Now verified by
  execution, not assertion: stages sum to the reported total (20.108 s of 20.110 s), the four
  `persist:*` sub-phases sum exactly to their parent (5.624 s of 5.624 s, so nesting does not
  double-count), and a stage still open when the breakdown is requested is emitted with
  `"open": true` so the accounting can never come up silently short.

### Gaps vs. gortex

Recorded as `G1`–`G8` in [PLAN.md](PLAN.md) §3. **All eight are now resolved**, the last four
by a concurrent session in `079bc50`: G1 language breadth (tier-2 recovery), G2 retrieval
ranking, G3 clone detection, G4 `devmap preview`, G5 notebooks, G6 wire-format compaction,
G7 `devmap savings`. G8 cross-repository graph stays **declined** — already declined in
PLAN.md as a product change, recorded so it is not re-opened as an oversight.

G6 is worth reading rather than ticking. It closed by *measurement* rather than by adopting a
binary format: `repo_map.json` fell 25.7% by dropping pretty-printing and a constant empty
`summary` field on 1,306 entries. A columnar/interned encoding reaching 72% was measured and
**declined**, because it changes the `files`/`dependents` shape that CLAUDE.md documents to
agents as the navigation contract. Saving tokens by breaking the interface agents are told to
use is not a saving.

### Not claimed

**The workspace suite was re-run and is green: 717 passed, 0 failed** (`cargo test --workspace
--no-fail-fast`, exit 0), superseding the stale 683 figure. That run includes a concurrent
session's in-progress clone-detection work alongside K1, K8 and all five class gates.
`cargo fmt --all` is clean.

Two caveats on that number, both real. First, it was taken while another session was editing
the same tree, and an earlier attempt failed in `devmap-store::store_hardening` — a test that
passed in isolation seconds later, because the file was being rewritten mid-run. **The suite
is currently non-deterministic for reasons that have nothing to do with the code**, and a run
should be repeated once the tree is quiet before it is treated as a gate. A second run failed
to *build* — `regex-automata` and `libsqlite3-sys` artifacts vanished mid-run because both
sessions share one `target/` — and succeeded on retry; that is contention, not breakage, but
it is another reason to re-run when the tree is quiet. Second,
`cargo fmt --all` reformatted files belonging to the other session's in-flight work; that is
idempotent and harmless but was not asked for.

Not run and not claimed: soak, multi-platform CI, production validation, mutation coverage.
The Python-side changes (freshness stamping; the TF-IDF embeddings since deleted, see K7) were verified against the
Python suite, and K2/K3 end-to-end against both corpora, before the concurrent edits began.

## Kernel audit, second pass (2026-09-02, evening)

The full record — every defect, its failing-first test, the live before/after
numbers — is the root `IMPROVEMENTS.md` → "Dev Map kernel audit, second pass".
This section is the ledger entry: what is now true of the kernel and its seam,
and what a contributor must verify before trusting it.

### What changed in the kernel (`rust-port/`)

- **Store/build/drain (K):** pending-queue hygiene (`K1` a–h: canonical enqueue,
  structural reconcile, refusals are not work, per-path attempt accounting, build
  reconciles, `devmap repair --pending`, status names stuck paths, drain batch
  64 → 8192; plus: a whole-tree build supersedes every row queued before it
  started). Reclaim that lands (`K2`: checkpoint before *and* after, and every
  row of `PRAGMA incremental_vacuum` stepped — rusqlite's `execute_batch` steps
  once, so the old code freed one page per build). Schema refusals that name the
  binary, the store and the remedy; `devmap --version` prints the schema; status
  never migrates (`K3`). `devmap build --full` (`K4`). Prose/data formats are not
  parse failures (`K5`). Default `--db` is the seam's store (`K6`). Directories
  carrying `CACHEDIR.TAG` are never walked, never `--affected`, never enqueued
  (`K7`). Poisoned mutex is an error (`K12`). Advisory writer lock that names its
  holder, `BEGIN IMMEDIATE` generation writes (`K13`).
- **Serve/query (S):** preview contained to the repository (`S1`); budgets
  honoured on snapshots, workspace search totals and semantic search reads
  (`S2`, `S4`, `S6`); scoped trace O(V+E) with parent pointers (`S3`: 2.67 s →
  0.08 s over 19,656 edges); `workspace.json` atomic + flock (`S5`); a timed-out
  query is cancelled, not abandoned on a pool thread (`S7`); SIGTERM/SIGINT
  release socket and lock (`S8`); socket path from the canonical root, printable
  with `devmap serve --print-socket-path` (`S9`); a daemon whose repository or
  store vanished exits (`S10`).
- **Verification:** `cargo fmt --all --check` clean, `cargo clippy --workspace
  --all-targets -- -D warnings` clean, `cargo test --workspace` **835 passed /
  0 failed** across 62 binaries; 60 new kernel tests, each observed failing
  before its fix. Known parallelism flake, characterised and made robust:
  `protocol::hardening_limit_tests::the_probe_refuses_live_endpoints_and_replaces_stale_files`
  now accepts either fail-closed refusal message.

### What changed in the seam (`src/devcouncil/devmap_engine.py`, `devmap_client.py`, `devmap_health.py`, `indexing/map_artifacts.py`)

- The kernel is the **only** writer of `repo_map.json` / `code_graph.json`;
  `refresh_map_artifacts` is the one build path and raises `DevMapEngineError`
  instead of stamping a lean map. See `CONSUMERS.md` → correction of 2026-09-02.
- One binary-selection rule (newest *capable* build; `DEVMAP_BINARY`), one
  socket-path formula (parity test against `--print-socket-path`),
  `DEVMAP_AUTOSPAWN=0` for probes and test suites.
- **Diagnosable / traceable / fixable while running:** every kernel run is a
  `devmap_run` trace event; a failing build carries `code` / `fix` / `run_id`;
  a running build writes `.devcouncil/codeintel/devmap-build.live.json`;
  `dev map status|doctor|runs|abort`, `dev map doctor --fix`, MCP
  `devcouncil_graph_doctor` / `devcouncil_graph_runs`. Contract:
  `docs/code-graph.md` → "When a map looks wrong".

### Live numbers on this repository (release kernel, schema 12)

| Measure | Before | After |
|---|---:|---:|
| pending rows / quarantined | 47,095 / 188 | 0 / 0 |
| files indexed | 2,363 (1,041 cargo output) | 1,318 |
| store | 669 MB, 66% free | 209 MB, 0% free |
| `dev map` changed / unchanged tree | refused (schema) | 4 s / 1 s |
| `dev map doctor` | critical | healthy |

### Phase ledger consequences

- **Phase 6 — cutover: the build path is cut over.** "Hybrid with Python
  fallback" no longer describes any writer. Query surfaces that still read the
  Python cache: `check`, `process`, `routes`, `shape-check`, `api-impact`,
  `cypher`, `explore`, `affected`, `pdg`, the HTML visualizers, and MCP
  `graph_query` / `graph_trace` / `graph_context`.
- **Python deletion is in progress and was interrupted** — state and next steps
  in `AGENT_PLAN.md` → "Handoff (2026-09-02, evening)".
- Adjuncts (Phase 7) unchanged.

## Required decisions / external gates

1. Approve a concrete set of new/upgraded Tree-sitter grammar dependencies before the remaining 29 language families can be linked and tested.
2. Decide whether B3 warrants the required validity-range/current-state schema redesign now; the existing full-generation carry-forward representation cannot meet <100 row writes at 10k.
   External-corpus evidence (SC2) raises the cost of deferring: at 4,731 files a one-file edit
   costs 110.88 s and 461,480 row writes, so the daemon cannot ship at this scale either way.
   **Updated 2026-09-02.** The 110.88 s figure no longer holds — a one-file edit on a
   3,674-file corpus now costs **7.63 s** (`touch`, min-of-3, scratch store). That is a large
   improvement and it does not change the decision, because the *shape* is unchanged: `touch`
   is still 60% of a full cold build, and `persist:write` is still larger on an incremental
   build than on a cold one (2,448 ms vs 1,490 ms on this repository). Both follow from
   resolution being deliberately global on every changed build — narrowing it was tried during
   this pass and broke liveness and community detection, so the cost is a consequence of a
   correctness decision, not an unexamined one. **The decision is now better posed than it was:
   B3 is not blocked on making the daemon viable at scale (7.63 s is viable), it is a question
   of whether incremental cost should scale with the change or with the repository.** Deciding
   it also decides whether the global-resolution constraint has to be revisited first.
3. Approve Phase 6 edits to the remaining frozen Python consumers when shadow parity data is ready; current repository rules prohibit changing the Python indexing owners during the port.
4. The two-week shadow soak, multi-platform CI/publication, and live production validation require elapsed/external work and cannot be represented by local unit tests.
5. Approve installing `cargo-mutants` if the optional mutation gate is required locally; it is not installed and repository policy forbids adding tools/dependencies without approval.
6. ~~Decide the generation retention policy (SC1).~~ **Resolved 2026-08-15 — no decision was needed.** The audit found `Store::prune_generations_except_latest` (`crates/devmap-store/src/db.rs:1543`) already implemented and correct, with zero production call sites: the only callers were two unit tests. SC1 was a wiring defect, not a missing capability, and needed no schema change. Retention is now `GENERATION_RETENTION = 2`, wired into both commit paths. **Correction:** the alias-chaining dependency (S16) cited here earlier does not exist in the Rust port — `devmap-resolve` depends only on `devmap-extract` and `serde`, contains zero references to generations or the store, and there is no `aliases` table in the schema. S16 describes an unimplemented feature, not a live constraint. The dependency is real in the *Python* incumbent (`src/devcouncil/codeintel/store/sqlite.py:1284`), which needs exactly one prior generation.
7. Decide whether to collapse speculative ambiguous calls into one edge carrying a candidate set (SC4). This is a representation change to the resolver and store, not a precision change, and it is the single largest lever on both the SC3 memory ceiling and SC2 build time.
8. Approve adding a peak-memory gate and an external-corpus build gate to `verify.sh`. Neither exists today; the current gates measure only wall time and database size against DevCouncil, which is why SC1, SC3 and SC5 were not caught. Recalibrating the 80 KiB/file size assumption (SC5) depends on the outcome of decisions 6 and 7.
   **Updated 2026-09-02 — the external-corpus half now has an implementation to approve, and
   the peak-memory half still has nothing.** `benchmarks/map_bench.py` takes `--repo` and runs
   any tree against a scratch store, with `--baseline` for run-to-run diffing and corpus sizes
   recorded in each result; it has been exercised on DevCouncil and scholarlm. Wiring it into
   `verify.sh` is a decision, not new work. It measures **wall time and artifact sizes only —
   it does not measure RSS**, so it closes none of decision 8's memory half.

   Two findings from this pass argue the gate list is still missing a category rather than
   just a threshold. **K4**: a query that matched returned zero results while both budget
   gates stayed green, because neither could tell an empty result from a no-match result.
   **K5**: the freelist gate as written reads a pre-checkpoint counter that reported 0 free
   pages on a store that was 33% garbage — a gate on that number would have passed a store it
   was specifically meant to catch. Both are the `N4` property applied to the gates themselves:
   *a check that could not run must not report as one that ran and passed.* Approving new
   gates without fixing what the existing ones measure buys less than it appears to.

9. **~~Approve the five class-level gates in [PLAN.md](PLAN.md) §3.1~~ — all five are implemented as of 2026-09-02 and need no approval.** Superseded by decision 11, which carries what is left. The Class B gate is `devmap-cli/tests/coverage_invariants.rs` (three assertions, all failing against the pre-fix tree); it is the shape that caught K1 and K2, and it is stated over the filesystem rather than the index because that is the only vantage from which a file the pipeline never saw is visible. Added 2026-09-02. Grouped by failure *shape* rather than by subsystem, the 47 findings collapse into five classes — and four of the five produced a **fresh instance in the Rust port after the Python instance had been found, fixed, and written into the plan as an acceptance property**. Class A ("a check that could not run reports as one that ran and passed") produced three instances across three unrelated subsystems: an analysis timeout (N4), a query budget (K4), and a storage reclaim (K5).

   The evidence that per-instance acceptance tests are not sufficient is direct: **all six defects closed in the 2026-09-02 pass were found by measurement, and none by the acceptance tests written to prevent their class.** The gates are stated in §3.1 and are auditable rather than aspirational — Class A's, for example, is mechanical: any function returning a bare collection, count or `bool` where a timeout, budget exhaustion, cap or skip is reachable is a candidate for review.

   This is a decision rather than work-in-progress because the Class A and Class B gates would each fail against parts of the current tree, and deciding to add them is deciding to fix what they catch. Class B's in particular — *every coverage claim stated over `git ls-files`, never over the indexer's own inventory* — is what surfaced K2, and is the only reason two entire languages missing from every graph were noticed at all.

10. **Decide the MCP protocol pin and the plugin packaging target.** Added 2026-09-02; see [PLAN.md](PLAN.md) §9. Two findings make this a decision rather than a task.

    First, **this repository has already accepted the largest breaking revision in MCP's history through a version range.** `pyproject.toml:18` pins `mcp>=2.0.0,<3`; the 2.x SDK line carries the **2026-07-28** protocol revision, which removes the `initialize`/`initialized` handshake and `Mcp-Session-Id` in favour of a stateless core, replaces server-initiated elicitation/sampling/roots with Multi Round-Trip Requests, and requires `Mcp-Method`/`Mcp-Name` headers on streamable transports. There is no compatibility statement anywhere in the tree. ("MCP 2.0" is an SDK major version, not a protocol version — the specification is date-versioned.)

    The good news is measured, not assumed: the server is **stdio-only and uses none of the deprecated or replaced features** — verified by search, no elicitation, sampling, roots, or MCP `logging/setLevel` call site exists. So today's exposure is latent rather than active. It becomes active on any remote transport, where header routing and the RFC 9207 authorization changes are prerequisites rather than enhancements.

    The cheapest item is additive and should not wait for the decision: **`ttlMs` / `cacheScope` on `tools/list`**. This repository ships 18 map/graph tools whose definitions are paid for on every request whether called or not — the same cost T1 and R1 target, with no migration.

    Second, **plugin packaging has no owner.** DevCouncil ships the components a plugin packages — MCP server, skills, hooks — distributed by repository checkout. SC21 is recorded as closed for the *binary*; the packaging class it belongs to is open. Claude Code's format offers three things that map onto open items: `${CLAUDE_PLUGIN_DATA}` as a per-installation writable home for a store now living at a repository-relative path, `userConfig` with `sensitive: true` as the declared way to take a token without it reaching a config file, and lockfile-driven dependency install with no lifecycle scripts. **Agent Plugins 1.0 is a separate coalition spec that Claude Code did not adopt**; targeting both means maintaining two manifests for one artifact, which is the decision to make.

11. **~~Fund the four unbuilt class gates~~ — built 2026-09-02. Decide whether to run Class A's workspace-wide audit.** Added 2026-09-02; see [PLAN.md](PLAN.md) §10. Seven worst-case scenarios are now written down, and **five of the seven have a demonstrated mechanism** from the 2026-09-02 pass — invented symbols in the graph (W1/K3), a language silently absent while parity was green (W2/K2), gates staying green over an unexamined system (W3/K4+K5), evidence that could not be reproduced under a concurrent writer (W6, demonstrated during the session itself), and a measurement attributed to the wrong phase (W7/K8). None of the seven is a crash: each produces a system that runs, answers, and reports success while being wrong.

    **All five class gates are now implemented** — 14 assertions in `devmap-cli/tests/coverage_invariants.rs` (Class B), `devmap-cli/tests/failure_class_gates.rs` (A, C, D) and unit tests beside `ProgressReporter` in `devmap-cli/src/main.rs` (E). Each was checked against a mutation or a revert rather than merely observed to pass: reverting K1/K2 turns all three Class B assertions red, and forcing a stage to record no time of its own turns Class E's attribution test red while the others stay green. W3 — the scenario where the shadow soak passes because its checks degrade to no-ops under exactly the load they exist to test — now has a gate.

    **What remains a decision.** A gate proves the shape holds *at the points it inspects*, not universally. Three coverage holes are known and named: Class A's mechanical audit (every function returning a bare collection or count where a decline is reachable) has **not** been run across the workspace; Class D does not cover the HEAD boundary (B5); Class E covers the build profiler and not every reported measurement. The Class A audit is the one worth funding — it is the shape that produced three instances in three unrelated subsystems, and the gate currently pins three known surfaces rather than the class.

## Independent integrity audit (2026-08-12, separate auditor session)

Full static audit of the workspace is in **[INTEGRITY.md](INTEGRITY.md)**. Worklist for the
build agent, in priority order:

- **Defects D1–D17** — D1 (dead receiver-resolution rung) is HIGH; D2–D7 medium
  (vacuous ambiguity guard, cache-that-never-caches, batch poisoning, missing
  busy_timeout/foreign_keys, ambiguous-only-dead confidence, route-error→Failed conflation).
- **Vacuous tests T1–T9** — strengthen before running the mutation gate; several currently
  cannot fail (assertions behind `if let`, generation-count-only checks, Mutex-serialized
  "concurrency" test).
- **Disruption protocol** — cargo-mutants gate + fault-injection matrix additions.
- **Insights-over-time** — schema-v6 `build_history`, `devmap history`, real `head_sha`, and
  real `pending_count` are implemented; the bounded manifest `trends` block remains open.
- Verification pipeline: `./verify.sh` (fmt → clippy → tests → determinism digest →
  self-build gates → optional `--mutants`).

Remediation follow-up from the build session:

- Closed with red regressions or direct invariants: D1–D9, D11–D13, D15–D16, edge dedup, and T1–T9. D5 and T5 now exercise explicit connection pragmas and two independent WAL handles.
- D10 is fail-loud (`source_unavailable_reason`) but durable snippets remain an explicit Phase 5 storage tradeoff.
- D14 is closed for repository-scoped defaults and path-length validation; a caller-supplied socket outside the managed runtime directory still relies on immediate post-bind `0600` hardening.
- D17 is closed: the durable unresolved-reference ledger landed as schema v9, pruned with its generation.
- Mutation testing, the kill-at-every-migration-boundary matrix, 30-minute soak, and Windows process CI were not run and are not claimed.

## Kernel hardening and honesty pass (2026-09-04)

Driven by a workspace-wide audit of one defect class — *a check that could not run must never
report the same result as a check that ran and passed* — plus a measured read-path profile.
Every fix below ships a test that was **verified red against the unmodified tree** before the
change; where the honest shape is a new type, the red was reproduced a second time by neutering
the new logic and re-running, so the test pins behaviour rather than the type's existence.

**Suite: 844 passed / 0 failed** (`cargo test --workspace`), `cargo fmt --all --check` clean,
`cargo clippy --workspace --all-targets` clean. Release binary rebuilt.

### Fixed

- **`repo_map.json` claimed a healthy graph it had not verified.** `manifest.rs` wrote
  `"graph_degraded": false` and `"graph_degraded_reason": ""` as literals on every path, while
  the same `AnalysisSummary` in the same build could say `Partial` — and `code_graph.rs:471`
  rendered it honestly. The consumer is `RepoMapper.map_is_stale`
  (`src/devcouncil/indexing/repo_mapper.py:1965`), whose fail-closed branch
  `if bool(repo_map.get("graph_degraded")): return True` **could never fire**, so `--if-stale`,
  `watch` and `verify` accepted a map built from a Louvain partition that never converged. Both
  keys now derive from `analysis.status`.
  Test: `a_degraded_analysis_is_not_reported_as_a_healthy_graph`.

- **`unwired_candidates` was a hardcoded `[]` in `repo_map.json`** while `code_graph.json`
  computed it — from the same `extractions` and `edges` that `consumer_manifest_json` already
  receives — so one build produced two artifacts contradicting each other, and an agent reading
  the map concluded nothing was unwired. Now computed, capped at `UNWIRED_CANDIDATE_CAP` (=
  `DEAD_CANDIDATE_CAP`) with the true total beside it.

- **`liveness_unreachable_unreliable` was conditional in the map and unconditional in the
  graph.** File-level reachability is never computed by this kernel, so `code_graph.rs:559` sets
  the flag unconditionally; `manifest.rs` gated it on `entry_roots.is_empty()`, which is false on
  any repository with an entry root — exactly disarming the escape hatch `docs/code-graph.md:274`
  tells agents to rely on. Now unconditional, with `liveness_meta.unavailable.unreachable_files`
  carrying the reason.
  Test: `the_manifest_reports_liveness_it_computed_and_flags_what_it_did_not`.

- **`liveness_meta.entry_roots.count` reported the truncated length as the total.**
  `entry_roots` is capped at 20 for the token budget and `count` was the post-cap length, so a
  25-root repository had `code_graph.json` listing 25 and `repo_map.json` saying 20. Downstream,
  `subsystem_map.py:123` `is_entry_root` reads the capped list and `handlers/map.py:361` emits
  `is_entry_root: false` for every genuine entry root sorting after the 20th. `entry_roots`,
  `dead_symbol` and `unwired` now each carry `{shown, total, truncated}`; `dead_symbol.count` is
  retained and has always meant the true total.
  Test: `entry_root_count_is_the_true_total_not_the_truncated_length`.

- **`trace` reported "no indexed path" when it had merely hit its depth cap.**
  `shortest_path` returned `Option<Vec<_>>`, collapsing four outcomes — zero budget, frontier
  pruned at `max_depth`, node cap, and a genuinely exhausted reachable set — and `trace_between`
  stated the strongest of them as fact. At the default depth, a path longer than `--depth` read
  to an agent as proof that two symbols are unrelated. Now `PathSearch::{Found, NoPath,
  Exhausted{depth_capped, node_capped, visited, ..}}`, and only `NoPath` produces the original
  sentence. `depth_capped` is raised only when a pruned node actually had unexplored successors,
  so a bounded walk over a small graph does not call itself uncertain.
  Test: `a_depth_capped_trace_is_not_reported_as_proof_that_no_path_exists`.

- **`impact` presented a capped walk as a complete blast radius.** `traverse_graph` had five
  silent decline paths (starts dropped by `max_nodes`, depth prune, node cap, enqueue cap,
  recorded-edge cap) and `TraversalResult` had no field for any of them; `budget_take` then
  computed `total` from what it *received*, so a truncated walk reported `truncated: false` with
  `total == shown`. Since the default depth is 3 and real graphs are deeper, this fired on
  essentially every call. `TraversalResult` now carries `TraversalStop`, and `Response<T>` gained
  `walk_incomplete: Option<String>` — deliberately separate from `shown`/`hidden`/`total`, which
  describe the *budget* and which clients check as `shown + hidden == total`; a walk that
  withheld an unknown quantity cannot be expressed there without breaking that invariant. Omitted
  from the wire form when the walk was complete. The CLI prints it beside the truncation line.
  Test: `a_capped_walk_reports_that_it_stopped_early` (verified red twice: once against the
  missing type, once by neutering the flag assignment).

- **A public class was reported dead at 0.90 confidence in 21 languages.**
  `generic_is_exported` read `get_node_text(node, source)` — the *whole* subtree, bodies included
  — and returned `false` on a `private `/`protected ` substring anywhere inside it. So a public
  Scala class holding one private field persisted with `is_exported = 0` and `devmap dead`
  proposed deleting it, on evidence that is not about the class at all. Java escaped only because
  `"public "` is tested first and Java spells the modifier — an accident of one keyword set.
  The scan is now bounded to `declaration_header`: the node's text up to the start of its body,
  which is the only region a visibility modifier can legally occupy. A modifier on the
  declaration itself is still read.
  Test: `a_private_member_does_not_make_its_enclosing_declaration_private`
  (pre-fix: `("Reg.scala::Registry", Class, false)`).

- **Every truncation report in `devmap-extract` was erased before persist.**
  `fallback::scan_declarations` computes `truncated` correctly and `extract_treesitter` pushes it
  into `diagnostics` under the comment "Reported, not silently dropped" —
  `Extraction::for_durable_store` then calls `diagnostics.clear()` on the payload written to
  `generation_files.extraction_json` and the extraction cache, and **no production code anywhere
  in the workspace reads `diagnostics`**. A 2,500-declaration `.proto` stored 2,000 symbols under
  the reason *"2000 declaration(s) recovered by pattern"*, and the missing 500 were
  indistinguishable from declarations that do not exist. The count now rides on the
  `ParseOutcome::Fallback` reason, which survives `for_durable_store`.
  `EXTRACTION_SCHEMA_VERSION` 28 → 29 so v28 rows cannot resurrect the old string.
  Test: `a_truncated_fallback_scan_survives_the_durable_store`.

- **A daemon start failed spuriously ~30% of the time under load, and said the wrong thing when
  it did.** `lock_ipc_endpoint` matched `Err(_busy)` over `TryLockError`, collapsing `WouldBlock`
  (contention) with `Error(io::Error)` (the check could not run) into one sentence asserting
  another live daemon owned the endpoint — a definite claim about another process, made by a
  check that never completed. It also treated a transient `WouldBlock` as permanent with no
  retry. `devmap-serve --lib` failed **6 of 14 runs** on
  `the_probe_refuses_live_endpoints_and_replaces_stale_files`; instrumenting the error showed
  `WouldBlock` clearing within 5–20 ms in every observed case, with `lsof` at failure time
  showing no remaining holder. The two variants are now distinguished, and contention is waited
  out for a bounded `LOCK_CONTENTION_WINDOW` (125 ms, a quarter of the `LIVENESS_PROBE_TIMEOUT`
  the very next step already spends). **0 failures in 15 runs** after.
  Tests: `a_briefly_held_endpoint_lock_is_waited_out_not_refused` (verified red by disabling the
  retry), `an_unlockable_lock_file_is_not_reported_as_a_live_daemon`.

### Measured, not fixed — characterised for the next session

- **~~`impact`/`trace` cost ~100 ms flat regardless of `--depth`~~ — fixed and measured
  2026-09-04.** The cost was never traversal: `resolved_edges` → `Store::latest_edges` re-ran a
  two-JOIN, fully-ordered scan of all 71,598 edges on **every** request. `Store` now holds the
  latest generation's full edge set, keyed by generation id so a committed build invalidates it
  by construction; `min_confidence` is applied per request against the same rounding rule the SQL
  used, so answers are unchanged.

  **A/B on one settled store (gen 799, 14,189 nodes, 71,598 edges), same harness, warm daemon,
  6 `impact` calls at depth 3:**

  | | cache bypassed | with cache |
  |---|---|---|
  | `impact` min | 89.0 ms | **36.6 ms** |
  | `impact` median | 96.2 ms | **38.5 ms** |
  | RSS at startup | 162.7 MB | 161.3 MB |
  | RSS after 26 queries | 480.4 MB | 499.8 MB |

  **Median latency −60% (2.5×) for +19.4 MB (+4%) of steady-state RSS**, which is the one
  retained edge set and matches the 14–20 MB predicted. Decision #8's memory concern was
  answered rather than deferred, and the answer inverted the intuition: the *uncached* daemon
  already allocated a fresh 71,598-edge vector per query and did not return the memory (531.6 →
  625.2 → 801.4 MB over 26 queries in the first run), so the cache replaces an unbounded series
  of transient allocations with one bounded retained one. The ~180 MB of growth over 26 queries
  is present in **both** arms — it is the per-request filtered clone, pre-existing and unchanged
  by this work.

  Correctness is pinned by `the_edge_cache_is_invalidated_by_a_new_generation`
  (`devmap-store/tests/test_fault_injection.rs`), which was **first written vacuous and caught
  as such**: both generations of the original fixture had 13 edges, so a length assertion passed
  with the generation key deliberately disabled. The fixture now adds callers so the second
  generation has strictly more edges, and the test fails with the key disabled.

- **Notebook cell truncation has the same shape as the fallback truncation above and is
  unfixed.** `notebook.rs:190` reports a `MAX_CELLS` overflow into `diagnostics`, which
  `for_durable_store` clears. A 5,201-cell notebook stores `parse_outcome: "Clean"` with one
  symbol. The honest fix needs a `ParseOutcome` variant (or a durable coverage field) rather than
  a reason-string edit, because the outcome here is `Clean` rather than `Fallback` — and the same
  variant is what finding H5 below wants. Left for one deliberate change instead of two partial
  ones.

- **`ParseOutcome::Failed` is reported for prose and data formats** — 293 of 1,294 files (22.6%)
  in the live store, engine `NotApplicable`. `Extraction::is_parse_failure()` reads it correctly
  and `db.rs:2218` uses it, so `history.parse_failed` honestly reports 1, not 294. But three
  other consumers read the raw outcome: `devmap deps README.md` answers *"README.md could not be
  parsed"*, `preview` says the buffer did not parse, and `liveness.rs:275` exempts every `.md`
  as a parse failure. A fifth variant `NotApplicable { language }` is the fix.

- **A grammar that fails to load is reported as a grammar that does not exist.**
  `extract_treesitter` funnels three outcomes into `unavailable_extraction`: no arm in the
  grammar table, `set_language` refusing the grammar (an ABI mismatch — i.e. exactly what a
  tree-sitter upgrade regression looks like), and `parser.parse` returning `None`. The resulting
  reason string *"no linked tree-sitter grammar for rust"* is false in the second case. A
  tree-sitter bump that broke the Rust ABI would silently downgrade every `.rs` file to regex
  fallback, be cached (`cache_admits(Fallback) == true`), exempt every Rust file from dead-code
  analysis, and leave the build green with no call edges. Nothing anywhere would say a grammar
  failed.

- **`CALL_EXTRACTION_LANGUAGES` (`langcalls/mod.rs:72`) has no production reader and is wrong by
  omission.** Its doc says it is "read by the coverage report"; `rg -uu` finds five call sites,
  all in `tests/`, and no coverage report exists. It also omits the ten languages whose calls
  come from `treesitter.rs` arms (Python, JS, TS, TSX, Rust, Go, C, C++, ObjC, CUDA), so a
  consumer that *did* read it would conclude Python has no call graph. See the SC34 correction
  above for the definitive 23-yes / 12-no table.

### Adversarial sweep (2026-09-04) — one 4 KB file could stall the indexer for over three minutes

Added `devmap-extract/tests/adversarial_corpus.rs`: every one of the 35 language specs against
14 hostile inputs (empty, whitespace, NUL bytes, BOM, CR-only line endings, a 200 KB single
line, 500 unbalanced delimiters in each direction, 2,000-deep nesting, polyglot declaration
soup, dense multibyte, a 50,000-character identifier), asserting no panic, no span outside the
source, no inverted span, no unnamed symbol, no callee that joins on an empty key, no
declaration invented from input containing no identifiers, and no non-clean outcome without a
reason. The sweep is stated over `LANGUAGE_SPECS` rather than a fixture list, and asserts its
own coverage count — a language nobody wrote a fixture for is exactly where an unchecked
assumption survives.

**It found a denial-of-service-shaped defect immediately.** A 4,000-byte C++ source consisting
of 2,000 nested braces took **199 seconds** to extract; Ruby took 8.8 s on the same input and
Python 58 ms. On a repository containing one generated or minified file of that shape, `dev map`
looks like a hang and a daemon rebuild holds its lock for the duration. Nothing reported it —
there was no per-file bound anywhere in the extractor.

**The first diagnosis was wrong, and measuring is what caught it.** The obvious cause is a
pathological parse, and a `tree_sitter::ParseOptions` progress callback was added to bound it.
It did not help: instrumenting the callback showed it firing 120 times with the parse
**completing in 2.98 ms**. The 199 s is entirely post-parse, in `walk_tree` and the per-node
extraction it drives — a superlinear cost over a 2,000-deep tree. Bounding the parse alone would
have shipped a bound that bounds nothing, and the test would still have passed on a machine fast
enough. *(Checked separately: this is pre-existing, not introduced by the `generic_is_exported`
change in this pass — the 199 s figure was measured with that change reverted, and the fixed
code is the faster of the two at 131 s.)*

**Fixed** by a single `DEFAULT_PARSE_BUDGET` (5 s) covering parse *and* walk: the parse carries a
deadline progress callback, and `walk_tree` checks the clock every `DEADLINE_CHECK_STRIDE` (256)
nodes and returns whether it completed. An overrun returns `refused_extraction` — a new sibling
of `unavailable_extraction` that reports `ParseOutcome::Failed` naming the budget, emits the
`File` node and **nothing else**. Two properties are load-bearing there: a cancelled tree-sitter
parse can still hand back a partial tree describing a prefix of the file, and the walk's
partially-filled `symbols`/`calls` describe a prefix of the tree — publishing either would be a
truncated extraction wearing a clean outcome, which every consumer reads as "these symbols do
not exist". No declarations are recovered by pattern either, because a file whose parse was
abandoned has an unknown structure.

`refused_extraction` also closes finding **H3**: `set_language` refusing a grammar that *is*
linked no longer falls through to `unavailable_extraction`'s *"no linked tree-sitter grammar for
{lang}"*. That sentence was false for an ABI fault — precisely what a tree-sitter upgrade
regression looks like — and it would have downgraded every file of a language to regex fallback,
cached the result, exempted them all from dead-code analysis, and left the build green.

5 s is far above any legitimate file measured here (the slowest real source parses in
single-digit milliseconds) and far below the pathological case.
Test: `a_pathological_source_is_refused_within_its_budget` — the `devmap-extract`
`stress_hardening` suite went from **159.87 s to 1.70 s**.

**Not fixed: the underlying superlinear walk.** The bound stops the bleeding; it does not explain
why 4,000 nodes cost 199 s. The shape points at an ancestor walk that copies node text at each
level (O(nodes) x O(depth) x O(text)), and `c_declaration_head` / `generic_enclosing_type` are the
candidates to profile first. Until that is done, a legitimate deeply-nested file will hit the
budget and be refused rather than indexed — an honest refusal, but a coverage loss.

### CLOSED — the vendored COBOL grammar does not terminate on malformed input (unlinked 2026-09-04)

Found by the adversarial sweep on 2026-09-04, and the most serious thing it turned up.

**Reproducer.** `extract_treesitter_with_budget("h.cbl", "cobol", "a\0b\0c\n", 300ms)` — a
**six-byte** source — ran past **three minutes** with no sign of finishing. So did
`"\u{feff}????\n"` (BOM followed by four question marks). For scale, the identical 14 hostile
inputs across the other **34** grammars complete in **5.03 s total**. COBOL is the only grammar
of the 35 that does this; that was established by running the sweep with COBOL excluded and
watching all 476 remaining cases pass in seconds.

**No in-process bound stops it.** The `DEFAULT_PARSE_BUDGET` added in this pass cannot:
`tree_sitter::ParseOptions`' progress callback is only polled from the parser's action loop
(`parser.c: ts_parser__check_progress`) and is never reached from inside a scanner that is
spinning, and Rust cannot kill a spinning thread. The deprecated `cancellation_flag` and
`end_clock` are checked at the same point and fail the same way.

**Mitigated, not fixed.** A source containing a NUL byte is now refused at the boundary before
any grammar sees it (`refused_extraction`, reason names the offset) — that closes the NUL case
for every grammar, including vendored ones this repository does not control, and is correct on
its own terms since a file containing a NUL is binary and every extractor here assumes text.
**The BOM case is not covered and there is no boundary rule that would cover it** without
rejecting legitimate files.

**Exposure.** One `.cbl` file with a stray NUL or a mangled header hangs `dev map` outright.
Under the daemon it is worse: the build holds its writer lock for the lifetime of the process,
so every subsequent query degrades and no later build can start.

**Resolved by measuring what the grammar was worth, not by weighing risk against a guess.**
On a realistic COBOL program — IDENTIFICATION/DATA/PROCEDURE DIVISION, two paragraphs, a
`PERFORM` — the grammar parsed `Clean` and yielded **only the File node**: zero declarations,
zero calls. The bounded fallback scanner recovers nothing either. COBOL was already one of the
twelve languages with no call extraction. So the grammar contributed nothing measurable while
carrying an unbounded hang, and there was no trade-off to make.

`cobol` is now listed in `UNSAFE_GRAMMARS` (`treesitter.rs`) and refused by
`refused_extraction` **before any parser sees the source**, with a reason that says why — so a
maintainer cannot silently re-link a grammar that hangs. COBOL files still get a File node and
stay addressable as edge targets. `grammar_matrix.rs` carries it as the second documented
exception beside VB.NET, each with its reason, so adding a third is a deliberate edit.

**The sweep's exclusion list is now empty.** `UNRUNNABLE_GRAMMARS` in
`adversarial_corpus.rs` is `&[]`: COBOL is back under test, exercising the refusal, and all
**35** specs × 14 hostile inputs run in 6.3 s with no exclusions. The coverage assertion
(`covered + excluded == total`) and the loud exclusion reporting stay, for the next grammar that
needs them.

### The MCP graph read path moved to the kernel (2026-09-04) — and the tool that did *not* get faster was the one worth chasing

`devcouncil_graph_query` and `devcouncil_graph_trace` were the last two Dev Map read tools still
answered by Python (`code_graph.py` loading `code_graph.json` and walking it in the interpreter).
Both now ask the Rust kernel first and fall back to Python only when the kernel declines — no
index, an empty generation, a symbol the store does not carry. Every response names the engine
that produced it (`source: "devmap"` vs `source: "code_graph"`), because the two do not always
agree and a caller must be able to tell which answered.

`devcouncil_graph_context` moved in the same pass, from a `subprocess` invocation of the CLI to
an in-process call.

Measured on a settled store (gen 799, 14,189 nodes, 71,598 edges), `DEVMAP_AUTOSPAWN=0`,
min-of-4:

*The Python column was measured on this repository (where the kernel declines, because its
own store carries a `node_count: 0` generation) and the kernel column on an equivalent settled
store. Same content, different roots — treat the ratio as approximate. The controlled number is
the same-process A/B below.*

| tool | Python | kernel | |
|---|---|---|---|
| `graph_context` | 688.5 ms | **0.83 ms** | −99.9% (subprocess removed) |
| `graph_trace` | 1014.6 ms | **379.2 ms** | −63% |
| `graph_query` | 1099.1 ms | **503.1 ms** | −54%, after the fan-out fix below, and now answering `callees` |

**The routing alone did not make `graph_query` faster — 1099.1 ms Python, 1112.1 ms kernel — and
the reason was that the bottleneck was never the language.** Its kernel path is a *composition*:
one `search`, then `impact` + `deps` for each of the first five definitions
(`_QUERY_EDGE_DEFINITION_CAP`), so 9-11 separate kernel calls. `DevMapClient._request` falls back
to a `devmap` subprocess whenever no daemon socket is live, so each of those was a process spawn.
Rewriting the analysis in Rust could not touch that. Only removing the fan-out could.

**So the fan-out was removed.** A composed `neighbors` command now answers both call-graph
directions for several targets in one exchange: `IpcCommand::Neighbors` for the socket transport,
a `devmap neighbors` subcommand for the subprocess one, and `StoreQueryEngine::neighbors` as the
single implementation both share. Eleven exchanges became one.

Measured same-process, same store, same binary, min-of-5, with the batch disabled to reproduce
the old path exactly: **828.3 ms → 359.9 ms, 2.30x** — and the two payloads compare
**byte-identical**. That equivalence is the load-bearing check, not the timing: a faster query
that answers something different is a regression with a good benchmark. It is asserted in
`neighbors_composition.rs` against the very calls it replaces, and verified red by swapping the
two directions.

The fan-out bound is **refused, never applied**: more than `MAX_NEIGHBOR_TARGETS` (16) targets is
an error at the IPC boundary and again in the engine, because trimming would hand back a short
list that reads exactly like a complete one. Verified red against a trimming implementation. The
Python client duplicates no limit of its own, so drift between the two surfaces appears as a loud
refusal rather than a quiet short answer. A `devmap` binary predating the command makes the batch
request fail and the per-target path answers instead — slower, identical, not broken. *Identical*
is enforced, not assumed: that fallback had to learn the same shape rule (fall through to the
traversal when `deps` declines), because otherwise a stale binary would report every symbol's
callees as unknown while the batched path reported them, and two paths that quietly answer
differently are worse than one that is merely slower.

**That fallback was, at first, broken in exactly the way it existed to prevent.** It called
`client.neighbors(...)` inside `try/except DevMapClientError` — which does not catch
`AttributeError`, and a client predating the method raises precisely that. So the case the
fallback was written for took the whole query down. The full Python suite caught it (five tests
in `test_graph_cmd_command.py`, whose `_FakeClient` is exactly that older shape) and it is worth
recording that the *unit* tests around the new code all passed, because their stubs had the
method. The fix probes rather than catches — `callable(getattr(client, "neighbors", None))` —
because an `except AttributeError` around the call would also swallow one raised inside the
response handling, turning a genuine shape error into a silent downgrade. `trace` is probed the
same way, and a client lacking it keeps the reason `deps` gave rather than losing it.
`test_a_client_without_the_batched_command_falls_back_instead_of_crashing` is red against the
original version.

**The measurement also exposed a gap that was then closed: `callees` never carried anything.**
For a symbol-shaped target the outbound side was *always* `Unavailable`, because `dependencies`
resolves a file path and a symbol id is not one. The field was honest and useless — and the
earlier attempt to fix it by asking about the containing file was worse, since a function's
"callees" became its whole file's outbound edges, which is why symbols appeared to call
themselves.

`neighbors` now picks the query by target shape, using `latest_file` — the store's own notion of
what a file is — rather than sniffing for `::`. A file keeps `dependencies`; a symbol gets the
forward traversal, which is symbol-scoped and answers exactly the question. On a live probe,
`RepoMapper.map_is_stale` went from `deps resolution unavailable: … is not indexed` to its six
real outbound `Calls` edges. `a_symbol_target_gets_its_own_callees_not_its_file_s` pins both
failure modes — reporting nothing, and reporting the file's edges — and is red against the
pre-fix engine with the exact "is not indexed" message.

**This is a behaviour change, and it costs time.** Callees now do real traversal work where they
used to fail instantly, so the end-to-end number is not the pure transport figure:

| | per-target fan-out | batched | |
|---|---|---|---|
| transport only, identical semantics | 828.3 ms | **359.9 ms** | 2.30x |
| as shipped, with callees answered | 1398.8 ms | **503.1 ms** | 2.78x |

The second row describes the shipped tool: 2.78x faster *and* 4 of 6 definitions gained a callee
list they never had (0 before). Both rows moved after an adversarial sweep rewrote what `callees`
means — see the next entry.

**`graph_trace`'s answers change, and the kernel is the correct one.** Python's BFS is
undirected across five edge kinds; the kernel's is directed over resolved edges. On a live probe
Python reported a path between two functions that ran through a test module importing both,
where the kernel correctly reported none. Callers relying on the old permissive behaviour will
see fewer paths.

**`handle_graph_impact` was deliberately left on Python.** The MCP tool is path/diff-shaped
("what does changing these files affect") while the kernel's `impact` is symbol-rooted. Routing
it would have silently changed the question the tool answers. Not a gap — a different tool.

Two process notes worth keeping. The first version of this wiring passed `query=` where the
callee reads `kwargs["name_or_path"]`; it would have raised on **every** call, and every mocked
test stayed green. `test_handlers_call_the_kernel_with_the_kwargs_it_actually_declares` drives
the real function so the keyword contract is pinned, and it is red against that bug. And the
first measurement returned `source="code_graph"` for both tools — the kernel had correctly
declined, because this repository's own store carries a `node_count: 0` generation. The numbers
above come from a store built and settled specifically for the measurement; a benchmark against
a store that is still sweeping measures the sweep.

**Follow-up left in the code:** `_devmap_query_payload` lazily imports from
`cli.commands.graph_cmd`, so the MCP layer now depends on the CLI layer. It belongs in a module
neither owns.

### `graph_runs` reported "I could not look" as "nothing happened" (2026-09-04)

Found by auditing the Dev Map MCP handlers for the Class A shape rather than by a failing test.
`devcouncil_graph_runs` returned `{"ok": True, "runs": []}` for **three** different outcomes:

1. the project has no trace log at all (`telemetry/traces.py:84` returns `[]` for a missing file),
2. the log exists but every line failed to parse — they are skipped at *debug* level
   (`traces.py:91-94`), and
3. the kernel has genuinely never run.

Only the third is what an empty list reads as, and a caller asking "has this been built?" acted
on the other two as though it had been answered. Separately `limit` truncated silently: a caller
asking for 10 got 10 whether the log held 10 or 10,000.

The fix keeps **one** reader. `read_trace_events_counted` returns `(events, log_present,
unparsed_lines)` and `read_trace_events` — twelve call sites — becomes a thin adapter over it, so
the two cannot drift. `devmap_engine.read_run_history` returns a `RunHistory` carrying
`log_present`, `unparsed_lines`, `total` and `truncated`; `read_runs` stays a thin adapter for
`devmap_health` and `dev map runs`, which want the runs and nothing else. `trace_file_path` now
owns a path that was written out three times in `traces.py` — this pass removed two of those.

`tests/unit/test_graph_runs_provenance.py` (6 tests) is **red on 5 of 6** against the pre-fix
handler; the sixth pins `read_runs`' unchanged list contract and is correctly green either way.

One existing test had to be corrected rather than satisfied: `test_devmap_diagnostics.py`
asserted `runs == {"ok": True, "runs": []}`, which **encoded the conflation as a requirement**.
The empty list is still right for that fixture — it has run nothing — so the assertion now checks
that alongside `log_present is False`, and says why in a comment. It was not weakened to make the
change pass; it was wrong.

### `repo_map` told you less about a file the better it was mapped (2026-09-04)

`handle_repo_map` has four success-shaped returns, and the symbol listing was attached to exactly
one of them.

A `path` argument resolves an area, and if that area names a known subsystem the handler takes
the **subsystem** branch — which returned no `symbols` and no field saying a listing had not been
attempted. A path in an *unmapped* corner fell through to the summary branch and got the full
listing. So `devcouncil_repo_map --path src/payments/models.py` (mapped) told you less about the
file than `--path top_level.py` (unmapped). That is exactly backwards, and nothing in the payload
disclosed it.

The no-path branch had the other half of the problem: a bare `symbols: []` with none of the
discriminating fields, so `[]` read as "this has no symbols" rather than "you did not ask", and a
consumer indexing `symbols_available` — the field that exists to separate those — got a
`KeyError` on two of the three branches.

`_symbol_fields(root, path)` now builds the block once and every success return spreads it.
`symbols_available` is the discriminator: `True`/`False` is a determined answer, `None` means the
question was never put, and `symbols_reason` says why. `[]` never stands in for "not determined".

Not found by a failing test — found by reading every `return` in the handler and asking which of
them could be mistaken for a determined answer. `tests/unit/test_repo_map_symbol_shape.py`
(3 tests) is red against the pre-fix handler; the 53 existing tests in `test_mcp_map_tools.py`
still pass unchanged, including the one that blesses the subsystem branch — it asserted what that
branch *returns*, never that symbols were absent from it.

### The write gate authorized against the wrong repository inside a worktree (2026-09-04)

**Security-relevant, and changed only on an explicit decision from the repository owner.**

`dev hook pre-tool-use` is DevCouncil's write authorization gate: the project root it resolves
decides whose lease, whose allowlist and whose active task authorize a tool call. It used the
*baked* `--project-root` — the directory the session started in, which does not follow Claude
Code into a git worktree — while all thirteen other hooks were moved onto the payload's `cwd`.

So inside a worktree the gate evaluated a **different repository's** active task than the one
being worked on. That is not "fail closed", it is checking the wrong thing: able to block
legitimate worktree work, and able to allow work the worktree's own task scope forbids. A
worktree carries its own `.devcouncil/` with its own `state.sqlite`, so the task the gate read
was not the task in progress.

The subagent that found it correctly declined to change it and escalated instead, because
widening access is not a drive-by. The decision was put to the owner, who chose consistency with
the other hooks.

**What this widens, stated plainly:** a worktree holding its own initialized `.devcouncil/` now
governs its own writes, so a permissive policy there is no longer overridden by the parent's.
The containment is that `_effective_root` switches *only* when `cwd` resolves to a directory that
is itself an initialized DevCouncil project **and** differs from the baked root. A plain
subdirectory, a missing or blank `cwd`, a non-dict payload, and an uninitialized worktree all
keep the baked root — those negative cases are the thing standing between this and a general
escape from the gate, so they are asserted individually in
`tests/unit/test_hooks_write_gate_root.py` rather than left implied.

The end-to-end test drives `pre_tool_use` itself rather than the helper, and that distinction is
load-bearing: the five helper-level assertions passed *before* the change, because the helper was
always correct — the gate simply never called it. Only the end-to-end test was red.

Two other hook gaps from the same audit, both red-verified by the subagent before fixing: seven
further call sites still used the baked root (`pre_compact` wrote its snapshot to the parent while
`session_start` read it from the worktree, so the post-compaction briefing was empty every time
inside a worktree), and `_emit_stop_result` was the only Claude-facing emit path that bypassed the
10,000-character output cap — its `reason` embeds failing-claim output tails bounded by `_tail` to
50 *lines*, not characters, so over the cap the gate still blocked while the corrective
instructions became an unreadable file path.

### The adversarial sweep found four defects in the composition it was pointed at (2026-09-04)

A subagent was given `neighbors` and told to break it. It did, four times, each with a failing
test. Three were in code written hours earlier in this same pass; the fourth was pre-existing and
inherited. Recorded because the composition had already passed a byte-identical equivalence check
against the calls it replaced — equivalence to the old behaviour proves you did not *change* the
answer, not that the answer was right.

**1. A filter that could not be evaluated answered `Available` with an empty list.** The same
documented comparison had two implementations that disagreed completely on NaN: the Rust
`admits` in the edge cache saturates `(NaN * 1000.0).round() as i64` to `0` and admits
everything, while SQLite stores NaN as NULL and `>= NULL` admits nothing. The comment sitting
directly above `admits` claimed the two "cannot disagree at a boundary value" — that claim was
mine, and it was false. Reproduced end to end: the IPC layer rejects non-finite input but the CLI
does not, so `devmap neighbors --min-confidence nan core.py` returned a populated `callers` list
beside `"callees":{"resolution":"Available","total":0,"items":[]}` — a positive claim that a file
depends on nothing, from a comparison that never ran. `checked_min_confidence` now refuses NaN at
the store boundary. Infinities and out-of-range finite values are deliberately *not* refused:
both implementations agree on them, and an empty answer there is a filter that ran and matched
nothing, which is a real result.

**2. `min_confidence` reached only half the answer.** `neighbors` hardcoded `min_confidence: 0.0`
into its `impact` call, so one composed answer reported every caller while reporting only the
callees that cleared the threshold. Its doc claimed "the same budget and confidence". `traverse`
honours the field, so there was never a reason not to pass it.

**3. `callees` carried edges pointing *into* the target.** `latest_edges_for_file` matches
`sp.path = ?2 OR tp.path = ?2`, so a file's "outbound" edges included its inbound ones and a file
appeared in its own callee list — the same "symbols appeared to call themselves" shape the symbol
path had already been fixed for, surviving on the file path.

**4. Two resolvers made one answer incoherent.** `impact` matches a path by suffix
(`path_matches`: `ends_with("/{query}")`) while `dependencies` matches exactly. With `core.py`
and `pkg/core.py` both indexed, a single composed answer's callers described one file and its
callees another: "what calls core.py before I delete it" answered with a file that does not.

**3 and 4 have one fix.** Both directions now resolve through the forward traversal — one
resolver, directed by construction — so `callees` cannot contain inbound edges and the two halves
cannot name different files. `deps` the command is untouched; only this composition tightened.
This **changed the file-target answer** (`core.py`'s callees went 6 → 2 on the equivalence
fixture; the 4 dropped were inbound and structural edges that were never callees), so the
equivalence test was retargeted from `dependencies` to `trace` and says why.

It also closed a fifth gap for free: a **partially parsed** file used to answer `Available` with
an empty list and no incompleteness marker, because `dependencies` refused only
`ParseOutcome::Failed`. It now reports `Unavailable` naming the reason.

**5. A composed answer could straddle two generations, silently.** The fan-out is
`2 * targets.len()` sub-queries, each taking and releasing the store lock on its own. Under
contention, 25 of 62 composed answers carried symbols from two generations with nothing in the
response disclosing it. Not a regression — the separate `impact`/`deps` calls straddled the same
way — but those were visibly separate exchanges and this is sold as one. Holding the lock across
the whole fan-out would block the writer for the duration of a composed query, a worse trade, so
the composition **detects** instead: it compares the generation id before and after, retries once
(commits are rare, so this resolves nearly all of them), and if the index moves again marks every
direction's `walk_incomplete`. A straddle is tolerable; a caller unable to detect one is not.

Three of the subagent's tests had to be rewritten rather than satisfied, and each for a reason
worth stating. Its NaN test named "either a refusal, or `Unavailable`" as acceptable and then
`expect()`ed an `Ok`, so its code contradicted its own message. Its `min_confidence` test compared
thresholds 0.0 and 1.0 on a fixture where every edge has confidence >= 1.0 — it passed whether or
not the filter was applied, and only its callees half, which used 1.1, could tell. And two more
were written to fail *informatively when the behaviour improves*, which is how the partial-parse
gap surfaced at all.

### Index: the 2026-09-04 four-lane pass

Four subagents ran in parallel on non-overlapping file ownership, plus this session's own lane.
What each found, and where it is written up above.

| Lane | Owned | Outcome |
|---|---|---|
| Hooks | `clients/hooks.py`, `cli/commands/hook.py` | 13 events installed (14 with `--write-gate`) of 33 in the spec. 4 of 6 mechanical checks clean. Two gaps fixed: 7 hooks ignored the worktree; the stop-gate reason bypassed the 10,000-char cap. Escalated the write-gate root rather than changing it. |
| MCP 2.0 | `integrations/mcp/**` | Tool annotations 0/73 → 73/73. Four defects fixed, incl. a killed CLI call reported as a result, and 33 blocking `subprocess.run` calls on the event loop. Declined `outputSchema`, elicitation, resource links with reasons. |
| Plugins 1.0 | `claude_assets.py` | The "passes `--strict`" claim was half wrong and its test asserted a proxy, never running the validator. Added the real-validator test plus `homepage`/`repository`/`license`. |
| Adversarial | new test files only | Four defects in `neighbors`, three of them written hours earlier in this same pass. See the entry above. |
| This session | `map.py`, `graph_cmd.py`, kernel | `graph_runs` and `repo_map` Class A defects; the composed `neighbors` command; the write-gate decision once the owner made it. |

**What this pass is actually evidence for.** Three separate ledger claims and two of this
session's own claims were falsified by measurement or by an adversary. The pattern is consistent
enough to plan around: *a claim written next to the code it describes is the least reliable kind*
— `admits`' "cannot disagree at a boundary value", `neighbors`' "the same budget and confidence",
and the plugin test's docstring were all wrong, and all three sat directly above the thing they
described. The tests that caught them were written by someone who did not write the code.

**Process notes worth keeping.** Two of four agents reached for `git stash` / `git checkout --`
for a red-check in a worktree carrying four lanes of uncommitted work; both restored what they
destroyed, one only because it had copied the file seconds earlier. Brief agents to use
file-level backups and tell them the tree is dirty — the session's own `gitStatus` says "clean"
and is a snapshot taken at session start. Separately, the venv's editable install resolves
`devcouncil` to the **main checkout**: `pythonpath = ["src"]` is set only under
`[tool.pytest.ini_options]`, so pytest sees worktree code and a bare `python script.py` does not.
That silently validated main-checkout code once before it was caught.

**Known and unfixed**, carried forward deliberately: `impact`'s suffix path matching still answers
about every file sharing a basename (now at least coherently across both directions);
`handle_graph_impact` stays on Python because the MCP tool is path/diff-shaped and the kernel's
`impact` is symbol-rooted; `_devmap_query_payload` still lives in `cli.commands.graph_cmd`, so the
MCP layer imports the CLI layer; `mypy src` reports 98 pre-existing errors (57 in `tool_specs.py`)
which `ci.yml` gates on.


### The Python seam dropped the kernel's "I stopped looking" signal (2026-09-04)

Found by the adversarial lane, fixed here because `devmap_client.py` is this session's file.

The kernel distinguishes two ways an answer can be short: the token budgeter trimmed a complete
set (reported in `shown`/`hidden`/`total`/`truncated`) and the *walk itself* stopped at a depth or
node cap (reported in `walk_incomplete`, because a walk that withheld an unknown quantity cannot
be expressed in those counters without breaking `shown + hidden == total`, which the client
enforces). `BudgetedResponse` carried the first and silently dropped the second.

That is the common case, not an exotic one: at the **default** depth of 1 — the default in the
CLI, in the client, and in the IPC schema — the reverse walk routinely reports `walk_incomplete`
alongside `truncated: false` and `hidden: 0`. So a partial answer reached Python wearing the exact
shape of a complete one.

`BudgetedResponse.walk_incomplete` now carries it, type-checked on the way in. And the place where
it actually misleads is closed: an **empty** edge list from a capped walk is no longer rendered as
`[]` — which reads as "nothing calls this", the reading that gets a live function deleted — but as
`{method} walk incomplete: {reason}` with the list reported as unknown. A non-empty list is left
alone; the caller has real edges, and the signal is on the response for anyone who wants it.
`test_walk_incomplete_survives_the_seam.py` (4 tests) and one test in
`test_neighbors_batching.py` are red against the pre-fix client, including a case asserting the
new field cannot become an escape hatch from the budget invariants.
### The kernel got its own MCP server, and MCP 2.0 became reachable (2026-09-05)

**Baseline this session started from, measured not assumed:** `cargo test --workspace
--no-fail-fast` = **889 passed, 0 failed**, 66 suites, exit 0. The handoff's "740/0" is a
2026-09-02 figure and is stale; nothing was wrong with it, it simply predates 149 tests.

**What was built.** `devmap-serve` gained `mcp.rs` and `mcp_http.rs`, and `devmap` gained an
`mcp` subcommand. An agent can now speak MCP to the Rust kernel directly:

| Transport | Command | Protocol revisions |
|---|---|---|
| stdio | `devmap mcp` | `2024-11-05` … `2025-11-25` (handshake era) |
| HTTP, single-exchange | `devmap mcp --http <addr>` | `2026-07-28` (modern era) |

Nine tools — `status`, `search`, `dependencies`, `impact`, `trace`, `neighbors`,
`dead_symbols`, `clones`, `preview` — every one annotated `readOnlyHint: true`,
`destructiveHint: false`, and deliberately `idempotentHint: false` (the index moves as the
repository changes; claiming idempotence invites a client to cache past the commit that
invalidated the answer).

**There is exactly one dispatcher, and that is the whole design.** MCP tool calls are not a
second implementation of "what does `impact` mean". `IpcCommand` is `#[serde(tag = "cmd")]`, so
a tool's argument object with `"cmd"` inserted *is* its wire form; it deserializes through the
same serde defaults and goes to the same `validate_request` and `dispatch` the socket protocol
uses. A hand-written `match` over tool names would have restated `default_budget` and
`default_depth`, and a restated default is a default that can disagree.
`http_and_stdio_answer_the_same_question_identically` compares the two transports' `result`
payloads on three successes and one failure, because agreeing on success and diverging on
failure is the shape that would actually ship.

**MCP-2 is closed, and closing it required a transport, not a config change.** The register row
asks for `ttlMs`/`cacheScope` on `tools/list`. The Python server already sets them correctly and
they have never reached a client: `HANDSHAKE_PROTOCOL_VERSIONS` (`mcp_types.version`) stops at
`2025-11-25`, and `ServerRunner._serialize` sieves the result through the negotiated version's
surface, which drops both fields on every version stdio can negotiate. That analysis, written in
`server.py`, was **verified correct against the installed SDK** — one of the few claims this
repository has checked and found true. The consequence is that the fields are not a
configuration item at all: `2026-07-28` uses a stateless per-request envelope reached over the
modern HTTP transport, so serving that transport is the only way to make them live. They are now
live and asserted, on `server/discover` and `tools/list`, by
`discover_advertises_the_modern_revision_with_live_cache_hints`.

**Measured, on a store built and settled for the purpose** (4,214 nodes / 20,051 edges;
`node_count > 0` and `pending_count == 0` are *assertions* in the harness, not notes, because
this repository has produced three wrong numbers from unsettled stores):

| Path | p50 | p95 |
|---|---|---|
| in-process `tools/list` | 40.2 µs | 42.2 µs |
| in-process `status` | 360 µs | 561 µs |
| in-process `search` | 3.26 ms | 3.77 ms |
| in-process `impact` | 14.5 ms | 29.2 ms |
| **subprocess `status`** | **11.6 ms** | 15.4 ms |
| cold-open + `status` | 640 µs | 961 µs |

**32× on `status`, and the cost is attributed rather than guessed.** The `cold-open` row exists
precisely so the win is not credited to the wrong cause: opening the store is 640 µs of the
subprocess path's 11.6 ms, so **~11 ms is process startup**. The lesson from the `neighbors`
work — count the round trips before writing more Rust — generalises here to *identify what the
time is actually spent on before claiming a rewrite bought it*.

The harness is `crates/devmap-serve/examples/mcp_bench.rs`, an `examples/` target because no
new dependency was authorised for benchmarking; `criterion` was offered and declined. It uses
`std::time::Instant` and reports quantiles with N, and its own doc comment says plainly that
this is enough to answer "10× or 1.1×" and **not** enough to catch a 3% regression. It refuses
to benchmark a call that returns `isError`, because measuring an error path and reporting it as
a query has happened here before.

**Three defects found by running the code, not by reading it.** Recorded because all three were
invisible to review:

1. `Commands::Mcp` built a `tokio::runtime::Runtime` inside `#[tokio::main]` — "Cannot start a
   runtime from within a runtime", every invocation, exit 101. Found by the first smoke test.
2. hyper panics at connection setup when `header_read_timeout` is configured with no timer to
   drive it. Not a warning, not a degraded mode: **every accepted connection died**. An
   in-process test of the handler would have passed while the server was unusable, which is why
   `mcp_http_modern.rs` drives a real socket on port 0.
3. The tool schemas declared `additionalProperties: false` while serde silently ignored unknown
   fields. A client misspelling `budget` passed schema validation, had the typo dropped, and
   received the 2,000-token default while believing it had asked for more — a short,
   correct-looking answer that is not the one requested.

**Defect 3 is worth more than its size, because the first fix was the defect again.** The fix
added a hand-written list of accepted property names directly beneath a comment claiming
"checked against the schema rather than a second hand-written field list, so the promise and the
enforcement cannot disagree". That is the *exact* pattern this ledger already documents from the
`admits` NaN comment and `neighbors`' "the same budget and confidence": **a false claim sitting
directly above the code that contradicts it, written by the person who wrote the code.** It
survived one self-review and was caught only on re-reading. The list is now derived from
`describe()`, the same function `tool_specs` publishes.

**One preventive change, labelled as such.** `FmtSubscriber::builder()` writes to **stdout** by
default, and `main` installed it that way for every command. Every command that emits a payload
emits it on stdout — `emit_json` prints there, and `devmap mcp` speaks JSON-RPC there — so a log
line is not noise beside the answer, it is a line *inside* it: `devmap search --json | jq` fails
on it and an MCP client's next parse fails on it. It is now `.with_writer(std::io::stderr)`.
**No INFO-level log currently fires on either path** (`devmap-store` and `devmap-query` contain
none; the 28 in `daemon.rs`/`watcher.rs` are not on it), so this fixes no observed failure and
ships **without** a red test. Saying so rather than pairing it with a test that passes either
way, which would be a gate nobody has seen fail.

**Known and unfixed, carried forward deliberately:** `handle_method` accepts a JSON-RPC *batch*
array nowhere — a batch arrives as a parse-shaped failure rather than a named refusal. The
modern transport is one-request-per-POST so batches are out of scope there, but stdio clients
may send them.

### The adversary found eleven defects in the MCP server, and all eleven are closed (2026-09-05)

A subagent was handed `mcp.rs` and told to break it. **44 attacks, 11 landed.** Written up here
because the denominator matters as much as the hits, and because the shape of what it found —
and of what it *could not* find — says where the risk in this design actually was.

**The central claim survived.** The module is built on "there is exactly one dispatcher": MCP
tool calls become the same `IpcCommand` the socket protocol speaks and go to the same
`dispatch`. The adversary diffed all nine tools, socket `handle_stream` against MCP
`tools/call`, payload for payload — `status`, `search`, `deps`, `impact`, `trace`, `neighbors`,
`dead`, `clones`, `preview` — and found them byte-equal every time. **Every one of the eleven
defects was at an edge — framing or schema publication — and not one was in an answer.** That is
the outcome the single-dispatcher design was chosen for, and it is the first time in this
ledger's history that a new surface's core claim held up to a dedicated adversary.

Also attacked and unbroken: tool-name spoofing (`cmd`, `Cmd`, `__proto__`-nested `cmd`,
case-variant names, a zero-width joiner in a name, `arguments` as a bare string — all refused);
the schema/serde field-name audit across all nine tools (no declared property serde rejects, none
accepted that is undeclared); `tools/list` determinism; `StoreSlot`'s lazy open under a
mid-session build and under concurrent first-calls; 400-deep JSON nesting; and numeric casts —
`mcp.rs` contains no `as` casts at all, so the saturating-cast shape that produced the `admits`
NaN bug is structurally absent here.

**The eleven, and what each actually cost a caller.** Five were one root cause: `handle_line`
took a `&str` and decoded straight into a struct.

| # | Defect | The wrong answer |
|---|---|---|
| 1 | `id: null` folded into `None` by `Option<Value>` | JSON-RPC §4 says a notification is a request *without* an id member; `null` is a legal id. A legal request got **zero frames** and its client waited forever |
| 2 | Frame bound checked after `lines()` had grown the buffer | The refusal named 4 MiB against a 1 MiB limit — the check reported the size of an allocation it had failed to prevent |
| 3 | Non-UTF-8 returned `Err` from the loop | One stray `0xFF` killed the whole session: no frame for that request **and none for any request after it** |
| 4 | `neighbors` declared `depth.default = 1`, serde applied 3 | An agent omitting `depth` was told it asked for direct neighbours and got a three-hop closure — two of three reported "callers" of `helper` do not call it |
| 5 | `preview` declared `min_confidence.default = 0.0`, serde applied 0.5 | The "what would my edit break" tool, with the filter declared off and silently on: a shortened breakage list that reads as "breaks nothing" |
| 6 | `targets: []` answered instead of refused | `{"neighbors": []}`, `isError: false`, no marker — indistinguishable from "these symbols have no callers or callees" |
| 7 | Every non-file at the db path reported as "the index has not been built" | `Path::is_file` is false for a directory *and* for any swallowed IO error. A definite claim from a check that never ran, sending the caller to `devmap build`, which fails again for the unstated reason |
| 8 | Well-formed JSON with a bad `method` reported as `-32700` against a null id | `-32700` means "invalid JSON was received"; this JSON parsed. The id was visible and discarded, so a correlating client hung |
| 9 | JSON-RPC batch arrays unhandled | Two well-formed requests got one `-32700`; neither id was ever answered |
| 10 | Declared `minimum`s unenforced | `depth: 0`, `budget: 0`, `min_nodes: 0` all accepted against schemas that forbid them — a client-side validator refuses what this server accepts |
| 11 | `notifications/cancelled` accepted, wired to nothing | Indistinguishable from honouring it |

**Fixes, by root cause rather than by row.**

*Framing (1, 2, 3, 8, 9).* `handle_line` now parses in two stages. Stage one asks "is this JSON
at all" — the only place `-32700` and a forced null id are correct. Stage two asks whether it is a
well-formed *request*, by which point the id is recoverable, so a structural fault is `-32600`
against its own id. Id presence is read as `object.contains_key("id")`, which is what the
specification actually says. Batches are handled per §6, including the empty-array `-32600` and
the all-notifications case that correctly produces no response array. `read_frame` replaces
`lines()` and applies the bound **while** accumulating, draining an over-long frame to the next
newline without holding it — so one bad frame costs its own response and not the session; invalid
UTF-8 is likewise one refused frame.

*Schema truthfulness (4, 5, 6, 10).* The declared defaults now match the applied ones, and
`preview` reads its default from `PREVIEW_CALLER_MIN_CONFIDENCE` rather than restating it. Every
other constraint the schema states — `minimum`, `maximum`, `minItems`, `maxItems`, `maxLength`,
`enum` — is enforced generically from the published schema, so a bound can only be enforced if it
was advertised and can only be advertised if it is enforced.

*Honesty (7).* `StoreSlot::explain_absence` distinguishes directory, unresolved symlink,
unreadable, genuinely-absent, and *the check itself failed* — five different problems with five
different fixes, told apart instead of guessed at.

*Cancellation (11).* This one needed an architecture change, not a patch. The loop read and
awaited one line at a time, so a cancellation could not be *read* until the call it cancelled had
already been answered. Requests now run concurrently, one task each, writes serialized behind one
lock, with a registry of in-flight `Cancel` handles keyed by the id's JSON rendering (so `1` and
`"1"` stay distinct). A cancelled request gets no response, per the specification. This also
fixes a limitation nobody had filed: a single 30-second `impact` used to block every other tool
call on the connection.

**One of the adversary's tests was rewritten rather than satisfied, and the reason is the
finding.** Its preview test asserted, as a *fixture precondition*, that the schema declares 0.0 —
encoding the pre-fix state as a premise, so the fix broke the premise rather than the assertion.
It was also the wrong shape: the same defect existed twice (4 and 5), and a single-instance test
names one. It is now
`every_published_default_is_the_default_that_is_applied`, a behavioural class gate: for every
declared default on every tool, the answer with the field **omitted** must equal the answer with
it sent **explicitly at the declared value**. Comparing the two literals would only prove two
constants match; comparing the two answers proves the published contract is the one a client
gets. **Verified red** by re-introducing the `neighbors` defect alone and watching it fail.

The adversary's frame-bound test parses the first bare number out of the refusal and asserts it
is within the limit — a proxy for "this is what we buffered". The fix made that proxy stale
rather than wrong, so the *message* was rephrased to lead with the bound that was enforced
instead of the total observed, and the test passes unmodified. Rewording a message to satisfy a
test is only legitimate when the new wording is the more honest one; it is here, because the old
phrasing led with a number that was the size of an allocation the check had not prevented.

**State after the round:** `cargo test --workspace --no-fail-fast` = **930 passed, 0 failed**
(baseline 889, +41). `devmap-serve` alone is 124 across five suites. `cargo clippy -p
devmap-serve --all-targets` clean — which required deleting `RpcRequest`, dead once the two-stage
parse replaced it. Re-benchmarked after the concurrency change: in-process `status` p50 277 µs
against subprocess 7.37 ms, ~27×, no regression.

### Two critical MCP containment defects, and a third the regression tests found (2026-09-05)

An audit lane proved two bypasses by execution against the Python MCP server. A fix lane closed
both and was **interrupted by a rate limit before writing any test**, so the fixes were verified
here independently and the regressions written afterwards
(`tests/unit/test_mcp_secret_and_debug_containment.py`, 15 tests).

**The guard and the read were looking at different files.** `handle_read_file` ran
`is_secret_path` on the caller's raw string, then resolved the path and opened the *resolved*
one. Anything that changed between those two steps was a bypass, and the failure is silent — the
tool returns the bytes with `ok: true`:

| Spelling | Before | Now |
|---|---|---|
| `.env` | refused | refused |
| `.ENV`, `.Env` | **returned the secret** (`fnmatch` is case-sensitive; APFS/NTFS are not) | refused |
| `notes.txt` → `.env` (symlink) | **returned the secret** — nothing in the name suggests one | refused |

**A capability gate the caller could open.** `devcouncil_debug_discover {"consent": true}` called
`set_debug_consent`, which *wrote* `auto_discover: true` into `.devcouncil/config.yaml` and
unlocked the other seven debug tools — one of which passed a caller-supplied `script` straight to
`subprocess.run` with no containment (`resolve_root` only ever constrained `projectPath`). Both
halves are closed: consent can no longer be granted from a tool argument, and `script`/`path`/
`source` route through the same `within_root` owner `projectPath` uses. Verified by probe:
absolute-outside, `../` traversal and a symlink escaping the root are all refused, while a
legitimate in-root path is still accepted — a containment check that refuses everything is a
different defect, so that case is asserted too.

**The third defect was found by writing the regressions, not by the audit.**
`SECRET_PATH_PATTERNS` carried `**/*.pem` and `**/*.key` with **no root-level twin**. `fnmatch`
gives `**` no special meaning, and `**/*.pem` still requires the `/`, so `certs/key.pem` was
protected and `key.pem` at the top of the repository was **not**. The list pairs both forms
everywhere else — `.env` beside `**/.env`, `*.pfx` beside `**/*.pfx`, `id_rsa` beside
`**/id_rsa` — which is what makes these omissions rather than a policy. `*.pem`, `*.key`,
`id_dsa` and `id_ecdsa` gained their root-level forms.

**Security impact, stated plainly: every change here narrows.** More paths are refused; nothing
became reachable that was not reachable before. The one thing to watch is over-refusal, which is
why `test_a_non_secret_file_is_still_readable` and the in-root debug-path case exist.

**Two notes on method, both of which changed a conclusion.**

*The red-test check reassigned credit.* Reverting the re-check added to `read.py` left all 15
tests **green** — it is redundant with the hardened `is_secret_path`, which resolves symlinks
itself. Reverting `util.py` to HEAD is what turns 4 red, and reverting the pattern list turns 2
red. Two independent guards over the same bypasses is a fine outcome, but without running the
revert the wrong change would have been recorded as the fix.

*One test asserted the wrong thing, and the code was right.* The first version of the case test
asserted plain case-insensitivity with no files on disk, and failed. The implementation folds
case only once `os.stat` confirms both spellings reach the *same file* — deliberate, because an
unconditional fold would refuse `.ENV` on a case-sensitive filesystem where it is a different,
ordinary file the patterns were never meant to protect. The test now asserts that condition, and
asserts the **non**-refusal on case-sensitive filesystems too, so the narrowing is a tested
property on Linux rather than an untested claim in a docstring.

### Stress: the concurrent transport, asserted rather than assumed (2026-09-05)

Making requests concurrent to support cancellation introduced two failure modes serial
processing could not have, and neither shows up in low-volume manual testing:

* **Interleaved writes.** Two responses whose bytes cross produce one unparseable line and the
  client loses both. `mcp_concurrency.rs` fires **200 requests down one connection before reading
  any response**, deliberately mixing methods that return immediately (`ping`, `tools/list`) with
  ones that take the store lock (`impact`, `search`) so fast responses race slow ones. The
  `serde_json::from_str` on every line *is* the interleaving assertion — crossed bytes are not
  valid JSON.
* **Lost or duplicated ids.** A client resolves pending calls by id: one missing id hangs that
  call forever, one duplicate resolves the wrong future. Every id is checked into a set (so a
  duplicate is caught) and the full range is asserted present at the end (so a loss is caught).
  Interleaved notifications are included, because one that ever produced a frame would show up as
  an extra.

A second test drives invalid UTF-8, non-JSON, and a valid-JSON-invalid-request **between** good
requests and asserts the good ones on both sides still answer — the session used to die on the
first `0xFF`, taking every later request with it.

**One of these tests was wrong on its first run, and the code was right.** It asserted that the
request following the bad frames was the *last* frame written. Concurrent responses have no
ordering guarantee — JSON-RPC does not promise one, and that is precisely the property being
bought here — so the assertion was rewritten to look the id up rather than take the last frame.
Worth recording because the tempting reading was "responses are coming back out of order, that is
the bug".

Workspace after: **932 passed, 0 failed**; `devmap-serve` alone 126 across six suites; `cargo
clippy -p devmap-serve -p devmap-cli --all-targets` clean.

### Repairing an interrupted fan-out, and the regression it was about to ship (2026-09-05)

A rate limit killed five agents mid-task. Four had landed code; none had finished. Recovering
that state produced two findings worth more than the recovery.

**The Python suite's real number, and why the first two readings were wrong.** `pytest tests/unit`
using the venv's `pytest` script gives **17 collection errors** — `tests/__init__.py` does not
exist, so `from tests.unit.support_maps import …` needs the repository root on `sys.path`, which
only `python -m pytest` supplies. That is pre-existing and has nothing to do with the fan-out, but
an agent running the documented command would read 17 errors as damage it had caused. A second
reading reported `4073 passed` where `--collect-only` said `4123` — a truncated capture, not a
result. The authoritative run is **4112 passed, 2 failed, 9 xfailed = 4123**, matching collection
exactly. Two readings disagreed with the collector before one agreed with it; the arithmetic is
what settled it, and it is worth doing every time.

**Both failures were the interrupted agents' work, and one was a regression about to ship.**

*The stale one.* `test_hook_map_refresh_defers_loudly_when_the_kernel_cannot_build` asserted
`paths == ["src/app.py"]` was handed to `refresh_map_artifacts` — a function that did
`del … paths …` on arrival. The test asserted plumbing that had no effect, and passed for as long
as the feature did nothing. Deleting the parameter (the audit's option (b)) was right; the
assertion is now a *guard against the plumbing being reintroduced*, and the queue assertion two
lines below is left alone, because knowing what changed still earns its keep there — it decides
whether to build at all, not what to build.

*The regression.* The H4 fix made an un-evaluable stop gate say so, which is correct and is the
Class A rule applied to a gate. But `fail_open` was already set by the blanket
`except Exception` in `evaluate_stop`, and `load_config` raises `FileNotFoundError` in **any
directory that has never been `dev init`-ed** — which is most directories a hook ever runs in.
So the new notice, "DevCouncil stop gate did not evaluate this stop: nothing was checked. This is
not a pass.", would have fired **on every stop outside an initialized repository**.

The flag was harmless while nothing rendered it; making it visible is what turned a latent
mislabelling into a user-facing defect. **"There is no DevCouncil project here" is a complete
answer, not a failure to answer** — and a warning that is always on is a warning nobody reads,
which would have drowned the real one the fix exists to deliver. `FileNotFoundError` now returns
a clean pass; every other exception still sets `fail_open`. Both directions are pinned
(`TestNotAProjectIsNotAFailureToEvaluate`), because a test of only the quiet case would pass
against a gate that had simply stopped reporting. **Verified red** against the pre-fix code.

**Two critical security fixes were verified independently rather than trusted**, because the
agent that made them was killed before writing a single test — see the entry above, including the
third defect the regressions themselves uncovered.

**The general lesson.** An interrupted agent leaves code with no test and no report, and its
last transcript line ("Now the H3 red tests", "Now the required red-test verification") is
evidence of intent, not of completion. Every fix recovered here was re-verified by running it,
and the two that mattered most — a security bypass and a notice that would have fired on every
stop — were both cases where the landed code looked finished and was not.

### The kernel audit: 53 defects from ~486 candidates, and the first three closed (2026-09-05)

A dedicated read-only audit of `devmap-extract/-resolve/-analyze/-store/-query` and the daemon
found **53 real defects from ~486 candidates examined**, 6 critical, **7 reproduced end-to-end
against the shipped binary** rather than through a unit harness. The full report is
`audit_kernel.md` (785 lines). What follows is this session's own share of the fixes;
`devmap-extract`, `devmap-analyze`/`code_graph` and `devmap-store` were taken by parallel lanes.

**What the audit found that matters most, in one sentence: `devmap dead` proposes deleting live
code at the top confidence tier because coverage loss reaches no output at all.** Two independent
paths converge there — discovery refusals (oversized/unreadable/non-UTF-8) and
`ParseOutcome::Failed` — and neither is recorded anywhere durable. `graph_degraded` is derived
*only* from `AnalysisStatus`, so the earlier fix that removed its hardcoded `false` covered the
analysis half and left the coverage half. Proven with the real CLI: a file whose only caller is
over the size ceiling is reported `{"confidence":0.9,"resolution":"Available","truncated":false}`
while `status` says `{"degraded_reason":null,"is_fresh":true}`. The Python fail-closed branch
`if bool(repo_map.get("graph_degraded")): return True` therefore can never fire.

**Also worth knowing, because it is the reverse of the usual finding:** R4 determinism is
*genuinely clean* — 0 live violations across ~70 sites, and `devmap-serve` and `devmap-resolve`
contain no `HashMap`/`HashSet` at all. FTS5 escaping is correct at all three `MATCH` sites against
17 hostile inputs. Migration atomicity holds, `user_version` was proven transactional, no
`SQLITE_BUSY` is mapped to an empty result, no SQL injection, no regex built from user input. The
`(NaN * 1000.0).round() as i64` shape is fixed at its canonical owner. Recording the clean
results so nobody spends a session re-deriving them.

**Closed here (`devmap-query`), each with a test verified red against the pre-fix code:**

*Q-3 — one emoji aborted `dev map manifest`.* `byte_span_to_line_range` counted newlines with
`source[..start]`, slicing a `&str` at an index that need not be a character boundary. Spans are
byte offsets recorded at extraction time while the source is re-read from disk at export, so any
multi-byte character inserted before an indexed offset put that offset mid-character — and the
release profile is `panic = "abort"`, so nothing recovered. **Two copies of one computation
existed and only one was safe**: `Span::line_range` has used `as_bytes()` all along. The wrapper
now delegates to it and keeps only the one thing it adds (clamping `end` to at least `start`).
Red-verified with the exact predicted panic: *end byte index 14 is not a char boundary; it is
inside '🦀'*. The test is exhaustive over every offset pair rather than sampled, because the
boundaries are precisely the offsets that never failed.

*Q-4 — `preview` returned a clean bill of health for a file it could not read.*
`read_to_string(..).ok()` collapsed *no such file* and *file present, unreadable* into one `None`,
and `None` meant `compared_against: "nothing"` — documented as "no such file, so every symbol is
an addition". So on a non-UTF-8 or unreadable file a genuine **removal disappeared**:
`symbols: ["mod.py:Added","alpha:Added"]`, `degraded_reason: null`, `delta_available: true`. This
is the tool whose entire purpose is "what would my edit break", answering "nothing" from a
comparison that never ran. There is now a third `compared_against` value, `unreadable`, and
`delta_available: false` with the errno in `degraded_reason`. **All three states are asserted** —
readable reports the removal, absent still reports `nothing` with `delta_available: true` (an
absent file is a *complete* comparison against nothing), unreadable reports neither.

*Q-7 — a depth-capped walk published as a complete blast radius.* `QueryEngine::impact`/`trace`
computed `walk.stop` — *"stopped at depth 2; the result is a lower bound, not the full blast
radius"* — and discarded it, returning `walk_incomplete: None, truncated: false`.
`StoreQueryEngine::traverse` has always carried it. For `impact` this is the reading that gets a
live symbol deleted, because an incomplete blast radius and a small one are indistinguishable.
The red check was made **discriminating on purpose**: reverting only `impact`'s line fails only
`impact`'s test while `trace` and the complete-walk control stay green, so the test is pinned to
the behaviour rather than to compilation. The complete-walk control exists because a fix that
always set the marker would pass the capped test and make the marker meaningless.

## Parse budget made a real bound (2026-09-05)

`DEFAULT_PARSE_BUDGET` is 5 s and every caller — `dev map build`, the daemon,
the MCP server — treats it as the guarantee that one hostile file cannot stall
the index. **It was not a bound.** Measured against the unmodified extractor:

| input (200 ms budget) | before | after |
| --- | --- | --- |
| `"fn (((("` x20,000 (140 KB Rust) | **174.69 s — 873x over** | 333 ms — 1.7x |
| `{` x2,000 / `}` x2,000 (C++) | 226 ms | 243 ms |
| `{` x10,000 (C++) | 229 ms | 221 ms |
| 4,000 Go interfaces + impls | 228 ms | 218 ms |

At the shipped 5 s budget the first row extrapolates to over an hour on a single
file, holding the daemon's writer lock for all of it.

**The cause was not tree-sitter.** Timed directly, that parse finishes in 11 ms
and polls its cancellation callback 1,400 times without ever needing to cancel
(`examples/budget_locate.rs`, since removed). Two defects in this crate's own
code, both of which the existing `stress_hardening.rs` bound test missed because
it allowed a 200 ms budget to take 5 s — 25x slack:

1. **Child iteration was quadratic.** `for i in 0..node.child_count()` with
   `node.child(i)` rescans the sibling chain from the first child on every call.
   A degenerate parse gives the root one child per token — 100,000 children for
   the file above — so the loop cost 5x10^9 sibling steps. Walking the *same*
   tree with a `TreeCursor` visited the same 2 nodes in **2.5 ms vs 37.1 s**, a
   14,800x difference for byte-identical output. 28 sites converted; three
   helpers (`push_children`, `push_children_reversed`, `push_named_children`)
   now own the fast form so the slow form has one place to be warned about.
2. **A stride is not a bound if one step is unbounded.** `walk_tree` reads the
   clock every `DEADLINE_CHECK_STRIDE` (256) nodes, which bounds the *number* of
   nodes between checks but not the *work* inside any one of them.
   `collect_non_symbol_locals` walks an entire scope subtree per call, so a
   single `extract_node` ran for a minute with the stride counter sitting at 1.
   It is now bounded by the same deadline and latches a `WALK_OVERRAN` flag that
   `walk_tree` consults on every node (a `Cell` read, not a clock read).

A set the deadline cut short is never cached in `SCOPE_LOCALS`: it is not that
scope's local-name set, and the file is refused either way.

**Refusal, not truncation.** A file that exhausts the budget yields only its
`File` node with `ParseOutcome::Failed`, and the reason names the cause —
verified end-to-end through the release binary on a repo holding one hostile
file beside real code:

```
src/hostile.rs :: {"Failed":{"reason":"extraction of 140000 bytes exceeded the 5s
budget for grammar rust while walking the syntax tree; no symbols are claimed for
this file"}}
```

The whole build took 6.04 s, exit 0; `helper`, `run` and `main` from the
neighbouring real files were all extracted. Downstream, the map admits the loss
rather than reporting a clean index: `graph_degraded: true`,
`parse_failed_files: 1`, and `src/app.py::main` is demoted from `extracted` to
**`ambiguous`** — "no inbound call edges, but call extraction did not cover every
file — not evidence of death". That demotion is the Class A rule holding at the
surface where it decides whether a live symbol gets deleted.

Tests: `tests/budget_is_a_real_bound.rs` (3). The hostile case asserts against
the budget directly and is build-profile independent, because its elapsed time is
set by the deadline the code enforces rather than by how fast the code is; the two
completion cases get their own generous budget, since their wall time is the cost
of real work and a debug build is an order of magnitude slower. Benchmark harness:
`examples/budget_probe.rs`, dependency-free.

**Not fixed:** four `(0..node.child_count()).rev()` index walks remain in the
`#[cfg(test)]` modules of `langcalls/lua.rs` and `langcalls/r.rs`. They walk
three-line fixtures and ship in no binary.

## Capped reads now say so (E-3 … E-7, 2026-09-05)

Five audit findings in `devmap-extract`, all one rule: **a capped or failed read
must not report what a complete one reports.** `cache.rs`'s own v29 note already
records this class being fixed once for the pattern scanner — the case was fixed,
the class was not. Each fix below was watched failing first.

- **E-5 (`treesitter.rs` `c_declaration_head`)** — a 256-byte head window sliced
  with `source.get(start..end).unwrap_or_default()`. When the window edge split a
  multi-byte character `.get()` returned `None`, the head became `""`, and every
  question asked of it answered "no". Red: `__global__ /* <100 em-dashes> */ void
  kern(int* p) {}` in a `.cu` file → `wiring=[]`, so the CUDA kernel lost its
  entry-point exemption and became a dead-code candidate; the same file with ASCII
  padding kept it. The boundary walk existed already in `clamp_receiver`
  (`langcalls/scope.rs`) and was not reachable from here, so it is now one owner,
  `floor_char_boundary`, called by both — a duplicated loop removed, not added.
- **E-3 (`notebook.rs`)** — the 5,000-cell cap was recorded only in
  `diagnostics`, which `for_durable_store()` clears before the payload reaches
  `generation_files.extraction_json` and the extract cache. Red: a 5,100-cell
  notebook → `durable.parse_outcome == Clean`, `durable.diagnostics == []`, and
  `cache_admits(Clean)` is true, so a prefix was pinned as complete coverage under
  a real content hash. The cap and the unlocatable-symbol count now ride on the
  outcome. A notebook under the cap stays `Clean` (asserted, both directions).
- **E-4 (`fallback.rs`)** — a line over `MAX_LINE_BYTES` was skipped and counted
  nowhere, so "N declaration(s) recovered" counted what the scanner kept and read
  as what the file has. Red: a `.proto` declaring three messages, one on a
  2,500-byte line → `"2 declaration(s) recovered by pattern"`. Now
  `FallbackScan::skipped_long_lines` is counted and folded into the reason.
  **The "X of Y" form is emitted only when Y is genuinely known** — a line that
  was never pattern-matched cannot contribute a known total, and printing
  "2 of 2" there would be the same overclaim in a smaller font.
- **E-6 (`treesitter.rs`)** — `ParseAttempt::NoTree`, the variant that exists to
  name "the parser returned no tree and did not say why", was the one not routed
  to `refused_extraction`. `Budget` and `GrammarLoadFailed` were routed correctly.
  A tree-sitter ABI break would therefore have published every file of a language
  as `RegexFallback` with the false reason "no linked tree-sitter grammar", been
  cache-admitted, exempted the whole language from dead-code analysis, and left
  the build green — the exact scenario `refused_extraction`'s own doc warns about.
  Fixed structurally; **reachability remains unverified** (a `NoTree` cannot be
  constructed without editing the crate), so this ships without a red test and is
  labelled as such.
- **E-7 (`notebook.rs`)** — `cells.find(|c| c.code.contains(declaration))` with an
  empty `declaration` matched cell 0 unconditionally, so a symbol took a guessed
  span and was reported located, contradicting the module doc. The parallel call
  path already guarded this. Symmetry fix; reachability inferred, not proven.

`EXTRACTION_SCHEMA_VERSION` is bumped **29 → 30**: v29's own doc says reusing
rows across a change of this kind keeps serving a prefix under a reason that
reads as a set, and that applies to both new fields.

`ParseOutcome::Fallback`'s doc is widened — it now has two producers (pattern
recovery, and a parse of only a prefix of the file) and one meaning that
consumers act on: absence of a symbol here is not evidence the file does not
declare it.

Tests: `tests/capped_reads_say_so.rs` (6 — three red-then-green, three controls).
`devmap-extract` 314 passed / 0 failed; clippy `-D warnings` and fmt clean.

## MCP fuzz and resource bounds (2026-09-05)

The four existing MCP test files are 43 hand-picked cases, each naming a defect
and pinning it. That leaves the shape a hand-picked suite cannot reach: *any*
byte string a client can send. `tests/mcp_fuzz_and_bounds.rs` asserts four
invariants over generated input rather than chosen input — no panic; every
response serializes to one line of valid JSON carrying `jsonrpc: "2.0"` and
exactly one of `result`/`error`; a request with an id is answered against that
id and a notification never is; and the work is bounded by the input rather than
by what the input asks for. The generator is a seeded xorshift64* over six valid
seed frames with 1–4 byte mutations each (4,000 rounds), so a failure replays.

**It found one: a batch could contain a batch.** `dispatch_value` treated any
`Value::Array` as a batch, including one nested inside a batch, and recursed
through `Box::pin`. JSON-RPC 2.0 §6 defines a batch as "an Array of Request
objects" — it does not nest. Red, at depth 64:

```
out="[[[[…64 deep…[{"error":{"code":-32600,"message":"a batch must contain at
least one request"},"id":null,"jsonrpc":"2.0"}]…]]]]"
```

That is not a response any client can use: it is an array of arrays, the `id` is
buried 64 levels down, and nothing correlates it to a pending call — the caller
hangs. It was also the only unbounded recursion on the request path; the sole
thing standing between `[[[[…` and the stack was the JSON parser's own default
nesting limit, which is a property of a dependency rather than a decision this
server made.

Fixed by splitting `dispatch_single` out: only the top-level value may be a
batch, a member is a request object, and a nested array is answered `-32600`
naming the rule. The recursion is gone with it — members are dispatched in a
loop, so `Box::pin` is no longer needed.

Also asserted and **already sound**, recorded so nobody re-derives them: a
5,000-member batch answers every member exactly once with no duplicate ids;
control characters, lone-surrogate pairs, RTL overrides and NUL escapes in a
string argument round-trip without corrupting the response frame; and every
malformed-but-identified request (`{"id":77}` with no method, wrong `jsonrpc`,
unknown method, unknown tool, missing/empty/unknown arguments) comes back under
id 77 unaltered.

`devmap-serve` MCP + IPC suites: 62 passed / 0 failed (`mcp_protocol` 17,
`mcp_adversarial` 15, `ipc_fuzz` 14, `mcp_http_modern` 9, `mcp_fuzz_and_bounds` 5,
`mcp_concurrency` 2).

## Audit report preserved

`AUDIT_KERNEL_2026-09-05.md` is the full kernel robustness audit (53 defects
from ~486 candidates, 6 critical, 7 proven against the shipped binary). It lived
only in a session scratchpad under `/private/tmp` and is now checked in, because
roughly a third of its findings are still open. Its per-section **"checked and
CLEAN — do not redo"** paragraphs and **denominators tables** are what stop the
next pass re-deriving work that was already done; read those first. The header
records which findings were closed, and that **S-7 was falsified** rather than
fixed.

## Q-9: the parse-off build now actually builds (2026-09-05)

`devmap-extract`'s `parse` feature exists so an embedder can read a persisted map
without linking tree-sitter and its 32 C-compiled grammars. That is not a
hypothetical configuration — `devmap-extract/Cargo.toml` records GitPulse linking
`devmap-query` to answer impact queries in-process, never indexing anything, and
inheriting 49 crates and 32 grammars to do it. `devmap-query`'s own feature doc
promises "Off, this crate builds without tree-sitter and answers questions about
a persisted map rather than building one."

**It did not build at all.** Red, executed:

```
error[E0433]: cannot find `cache` in `devmap_extract`
  --> crates/devmap-store/src/db.rs
note: found an item that was configured out
  --> crates/devmap-extract/src/lib.rs:6  (#[cfg(feature = "parse")] pub mod cache;)
```

Two sites in `devmap-store` reached straight into the parse-gated
`devmap_extract::cache::current_payload_identity`. That function cannot move out
of the gate — it calls `grammar_version_for`, which reads the compiled grammars —
so the fix belongs at the call sites, and it is a Class A question rather than a
plumbing one: **without the grammars there is no current identity, so the answer
is neither "current" nor "stale" but *unknown*.**

One owner, `db.rs::current_payload_identity -> Option<(String, String)>`, returns
`None` when this build cannot know, and neither caller may turn `None` into a
match:

- The carry-forward in `save_generation_with_metadata` treats `None` as **not a
  match**. Carrying a row on an identity this build could not compute would claim
  a currency nothing checked; not carrying is merely conservative.
- `latest_generation_payload_is_current` **fails loudly** on `None`, naming the
  language and the missing frontend. `false` would mean "rebuild", which a build
  with no parsing frontend cannot do — the caller would loop. `true` would be
  worse: a currency claim from a check that did not run.

Making the configuration compile then revealed dead-code warnings that had never
been visible because it had never compiled: five items in `clonesig.rs` used only
by its own `#[cfg(feature = "parse")] mod parse_impl`, and five more across
`devmap-store`/`devmap-query` whose single call site is parse-gated. Each now
carries the gate its only caller has. The compiler reporting them unused with
`parse` off is itself the proof that those callers are parse-gated.

`cargo check -p devmap-query --no-default-features` passes. **Not closed:**
`--no-default-features --all-targets` still fails on integration-test targets in
both crates that need `extract_file`/`preview`; closing that needs
`[[test]] required-features = ["parse"]` entries, which is a separate decision.

## `impact` costs the whole edge set per call (diagnosed, not fixed — 2026-09-05)

Benchmarked through the MCP surface on a settled 14,989-node / 77,904-edge store
(`devmap-serve/examples/mcp_bench.rs`, in-process, p50 over 50 calls after 5
warmups):

| tool | p50 |
| --- | --- |
| `tools/list` | 34 µs |
| `status` | 807 µs |
| `search` | 2.1 ms |
| **`impact`** | **35.6 ms** |

`impact` is ~40x the next slowest tool, and the cost is independent of the
answer: a leaf symbol with two callers pays the same as a hub.

**Where it goes** (`devmap-query/examples/impact_breakdown.rs`):

```
latest_edges (SQL read only)        p50 =  5.28 ms
impact (read + convert + walk)      p50 = 33.58 ms
=> the read is 16% of the whole call
```

So it is **not I/O**. `traverse` calls `resolved_edges`, which pulls every edge
in the generation and runs `stored_edge_to_resolved` over all 77,904 rows —
allocating owned `String`s per edge — *before* it has looked at the target at
all. The traversal that follows is bounded by `max_depth`/`max_nodes` and
typically visits a few dozen nodes. The work scales with repository size; the
answer does not.

**Not fixed, deliberately.** The two real options — memoising the converted edge
set per generation, or making the walk borrow from the stored rows instead of
materialising owned ones — are both engine-wide changes, and the first alters
what "fresh" means for a long-lived `devmap mcp` process. That is a scope call
for the owner rather than something to slip into a hardening pass. The
measurement and the harness are checked in so the decision can be made against
numbers.

## K-A2 closed: discovery refusals are coverage loss (2026-09-05)

The audit's highest-severity finding, and the half that survived the first fix.
`graph_degraded` was made honest about files that **failed to parse**; it stayed
blind to files discovery **refused to read at all** — oversized, unreadable, or
a non-UTF-8 path.

The reason the second half outlived the first is worth stating, because it is
the shape of the bug rather than an accident: every coverage check is computed
from the `&[Extraction]` slice, and a file discovery turned away has no
`Extraction` in that slice. So the check ran, found every file it could see
intact, and reported a complete corpus. **The count cannot be derived; it has to
be carried in.**

Audit proof C, reproduced and then closed against the release binary. `lib.py`
defines `helper()`; its only caller `app.py` is 1,134,064 bytes, over
`MAX_SOURCE_BYTES`:

| | before | after |
| --- | --- | --- |
| `repo_map.graph_degraded` | `false` | **`true`** |
| `graph_degraded_reason` | `''` | `…1 refused by discovery and never read at all` |
| `code_graph.dead_code` confidence | `extracted` | **`ambiguous`** |
| `devmap dead` | **0.9** — the confident tier | **0.35** |

The map no longer proposes deleting a live function because the file that calls
it was never read.

**Shape of the fix.** `ExtractionCoverage` gains `discovery_refused_files`, and
a `DiscoveryCoverage` input type carries it in — a type rather than a bare
`usize` so that "no discovery step ran" is spelled `DiscoveryCoverage::none()`
and cannot be confused with a count that happens to be zero. That distinction is
load-bearing: `analyze` (a caller supplying its own corpus — the single-file
preview path, and tests) is correct to report full coverage, and
`analyze_with_discovery` (a caller that walked a tree) is not.

The count is folded into coverage **before** the confidence cap, not after the
reports are built. A file never read may hold the only call to a symbol, so a
refusal has to reach `coverage.cap()` by the same route a parse failure does —
setting a flag on the way out would satisfy `graph_degraded` and still offer
`helper` at 0.9.

**No schema change was needed.** `Commands::Manifest` reads the *persisted*
`AnalysisSummary`, so once the count reaches `analyze()` at build time the
degraded status propagates to `repo_map.json`, `code_graph.json` and a later
standalone `devmap manifest` for free. A new table was considered and rejected
on that basis.

Tests: `devmap-analyze/tests/discovery_refusals_are_coverage_loss.rs` (4). Red
proof, with the single line carrying the count reverted:

```
a_refused_file_makes_the_analysis_partial
  … reporting Ok is a check that could not run answering like one that ran and passed
a_refused_file_caps_dead_symbol_confidence
  a refused file may hold the only call to `helper`, so the finding must be
  downgraded: 0.9 is not below 0.9
```

Both controls — a fully-discovered corpus is **not** marked partial, and the
plain `analyze` entry point still reports a complete corpus — stayed green under
that revert, so the tests are not trivially red.

### K-A2, daemon half — the resync that erased the refusal

The note that stood here said the daemon's drain still called plain `analyze`
and left this "not in scope at that call". That was wrong about the
consequence, and the consequence is what matters: **the drain overwrites
`analysis_json`.** So the `Partial` that `devmap build` correctly recorded had a
lifetime of one watcher event. Someone saves an unrelated file, the daemon
resyncs, and a corpus with unread files is relabelled `Ok` — with dead-code
confidence back at 0.9. Fixing only the build path bought nothing in the state a
repository actually sits in, because the daemon is the long-running path.

Two branches, two different defects:

* **Full rebuild** (stale payload or moved HEAD) walked the tree and discarded
  the answer — literally `let (whole_tree, _report) = …`. The one branch that
  *could* measure refusals was the one that threw the measurement away.
* **Incremental resync** — the common case — carries the previous generation's
  extractions forward and never re-walks discovery, so it cannot measure. It
  called `analyze()`, which *asserts* nothing was refused.

The full-rebuild branch now measures via `report.refused_count()`. The
incremental branch carries the previous generation's count forward, which
required persisting it: `AnalysisSummary.discovery_refused_files:
Option<usize>`. It cannot be recomputed from the stored graph, because a file
discovery turned away has no rows to count.

`Option`, not `usize`. `None` means *not recorded*; `Some(0)` means *measured,
nothing refused*. Rounding the first to zero is the same class of lie this whole
item is about, so `DiscoveryCoverage` gained the same split —
`DiscoveryCoverage::none()` now records `None` rather than `0`, and `charged()`
is what counts against coverage. An unmeasured discovery charges nothing on
purpose: every test and the single-file preview path build their own corpora,
and degrading all of them would make the marker useless.

**One canonical owner for "is this skip coverage loss".** The rule was an inline
closure in the CLI. The daemon reads the same `DiscoveryReport` and needed the
same verdict, and a *third* copy already existed in the connect-time sweep. It
now lives once, as `DiscoverySkipReason::is_refusal`, written as an exhaustive
`match` so a skip reason added later fails to compile until someone decides
which side it falls on — the default a wildcard picks ("not a refusal") is the
one that loses coverage silently.

Four tests, `crates/devmap-serve/tests/daemon_discovery_refusals.rs`. Both
defects were watched failing against the unmodified daemon:

```
a_full_rebuild_that_refused_a_file_does_not_persist_a_clean_status  FAILED
an_incremental_resync_does_not_erase_a_recorded_refusal             FAILED
a_full_rebuild_with_nothing_refused_stays_clean                     ok
an_incremental_resync_of_a_clean_corpus_stays_clean                 ok
```

The two controls passed while the defects were still live, so they constrain the
fix rather than following it: a change that marked every rebuild degraded would
satisfy the first two and break these. That matters more than usual here —
`NonSource` skips are constant and everywhere, so "always degraded" is one
careless predicate away.

End-to-end through the real binary, on a corpus whose only caller of `helper` is
past the 1 MiB ceiling:

```
degraded:  devmap dead -> 0.35   status Partial "… 1 refused"   discovery_refused_files: 1
clean:     devmap dead -> 0.90   status Ok                      discovery_refused_files: 0
```

**Known residual, stated rather than hidden.** Once a refused file is *fixed* —
shrunk below the ceiling, made readable — the incremental branch keeps carrying
the old count until a full re-extraction or a `devmap build` re-measures. That
under-claims coverage instead of over-claiming it. It is the correct direction
for this failure: an unread file wrongly reported as read is what deletes
working code, and the reverse merely caps confidence until the next full pass.
Making it exact would mean persisting the refused *paths* and maintaining that
set across batches, which is real state for a bounded and self-clearing gain.

### K-A4 — a search answer stitched from two generations

`StoreQueryEngine::search` resolved "the latest generation" four separate
times: `latest_generation_id`, `count_search_symbols`, `search_symbols`,
`latest_repo_root`. Each took and released the connection lock on its own, so a
writer committing between any two of them split one answer across two
generations — and the daemon is *designed* to commit while clients query.

`Response` states the contract this breaks: clients enforce
`shown + hidden == total`. When the newer generation matched more rows than the
older one counted, `total` came back smaller than `shown`,
`total.saturating_sub(shown)` clamped `hidden` to zero, and the response
claimed `truncated: false`. Measured against the pre-fix code:

```
shown=40 hidden=0 total=1 truncated=false
```

Forty rows returned under a count of one, with nothing marked withheld, on a
tool exposed over MCP.

`Store::search_page` now takes the count, the rows and the repo root under one
lock against one explicitly pinned generation. `neighbors` answers the same
race by detecting a straddle and disclosing it instead of locking, and its
comment says why: holding the lock across a whole fan-out blocks the writer too
long. That reasoning is about fan-outs — `search` is a count, one limited
select and one row, so it can afford the exact answer, and an exact answer
beats a disclosed approximation whenever it can be had.

**On the tests, and what they are worth.** The race can only be *exercised*
probabilistically: against the pre-fix code the concurrency probe caught it in
2 of 3 runs, at 2 violations per 2,000 queries — and the third run passed with
the defect fully present. A guard that green-lights a live bug a third of the
time is the same failure this repository exists to prevent, so it is not the
guard. It is kept, shortened, and labelled: only its *red* carries information.

The guards are deterministic and test the structure the fix installed —
`search_page` names the generation it read and its count matches its own row
list; a store with no generation returns `None` rather than an empty page,
which is a different answer; and `search` reports the current generation's
count across a generation change. Those hold the fix in place without depending
on winning a race.

### Q-9 — the configuration an embedder ships had never been tested

`parse` exists so an embedder can link the query half without tree-sitter and
its 32 C grammars: `devmap-extract`'s manifest says so at length, and GitPulse
is named as the consumer. The feature-off build worked. Its **tests did not
compile at all**.

```
cargo test -p devmap-extract --no-default-features
    error[E0432]: unresolved import `devmap_extract::extract_file`
    error[E0433]: cannot find `treesitter` in `devmap_extract`
```

Thirteen integration tests, one example and three inline tests import items that
are `#[cfg(feature = "parse")]`. One unresolved import fails a whole target, so
the command returned a build error instead of results — and a build error is
easy to read as "this configuration is not meant to be tested", which is how it
survived. Zero tests had ever run against the shape an embedder actually links.

Fixed with `required-features = ["parse"]` on the fourteen affected targets, so
they are *skipped* when the feature is off rather than failing the build, and
`#[cfg(feature = "parse")]` on the three inline tests and the one helper that
needs a real extraction. `ignore_rule_tolerance` touches only the model and the
walker and runs in both configurations.

```
--no-default-features   before: build error, 0 tests   after: 60 passed, 0 failed
default features        before: 318 passed / 16 targets  after: 318 passed / 16 targets
```

The second line is the one that matters as much as the first: gating tests is
one careless attribute away from silently deleting them from the build everyone
actually runs, so the default count and target count are asserted unchanged.

### Q-9 (continued) — the same gap in four more crates

The first pass fixed `devmap-extract` alone. Four other crates carry the same
`parse` feature — `devmap-resolve`, `devmap-analyze`, `devmap-query`,
`devmap-store` — and three had the identical defect, including `devmap-query`,
which is the crate GitPulse actually links. Fixing one and stopping would have
left the named consumer's configuration exactly as untested as before.

**My own tooling failed the way the bug does.** I wrote a script to derive
`required-features` from compiler output. For `devmap-resolve` it reported

```
devmap-resolve: declared 0 target(s)
```

which reads as "already clean". It was not: the build was still broken. The
script looked for `could not compile … (test "name")` and broke out of its loop
whenever that pattern matched nothing — and `devmap-resolve`'s failure was in
the **lib-test** target, which stopped cargo before it reached any integration
test. A check that could not run reported what a check that ran and passed
reports, inside the tooling for the fix. It was caught only by running the tests
afterwards instead of trusting the script's summary.

`devmap-resolve` also needed more than target declarations. Its inline tests are
fixture-driven, and gating only the functions that name `extract_file` broke
every test calling *those* — so the gate has to be a fixpoint over the call
graph (7 seeds closed to 32 functions), plus three module-level
`use devmap_extract::extract_file;` statements that live outside any function,
plus five helpers left dead once their only callers were gated. Gating per
function rather than per module is what keeps 15 of its tests running with the
feature off instead of none.

| crate | default (before → after) | feature-off (before → after) |
|---|---|---|
| devmap-extract | 380 → 380 | build error → 60 |
| devmap-resolve | 113 → 113 | build error → 15 |
| devmap-analyze | 86 → 86 | build error → 25 |
| devmap-query | 166 → 166 | build error → 38 |
| devmap-store | 101 → 101 | 101 → 101 (never had the defect) |

The default column is asserted from baselines captured *before* any gating, for
the reason the whole item exists: gating tests is one careless attribute away
from deleting them from the build everyone runs, and that deletion would be
invisible. `cargo clippy --all-targets` is clean in **both** configurations —
feature-off clippy is what surfaced the five orphaned helpers, which no test run
would have reported.

### K-A6 — an index that does not exist, reported as current

`is_fresh` was `pending_count == 0`, computed independently at two call sites —
the daemon's `status` dispatcher and `devmap status` — and neither consulted
`latest_generation`. A store whose schema exists but which holds **zero
generations**, the state a `devmap build` that aborts partway leaves behind, has
nothing queued. So both answered `is_fresh: true` about an index that does not
exist, with `degraded_reason: null`. Downstream, `devmap_client.is_map_stale()`
is `not is_fresh or pending_count > 0`, so the entire stack reported the map as
current while nothing at all had been indexed.

"Nothing is queued" and "the index is up to date" are different claims. An empty
store is exactly where they come apart, and only one of them was being checked.

Red proof, against the unmodified dispatcher:

```
a_store_with_no_generation_is_not_reported_fresh
  left: Bool(true)   right: false
```

The control — a store with a generation and an empty queue *is* fresh — passed
while the defect was live, so it constrains the fix rather than following it. A
change that reported everything stale would satisfy the first test and fail this
one.

End to end, through the release CLI against a current-schema store with its
generations removed:

```
schema_version: 13   generation_id: null   is_fresh: false
degraded_reason: "this store holds no generation: nothing has been indexed yet — run `devmap build`"
```

**One owner.** The rule was duplicated verbatim across two crates, which is how
it survived: fixing one copy leaves the other lying. `index_is_fresh` and
`freshness_degraded_reason` now live once, in `devmap-serve::protocol`, and
`devmap-cli` calls them. They were deliberately *not* put on `StoreStatus`: from
the store's side a generation-less store is not damaged, merely empty — it is
only the *freshness* claim that an empty store falsifies. Returning
`is_fresh: false` with `degraded_reason: null` would have moved the same defect
one step downstream, leaving a caller told the index is stale with no way to
learn why.

### K-B3 — a torn-read guard that quietly became a length check

`read_stable_source` proves a changed file was not being written while it was
read: stat, read, stat, and admit only if nothing moved. The comparison was

```rust
before.modified().ok() == after.modified().ok()
```

On a filesystem where `modified()` errors, both sides collapse to `None`,
`None == None` is true, and the guard degrades to comparing lengths — which is
precisely what a same-size in-place edit survives. The read is then admitted as
clean and stored as though it were the file on disk, with nothing recording
that the timestamp half of the check never ran. The timestamps are the only
signal separating "nothing moved" from "it changed to something the same size",
so losing them has to make the answer *I cannot tell*, not *yes*.

**No red test was possible through the IO path** — `modified()` does not fail on
macOS or Linux, and the audit's own reachability label for this was INFERRED,
not verified. Rather than write a test against the fix and imply it had proved
something, the decision was **extracted first, unchanged**, and the existing
stat-read-stat test re-run to show the extraction altered no behaviour. The red
proof is then against the extracted pre-fix logic:

```
a_read_with_no_usable_timestamp_is_not_called_stable  FAILED
  with no modification time on either side there is nothing left but the length,
  and equal lengths are exactly what a same-size in-place edit produces
```

The control passed throughout and pins the three genuine rejections — length
moved, mtime moved, content shorter than the file — so a "fix" that answered
`false` unconditionally would fail it. One side alone is also rejected: half a
timestamp comparison is not a comparison.

The failure message was widened too. It said only "did not stabilize", which
misdirects for this case: a file genuinely churning settles, a filesystem that
cannot report modification times never will, and an operator reading the old
message would keep waiting for the second to clear.

### K-B2 — two unbounded queues in the watcher

Both were bounded only by how fast the user's tree changes, which is not a
bound. A `git checkout` across a large repository is the ordinary way to reach
either.

**The debounce set.** `pending` was an uncapped `BTreeSet<String>` held for up
to `MAX_DEBOUNCE_HOLD` (10 s), growing with every changed path for the whole
window. It now collapses at `MAX_DEBOUNCE_PATHS` (4,096) — *collapses*, not
drops. Past that many paths the itemised list has stopped being cheaper than
re-walking the tree, and the root entry the drain already expands covers every
path the buffer was holding plus anything it had not yet been told about. The
only thing lost is the itemisation. Discarding paths would lose real edits with
nothing recording the loss, which is the failure the cap exists to prevent.

Red proof, with the cap reverted:

```
a_flood_of_paths_collapses_to_a_rescan_instead_of_growing  FAILED
  the buffer held 4596 paths; the cap is 4096
```

The control — two edits still flush as two paths — stayed green throughout, so
a buffer that answered "rescan everything" to every batch would fail it.
`watcher.rs` was restored byte-for-byte after the revert (shasum matched).

Writing that test corrected the fix. The first version asserted the flush equals
the rescan sentinel and **failed with the cap already in place**: the buffer
collapsed correctly and then re-accumulated the next 500 paths on top. The bound
still held, but itemising after a collapse is pure cost paid during exactly the
churn that caused it, so the collapse is now sticky until the batch flushes.

**The event queue.** notify delivered into `std::sync::mpsc::channel()` —
unbounded. Bounding it alone would have been the wrong fix: `SyncSender::send`
blocks when full, and the thread it would block is the one the OS delivers to,
so stalling it is precisely how events get dropped. The send is a `try_send`,
and a refusal sets a flag the loop answers with a whole-tree rescan — the same
answer K-A3 already gives the kernel's own "I dropped events" notice, and for
the same reason: lost coverage must not look like a quiet tree.

**No direct test for the overflow path.** It needs the real notify thread
outrunning the consumer, which this suite cannot stage deterministically. The
reachable half — the debounce cap — is covered; the queue bound is argued from
construction, and is recorded here as such rather than implied to be tested.

### Q-10 — two reads, two generations, one answer (devmap-query)

The K-A4 class that `search_page` closed had two more instances in the query
engine. Both compose a *pair* of store reads, and `Store`'s reads each open
their own snapshot, so the pair can straddle a commit.

**`preview`.** It lists callers at or above a confidence floor, counts callers
at floor 0.0, and subtracts to report how many the floor excluded. The two
differing floors are deliberate and documented — the difference is precisely
what the floor hid. The defect is the straddle, and `saturating_sub` is what
makes it silent: when the newer generation holds fewer callers the difference
clamps to zero and `preview` reports that the floor excluded nothing. "No
ambiguous callers" and "I counted a different corpus" become the same answer,
on the surface whose entire job is to say what an edit would break. Drawn from
one generation the floored set is a subset of the unfiltered one, so the
subtraction cannot underflow at all.

**`dead_symbols`.** It resolved the generation three times — an existence
check, `latest_analysis`, then `latest_dead_symbols` — and attached the
coverage disclosure from one read to rows from another. A disclosure saying
the corpus was fully covered, over rows from a generation where it was not, is
the exact combination that promotes a finding from "look at this" to "safe to
delete".

Both now go through `Store::callers_page` / `Store::dead_page`, built on the
`latest_snapshot` mechanism `search_page` already introduced rather than a
second pattern beside it.

**On the tests, plainly: four of the five are structural guards, not red
tests.** They pin `callers_page`/`dead_page` to a single generation going
forward, but they are green against the pre-fix code too, because with nothing
interleaving both spellings return the latest generation. No test written
against the new API can be red against the old one: the defect needs a commit
to land *between* the two reads, and there is no longer a gap between them to
land in.

So the evidence is the fifth test, which reproduces the straddle itself — two
handles on one file-backed store, a commit forced between the two unpaired
reads, showing the analysis describing generation 1 while the rows describe
generation 2. Not a race; a forced schedule. It passes before and after the
fix, because it exercises the old composition, which still exists and is still
unsafe to use as a pair. That is recorded in the test's own doc comment so five
green checkmarks cannot be read as five proofs.

The third instance the store audit found — `latest_dead_symbols` +
`count_dead_at_least` — is deliberately left alone: `count_dead_at_least` has
only a test caller, and pairing it would mean shipping an unwired API.

### Q-11 — the feature-off gate regressed, because nothing ever ran it

`required-features` gating was added to `devmap-query` so the crate's tests
compile with `--no-default-features` — the shape an embedder links. Two targets
added after that pass, `query_work_is_bounded_by_the_answer` and the
`query_bench` example, were never gated, and

```
cargo check -p devmap-query --no-default-features --all-targets   → exit 101
```

The gap survived for the same reason it did the first time: a build error reads
as "this target was not meant to be built here" rather than as a failure, and
no gate anywhere ran that configuration. `--no-default-features` appeared in no
CI workflow at all.

Only those two targets were gated. The other four ungated targets
(`workspace_registry_concurrency`, `impact_breakdown`, `query_phase_ab`,
`query_snapshot`) compile feature-off and are deliberately left ungated:
`required-features` means *skip*, so gating a target that would have built is
coverage quietly deleted. Gating is a cost, not a cleanup — `devmap-query` runs
39 tests feature-off against 169 with `parse` on, and that gap is the price.

**The new CI job must stay per-crate, and this is the part worth not
"simplifying".**

```
cargo check --workspace  --no-default-features --all-targets   → exit 0
cargo check -p devmap-query --no-default-features --all-targets → exit 101
```

`--no-default-features` turns off the *workspace members'* own defaults, but
`devmap-cli` and `devmap-serve` depend on `devmap-query` without
`default-features = false`, so unification switches `parse` back on for the
shared graph. The workspace-wide spelling is a check that cannot fail for this
class — it reports the same green as a check that actually examined something.
It is the tidier-looking command and the useless one.

Post-fix the per-crate matrix is green across all five `parse` crates, with 245
tests actually running feature-off: extract 60, resolve 15, analyze 25, store
106, query 39.

### Q-12 — `cargo fmt --check` had never run on the port

CI's first step is `cargo fmt --all -- --check`. The workflow has never been
pushed, so that step has never executed, and 17 files were unformatted —
committed ones included. Every commit in this pass would have failed CI before
reaching a single test. Formatting is now clean workspace-wide; no logic
changed.

### Q-13 — a fan-out paid for one index per walk, not one per direction

`traverse_graph` builds an adjacency map over the whole edge slice before it
walks. `neighbors` asks for both directions of up to 16 targets and `explore`
does the same per definition, so N targets rebuilt the same two maps 2N times
over an edge slice that does not change between them. The lane that found this
measured it and left it, correctly, as out of its scope: `traversal.rs` said in
so many words that sharing an index "is a decision for the caller that owns the
query loop". This closes it from the caller's side.

`AdjacencyIndex::build(edges, reverse)` is now the index, and
`traverse_graph_indexed` the walk; `traverse_graph` is a thin wrapper that
builds a single-use index and calls it, so there is still exactly one
implementation of the walk and every existing caller is untouched.

**The direction lives in the index and nowhere else.** `traverse_graph_indexed`
takes [`TraversalLimits`], which deliberately has no `reverse` field, and reads
the direction off the index. An index built one way cannot be walked the other,
because the mismatch is not expressible — not because an assertion catches it.
That distinction matters here more than usual: a reversed walk over a forward
index does not fail, it answers plausibly and wrongly, which is how "what
depends on this" silently becomes "what this depends on".

**Measured, interleaved before/after** on a generated corpus of 2,000 files /
126,000 symbols / 330,000 edges, three rounds alternating the two binaries
built from the same tree with only the hoist reverted:

```text
neighbors (8 targets)      cold                warm p50
  before                   787.83 ms           620.50 ms
  after                    351.26 ms           180.10 ms
                           -55% (2.2x)         -71% (3.4x)
```

Round-to-round spread was under 1% on both sides, and every run returned the
same answer — "8 answers, 96 edges shown" — so this is the same work done
fewer times, not less work done.

**Red proof.** `a_fan_out_pays_for_its_index_once_per_direction.rs` counts
allocations rather than time, for the reason its sibling gives: counts are
deterministic and a timing assertion turns a loaded machine into a red build.
It compares `neighbors` against *itself* at one target and at sixteen, which is
the only shape that isolates a per-target cost — the existing comparison
against separately-issued queries stays green either way, because the
composition is still cheaper by the whole edge read.

```text
index per target     299,720 allocations for 16 targets vs 88,310 for one  3.4x
index per direction   75,160 allocations for 16 targets vs 74,275 for one  1.0x
```

With the hoist reverted the test fails at 3.4x; restored, it passes at 1.0x,
and `engine.rs` was restored byte-for-byte afterwards (shasum matched). The
bound is 2, which sits between the two worlds rather than being tuned to
either.

`blast_walk` builds its own inbound index and is deliberately left alone: it
builds once per call rather than once per target, and it filters on
`min_confidence`, so it is a different index answering a different question.
Folding it in would be the kind of unification that produces a helper with a
boolean meaning "actually do the other thing".

**Still open, measured on the same corpus:** `dead_symbols` reads all 80,000
non-exempt dead rows to show 66 (39.6 ms warm). The read is genuinely
unbounded, but `total: 80000` is the honest denominator that makes "66 shown of
80,000" true, and clients enforce `shown + hidden == total` — so a `LIMIT` has
to come with a `COUNT`, the way `explore` already pairs them. That is the next
one, not this one.

### Q-14 — a delete-this list read the whole corpus, twice

`dead_symbols` shows a few dozen rows. It read every dead-symbol row of the
generation, discarded the exempt ones in Rust, and the budgeter kept 66. On the
benchmark corpus that is 80,000 rows read to show 66; on this repository, 6,976
of the 7,176 rows read were exempt and dropped on arrival.

The row read is now `WHERE is_exempt = 0 ... LIMIT ?`, one more row than the
budget can seat so `budget_take` still reports `truncated` for the right
reason. Ordering is unchanged: the old query sorted by `is_exempt` first, so
filtering on it makes that key constant and leaves the survivors in the order
they already had.

**The bound is only safe because the count survives it.** `Response` carries
`shown + hidden == total` and clients enforce it, so a bounded read that also
shrank `total` would turn "66 of 80,000" into "66 of 66" — a capped list
reporting itself as the whole truth. `DeadPage` therefore carries
`total_non_exempt` from a `COUNT(*)` in the same pinned snapshot, and the
engine restores it as the denominator, the way `explore` already does.

**Writing the test corrected the diagnosis.** With the SQL bound in place the
allocation test still failed, at 8.8x. The cause was not the row read at all:
`AnalysisSummary` embeds `dead_symbols: Vec<DeadSymbolReport>`, a second
complete copy of the very list being paged, and `dead_page` deserialized the
whole summary to read one status field. Measured on the benchmark corpus, the
stored blob is 10,122,764 bytes and 10,084,001 of them — 99.6% — are that list.
Bounding the SQL read while parsing the blob accomplishes nothing.

So the fix is two halves, and each was proved load-bearing by reverting it
alone:

```text
both applied                          52 allocations vs 52     1.0x   pass
SQL bound reverted, disclosure kept 4,016 vs 413              9.7x   FAIL
disclosure reverted, SQL bound kept 4,065 vs 462              8.8x   FAIL
```

The second half is `AnalysisDisclosure`: the scalar fields a coverage
disclosure needs, read from the same JSON, with serde stepping over the two
vectors instead of building them. No second format and no second writer —
`analysis_disclosure_agrees_with_the_summary_it_reads` pins the two together,
because a field that drifts out of the disclosure would silently start reading
as its default, and "0 unattributed calls" is exactly the reassuring answer
this type must never invent.

**Measured, interleaved, both binaries warm**, four rounds on the 330,000-edge
corpus:

```text
dead_symbols warm p50   before  42.96  40.69  39.15  49.19   median 41.83 ms
                        after   14.93  25.39  14.60  13.78   median 14.77 ms
                                                             -65% (2.8x)
```

All eight runs answered `shown=66 total=80000`, so neither the answer nor the
denominator moved. Three of the four `after` rounds sit in 13.8–14.9 ms; the
25.39 outlier was three concurrent agent lanes competing for the machine, and
is reported rather than dropped.

**What this did not fix, measured.** Allocations fell 78x (4,065 to 52) but
wall clock only 2.8x, and the gap is the point: the 10 MB blob is still
`SELECT`ed whole into a Rust `String` and still lexed end to end.

The follow-up is Q-18 below, and it is worth reading for how the prediction
came out: stripping the arrays inside SQLite cut what crosses the boundary by
50,613x and moved wall clock by 4.5%. The blob still has to be *parsed* to be
stripped; the parse moved from serde to SQLite's JSON1 and did not disappear.
The only fix that removes it is to stop storing `dead_symbols` in
`analysis_json` when `generation_dead_symbols` already holds it — a
stored-format change, so a migration question rather than a patch.

`Store::latest_dead_symbols` stays unbounded and unfiltered on purpose. Its
callers compare whole generations for incremental-vs-cold equivalence, where an
omitted row is the failure they exist to detect. Conflating the two reads broke
exactly those seven tests during this work, which is how the distinction got
documented on the function.

## The MCP / hooks / plugins fan-out

Three lanes, file-disjoint, each with its own `CARGO_TARGET_DIR`, briefed to
read the live specification rather than recall it and to say plainly when a
test is a structural guard rather than a watched-red proof. Every claim below
was re-verified against the code before its commit landed; the reports were not
taken at face value.

### Q-15 — the MCP server did not conform to the spec it advertised (`49be1c5`)

Twelve defects. The three that matter are honesty failures rather than crashes.

`server/discover` answered on stdio advertising `2026-07-28`, telling dual-era
clients to probe with it, and the request handler then read no version at all.
`id: null` was answered with a *successful result* — one the client is obliged
to reject. And every success emitted `structuredContent` with no `outputSchema`
declared anywhere, so the fields a client would use to detect an incomplete
answer — `truncated`, `walk_incomplete`, `shown`/`hidden`/`total`, the whole
basis of the honesty contract in this file — were undeclared. A client could
not have validated the very markers that exist to stop it over-trusting us.

`devmap_neighbors` also declared a budget it multiplies by up to 32:
`neighbors_once` spends the full `token_budget` per target *and* per direction,
over up to `MAX_NEIGHBOR_TARGETS` (16) targets. The behaviour is right — each
walk needs its own budget — so the declaration was fixed, and it now derives
the multiplier from the constant so it cannot drift.

Left as a genuine conflict rather than a guess: error frames emit `"id": null`
when the id is unrecoverable. JSON-RPC 2.0 §5 says it MUST be Null; the MCP
schema types `id` as `string | number`, implying omission. The existing test
encoding the JSON-RPC reading was **not** weakened to suit the change.

### Q-16 — Dev Map could not integrate itself (`4f4ec2f`)

Hook specs and plugin manifests were emitted only by the Python layer. The Rust
CLI now emits and validates its own: `devmap claude hooks | events | plugin |
validate`.

33 hook events exist; Dev Map handles 2 and records a machine-readable reason
for the other 31, and `devmap claude events` reports "2 of 33" rather than just
the handled count — a table showing only what it covers reads as coverage it
does not have.

`PreToolUse`, `PermissionRequest` and `PermissionDenied` are the events whose
output can allow or deny a tool call, rewrite its input, or persist a rule up to
`bypassPermissions`. Nothing is implemented on them, and that is *enforced*:
the writer bails on any spec whose event is in `PERMISSION_DECIDING_EVENTS`. A
code index has nothing to contribute to an authorization decision. The writer
also refuses a hook naming a `devmap` subcommand the binary does not register —
such a hook fails on every fire, silently, for ever.

The `claude plugin validate --strict` acceptance test is `#[ignore]`d because CI
has no `claude` binary, so an ordinary run reports it *ignored* and never as a
pass.

### Q-17 — the drain read its own mistake back as proof (`0db537e`)

The drain read git HEAD twice: once to decide whether to rebuild, once to stamp
the generation, seconds apart across a full resolve. A checkout landing between
them made it decide "unmoved, carry forward" and then stamp the generation with
the HEAD it *had* moved to — and that stamp makes the error permanent, because
the next drain compares HEAD against the stamp, finds them equal, and carries
forward again. One read now, returning `{sha, moved}`, with the stamp being that
same reading.

Four more of the same family: a file removed mid-read was charged a retry toward
*permanent quarantine*; `exists()`/`is_file()` collapsed every stat failure
(symlink loop, lost `+x`, stale NFS handle) into "deleted", which drops the
file's rows and hands the dead-code pass symbols to call unreferenced; the
watcher fabricated U+FFFD paths for non-UTF-8 names and recorded deletions of
files nothing had ever indexed; and `Err(_) => return Ok(())` in the IPC handler
returned exactly the `Ok(())` a *served* request returns.

The O(batch²) claim lookup is the one performance fix here, measured at the real
8192 bound: 62.35 ms → 348.6 µs (178.9x), with 16 ms → 62 ms across a 2x batch
confirming the quadratic. **It ships with no red test, stated rather than
implied** — identical observable behaviour, so nothing can fail against the
pre-fix code. The measurement is the evidence.

The watcher event-queue overflow, recorded under K-B2 as argued-from-
construction because it needed the notify thread to outrun the consumer, is now
staged deterministically with no OS involvement: the flag stays clear for all
4,096 events, the 4,097th sets it, and the rescan is delivered *ahead* of the
itemised paths still queued behind it.


### Q-18 — a 10 MB blob crossed the boundary to deliver 200 bytes

Q-14 stopped `dead_page` *materialising* the analysis summary's embedded
dead-symbol list, but not *transferring* it: the column was still `SELECT`ed
whole into a Rust `String`. It is now stripped inside SQLite —
`json_remove(analysis_json, '$.dead_symbols', '$.communities')` — so only the
disclosure fields cross. Measured on the benchmark corpus: **10,122,764 bytes
in the column, 200 bytes out**.

**Call this what it is: a memory bound, not a speed-up.** Interleaved, three
rounds, both binaries warm:

```text
dead_symbols warm p50   before  13.48  13.62  13.47   median 13.48 ms
                        after   12.96  12.87  12.67   median 12.87 ms
                                                      -4.5%
```

The ranges do not overlap, so the 4.5% is real — and it is far less than the
byte reduction suggests, which is the useful part of the result. SQLite must
still parse 10 MB of JSON in order to strip it; the parse moved out of serde
and into JSON1 rather than going away. What genuinely changed is the transient
allocation: a 10 MB `String` per call became 200 bytes, and with the MCP server
now admitting up to 256 in-flight requests that is the difference between a
bounded read and one whose peak memory is set by the size of the corpus.

The safety of the strip rests on corruption and absence staying
distinguishable. They do, and by SQLite's own behaviour rather than by a guard
of ours: a malformed blob **raises**, where `json_extract` on a missing path
would have returned `NULL` — and `NULL` is exactly what a generation with no
analysis produces, so that spelling would have turned "could not be read" into
"is not there" with nothing failing.

`a_corrupt_analysis_is_not_an_absent_one.rs` pins it, and **is a structural
guard, not a red test — checked, not assumed.** Run against the previous
`SELECT analysis_json` spelling it still passes, because serde rejected the
malformed blob just as SQLite now does. There is no pre-fix state in which it
fails; what it guards is the future change that would reintroduce the hazard.
## Seam lane: the per-tool-call refresh moved onto the kernel (2026-09-05)

`dev hook post-tool-use` runs after every tool call an agent makes. On an
**unchanged** tree it cost **1.41 s**, and essentially none of that was work:

| what | cost | why it was spent |
|---|---|---|
| `import devcouncil.cli.commands.hook` | 149 ms | `live.tasks` → `storage.db` loaded SQLAlchemy + SQLModel (90 ms) and `json_persist`/`hook_policy`/`telemetry.traces` loaded pydantic (42 ms), on a path that opens no database and writes no trace |
| `compute_freshness` in Python | ~150 ms | two `git ls-files` passes and a stat walk over 1,388 files, immediately after the kernel had walked the same tree |
| `devmap build` | 250 ms | the real incremental check |
| `devmap manifest` | 430 ms | re-read the whole generation out of SQLite and re-serialized a **22 MB** `code_graph.json` to produce bytes identical to the ones on disk; `write_atomic` then compared them and declined the rename |
| `devmap status` | 10 ms | a third process, asking the store a question the build already knew |
| debounce | 300 ms | deliberate burst coalescing; unchanged |

Three process spawns and 22 MB of JSON for a tree that had not moved.

### What changed

**One invocation.** `devmap build --manifest` writes `repo_map.json` and
`code_graph.json` from the generation the build leaves current, in the same
process and the same store open, and embeds the store's own status in its
`--json` result. `devmap manifest` stays a command of its own. `devmap_engine`
gained `build_map_result`, which is now the one function that runs the kernel;
`build_map` is its path-returning adapter. The seam's `_kernel_status` spawn is
gone from the fused path and kept as the fallback for a kernel that cannot
report it.

**Artifacts are rewritten only when they would change.** A sidecar beside the
store (`<db>.artifacts.json`) records the writing binary's own `(path, size,
mtime)`, every input the artifacts derive from — generation id, pending count,
built head, repo root, the three freshness stamps, the code-graph schema — and
each output's `(len, mtime, inode)` as written. When all of it still holds the
generation is never read. Fail-closed in every direction: an unreadable sidecar,
a missing or edited output, a different binary, one differing input — each is a
miss, and a miss regenerates. `artifacts.rs`'s existing `should_regenerate`
could not do this job: it reads the finished file back, which for a 22 MB graph
means the serialization has already happened.

**The kernel computes its own freshness digests.** `devmap_query::freshness` is
a deliberate transcription of `RepoMapper.get_git_files` /
`_files_fingerprint` / `_content_fingerprint`, down to the `\0` separator, the
`c2:` scheme and the `(size, mtime_ns, ctime_ns)` memo key — it reads and writes
*the same* `.devcouncil/cache/content_hashes.json` Python does, rather than
keeping a second memo of the same computation. SHA-1 and BLAKE2b are implemented
in-tree (no hashing crate is linked and none was added) and pinned to FIPS 180-4,
RFC 7693 Appendix A and CPython `hashlib` vectors. Only the git path is
implemented: when git cannot answer, the kernel reports
`freshness_source: "unavailable"` and the Python `stamp_freshness` fallback runs,
because a second transcription of a *different* enumeration is where the two
would silently disagree.

**`devmap freshness`** answers `{stale, reason, checked:{head, inventory,
content}}` for a working tree, with no store required. It short-circuits exactly
as `map_is_stale` does — the content fingerprint is computed only once head and
inventory match — and a skipped field carries `checked: false` and no verdict.

**Python shrank.** `hook.py`'s six heaviest imports are deferred (as thin
functions, not a PEP 562 module `__getattr__`: `__getattr__` is consulted for
`module.name` and **not** for a bare global inside a function, which is why the
existing `get_db` deferral had to spell out `sys.modules[__name__].get_db` — and
why deferring `dump_json` that way raised `NameError` inside
`_try_acquire_refresh_lock`, silently disabling the map refresh under the hook's
`except Exception`).

### Measured

Release kernel pinned via `DEVMAP_BINARY`, on a quiet copy of this repository
(1,414 tracked files, 22.5 MB `code_graph.json`), min of 8:

| `dev hook post-tool-use` | before | after |
|---|---|---|
| unchanged tree | **1.41 s** | **0.77 s** (−45%) |
| one edited file | 4.12 s | 3.93 s |
| a new file | 4.18 s | 3.91 s |

A confirmation run of the unchanged row against the *final* kernel binary
(rebuilt after the port lane's last merge landed under it) gave 0.83 s, min of
5. Both numbers are reported; the table's 0.77 s is the min of 8.

Kernel only, unchanged tree, same corpus:

| | before | after |
|---|---|---|
| `build` | 0.25 s | — |
| `manifest` | 0.43 s | 0.06 s standalone (skip) |
| `status` | 0.009 s | — |
| **the three together** | **0.69 s, three processes** | **0.31 s, one process** |

`import devcouncil.cli.commands.hook`: **149 ms → 31 ms**, and a cold import now
loads neither SQLAlchemy, SQLModel, pydantic nor rich.

The two changed-tree rows moved less than their own spread (before 4.12–6.06 s,
after 3.93–5.87 s). They are dominated by the kernel's whole-tree resolve and
analyze, which this lane does not touch, and are reported as unmoved.

### `map_is_stale` was left on Python, with the measurement that says why

The brief asked for `RepoMapper.map_is_stale` to delegate to the kernel. It was
built (`devmap freshness`), measured, and **not** switched. On this repository,
in-process:

```text
python get_git_files (2 x git ls-files + 1,388 stats)   52 ms
python _content_fingerprint (warm memo)                  7 ms   (cold: 50 ms)
python map_is_stale, whole call                         81 ms   (min of 5)
kernel  devmap freshness, whole call incl. spawn        54 ms   (min of 5)
```

The check is dominated by `git ls-files` and the stat walk, which both
implementations pay identically; Rust's advantage on hashing is most of the way
cancelled by a process spawn. Delegating would trade ~27 ms for a second code
path in a fail-closed correctness check with twelve call sites, and would make
the check *unavailable* (hence stale, hence a rebuild loop) wherever the kernel
binary is missing but Python can answer perfectly well. The command ships
because the fused build uses the same code in-process, where it replaces
Python's work rather than duplicating it, and because a non-Python consumer can
now ask.

### The daemon path: the premise does not hold, so nothing was changed

The brief asked that the hook not spawn a build when a daemon socket is live,
since "the watcher already indexes changes". Verified, and it is the store the
watcher indexes, not the artifacts: `rg 'write_manifest_atomically|repo_map.json|
write_code_graph' rust-port/crates/devmap-serve/src/` matches nothing, so
`devmap serve` never writes `repo_map.json` or `code_graph.json`. Eleven Python
modules read the graph and the agent guides tell agents to open the map, so a
hook that skipped the build behind a live daemon would leave both permanently
behind the store. (For the record on the other half: the hook does not consult
`DevMapClient` or a socket at all today — `rg 'DevMapClient|ipc|socket'
src/devcouncil/cli/commands/hook.py` matches nothing — so there is no existing
daemon branch to make consistent.)

What the fused command does instead is the right shape for that case: when the
daemon has kept the store current, `build --manifest` takes the unchanged
early-return and the artifact stamp declines both writes, so the hook's cost
behind a live daemon is already the 0.31 s floor. Giving the daemon the
artifacts is a `devmap-serve` change and is left as a named follow-up.

### Red tests

| what it holds | test |
|---|---|
| the fused command writes both artifacts and reports the store | `devmap-cli/tests/manifest_is_written_once.rs::one_invocation_writes_the_artifacts_and_reports_the_store` |
| a no-op leaves length, mtime and inode untouched | `…::a_second_run_over_an_unchanged_tree_rewrites_nothing` |
| every input that changes the artifacts defeats the skip | `…::every_input_that_changes_the_artifacts_defeats_the_skip` |
| what a skip preserves equals what a write produces | `…::the_skipped_artifacts_equal_the_ones_a_forced_write_produces` |
| `freshness` answers field by field and fails closed | `…::freshness_answers_field_by_field_and_fails_closed` |
| a skipped content check reports no verdict | `…::a_skipped_content_check_says_it_was_skipped` |
| the kernel's three digests equal Python's, byte for byte | `devmap-query/tests/freshness_parity.rs::the_kernel_and_python_agree_on_all_three_freshness_digests` |
| …on an empty repo, quoted and non-ASCII names, an excluded file name, a 1 MiB file, untracked and deleted paths | `…::the_two_agree_on_an_adversarial_tree` |
| SHA-1 / BLAKE2b against published vectors, and streaming = one-shot | `devmap-query/src/digest.rs::{sha1_matches_the_fips_180_vectors, blake2b_matches_rfc_7693_and_cpython_hashlib, sha1_streaming_equals_one_shot, blake2b_streaming_equals_one_shot}` |
| the stamp skips only when every question answers the same | `devmap-query/src/artifacts.rs::the_artifact_stamp_skips_only_when_every_question_answers_the_same` |
| the hook module loads no ORM and no pydantic | `tests/unit/test_hook_import_cost.py::test_importing_the_hook_module_does_not_load_the_orm_or_pydantic` |
| every deferred name resolves *as a global*, not only as an attribute | `…::test_every_deferred_name_still_resolves_and_is_still_patchable`, `…::test_the_lock_helper_can_serialize_without_a_module_scope_import` |

`sha1_streaming_equals_one_shot` failed on the first run against the
implementation written for it — `Sha1::update` reset its buffer to empty
whenever a chunk arrived that did not complete a block, so any input fed in
pieces smaller than 64 bytes digested wrongly while the one-shot path was
correct. That is why the streaming tests exist beside the vector tests.

## Port of the 1a2151 round-1 work onto main (2026-09-05)

Two sessions carried `AUDIT_KERNEL_2026-09-05.md` at the same time. One landed
its fixes directly on `main`; the other landed an independent set on
`claude/dev-map-performance-hardening-1a2151`, commit `d2fb25e`. `main` is the
integration line, so `d2fb25e` was **not merged** — it is kept intact as the
record of the second set, and the pieces `main` lacked were ported onto
`claude/devmap-reconcile-1a2151` one at a time, each adapted to `main`'s owners.

The rule the whole pass ran under: **where both lines had an implementation of
the same behaviour, exactly one survives.** A port that lands beside what it
duplicates is the failure mode this section exists to rule out, so every row
below says what was kept from each side.

### Piece by piece

| piece | verdict | what was kept from each side |
|---|---|---|
| Discovery refusals as coverage loss (K-A2) | **already on main** (`a3ed650`, `c8ea0a7`) | `main`'s `DiscoverySkipReason::is_refusal` + `DiscoveryCoverage` + `analyze_with_discovery` stay the refusal owner. The port's `refused_by_discovery` fold was dropped as a second owner; its test was rewritten against `main`'s spelling (`devmap-cli/tests/discovery_refusal_is_coverage_loss.rs`) |
| `is_fresh` requires a generation (K-A6) | **already on main** (`b338c0e`) | `devmap_serve::index_is_fresh` / `freshness_degraded_reason` stay the one owner. The port's `StoreStatus::is_fresh()` is the same fix by another name and was dropped |
| Watcher queue bounds (K-B2), torn-read mtime guard (K-B3) | **already on main** (`91ecdac`, `80e3fff`) | `main`'s, unchanged |
| Search counted from one generation (K-A4), store snapshot pin | **already on main** (`14560f7`, `ec1e862`) | `main`'s. `ec1e862` is a correctness fix the port did not have; it stays and the ported index is built from rows read *inside* that snapshot |
| Feature-off builds, MCP 2026-07-28 conformance | **already on main** (`88d8cf8`, `6ff1a07`, `b458f37`) | `main`'s. The ported stress tests were rewritten to send the conformant headers (`serve_stress.rs::post` derives `_meta` and the mirrored `MCP-Protocol-Version` / `Mcp-Method` / `Mcp-Name` from its own body) |
| Ambiguous fan-out cap | **already on main** | `AMBIGUOUS_FANOUT_CAP`; see AGENT_PLAN.md's SC4 row |
| One owner for `MAX_TOKEN_BUDGET` / `MAX_TRAVERSAL_DEPTH` | **ported**, `1a7fd98` | Kept `main`'s `validate_request` wording and its `assert_eq!` pins, which now check the imported constants and so pin the whole chain. Replaced: the `const` pair in `protocol.rs`, the `.min(64)` / `.clamp(1, 64)` literals in `engine.rs`, `ipc_fuzz.rs`'s private copies |
| CLI boundary validation, one-line `--json`, `--version` naming both schemas, `Store::latest_analysis_status` | **ported**, `d6ac670` | Kept `main`'s freshness and refusal owners (above). `combine_reasons` is `main`'s joiner and does the joining here. Tests `cli_json_contract.rs` (5), `discovery_refusal_is_coverage_loss.rs` (4) |
| Per-generation edge index | **folded**, `1dcbc69` + merge `c063a89` | Both lines answered "stop rebuilding the adjacency map per question". `main`'s `e8c3521` hoisted it to once per direction per request; this branch caches it on the store per generation. Kept: `main`'s `GraphIndex` API shape and `TraversalLimits` (direction lives in the index, so a mismatch is not expressible), `main`'s allocation work in full, `main`'s `path_matches`, `latest_snapshot`, and `latest_edge_rows` keying the cache by the generation the rows were *read* from. Kept from the port: `GenerationEdges` / `DirectedEdges` as the storage, and `traverse_indexed` as the one walk. `AdjacencyIndex` stays for callers holding a loose edge slice — `main`'s per-direction path is intact, just no longer the only one |
| K-B1: bounded search page and per-hit read | **ported**, `2f9916a` | The half `14560f7` / `ec1e862` did not cover: those made `{total, shown, hidden, truncated}` come from one generation; this bounds the I/O the page buys (`SEARCH_PAGE_MAX = 200`, `read_source_prefix`). Fixed a feature-off regression `e8c3521` had introduced (`query_bench` / `query_work_is_bounded_by_the_answer` call `extract_file` without `required-features`) |
| Admission control on three transports, `BODY_READ_TIMEOUT` | **ported**, `177abbd`, then **folded** against `49be1c5` | `devmap-serve/src/admission.rs` is the one implementation; the socket's existing semaphore is *replaced* by it, not joined by it. `main` then added its own stdio ceiling, and the merge is described below. Tests `serve_stress.rs` (9), `daemon_binary_retirement.rs` |
| K-B4: claim index | **folded**, `3425d76` + merge with `0db537e` | Both lines found it and wrote the same `HashMap`. `main`'s code was kept; the port's comment contributed the equivalence argument `main`'s lacked — `pending_paths.path` is the table's conflict target, so a claim set holds each path once and the map is *exactly* the scan, same claim and same panic. `main`'s measurements stand (batch 1024 898.8 µs -> 45.5 µs; 4096 16.18 ms -> 286.3 µs; 8192 62.35 ms -> 348.6 µs). No red test on either side — a cost, not an answer |
| E-8 `recover_lock`, the `ignore_rule_tolerance` diagnostic pin, `mutation_fuzz.rs`, the `budget_probe` throughput table | **ported**, `0306e9e` | Not ported: the `test_process_recovery.rs` fixture change, which existed only to work around the round-1 branch's refusal-as-`ParseOutcome::Failed` fold. `main` records refusals through `DiscoveryCoverage` and still leaves an oversized file queued, so that fixture passes unchanged |
| `extract_tree_with_report` as the one fold owner; `DiscoveryReport::refusals` in the daemon | **ported**, `4795d23` | Adapted to `main`'s refusal owner rather than adding a second. The daemon's two `!matches!(reason, NonSource)` wildcard copies now call `discovery.refusals()`, so a skip reason added later cannot silently default to "not a refusal" on one path only |
| MCP client entry (`mcp --print-config`) | **main's kept**, merge `d5fbeb0` | `main`'s `claude::mcp_entry`, which the plugin bundle also calls, so a host configured from `--print-config` cannot point at a different server than one configured from the emitted plugin. The port's inline builder is gone |
| Ruff fixes | **ported**, `cdfbea1` | The six findings the merge carried |
| Seam: one kernel invocation per refresh, skip-on-unchanged artifacts, kernel-side freshness digests | **ported**, `035fe1e` + `3231018` | The CLI half (`build --manifest`, `freshness`, `StampFlags` / `InventoryFlags` shared by `build` and `manifest`) was staged while `main.rs` was owned by the port lane and applied afterwards by three-way merge against the reconciled file, which had since gained `main`'s `claude` subcommand. Pinned by `devmap-cli/tests/manifest_is_written_once.rs` (6) |
| Ledger edits (`AGENT_PLAN.md`, `IMPROVEMENTS.md`, `AUDIT_KERNEL_2026-09-05.md`) | **re-derived** | Written against `main`'s state rather than copied from `d2fb25e`, because several rows the round-1 branch closed were closed differently here |

### Performance

Release, in-process MCP, `crates/devmap-serve/examples/mcp_bench.rs`. Both sides
measured **in the same session on the same machine**, runs interleaved:
`main`'s binaries against corpora `main`'s kernel built, this branch's binaries
against copies of the same trees rebuilt with this kernel after the merges. The
rebuild is not a confound — the two stores agree exactly on size (15,080 nodes /
74,729 edges and 41,276 / 271,508), which is also the strongest available
equivalence check on the port.

**The machine was not quiet** (load 3–5 from other sessions), so what is quoted
is the **minimum p50 across the runs**, the convention this repository's other
sweeps use. A noisy round is reported here rather than dropped: `impact` on
scholarlm read 73.06 ms / 2.82 ms in the quiet rounds and 99.56 ms / 10.11 ms in
the loudest, which is the size of the spread you should expect if you re-run it.

| store | metric | `main` | this branch | ratio | `d2fb25e` |
|---|---|---|---|---|---|
| corpus (15,080 / 74,729) | status | 774 µs | **13.6 µs** | 57x faster | 16 µs |
| | search | 2.038 ms | 2.129 ms | **+4%** | 2.43 ms |
| | impact | 18.62 ms | **947 µs** | 19.7x faster | 0.93 ms |
| | tools/list | 32.3 µs | 73.3 µs | **+127%** — `main`'s, see below | — |
| scholarlm (41,276 / 271,508) | status | 2.689 ms | **12.8 µs** | 210x faster | 18 µs |
| | search | 3.396 ms | 3.386 ms | — | 3.87 ms |
| | impact | 73.06 ms | **2.823 ms** | 25.9x faster | 3.0 ms |
| | tools/list | 33.3 µs | 70.0 µs | **+110%** — `main`'s, see below | — |

Every row is within 20% of `d2fb25e`, which was the gate this port had to clear.

`impact_breakdown`, same stores: the SQL read is unchanged (4.98 ms -> 5.06 ms,
17.69 ms -> 17.64 ms) — the index is built from the rows that read returns, so it
adds no I/O — while the whole `impact("helper")` call falls 17.57 ms -> 814 µs and
45.98 ms -> 2.61 ms. The read is now 622% and 675% of the call it feeds, which is
the point: what remains is the read, not the walk.

**`tools/list` doubled, and it is not this port.** `main`'s `49be1c5` declares an
`outputSchema` on every one of the 18 tools — the fix for "every success emitted
`structuredContent` with no `outputSchema` declared", where the undeclared fields
were exactly the incompleteness markers (`truncated`, `walk_incomplete`,
`shown`/`hidden`/`total`) a client had no way to validate. The baseline binaries
predate that commit, so this row is not like-for-like: it is the cost of the tool
definitions getting bigger, paid once per `tools/list`, and it buys a client the
ability to check the honesty markers this codebase's contract rests on.

**`search` is flat to +7% against `main`, and where it is slower that is the
price of K-B1.** Bounding each hit's read to `span_end + 3` bytes replaces one
`read_to_string` per hit with a bounded read that must also handle the short-read
and mid-character cases, and the 200-hit page cap does not help a query whose
page was already smaller. Recorded rather than smoothed over: the unbounded
version was faster and would open 5,001 files for a generous budget. Interleaved
medians over three rounds put it at +6.6% on corpus and −8% on scholarlm, which
is inside this machine's noise either way.

### The second merge: `main`'s `49be1c5` and `0db537e` (2026-09-05, later)

`main` moved two more commits after the port had landed, and both touched
`devmap-serve`. Both are folds, not additions, and both left two implementations
of one behaviour in the auto-merge:

**The stdio in-flight ceiling.** The port bounded it with `Admission` at 32,
waiting up to 5 s and then answering a JSON-RPC refusal that named the ceiling.
`main` bounded it at 256 by waiting on the oldest task, and its commit message
records that its own first attempt *shed* at 64 and was caught by
`concurrent_requests_never_interleave_or_lose_an_id`. Git merged the two
cleanly into a file carrying **both** ceilings — the failure mode this whole
pass exists to prevent.

`main`'s policy wins, and its reasoning is now the reason in the source: a bound
that discards a well-formed request from a client doing nothing wrong is data
loss wearing a good error message, and an agent host draining a plan pipelines
hundreds of requests as a matter of course. The port's *structure* wins: the
ceiling is held by `Admission`, the one implementation the socket and HTTP
transports already use, so the number is visible (`peak()`, `shed()`) instead of
being a `while tasks.len() >= N` nobody can measure. So: `MAX_IN_FLIGHT_REQUESTS
= 256` (`main`'s number and doc), enforced by `Admission::admit()` (waiting,
`main`'s policy — the same call the socket transport makes), and `main`'s
`while tasks.len() >= …` loop removed as the second ceiling.

`Admission::admit_within` — the wait-then-shed method — had exactly one caller,
this one, and is **deleted** rather than left as an unused public method that
re-invites the policy `main` rejected. Verified callerless with
`rg --no-ignore --hidden` over every `.rs`/`.py`/`.md`/`.toml`/`.go` file and by
the crate building without it. Its unit test is replaced by one for the policy
that is actually in force: `a_full_pool_queues_the_waiter_and_a_freed_permit_wakes_it`
asserts both halves — a full pool queues rather than refusing, and `shed()` stays
zero while it waits.

`serve_stress.rs::ten_thousand_pipelined_stdio_requests_stay_inside_the_ceiling`
was strengthened accordingly: it accepted "a result, or a refusal naming the
ceiling" and now requires a result, with `shed() == 0`. Measured under the new
policy: in-flight peak **8,333 unbounded against 32 bounded**, 10,000 requests,
none shed, none lost. `main`'s
`a_pipeline_deeper_than_the_in_flight_bound_answers_every_id_exactly_once`
(1,024-deep) passes unchanged.

**The drain's claim index (K-B4).** The port's `3425d76` and `main`'s `0db537e`
are the same `HashMap`, arrived at independently. `main`'s code was kept; the
port's comment contributed the equivalence argument `main`'s lacked.

`cargo test -p devmap-serve --no-fail-fast` after the fold: **231 passed, 0
failed, 0 ignored**, which includes `main`'s new `mcp_spec_conformance.rs` and
`daemon_drain_races.rs` alongside the ported `serve_stress.rs`.

**`search` is 5-7% slower than `main`, and that is the price of K-B1**, not
noise. Bounding each hit's read to `span_end + 3` bytes replaces one
`read_to_string` per hit with a bounded read that must also handle the
short-read and mid-character cases, and the 200-hit page cap does not help a
query whose page was already smaller. Recorded rather than smoothed over: the
unbounded version was faster and would open 5,001 files for a generous budget.
Both figures are within 20% of `d2fb25e` on every row, which was the gate this
port had to clear.

### The third merge: `main` merged this branch, then moved on

`main` integrated the reconcile branch (`862c619`, `f21f816`) while this pass was
finishing, and added four commits after it. Merged back with no conflicts:
`db.rs` keeps `latest_analysis_status` beside `f892725`'s `json_remove` strip —
including `main`'s correction of that method's doc comment, which had explained
the split between it and `AnalysisDisclosure` by a fact `f892725` had since made
false — and this ledger keeps both sessions' sections.

**One defect `main`'s merge carried, found here and fixed:**
`tests/unit/test_devmap_engine.py::test_an_older_kernel_without_the_stamp_flags_still_gets_a_stamped_map`.
It simulates an old kernel by turning off *one* of the two capability probes the
seam makes, and read green for exactly as long as the CLI half of
`build --manifest` was held back. Once that landed, `_build_accepts_manifest`
answered for the real binary, the engine took the fused path, and the fallback
the test exists to protect stopped being exercised — the assertion the test
makes (`assert called`) went red rather than silently passing, which is the good
outcome, but only because the spy was on the fallback itself.

The production fallback was never broken: a genuinely old kernel has neither
capability, `_write_manifest_separately` runs and `stamp_freshness` patches the
digests in. The *fixture* was wrong, so the fixture was fixed — both probes off,
which is what an old kernel looks like — and the reason they can be switched
together is now pinned rather than assumed:
`test_the_two_kernel_capabilities_the_seam_probes_ship_together` asserts against
the real binary that `build --manifest` and `manifest`'s three stamp flags are
one capability, because in the CLI they are one `StampFlags` group flattened
into both subcommands. Removing the added `monkeypatch` line reproduces the
failure, so the fix is load-bearing. `main` verified its merge with cargo only,
which is why this reached it.

### Verification on the final tree

Numbers taken at `7cd9813`, which is `main` plus this branch's remaining commits.
`main` moved five times while this pass ran and was merged in each time; the
gates below were re-run after every merge, and only the last run is quoted.

| gate | result |
|---|---|
| `cargo fmt --all -- --check` | exit 0, no output |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo test --workspace --no-fail-fast` | **1,357 passed, 0 failed, 2 ignored**; zero `test result: FAILED` lines |
| `cargo check -p devmap-{extract,query,store,analyze} --no-default-features --all-targets` | 4/4 exit 0 |
| `cargo build --release` (worktree's own target) | exit 0; `devmap 0.1.0 (store schema 13, code graph schema 2)` |
| `pytest tests/unit -q` | **4,167 passed, 0 failed, 9 xfailed**; last run 1,463 s, against 450 s earlier, because the machine went to load 150-500 |
| `ruff check src tests` | `All checks passed!` |
| `dev map doctor --json` | `ok: true` |
| `dev hook post-tool-use`, unchanged tree | **0.83 s** (min of 8 at load 5.4; 0.85 s min of 5 at load 7.8), against the 1.5 s pre-seam baseline |

The two ignored tests are pre-existing and self-describing:
`the_emitted_bundle_passes_claude_plugin_validate_strict` needs the `claude` CLI
on PATH, and `concurrent_prune_writer_child` is a child-process helper.

The hook figure does not reproduce the seam lane's 0.77 s and is not claimed to:
this machine ran at load 5-10 throughout (several sibling sessions building), and
0.83 s is what it gives under that. The shape of the improvement holds; the last
60 ms is not evidence either way.

## Adversarial pass on the reconciled kernel (2026-09-05)

An attack pass over the kernel as reconciled, driven through the release binary
and the real transports rather than through the library where a fixture can
flatter it. Six defects, each with a test that was watched failing against the
tree that carried it; everything else in the table survived, which is a result
too and is recorded as one.

| surface | attacks run | verdict | closed by |
|---|---|---|---|
| **discovery: hostile trees** | symlink loops; symlinks escaping the root, absolute and `../..`; symlinked directories; 13 hostile file names (newline, tab, RTL override, 4-byte emoji, combining marks, ZWJ, `CON`/`PRN`/`NUL`/`COM1`/`LPT1`, trailing space, 250-char name); 30,000 files; a 377-level directory chain; a `.gitignore` of 10,000 patterns with one malformed glob; `CACHEDIR.TAG`; an unreadable file; an unreadable directory; a source exactly at `MAX_SOURCE_BYTES` and at +1; a 5,000-cell notebook; a 50 MB minified JS one-liner; a 400 KB single-line JS expression | **2 defects** | `discovery_stays_inside_the_root.rs`, `an_unreadable_subtree_is_a_hole_not_a_dead_build.rs` |
| **discovery: torn reads** | a writer thread flipping bytes to `NUL`/`0xFF`/quotes/braces at random offsets throughout 10 consecutive full builds | survived — every mutated file became an `Unreadable` refusal, `degraded_reason` named all 120, `pragma integrity_check` `ok`, status never claimed fresh over nothing | — |
| **concurrency** | 6 simultaneous `build --full` on one store; 50 `SIGKILL`s at uniformly random points of a build, each followed by a status probe, then a rebuild compared against an uninterrupted build's edge digest | survived — 6/6 exit 0 with 6 distinct generations, 0 of 50 kills left an unreadable or dishonest store, digest **converged** (`0b8ef0e5b555df72`) | — |
| **query semantics** | `neighbors` cross-checked against the `impact` and `trace` it claims to compose, over a 61-module generated corpus, at floors 0.0/0.4/0.9 and depths 1/2/3; every reported edge checked against the generation's edge table and against the floor it claimed; `dead` checked against every inbound `Calls` edge at any confidence; `shown + hidden == total` and `truncated == (hidden > 0)` over 100+ budgeted answers; `count_callers_of` against `callers_of` at four floors, two exclusions, and a chunk-crossing batch with duplicates | **1 defect** (the depth) | `answers_agree_with_each_other.rs` |
| **artifacts sidecar** | in-place rewrite of an artifact at equal length with its mtime restored by `utimensat`; corrupt, truncated and foreign-version sidecars; a deleted artifact; a foreign writer | **1 defect** (the restored mtime) | `artifacts_sidecar_adversarial.rs` |
| **`devmap freshness`** | a repository with no commits; a detached HEAD; a tracked file deleted from disk; in-tree and escaping symlinks; a directory that is not a repository; `git` absent from `PATH`; each with and without `--expect-*` | honesty rule survived — every unanswerable case reported `source: "unavailable"` with a reason, `stale: null` when nothing was asked and `stale: true` when something was, and the skipped content check carried `checked: false` rather than a `match`. Never a false `fresh`. **1 defect** in what the inventory counts | `the_freshness_inventory_counts_only_what_discovery_can_index.rs`, `test_repo_mapper_helpers.py` |
| **MCP over HTTP** | HTTP/1.0; `Expect: 100-continue` with and without the body; three pipelined keep-alive requests; a `Content-Length` of 100000 with a 60-byte body; 64 MiB declared; chunked encoding; a 70 KB header; `GET`; a garbage request line; no `Content-Type`; a non-JSON body; headers then `FIN`; 64 and 200 concurrent `tools/call` | survived — `100 Continue` then the answer; `408` after `BODY_READ_TIMEOUT` on the lying length; `413` *before* buffering on the 64 MiB declaration; `415`, `405`, `400` each on its own case; three answers to three pipelined requests; 200/200 `200 OK`, no `500`, no hang. The rebinding guard also refused every request whose `Host` was a name (`403`), which is what it is for | — |
| **MCP over stdio** | 300 pipelined `tools/call` from a client that never reads its socket; 300 with a concurrent reader | survived — RSS grew 34 MB and stopped, the process stayed alive, and on draining, **301 of 301** answers were present; with a reader, 301 distinct ids and 0 errors. Nothing shed, nothing lost | — |
| **determinism (R4)** | the same 120-file, three-language corpus built 5 times in fresh directories on `RAYON_NUM_THREADS=1` and 5 times on 8 | survived — all 10 `code_graph.json` byte-identical after stripping timestamps (`1a947483159beb72`, 1,654,041 bytes) | — |
| **digests** | reviewed against the pinned FIPS 180-4 / RFC 7693 / CPython vectors already in `digest.rs`, including 55/56/64/128/129 byte boundaries, 1 MiB + 1, and chunked-by-1 streaming | already closed, pinned by `digest.rs::{sha1_matches_the_fips_180_vectors, sha1_streaming_equals_one_shot, blake2b_matches_rfc_7693_and_cpython_hashlib, blake2b_streaming_equals_one_shot}` and `tests/freshness_parity.rs` | — |
| **tool surface** | `tools/list` over both transports | survived — 9 tools on each, `outputSchema` on 9 of 9, same names. (The brief's "18 tools" is not this server's count) | — |

### The six defects

**1. An artifact stamp a restored mtime can satisfy.** `ArtifactStamp::still_current`
recorded `len`, `mtime_ns` and `ino`, all three of which an unprivileged writer
can restore: rewrite `code_graph.json` in place at the same length and put its
modification time back — what `cp -p`, `rsync --times`, `tar -x` and
`File::set_times` all do — and `build --manifest` skipped regeneration and
reported `artifacts_unchanged: true` over a graph belonging to another
generation. Permanently: every later run compared against the same doctored
stat. `ArtifactRecord` now also carries `ctime_ns`, which userspace cannot
back-date and which `freshness::stat_key` in the same crate already carried for
exactly this reason; `ARTIFACT_STAMP_VERSION` goes to 2.

**2. A walk over a partly-read corpus.** Found here and, independently, on
`main`. `impact` answered `shown: 0, total: 0, Available, walk_incomplete: None`
over a generation whose own analysis summary recorded that the only caller's
file had failed to parse — while `dead` quoted that summary and `deps` refused
outright. The answer an agent reads as "nothing calls this, it is safe to
delete" was the one of the three that gave it unqualified. `main`'s
implementation is the one kept, because it hangs the disclosure inside
`GenerationEdges` keyed to the generation the rows came from — no straddle
window and no per-query cost, against the 2.9 µs separate read this branch
wrote. What this branch keeps is the second exit: `traverse_over` also returns
early with "no indexed traversal start", and that sentence is much weaker when
the file the symbol lives in was never read.

**3. A composition that spent the caller's depth on one direction.** `neighbors`
walked `max_depth` inbound and a hardcoded single hop outbound, so a composed
answer at depth 3 reported a three-level caller tree beside one hop of callees
with nothing saying which half was cut short. Measured at depth 2: 17 edges
against `trace`'s 35. It is the defect the `min_confidence` note in the same
function describes and closes, surviving in the parameter beside it — and
`neighbors_composition.rs` compares every field of both halves against the calls
they replace at depth 1, the one depth where the pinned value and the requested
one agree.

**4. The cold walk did not stop at the repository boundary.** `preview` refuses
a path that "resolves outside the indexed repository root" and
`classify_pending_entry` refuses a queued path that is not a regular file or
directory. `collect_sources_with_report` enforced neither: `WalkBuilder` never
*descends* through a symlink, but `Path::is_file` and `fs::read_to_string` both
follow one, so `src/creds.py -> <outside>/credentials.py` was read and its
symbols indexed — and `preview` then declined to show the symbols the build had
just written. Now refused and recorded as `DiscoverySkipReason::EscapesRoot`,
which `is_refusal` counts as coverage loss; a symlink whose target is inside the
repository is still read, because the walk reaches those bytes under their real
name anyway.

**5. One unreadable directory cost the whole map.** An unreadable *file* is a
recorded `Unreadable` refusal and the build describes everything else; an
unreadable *directory* propagated out of the walker and `devmap build` exited 1
with `node_count: 0`, "nothing has been indexed yet". The same `result?` sat in
`collect_go_modules`, which `devmap build` calls over the same root immediately
afterwards, so fixing only the discovery walk left the binary failing exactly as
before — caught because the fix was checked through the release binary and not
only through the unit test. An error naming a path below the root is now
recorded (discovery) or stepped over (go.mod enrichment, whose loss the
discovery pass over the same tree already records); an error naming the root, or
naming no path, stays fatal.

**6. The freshness inventory did not stop at the boundary either.** The second
shape of the disagreement `aecb670` closed for tagged cache directories, found
by attacking the pair rather than by re-reading the cache rule. `git ls-files`
lists a tracked symlink — it is an ordinary mode-120000 entry — and
`keep_indexable`'s closing `is_file()` follows it, so the inventory counted a
path discovery refuses and hashed bytes the map does not describe. Measured:
`devmap build` reported `files_indexed: 1, discovery_refused_files: 1` while
`devmap freshness` reported `files: 2` and its fingerprint moved from
`c2:496ee980…` to `c2:8bcb1a2b…` on an edit that touched only a file *outside*
the repository. Same cost as the cache case — the map reads stale on a change it
can never absorb. `escapes_root` is now public and both walks call it, and
`RepoMapper._escapes_root` transcribes it into `_keep` because
`freshness_parity` compares the two inventories file for file.

### A flaky test that was not a kernel defect

`a_fan_out_pays_for_its_index_once_per_direction` was reported failing at load
average ~350 on "the fixture must produce real callers", and passing in
isolation — which reads as a load-sensitive defect in `neighbors` or the
per-generation edge index. It is neither. Its `core.py` is 312 KB of 6,001
functions and `extract_file` gives a file `DEFAULT_PARSE_BUDGET`, five seconds,
after which **no symbols are claimed for it at all**. Measured in a debug build
on an idle machine: 2,882 ms; with a cargo build alongside: 5,018 ms, at which
point extraction returns `ParseOutcome::Failed { "extraction of 312038 bytes
exceeded the 5s budget … " }`, the store holds 9 edges instead of 12,018, and
`neighbors` correctly answers `Unavailable` for all sixteen targets. The fixture
sat at 58% of its budget with the variance to cross it. Both `chain_store`
fixtures now extract through `extract_treesitter_with_budget` with a budget no
machine can exhaust and assert `ParseOutcome::Clean` on their own inputs, so a
future refusal is reported as a refused fixture rather than as an assertion
about the query engine two hundred lines away.

### Left open, with the reason

* **Cold and incremental still disagree about an in-repository symlink.**
  `db.rs::classify_pending_entry` refuses *every* symlink — `symlink_metadata`
  gives neither `is_dir()` nor `is_file()`, so it lands on
  `Ok(_) => Err("not a regular file or directory")` — while
  `collect_sources_with_report` indexes one whose target is inside the tree. A
  cold build therefore holds `src/util.py -> shared/util.py` and the drain drops
  the row that would refresh it. Pre-existing, not touched by this pass, and the
  choice between the two rules is a product decision rather than a bug fix.
* **Resolver/liveness honesty (brief surface 5) was not attacked.**
  `Resolution::confidence` is already the single owner of the tier mapping with
  its own tests, so the marginal value of a second pass was lower than the
  surfaces above and the budget went there instead. Not "clear" — unexamined.
* **The 200-cycle `tools/soak.sh` run on a copy of `corpus_scholarlm` (brief
  surface 6) was not run.** The 50-kill convergence run and the 6-way concurrent
  build cover the crash and writer-contention halves; the RSS-plateau half is
  unmeasured.
* **A clap argument error is not one-line JSON.** `devmap preview src/x.py`
  (positional instead of `--file`) prints clap's usage block to stderr and exits
  2, whichever way `--json` is set. Every exit from the program's own code is
  one line of JSON; this one is before `main` runs. Recorded rather than
  changed, because exit 2 with usage on stderr is the conventional contract and
  a caller can already tell it apart.

### Verification

Taken at `94cf7af`, which is `main` plus this pass's commits; `main` moved twice
while the pass ran and was merged in each time, and only the last run is quoted.

| gate | result |
|---|---|
| `cargo fmt --all -- --check` | exit 0, no output |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo test --workspace --no-fail-fast` | **1,384 passed, 0 failed, 2 ignored**; zero `test result: FAILED` lines in the log |
| `pytest tests/unit -q -p no:cacheprovider` | **4,173 passed, 0 failed, 9 xfailed** in 465 s |
| `ruff check src tests` | `All checks passed!` |
| `cargo build --release -p devmap-cli` | exit 0; every attack above re-run against that binary |

The two ignored tests are the pre-existing pair
(`the_emitted_bundle_passes_claude_plugin_validate_strict`, which needs the
`claude` CLI on PATH, and `concurrent_prune_writer_child`, a child-process
helper).

Two of the six fixes were verified through the release binary and not only
through their unit tests, and one of those checks is why defect 5 is complete:
`collect_sources_with_report` was fixed, the test went green, and the binary
still exited 1 — because `collect_go_modules` carried the same `result?` and
`devmap build` calls it over the same root two lines later.

---

## Go: the compact graph has a reader (2026-09-05)

X39 closed the producer half of the interned encoding on 2026-09-05 and left the
row that mattered: **nothing read it**. `devmap manifest --compact-graph-output`
wrote a second encoding of the same model, `AGENT_PLAN.md` named
`backend/go_orchestrator/repomap` as its intended first consumer, and `repomap`
went on decoding the verbose artifact whole. This closes that: `repomap.Load`
now accepts either encoding and builds the identical `Map` from both.

### What was built

`backend/go_orchestrator/repomap/compact.go` — a decoder for the
`devmap-compact-v1` layout, wired into `Load` so **one** entry point reads both
wires. `Load` dispatches on the shape of each table, not on a flag, and both
converge on the same `graph` value before `build` sees it; `build` is unchanged.
Two decode paths that each produced a `Map` would be two implementations of the
map, and the failure mode would not be a crash — it would be a gate answering
"not a neighbour" on one wire and "neighbour" on the other with nothing to
notice which file it had read.

It decodes rather than reconstructs. Rust's `decode_compact` rebuilds the whole
verbose document because its contract is the round trip; `repomap` reads nine
string columns out of seventeen, so it fills `[]node` and `[]edge` directly and
steps over `extras`, `line`, `end_line`, `exported`, `language`, `name` and
`reason` without materialising them. That is where most of the difference in
cost comes from.

### The equivalence test

Both artifacts generated from one store by the final round-2 kernel, on a copy
of the 4,499-file scholarlm corpus (41,196 nodes / 271,240 edges, generation 1):

```
devmap manifest . --graph-output code_graph.json \
                  --compact-graph-output code_graph.compact.json
```

The two `Map` values are equal by `reflect.DeepEqual` over the whole struct —
`areaOf`, `areas`, `adjacent`, stats, provenance and the vocabulary record — and
separately by stats, provenance, the 287 areas, the neighbour list of every one
of those areas, and `Degraded()`. Committed as
`dc/devmap/interop_test.go::TestTheLiveCompactGraphBuildsTheSameMap`, which
drives the **real** binary rather than a fixture: every part of the layout is a
contract with a producer built from another workspace, and a hand-written
fixture asserting it would assert only that the fixture and the decoder were
written by the same hand. It also checks the producer's `encoding` string
against `repomap.CompactEncoding` and refuses to compare two documents when the
artifact interned nothing, so it cannot pass by comparing two verbose files. CI
already fails on a `--- SKIP` from any `TestTheLive*` in that package, so this
one is covered without a workflow change.

The hermetic half is `repomap/compact_test.go::TestTheInternedEncodingBuildsTheSameMap`.

### What the encoding bought

`BenchmarkLoadRealisticGraph`, darwin/arm64, Apple M5 Pro, `-benchtime 5x
-count=3`, on the package's existing 14,608-node / 74,061-edge fixture:

| | verbose | interned | |
|---|---|---|---|
| artifact | 13,481,813 B | 2,388,876 B | **−82.3%** |
| `Load` | 80.6–87.7 ms | 26.1–26.2 ms | **3.2x** |
| `Load` B/op | 59,384,083 | 24,840,873 | **2.4x less** |
| `Load` allocs/op | 371,488 | 17,237 | **21.6x fewer** |
| `decodeGraph` | 69.9–71.8 ms | 18.3–18.6 ms | **3.8x** |
| `decodeGraph` allocs/op | 369,385 | 15,134 | **24.4x fewer** |

Two honesty notes on that table. Every column of that fixture holds a string, so
the encoder interns all of them and the decoder skips nothing — it is the
encoding's best case. And the measurement that matters is the real artifact,
which has four raw columns per node to step over. On the scholarlm pair above,
`Load` best-of-5 with allocation counted by `runtime.MemStats`:

| | verbose | interned | |
|---|---|---|---|
| artifact | 85,001,360 B | 17,104,715 B | **−79.9%** |
| `Load` | 405.0 ms | 114.9 ms | **3.5x** |
| allocated | 242.6 MiB | 112.0 MiB | **2.2x less** |
| allocations | 1,291,942 | 79,699 | **16.2x fewer** |

**What it cost the verbose wire — the encoding every existing consumer reads —
is essentially nothing, and that took two attempts.** The first row reader used
`json.Decoder.Token` per cell and was *slower in allocations than the verbose
path it was beating in time*: 12.6 M allocations against 1.6 M on the scholarlm
artifact, because `Token` boxes every value and reading an index exactly
requires `UseNumber`, which turns each one into a heap string. Rows are now
scanned in place; nothing outside a row is hand-parsed. Separately, the shape
dispatch first decoded the verbose array element by element, and `&row` escaping
into the decoder's interface argument put every row on the heap twice: +90,811
allocations and +5.9 MB per load, paid by the wire that was not supposed to be
charged anything. `BenchmarkUnmarshalRealisticGraph` is kept as the baseline —
one `json.Unmarshal` of the whole document, which is what `Load` did before —
and against it the current verbose decode is **+3,009 B and +41 allocations per
op**, with the time inside the run-to-run spread.

### A defect the equivalence test found

`Stats.WidestArea` was decided by Go map iteration order. `build` walked
`adjacent` keeping the first area with a degree *strictly* greater than the best
so far, so on a tie — the ordinary case on an evenly coupled repository — the
name reported was whichever key the runtime handed over first, and one file
loaded twice in one process named two different areas. An operator reads that
field to decide whether the neighbour rule is worth relying on. Ties now break
by name. Red first:
`repomap/compact_test.go::TestTheWidestAreaIsTheSameAreaOnEveryLoad` failed on
the unmodified `build` at load 26 of 64.

### The adversarial pass

`repomap/compact_test.go::TestAnInternedGraphIsRefusedRatherThanShortened` and
its neighbours. Every case is a document that could have been read as a
*smaller* graph, which is what makes a gate answer "not a neighbour" with
confidence, so each is a refusal that names what it found: an unknown layout
identity (`ErrUnknownCompactLayout`, reported separately from a parse failure —
the remedies differ), a missing string table, an index past the end of the
table, a column declaring a storage kind that is neither `s` nor `j`, a field
list naming one column twice, a row whose cell count is not the declared width,
an interned cell holding a string, a truncated file, an interned layout carrying
no `encoding`, an `interned_tables` list that disagrees with the shape the rows
actually arrived in, and a table whose rows precede its field list. An interned
document with zero rows is refused with the same "holds no nodes" as an empty
verbose one, a `verbatim_tables` table inside a compact document is still read,
duplicate node ids resolve identically on both wires, and `MaxGraphBytes` bounds
both — the producer chooses which file a consumer is pointed at, so a bound that
held on one encoding would be no bound at all.

### Verification

`backend/go_orchestrator`, go1.26.4 darwin/arm64, `MANVI_MAP_BINARY` pointed at
the final round-2 kernel so the live tests ran rather than skipped:

| gate | result |
|---|---|
| `gofmt -l .` | no output |
| `go vet ./...` | exit 0 |
| `go test ./... -count=1` | all 7 packages `ok` |
| `go test -race ./repomap/... ./dc/devmap/...` | both `ok` |
| `go test ./dc/devmap/ -run TestTheLive -v` | 4 PASS, 0 SKIP |

### `repomap` and `dc/devmap` still have no caller in this repository — and must not be deleted

The register asks whether the two packages should go. Both proofs the contract
requires say there is no caller **here**, and a third piece of evidence says
deleting them would be a mistake.

1. `RIPGREP_CONFIG_PATH= rg -uu` over the whole worktree (excluding only
   `.git/`, the generated `.devcouncil/`, cargo `target*/` and `*.sqlite`):
   no `package main` anywhere under `backend/go_orchestrator`, and the only
   importer of either package is `dc/devmap/interop_test.go`. Nothing in
   `src/`, `tests/` or `.github/` names them except the workflow's path filter.
2. In a scratch copy with `repomap` removed, `go build ./...` exits 0 and
   `go vet ./...` fails only on that one test file; with `dc/devmap` removed too,
   both exit 0.

**Why they exist.** MANVI (`module manvi`) holds its own copies at
`manvi/repomap` and `manvi/dc/devmap`, and there they have five production call
sites — `repomap.LoadIfPresent` in `cmd/manvi/main.go` (four) and
`cmd/manvi/tui.go`, `Subsystems *repomap.Map` and `Map *devmap.Client` in
`devcouncil/tools.go`, `devmap.New` at `cmd/manvi/main.go:427`. MANVI does not
import this module; it vendors by copy, which is the arrangement `rust/README.md`
§Consumers and `rust/STATUS.md` §1 describe for the whole analysis plane. The
copy here is the **upstream**: 809 lines against MANVI's 584, and it alone
carries `MaxGraphBytes`, `SchemaDeclared`, `DistinctOrphanEndpoints` and now the
interned decoder — every one of which is a defect fixed here and not yet carried
across (MANVI's copy last moved 2026-08-31). Deleting it would destroy the
upstream of a package with live callers and freeze those callers at a version
with no bound on the artifact it reads.

So the answer is **(b) with the deletion refused**: the two proofs establish "no
caller in this repository", which is not the same claim as "no caller", and the
difference is exactly what the two-signal rule exists to catch.

**A `package main` was considered and not written.** It would serve nobody now:
MANVI links the package rather than spawning a binary, and DevCouncil's Python
already owns the same three questions in `src/devcouncil/indexing/subsystem_map.py`
(`area_for_path`, `are_neighbors`, `neighbors_for_area`), read by
`execution/policy_engine.py`, `verification/checks/subsystem_boundary.py`,
`execution/prompt_builder.py` and `integrations/mcp/handlers/map.py`. Spawning a
Go binary from Python would put a second owner of the neighbour rule beside the
Python one. What that Python owner needs is not a Go CLI — see the next section.

### Left open, for the Python and Rust lanes: the neighbour rule is answering from a field the kernel stubs

Found while looking for a consumer, and it is a live defect rather than a
tidiness note. `devmap-query/src/manifest.rs:302` emits `"neighbors": []` as a
literal for every subsystem. `subsystem_map.are_neighbors` reads exactly that
field. So:

- `execution/policy_engine.py:628` allows a write into a *neighbouring*
  subsystem of a planned file — a rung that can now never fire. Every unplanned
  cross-subsystem write falls through to `deny`.
- `verification/checks/subsystem_boundary.py` raises an `architecture_drift`
  gap for every cross-area change, because `cross_boundary_pairs` reads "no
  declared neighbours" as "not adjacent".

Measured: this repository's `.devcouncil/repo_map.json` has 16 subsystems, 0
with a non-empty `neighbors`; the scholarlm map written by the final kernel has
12 subsystems, 0 with a non-empty `neighbors`. The Python map writer was retired
2026-09-02, so the kernel is the only writer and the field is always empty.

This is reason 1 in `repomap`'s own package doc, live in the Python half: *a
consumer reading the field cannot tell "this repository has no adjacent
subsystems" from "this producer does not compute the field"*, and the first is a
decision while the second is a missing feature. The Go package sidesteps it by
deriving adjacency from cross-area `extracted` edges. The fix belongs to one of
two owners and not to this lane:

- **Rust** — `manifest.rs` `build_repo_map_value` (the literal at line 302):
  compute `neighbors` from cross-area edges the way `repomap.build` does, or
- **Python** — `indexing/subsystem_map.py::neighbors_for_area`: derive adjacency
  from the code graph instead of reading the stubbed field, and until then have
  `are_neighbors` report *unknown* rather than *not adjacent* so the deny reason
  does not claim something the map never established.


## Symlink rule, soak plateau, resolver honesty (2026-09-05)

Four items on the `claude/devmap-open-items` line: the one symlink rule, the
soak's memory-plateau half, an adversarial pass over the resolver/liveness
honesty surface, and — handed to this lane mid-pass by the Go lane — the
`subsystems[].neighbors` literal the policy gate reads.

### 1. One symlink rule for the cold walk and the drain — **closed**

The two halves of the kernel decided independently what a symlink means and
reached opposite answers. `collect_sources_with_report` kept a symlinked file
whose target resolves inside the repository (a monorepo's shared config, a
vendored header: the bytes are the repository's either way) and refused only one
that escapes it, recording `DiscoverySkipReason::EscapesRoot` as coverage loss.
`classify_pending_entry` refused **every** symlink as "not a regular file or
directory". Measured against the pre-fix tree:

```text
# src/util.py -> shared/util.py, both inside the repository
devmap build .                 -> both indexed (util_v1)
<edit shared/util.py>
reconcile_pending_paths(root)  -> dropped: [("src/util.py",
                                     "not a regular file or directory")]
drain                          -> src/util.py still says util_v1

# src/creds.py replaced by a link out of the repository
drain                          -> Err: all 1 claimed path(s) failed:
                                    watched file resolves outside daemon root
```

The queued edit was deleted as structurally unprocessable, the stored extraction
stayed at whatever the cold build had read, and `status` reported fresh.

**Decision applied: the cold walk's rule wins**, stated once in
`devmap_extract::candidate_kind` (`devmap-extract/src/lib.rs`). Both callers ask
it; neither re-derives it.

| Shape | Cold walk | Drain |
|---|---|---|
| symlink -> file inside the root | indexed under the link's name | ordinary work |
| symlink -> outside the root | `EscapesRoot`, counted as coverage loss | rows removed as a full build would, refusal charged |
| symlink with no target (ENOENT) | `EscapesRoot` | same |
| symlink that will not resolve (ELOOP, EACCES parent, stale mount) | `Unreadable`, a refusal | **retried**, never read as absence |
| symlink -> directory inside the root | not descended | refused; its files are queued under their own names |

Three defects the one owner exposed, each with a red test watched failing:

1. `is_file()` follows a link and answers `false` for a dangling link, a loop and
   a directory link alike, so a `src/x.py -> /nowhere` was not indexed, not
   refused and not mentioned. Now recorded.
   (`discovery_stays_inside_the_root.rs::a_dangling_symlink_is_recorded_as_a_refusal_rather_than_passed_over_in_silence`,
   pre-fix: "a link whose target will not resolve is coverage loss, not
   silence: []"; and `::a_symlink_loop_is_recorded_as_a_refusal`.)
2. `ignore::DirEntry::metadata()` reports the *link's* size, so a file over
   `MAX_SOURCE_BYTES` reached through an in-root symlink walked straight through
   the ceiling that bounds the walk's memory. `CandidateKind::File` now carries
   the size of the bytes that will actually be read.
   (`::a_file_over_the_ceiling_reached_through_an_in_root_symlink_is_still_refused_by_size`.)
3. The drain met an escaping link with `bail!`, charging retries toward
   quarantine while the rows a full build had stopped writing stayed in the
   generation. It now reconciles it as a deletion and charges the refusal to
   coverage. `DiscoverySkipReason::is_containment_refusal` draws the line
   between a refusal about *this attempt* (`Oversized`, `Unreadable` — the file
   may shrink or regain `+r`, so its last good extraction is kept) and one about
   the path itself (`EscapesRoot` — a full build writes no rows for it), and the
   directory branch of the drain reads the same predicate.

The carried-forward `discovery_refused_files` on the incremental drain is now
read as a **floor** (`previous.max(this batch's refusals)`): a drain turned away
from a file cannot report the corpus fully walked because the last full walk
happened to refuse nothing. `max`, not a sum — the carried number is a
whole-tree measurement and the batch's is a handful of paths that may already be
inside it.

One invariant was deliberately preserved rather than overridden. A link whose
`canonicalize` fails is split by errno: ENOENT is a fact about the link (a
refusal, rows removed), everything else is a question that could not be answered
(`Undecidable`, retried). Collapsing the two would have made
`daemon::tests::a_path_whose_existence_is_undecidable_is_not_reported_deleted`
fail, and that test is right: a wrongly-kept row is stale, a wrongly-dropped one
is a symbol the dead-code pass is free to call unreferenced.

Red tests, all watched failing against the pre-fix tree:
`devmap-store/tests/one_symlink_rule.rs` (4 of 5; the ordinary-paths control was
green throughout), `devmap-serve/tests/one_symlink_rule_end_to_end.rs` (2 of 3),
`devmap-extract/tests/discovery_stays_inside_the_root.rs` (3 new of 7).
Commit `e466c29`.

### 2. The soak's memory-plateau half — **closed, with one defect in the harness itself**

`tools/soak.sh` read `.devcouncil/codeintel/index.sqlite` — the **Python**
engine's store. The Rust kernel writes `devmap.sqlite`. So for this script's
whole life `digest()` returned the empty string and `db_bytes()` returned 0 on
every cycle: the digest comparison held trivially (`"" == ""`) and the growth
limit compared 0 against `0 * 3 + 1048576`. A check that could not run reported
exactly what a check that ran and passed reports, in the one script whose entire
job is to notice drift. Every "SOAK OK" this repository has recorded was that.

The harness now: reads the kernel's store and **fails** if the baseline digest
is empty; counts the main database *plus* its WAL, because a WAL that never
checkpoints is exactly the unbounded growth this looks for; honours
`CARGO_TARGET_DIR`/`DEVMAP_BIN` so a lane cannot silently measure a stale
binary; samples `(cycle, RSS, store bytes)` into a CSV every cycle; and asserts
a plateau by comparing the means of two halves taken after a warm-up, because
the first cycles are still filling the extraction and page caches and calling
that growth a leak would fail every healthy kernel. A run under 40 cycles is
reported as a smoke test — digest asserted, growth not — in a verdict line
distinct from the plateau's; how the warm-up is found is in 2b below.

A second mode, `--daemon`, drives a long-lived `devmap serve` through the same
edit cycles with `status`/`search`/`impact` over IPC — the process an agent host
keeps open, and the only one where a leak accumulates across hours. Two further
harness defects were found by running it: the socket was named beside the corpus,
where a scratch path is routinely longer than `sun_path`'s 104 bytes (the daemon
then never binds and the failure says nothing about why), and a failed start
killed the `/usr/bin/time` wrapper while leaving the daemon serving — one such
orphan ran for five minutes beside a build-mode soak **on the same corpus**, so
every number that soak produced described two writers rather than one. Both are
fixed; the daemon's pid is now found through its own socket and released by a
trap on every exit path.

**The runs.** Both on the settled 4,499-file Go/TS/Py corpus (41,276 nodes /
271,543 edges) — the daemon half on a copy of the 15,080-node corpus, because a
long-lived process there sees the same shape at a size that fits the session —
against one release binary saved aside for the whole run (`DEVMAP_BIN`), so a
rebuild could not change the thing being measured halfway through.

`devmap build`, 200 cycles (edit -> build -> query -> restore -> build), RSS is
the build process's peak per cycle:

| cycle | peak RSS | store bytes |
|---|---|---|
| 1 | 1,592.3 MB | 1,292.9 MB |
| 20 | 1,598.8 MB | 1,292.8 MB |
| 50 | 1,603.0 MB | 1,292.8 MB |
| 100 | 1,596.8 MB | 1,292.8 MB |
| 150 | 1,601.7 MB | 1,292.8 MB |
| 200 | 1,595.6 MB | 1,292.9 MB |

```text
plateau ok: rss mean(51-125) 1597314389 -> mean(126-200) 1597455291 (limit 1757045827)
plateau ok: db  mean(51-125) 1292865426 -> mean(126-200) 1292879189 (limit 1422151968)
SOAK OK (200 cycles, digest stable, growth bounded)
```

RSS moved **+0.01%** and the store **+0.001%** between the two halves; every
sample after cycle 50 sat between 1,591.6 MB and 1,604.0 MB, a ±0.4% band. The
edge digest was identical on all 200 restored builds.

`devmap serve`, 200 edit-and-query cycles, RSS sampled per cycle with `ps` and
the process's peak read from `/usr/bin/time` at exit:

| cycle | resident RSS | store bytes |
|---|---|---|
| 1 | 239.6 MB | 247.3 MB |
| 20 | 239.7 MB | 247.3 MB |
| 50 | 743.4 MB | 497.9 MB |
| 100 | 853.9 MB | 497.9 MB |
| 150 | 892.2 MB | 497.9 MB |
| 200 | 906.2 MB | 497.9 MB |

```text
plateau ok: rss mean(51-125) 900551365 -> mean(126-200) 844318201 (limit 990606501)
plateau ok: db  mean(51-125) 497900216 -> mean(126-200) 497900216 (limit 547690237)
```

**Tolerance: 10%, chosen from these numbers.** The daemon's second-half mean is
**6.24% below** its first-half mean and the store is flat to the byte from cycle
50 on; the build half moves +0.01%. Ten percent is four times the largest
observed drift and an order of magnitude under what growth looks like — the
daemon's own warm-up quarter moved **+210%**.

**Cycle 20 is not a warm baseline, and that is a finding rather than a
convenience.** The daemon's first full drain lands between cycles 20 and 50 and
takes RSS from 240 MB to ~900 MB and the store from 247 MB to 498 MB: the
process loading what it is for. Asserting cycle 200 against cycle 20 would fail
on every healthy kernel, which is why the harness excludes a warm-up — for the
daemon, one that ends where the samples show the first drain landing (2b) —
and compares the two halves of the remainder by mean. The build half needs no such
allowance — its store reaches its steady size inside cycle 1 — and passes the
same rule at +0.01%.

**No leak was found, so none was fixed.** The edge cache, adjacency index,
extract cache, WAL and generation retention all hold: the store is byte-stable
from cycle 50 to cycle 200 on both runs, which is what a WAL that checkpoints
and a retention policy that prunes look like. The daemon's per-cycle RSS swings
±12% around its mean (755-1,062 MB) with no trend — allocator behaviour, not
accumulation.

### 3. Resolver / liveness honesty — **one defect, two surviving surfaces**

Nine shapes where a resolver is tempted to claim more than it knows — a bare
name a same-named instance method could absorb, an import shadowing a builtin,
one class declared twice in a package, a re-export chain whose middle file
declares nothing, a decorator-registered route, a receiver typed two statements
above the call and then reassigned, `self.method()` declared only by a parent, a
Go interface method against a concrete method of the same name, and a TypeScript
default export renamed at the import — held to four invariants over the whole
corpus rather than shape by shape
(`devmap-resolve/tests/resolution_honesty.rs`):

1. every emitted edge's confidence is exactly what its `Resolution` entitles it
   to, re-derived in the test rather than obtained from the owner;
2. no `DETERMINISTIC` edge without deterministic evidence;
3. ambiguity never picks a winner — the fan-out at `SPECULATIVE` or no edge,
   never one confident edge among equals;
4. a deterministic edge points where its evidence points.

**The defect, found by invariant 4.** The Go package star edge (G20) ran from
`geo/measure.go` to the synthetic node `package:geo/geo` carrying
`Resolution::SameFile`, whose documented meaning is "the declaration is in this
very file" — the only shape in the emitted graph where `resolution` and the edge
disagreed about where the target lives. Split into `Resolution::Structural`: a
relation the graph asserts about its own shape, deterministic for the reason it
always was (a Go package clause is written at the top of the file), now saying
so. One construction site; `confidence()` and `target()` extended; the two
exhaustive matches in `audit_regressions.rs` extended rather than widened.

**Surviving surfaces, recorded as results.**

- Liveness never reported a symbol dead at `extracted` confidence — the tier
  `CLAUDE.md` tells agents to act on by deleting code — while any non-structural
  edge reached it, across all nine shapes. With the off-direction control that a
  symbol nothing reaches is still found, so the rule is not vacuous.
  (`devmap-analyze/tests/liveness_honesty_over_the_hard_shapes.rs`.)
- `devmap-query::resolved_edge_from_stored` round-trips every edge of the same
  corpus at the confidence the resolver gave it, and the ambiguous fan-out does
  not gain confidence by passing through SQLite's REAL column.
  (`devmap-query/tests/stored_edges_keep_the_confidence_their_evidence_earned.rs`.)
  **What that file cannot assert, and says so rather than implying otherwise:** a
  re-read edge carries no evidence, because no `resolution` column is persisted,
  so the invariant on the read path rests on the round trip plus the write-side
  constructor — never on a second independent reading of the same fact.

Commit `85c00f8`.

### 4. `subsystems[].neighbors` was a literal — **closed** (handed over mid-pass)

`build_repo_map_value` emitted `"neighbors": []` for every subsystem, for the
field's whole life in this kernel. `subsystem_map.are_neighbors` reads exactly
that field, so `policy_engine.py`'s "File is in a neighboring subsystem of a
planned file" rung could never fire and `verification/checks/subsystem_boundary.py`
raised `architecture_drift` for every cross-area change.
`backend/go_orchestrator/repomap` had already given up on the field and derives
adjacency itself, saying so in its package doc. Measured before: this
repository's `.devcouncil/repo_map.json` had 16 subsystems and 0 with a
non-empty `neighbors`; the scholarlm map, 12 and 0.

Derived exactly as the Go reader derives it, so the two cannot disagree: two
areas are neighbours when a `calls`/`references`/`imports` edge at `extracted`
confidence runs between them, symmetrically. Ambiguous edges are excluded — this
relation *widens* what a task may write, and a scope decision may not rest on a
resolution the analyser declined to make. Areas are resolved by
`subsystem_map.area_for_path`'s own rule (longest declared subsystem prefix,
else the file's parent directory), because a relation keyed by a vocabulary the
lookup does not use is an empty relation with extra steps — the
`community-4`-against-a-directory-path defect this field already had once.

A second defect fell out of running it on a real store: not every endpoint is a
file. The resolver emits one synthetic node per Go package and points the
package's imports at it, so `backend/go_orchestrator/dc/dcgrep` named both
`backend/go_orchestrator/internal/proc` and
`package:backend/go_orchestrator/internal/proc` — the same coupling twice, once
in a vocabulary no consumer can match, spending a slot of the cap to say it.
Resolved through the graph rather than by knowing how the node is spelled: every
file of a package carries a `MemberOf` edge to its package node, so the node's
area is the area of any member. Dropping the endpoint would also have removed
the synthetic name and would have been *worse* than the duplicate, because on a
pure-Go pair the only edge recording the import points at the package node; the
test asserts the coupling survives, not just that the name is gone.

`liveness_meta.subsystems` now carries `neighbors_computed`, `neighbors_shown`,
`neighbors_total`, `neighbors_truncated` and `neighbors_endpoints_unresolved`,
so a consumer can tell a computed empty list from a producer that stubs the
field — which is reason 1 in `repomap`'s package doc, answered.

Verified with the release binary against the settled 15,080-node corpus:

| | before | after |
|---|---|---|
| subsystems | 16 | 16 |
| with a non-empty `neighbors` | 0 | 12 |
| synthetic `package:` names | — | 0 |
| `neighbors_shown` / `neighbors_total` | — | 155 / 170 |
| `neighbors_endpoints_unresolved` | — | 0 |

Commits `696e1b8` and `4522f8e`.

### Out of band: a pre-existing red in `devmap-query`

`engine::search_bounds_tests::a_hit_near_the_top_of_a_huge_file_reads_a_bounded_prefix`
was failing at HEAD `385baa0`, before any change in this pass — verified by
restoring the four source files this lane had touched and re-running. It
extracted its 50 MB fixture through `extract_file`, and `extract_treesitter`
shares one `DEFAULT_PARSE_BUDGET` (5 s) between the parse and the walk, so in a
debug build the budget is overrun whenever the machine is busy, the generation
holds no symbol, and the search finds nothing — reported as "the fixture must
produce exactly one hit", which points at the read bound rather than at the
clock. Green as the only test in the filter, red with either of its two module
siblings and red under `--test-threads=1`: neither ordering nor parallelism, but
wall time.

The generation is now extracted from the head — a prefix of the file, so the
recorded span is identical — while `huge.py` on disk stays 50 MB, which is the
only half `hit_from_stored` reads and the only half the bound is about. The
fixture premise is asserted rather than assumed. The `devmap-query` lib suite
drops from 12.2 s to 0.8 s as a side effect. Commit `ec31350`.

### Verification

```text
$ cargo fmt --all -- --check
(no output)

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.80s

$ cargo test --workspace --no-fail-fast          # logged, `test result: FAILED` counted
exit=0
"test result: FAILED" occurrences: 0
passed=1410 failed=0 ignored=2

$ cargo check -p devmap-{extract,query,store,analyze} --no-default-features --all-targets
devmap-extract   Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.45s
devmap-query     Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.05s
devmap-store     Finished `dev` profile [unoptimized + debuginfo] target(s) in 3.58s
devmap-analyze   Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.29s

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 58.87s
```

`devmap manifest` on the 15,080-node corpus, five forced rewrites each, with the
soak running beside both: p50 0.90 s before the neighbour derivation and 0.90 s
after — no measurable cost.

**Left undone by this lane, precisely — closed by the lead the same evening,
see 2b.** `tools/soak.sh` reports "did not
plateau" when a run is too short for its warm-up quarter to cover the
working-set load — a 40-cycle daemon run on the 15,080-node corpus does exactly
that, because the first full drain lands at cycle ~33, inside the compared
window. The reading is not wrong but the *message* names the wrong cause. The
fix is a step guard before the comparison, and here are the numbers to set its
threshold from: the largest cycle-to-cycle ratio inside the compared range is
x1.05 (daemon) and x1.00 (build) once warm, against x1.57 and x2.01 during
warm-up, so x1.25 separates them cleanly. On a step, refuse the assertion with
"the working set was still loading at cycle N" and exit non-zero rather than
reporting a leak. This lane did not apply it because the script was executing
throughout the window in which it could have been; whoever does owns the change
in `tools/soak.sh`'s plateau block.

### 2b. The soak's warm-up is read from the samples, not assumed (lead, 2026-09-05)

**Found, running the committed harness at its own default.** `tools/soak.sh <copy
of this repository's corpus> 40 --daemon` with the final release binary
(`devmap 0.1.0`, store schema 13):

```
soak baseline: mode=--daemon digest=216e37ba3170 db=123252736 tolerance=10%
  cycle 30 ok rss=308592640 db=123256888
  cycle 40 ok rss=654868480 db=497552080
SOAK FAIL: rss did not plateau — mean(11-25) 250298368 -> mean(26-40) 462048460 (limit 275328204)
SOAK FAIL: db did not plateau — mean(11-25) 123252736 -> mean(26-40) 264666023 (limit 135578009)
```

A cycle on this corpus takes about half a second, and the daemon's first drain
is a wall-clock event — the watcher holds a burst for its 2 s quiet window and
at most 10 s (`MAX_DEBOUNCE_HOLD`) — so it landed at cycle 30, inside the
compared window, and the process loading its graph was reported as a leak. On
the 15,080-node corpus the same drain lands at cycle 31–33 of a run whose
cycles take 4–5 s; a fixed "first quarter" was calibrated to that run and
nothing else. The lane's own "Left undone" paragraph above names this.

A second defect in the same block: a run under 40 cycles printed
`SOAK OK (… growth bounded)` — the very line a plateau run prints — for a
check that had not run. `verify.sh` step 8 runs five cycles through that path.

**Fix.** The plateau block is now `assert_plateau` (`tools/soak.sh`, between
the `plateau begin` / `plateau end` markers). In daemon mode the warm-up ends
at the first sampled cycle whose store bytes differ from the baseline — the
first generation the daemon wrote — plus a quarter of what remains, for the
generations and index that follow it; build mode keeps the first quarter,
which its flat-from-cycle-1 data supports. Fewer than 30 cycles after warm-up
refuses the comparison with `run more cycles`; a daemon that never wrote a
generation refuses with `the plateau check could not run`; both exit 1. Under
40 cycles the verdict is a different word, `SOAK SMOKE OK (… growth NOT
asserted)`, and `verify.sh` now greps for that one. A ratio-based step guard
(the x1.25 the lane measured) was not used: it detects that a step happened
but not where warm-up ends, and the store-bytes column answers that directly
without a threshold.

**Evidence.** The shipped function, sourced from between its markers and run
over recorded and synthetic curves:

| curve | verdict |
|---|---|
| lane's daemon run, 200 cycles (`final_daemon.csv`) | first drain at cycle 31, warm-up ends 73; `rss mean(74-136) 902 MB -> mean(137-200) 842 MB`, store flat — ok |
| lane's build run, 198 cycles (`final_build2.csv`) | `rss 1597.46 MB -> 1597.38 MB`, store +0.001% — ok |
| lead's 40-cycle daemon run above | `9 cycles after warm-up (which ends at cycle 31 of 40) is fewer than the 30 … — run more cycles` |
| the same run cut at cycle 29 | `the daemon never wrote a generation in 29 cycles, so the plateau check could not run` |
| synthetic +1 MB/cycle leak, 200 cycles | `rss did not plateau — mean(55-127) 191000000 -> mean(128-200) 264000000` |

End to end, fresh corpus copies, the final binary, the shipped script:

| run | verdict |
|---|---|
| build, 5 cycles | `SOAK SMOKE OK (5 cycles, digest stable; growth NOT asserted — under the 40-cycle minimum)`, exit 0 |
| daemon, 40 cycles | first drain at 29; `SOAK FAIL: 9 cycles after warm-up … run more cycles`, exit 1 |
| daemon, 600 cycles | first drain at 30, warm-up ends 172; `rss mean(173-386) 853 MB -> mean(387-600) 897 MB`, `db 493 MB -> 497 MB`, peak 1.03 GB; `SOAK OK (600 cycles, digest stable, growth bounded)`, exit 0. A second 600-cycle run on another copy, sampled by the previous block and re-read through this one: `rss 851 MB -> 869 MB`, `db 492 MB -> 494 MB` — two runs, ±3% of each other, no trend |

The two 600-cycle runs are ~30 drains each on this repository's corpus; with
the lane's 200 cycles on the 15,080-node corpus (about 45 drains) that is the
final kernel holding a plateau in daemon mode on both corpora. **No leak — none
fixed**; what changed is that the harness now says which of "leak", "still
loading" and "not measured" it saw.

## `subsystems[].handoff_paths` was a literal too (2026-09-05)

The second half of the defect [§4 above](#4-subsystemsneighbors-was-a-literal--closed-handed-over-mid-pass)
closed: `build_repo_map_value` emitted `"handoff_paths": []` for every subsystem,
for the field's whole life in this kernel, so no reader could tell "this
subsystem hands off to nothing" from "this producer does not compute this
field". The neighbour fix left it untouched two lines below the field it fixed.

This field is read further out than `neighbors` was. `indexing/map_artifacts.py`
writes step 6 of the *generated agent guide* — the `CLAUDE.md` / `AGENTS.md` that
`dev map` puts in every workspace — as "Use `neighbors` and `handoff_paths` in
`subsystems` to follow cross-subsystem flow". So the instruction sent every agent
to a stubbed field, and the stub answered "there is no flow here". Four more
readers repeated it: `execution/prompt_builder.py` (the "Cross-subsystem flow"
line in a coding agent's prompt), `knowledge/wiki.py` (the "Handoff paths"
section, omitted entirely when empty), `integrations/mcp/handlers/map.py` (the
subsystem detail row), and `indexing/map_viz.py` (the graph HTML detail pane,
which printed `(none)`).

Measured before, on this repository's `.devcouncil/repo_map.json`: 16 subsystems,
**0** with a non-empty `handoff_paths`, while 12 already had non-empty
`neighbors` — the two fields disagreeing in the artifact itself.

**Derived from the same sweep as `neighbors`, not a second one beside it.**
`area_adjacency` already walked the generation's `calls`/`references`/`imports`
edges at `extracted` confidence and resolved both endpoints to areas; it now
returns an `AreaCoupling` carrying both relations. `neighbors` is that relation
made symmetric and projected onto area names; `handoff_paths` is the same
relation kept **directed** and kept at file granularity — the ordered pair
`source -> target`, recorded only on the area the edge *leaves*. Recording it on
both ends would tell a reader that the called subsystem calls back. One sweep
means the two fields cannot drift apart, and a test asserts the invariant
directly: every handoff starts inside its own area and lands on an area that
area lists as a neighbour.

The synthetic-Go-package trap from §4 applies here in a sharper form: a handoff
must name a *file a consumer can open*, and both Python validators split on `->`
and assert each half is an indexed path. `endpoint_file` resolves a
`package:<dir>/<pkg>` node through the same `MemberOf` edge `endpoint_area` uses,
so an endpoint can never contribute an area without also contributing the path
that justifies it. On the real corpus this is what turns
`backend/go_orchestrator/dc/dcgrep/dcgrep.go -> package:...` into
`… -> backend/go_orchestrator/internal/proc/proc.go`.

Capped at 16 per subsystem — half the neighbour cap, because each entry costs
about four times as much (two full repository paths and an arrow, against one
area name) and this artifact is written compact precisely because it is the file
an agent opens before searching. `liveness_meta.subsystems` now carries
`handoff_paths_shown` / `handoff_paths_total` / `handoff_paths_truncated`, and
`meta.devmap_rust.handoff_paths_computed` marks the provenance.

**The counts are a separate question from the neighbour counts, deliberately.**
An area with two neighbours can reach them through fifty file pairs, so
`neighbors_truncated: false` says nothing about whether the handoff lists are
whole; `_handoff_paths_answer_is_partial` reads the handoff counters and only
shares `neighbors_endpoints_unresolved`, which is genuinely one sweep's rejects.
On this repository the two answer differently on the first run — neighbours are
complete (67/67) while handoffs are capped (145/508) — so a shared flag would
have been wrong immediately.

Verified with the release binary against this repository's store
(1,417 files, 16,559 symbols, 88,479 edges):

| | before | after |
|---|---|---|
| subsystems | 16 | 16 |
| with a non-empty `handoff_paths` | 0 | 12 |
| synthetic `package:` endpoints | — | 0 |
| `handoff_paths_shown` / `handoff_paths_total` | — | 145 / 508 |
| `handoff_paths_truncated` | — | `true` (reported, not hidden) |
| `meta.devmap_rust.handoff_paths_computed` | absent | `true` |
| `repo_map.json` | 336,790 B | 349,490 B (+3.8%) |

`devmap manifest` on that store, 11 runs each, release binary, no other load:

| | p50 | full sorted sample (s) |
|---|---|---|
| before | **0.17 s** | 0.13 0.13 0.15 0.16 0.17 **0.17** 0.18 0.19 0.21 0.22 1.09 |
| after | **0.17 s** | 0.13 0.13 0.13 0.13 0.13 **0.17** 0.17 0.17 0.23 0.30 2.23 |

No measurable cost, the same result the neighbour derivation got. **These
conditions are not the same as §4's**, which reported 0.90 s → 0.90 s on the
15,080-node corpus *with the soak running beside it*; this pass measured a clean
machine, so the absolute numbers are not comparable across the two entries — only
the before/after within each. The high tail sample in each column is the first
run against a cold page cache.

**Honesty surface, Python side.** `indexing/subsystem_map.py` gains
`handoff_paths_established`, `can_rule_out_handoffs` and the tri-state
`handoff_paths_for_area` (a list, or `None` for unknown), mirroring
`neighbors_established` / `can_rule_out_adjacency` / `are_neighbors` rule for
rule: a *positive* is definite whatever the map omits; only the empty list has to
prove the producer was in a position to give it. Every reader now answers
honestly when the marker is absent — the guide's step 6 says the field was not
computed and stops pointing at it, the prompt builder and the wiki say so
explicitly instead of omitting a section, the MCP detail carries
`handoff_paths_computed` beside `neighbors_computed`, and the graph HTML prints
`(not computed for this map)` rather than `(none)`.

Two supporting fixes fell out of wiring that up:

- **`RepoMap` was silently dropping `meta`.** The model declared
  `liveness_meta` but not `meta`, so pydantic validation discarded the
  `devmap_rust` block — every consumer reading the map *through the model* (the
  wiki builder and the agent-guide writer among them) lost the only positive
  evidence the marker provides and could only guess from the lists. Declared.
- **One adapter, not two copies.** `repo_mapper.handoffs_computed(repo_map)` is
  the single bridge from the model to the dependency-free reader; the wiki and
  the guide both call it, because a second copy is how the guide comes to promise
  a field the wiki knows is absent.

**Tests.** `crates/devmap-query/tests/subsystem_handoff_paths_are_computed.rs`
(7 tests, gated on the `parse` feature like its neighbour sibling) — 3 fail
against the pre-fix literal for the right reasons (empty list where a pair was
derived, empty list on the Go fixture, absent provenance marker) and the other 4
are the guards that only become meaningful once the field is populated.
`tests/unit/test_subsystem_map.py` gains 8 reader tests covering the tri-state,
the cap, the separate-counters rule, and malformed input. The strict-`xfail`
`test_repo_map_subsystem_roles_handoffs_and_file_kinds` still xfails — on
`role_files` and file kinds, which this pass does not touch — and its reason
string was corrected, because it claimed the kernel "writes no handoff_paths".

**Left undone by this lane, precisely.** `role_files` is still `{}` for every
subsystem and every file is still classified `kind=code`, so step 5 of the same
generated guide points at a field with the same defect this one just closed —
the third instance of the shape, and the one that keeps that test xfailing.
`summary` is likewise `""` for every subsystem. None of these were derived here.

## `role_files` and `files[].kind` were constants too (2026-09-05)

The third and fourth instances of the shape closed in the two sections above,
and the last of the four navigation fields the *generated agent guide* points
at. `dev map` writes that guide into every workspace it maps, so each constant
was an instruction pointing at a question nobody had measured:

- **`role_files` was `{}` for every subsystem.** Step 5 of the guide reads "Use
  `role_files` in `subsystems` for subsystem role buckets". Four readers took it
  at its word and got nothing — `knowledge/wiki.py`, `indexing/map_viz.py`,
  `integrations/mcp/handlers/map.py`, and, functionally,
  `verification/test_resolver.py`, which resolves a subsystem's tests through
  `role_files["tests"]`. That last one is not cosmetic: a change with no direct
  test importer had **no fallback at all**, and the emptiness read as "this
  subsystem has no tests" rather than "nothing computed this".
- **`files[].kind` was the literal `"code"`.** Measured before: 1,417 of 1,417
  entries, including 108 markdown files, 172 JSON files, and `README.md` — on a
  record that already carried `language: markdown`. A constant is not a
  classification, and `wiki.py` renders this field per file.

**The vocabulary is the retired Python writer's, deliberately.** `role_files`
uses `_GENERIC_ROLE_RULES` (removed in d232dea) — tests, migrations, entry, api,
models, services, config, docs — in its original order, most-specific first,
first match wins, so `routes/user_test.py` is a test rather than an `api` file.
Consumers were written against those names; a second vocabulary here would make
the field's history unreadable for no gain. The *curated* half of that writer
(`_SUBSYSTEM_ROLE_FILES`, a hand-written table keyed on DevCouncil's own paths)
was **not** ported: its own docstring records that it produced `{}` for all 10
subsystems of a 4,082-file polyglot repo, which is the defect, not the fix.

**Two deliberate departures from the ancestor, both measured.**

1. *`other` is always emitted when anything is left over.* The Python version
   returned `{}` when no rule matched, which reintroduces exactly the ambiguity
   being removed. An area the map emits has files under it by construction, so
   it always has an answer, and "these play no role I recognise" is one. This is
   what makes "every subsystem has a non-empty bucket" true rather than lucky.
2. *A test/doc directory counts at any path depth, not just the top level.* The
   ancestor checked only the first segment, which was sound when the Python
   engine mapped `src/devcouncil` alone. This kernel maps the whole repository.
   Measured on the corpus: of 691 files now classified `test`, the top-only rule
   would have caught **363** and missed **290** under `rust-port/` and **29**
   under `backend/` — every nested `crates/*/tests/` and `testdata/` tree.

`role_files` is a **capped sample for orientation, never an inventory** (4 per
role). `role_file_counts` now travels beside it with the real per-role totals,
because four names in `tests` otherwise reads as "this subsystem has four
tests" — on `src/devcouncil` the true `other` count is 356.

Verified with the release binary against this repository's store
(1,417 files, 16,559 symbols, 88,479 edges):

| | before | after |
|---|---|---|
| subsystems with a non-empty `role_files` | 0 | 16 of 16 |
| `role_file_counts` | absent | present on every subsystem |
| `role_files_shown` / `role_files_total` | — | 78 / 628 (truncated, reported) |
| `files[].kind` distinct values | 1 (`code`) | 7 |
| `kind` breakdown | `code` 1,417 | `test` 691, `module` 497, `doc` 107, `config` 67, `package` 43, `script` 10, `file` 3 |
| `README.md` kind | `code` | `doc` |
| `meta.devmap_rust.role_files_computed` / `file_kinds_computed` | absent | `true` |
| `repo_map.json` | 349,490 B | 371,086 B (+6.2%) |

**Cost: none measurable — but only a controlled A/B says so, and the first
answer here was wrong.** Sequential sampling across this session said p50
0.17 s before the pass and 0.19 s after, and that was written up as a real
+0.02 s. It was not. Re-measured back-to-back on the merged tree, same binary
target, same store, nothing else changed but `manifest.rs`:

| `manifest.rs` | p50 | min | n |
|---|---|---|---|
| main's (no `handoff_paths`, `role_files` or `kind`) | 0.14 s | 0.11 s | 21 |
| this branch's | **0.13 s** | 0.09 s | 21 |

The two are the same within noise, and the branch is if anything marginally
faster. What moved in the earlier series was the *machine*: the floor of the
sample dropped from 0.13 s to 0.09 s between the first reading and the last,
which is a 30% shift in the least noisy statistic available and larger than the
effect being claimed. A sequential before/after separated by an hour of builds
measures the hour, not the diff.

Recorded this way deliberately: the discarded numbers are left visible above
because "we measured a regression that was not there" is the more useful entry.
The rule it cost: **a perf delta smaller than the drift in the sample floor is
not a result** — rebuild both sides and interleave them, or report no answer.

Two avoidable costs were removed along the way regardless, because they were
real work the field did not need:
`file_kind` ran six independent `split('/')` scans per file (now one pass
setting both flags), and `role_buckets` normalized each path *once per area* —
areas nest, so `src/devcouncil/cli/main.py` was lowercased once for
`src/devcouncil` and again for `src/devcouncil/cli`. Paths are now normalized
once for the whole generation and borrowed.

**Honesty surface, Python side.** `indexing/subsystem_map.py` gains
`role_files_established`, `role_files_are_complete`, the tri-state
`role_files_for_area` (a mapping, or `None` for unknown) and `role_file_total`,
which reads the companion counts. `role_file_total` returns `None` rather than
`len(bucket)` when the map carries no total — answering with the sample size is
the specific way a capped list comes to be reported as an inventory.
`role_files_are_complete` is `False` on any real map, which is the honest answer
for a documented sample. Readers updated to match: guide step 5 now says the
buckets are a sample and names the real roles (or says the field was not
computed and points at `files` instead), the wiki prints `showing 4 of 356` and
says so explicitly when the field is absent, the MCP detail carries
`role_files_computed` and `role_file_counts`, and the graph HTML prints
`(not computed for this map)` rather than an empty roles block.
`repo_mapper.role_buckets_computed` is the single model→reader adapter, beside
`handoffs_computed`.

**Tests.** `crates/devmap-query/tests/subsystem_roles_and_file_kinds_are_computed.rs`
(9 tests) — 8 fail against the pre-fix constants. Two of them are guards against
the *cheap* fix rather than the real one: `kind` must take at least four
distinct values across a mixed fixture (asserting `README.md == "doc"` alone
would pass for a writer that swapped one constant for another), and a test file
under an `/routes` path must land in `tests`, not `api`, or the buckets stop
partitioning and `test_resolver.py` runs a route handler as a test.
`tests/unit/test_subsystem_map.py` gains 8 reader tests.

**`test_repo_map_subsystem_roles_handoffs_and_file_kinds` is no longer
`xfail`.** It was `xfail(strict=True)` for as long as all three fields were
constants; it now passes, un-xfailed rather than deleted because the assertions
were always the right ones. It was also strengthened while it was open: it now
asserts `kind` takes ≥4 values, that every role bucket has a matching count, and
that all three provenance markers are present.

**A known false positive, found by checking the consumer rather than the
field.** `verification/test_resolver.py` now populates `_tests_by_area` for 6
areas where it had 0 for the field's whole life — the dead fallback is live. Two
of those entries are not runnable tests:
`src/devcouncil/verification/test_resolver.py` itself (a production module whose
filename starts with `test_`, which pytest's own collection convention would
also claim) and `rust-port/testdata/fixtures/tier_a/ts_app/index.ts` (extractor
*fixtures*, matched by the `/testdata/` token). The rule is faithful to its
ancestor and the corpus shows no *mid-name* `test_` matches to tighten against —
0 of 1,417 files have `test_` anywhere but at the start of a filename — so this
is name-based classification's floor, not a bug in the port. It is recorded
because it has a consequence: `test_resolver` may suggest running a file that is
not a test. Narrowing it needs a signal the map does not carry (whether a file
is collected by a test runner), which is a separate piece of work.

**Left undone by this lane, precisely.** `subsystems[].summary` is still `""
for every subsystem, and the second surviving `xfail` in that file still stands:
`package_managers`, `candidate_files` and `lsp` are written as constants
(`[]`/`{}`) where the Python writer detected uv/npm from lockfiles and the LSP
languages, and `wiki.py` and the MCP map handler render them. Same shape, not
touched here.

## Refusal inventory and edge resolution (2026-09-06)

Two open items from the previous pass, on branch `claude/devmap-open-items`
(forked from `main` at `385baa0`; merged forward to `e1ff55f` before the work
and to `4994695` after it, the second merge conflicting only in this file —
both sections kept, newest last).

### 1. `discovery_refused_files` became an inventory — **closed**, `5fd2a8c`

**The residual as stated.** The drain's comment in `daemon.rs` recorded it
itself: the incremental branch never re-walks discovery, so it carried the
previous generation's *count* and took `max(previous, refused.len())` as a
floor. Two wrong answers followed, and the comment named the first — a repaired
file stays counted until a full re-extraction or a `devmap build`. The second is
the expensive one and was not named: a refusal **this batch met** was only
counted when it exceeded the carried number, so a new refusal in a repository
whose last full walk refused two files disappeared into the two. Coverage
reported as no worse than it already was, when it is worse.

Both fall out of the same thing: a count cannot be maintained, only replaced.

**Red first, watched fail against the pre-fix kernel** (the assertion text is
`assertion 'left == right' failed`, `left: Some(1)`, `right: Some(0)`):

- `devmap-serve/tests/discovery_refusal_inventory.rs::a_shrunk_file_leaves_the_refusal_inventory`
  — "a corpus with nothing left unread must say so"
- `…::a_repaired_symlink_clears_the_refusal_and_lands_in_the_graph`
  — "the link resolves inside the repository now"
- `…::a_new_refusal_is_recorded_by_path_and_an_unrelated_batch_leaves_it_alone`
  — the count half passes on the pre-fix kernel for the wrong reason (the floor
  holds the number up), so the assertion is on the inventory, which did not
  exist. Recorded as red-by-non-compilation rather than implied to be more.
- `devmap-store/tests/coverage_gap_inventory.rs` — four tests, including the
  write-side guard and the `None`/`Some(0)` distinction.
- `devmap-cli/tests/status_names_what_it_could_not_read.rs` — two tests through
  the real binary; the pre-fix `devmap --json status` on the same fixture
  carries no `coverage_gaps` key at all (captured before the change).

**A fixture defect found while writing them.** `daemon_discovery_refusals.rs::
an_incremental_resync_does_not_erase_a_recorded_refusal` documents the
incremental branch and never reached it. `save_generation` stamps `head_sha` as
`"unknown"`; the daemon's own HEAD reading outside a git repository produces
`"unavailable"`; so `head_moved` was true on every drain in that fixture and it
measured the full-rebuild path under a comment saying it did not. The stamp is
now explicit and the precondition asserted, in that file and in
`one_symlink_rule_end_to_end.rs`. Both suites still pass (4 and 3 tests).

**The fix, at the owner.** Schema 14 adds `generation_coverage_gaps`: one row
per path, keyed `(generation_id, gap, path)`, `WITHOUT ROWID`.
`discovery_refused_files` is `COUNT(*)` over the `discovery_refused` rows,
derived in `Store::latest_analysis` and never carried. A full walk replaces the
inventory outright; the drain carries the previous one minus every path in this
batch's affected set and adds what this batch was turned away from — so a path
nothing touched keeps its verdict (the watcher would have reported a change) and
a path this batch touched is re-decided by `candidate_kind`. `PendingDelta`'s
`refused` became a path→verdict map for that reason. The floor is gone, and so
is the `max`.

`save_generation` refuses a generation whose summary and inventory disagree, the
same guard the edge count already gets:

```
generation would store None discovery refusal(s) but its analysis was computed
over Some(1); `discovery_refused_files` is derived from the inventory and the
two must be one measurement
```

`Some(0)` (measured, nothing refused) and `None` (nobody walked) stay distinct
end to end — the summary field keeps that, the row count cannot express it.

**`status` now names the files.** The same table carries the two extraction
gaps, so all three of `degraded_reason`'s numbers have paths behind them. On
this repository the refusal is `rust-port/vendor/grammars/cobol/parser.c`,
30,660,349 bytes against the 1,048,576 ceiling — the correct verdict, and until
now indistinguishable from a broken indexer without opening the database.
Rendered once by `devmap_serve::coverage_gaps_json` for both `devmap status
--json` and the daemon's IPC `status`, each list capped at 50 with `{shown,
total, truncated}`. The extraction gaps are derived at *write* time from
`devmap_analyze::extraction_gaps` — now the owner `extraction_coverage` folds,
so a path list and a count cannot describe different files — because deriving
them on read means walking past a ~47 KB `extraction_json` on every row of the
generation, which is the scan the v13 index exists to avoid.

They carry forward on a differential write exactly as the file rows do
(skipping `affected` and `deleted`), because the daemon hands `save_generation`
only the extractions it re-read. The refusal half is never carried there: this
function cannot tell a path the caller re-decided from one it never looked at,
so the caller that walked owns that decision.

### 2. Each edge's `Resolution` kind is persisted — **closed**, `9fa873e`

The limitation recorded in "3. Resolver / liveness honesty": no `resolution`
column, so `devmap-query`'s `stored_edge_to_resolved` rebuilt every edge with
`resolution: None` and the invariant on the read path rested on the write-side
constructor plus a round trip.

Schema 15 adds `generation_edges.resolution`, written through
`resolution_kind_label` — an exhaustive match on `Resolution`, beside
`edge_kind_from_stored` and for the same reason. `edge_resolution` decodes it;
a row without the column is reconstructed from the edge's own file layout and
labelled `ResolutionSource::Reconstructed`, so a guess can never read as a
measurement. `GenerationEdges` carries the decoded tier, read in the same
statement and cached under the same generation id as the rows it labels, and
`devmap status --json` gains `edge_resolution_source`.

**Red first:** `devmap-store/tests/coverage_gap_inventory.rs::
a_stored_resolution_kind_beats_the_reconstruction_a_row_would_support` writes a
`Structural` edge whose endpoints share a file — so the only reconstruction the
row supports says `SameFile` — and asserts the read returns `Structural`/
`Stored`; then clears the column and asserts the same read answers `SameFile`/
`Reconstructed`. Against the pre-fix kernel neither the column nor
`GenerationEdges::resolution` exists, so this is red by non-compilation; the
runtime evidence that the surface was absent is the pre-change `devmap --json
status`, which carries no `edge_resolution_source` key.

`status` reads one row for that field, and that is a property rather than a
sample: `save_generation` writes a generation's edges in one transaction from
one `resolution.edges` and never carries edges forward, so the column is present
for all of a generation's edges or for none.

**Numbers.** Release binaries, this repository copied fresh into
`<scratchpad>/lane-f/c_{before,after}` and built cold, `devmap --json build .`:

| | store bytes | build (real) | files / symbols / edges |
|---|---|---|---|
| before (`5fd2a8c`) | 123,256,832 | 5.55 s | 1,334 / 15,080 / 74,729 |
| after (`9fa873e`) | 124,301,312 | 5.36 s | 1,334 / 15,080 / 74,729 |

**+1,044,480 bytes, +0.85%** — 14 bytes per edge, which is the column. The build
times are one run each and differ by less than the run-to-run spread; nothing is
claimed from them.

`cargo test -p devmap-store`, debug, warm target dirs, five interleaved runs
each: before 11.31 / 11.70 / 11.78 / 12.65 / 14.48 s, after 11.42 / 13.16 /
13.46 / 13.52 / 17.09 s. Minima 11.31 vs 11.42 s. The suite gained one test and
the spread is 3 s wide, so **the medians (11.78 -> 13.46 s) are not evidence of
a cost in the column** and are reported only because they were measured.

### Left undone, precisely

- `devmap-query/src/engine.rs:2390` `stored_edge_to_resolved` still sets
  `resolution: None`. Reaching the new column from a bare `StoredEdge` needs
  either a field on `devmap_store::StoredEdge` — which breaks the five struct
  literals in `devmap-query` (`engine.rs:3651`, `engine.rs:4255`,
  `tests/stored_edges_keep_the_confidence_their_evidence_earned.rs`) — or a
  second entry point taking `(StoredEdge, EdgeResolution)`. Both are edits in a
  crate this lane does not own. The red test for it is written and left
  **unstaged** at `devmap-query/tests/stored_edge_resolution_is_read_not_guessed.rs`.
- The Python doctor (`src/devcouncil/devmap_health.py`) prints the three
  coverage numbers and not the paths. `coverage_gaps` is now in the kernel's
  `status` JSON for it to read; the Python change is out of this lane's scope.

## The read path carries the evidence it stored (lead, 2026-09-06)

Closes the item the refusal-inventory lane left undone: schema 15 persisted
each edge's resolution *kind*, and `devmap_query::resolved_edge_from_stored` —
the path `devmap manifest` builds `code_graph.json` from — still set
`resolution: None` on every edge, so the artifact every agent reads asserted
confidences whose justification nothing could read back. The lane's own red
test, left unstaged, failed at `left: None / right: Some(Evidence { kind:
Structural, source: Stored })` against this tree before the change.

**One owner for the kind.** `ResolutionKind` now lives in `devmap-resolve`
beside `Resolution`: the variant set (`Resolution::kind()` is an exhaustive
match), the confidence ladder (`ResolutionKind::confidence`, which
`Resolution::confidence` delegates to) and the stored spelling
(`label`/`from_label`). `devmap-store`'s `StoredResolutionKind`,
`ResolutionSource` and `EdgeResolution` are re-exports of the resolver's
`ResolutionKind`, `ResolutionSource` and `Evidence`; the two hand-written
tables it held are gone. `ResolutionSource` gained `Resolver` for an edge the
resolver built in-process, so the three provenances are never spelled the same.
Pinned by `devmap-resolve/tests/resolution_kind_is_one_owner.rs` (kinds ==
variants, payload cannot change the tier, spellings round-trip and are exact).

**What a row can answer.** `StoredEdge` carries `resolution: Option<String>`
(the column as stored), every reader fills it, and
`edge_resolution(&StoredEdge)` decodes it — `Stored` when present,
`Reconstructed` from the file layout when the generation predates the column.
`ResolvedEdge` gained `evidence: Option<Evidence>`: `Some(.., Resolver)` from
`ResolvedEdge::resolved`, `Some(.., Stored | Reconstructed)` on a re-read edge.
`resolution` itself stays `None` on the read path on purpose — an
`ImportScoped` row does not carry `imported_from`, and a `Resolution` invented
to fill the variant would be a guess wearing the resolver's type.

**The artifact says so.** Every edge in `code_graph.json` (both wires) now
carries `resolution` (the kind's spelling) and `resolution_source`
(`resolver` / `stored` / `reconstructed`), mirrored in
`indexing/graph/schema.py`'s `GraphEdge`. The dedup key stays the four-field
identity; when two edges share it but not a tier the sort makes the survivor
the smallest label, deterministic (R4) and stated at the site.

**The read-side half of the honesty invariant.** On the way in,
`ResolvedEdge::resolved` makes `confidence` a function of the resolution; on
the way out nothing compared the two, so a row whose confidence had been
changed under a stored kind read back as a measurement. `GenerationEdges`
counts such rows when the index is built (`confidence_mismatches()`, in
milliconfidence, stored kinds only — a reconstruction cannot convict the row)
and `Store::edge_confidence_mismatches()` answers the same question in SQL for
a process that holds no index, with the `CASE` ladder generated from
`ResolutionKind::ALL` so it cannot hold a second copy of the table. Both
`status` surfaces report it as `edge_confidence_mismatches` beside
`edge_resolution_source` (the daemon's IPC status gained both). Red first:
`coverage_gap_inventory.rs::a_stored_confidence_that_contradicts_its_stored_kind_is_counted_not_trusted`
(index 1 / SQL 1 after one `UPDATE`, 0 / 0 after the column is cleared) and
`status_names_what_it_could_not_read.rs::status_counts_stored_edges_whose_confidence_contradicts_their_evidence`
(the real binary, both directions).

**Numbers.** Workspace: fmt and clippy clean, **1,448 tests / 0 failed / 2
ignored**, feature-off checks green, release `devmap 0.1.0 (store schema 15,
code graph schema 2)`. Python: ruff and mypy clean, the 40 unit files that
touch the graph schema 776 passed. Go, against this kernel's artifacts:
all seven packages `ok` with `MANVI_MAP_BINARY` pointed at it, including `dc/devmap`'s live interop test that builds one `Map` from both wires. `devmap --json status` on a fresh 41,276-node / 271k-edge
scholarlm store, 21 runs: p50 61 ms / min 54 ms wall, of which the SQL count is p50 33 ms (271,508 rows scanned, 0 mismatches). That cost is paid only by the fresh-process CLI path — the daemon answers from the index it already holds — and it scales with the edge count; a partial index on the mismatch predicate would make it O(mismatches) but would bake the ladder into DDL, a second copy of the table, and was not done.

---

## Coverage as a ratio, not a flag (2026-09-06)

An audit of the shipped P0–P4 work orders, run against a real corpus rather
than against fixtures. Five defects, each reproduced by measurement before it
was fixed, each with a test watched go red against the pre-fix tree.

### The confidence ladder was collapsing on every real repository

`ExtractionCoverage::cap` was a **binary** gate. `is_complete()` asks whether
any of four corpus-wide counters is non-zero; if one is, every non-exempt
finding in the generation is set to `COVERAGE_LOSS_CONFIDENCE_CAP`.

Measured on this repository: 1,502 files, **10 of them blind** — 3 parse
failures, 2 pattern-recovered, 1 oversized vendored `parser.c`, 4 in a
call-blind language. **0.67% of the corpus.** All **214** dead findings came
back at exactly 0.35.

Two consequences:

- `generation_dead_symbols` is read `ORDER BY confidence DESC, file_path`, so
  with every confidence tied the ranked list an agent reads degenerated to
  **alphabetical order**. The default 2,000-token budget showed it the first
  ~66 filenames rather than the strongest evidence.
- Three claims printed as one number: "no edge in the generation names this
  symbol" (0.9), "something calls it and the resolver could not say which"
  (0.4), and "an unresolved site names it" (0.4). The first is evidence *for*
  death and the other two are evidence *against* it. That is not conservatism.
  Conservatism lowers the ceiling and keeps the order.

The ceiling now tracks the size of the hole: `(1 - blind_share)^8`, clamped
into `[0.35, 0.89]`. `files_with_call_extraction` is the denominator the five
gap counters never had, counted from the same two predicates `extraction_gaps`
charges `CallBlind` from. Prose is in neither side — in the numerator it would
mark every repository degraded, in the denominator a README would raise a
broken repository's confidence.

The exponent is a policy dial and is documented as one. At 8 the ceiling reaches
the floor at a blind share of **12.3%**, so a substantially-unread corpus keeps
exactly the previous behaviour and only a small hole buys grading. Every older
coverage fixture in `devmap-analyze` is a two- or three-file corpus — 33% to
50% blind — which is why all of them still pass unchanged; that is pinned by
`the_ceiling_reaches_the_floor_well_before_the_corpus_is_half_unread`, not left
to luck.

Nothing is promoted. `cap` never raises a confidence, and
`HIGHEST_DEGRADED_CONFIDENCE` (0.89, rendering 890 millis) keeps an incomplete
scan below `EXTRACTED_FLOOR_MILLIS` at **any** blind share.

**Measured after, same corpus, same command:** the same 214 findings, now 49 at
0.89 and 165 at 0.40, ranked by evidence. `dev map dead --confidence inferred`
returns the 49 rows with no inbound edge at all; before, it returned nothing on
this repository, because `extracted` was structurally unreachable and
everything else was `ambiguous`.

### A cluster outranked the single-symbol verdict on identical evidence

`is_reaching_edge` excludes ambiguous and unresolved edges — correctly, since a
cluster kept alive by a guess is a finding silently suppressed — and then
nothing looked at them again.

Reproduced with one call site, `it.alpha()`, and two candidates:
`Widget.alpha` came back at 0.4 with `only_ambiguous_callers`; `Task.alpha`,
reached by the *same* ambiguity, came back inside a cluster at **0.5** saying it
was "reached by nothing outside the component", which the edge list contradicts.
`Task.alpha` never appears in the single-symbol list at all — its sibling calls
it deterministically — so the cluster row was the only thing naming it, at the
wrong tier with the more absolute prose.

A component something names through an unbindable call now carries
`DEAD_CLUSTER_QUALIFIED_CONFIDENCE` (0.4) and a reason that says what reaches
it. The unresolved ledger reaches it through `unresolved_namesake_names`, now
shared with liveness rather than copied. A component nothing reaches is
untouched, which is pinned.

`cap_cluster` also prices the component claim separately: it compounds the
coverage ceiling over the membership, because a missed edge into a component
makes the whole finding wrong where the same edge costs a single-symbol finding
only itself — which `dead_clusters` already argued in prose and nothing
implemented.

### A circular import was a dead cluster waiting for one unrelated change

`Imports` edges carry the file path in **both** symbol positions, so
`a.py -> b.py -> a.py` is a two-node strongly connected component in the graph
Tarjan walks. Nothing came of it only because all three `File`-symbol
construction sites emit `is_exported: true`, putting every file into
`externally_reachable_symbols`. Nothing stated that dependency and nothing
tested it — and the day a `File` node stops reading as exported, which is a
reasonable change since a file node is not public API, every circular import in
every TypeScript barrel and Python package becomes a dead cluster.

Import edges are excluded from the component graph outright. They connect file
nodes only, liveness never reports a `SymbolKind::File`, and "a cluster of
files" is not this pass's claim — so the exclusion costs nothing that could have
been a finding, and shrinks the graph by one node per file plus every import
edge. Pinned with a hand-built extraction whose `File` symbol is *not* exported,
because that state is unreachable from `extract_file` and is exactly the point.

### Two numbers for one fact

`devmap status --json` reports `coverage_gaps.import_blind.total = 71` on this
repository; the manifest beside it, same generation, reports
`liveness_meta.unwired.excluded_import_blind = 355`.

The gap is prose. `extraction_gaps` charges `ImportBlind` only for files a
grammar read; `unwired_candidates` never asked — it runs the parse-failure
branch first, which `is_parse_failure` deliberately answers `false` for prose,
then charges everything left with no `Imports` capability, Markdown included.

The predicate now has one owner, `Extraction::grammar_read_this_file`, beside
`is_parse_failure` where the same line is already drawn. **Measured after:
355 -> 57**, against a population of 71.

The two are not meant to be equal, and that is the sharper form of the defect.
`coverage_gaps.import_blind` is every file whose language has no import
extractor; `unwired_excluded_import_blind` is only those the filter dropped,
which excludes any that returned earlier for being an entry root, a test,
vendored, or already imported. So the law is `<=`, and 355 against 71 broke it
in the one direction that cannot be explained away: a filter cannot drop more
files than exist for it to drop. The test asserts the subset law first, and the
fixture's coincidental equality second, so a fixture where the two happen to
match cannot hide the impossibility.

The *exclusion* was load-bearing and older than the reason given for it: before
the W0.3 capability gate, every `.md`, `.json` and `.yaml` in every repository
was an unwired candidate, and the gate swept them up by accident. They are now
excluded on their own grounds and counted in neither number.

### `verify.sh` had never run past step 5 from a clean checkout

Every shell script under `rust-port/` is committed `100644`. `verify.sh` invokes
`./tools/fanout.sh`, `./tools/memory_model_probe.sh` and `./tools/soak.sh`
directly, so **steps 5 through 9 have never run** for anyone who did not
`chmod` their own working tree. CI runs `bash ./verify.sh`, which starts the
script and then hits the same wall inside it.

It failed the way this ledger's own discipline warns against, twice: a true
sentence with the wrong cause. `GATE FAIL: could not derive the ambiguity
fan-out` was about neither the fan-out nor the derivation.

Steps 1 and 2 were also red at HEAD on rustc 1.98 — six clippy lints and a
formatting drift, none of them in code this pass had touched — so steps 3
onward did not run either. And step 5 hardcoded `./target/release/devmap` while
step 4 builds it with `cargo run --release`, so any run with `CARGO_TARGET_DIR`
set reported "could not measure peak RSS" when the measurement was fine and the
binary was elsewhere.

### Adversarial input found three overflow panics in the new code

`blind_files()` and `blind_share()` summed `usize` counters with a plain `+`.
Four of the five are bounded by a file count, but `discovery_refused_files` is
folded in from outside — `DiscoveryCoverage::refused(n)` takes whatever a caller
passes — and it is the numerator of a division. Debug panics; **release wraps a
saturated blind count round to a small one and hands a near-`extracted` ceiling
to a corpus nothing read.** Saturating adds, and the ratio clamped after the
division.

`graded_cap_under_hostile_input.rs` carries twelve cases: saturated counters
together and singly, `NaN`/`INFINITY`/`f32::MAX` as confidences, a cluster of
`usize::MAX` members against a `powi` exponent, a corpus that is entirely
refusals, ratio-independence from corpus size, a 100,000-node ring through the
public entry point, the oversized-graph refusal stated rather than returned as
an empty result, degenerate self-loops and empty names, a symbol named only by
`::` and `.` separators, and a determinism sweep over the new qualification
pass.

### Examined and found not to be defects

- **The Python resolution-rate ratchet has zero tolerance where the Rust CI
  fence has 10 permille.** Predicted as a false-positive generator; **measured
  and refuted.** On a 423-file corpus with ~44k attribution sites, adding one
  unresolvable call moves the net rate by **0 permille** (372 → 372); ~93 new
  unresolvable sites are needed to move it by one. The two thresholds are
  calibrated for different denominators — tiny deterministic fixtures against a
  whole repository — and the divergence is explained rather than accidental.
- **A file with both a `CallBlind` and an `ImportBlind` gap writes two rows.**
  `generation_coverage_gaps` is keyed `(generation_id, gap, path)` and every
  count is `WHERE gap = ?`, so neither bucket double-counts.
- **`unwired_candidates` counts ambiguous and unresolved `Imports` edges as
  wiring** where liveness and the component pass exclude them. Inconsistent, but
  in the conservative direction for this question — an ambiguous import edge is
  evidence the file *may* be imported — so it errs toward fewer findings.

### Numbers

The gate suite, step by step, before and after. "never reached" means the run
stopped at an earlier step, which is where all of these sat.

| step | before | after |
|---|---|---|
| 1 format | **FAIL** — drift in `rung.rs`, `code_graph.rs` | pass |
| 2 clippy `-D warnings` | **FAIL** — six lints on rustc 1.98 | pass |
| 3 `cargo test --workspace` | never reached | pass |
| 4 determinism double-build | never reached | pass — `c613ef69…ca736e`, identical |
| 5 self-build gates | never reached | pass — build 4,093 ms, 1,506 files, peak RSS 659 MiB against a 774 MiB budget |
| 6 memory-model probe | never reached | **FAIL** — see the section above; measured, named, not papered over |
| 7 growth gate | never reached | pass — 925,696 → 1,720,320 → 1,785,856 → 1,789,952 → 1,794,048 B over five builds, 2 generations retained |
| 8 incremental-vs-cold | never reached | pass — `SOAK SMOKE OK`, digest stable across 5 cycles |
| 9 mutation testing | optional | not run |

Beyond the suite, a synthetic hostile corpus — 411 files: three-way circular
imports, a 5,000-function mutually recursive ring, non-ASCII identifiers, a
200,000-term single line, 400-deep nested braces, 40-deep directories, 200
same-named modules, a self-importing module, invalid UTF-8, an empty file, a
symlink loop and a dangling symlink. Cold build **0.64 s**, exit 0, refusing
exactly two files with named reasons (`Unreadable`, `EscapesRoot`). Every
subcommand answered without a panic. The ring came back as **one** cluster of
5,000, not 5,000 findings; the import cycle as a three-member cluster of
*functions*, not of files; `trace` past its depth bound returned `Unavailable`
with the bound named rather than "no path". Modify, add and delete cycles
converged on the cold digest exactly.

### Step 6 has never run, and it is red — with a named cause

Making `tools/memory_model_probe.sh` executable and pointing it at the right
binary ran the memory-model probe for what is, as far as this ledger can tell,
the first time. Its arithmetic preconditions failed immediately:

```
PROBE FAIL: derived widest fan-out 16, corpus has 100
```

**16 is `AMBIGUOUS_FANOUT_CAP`.** The probe designs a corpus with 100
candidates per name and asserts a widest fan-out of 100; audit R-7 capped one
site's *emission* at 16 edges after this probe was written, and every one of
its four arithmetic preconditions was stated in terms of `DEFS`. The probe now
reads the constant from its owner — the pattern `verify.sh` already uses for
`DB_SIZE_GATE_PER_FILE` — computes the effective width as `min(DEFS, cap)`, and
fails closed if the constant cannot be read. All four preconditions pass.

**The three coefficient caps then fail, and that is the real finding.** The cap
bounds edges; it does not bound the candidate list, which the `Arc<Resolution>`
still holds in full and deliberately so — that is what keeps `impact`
answerable on candidates 2..N. So since R-7 the memory is proportional to
*candidates* while every denominator in the probe and in `verify.sh`'s RSS
budget is derived from *emitted edges*, and the two stopped being the same
number.

Measured, both runs after the preconditions were corrected:

| corpus | milli-B/pair (cap 40,000) | milli-B/edge (cap 800,000) | % of model (max 125) |
|---|---:|---:|---:|
| `DEFS=100` — cap active, 100 candidates → 16 edges | 77,145 | 1,234,329 | 193% |
| `DEFS=16` — cap inert, 16 candidates → 16 edges | 45,926 | 734,822 | 138% |

Removing the cap's effect alone takes bytes-per-edge from 1,234 to 735 and back
inside its 800 cap, which is the decoupling measured rather than argued. What
is left — 46 B/pair against 40, 138% against 125% — is either a real per-edge
regression from the 410 B the model assumes, or an artifact of comparing a
16-wide 116-file corpus against coefficients measured on a 100-wide one. **It
was not settled here, and no number was moved to make the gate green**; that
would be the "raised to fit" this ledger refuses two sections above.

Fixing the denominator properly needs a schema change. `tools/fanout.sh`
derives from the persisted graph and `generation_edges` has no `details`
column — the candidate total lives only on the in-memory `ResolvedEdge`. The
same staleness reaches `verify.sh` step 5, whose `RSS_PER_FANOUT_EDGE = 600 B`
is justified as "1.5x the 401-415 B/edge measured by
tools/memory_model_probe.sh": that measurement was taken on the pre-cap
resolver. Step 5 passes today (659 MiB against a 774 MiB budget) with a
denominator that counts the wrong thing.

### Still open, and deliberately not started here

- **B3/SC2 — write amplification.** Re-measured, and this ledger's
  characterisation is stale. On a 1,060-file / 58,651-edge corpus a one-file
  edit costs: extract 0.565 s, **resolve 0.340 s (15%)**, analyze 0.109 s (5%),
  **persist 1.221 s (55%)**. The "resolve and analyze are 54% of a rebuild"
  figure was measured on a different corpus and does not hold here. The narrowed
  write and a `--full` write cost about the same (~1.2 s each), which is write
  amplification measured rather than argued. The fix is the validity-range
  schema redesign AGENT_PLAN marks **decision (#2)**; it is a store-schema
  rewrite and was not started unilaterally.
- **W0.3 move 2 — import extraction for the C family and the JVM/CLR
  languages.** `#include` still has no handler anywhere in the extractor, and 23
  of 35 languages declare no `IMPORTS` capability, so `unwired_candidates` is
  permanently empty for them. On this repository that is 71 files; in a Java or
  C++ shop it is the whole tree. The W0.3 plan shipped move 1 ("stop the
  bleeding") deliberately and named move 2 as the follow-up.
