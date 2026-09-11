# INTEGRITY — independent verification report (2026-08-12)

Independent static audit of the full workspace (every `.rs` line, both manifests, lockfile,
all test files) against AGENT_PLAN ground rules and the 117-property spec. Performed by a
separate auditor session while the build agent was active; line numbers reference the tree as
of 2026-08-12 ~08:25.

> **Snapshot notice (updated 2026-08-12):** the defect and vacuous-test tables below are the
> disruption baseline, not the current defect count. Post-audit work addressed D1–D9 and
> D11–D16 and strengthened T1–T9; see `STATUS.md` for the current command evidence. D10 still
> needs durable repository-root resolution, D17 still needs a persisted unresolved-call ledger,
> and the grammar, B3, manifest-trends, soak, platform, and mutation gates remain open. The
> schema-v6 build-history table and `devmap history` CLI described below are now implemented.

## Verdict

**What this audit confirms:** the structural rules mostly hold — determinism (R4),
parse-outcome honesty (R6), budget honesty (R7), output escaping (R8), atomic migrations
(S2), FTS hygiene (S3), durable pending queue (B1), and deletion reconciliation (N2) all
PASS with cited evidence. The single language authority (X8) holds with one caveat.

**What no audit can confirm today, and this one does not claim:** "all bugs resolved" is
false while 29 of 35 languages return `ParseOutcome::Failed` (open blocker, needs grammar-dep
approval), B3's <100-row write gate is unclaimed, the migration kill-matrix / WAL
reader-benchmark / soak items in STATUS.md remain open, and 17 defects below are unfixed.
Integrity is *tracked and provable*, not *complete*. Anyone claiming otherwise is selling
something.

## Compliance matrix

| Rule | Verdict | One-line evidence |
|---|---|---|
| R4 determinism | PASS | all emit paths BTree/sorted with tiebreaks; double-build tests exist |
| R5 confidence | PARTIAL | fan-out + SPECULATIVE correct; but `Unresolved` never constructed (no ledger), and **no edge dedup at all** |
| R6 parse outcomes | PASS* | Clean/Partial/Failed + has_error walk; Failed never cached; *no parse timeout modeled; defect D7 |
| R7 budgets | PASS* | {shown,hidden,total,truncated,tokens_used} computed by the truncators; *StoreQueryEngine SQL-LIMITs in bm25 order before re-rank |
| R8 escaping | PASS | single sinks html_escape/json_script_escape; hostile-name + `</script>` breakout tests real |
| S2 migrations | PASS | IMMEDIATE tx per step, user_version inside tx, fail-closed future versions |
| S3 FTS | PASS | quoted-phrase wrapping at all 3 MATCH sites; two fuzz corpora |
| S9/S11 connections | FAIL | **no busy_timeout anywhere** (default 0 → instant SQLITE_BUSY under contention); foreign_keys never enabled |
| B1 queue | PASS | delete only after commit, attempts-guard, real kill-9 subprocess test |
| B3/N2 | N2 PASS / B3 OPEN | deletion reconciliation at 3 layers; carry-forward still O(repo) rows |
| X8 authority | PASS* | languages.rs frozen vs Python registry; *treesitter.rs has a 6-grammar dispatch subtable (29 documented-unavailable) |

## Defects (fix in this order)

| # | Sev | Where | Defect | Fix direction |
|---|---|---|---|---|
| D1 | HIGH | resolver.rs:209 | Receiver rung dead: looks up `"{Type}::{method}"` but index only holds `"{file}::{name}"` / `"{file}::{Type}::{fn}"` — N6 constructor tracking never resolves | index methods under type-qualified keys at extraction (requires class-nested Python methods, see D-note below) or translate type→defining-file before lookup |
| D2 | MED | resolver.rs:135-143 | Ambiguity guard vacuous: maps every candidate to the same string, set size always 1 — 10 same-named classes still "unique" | collect `(file,kind)` of candidates, not `reference.name`; expect at :146 then removable |
| D3 | MED | extract_cache.rs:22-33 | Cache never avoids parsing (parses first, then looks up by the result's own hash); admit errors swallowed with `.ok()` | hash source bytes before parse; propagate admit errors |
| D4 | MED | daemon.rs:43-48,153-162 | One oversized/unreadable file poisons its whole ≤64-path batch → collective retry → collective quarantine | per-path skip classification; only the offending path fails/quarantines |
| D5 | MED | db.rs:276-284 | No `busy_timeout`, no `foreign_keys=ON` at open | `PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;` in `Store::open`; add cross-process contention test |
| D6 | MED | liveness.rs:17-27,76-83 | Ambiguous-only callees reported dead at confidence 0.9 non-exempt — "maybe called" becomes "confidently dead" | new tier: ambiguous-only inbound ⇒ confidence ≤0.4 with reason `only_ambiguous_callers` |
| D7 | MED | treesitter.rs:115-118 | Route-matcher error rewrites a clean parse as `ParseOutcome::Failed` | route errors are diagnostics on a Clean outcome, never a parse failure |
| D8 | LOW | treesitter.rs:557 | `require()` zero-arg extracts `)` as module specifier | guard `args.named_child_count()` |
| D9 | LOW | treesitter.rs:172 | unbounded AST recursion (stack overflow risk within 1 MiB cap) | explicit worklist or depth cap → Partial |
| D10 | LOW | engine.rs:41 | stored repo-relative paths read against CWD → silent empty spans off-root | resolve against a stored repo root; report `source_unavailable` instead of "" |
| D11 | LOW | treesitter.rs:24 | `DefaultHasher` persisted as cache key — unstable across Rust releases (safe-direction invalidation, undocumented) | switch to a pinned hash (e.g. xxh3/sha256 prefix) or document |
| D12 | LOW | db.rs / manifest.rs | `head_sha:"head"`, `pending_count:0` hardcoded — freshness surface decorative | see "Insights over time" below; wire real HEAD + pending count |
| D13 | LOW | db.rs save | `latest_analysis()` returns `dead_symbols: []` by design; nothing enforces pairing with `latest_dead_symbols()` | type-level: summary field becomes `dead_symbol_count`, not an empty list |
| D14 | LOW | daemon.rs:319, protocol.rs:300-307 | fixed `/tmp/devmap.sock` (multi-user collision); 0600 chmod after bind | per-user runtime dir (`$XDG_RUNTIME_DIR` / `~/Library/Application Support`); pre-set umask |
| D15 | LOW | protocol.rs dispatch | sync rusqlite + fs reads inside tokio handlers stall the reactor | `spawn_blocking` around store/query calls |
| D16 | LOW | all crates | `thiserror` declared 7×, used 0×; errors shoehorned into `rusqlite::Error::InvalidParameterName` | per-crate error enums (R9), or drop the dep |
| D17 | LOW | resolve/model | `Resolution::Unresolved` never constructed — unresolved calls silent | emit + persist ledger rows (R5: "never silence") |

Also: unused deps (`regex`/`rayon` in resolve+analyze, `tracing` in 5 crates, `ignore` in cli)
— run `cargo +nightly udeps` or prune manually. Dead `"async_function_definition"` arm
(treesitter.rs:268). CLI `manifest` ignores its `path` arg (main.rs:330). `build --affected`
still extracts the whole tree.

## Vacuous tests — the disruption audit found tests that cannot fail

These pass regardless of whether the logic they name works. Strengthen exactly as stated;
then mutation testing (below) keeps them honest.

| # | Test | Why vacuous | Required assertion |
|---|---|---|---|
| T1 | test_store.rs:41 deletion_reconciliation… | asserts only generation numbers; comment claims absence check | assert f2.py nodes/edges absent from gen2 query results |
| T2 | test_phase3_hardening.rs:265 constructor_assignment… | passes via same-file rung; receiver rung is dead (D1) | move callee to a *different* file so only the receiver rung can resolve it |
| T3 | test_g5_no_multicandidate_extracted / test_g6… | assertions inside `if let Some(edge)` — pass when no edge found | assert the edge/candidate set exists first (`let edge = ….expect()`) |
| T4 | test_g3_stdlib_guard_python_only | asserts only `!edges.is_empty()` | assert the stdlib-named call resolves for py candidates and is absent cross-family |
| T5 | store_hardening.rs:515 test_b4_reader_unblocked… | reader+writer share one Mutex<Connection> — proves no deadlock, not WAL concurrency | second `Connection` on its own handle; assert reader latency bound while writer holds a long tx |
| T6 | test_hardening.rs:711 test_stress_scale | comment says 2 s, asserts <10 s; dead_count<2000 unfalsifiable | tighten to measured p95 + margin; assert exact dead set on a seeded fixture |
| T7 | test_stress.rs test_large_repo_stress | prints timings, asserts none | assert build < ratchet, query p95 < 50 ms, RSS if obtainable |
| T8 | test_extraction.rs:53 test_adversarial_inputs | echo-only smoke | assert each adversarial case's ParseOutcome variant explicitly |
| T9 | test_findings_suite.rs:16 test_g8_parametric_depth | `deep >= shallow` also true if depth ignored | assert strict node-set inclusion + a node present at depth 3 absent at depth 1 |

## Disruption protocol (pre-deployment)

1. **Mutation testing** — the systematic form of "disrupt existing logic": run
   [`cargo-mutants`](https://mutants.rs) per crate (`cargo mutants -p devmap-resolve …`),
   budget 30 min/crate. Every *missed* mutant in resolver/liveness/db/engine is either a new
   test or a documented exclusion. T1–T9 above are exactly the tests mutants will sail
   through until strengthened. Gate: 0 unexplained missed mutants in
   resolve/analyze/store/query core modules.
2. **Fault-injection matrix** (STATUS.md already tracks some as open — they remain required):
   migration kill-at-every-statement; kill-9 mid-batch (exists — keep); WAL reader-under-
   long-writer with a real second connection (replaces T5); watcher stat-read-stat race;
   SQLITE_BUSY storm (two processes hammering after D5 lands); disk-full during
   save_generation (tmpfs quota); clock skew on retry backoff.
3. **Hostile-input corpus** (extend existing): zero-byte file, 1 MiB−1 boundary, UTF-16/BOM,
   10k-deep nesting (bounds D9), symlink loop, path with `"` `'` `<` `%` `?`, filename at
   NAME_MAX, git checkout mid-build.

## Stress plan

Scale ladder 1k → 10k → 50k synthetic files (mixed tiers, realistic import fan-out), each
run asserting: cold build ratchet, 1-file resync row count (B3 gate once closed), query p95
< 50 ms, peak RSS < 2 GB, index size per file, determinism digest equality across two runs.
Then the 30-minute burst soak (STATUS.md open item) with randomized edit/delete/rename mix
at 2–10 s intervals, zero manual unlocks, freshness lag p95 < 5 s.

## Insights over time (progress indicator)

The per-build `[n/5]` stage reporter exists (main.rs, TTY-gated). What's missing is the
*longitudinal* view. Spec:

1. **`build_history` table** (schema v6): `generation_id, built_at, head_sha (real), files,
   symbols, edges, dead_confident, dead_ambiguous (post-D6), parse_failed, languages_covered,
   build_ms, resync_ms, db_bytes, quarantined`. Written in the same transaction as the
   generation commit. Real `head_sha` from `git rev-parse HEAD` (closes D12); real
   `pending_count` in the manifest.
2. **`devmap history [--last N] [--json]`**: table of generations with deltas
   (Δsymbols, Δedges, Δdead, Δbuild_ms) and sparkline-style trend markers on TTY; feeds
   dashboards via `--json`.
3. **Manifest gains a `trends` block** (≤150 tokens): last-5-generation deltas so agents see
   drift ("dead_confident +12 since yesterday") without a second query.
4. Retention: prune with generations but keep a capped 500-row history (history rows are
   ~100 B; exempt from generation prune).

## Verification pipeline

`./verify.sh` (repo root of rust-port, requires local Rust toolchain — the audit sandbox has
none, so **compile-level confirmation must run on the host**):
fmt → clippy `-D warnings` → `cargo test --workspace` → double-build determinism digest →
release build of DevCouncil itself + size/time gates → (optional) `cargo mutants` on changed
crates. CI should run the same script.

---
*Auditor session artifacts: this file, verify.sh, STATUS.md pointer. No source files were
modified — the build agent owns the tree; defects D1–D17 and tests T1–T9 are its worklist.*

## Build-agent remediation follow-up (2026-08-12)

The original findings above remain the independent point-in-time record. Afterward, the build
session added red regressions and closed D1–D9, D11–D13, D15–D16, deterministic edge dedup, and
all T1–T9 vacuity findings. D10 now reports source unavailability explicitly but durable source
snippets remain open. D14 now has repository-scoped short defaults and pre-bind length validation;
caller-supplied sockets outside the managed runtime directory still receive `0600` immediately
after bind. D17 remains schema-bound and open.

Host verification after remediation: 166 Rust tests; fmt and clippy `-D warnings`; 88.88% line
coverage; deterministic graph digest `528f9bb1ab5e4fec02a4514827aabf867d75e8dfc2315bcc72666b583d0ce846`;
release self-build 2.656 s and 52 MiB; 150 focused Python cutover tests; two identical 121-file
snapshot digests `c3af36d772f6212202c155435402066674f3f4c1c4285726a5198311b22d0499`.
Mutation testing, the migration kill matrix, the 30-minute soak, and Windows process CI remain
unrun and must not be inferred from these local passes.
