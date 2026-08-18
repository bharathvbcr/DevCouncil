# AGENT_PLAN — building `devmap` (Rust) from scratch

**Audience:** a coding agent executing this port. Not a design document — PLAN.md owns design,
AUDIT.html owns findings. This file owns *execution*: what to do, in what order, under which
rules, with which proof.

**Basis:** PLAN.md §3–§7 · AUDIT.html (84 findings, all-Rust disposition) ·
PHASE1_CONTRACT.md (budgets, scoping) · current scaffold under `crates/`.

---

## 0. Session protocol

Every working session, in order:

1. Read `STATUS.md` (create it in task 0.4 if absent). It names the active phase, the active
   task, and the last gate passed. Never infer progress from the code.
2. Read the phase section below for the active task. Read its **Inputs** before writing code.
3. Work the task. One task per commit minimum; commit messages reference the task ID
   (`RP2.3`) and any finding IDs closed (`closes G19, X10`).
4. Before claiming a task done, run its **Proof** commands and paste their output into
   `STATUS.md` under the task entry. A task without recorded proof is not done.
5. Update `STATUS.md` (task state, next task, blockers) as the final act of the session.

## 1. Ground rules — apply to every phase

- **R1 · The Python implementation is frozen.** Never modify `src/devcouncil/indexing/`,
  `src/devcouncil/codeintel/`, or `src/devcouncil/cli/commands/map.py` — no fixes, no
  instrumentation, no "small improvements". The only sanctioned Python changes are the Phase 6
  thin client and the Phase 1 snapshot tooling under `rust-port/tools/` (which imports the
  frozen code read-only).
- **R2 · Findings are tests.** Each of the 117 properties (PLAN §3's 31+7, AUDIT's 84 minus
  duplicates) is closed only by a test that names its ID in the test name or a
  `// closes: G19` comment. Fixing behaviour without the test does not close the finding.
- **R3 · The baseline is wrong in known places.** Do not chase byte-parity where AUDIT says
  the Python output is defective. Record every deliberate difference in `DIVERGENCES.md`
  (task 0.4) as: finding ID → what Python does → what devmap does → test proving it.
- **R4 · Determinism is non-negotiable.** No iteration over hash maps into any output. Use
  `BTreeMap`/`BTreeSet` or sort before emit. Gate: build twice, byte-identical artifacts
  (G4, G26).
- **R5 · Confidence honesty.** The `Resolution` enum (AUDIT §8.2) is the only way to make an
  edge. Multi-candidate picks can never emit `Extracted`. Unresolved is a ledger row, never
  silence (G5, G6, G7).
- **R6 · Failure ≠ emptiness.** `ParseOutcome` (AUDIT §8.1) is the only extraction return
  type. `Failed` is never cache-admitted under a real content hash (X6, X7).
- **R7 · Counts never lie.** Every truncating surface returns `{shown, total, truncated,
  tokens_used}` computed by the truncation function itself; rank before truncate (T3, V12,
  V13, G24).
- **R8 · Escape at the sink.** Any generated HTML/JSON interpolation goes through one escape
  helper with a hostile-name fixture in CI (V1).
- **R9 · Toolchain hygiene.** `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
  `cargo test --workspace` green before any gate claim. No `unwrap()` outside tests; errors
  are `thiserror` types per crate.
- **R10 · Don't invent scope.** Anything not named by PLAN, AUDIT, PHASE1_CONTRACT, or this
  file goes to `STATUS.md → Open questions` instead of into code.

---

## Phase 0 — Reconcile the scaffold (½–1 day)

The workspace is not empty: 7 crates (~4k LOC), `testdata/` starter corpus, hardening tests.
Do not assume any of it is correct — it predates most audit findings.

- **0.1** Inventory: `cargo test --workspace` and record what exists per crate vs what this
  plan requires. Map each existing module to the plan task that owns it.
- **0.2** Audit-check the scaffold against the high-severity findings that shape types:
  does `devmap-extract` return a `ParseOutcome`-equivalent (R6)? Does `devmap-resolve` have
  the `Resolution` enum (R5)? Does `devmap-store` run migrations in single transactions (S2)?
  Are FTS terms quoted (S3)? Record gaps as pre-work items on the owning phase task.
- **0.3** Delete `test_regex.rs` at the workspace root (stray file) and add `.DS_Store` to
  `.gitignore` if untracked.
- **0.4** Create `STATUS.md` (phase/task ledger) and `DIVERGENCES.md` (R3 ledger, seeded with
  the AUDIT §7 parity-trap list: G1–G7, X1–X3, X9, G19, G4/G26 nondeterminism, S17 races).
- **Proof:** `cargo test --workspace` output recorded; both ledgers exist; gap list in
  `STATUS.md` maps every scaffold module to a plan task.

---

## Phase 1 — Freeze the contract (REQ-RP-1)

Objective: pin what "correct" means before building against it. PHASE1_CONTRACT.md is the
working record; this phase completes it.

- **1.1 Language authority.** In `devmap-extract/src/languages.rs`, make `LanguageSpec` the
  single source: name, extensions, grammar, `ExtractorId` (exhaustive enum — a language
  without an extractor must not compile), LSP id, viz color. Port the 35 languages from
  Python's `codeintel/languages/registry.py` `LANGUAGE_SPECS`. Include the alternate
  extensions X8 found dropped: `.pyi .mts .cts .ets .phtml .mm .sc .dpr .cuh .cpy .hrl`.
  Test: every extension in the Python registry maps to the same language here; every spec
  routes to a named extractor. *(closes X8 structurally)*
- **1.2 Golden corpus.** Extend `testdata/` to one fixture repo per language **tier** (see
  2.2), each containing the §4.1–§4.3 corpus requirements from PLAN plus the audit's
  blind-spot syntax: TS `enum`/`namespace`/`declare`/`abstract`/overloads/`export *`/
  `new Foo()`/decorators/`import * as ns` (X1, X2, X3, X20, G18); Rust generic impls +
  grouped `use` (G19, X9); Python `__all__ +=`, stdlib-named methods (G3); Go composite
  literals + block imports (X2, X17); hostile file name `x<img src=x onerror=alert(1)>.ts`
  (V1); a file that cannot parse (X6).
- **1.3 Baseline snapshots.** Under `rust-port/tools/snapshot/` (Python, imports frozen code
  read-only), build the snapshotter: run the Python graph build on each fixture repo with
  `PYTHONHASHSEED=0`, emit normalized sorted sets per PLAN §7.8 (`nodes`, `edges` with
  confidence, `dead`) to `testdata/golden/<fixture>/`. Commit the snapshots — they are the
  parity baseline forever; regenerate only by explicit decision recorded in `DIVERGENCES.md`.
- **1.4 Consumer contracts.** Enumerate the 37 consumer files (`grep -rl "devcouncil.indexing\|
  devcouncil.codeintel" src/ --include='*.py'` minus the subsystem itself). For each: which
  functions/artifacts it touches, what shape it assumes, tri-state expectations (X14:
  unknown ≠ verified-zero). Write `CONSUMERS.md`. The six private-internal reaches listed in
  AUDIT (X audit §e) get explicit "will break at cutover" entries.
- **1.5 Token budgets.** Already drafted in PHASE1_CONTRACT.md — verify each surface listed
  there has a matching `Budget` constant in `devmap-query` and a test asserting the manifest
  stays ≤2k tokens on the DevCouncil repo itself (T1).
- **Gate:** corpus covers every §4.1–§4.3 row + every audit syntax fixture (checklist in
  STATUS.md); golden snapshots committed for all fixtures; CONSUMERS.md complete; language
  authority test green.

---

## Phase 2 — Extraction (REQ-RP-2 · crates: devmap-extract)

Objective: `ParseOutcome`-based extraction, tree-sitter throughout, parity on the corpus.

- **2.1 Contract types.** `ParseOutcome { Clean, Partial{error_ranges}, Failed{reason} }`;
  `Extraction { engine, symbols, imports, calls, exports, references }`. Check
  `root_node().has_error()` after every parse — an ERROR tree is `Partial`, never `Clean`
  (X5). Byte-offset spans only; convert to line/col once at the boundary (UTF-8 safe).
- **2.2 Language tiers.** Implement in order, gating each tier on corpus parity:
  **Tier A** Python, TypeScript, TSX, JavaScript (the repo's own languages);
  **Tier B** Go, Rust, Java, C#, Ruby, PHP, Swift, Kotlin, C, C++;
  **Tier C** the remaining specs via generic tree-sitter queries with the unified capture
  vocabulary (`@definition.*`, `@call.name`, `@import.source`, `@heritage.*` — GitNexus
  pattern) so Tier C needs no per-language code.
- **2.3 Audit-driven syntax.** Within Tier A/B close: X1 (TS enum/abstract/namespace/declare/
  overloads — and recurse into unmatched nodes), X2 (`new_expression` as call; Go
  `composite_literal` as reference), X3 (`export *` sentinel), X20 (JS/TS decorators as
  calls), G14 (decorator line = `dec.lineno` semantics: attribute to enclosing scope, not the
  decorated symbol), G16 (JSX member tags), G18 (`namespace_import` → alias map), G19/X10
  (generic impls), X9 (grouped `use` forks per leaf), X4-class noise (consts are symbols only
  with function initializers).
- **2.4 Imports.** Port §4.1 capabilities: Python module-suffix + `__init__` ancestry;
  JS/TS relative + extension/index probing + tsconfig `extends`/project-ref aliases +
  workspace mapping + re-export chains (memoized once per build — V9); Go module prefix;
  Rust `mod.rs`/`name.rs`. Case-sensitivity explicit: resolve against the file set, note
  case-insensitive hits as `Inferred`.
- **2.5 Cache.** Key: `(content_hash, language, grammar_version, analyzer_version)` where
  grammar_version is the compiled grammar's real version — never a constant (S14). Admission
  takes `ParseOutcome`; `Failed` goes to a retry table with attempt count (X7). Files >1 MB
  and gitignored/vendored paths skipped by default via the `ignore` crate (CodeGraph
  hygiene). Parse in parallel with `rayon`, pool = real cores (B12, N9); grammars are linked
  in, so there is no worker pool and no S8 class.
- **2.6 Wiring/liveness inputs.** Port §4.2 exemptions (entry points from pyproject **at any
  depth** + setup.cfg, package.json, Cargo.toml `[[bin]]`; launcher references that also
  *seed* roots; `__all__` incl. mutation forms; generated/vendored/test detection; decorator
  wiring) — closing V6's gap list. Emit a `DiscoveryReport` (per-source yields) as data (V5).
- **Gate:** corpus parity per tier with divergences enumerated in `DIVERGENCES.md`; the
  X1/X2/X3/X9/G19 fixtures extract correctly (Python golden shows them missing — expected
  divergence); kill-the-parser test proves no empty cache entry under a real hash (X7);
  two builds byte-identical (R4); near-linear scaling on a 1k-file synthetic.

---

## Phase 3 — Resolution & analysis (REQ-RP-3 · crates: devmap-resolve, devmap-analyze)

Objective: the confidence ladder as types; liveness and clustering on an honest graph.

- **3.1 Resolution enum** (AUDIT §8.2) with the three invariants: no multi-candidate
  `Extracted`; best-confidence dedup upgrading in place (G5); `LangFamily` parameter on every
  global rung (G3, G7). The stdlib-name guard lives inside the unique-global rung only,
  Python-family only (G3). Import-scoped tier never silently widens to "anything imported"
  (G6). `Unresolved{reason}` rows persist (GitNexus provenance).
- **3.2 Ladder.** Same-file → import-scoped → unique-global, with receiver types from
  constructor assignment tracking, not name guessing (N6). Inheritance: family-guarded,
  import-preferring; ambiguity fans out or abstains (G7). MRO strategies per language family
  (first-wins / C3 for Python) for override/implement edges.
- **3.3 Package edges.** Go same-package membership is a star via a synthetic package node,
  not a clique (G20).
- **3.4 Liveness.** Reimplement `file_liveness` + `symbol_reachability` on the traversal
  kernel (3.6). Entry roots from the 2.6 DiscoveryReport; unreachable-ratio gate returns its
  meta (reason, ratio, gap points) into `graph.meta` (V5). Re-export protection is
  deterministic and conservative (G4). Parse-failed files are never dead candidates (X6).
- **3.5 Communities.** Leiden (or Louvain + connectivity post-pass) on **weighted** edges
  (call multiplicity), timeout surfaces `status: timed_out` — never an empty-but-ok result
  (N4, N5).
- **3.6 Traversal kernel** (shared, in devmap-analyze): `enqueued` separate from `visited`
  (G21); edge identity `(source,target,kind,line,col)`; per-add budget caps; edge-kind
  priority contains→calls→rest; parametric depth (G8); impact semantics: no upward
  `contains`, `instantiates` is a caller, container children fold at same depth (CodeGraph
  #536/#774). One kernel serves impact, processes, trace, dead — semantics cannot fork.
- **Gate:** full-graph corpus parity with enumerated divergences; determinism (two seeds,
  identical output); resolution-precision ≥ Python baseline on the DevCouncil repo; the
  G3/G5/G6/G7 fixtures produce the corrected edges; cold build of DevCouncil < 2 s.

---

## Phase 4 — Store & daemon (REQ-RP-4 · crates: devmap-store, devmap-serve)

Objective: v3 schema, one long-lived writer, freshness by construction.

- **4.1 Schema v3** per PLAN §7.6 (interned paths, clean PKs, `pending_paths`). Migrations:
  one `TransactionBehavior::Immediate` per step, `PRAGMA user_version` as the last statement
  inside it; kill-at-every-boundary test (S2). Open by path, never assembled URI strings
  (S9). Read-then-write pairs inside immediate transactions (S11). Checkpoint results read
  and acted on (S18). Aliases chain across retained generations (S16).
- **4.2 Differential writes + deletion reconciliation** — one work item, per PLAN §7.5
  (B3+N2). Carry-forward, delete-affected, insert-fresh, reconcile-against-`git ls-files`.
  1-file edit at 10k files writes <100 rows.
- **4.3 FTS.** External-content FTS5 over a current-generation search table synced by
  triggers; every user term quoted (`"…"` with internal quote doubling), optional trailing
  `*` for prefix (S3). Fuzz test with quotes/hyphens/parens/column-syntax.
- **4.4 Daemon.** `devmap serve`: owns the write connection, watcher, durable queue, and the
  MCP/socket surface. Status is a socket query — no PID probing anywhere (S1, D3); no unlock
  heuristics (S4, S5 have no analogue). Supervised event loop: per-tick panic capture →
  `degraded` + continue (S6); exponential backoff + poison-file quarantine surfaced in
  status (S7).
- **4.5 Watcher.** `notify` events, 2 s debounce; in-process ignore matching cached against
  .gitignore mtimes — no subprocess per event (S12). Freshness layer 3: stat-read-stat with
  retry (S17), connect-time (size,mtime)+hash sweep. Per-response staleness banner for
  pending files (CodeGraph).
- **Gate:** kill −9 mid-build replays (B1); migration kill-matrix clean (S2); FTS fuzz clean
  (S3); readers unblocked during build (B4, WAL snapshot test); index <60 MB on DevCouncil;
  1-file resync writes <100 rows; 30-min multi-session burst keeps freshness with zero
  manual unlocks.

---

## Phase 5 — Query surface & artifacts (REQ-RP-5 · crates: devmap-query, devmap-cli)

Objective: token-budgeted, confidence-honest answers; artifacts that admit what they are.

- **5.1 `Budgeted<T>`** as the only response wrapper (R7). Ranking before truncation, with
  confidence ordering extracted>inferred>ambiguous so truncation drops noise first (G24).
  Tri-state dependents: `None` = unknown, rendered `resolution: "unavailable"` — never an
  empty list (X14).
- **5.2 Tools** per PHASE1_CONTRACT budgets: `search` (FTS + later RRF), `deps`, `impact`
  (kernel semantics, parametric depth — G8), `trace`, `dead` (honest hidden counts — V12),
  `manifest` (≤2k tokens, replaces repo_map — T1), `status`. Route/shape analysis computes
  its scan once per query (G9) with the G15 span guard. No Cypher shim (per
  PHASE1_CONTRACT; N10/G11 retire).
- **5.3 Precompute.** Entry-point→leaf process flows at build time; symbol queries return
  participating flows with step indices (PLAN §7.4). `min_confidence` defaults conservative.
- **5.4 Manifest + CLAUDE.md.** Emit the ≤2k-token manifest; draft the CLAUDE.md § replacing
  "open repo_map.json" with "query the map" (do not edit the live CLAUDE.md until Phase 6).
- **5.5 Artifacts.** All writes tmp+rename (V14); every page/JSON stamps
  `{generated_head, built_at, fingerprint}` and renders a staleness banner. Subsystem map:
  deterministic layered blueprint layout, no physics (CodeGraph). Symbol explorer: payload in
  `<script type="application/json">` per mode (V2), legend/filters derived from payload with
  per-kind colors (V3), size tiers + confirm gate >5k nodes (GitNexus), one `esc()` at every
  sink with the hostile-name fixture (V1), badges show "N of M" (V13).
- **Gate:** explore/dead p95 <50 ms at 10k files; no response exceeds its budget; every
  truncation reconciles shown+hidden=total; manifest ≤2k tokens; hostile-name fixture renders
  inert; artifact regeneration skipped when fingerprint unchanged.

---

## Phase 6 — Cutover (REQ-RP-6)

Objective: the 37 consumers on the daemon; core Python analysis deleted.

- **6.1 Thin client.** `src/devcouncil/devmap_client.py` (~400 LOC): typed request/response,
  socket transport, auto-spawn `devmap serve`. No analysis logic — reject any PR into the
  client that computes anything.
- **6.2 Migrate consumers** in dependency order (CONSUMERS.md), verification checks last.
  Each consumer keeps its existing tests; the six private-internal reaches get supported
  APIs. `dev map` CLI: human summary on TTY, `--json` for the full payload (V16).
- **6.3 Shadow soak.** Both systems live on real work; parity harness diffs every build until
  quiet for two weeks, all diffs either fixed or ledgered.
- **6.4 Delete** `indexing/` and `codeintel/` **except** the Phase 7 adjuncts (PDG, LSP,
  debug/DAP — per PHASE1_CONTRACT they are not cutover-blocking). Update CLAUDE.md (5.4
  draft). Publish binaries per platform; deprecate `devcouncil-codeintel-grammars`.
- **Gate:** all 16,534 LOC of surviving tests green against the client; core Python analysis
  gone; token cost of the map surface ≤2k default; no consumer imports `indexing.graph` or
  `codeintel.store`.

## Phase 7 — Adjunct ports & final deletion

Per project direction the port is *full*: the "keep in Python for now" items from
PHASE1_CONTRACT land here, each against its audit spec, then the last Python goes.

- **7.1 PDG** in devmap-analyze on a correct CFG contract: entry/exit threading (G1), leader
  lines owned by blocks (G2), block-scoped line ordering (G13), parameters as sources +
  exact-match sinks (G23), content-hash tie to generation (no stale PDG). Batch shard reads
  (G10).
- **7.2 Semantic snapshots** in devmap-query: no arbitrary 500-symbol cap — budgeted with
  truncation flags (X16); imports from extraction, not regex (X17); per-language public
  rules (X18).
- **7.3 LSP adjunct** (optional — cut if unused): async client with separate write lock
  (X12), init timeout = failure (X11), negotiated position encoding (X13), tri-state results
  (X14). Decide keep/cut from real usage telemetry during the soak; record the decision.
- **7.4 Delete the remainder** of `indexing/` + `codeintel/`; the parity harness archive and
  `DIVERGENCES.md` stay as the historical record.
- **Gate:** G1/G2/G13/G23 fixtures pass; zero Python analysis code remains; 117-property
  suite green in CI.

---

## Continuous obligations

- `STATUS.md` current every session (§0). `DIVERGENCES.md` for every deliberate difference
  (R3). `CONSUMERS.md` updated when a contract is touched.
- CI runs: fmt, clippy `-D warnings`, workspace tests, determinism double-build diff,
  hostile-name fixture, migration kill-matrix, FTS fuzz — from Phase 2 onward.
- The 117-property suite is append-only: a finding's test, once green, never gets deleted or
  weakened without a `DIVERGENCES.md` entry.
- When blocked or when this plan conflicts with PLAN.md/AUDIT.html, stop and record the
  conflict in `STATUS.md → Open questions` rather than choosing silently (R10).
