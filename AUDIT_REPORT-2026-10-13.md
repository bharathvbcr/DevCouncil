# DevCouncil audit — 2026-10-13

Read-only audit of this checkout. No source, config, or dependency was changed. The only writes are this file and GitPulse board updates. Nothing was moved to done. Done still needs Manvi verification.

Board before filing: 61 open tasks, `has_more: false`, no `in_progress`. Board after filing: 82 tasks, `has_more: false`, no `in_progress`. Twenty-one new cards, eight folds. Label on every new and folded card: `audit-2026-10-13`.

A second session was filing the same label onto other cards during this pass. It rewrote `ft-dbde8de0` after the first fold and dropped the three new criteria. Those criteria were restored on revision 8, on top of that session’s text. The other seven folds were still at the revisions this session wrote.

## Severity × dimension

Counts are this audit’s confirmed findings, one primary dimension each. Two cards that were already critical stay critical. This audit did not add a new critical finding.

| Dimension | critical | high | medium | low |
|---|---:|---:|---:|---:|
| Security | 0 | 9 | 5 | 0 |
| Correctness (Win / macOS / Linux) | 0 | 4 | 6 | 0 |
| Defensive validation at trust boundaries | 0 | 4 | 2 | 0 |
| Async error boundaries | 0 | 0 | 3 | 0 |
| Deterministic testability | 0 | 0 | 2 | 0 |
| Performance and benchmarks | 0 | 0 | 0 | 2 |
| Strict typing | 0 | 0 | 0 | 1 |
| Modular architecture | 0 | 0 | 0 | 0 |

Security, high: A-1 inline `-e`/`-c`, A-2 `git -c`, B-1 verified with no bytes, B-2 coverage line leak, B-3 same-line secret split, B-6 unscanned binary, D-1 dcgrep secret paths, E-2 hook quoting, H-3 install environment.

Security, medium: A-3 Windows ADS, C-4 brief newline, E-1 hook probe, I-1 plan-supplied shell, I-2 unpinned actionlint.

Correctness, high: B-7 `hasNoHead`, CStyle cross-language UniqueGlobal, F-2 unreadable `sys.path`, H-1 missing task prints success.

Correctness, medium: B-4 split `panic!(concat!)`, D-2 non-Unix reparse, D-3 pathspec magic, F-1 embedded parse dropped, F-4 LSP URI, G-3 Windows immutable URI.

Defensive validation, high: C-3 schema backup race, E-3 Codex plugin overwrite, G-1 generation race, H-2 `dcrelease` join.

Defensive validation, medium: C-1 non-canonical `git_dir`, F-3 absolute `gitdir`.

Async, medium: E-4 route scan, E-5 default `ScanBudget`, G-2 `graph_core_for_store` (one card, with a bench criterion).

Testability, medium: I-3 sparse `truncated: false`, J-1 PDG empty-on-grammar-failure presented as success.

Performance, low: I-4 `terms_of`, J-3 `detect_communities` (one baseline card).

Strict typing, low: J-2 `parent_dir` splits only on `/`.

## Top 10 risks

1. **Verified with no bytes** (`ft-b9ef3ccf`, priority 0, high). Untracked and dirty-submodule names are listed, then only `git diff HEAD` is scanned. A blank body skips rigor without `unavailable`, and the verdict stays verified. In flight on `lane/workbench`.
2. **Policy still fail-open on inline interpreters and `git -c`** (`ft-dbde8de0`, priority 1, critical). `perl -e`, `python -c`, `node -e`, and `git -c` / `--config-env` are not the existing `-i` / `checkout` criteria. The dirty operand walk on this checkout does not close them.
3. **Hook install double-quotes the executable** (`ft-54fcf95e`). `$(...)` and backticks in the binary path still run. Unix detach already single-quotes under absolute `/bin/sh` and `/bin/rm`. That path is not the bug.
4. **Schema backup is check-then-VACUUM outside the lock** (`ft-0e6cfa7a`). Two hosts can both pass `exists()`. The later vacuum can replace the pre-upgrade copy. Filing the card did not touch the database.
5. **`dcrelease` joins `..` into artifacts** (`ft-9bf9409d`). A joined path outside the checkout can still print OK.
6. **Unreadable `sys.path` insert emits no veto** (`ft-87363ce5`). The comment promises a veto. `Vec::new()` means the resolver never sees the row.
7. **Generation check, then prune, then a stored empty read** (`ft-9450b451`). `list_generation_paths` already errors when `EXISTS` is false. The hole is the race after a true check. Unfiltered `search_page` is not this hole.
8. **Secrets still leave the process.** Same-line `concat!` (`ft-0a53607f`, raised to priority 1, high), dcgrep of `.npmrc` / `secrets/**` / `*.pem` / `id_rsa` / `.env` (`ft-11a3d890`), and a raw coverage line in `CoverageError` (same card).
9. **CI runs plan-supplied shell and `curl|bash` with no digest** (`ft-81228493`). Fork-token downgrade and the `GITHUB_PATH` prepend were inferred, not re-read.
10. **Schema-parity install inherits the parent environment** (`ft-bccb7d54`). Scrub the environment and pass the checkout as an argument. A `--gitpulse` flag does not stop the leak. This is not the verify-runner scrub on `ft-7121a94b`.

## Folded cards

| Item | Rev | Status | Priority | Severity | What was added |
|---|---:|---|---:|---|---|
| `ft-dbde8de0b5547abe18cc7a757beeb199` | 8 | review | 1 | critical | A-1, A-2, A-3. Restored after a parallel overwrite. |
| `ft-b9ef3ccf4f898d019658aa5ca4bbd11c` | 7 | review | 0 | high | B-1, B-4, B-6. Priority raised 1 → 0. One criterion marked met. |
| `ft-1ca67c6cbfabbebf3d9805bdb4be71ac` | 7 | review | 1 | high | B-7 `hasNoHead`. |
| `ft-0a53607fddfe1cc625b2b9e8b63704bb` | 4 | review | 1 | high | B-3 same-line split. Priority 2 → 1. Severity medium → high. Logs kept. |
| `ft-11a3d890984b07998d8984e55dd266e0` | 5 | review | 1 | high | B-2 coverage line, D-1 secret paths. `.git` / `.devcouncil` criterion marked met. |
| `ft-32522da4e39ec1cd35c4b311d3a9610a` | 11 | review | 1 | critical | CStyle UniqueGlobal criterion. One `## Planned files` section. Logs kept. |
| `ft-e9b14104e13c5ab58eee80915cdb430e` | 4 | review | 1 | high | E-3 Codex `plugin.json` overwrite. |
| `ft-93ece913a650e0a154c540dac6189e90` | 3 | backlog | 2 | medium | No new finding. Prefix criterion marked met. |

Met marks this audit applied, and only these:

- `ft-b9ef3ccf`: `coverage_unmeasured` non-empty means `coverage_measured: false` with a reason (`rigor.go:184-195`). Tests not re-run.
- `ft-11a3d890`: dcgrep refuses `.git` and `.devcouncil` (`lib.rs:166-178`, `:756-775`). Tests not re-run.
- `ft-93ece913`: diff passes `--src-prefix=a/` and `--dst-prefix=b/` (`change.rs:314-322`, used at `:355` and `:359`). The other ten criteria were not re-verified.

A parallel session marked seven criteria on `ft-dbde8de0` `[MET — verified in source]` and added a `yolo` criterion. This audit left those marks in place and did not re-verify them. They do not cover inline `-e`/`-c`, `git -c`, or Windows alternate data streams.

## New cards

| Wave | Item | Task id | Priority | Severity | Status |
|---|---|---|---:|---|---|
| 1 | `ft-0e6cfa7a04d865dcf84f08b9d54ebdc3` | `audit-2026-10-13-schema-backup` | 1 | high | ready |
| 1 | `ft-9bf9409d9f443e6d8390fac89d131790` | `audit-2026-10-13-dcrelease-join` | 1 | high | ready |
| 2 | `ft-87363ce548f556bd9002826e46a0b574` | `audit-2026-10-13-syspath-veto` | 1 | high | ready |
| 2 | `ft-54fcf95eeebb55a556c36a345545a480` | `audit-2026-10-13-hook-quoting` | 1 | high | ready |
| 3 | `ft-b1a225db8c59b2623242237b2cc20f3f` | `audit-2026-10-13-gitdir-containment` | 1 | medium | ready |
| 3 | `ft-81228493b280cc133be5e05d932f9f36` | `audit-2026-10-13-ci-shell` | 1 | medium | ready |
| 4 | `ft-9450b451787bf1445d8c1794a9d452d3` | `audit-2026-10-13-generation-race` | 1 | high | ready |
| 4 | `ft-15c57b2fd44a54546b0675e9e4675a49` | `audit-2026-10-13-embedded-parse` | 1 | medium | ready |
| 4 | `ft-bccb7d54a122e0457d73fc12cd810c1a` | `audit-2026-10-13-install-env` | 2 | high | ready |
| 5 | `ft-9ac4f2894bdaa64defe9c8d90f430cf2` | `audit-2026-10-13-scan-cancel-bench` | 2 | medium | ready |
| 5 | `ft-befc1ff08b367b69523607d0f381c1ac` | `audit-2026-10-13-blast-pathspec` | 2 | medium | ready |
| 5 | `ft-df74d364c3dfbdc458864d02a781741b` | `audit-2026-10-13-gitdir-identity` | 2 | medium | ready |
| 5 | `ft-6fb77a6fdff742b5eddc4536caa03efa` | `audit-2026-10-13-brief-newline` | 2 | medium | ready |
| 6 | `ft-fd75b294eca3a1dc489d38c489b0cfa1` | `audit-2026-10-13-dcgrep-reparse` | 2 | medium | ready |
| 6 | `ft-e5e19ae39c7223e32b77ec72f10c0a02` | `audit-2026-10-13-sparse-truncated` | 2 | medium | ready |
| 6 | `ft-08e1753c3c6640fa9dc25a79004f5519` | `audit-2026-10-13-pdg-honesty` | 2 | medium | ready |
| 6 | `ft-2e510bf3fe10e5506a0533921ede9997` | `audit-2026-10-13-lsp-uri` | 2 | medium | ready |
| 7 | `ft-716f2358c914c5022616a9bfe5acc6fd` | `audit-2026-10-13-windows-uri` | 3 | medium | backlog |
| 7 | `ft-a43bfbc6ac1ba32e5b0e894281bd0c47` | `audit-2026-10-13-parent-dir` | 3 | low | backlog |
| 7 | `ft-0b23fbe1e5cb586e646edb8630668313` | `audit-2026-10-13-bench-baseline` | 3 | low | backlog |
| 7 | `ft-4966b304c992b0a4b65f63c9e9134ecd` | `audit-2026-10-13-task-status-inflight` | 1 | high | backlog |

`related_tasks_exist` fired because the shared label plus one title word matches many open cards. Each refusal was read by title. None of those cards was the same concern, so each new card was retried with `reviewed_related` listing every id the server named. No card was merged or deleted.

## Verified-stale cards

Not moved to done.

- `ft-b9ef3ccf`, `ft-11a3d890`, `ft-93ece913`: one criterion each, listed above. The review logs that claim the rest landed on `hx/w6-verify`, `hx/w6-hostile` (`ffc90359`), or `d78cd9de` were not marked met.
- `ft-dbde8de0`: the parallel session’s met marks stand as that session wrote them. This audit’s three new criteria are unmet.
- `ft-5dd0f6f0`: the auditor read `go install @latest` as already met. This parent did not re-read `catalog.go`. The card was not marked done. `cargo install --git` without `--rev` stays accepted design. Another session has since edited this card (revision 5).
- `ft-d473c81d` (done): production `dc-proc::git` resolves outside the repo. The card was not reopened. `session_command` was not re-read and was not filed.
- `ft-7121a94b`: a parallel session marked the verify-runner env scrub met and folded a weaker `env_allow` regex into it. This audit did not re-verify that. H-3 stays the separate install card.
- `ft-7cb2d308`: CI-green, Windows, gofmt, and `ci:local` were not re-verified. Nothing on it was marked met.
- `ft-ff2153d9`: filter-driver owner decision stays open. A-2 and D-3 were not folded into it.

## Fable’s waves

No shared files inside a wave. Later waves wait on the files named.

**Wave 1.** `ft-dbde8de0` (A-1, A-2, A-3), `ft-b9ef3ccf` (B-1, B-4, B-6, the only priority 0), `ft-0e6cfa7a` (schema backup), `ft-9bf9409d` (`dcrelease` join).

**Wave 2,** after wave 1, because both verify cards edit `orchestrate.go`. `ft-1ca67c6c` (B-7), `ft-0a53607f` (B-3), `ft-87363ce5` (sys.path veto), `ft-54fcf95e` (hook quoting). F-2 edits `resolver.rs` and must finish before wave 3’s resolver card.

**Wave 3,** after wave 2. `ft-11a3d890` (B-2, D-1), `ft-32522da4` (CStyle, after F-2), `ft-b1a225db` (gitdir containment), `ft-81228493` (CI shell).

**Wave 4,** after wave 3 where noted. `ft-e9b14104` (Codex plugin), `ft-9450b451` (generation race), `ft-15c57b2f` (embedded parse), `ft-bccb7d54` (install env).

**Wave 5,** after wave 4. Shares `engine.rs` and `query.rs` with the generation card, and `protocol.rs` with `ft-93ece913`. Do not build D-3 beside `ft-93ece913`. `ft-9ac4f289` (scan cancel and bench), `ft-befc1ff0` (blast pathspec), `ft-df74d364` (gitdir identity), `ft-6fb77a6f` (brief newline).

**Wave 6,** after wave 3, because D-2 shares `dc-grep` `lib.rs` with D-1. `ft-fd75b294` (reparse), `ft-e5e19ae3` (sparse truncated), `ft-08e1753c` (PDG honesty), `ft-2e510bf3` (LSP URI).

**Wave 7,** backlog. `ft-716f2358` (Windows URI), `ft-a43bfbc6` (`parent_dir`), `ft-0b23fbe1` (bench baseline), `ft-4966b304` (in-flight task status). The last one is `lane/workbench` only. Do not add a `task` command on main. Its planned files exist only on that worktree.

## Rejected findings

- **A-3 device names** (`CON`, `NUL`, `COM1`). The lines that were read do not mention them. Out of scope.
- **B-5** test-path exclusion before the extension list. The comment says that is intentional.
- **B-8** `dcverify` ignoring `runErr` when stdout says `ok: true`. A substituted binary can already exit 0. Dropped.
- **C-2** unique `work_runs_session` with no state filter. That is the one-session-one-run contract.
- **Soak bare `ps`.** `rust/tools/soak` is `publish = false`, `dist = false`.
- **SQL injection in `enhancements.rs`.** Compare and direction are only `<`/`DESC` or `>`/`ASC`.
- **`cargo install --git` without `--rev`.** Accepted design. Not a second copy of `ft-5dd0f6f0`.
- **Cross-line secret splits, headerless PEM, and missing prefixes.** Already on `ft-0a53607f`. Same-line `concat!` is the new criterion.
- **`perl -pi`, `find -exec rm`, `curl -o`, `dd of=`, `git checkout` / `restore`.** Already on `ft-dbde8de0`.
- **dcgrep `.git` / `.devcouncil`.** Met in source. Not refiled.
- **Generation write already one immediate transaction.** Not refiled.
- **FTS total loss.** Still an error on a done card. Not refiled.
- **`ft-d473c81d`.** Not reopened.
- **`session_command` bare spawn.** Not re-read. Not filed.
- **`.github/workflows/ignored-tests.yml` unpinned global `claude-code` install.** Seen and left off the CI card. Widening that card was not in Fable’s grouping.
- **Partial-as-Clean on embedded parse.** Not re-read. The filed criterion is the Failed drop. Include Partial only if a build pass finds it unmarked.

## Decisions made without asking

- `spawn_subagent` has no model argument. Sonnet, Opus, and Fable could not be routed. Slice audits were Grok `code-reader` subagents. The parent re-read the cited lines for the findings that the cards call parent-verified. The advisor pass was a `code-reader` subagent, and its filing plan was followed.
- DevMap was not rebuilt. Gap records were not written. Both would write `.devcouncil`. The index generation was 4441. Official `devmap status` and GitPulse’s copy disagree on freshness. `walk_incomplete` stands, so an empty graph answer is unknown. Supplemental search was a lead, not a DevMap answer.
- No source edits. No merge and no delete of any board card.
- The full board was listed (61, then 82). `gitpulse_get_task` was used on the eight fold targets immediately before writing, and again on the policy card before the restore. It was not called on every open card before the plan.
- This checkout was audited. The workbench worktree was read only to confirm H-1 and that B-1’s hole is still in that dirty `orchestrate.go`.
- C-3, C-4, A-1, A-2, and H-3 were filed rather than stopped. Filing C-3 does not change the database. C-4 rejects newlines in header fields. H-3 scrubs an environment. None of those is a product decision that changes who may access the system.
- Parent downgrades before the advisor pass: C-1, D-2, B-3 (priority 0 → 1), B-5, B-8, J-1, E-1. The advisor then rejected C-2 and B-8, raised `ft-b9ef3ccf` to priority 0, and raised `ft-0a53607f` to priority 1 / high.
- E-1’s first draft said the detach path was unquoted. Re-reading `hook.rs` showed unix detach already single-quotes the executable and uses absolute `/bin/sh` and `/bin/rm`. The filed card says the bug is `hooks_block` double-quoting, plus `Command::new(current_exe())` with no outside-repo check. Scope unchanged.
- A-1’s table says dev, strict, and `yolo`, not `host`. A parallel verification of the same card showed there is no `host` posture and that `yolo` is the posture that demotes. The refusal still has to hold there.
- Allow outcomes for A-1 and A-2 were inferred from control flow. No git probe, policy probe, or test suite was run.
- These sites were not re-read by the parent. The cards say so: `lsp.rs` outbound URI (F-4), `liveness.rs` `parent_dir` (J-2), `fileset.rs` walk cap, `release.go` `MissingArchives`, and the `git_metadata` exclude caller. The filed `fileset` fact is the `truncated: false` assignment. The LSP criterion is Fable’s narrower one: outbound percent-encoding and containment. `repo_relative_from_uri` is out of scope.
- `ignored-tests.yml` was not added to the CI card.
- Related-task refusals were treated as label collisions after the titles were read. The closest pairs were kept separate on purpose: schema backup vs `ft-9f2169e4`, sys.path veto vs `ft-32522da4` and `ft-c666ba7c`, hook quoting vs the SessionEnd / false-positive hook cards, blast pathspec vs `ft-93ece913`, install env vs `ft-7121a94b`, sparse `truncated` vs `ft-0bf5b602`, dcgrep reparse vs `ft-11a3d890`, gitdir identity vs gitdir containment.

## Owner decisions still open

Fable already decided these. They are written on the cards. They are not questions.

- Refuse `git -c` and `--config-env` on every subcommand. No key allowlist.
- Scan untracked and dirty-submodule bytes. If a named path cannot be included, block with `rigor_check_unavailable`. Never report verified on a blank body while names exist.
- Reject CR, LF, U+2028, and U+2029 in title, kind, status, severity, and owner. Do not escape them. Description and criteria stay author body.
- Scrub the schema-parity child’s environment and pass the checkout as an argument.

Still the owner’s, and not decided here:

- Sign the security-impact statement on `ft-dbde8de0`. The change only widens refusals. The card already says the owner signs it.
- The filter-driver decision on `ft-ff2153d9`.
- The existing resolver decision on `ft-32522da4`: ledger-only removes speculative edges from the graph GitPulse reads. This audit did not decide it.
- The verify-runner allow-list on `ft-7121a94b` (a parallel session). Home stays readable, and names such as `DATABASE_URL` still pass the regex. That is not H-3.

## In flight

- This checkout, branch `main`: dirty `backend/go_orchestrator/policy/command_operands.go` and `policy/zfix_w6_operands_test.go`. Findings in those files are in flight. The dirty walk does not close A-1 or A-2.
- `/Users/bharath/Code/devtools/DevCouncil-l3-workbench`, branch `lane/workbench`: holds H-1, and its dirty `verify/orchestrate.go` still has the B-1 / B-7 hole. Do not start that file on main while the lane holds it.
- `/Users/bharath/Code/devtools/DevCouncil-l4-policy`, branch `lane/policy`: policy files. Wave 1’s policy card lands there.

## What was not verified

No test suite, policy probe, or git experiment was run. Met marks are source reads. Parent re-reads that did happen are named on the cards (CStyle, diff prefixes, hook quoting, schema backup call order, `hasNoHead`, coverage measured flag, dcgrep `.git` refusal). The sites listed under decisions as not re-read stay auditor-cited.

## DevMap reliability and performance (second pass)

Same rules as the first pass. No source, config, or dependency edits. The index was not rebuilt. No DevMap gap record was written. Nothing was marked done. No card was deleted or merged.

The store was read this turn, read-only: generation 4459, 1,649 `generation_files` rows, every `analyzer_version` `0.2.4:extract-v89`. A watcher moved the generation during the pass (4457, then 4459). This audit did not write it.

### Filed

| Card | Priority | Severity | What |
| --- | --- | --- | --- |
| `ft-3018ab7669e7105678cfc313b0026c37` | 1 | high | GitPulse's vendored extractor is extract-v87. DevCouncil and the live store are extract-v89. `check-schema-parity` does not pin that constant, so a query-only reader reports the store payload-obsolete. |
| `ft-3f82536e8847798d74ec0251fb125e5a` | 1 | high | A daemon drain that sees a moved git HEAD and any hash change re-extracts the tree and passes an empty affected set, which turns edge digests off and rewrites every edge. An identical tree only restamps. |
| `ft-359e6751808b3ace1ed6b639268fcccf` | 2 | high | `parse_c_probe` and `probe_macro_body` call `Parser::parse(..., None)`. The progress callback in `parse_within_budget` is the only tree-sitter deadline. The daemon holds the writer lock across the probe. |
| `ft-7661cacbeb17363d702705f9cb286ba0` | 2 | medium | A glob hop inserted on an earlier round is never poisoned when a second glob publishes the same name later. The 200,000-hop cutoff returns a partial map with no degraded bit. The cross-round fixture was not run. |
| `ft-dff2a712a11b4ac47dcb7ea8b2debf83` | 2 | medium | An affected path missing from both the extraction slice and `deleted_paths` is committed as a delete. The stale-identity refusal next to it does not cover that shape. Daemon `LinkedDirectory` is a live caller. |

Folded into `ft-9ac4f2894bdaa64defe9c8d90f430cf2` (revision 2, still priority 2, severity raised medium to high, still `ready`): `ask` decodes every extraction under the sqlite connection before cancel; `clones` groups every candidate before the budget; `dead_symbols` and `locate_dead_evidence` do not check cancel. Dead's SQL page is already bounded. `rust/devmap-store/src/db/query.rs` was added to the existing planned-files section.

### Not filed, after a parent re-read

- `is_fresh: true` beside a JSON `degraded_reason` about partial extraction. The CLI joins coverage onto the freshness reason. Index currency and analysis coverage are different claims.
- Charged partial parses already enter `blind_by_language`. In-body four-byte errors are listed and not charged, on purpose (`partial_parse_erased_source`). Import-blind stays out of `is_complete` on purpose and is listed on status.
- Full resolve, full liveness, and Louvain on every one-file edit. Documented so dead-code stays sound. `ft-0b23fbe1` remains the Louvain baseline.
- Incremental vacuum's 256 MiB figure is a cap. The authors measured a steady state near 2 ms and a file that stays near 295 MB. Not filed as a per-edit cost. The checkpoint error is turned into `None` on purpose; the 250 ms busy bound is the stated wait. `GENERATION_RETENTION` is 2, so a prune runs FTS `optimize` inside the transaction. That reclaim is documented. It is not the 6n+1 copy on `ft-f6abefd9`.
- `list_generation_paths` joining `generation_nodes` would miss a symbol-less file. Every production extraction emits a `File` node, including refused and grammarless files. Not the generation-race card.
- Canonicalize of the worktree root runs inside the open generation transaction. The cost was not measured. Not filed.
- The edit hook's `devmap build` still computes `affected_closure`. It does not disable edge digests.
- `ft-02f43146` (the 2026-10-07 disk-write symptom) was not attributed to the v89 skew, the vacuum cap, or the HEAD-move rewrite.
- Done cards left closed: `ft-ab119272` (null freshness without grammars), `ft-fb65f8b0` (old O(repo) differential), `ft-b3d2c104` (edit-hook size cap).
- Dirty `integrate.rs` is an in-progress comment-preserving TOML edit for `ft-e9b14104`. Left alone. The `dc-store` retired-table drop and the rustfmt hunks are not DevMap reliability findings.

### Where these sit

The extraction-stamp card is a GitPulse vendor bump. Do not bump the pin from a DevCouncil session. It does not block the security waves.

`ft-3f82536e` and `ft-dff2a712` both edit `daemon.rs` and `write.rs`. Build one, then the other.

`ft-359e6751` edits `treesitter.rs`, which `ft-90cef8de` also plans to split. Do not merge the cards. A split has to keep the deadline on the moved probes.

`ft-7661cacb` edits `resolver.rs`. Build it after `ft-32522da4`, not beside it. It is not the TypeScript re-export accuracy card (`ft-736b0b22`).

The cancel fold stays on wave 5 with `ft-9ac4f289`. It still shares `engine.rs` and `query.rs` with the generation-race card, and `protocol.rs` with `ft-93ece913`.

Stopped here. No build until you say so.
