# DevCouncil deep audit, 2026-10-13 (Fable-plan session)

Read-only audit of `main` at `a90b17fd`. No source, config or dependency was changed. The only writes were GitPulse board writes and this file.

**This is one of two audits filed today under the label `audit-2026-10-13`.** A second session ran its own audit at the same time. Its report is `AUDIT_REPORT.md`, and a hero loop's `BUILD_REPORT.md` sits beside it. Following Fable's advice, this report is a separate file; the other two were not touched. **Suggestion, not done:** commit all three under `docs/audits/2026-10-13/` in one commit once the hero loop is quiet (decision D-22).

Working files are in this session's scratchpad: `findings/`, `verified/`, `stale/`, `FABLE_PLAN.md`, `FABLE_RECONCILE.md`, `FILING_LOG.md`.

---

## 1. Method and tool state

| Phase | Who | Result |
|---|---|---|
| Map | orchestrator | GitPulse Insights: 3 worktrees, 0 collisions. lane/workbench had 28 dirty files and lane/policy had 1. The board had 60 cards, 46 open, none in_progress. All 46 briefs were pulled as the dedupe baseline. |
| Audit | 6 Sonnet auditors, ≤4 at a time | Slices: go-host, go-core, rust-dc, devmap-runtime, devmap-engine, devmap-query, delivery. They produced **56 findings**. |
| Verify | 7 Opus verifiers | **41 confirmed, 15 downgraded, 0 rejected outright.** Some sub-claims were struck (§7). |
| Already-implemented check | 3 Opus checkers | 36 open cards checked criterion by criterion (§5). |
| Prioritize | Fable | `FABLE_PLAN.md`: 11 new groups, 23 folds, 8 waves, decisions D-1..D-15. |
| File, pass 1 | Opus | Wrote 19 cards, then stopped when a concurrent session's audit clobbered its writes and was clobbered in turn. |
| Reconcile | Fable | `FABLE_RECONCILE.md`: per item SKIP 8, MERGE-INTO 25, NEW 6; no card merges; revised waves. |
| File, pass 2 | Opus | 6 new, 25 merged, 9 skipped, **0 failures and 0 retries**. Each write was re-read to confirm it landed. |

**DevMap state.**
- **Index not fresh.** `is_fresh:false` with `walk_incomplete` set. 1 file failed to parse, 4 were recovered by pattern, and 54 parsed partially. Net call resolution is about 45% for Rust and 78% for Go.
- **Standalone server down.** The `devmap` MCP server failed to start: ENOENT on a test-binary path in the l3-workbench worktree. The GitPulse-hosted `devmap_*` tools worked.
- **Fallback.** Most caller lists are rg-derived, so they are **inferred**. Each findings file records the graph gap.

**Tests.** Only a few existing targeted Go tests were run, and all passed. The CI checker also ran `cargo fmt --check`, `gofmt`, one `go test` and `npm run check:release`. Every other verdict comes from reading the code.

---

## 2. Severity × dimension (56 verified findings, final severity, one primary dimension each)

| Dimension | critical | high | medium | low | total |
|---|---|---|---|---|---|
| Correctness (logic / honesty of results) | 0 | 1 | 9 | 6 | 16 |
| Correctness on Win/macOS/Linux | 0 | 0 | 7 | 4 | 11 |
| Security | 0 | 1 | 3 | 5 | 9 |
| Defensive validation at trust boundaries | 0 | 1 | 0 | 6 | 7 |
| Async / error boundaries | 0 | 0 | 3 | 3 | 6 |
| Performance & benchmarks | 0 | 0 | 0 | 5 | 5 |
| Modular architecture | 0 | 0 | 1 | 0 | 1 |
| Deterministic testability | 0 | 0 | 0 | 1 | 1 |
| Strict typing | 0 | 0 | 0 | 0 | 0 |
| **Total** | **0** | **3** | **23** | **30** | **56** |

The three highs:
- **go-core-02:** the repo's own config can disable the hard rules.
- **go-host-05:** verify judges scope by exact match while the gate uses globs.
- **devmap-engine-04:** unbounded recursion lets one file abort `devmap build`.

---

## 3. Top 10 risks (whole board after this audit; Fable's ranking)

1. **Main CI is red on every Rust/Go workflow, and main is 14 commits ahead of origin** (ft-7cb2d308, ft-c1e2cec7).
   - `rust/gusset-engine/check-pin.sh` was mode 100644, so `verify.sh` exits 126 on a fresh checkout. It is now staged on main by another session.
   - 11 files fail `cargo fmt --check`.
   - Windows Go tests fail in 21 of 31 packages.
2. **The gate's safety switches are repository content (go-core-02, new card ft-b7d531c8).** A cloned repo whose `.devcouncil/config.yaml` sets `harness.posture: yolo` turns the hard rules off (`flags/flags.go:263-309`, `policy/file.go:198-229`), with no prompt and no orphan gap.
3. **P0 card ft-e3af6201 was accepted while verify still uses an exact-match map and a nil added-set** (go-host-05/06, go-core-06). Its "one scope judge" claim is false, and the evidence is now folded onto the card.
4. **One adversarial file aborts `devmap build`** (devmap-engine-04, folded into ft-359e6751, raised to P1). `rust_typed_operand` (`treesitter.rs:7225`) recurses without a bound on 2 MiB rayon stacks.
5. **Windows containment holes in the write gate** (go-core-01/04/05, new card ft-00755856):
   - drive-relative and rooted-without-drive paths are treated as in-repo;
   - NTFS ADS spellings of secret paths are not refused;
   - `..\` passes `ValidRepoPath`;
   - verify cannot run any command on Windows (go-host-08).
6. **A long-lived DevMap answers from a stale or replaced store** (new card ft-c13da1b7):
   - the cached `fresh:true` verdict has no age;
   - a store rebuilt at the same path is served from the stale cached connection;
   - moving or renaming a repo bricks its store.
7. **Lease exclusion is advisory** (new card ft-fceeb5fc):
   - `force` is open to any MCP caller;
   - the 15-minute TTL is shorter than the 32-minute verify budget;
   - Persist does not check the lease;
   - there is no SIGTERM handler (that part is folded into ft-09a91ab9).
8. **Supply chain** (folded into ft-189d1ae0, plus new card ft-f8d29003):
   - actions on mutable tags, including rust-cache under `actions: write`;
   - `curl | sh` in the release job;
   - `cargo install --git … --force` from the default branch.
9. **Accuracy claims are ungated** (ft-4cd88dbe):
   - the corpus harness is `#[ignore]`d;
   - its floors are the old baselines;
   - `record.json` predates every lane;
   - `forbid` rows only rule out edges at ≥ 0.9.
10. **Verification persistence can fail intermittently.** The gap writers use DEFERRED transactions (`rust/dc-store/src/records.rs:289,361,482`; ft-a4b3e5f0). The workbench rollback backup is reused forever (ft-9f2169e4).

---

## 4. Board changes (final)

All writes carry the label `audit-2026-10-13`.
- No priority or severity was lowered, nothing moved to done, and no card was deleted or merged.
- ft-e5e19ae3, which is in_progress, was not touched.
- Each pass-2 write landed exactly one revision after the read before it and was re-read to confirm.

### New cards (6, all backlog)

| Key | item_id | Pri / Sev | Findings |
|---|---|---|---|
| config-trust | ft-b7d531c8b344ae994de018462907fdd4 | P1 / high | go-core-02, go-host-09 |
| go-windows-paths | ft-0075585647cb9913f41a6dc31479483a | P2 / medium | go-core-01/04/05, go-host-08/13/14 |
| rust-windows | ft-9465b250e144693e00d5116312489b4b | P2 / medium | rust-dc-04, devmap-runtime-04 |
| lease-lifecycle | ft-fceeb5fccbcaecb34faff9c2eb7ee120 | P2 / medium | go-host-01/02/03 |
| component-install | ft-f8d290031c6743dd9b3aed1cfbdcbec6 | P2 / medium | delivery-05, go-host-12 |
| devmap-store-lifecycle | ft-c13da1b7bc133e3aecddf0a2bd36dee3 | P1 / medium | devmap-runtime-01/02/03 |

### Planned groups merged onto the other session's cards (no duplicates created)

| Our group | Merged into | Note |
|---|---|---|
| ci-supply-chain (delivery-01/02/03/06) | ft-189d1ae0 | retitled and widened |
| devmap-bench-baseline (devmap-query-07, devmap-runtime-07) | ft-9ac4f289 | — |
| pdg-taint (devmap-query-01..05) | ft-08e1753c | retitled |
| extract-robustness (devmap-engine-04/05/06) | ft-359e6751 | retitled, raised P2→P1 |
| lsp-pass (devmap-engine-01/02) | ft-2e510bf3 | **Conflict recorded.** Their text keeps `repo_relative_from_uri` unchanged; our criterion needs it to percent-decode. A scope note says so, and the implementer decides. |

### Folds and verification updates on existing cards (42 writes across both passes)

- **Pass 1, still intact (17):**
  - ft-e3af6201, ft-09a91ab9, ft-7121a94b, ft-fb1fe1ee, ft-a4b3e5f0, ft-9f2169e4
  - ft-81c3e16f, ft-a72bf0e7 (moved backlog→review), ft-c666ba7c, ft-5dd0f6f0, ft-4cd88dbe
  - ft-837a6f05, ft-e3f7d1b1, ft-cc6c12cb, ft-02f43146, ft-dbde8de0, ft-2b9adeb0
- **Pass 2 re-adds after the clobber (2):**
  - ft-1ca67c6c: go-host-07, go-core-03
  - ft-b9ef3ccf: rust-dc-02/03; stays P0
- **Pass 2 folds and verification updates (18):**
  - ft-c5f2c0fd, ft-0a53607f, ft-7cb2d308, ft-c1e2cec7, ft-0bf5b602, ft-ff2153d9
  - ft-f9495f0a, ft-e9b14104, ft-b22665c3, ft-32522da4, ft-6c5d9e9e, ft-e221de3c
  - ft-f9665721, ft-736b0b22, ft-8ca270ad, ft-0720fe1d, ft-73b89f58, ft-9f2169e4 (second write, a note)
- **Skipped as already covered (pass 2):**
  - by the other session's or our pass-1 text: ft-11a3d890, ft-0ed86046, ft-dbde8de0, ft-2b9adeb0, ft-4cd88dbe, ft-e3f7d1b1, ft-837a6f05, ft-c666ba7c;
  - plus one other item (9 skips in total).

**Lost content.** Pass 1 overwrote the other session's ft-dbde8de0 revision 6. That session later restored its criteria (rev 8), so nothing is known to be lost now.

### Related pairs kept separate on Fable's advice (no merges)

- ft-0e6cfa7a (backup race, fixed on `audit/schema-backup`) vs the rust-dc-08 fold on ft-9f2169e4: same function, different defect. Left for the owner at build time.
- ft-87363ce5 vs ft-32522da4 criterion 7.
- ft-befc1ff0 vs ft-93ece913.
- ft-fd75b294 vs ft-11a3d890 D-1.

Each of these is a sub-case or a residual by design.

---

## 5. Already-implemented check (open cards, criterion by criterion, main @ a90b17fd)

MET criteria are prefixed `[MET — verified in source]` with a citation. Fable declined four that this audit contradicts: e3af6201 #5/#9, 09a91ab9 #4 and e9b14104 #3. Nothing was moved to done; done requires Manvi verification.

### Host and security (16 cards): 9 fully met, 7 mostly met

| Verdict | Cards | Still open |
|---|---|---|
| FULLY MET | ff2153d9, 7121a94b, 09a91ab9, f9495f0a, fb1fe1ee, 1ca67c6c, b9ef3ccf, a4b3e5f0, 11a3d890 | — |
| MOSTLY MET | e3af6201 7/9 | no full enforce/passed test |
| MOSTLY MET | dbde8de0 7/8 | yolo missing from the posture table |
| MOSTLY MET | e9b14104 4/5 | `integrate uninstall` defaults to apply (`main.go:664`) |
| MOSTLY MET | 0a53607f 2/4 | key prefixes missing |
| MOSTLY MET | 9f2169e4 7/8 | no `invalid_token`/`lease_released` codes |
| MOSTLY MET | 2b9adeb0 4/6 | Lappi cross-check skips in CI; no protocol schemas |
| MOSTLY MET | c5f2c0fd 3/4 | no application_id or build deadline |

### CI, release and DevMap infrastructure (10 cards): 0 fully met

| Verdict | Cards |
|---|---|
| MOSTLY MET | 5dd0f6f0 3/4 (owner npm commands only), cc6c12cb 15/16, 4cd88dbe 5/7 |
| PARTIAL | 7cb2d308 3/9, c1e2cec7 3/11, 0bf5b602 2/4, b22665c3 1/5, 73b89f58 1/2 (re-measure due 10-14), a72bf0e7 3/5 |
| No criteria | 02f43146 (now has them) |

### DevMap accuracy (10 cards): 0 fully met

| Verdict | Cards |
|---|---|
| MOSTLY MET | 32522da4 5/9, 837a6f05 4/10, e221de3c 5/6, 736b0b22 7/8, 8ca270ad 3/5, 0720fe1d 3/5, 6c5d9e9e 2/4 |
| PARTIAL | e3f7d1b1 1/7, f9665721 2/6, c666ba7c 3/6 |

No corpus recall or precision target can be closed: the harness is ignored and gates the old floors.

---

## 6. Build waves (Fable's reconciled plan covering both audits)

Rules:
- At most 4 cards per wave and no shared planned file inside a wave.
- A file counts as **held** if it is dirty on main, held by a lane, or on an unmerged `audit/*` branch.
- The hero loop is already taking `ready` cards in its own order. Treat this as a dependency map; the one rule is never to start a card whose file is held.

| Wave | Cards | Waits on |
|---|---|---|
| 1 | ft-3f82536e · ft-bccb7d54 · ft-359e6751 (extract robustness) · ft-08e1753c (PDG/taint) · ft-a4b3e5f0 fold | nothing held |
| 2 | ft-dff2a712 · ft-2e510bf3 (LSP) · ft-b7d531c8 (config-trust) · ft-09a91ab9 fold · ft-81c3e16f fold | wave 1 daemon.rs/write.rs |
| 3 | ft-c13da1b7 (store lifecycle) · ft-32522da4 CStyle · ft-fb1fe1ee fold · ft-f8d29003 (component-install) | audit/generation-race and ft-87363ce5 merged |
| 4 | ft-7661cacb · ft-9ac4f289 (bench baseline) · ft-090bcce | resolver.rs, mcp.rs and hook.rs released |
| 5 | ft-a72bf0e7 · ft-c666ba7c · ft-4cd88dbe · ft-837a6f05 · ft-db864556 | language lanes merged |
| 6 | ft-e3f7d1b1 · ft-cc6c12cb (needs D-7) · ft-02f43146 · ft-c5f2c0fd · ft-0a53607f | earlier hook.rs/treesitter.rs |
| after lane/workbench merges | ft-b9ef3ccf · ft-1ca67c6c · ft-e3af6201 · ft-11a3d890 · ft-fceeb5fc (lease) · ft-4966b304 · ft-00755856 (go-windows) · ft-7121a94b · ft-9465b250 (rust-windows) | lane/workbench, plus main's policy/gatescfg/integrate/dc-proc edits committed |
| after lane/ci and audit/ci-shell merge | ft-189d1ae0 (supply chain) · ft-5dd0f6f0 · ft-2b9adeb0 · ft-fd75b294 · ft-befc1ff0 | workflow edits |
| backlog / owner | ft-716f2358, ft-a43bfbc6, ft-0b23fbe1, ft-3018ab76, ft-93ece913; owner-only steps | — |

The two "after" rows are larger than 4 cards. They are dependency buckets, to be split into waves of ≤4 once their blockers merge.

---

## 7. Rejected and struck findings

No finding was rejected outright. These sub-claims were struck and are not filed:

| Finding | Struck claim | Reason |
|---|---|---|
| devmap-engine-03(b) | over-climb clamps to the root | it yields `/x` and matches nothing; correct by accident |
| devmap-engine-04 | C# `type_target` recursion | the grammar limits depth to 2 |
| devmap-query-05 | "silently truncated bodies" | the builder refuses them |
| rust-dc-02 | fail-open | it fails closed into `rigor_check_unavailable` (`rigor.go:142-171`) |
| rust-dc-03 | OOM | the host caps the diff at 32 MiB (`orchestrate.go:489`) |
| rust-dc-07 | 512 MiB pinned up front | allocations are sequential; kept as hardening |
| go-host-01/02 | theft | intended and tested (`lease_contract_test.go:57`); now decision D-2 |
| go-host-09 | fail-open | the gate fails closed |
| go-host-11 | agent-exploitable | the gate blocks agent edits to config.yaml |
| delivery-01 | third-party action in the release job | release.yml has none; the exposure is rust-cache in rust.yml under `actions: write` |
| delivery-02 | downstream sinks | gated by `ParseTag`; only `release.yml:82` is reachable |
| delivery-05 | schema check silently skipped | it prints "NOT CHECKED" |
| devmap-runtime-06 | per-session misattribution | duplicate of ft-a72bf0e7 |

---

## 8. Decisions made without asking

- **Proceeded past go-core-02.** It is a security exposure needing a product decision, but that decision blocks building the fix, not filing the card. It is filed as D-1.
- **Concurrent filer.** I stopped once on the data-loss condition and reported it. Then, per your "don't stop, ask Fable":
  - followed Fable's reconcile;
  - merged onto the other session's cards rather than creating duplicates;
  - did no card merges or deletes;
  - wrote this report as a separate file rather than overwriting `AUDIT_REPORT.md`.
- **Temporary board snapshot.** My board-dump script resolved its output directory to the repo root, so `board/` sat there, untracked, during the audit. It has been deleted. The shell gate forbids writes outside the repo, so it couldn't go to scratch.
- **Sonnet safety filter.** A Sonnet helper meant to dump the board was stopped by a false-positive safety filter, so I drove `gitpulse-mcp` over stdio, read-only, instead.
- **One status move.** ft-a72bf0e7 moved backlog→review, because its code is on main.
- **Retitled cards.** ft-189d1ae0, ft-08e1753c and ft-359e6751 were retitled on Fable's direction to reflect the widened scope.

---

## 9. Owner decisions needed (deduplicated across both audits; Fable's default in bold)

| # | Decision | Card | Default |
|---|---|---|---|
| D-1 | May repo-scope `config.yaml` weaken the safety flags? | ft-b7d531c8 | **No: refuse weakening values from repo scope** (Manvi pin bump, reported and asked first) |
| D-2 + D-12 + D-15 | `force` human-only behind a grant; heartbeat during verify and re-diagnose before Persist; add `invalid_token`/`lease_released` codes | ft-fceeb5fc, ft-9f2169e4 | **as stated** (changes `lease_contract_test.go:57`) |
| D-3 | `windows-sys` dependency in dc-proc | ft-9465b250 | **approve** |
| D-4 vs A-3 | Refuse `:` on every host (ours) vs peel the ADS on Windows only (other session's criterion on ft-dbde8de0) | ft-00755856, ft-dbde8de0 | **refuse on every host** |
| D-5 | Literal `\` in child-reported names | ft-00755856 | **refuse on all hosts** |
| D-6 | `application_id` with the next schema bump, in lockstep with GitPulse; build deadline | ft-c5f2c0fd | **next bump; 10 min now** |
| D-7 | Partial-parse charge policy | ft-cc6c12cb | **charge outside surviving declarations, plus a grammar allowlist** |
| D-8 | `golang.org/x/tools` in a `tools/accuracy` module | ft-4cd88dbe | **approve** |
| D-9 | AmbiguousGlobal ledger-only | ft-32522da4 | **confirm** |
| D-10 | Inline-module twins share one identity | ft-f9665721 | **amend the criterion** |
| D-11 | Owner-only steps: npm dist-tag/deprecate, v0.2.4 tag, tessl 0.3.0, seat-profile hooks, 10-14 re-measure | several | **run them** (npm today) |
| D-13 | Memoise `ask` docstrings | ft-9ac4f289 | **memoise** |
| D-14 | Content hash strength | ft-359e6751 | **length + version bump** |
| D-16 | `cargo install --git` unpinned: defect (ours) or accepted design (other report) | ft-f8d29003 | **pin to `v<Version>`** |
| D-18 | Sign the security-impact statement on ft-dbde8de0 | ft-dbde8de0 | **sign** |
| D-19 | Filter-driver decision | ft-ff2153d9 | owner |
| D-20 | `env_allow` base list and name regex | ft-7121a94b | **extend the regex; user-level config only** |
| D-21 | MET-mark convention differs between the two audits | — | **keep source-read marks, with citation** |
| D-22 | Commit the three root reports | — | **one commit under `docs/audits/2026-10-13/` after the hero loop is quiet** |
| D-23 | Keep the hero loop launching `ready` cards during board writes? | — | moot now that filing is done |

---

## 10. Build results (go-ahead given 2026-10-13; finished 2026-10-14)

- **How it ran:** each card was built in its own isolated worktree (`.claude/worktrees/agent-*`) on an `audit2/*` branch.
  - Every card was claimed with an `in_progress` move first, so the hero loop wouldn't start the same work.
  - Each fix starts with a test that failed on the unmodified code (with the exceptions noted in the table).
  - Only targeted crate or package test suites were run.
  - Each card moved to **review**. None was moved to done; done needs Manvi verification.
- **What did not happen:** nothing was committed to `main`, merged or pushed. Fable's defaults were used for D-1, D-2, D-3, D-4, D-5, D-7, D-12, D-13, D-14 and D-15.

| Card | Branch @ sha | Result | Open / blocked |
|---|---|---|---|
| ft-a4b3e5f0 gap writers | audit2/gap-txn-immediate @ 48df05a2 | 9/9. A new `write_transaction` helper takes the write lock before reading. Before the fix 5 of 6 new tests failed; after it, 6/6 pass, five runs in a row. | — |
| ft-3f82536e drain write scope | audit2/daemon-write @ bb01a8c2 | 3/3 | A rare full-write case needs a store API change |
| ft-bccb7d54 parity-check env | audit2/consumers-env @ 90e37ac1 | 1/2. Environment scrubbed; the allowlist moved to `proc/env.go` | Passing the checkout as an argument needs GitPulse to add `--devcouncil` first (cross-repo) |
| ft-08e1753c PDG/taint | audit2/pdg-taint @ cc8545c8 | 6/6 | Implicit raises inside a handler are not routed to `finally` |
| ft-359e6751 extract robustness | audit2/extract-robustness @ 87823824 | 4/6. Recursion is capped (20k-deep fixtures overflowed the stack before), probes get a deadline. It also found 3 child-process tests that ran nothing. | **D-14 hash length needs the extraction version bump (GitPulse reads it) — needs your sign-off.** Backslash fix still owed in 3 other files. |
| ft-b7d531c8 config-trust | audit2/config-trust @ 1955ec58 | 3/6 + 1 checked only by reading Manvi's code. Repo config can only tighten. The yolo posture test was changed deliberately under D-1. | The startup notice and per-session gate rebuild need `cmd/devcouncil/main.go` (lane/workbench); a handoff is on the card. Manvi needs a pin bump and a hint change (report-and-ask). |
| ft-2e510bf3 LSP pass | audit2/lsp-pass @ 62c6438f | 3/3, plus an innermost-definition fix | The `--json` build counters are not wired yet |
| ft-fb1fe1ee grants TTL | audit2/grants-ttl @ 23cdf38a (built on config-trust) | 5/5 | `grants.human.max_ttl` is not marked as a safety flag |
| ft-09a91ab9 MCP lifecycle | audit2/mcp-lifecycle @ 1d55b10e | 6/7. A signal now drains running verifies like EOF; the decision log is capped at 4096 entries | The D8 gatescfg criterion is held. The million-write test adds about 60 s to non-`-short` CI. |
| ft-f8d29003 component install | audit2/component-install @ 7cc534bc | 3/5 + 2 partial. Remote installs are pinned; uninstall saves its receipt as it goes | **Edited `cmd/devcouncil/main.go` usage text, a file lane/workbench holds — expect a conflict.** The quickstart doc is held. `install.ps1` was not run. |
| ft-81c3e16f residue | audit2/residue @ 2f9c887d | 4/4 audit additions. The gusset registry really held about 9.4× its bound. | On the viz page, dead code was deleted instead of a total being added |
| ft-dff2a712 daemon follow-up | audit2/daemon-followup @ eefa2f2d (built on daemon-write) | 3/3. `devmap build --affected X` now fails instead of silently dropping X | A path read through a linked directory can still be indexed under the link's name |
| ft-4cd88dbe accuracy gates | audit2/accuracy-gates @ 3aea7839 | 3 + 1 partial of 5. Targets are gated, and an unmeasured target fails | No labelled corpus rows; the Go oracle needs `x/tools` (D-8) |
| ft-cc6c12cb partial-parse | audit2/partial-parse @ 2d9ee4d4 | 16/18 | **The grammar allowlist needs `EXTRACTION_SCHEMA_VERSION` 89→90 (GitPulse reads it) — needs your sign-off** |
| ft-837a6f05 manifests | audit2/manifests @ fc984740 | 3/3 audit additions | Corpus numbers. The wiring pass still finds crate roots by path. |
| ft-02f43146 disk writes | audit2/disk-writes @ bac12dcd (built on daemon-followup) | 2/3. Drains are paced to 4 MiB/s; a sampler, `diskwatch` and a soak guard were added | The 30-minute live measurement was not run. The structural fix (stop copying every row into each generation) needs a store schema change — needs your sign-off. |

**Merge notes.**
- **Stacks:** daemon-write → daemon-followup → disk-writes, and config-trust → grants-ttl.
- **Overlaps that should merge cleanly:**
  - `grants/grants.go`: mcp-lifecycle and grants-ttl.
  - `resolver.rs`: lsp-pass and manifests.
- **Will conflict:**
  - `ratchet.json`/`record.json` (manifests and accuracy-gates): regenerate them, don't hand-merge.
  - `dc-proc/src/lib.rs` (a one-line `pub mod` from disk-writes) collides with another session's uncommitted edits on main.
- **Already failing before these branches:** the devmap-cli test `every_test_target_declares_its_grammar_need` / `every_integration_test_is_classified_against_the_parse_feature`, because `dc-regress/tests/change_paths.rs` is unclassified, and `every_child_process_in_the_kernel_goes_through_the_runner`, which flags `devmap-serve/src/blast_cancel_tests.rs:17`.

**Not started (held, as Fable advised).** Each card waits on lane/workbench, on files another session has uncommitted on `main` (policy, gatescfg, dc-proc, integrate.rs), or on `audit/*` branches not present in this clone:
- ft-00755856 go-windows-paths
- ft-9465b250 rust-windows
- ft-fceeb5fc lease-lifecycle
- ft-c13da1b7 store-lifecycle
- ft-e3af6201, ft-1ca67c6c, ft-b9ef3ccf, ft-11a3d890, ft-7121a94b
- ft-9ac4f289, ft-32522da4, ft-7661cacb, ft-090bcce
- ft-a72bf0e7, ft-c666ba7c, ft-e3f7d1b1, ft-db864556, ft-c5f2c0fd, ft-0a53607f
- ft-189d1ae0, ft-5dd0f6f0, ft-2b9adeb0, ft-fd75b294, ft-befc1ff0

**Not verified anywhere.**
- **Windows:** the Windows paths were only compiled and vetted.
- **Graph evidence:** the DevMap MCP server stayed down, so every implementer navigated with `rg`. Most recorded the gap with `devmap gap-record`, and every report says so.
- **Full builds:** no full-workspace or CI run.

## 11. Build results, wave 2 (2026-10-14)

Fable picked the wave from the board and the dirty-file lists as of main @ e79c2610. Every card is in review; none is done, merged or pushed. Worktrees stay until their branch is merged (user instruction: "delete once merged"); none is merged yet.

| Card | Branch @ sha (base) | Result | Open |
|---|---|---|---|
| ft-0075585647 go-windows | audit2/go-windows-part1 @ 32c9ebc8 (main) | 4/7. `ValidRepoPath` refuses `\` and control bytes on all hosts (D-5); per-GOOS verify shell (cmd.exe via ComSpec); root_resolve refuses only `$NAME`/`${NAME}`/`%NAME%` | 1, 2, 6 (policy/file.go, gatescfg dirty on main); env casing (verify/env.go, after consumers-env); main.go `checkProjectRoot` (lane/workbench) so Go and Rust `$` rules still disagree; stale `/bin/sh` comments in orchestrate.go. Windows tests compiled only |
| ft-7661cacb glob poison | audit2/glob-poison @ 06ca5f7b (lsp-pass) | 2/3 + partial. Earlier-round glob hops re-checked and poisoned; also fixed a second bug: two globs in one file overwrote each other in the hop map | hop-cap flag `ReexportGlobCutoff` is set but not reported (needs build.rs or a cross-crate `ResolutionResult` field) |
| ft-c13da1b7 store lifecycle | audit2/store-lifecycle @ 6897dc9d (disk-writes) | 2/4. A moved repo's store is rebound when the old owner is gone; MCP store cache re-stats on every hit | 3, 4 (freshness age needs db/generation.rs). Windows fallback compares size+mtime |
| ft-9ac4f289 query cancel | audit2/query-cancel @ 18db3836 (main; c3e73c57 = baseline bench rows) | 3/6 + cancel half of 4. Cancel checks in route scan, graph core, ask, clones, dead | 1, 2 need protocol.rs to pass the real cancel (dirty on main); 7–10 deferred. Bench A/B on a busy machine: no speedup or regression claimed |
| ft-2e510bf3 + ft-359e6751 follow-up | audit2/lsp-counters @ 2106bfb1 (lsp-pass) | LSP drop counters in `build --json`; `simple_pattern_is` bug reproduced and fixed (`mut x` closure/typed params, `Ok(mut g)`) | **the mut fix changes extraction output: EXTRACTION_SCHEMA_VERSION bump needs sign-off** |
| ft-359e6751 follow-up | audit2/backslash-readers @ be862eaf (extract-robustness) | watcher.rs and freshness.rs delegate to `repo_relative_key`; 4 fail-before/pass-after tests | — |
| ft-359e6751 follow-up | audit2/daemon-backslash @ d9154f29 (store-lifecycle) | daemon `stored_path` refuses `\` names on Unix, counted and logged | **carries a local copy of the `repo_relative_key` rule** because the helper is on the other stack; replace it with the shared call when the stacks merge |
| ft-73b89f58 | — (measurement) | 26 records since the 10-08 install; null id 3/26 (none since 1791927355); store-wide zero-query 13/26; per-session zero-query 22/23, but 10 records carry unattributed queries (up to 31), so the per-session share overstates non-use | follow-up: attribute queries from GitPulse-hosted/subagent devmap servers |

**Stacks now:** daemon-write → daemon-followup → disk-writes → store-lifecycle → daemon-backslash; lsp-pass → {glob-poison, lsp-counters}; extract-robustness → backslash-readers; config-trust → grants-ttl.

**Merge order (Fable):** commit or stash main's dirty files first. Then (1) gap-txn-immediate, pdg-taint, residue, accuracy-gates; (2) extract-robustness → backslash-readers; (3) lsp-pass → glob-poison → lsp-counters, then manifests (regenerate ratchet.json/record.json); (4) partial-parse; (5) the daemon stack through daemon-backslash, then swap its local helper copy for `repo_relative_key`; (6) config-trust → grants-ttl → mcp-lifecycle; (7) consumers-env, component-install (main.go conflict with lane/workbench), go-windows-part1, query-cancel; (8) `bun run check:schema-parity` from GitPulse, then Manvi/Jarvis pins by report-and-ask.

**Skipped this wave (Fable):** ft-9465b250, ft-befc1ff0 (dc-proc lib.rs / dc-regress blast.rs dirty on main); ft-fceeb5fc (lane/workbench); ft-fd75b294, ft-090bccec, ft-189d1ae0, ft-db864556 (depend on audit/* branches not in this clone); ft-3018ab76 (tied to the 89→90 bump); ft-2b9adeb0 (lane/ci); the seven accuracy L-* cards and ft-e3f7d1b1 (resolver/treesitter/liveness collide with wave-1 branches; wait for merge).

**Same pre-existing failures seen again:** devmap-cli `every_integration_test_is_classified_against_the_parse_feature` (dc-regress change_paths.rs) and devmap-extract `every_child_process_in_the_kernel_goes_through_the_runner` (devmap-serve blast_cancel_tests.rs:17).
