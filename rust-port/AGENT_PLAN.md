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

## Consolidated open-work register (2026-09-02)

**Why this section exists.** Open work was spread across eleven documents in two workspaces,
and the same item appeared in three of them with different status. This is the single place a
worker reads to know what is left. It is an **index, not a copy**: each row names the document
that owns the detail, and that document stays authoritative. Adding a twelfth checklist would
have been the duplication this repository's rules forbid, so this extends the file that
already owns execution rather than standing beside it.

### Document map — who owns what

Read the owner before working an item; do not restate an owner's content here.

| Document | Owns | Do not use it for |
|---|---|---|
| `rust-port/PLAN.md` | Design, the 47 findings + `K1`–`K8` + `G1`–`G8`, §3.1 failure classes, §9 MCP/plugin surfaces, §10 worst-case scenarios | Execution order or progress |
| `rust-port/STATUS.md` | Phase progress, verification evidence, the 11 required decisions | Design rationale |
| `rust-port/AGENT_PLAN.md` (this file) | Execution: order, ground rules, proof obligations, **this register** | Design, findings, evidence |
| `rust-port/INTEGRITY.md` | The independent audit's `D1`–`D17`, `T1`–`T9` | Current status — see STATUS.md |
| `rust-port/DIVERGENCES.md` | Every deliberate difference from the Python baseline (R3) | Bugs |
| `rust-port/CONSUMERS.md` | The 37 downstream contracts | Anything else |
| `rust-port/PHASE1_CONTRACT.md` | Token budgets, §4.5 scoping | Progress |
| `rust/STATUS.md` | The **separate** analysis-plane port (`dc-glob`/`grep`/`store`/`verify`) | The devmap kernel |
| `IMPROVEMENTS.md` (root) | Dated narrative of landed work | A to-do list |
| `rust-port/PLAN.html`, `AUDIT.html` | Nothing authoritative — styled snapshots of earlier revisions, no generator, not in sync with their `.md` counterparts | Anything current |

**Two Rust workspaces exist and they are not duplicates.** `rust-port/` is the devmap
code-intelligence kernel; `rust/` is the analysis plane ported from MANVI on 2026-09-01. They
share no crates. `rust/STATUS.md` §1 records why the second workspace was not folded into the
first — read it before proposing a merge.

### Handoff (2026-09-02, evening) — start here

This is the state a new contributor inherits. Verify it before building on it:

```bash
DEVMAP_AUTOSPAWN=0 .venv/bin/dev map doctor --json      # expect ok: true (run `dev map` first if freshness warns)
.venv/bin/python -m pytest tests/unit -q -p no:cacheprovider   # ~4 min; totals at the end of this section
cd rust-port && cargo test --workspace                   # 835 passed on 2026-09-02
```

**Done and verified (evidence in root `IMPROVEMENTS.md` → second pass, and `STATUS.md` → "Kernel audit, second pass"):**

1. The Rust kernel is the only writer of the map artifacts. Every caller goes
   through `indexing.map_artifacts.refresh_map_artifacts`; no Python fallback.
2. Kernel defects `K1`–`K7`, `K12`, `K13`, `S1`–`S10` fixed with failing-first
   tests; release binary rebuilt (`rust-port/target/release/devmap`, schema 12).
3. Seam: binary selection, socket-path parity, probe-safe status, event-driven
   watcher, bounded query fan-out, guides rebuild, fail-closed artifacts.
4. Diagnosability: `dev map status|doctor|runs|abort`, `doctor --fix`, MCP
   `devcouncil_graph_doctor` / `devcouncil_graph_runs`, `devmap_run` trace
   events, live build marker. Tests: `tests/unit/test_devmap_diagnostics.py`,
   `tests/unit/test_devmap_seam_hardening.py`, `tests/stress/test_devmap_stress.py`.
5. Docs: `docs/code-graph.md`, `docs/cli-reference.md`, `CONSUMERS.md`,
   `DIVERGENCES.md` (`K5`, `K7`, `SEAM-1`, `SEAM-2`), `PLAN.md` §3 pointer.

**Interrupted — the Python writer retirement (an Opus subagent was stopped at its
final verification step to save usage limits).** Its working-tree changes are
*kept*, uncommitted, and are the state to continue from:

- Deleted: `src/devcouncil/codeintel/sync/{coordinator,incremental,scope}.py`,
  `src/devcouncil/codeintel/build_worker.py`; `codeintel/sync/__init__.py`
  reduced to the lease exports.
- Rewritten: `codeintel/build_control.py` (−598 lines: `run_isolated_full_build`
  and its worker helpers gone; `graph_build_session`, `unlock_writer_lease`,
  `read_build_status` stay), `indexing/map_refresh.py` (−128:
  `refresh_repo_map_from_graph` gone), `indexing/graph/build.py` and
  `indexing/graph/__init__.py` (`refresh_map_for_paths` gone),
  `cli/commands/hook.py` (post-tool-use refresh calls the kernel seam),
  `integrations/mcp/server.py` (lifespan warms the kernel daemon; no Python
  watcher), `integrations/mcp/handlers/codeintel.py` (`_sync` has no Python
  fallback), `codeintel/query/engine.py` (`sync` in the envelope comes from the
  kernel), `integrations/mcp/handlers/map.py`.
- Tests rewritten or deleted accordingly: `test_codeintel_runtime_branches.py`,
  `test_coverage_wave7.py`, `test_coverage_wave8.py`, `test_coverage_wave9.py`,
  `test_graph_incremental.py`, `tests/integration/test_codeintel_platform_runtime.py`,
  `tests/performance/{benchmark_harness.py,test_codeintel_benchmark.py,thresholds.json}`.
- The agent's last message before it was stopped: it had one remaining unit-test
  failure it believed pre-existing and was proving that with file copies. The
  suite totals at the end of this section are the authoritative state.

**Next steps, in order:**

1. Run the unit suite; for each failure decide: retired expectation (delete the
   test and say why in `IMPROVEMENTS.md`) or a real regression of the
   retirement (fix at the owner). Two deletions need a two-signal proof before
   they are trusted: `RIPGREP_CONFIG_PATH= rg -uu -n '<name>' --glob '!*.pyc' .`
   and `DEVMAP_AUTOSPAWN=0 .venv/bin/dev map query <name>`.
2. **Done in the same evening, after the agent was stopped:** `RepoMapper.map_repo`, the
   Python graph builder, `indexing/graph/resolve.py` and `extract_ts.py` are gone; the
   fixture-style tests moved to `tests/unit/support_maps.py`; the two breakages the
   interruption left (`build_code_graph` re-export, `_files_fingerprint` import in
   `compute_freshness`) are fixed. Details and the kernel-coverage gaps in `IMPROVEMENTS.md`
   → "`RepoMapper.map_repo` and the Python graph builder are gone".
3. ~~Item 6 of the retirement brief may be open: `dev map --pdg` rewrites
   `code_graph.json` without the kernel's `meta.map_engine` stamp and
   `dev map doctor` then reports `foreign_writer`.~~ **Refuted by measurement,
   2026-09-04 — this does not reproduce and needs no fix.** Run on a scratch
   repo: `dev map` (kernel) → `meta.map_engine == "devmap-rust"`,
   `doctor --json` `ok: true`; then `dev map --pdg` → `map_engine` is still
   `"devmap-rust"`, `meta` keys are
   `[compatibility_export_tier, devmap_rust, liveness_unreachable_unreliable,
   map_engine, pdg]`, and `doctor` is still `ok: true`. The kernel's `meta`
   survives because `load_code_graph` imports the compatibility artifact (which
   carries it) when the Python store is empty, and `write_code_graph` re-exports
   it intact. Do not spend a session on this.
4. Move the remaining Python-cache query surfaces to the kernel (MCP
   `graph_query` / `graph_trace` / `graph_context` first — they are what agents
   call), then delete `codeintel/query`, `indexing/graph/build.py`'s import path
   and the cache itself.
5. Toward gortex parity (`STATUS.md` → "Gaps vs. gortex"): tiered map reads for
   token economy; ship `devmap` with the tool install; grammar breadth; an
   embedding index; cross-repository contracts; diff-scoped review.

**Conventions that held today and should keep holding:** every fix ships with a
test that fails first; delete only with two independent proofs of no callers;
no new dependency without asking; `DEVMAP_AUTOSPAWN=0` in every test shell;
never run `devmap build` against the repository root's store from a test.

### Standing warnings — read before touching anything

1. **R1 (the freeze on `src/devcouncil/indexing/`, `codeintel/` and `cli/commands/map.py`)
   is superseded as of 2026-09-02.** The cutover it protected has happened: the kernel is
   the only writer, and those trees are being *deleted*, not preserved. Treat the Handoff
   section above as the authority on what in them is live. The earlier note about
   `embeddings.py` / decision 3 is moot — the embeddings module was removed with the
   Python engine.

2. **`dc-verify` stub detection is a capability regression.** `rust/STATUS.md` §4: Python's
   `stub_detector.py` does an AST parse with per-language idioms; `rigor::detect_stubs` does
   substring matching over added diff lines. Cutting this over **weakens a gate**. The honest
   first cutover is `dc-store`, where interop is proven and semantics are identical.

3. **A second session may hold this tree.** During the 2026-09-02 pass a concurrent session
   refactored `devmap-extract`, `devmap-analyze`, `devmap-store` and `devmap-query` while work
   was in flight. Two full-suite runs failed for reasons that were not the code — a test file
   rewritten mid-run, and shared `target/` artifacts vanishing under a build. Both passed on
   retry. **Check `git status` and file mtimes before assuming a failure is yours**, and
   re-run a suite before treating it as a gate (W6 in PLAN.md §10).

4. ~~The working store is nine schema versions stale.~~ **Stale as of 2026-09-05:** the store
   the seam opens is `.devcouncil/codeintel/devmap.sqlite` (`devmap_client.DEFAULT_DB_PATH`), and
   `find_engine_binary` no longer ranks candidate kernels by mtime — it probes each one's
   `devmap status` `n` (the schema it writes), keeps only the capable, and prefers a release
   build among equals, because mtime was wrong in both directions (a newer release build can
   carry an older schema, and two builds at the same schema differ in mtime for no reason that
   should decide anything). The measurement rule stands for a different reason: **never
   benchmark against this repository's own live store** — build and settle a copy, and see
   STATUS.md's dated 2026-09-05 sections for the corpus procedure.

5. **Two sessions carried the same audit on 2026-09-05, and `main` is the integration
   line.** A second session implemented many of the same findings directly on `main` while
   branch `claude/dev-map-performance-hardening-1a2151` (commit `d2fb25e`) implemented them
   independently. `d2fb25e` is kept intact as the *record* of the second set and is not
   merged; branch `claude/devmap-reconcile-1a2151` is `main` plus the pieces of it `main`
   lacked, ported one at a time. **Before re-implementing anything from
   `AUDIT_KERNEL_2026-09-05.md`, read that file's "Where these fixes live" paragraph and
   STATUS.md's "Port of the 1a2151 round-1 work onto main" section**: several findings look
   open on one line and closed on the other, and the fix for each already exists somewhere.
   Where both lines had an implementation, exactly one was kept — never leave two.

### Open work

**What closed on the reconcile line (2026-09-05).** Detail, tests and numbers in STATUS.md's
"Port of the 1a2151 round-1 work onto main (2026-09-05)" section; the commits are on
`claude/devmap-reconcile-1a2151`.

| Item | Commit |
|---|---|
| One owner for `MAX_TOKEN_BUDGET` / `MAX_TRAVERSAL_DEPTH` in `devmap-query` | `1a7fd98` |
| CLI argv bounds (`validate_limits`) and one-line `--json` on every exit | `d6ac670` |
| Per-generation edge index (`devmap-store/src/edge_index.rs`), replacing the per-question read; `impact` 18.6 ms -> 0.95 ms and 73.2 ms -> 3.04 ms p50, `status` 866 µs -> 15 µs and 2.77 ms -> 13 µs | `1dcbc69` |
| K-B1: a search page is bounded in files and in bytes | `2f9916a` |
| Admission control across the three transports, plus an HTTP body deadline | `177abbd` |
| K-B4: the drain batch's claims are indexed once instead of scanned per path | `3425d76` |
| E-8: `recover_lock`, and grammar mutation fuzzing | `0306e9e` |
| One owner for uncached tree extraction and for the refusal rule | `4795d23` |
| Seam: one kernel invocation per map refresh, skip-on-unchanged artifacts, kernel-side freshness digests (`hook.py` import cost 149 ms -> 31 ms) | `035fe1e` |
| `devmap build --manifest` and `devmap freshness` | `3231018` |

Status is one of: **open** (not started), **partial** (started, gated), **decision** (blocked
on a human). Nothing here is "done" — closed items live in STATUS.md's dated sections.

#### A. Correctness and capability — devmap kernel

| ID | Item | Owner doc | Status |
|---|---|---|---|
| SC34 | Call-graph blackout across ~26 non-C languages | STATUS.md | **closed 2026-09-05** — 11 languages closed earlier; `erlang`, `pascal`, `solidity`, `shell`, `sql`, `nix` closed in this pass; `svelte`/`vue`/`astro`/`liquid` closed by embedded `<script>` extraction. `hcl` and `cfml` **declined with reasons and pinned by tests**; `cobol` stays unlinked (`UNSAFE_GRAMMARS`) and `vb.net` still has no grammar (decision #1). Orphaned call edges 0 for every language added |
| SC14 | Nested-symbol identity collisions inside anonymous callbacks and nested types | STATUS.md | partial |
| SC29 | Unexplained SC26 per-file memory increase | STATUS.md | open |
| SC2/B3 | Write amplification + genuinely incremental resolve | PLAN.md §7.5 | **decision** (#2) |
| SC4 | Collapse speculative ambiguous calls into one edge with a candidate set | STATUS.md | **superseded 2026-09-05** — the fan-out is not collapsed (no winner is picked) but its *emission* is bounded: audit R-7 capped one call site at `AMBIGUOUS_FANOUT_CAP` (16, `devmap-resolve/src/model.rs:244`) edges, each carrying candidate total and cap in `details`, with the full candidate list kept on the `Resolution`; pinned by `audit_regressions.rs::an_ambiguous_fanout_is_capped_and_says_so` and `adversarial_resolve.rs::every_ambiguous_candidate_names_an_indexed_file`. Decision #7 is answered by that bound |
| — | VB.NET: the one frozen `LANGUAGE_SPECS` entry with no linked grammar | STATUS.md | **decision** (#1) |
| G4 | Speculative edit preview — "what breaks if I change this" without writing | PLAN.md §3 | **closed** — `devmap preview` ships with `crates/devmap-cli/tests/test_preview.rs`, so the R2 objection below is answered. The paragraph that follows is retained only as the record of why it was once fenced off |
| G6 | Compact wire format — `code_graph.json` is 84 MB / ~21M tokens on a 3,674-file corpus | PLAN.md §3 · `DIVERGENCES.md` X39 | **closed 2026-09-05** — `devmap manifest --compact-graph-output`; interned encoding of the same model from the same traversal, 21,186,034 B -> 5,001,998 B (-76.4%) on this repository, round-trip asserted. Note it does **not** make the graph agent-readable (5.3M tokens -> 1.25M), and **nothing reads it yet** — the intended first consumer is `backend/go_orchestrator/repomap` |
| G7 | Savings accounting — tokens saved versus reading the files | PLAN.md §3 | **already closed** — `devmap savings` exists and PLAN.md §3 records it closed; this row was stale |

**Superseded 2026-09-05 — G4 is closed and this paragraph is history, not instruction.** `crates/devmap-cli/tests/test_preview.rs` exists and passes, so the "ships no tests yet" objection no longer holds; do not read the "do not start" below as current. The original note follows.

> **G4 was being built by another session as of 2026-09-02 15:23.** `devmap-query/src/engine.rs`
carries a `preview()` entry point and a `PreviewChange` enum; `devmap-cli/src/main.rs` carries
a `Preview` subcommand and `emit_preview`. It compiles and the suite is green, but it ships no
tests yet, so it does not meet R2. **Do not start G4.** Either take it over from that session
deliberately, or leave it. What it still needs, whoever finishes it: tests that fail against
the pre-fix code, and a decision on the unsigned-body case the current code already flags —
when one side has no `body_signature`, nothing compared the bodies, and "unchanged" would be a
claim rather than a finding. That is the Class A distinction (§3.1) and the existing code
appears to get it right; it is untested either way.

**G7 — savings accounting: what to build.** The claim is "answering from the graph cost N
tokens; reading the files would have cost M". Both numbers must be *measured*, never modelled.

- `Response<T>` already carries `tokens_used`; extend that struct rather than adding a
  parallel accounting type. It is the canonical owner of per-response cost.
- The denominator is the sum of the byte lengths of the distinct files a hit set touched,
  converted at the same `BYTES_PER_TOKEN` the budgeter already uses (`engine.rs`). Reusing
  the budgeter's constant is what makes the two numbers comparable; a second constant would
  make the ratio meaningless.
- **Class A applies.** A file whose size could not be read is not a zero-cost file. The
  denominator must distinguish "measured across all N files" from "measured across the N−k we
  could stat", and never present a partial denominator as a total — that would overstate
  savings, which is the flattering direction and therefore the one to guard.
- **Class C applies.** Savings are derived, so they carry provenance: how many files were
  measured, how many were not, and the conversion used.

**G6 — compact wire format: what to build.** 84,276,217 bytes on a 3,674-file corpus, roughly
21M tokens. Read PLAN.md §3 `G6` and T2 before starting.

- `devmap-query/src/code_graph.rs` owns the export. Extend it with a second *encoding* of the
  same model; do not create a second exporter with its own traversal, or the two will drift
  and one will silently lose a field.
- The existing JSON export stays the interchange format (T2 makes it opt-in and out of the
  context directory). The compact form is an addition, not a replacement — say which is
  canonical in `DIVERGENCES.md`.
- Path interning is the obvious first win: `B10` already interns paths in the store, and the
  export re-expands them to full strings on every node and every edge endpoint.
- **Determinism (R4) is the gate that will catch mistakes here.** Build twice, diff the bytes.
  A compact encoder that iterates a hash map produces a different file each run and no test
  will notice until a consumer diffs two exports.
- Ship a round-trip test: compact-encode, decode, and assert the decoded model equals the one
  the JSON path produces. An encoder without a decoder cannot be checked and must not land.

#### B. Hardening — the failure classes

All five gates are implemented (`coverage_invariants.rs`, `failure_class_gates.rs`, and unit
tests beside `ProgressReporter`). What is left is coverage, not mechanism.

| ID | Item | Owner doc | Status |
|---|---|---|---|
| A-audit | Class A's mechanical audit has been run once (5 candidates, 2 real, both fixed). It is not wired into CI, so it does not run on new code | PLAN.md §3.1 | open |
| E-scope | Class E gates the build profiler; "every measurement carries its provenance" is a standard applied per measurement, not a single task | PLAN.md §3.1 | standing |
| W1–W7 | Seven worst-case scenarios; five have a demonstrated mechanism | PLAN.md §10 | standing |

#### C. Surfaces — MCP and packaging

| ID | Item | Owner doc | Status |
|---|---|---|---|
| MCP-1 | Pin and assert the MCP **protocol** revision. `pyproject.toml:18` is `mcp>=2.0.0,<3`, and the 2.x line carries the breaking 2026-07-28 revision. No compatibility statement exists | PLAN.md §9.1 | **decision** (#10) |
| MCP-2 | Adopt `ttlMs` / `cacheScope` on `tools/list`. Additive, no migration, attacks the same cost as T1/R1 across 18 tools | PLAN.md §9.1 | open — cheapest item here |
| MCP-3 | Compatibility statement recording that none of the deprecated features are used (verified) | PLAN.md §9.1 | open |
| PKG-1 | Plugin packaging target: Claude Code's format vs Agent Plugins 1.0. Targeting both means two manifests for one artifact | PLAN.md §9.2 | **decision** (#10) |
| PY-1 | `assert`-as-guard fragility: `_DB_REQUIRED_TOOLS` is currently complete (26 vs 24, zero gaps) but nothing enforces it, and `assert` is stripped under `python -O` | PLAN.md §9.3 | open |

#### D. Process and evidence

| ID | Item | Owner doc | Status |
|---|---|---|---|
| GATE-1 | Peak-memory gate and external-corpus gate in `verify.sh`. `benchmarks/map_bench.py` supplies the external-corpus half; **nothing measures RSS** | STATUS.md #8 | **decision** (#8) |
| GATE-2 | Workspace-wide mutation coverage beyond the retention surface | STATUS.md | **decision** (#5, cargo-mutants) |
| SOAK-1 | Two-week shadow soak, multi-platform CI, production validation | STATUS.md #4 | **decision** (#4) |
| CUT-1 | Consumer cutover, deletion of `indexing/` + `codeintel/`, platform publication | AGENT_PLAN Phase 6–7 | open |
| PROP-1 | Full 117-property suite (PLAN §3 + AUDIT) | AGENT_PLAN R2 | partial |

#### E. Analysis plane (`rust/`) — separate workspace

| ID | Item | Owner doc | Status |
|---|---|---|---|
| AP-1 | `dc-store` cutover — interop proven, semantics identical; the honest first move | rust/STATUS.md §4 | open |
| AP-2 | `dc-verify` rigor gates as a **second opinion** only, never a replacement, until a differential run says otherwise | rust/STATUS.md §4 | open |
| AP-3 | Differential corpus run for secret scanning — both implementations exist, neither is measured | rust/STATUS.md §4 | open |

### Handoff — state as of 2026-09-02 15:24

Written for a contributor picking this up cold. Everything below was verified by running it,
not inferred from the code.

**Verified state**

| Check | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` | **740 passed, 0 failed**, exit 0 |
| `cargo fmt --all -- --check` | **one file dirty** — see below, deliberately not fixed |
| Workspace builds | yes, debug and release |
| Uncommitted files | **59**, nothing committed |

**Three things to know before your first commit**

1. **Nothing is committed, and the 59 changed files have two authors.** A second session was
   editing this tree concurrently and is *still active* — it wrote
   `devmap-query/src/engine.rs` (+364 lines) and `devmap-cli/src/main.rs` at 15:22 and 15:23,
   building **G4 speculative edit preview** on top of its own clone-signature work
   (`PreviewChange`, `body_signature`). That work is mid-flight: it compiles and the suite is
   green, but it adds no tests yet. **Do not `git add -A`.** Separate the two authors' changes
   before committing, or coordinate with whoever holds the other session.

2. **`cargo fmt --all` will reformat someone else's in-flight file.** The single fmt failure
   is in `devmap-query/src/engine.rs:373`, inside the G4 work above. It was left alone on
   purpose. Run `cargo fmt -p <your-crate>` instead until the tree is quiet.

3. **R1 is violated in the working tree** (see standing warning 1). Awaiting required
   decision 3. Do not build on it; do not revert it.

**What landed in the 2026-09-02 pass** — detail in STATUS.md, design in PLAN.md

- **`K1`–`K8` all closed**, each with a regression that fails against the pre-fix code. The
  two worth knowing: `K1`, grammarless files had *no graph node at all* and so could not be an
  edge target; `K8`, the phase profiler attributed every measurement to the wrong phase, which
  had already propagated into a code comment asserting the wrong phase was a build's largest.
- **All five failure-class gates implemented** — `devmap-cli/tests/coverage_invariants.rs`
  (Class B), `devmap-cli/tests/failure_class_gates.rs` (A, C, D), and unit tests beside
  `ProgressReporter` (Class E).
- **Class A's mechanical audit was run** across the kernel: 5 candidates, 3 false positives,
  2 real. Following them surfaced the larger finding — `AnalysisSummary::status` was the
  literal `AnalysisStatus::Ok` on every path, making `Partial`/`Timeout` unconstructible
  anywhere and their render arms unreachable. The field exists to satisfy N4, whose acceptance
  is "timeout surfaces `status`". Now computed from clustering convergence.
- **B5** — the watcher could not see the checkout move at all, because `.git/` is pruned. It
  now admits `HEAD`, `refs/**` and `packed-refs` and nothing else, and the daemon forces a
  full rebuild when HEAD differs from the generation's `head_sha`.
- **G5** — notebook extraction, with symbols relocated from the reconstructed buffer back into
  the raw `.ipynb` so spans index the file on disk.
- **Performance:** end-to-end `dev map` −57.5%, cold −36.8%, incremental −35.7%. Most of it
  was Python seam overhead, not kernel work.

**Start here**

1. Read the standing warnings above, then `git status` and file mtimes.
2. Pick an item from the register that is **open**, not **decision** — the seven `decision`
   rows are blocked on a human, not on work.
3. `MCP-2` (`ttlMs`/`cacheScope` on `tools/list`) is the cheapest real win: additive, no
   migration, and it attacks the cost of 18 tool definitions paid on every request.
4. Reproduce the baseline before changing anything:
   `cargo test --workspace --no-fail-fast` should give 740/0.

### Documentation debt (2026-09-02)

Found by auditing every doc against the pass, after the code work closed. Recorded because a
doc that contradicts the code is worse than a missing one — a reader trusts it.

| ID | Item | State |
|---|---|---|
| `DOC-1` | `PLAN.html` was a 2026-08-12 snapshot with different structure (8 sections vs PLAN.md's 11) and no generator, while its own `.md` called it "same content, styled" | **closed 2026-09-02** — `tools/render/render_plan.sh` regenerates it from PLAN.md with pandoc, styling preserved in `tools/render/plan_head.html`. Structural parity verified: 11 `<h2>` in both, 25 tables rendered. **Re-run after editing PLAN.md** |
| `DOC-2` | `AUDIT.html` had no `AUDIT.md` in the tree: 84 audit findings existed only as rendered HTML, ungreppable and undiffable | **closed 2026-09-02** — `AUDIT.md` recovered with `pandoc --from=html --to=gfm` (23 headings, 9 sections, 30 tables). It is labelled a **reading copy, not a build source**: AUDIT.html stays hand-authored and authoritative, and is deliberately *not* regenerated, because its masthead and severity chips (1 critical / 21 high / 40 medium / 22 low) cannot be expressed in markdown and a round-trip would silently delete them |
| `DOC-3` | **`CLAUDE.md` / `AGENTS.md` are never regenerated, at either root, while carrying a marker that says they are kept in sync.** Three diagnoses were tried here; the first two were wrong | **open, gated on decision 3.** All four guides are **gitignored** (`rust-port/.gitignore:35`), so they are per-checkout artifacts that no commit can carry: a fresh clone gets whatever `dev map` writes, which is nothing. The generator is therefore the only fix that reaches anyone. The `rust-port` pair on *this* checkout was hand-corrected on 2026-09-02 (its surfaces list read `crates: model, model, model`) with an in-file comment pointing here — that helps agents on this machine and **nowhere else**, and is mitigation, not a fix |
| `DOC-4` | `CONSUMERS.md` did not mention the `devmap_engine.py` seam changes or the Python embedding replacement (K7) | **closed 2026-09-02** — and the audit found more than the row described: the 37-row matrix does not track the seam at all, and it still records `cli/commands/map.py` as frozen under R1 when that file has been modified |

**`DOC-3` in full, because the finding is larger than the row.** Verified 2026-09-02 by running
`dev map` at both roots with the tree quiet:

| Artifact | Written by today's run | |
|---|---|---|
| `.devcouncil/repo_map.json` | 16:40 today | rewritten |
| `rust-port/.devcouncil/repo_map.json` | 16:40 today | rewritten |
| `CLAUDE.md`, `AGENTS.md` | **2026-08-11 23:21** | untouched |
| `rust-port/CLAUDE.md`, `AGENTS.md` | **2026-08-12 11:00** | untouched |

Both maps rebuilt; none of the four guides moved. They are not merely stale — they are frozen,
and they carry `AGENT_GUIDE_MARKER` (`<!-- Managed by dev map: keep this file in sync … -->`),
so each one **advertises itself as kept in sync while never being rewritten**. The marker is
present in all four, which rules out the "don't clobber a hand-authored file" guard in
`write_agent_guides` (`indexing/map_artifacts.py:208`) as the cause.

This matters more than a doc chore: `CLAUDE.md` is loaded into an agent's context
automatically, so it is the most-read document in the repository. Its `rust-port` surfaces
list currently reads `crates: model, model, model` — three identically-named files — and its
subsystem areas (`crates/`, `testdata/`, `tools/`) do not match the freshly built map's, which
are path-level (`crates/devmap-analyze/src`, `crates/devmap-cli/tests`, …). The guide is
describing a map shape that no longer exists.

Not isolated: whether `write_agent_guides` is unreached (an early return before
`map_artifacts.py:464`) or reached and skipping. The equality skip is implausible given the
subsystem shapes above differ. One adjacent smell — `cli/commands/map.py:21` binds
`_write_agent_guides = write_agent_guides` and **never calls it**, a dead alias.

One further fact makes this sharper than a stale-file problem: **all four guides are
gitignored** (`rust-port/.gitignore:35` and the root equivalent). They are never committed, so
a fresh clone has no workspace guide until `dev map` writes one — and `dev map` does not write
one. The stale copies on any given machine are the only ones that exist, and they cannot be
repaired by a commit. Hand-editing them, as was done here on 2026-09-02, fixes one checkout.

Fix is gated on required-decision 3: `map_artifacts.py` is a Python indexing owner under the
R1 freeze, the same freeze `cli/commands/map.py` is already recorded as violating.

Closed in the same audit: `DIVERGENCES.md` gained `X33`–`X37` for the five behaviours this
pass introduced that differ from the Python baseline; `PHASE1_CONTRACT.md` gained the K4
amendment showing its four truncation fields are not sufficient; `PLAN.md` and `README.md` had
their "Python is the live system" and "PLAN.html is the same content" claims corrected.

### How to work an item

Unchanged from §0 and §1; restated only where this register adds an obligation.

1. Read the **owner document** first. This register is an index; it is not sufficient input.
2. Check the standing warnings above, then `git status` and file mtimes (warning 3).
3. **Extend, do not add beside.** Before writing a new module, function or document, find the
   existing owner of that behaviour and extend it. A second implementation next to the first
   doubles the surface even when both are correct — the same rule that made this register a
   section of AGENT_PLAN.md rather than a twelfth file.
4. Every fix ships with a test that **fails against the pre-fix code** (R2). Watch it go red;
   a gate nobody has seen fail is a gate nobody has tested.
5. Record proof in `STATUS.md`; record deliberate differences in `DIVERGENCES.md` (R3).
6. On conflict between documents, stop and record it (R10) rather than choosing silently.

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
