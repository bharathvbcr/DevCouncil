# Build report: DevMap and language accuracy, 2026-10-15

This run covered DevMap accuracy and language accuracy. It ran in two parts. In part 1, nothing was merged, because the hot files were held by idle worktrees. In part 2, the owner said "ask Fable and proceed", and Fable's rulings were carried out.

Local `main` is now `b8fb09dc`: eleven merges on top of `e7a70d20`, listed under "Merged to local main", "Part 3" and "Part 4" below. **Nothing is pushed.** No card reached done, because Manvi has not judged any of them. `dev verify` was not run. The previous report is kept as `BUILD_REPORT-2026-10-14.md`.

**Roles in this run:**
- **Fable** made the sequencing and judgment calls: D1–D5, R1–R4 and F1/F2 in part 1, and rulings 1–8 in part 2.
- **Opus** wrote the briefs and reviewed every diff.
- **Sonnet** ran the implementation, adversarial, labelling and investigation lanes, with at most four live at once.

## Merged to local main (not pushed)

| Merge | Content | Verification |
|---|---|---|
| `0b1044f8` ← `integration/accuracy-2026-10-15` (`4e682dca`) | The dead-tier reader now reads an empty tier as 0 rows. Adds characterization rows for defects A and B, Kotlin overload rows and Swift `deinit` rows. Tests and fixtures only. | `accuracy_fixtures` 12/12, `dead_tier_eligibility` 11/11, `kotlin_receivers_and_packages` 15 + 1 ignored, `real_corpus_accuracy` 17 + 1 ignored. 11 known-failing rows, each owned by a card. |
| `faa89e34` ← `lane/confident-tier-predicate` (`edbbadc8`), **ft-7327fd12** | A new `devmap-analyze/src/dead_tier.rs` holds one `in_confident_branch` predicate, used by both the corpus runner and the eligibility gate. Both local copies are deleted, including a second one in the runner's `tier_label`. The gate now refuses a fixture whose `status.degraded_reason` is set. | An adversarial pass found that an incomplete-coverage fixture flipped the gate's verdict; that is now fixed. The test `an_incomplete_fixture_is_refused_not_scored` failed before the fix. |
| `76d16f3f` ← `accuracy/corpora-pin-and-scrub` (`66399694`) | Pins LiquiTask, SalEdge, AcademiaTrack, Chronicle and DevPrism in `corpora.json`; every commit was verified by a local clone. 130 private `labelled_rows` are now `h:<sha256 prefix>` plus an evidence category. Adds a new `record_names_no_private_identity` test. | The leak test fails on the old record (260 places) and passes on the new one. `dead_tier_eligibility` outcomes are unchanged. |
| `802f3267` ← `accuracy/record-2026-10-15-labelled` (`26c5139b`), **ft-7327fd12** | The 2026-10-15 labelled record. The runner writes hashed `labelled_rows` through `devmap-analyze/src/record_identity.rs`, which the leak test shares. It fails loudly on a missing or malformed hash or category. No `sha2` dependency was added: hashes are computed outside the repo, next to the labels. | `record_names_no_private_identity` 10/10, `dead_tier_eligibility` 14/14, `real_corpus_accuracy` 26 + 1 ignored, `accuracy_fixtures` 12/12, `devmap-analyze --lib` 91/91. Opus re-ran the leak check on the record diff: clean. |

**Main checkout hygiene.** Another session owns two things in the main checkout:
- a staged +x on `rust/gusset-engine/check-pin.sh`;
- a rustfmt reflow of `rust/devmap-analyze/src/lib.rs`.

For each merge, both were set aside, the merge was done, and both were restored bit-for-bit:
- the mode change with `git update-index --chmod=+x`;
- the reflow from `tmp/main-lib-rs-reflow.patch`.

The checkout still shows the same 35 dirty paths.

## Corpus record now on main (guarded)

**How it was measured.**
- `RAYON_NUM_THREADS=4`, with load at 30–50.
- "Failed to parse" counts matched the stable baselines on every corpus: scholarlm 8, Lappi 0, GitPulse 0, DevType 0, ChronosFlow 3.
- Budget refusals were 0, checked before and after the run.
- The runner refreshed `open_gates` itself, from 155 to 121. `DEVMAP_ACCURACY_BLESS` was never used.

**How the labels were made.**
- Labels are model-read with file:line evidence; they are not a compiler oracle.
- Round 3 used seed 20261017 and excluded every site in rounds 1 and 2. The TS round accidentally used the intersection rather than the union of earlier sites; the 45 overlapping sites were dropped, which is equivalent to rejection sampling.
- 20% of each round was double-read by an independent Sonnet:
  - TS: 128/131 agreed, and all 3 disagreements were adjudicated to the first labeller.
  - Python: 47/47.
  - Kotlin: 69/69.
- The labels are in `~/.local/share/devmap-accuracy/`, which is not backed up.

**Results.** Wilson 95% lower bounds are in brackets.

| Corpus / language | Confident precision | UniqueGlobal | Recall | Earlier recall | Honest miss |
|---|---|---|---|---|---|
| scholarlm TypeScript | 172/179 | 27/34 ✗ | **243/270 (.900) [.858]** | 40/43 [.81] | 26/27 (96.3%, under the 98% floor, ungated) |
| Lappi-decision Python | 145/169 ✗ | 1/24 ✗ | **162/179 (.905) [.853]** | 66/74 [.80] | 17/17 |
| ChronosFlow Kotlin | 179/180 | 46/47 | **169/182 (.929) [.882]** | 56/60 [.84] | 13/13 |
| DevType Swift | 175/180 | 32/35 ✗ | 73/85 (.859) ✗ | same | 12/12 |
| ChronosFlow Swift | 173/177 | 31/32 | 55/62 (.887) ✗ | same | 7/7 |

- **Kotlin's dead tier is re-proven:** `dead_fp_by_tier["0.9"]` is 0 false out of 9.
- **Weak constructs:**
  - TS: `module_qualified` 2/10, `typed_receiver` 9/16.
  - Python: `super` 3/8, `typed_receiver` 13/22.
  - Kotlin: `typed_receiver` 26/32.
- **Python recall** is .905 as a point estimate, but its lower bound (.853) is below the .88 target.
- **Python precision** fails mostly because of `ft-0e6cc90f`: 23 of its 24 bad edges.
- **Superseded entries:** scholarlm's python, tsx and go entries moved to `superseded`, because the runner keeps only pinned languages. Eligibility still passes.

## Part 3: merged after the rewrite (local main, not pushed)

| Merge | Content | Verification |
|---|---|---|
| `4d6c2053` ← `lane/honest-miss-floor`, ft-4cd88dbe | Confident precision, honest-miss and 0.9-tier dead FP are absolute gates: open on any miss, including a first measurement. Recall floors stay a ratchet. | Two previously ungated misses are now open gates: scholarlm TS honest-miss 0.963 < 0.98, and Lappi Python confident precision 0.858 < 0.95. A new test fails with the fix disabled. |
| `041ac5f7` ← `accuracy/record-2026-10-15-swift-r3`, ft-8ca270ad | Swift round-3 recall labels: DevType +97 in-repo sites, ChronosFlow +160, double-read 36/36 and 64/65 (one class-only mismatch). Guarded re-measure. | DevType 157/182 = .863 [.805], ChronosFlow 186/222 = .838 [.784]; both miss .90. The runner's "ChronosFlow regressed .887 → .838" is the old n=62 sample being optimistic at the same code and commit, not a DevMap regression. Kotlin rows unchanged. |
| `ad072253` ← `lane/swift-recall-characterize` | 19 known-failing Swift fixture rows, one per missed shape found by a read-only investigation, plus 2 forbid guards. All synthetic. | Before landing, `record.json` was trimmed to change only the Swift block. |
| `e7e31d24` ← `lane/swift-recall-final`, ft-8ca270ad | Swift receiver typing in `swiftscope.rs` and `langcalls/swift.rs`: `$0` from member collections, inferred locals and function-type annotations; if-let call results; prefix-operator and multi-line constructor receivers; try/await stripped. | Two adversarial rounds (43 attack tests); 9 confirmed wrong-edge cases fixed by abstaining. Corpora vs base: DevType +37/−0 confident edges, pair recall 157 → 160/182; ChronosFlow +11/−0, 186 → 191/222; labelled precision unchanged; all 48 added edges read correct. Benchmark neutral. Fable chose this over a stricter version that lost two base edges to block a shape found 0 times in either corpus. Residual: a typealias declared in an extension in another file (`sw.Y1`, plus an ignored attack test); fixing it needs the resolver (S5). |

**Other part-3 results (nothing committed):**
- **TS compiler-oracle check (ruling 6):** TypeScript 6.0.3 over scholarlm's own tsconfig agreed with 46/46 recall labels and 24/24 excluded labels. The labelling conventions are now in `~/.local/share/devmap-accuracy/CONVENTIONS.md`.
- **Scorer granularity:** `score_labels` credits a recall site when any caller → truth edge exists (pair recall), not when that site resolved. Measured inflation is at most about 1 point (DevType chained 3/7 is really 1/7). The fix sits in L-Go's held block; a note is on `ft-4cd88dbe` and on the L-Go card.

## Part 4: next pass (local main, not pushed)

| Merge | Content | Verification |
|---|---|---|
| `735e210e` ← `lane/parse-budget-retry-r` (`88ced921`), ft-e007ef1b | The parse-budget serial retry, rebased onto `e7e31d24`, with the clippy warning fixed. | Hunks verified disjoint from S5's `treesitter.rs` edits. Bench vs `e7e31d24` on a pinned corpus of 1946 files, 5 interleaved release pairs at load 12–29: min +3.7% (limit 5%), median −3.4%. `devmap-extract` and `devmap-store` pass, apart from the pre-existing `every_child_process…`. |
| `e5cc3005` ← `lane/py-kt-characterize` (`f92553ff`), ft-c666ba7c / ft-0720fe1d | 16 known-failing synthetic rows and 6 passing guards and controls. **Python:** `super()` with a duplicate class name, `f(x).m()`, tuple unpack, pytest fixture params, class-body field hops. **Kotlin:** stdlib-return locals, function-type lambda params, if-expression locals, expression-body factories, `mockk<T>()`, `X::class.java`. Test data only. | Every miss is unresolved; none binds to a wrong target. Every fix needs `treesitter.rs`/`resolver.rs` (S5). |
| `b8fb09dc` ← `accuracy/record-2026-10-15-swift-r4` (`91db2b1c`), ft-8ca270ad | Swift corpus rows re-measured on `735e210e`. | DevType recall 160/182 = .879 [about .824]; ChronosFlow 191/222 = .860 [about .809]. Precision, UniqueGlobal and Kotlin are unchanged. The spurious "recall regressed" gate is gone. The full accuracy suites pass on `e5cc3005` and `b8fb09dc`. |

**Investigations this pass (read-only; full analyses in the session scratchpad, which is not committed):**
- **TS honest-miss 26/27.**
  - The silent miss is in a file whose parse was damaged.
  - The resolver classes it `no_namesake` without checking parse health.
  - That is an honesty defect, in `resolver.rs`.
- **TS UniqueGlobal 27/34.**
  - All 7 wrong edges are ft-0e6cc90f's shape: 4 module-scope `vi.fn()`, 2 destructured-param defaults and 1 from the damaged parse.
  - The fix needs a module-scope binding table.
- **TS weak constructs.** All are `uninferred_receiver`:
  - imported literal with a spread: 7;
  - property alias: 1;
  - renderHook: 2;
  - `ReturnType<typeof>`: 2;
  - factory module member: 2.
  - Draft rows are on ft-736b0b22, because `ts/edges.tsv` is S5-held.
- **Data note.** 19 sample-2 TS labels no longer join `recall_key_2.json` on disk. Check this before the next TS re-measure. Resolved in Part 5: the key on disk is a later re-draw, and the scorer never reads it.

## Part 5: 2026-10-16 pass (local main `7f97d468`, not pushed)

Fable chose three lanes. Each got an Opus brief, a Sonnet implementation, an adversarial pass and Opus review.

| Merge | Content | Verification |
|---|---|---|
| `c257908b` ← `lane/degraded-status` (`d40fac25`), ft-47124ed7 parts a and d | `devmap status` gains an additive `budget_refused {total,shown,truncated,paths}` field, capped at 50, plus a `budget-degraded:` clause in `degraded_reason`. Both are read from the per-generation `generation_coverage_gaps`, not the shared payload view. A build no longer short-circuits as unchanged while refusals exist; refused paths join `affected` and are re-extracted. Contention-versus-idle tests compare edge inputs. | The attack pass found two real bugs (F1 and F2) and one likely (F3). `generation_files` is a VIEW over shared `file_payloads`, so the in-place promotion rewrites retained generations, and a refusal over a good payload was hidden. Fable ruled: F2 fixed via the gap table; F3 fixed; F1 kept as a declared interim with its test `#[ignore]`d against ft-3bdf1450, an owner-gated schema change. The status and rebuild tests fail on pre-fix code. A flake table of 12 tests, interleaved with base at load 30–160, showed no consistent head-only failure. Bench: cold min 13.27 → 12.82 s, no-change min 0.143 → 0.136 s. |
| `251d798f` ← `lane/swift-recall-r5` (`5b496dd6`), ft-8ca270ad | Swift receiver typing from the file's own text: same-file typealias, casts, tuple-parameter members, sum-before-chain, a type's own parameterless method returns, SwiftData `fetch` on a provable ModelContext, and own-initializer scoping (including `for`). Extraction schema 89 → 90. | The attack pass found 15 wrong-type or wrong-edge cases (tuple rebinding, an alias shadowed by a generic, the `?.` optional-member map, `fetch` on any receiver, a generic as nominal, an optional alias, `for x in x`). All are fixed by abstaining; no test was weakened. Bench on retired instructions (load-robust): +1.7% DevType, +0.4% ChronosFlow; RSS flat. |
| `8bf39ec8` ← `lane/classify-parse-tests-c` (`a5272c60`), ft-c1e2cec7 | Classifies 5 integration tests against the parse feature. 4 are Swift attack tests from our `e7e31d24` merge, which had broken `every_integration_test_is_classified_against_the_parse_feature`. | All 5 were already running under default features. The classification test passes. The lane worktree's git hung on fsmonitor, so the commit was built with plumbing from the main repo. |
| `7f97d468` ← `accuracy/record-2026-10-16-swift-r5` (`88902fe4`), ft-8ca270ad, ft-8ed75897 | The Swift record re-measured at `251d798f`. The ChronosFlow Kotlin row was measured fresh. Its nine 2026-10-15 dead rows, whose labels are lost, moved to `carried_dead_evidence` with `verified: false`. Kotlin leaves `CONFIDENT_TIER_ELIGIBLE` and publishes at 0.4 `confident_tier_unproven`, like Swift and Python. | DevType 166/182 = .912 [.862]; ChronosFlow Swift 198/222 = .892 [.844]. Precision unchanged; parse failures 0/3; budget_refused 0. `dead_tier_eligibility`, `test_hardening`, `accuracy_fixtures` and the whole devmap-analyze package pass. Three expectations that encoded "Kotlin is eligible" were updated. |

**Verification of merged main `7f97d468`** (full-tree archive, load 27–88):
- devmap-analyze and devmap-store: the whole packages pass.
- devmap-extract: lib 383 pass; `it` 650 pass, 1 fail. The failure is the pre-existing `every_child_process…` (`devmap-serve/src/blast_cancel_tests.rs:17`).
- devmap-cli: bin 241 pass; `it` 555 pass, 1 fail. `a_watched_file_growing_past_the_limit_settles_without_retries` hit its 20 s drain timeout in both full runs and passed in both solo reruns (load ~80). The cause, load, is inferred.
- The parse-feature classification test now passes.

**Hold exceptions (Fable, recorded in the merge messages and on ft-e3f7d1b1):** files held by the parked S5 got disjoint-hunk edits. `treesitter.rs` got two Swift hunks, `cache.rs` the version constant, and `liveness.rs` the eligibility constant and its doc. These follow the parse-budget precedent and do not decide the hunk-level-holds question. The main checkout's uncommitted `liveness.rs` hunk was saved, reversed, merged around and re-applied; it still applies.

**TS label reconcile (read-only, ft-736b0b22):**
- The on-disk `recall_key_2.json` is a re-draw of sample 2. 27 of 29 labelled recall rows don't join it. The labels are correct at the pin.
- The scorer joins on caller→truth and never reads the key, so TS .900 at n=270 is unaffected.
- Open: 281 labelled versus 270 scored. 3 rows are duplicated constructors; 8 self_method rows are unexplained, because the merged labels file is not on disk (ft-8ed75897).
- `dl_agree.jq` pairs the wrong sites for sample 2.

**Gaps:**
- The DevMap MCP was down all pass, so no lane's navigation is graph-confirmed.
- `gitpulse_insights` timed out at 60 s for every lane, so there is no collision facet.
- At load 60–240, git hung on fsmonitor in worktrees; several commits were built with plumbing.

## Parse-budget fix (now merged in `735e210e`; ft-e007ef1b stays in review)

- **`lane/parse-budget-retry` → `lane/parse-budget-retry-r`, ft-e007ef1b.** The original commits are `9eecc2ca`, `9035c770` and `8c491aa0`. It was merged in part 4, after the benchmark that had blocked it passed (see the Part 4 table). The card stays in review because two criteria are unchecked: a slow-time test showing the 131 s C++ input stays bounded, and true measured numbers in the `DEFAULT_PARSE_BUDGET` doc comment.
  - **What it does:**
    - The parallel pass keeps its 5 s budget.
    - Files refused for budget are re-run one at a time at 25 s. At most 16 files are retried, and cancellation is checked before each one.
    - One constructor stamps every budget refusal, including all the notebook sites. Non-budget failures are never retried or relabelled.
    - A still-refused file's reason says whether it was refused again, skipped by the cap, or skipped by cancellation. `status` shows the path through `parse_failed`.
    - The build JSON gains an additive `file_progress.extraction.budget_retry` field.
  - **Adversarial pass:**
    - Notebook stamping was a confirmed defect; it is now fixed.
    - The cache never admits or reuses a refusal.
    - A recovered extraction is identical to a first-pass one in six languages.
  - **What had blocked the merge (both resolved in part 4):**
    - **An idle benchmark.** The first attempt, at load ~30, was inconclusive: +7% on the minimum, −3% on the median. The real harness is the ignored test `rust/devmap-extract/tests/extract_all_bench.rs`; there is no `bench.sh`.
    - A clippy re-run, which is now clean in the touched files.
  - **Suites:** they pass, except two kinds of failure, neither new:
    - the pre-existing `every_child_process_in_the_kernel_goes_through_the_runner`;
    - load-timing tests that pass when run alone.

## Tasks

| Card | State | Notes |
|---|---|---|
| `ft-7327fd12` dead-tier definition (p1) | done requested, held for review | Merged in `faa89e34` and `802f3267`. Manvi has no verification record. |
| `ft-e007ef1b` parse budget (p1) | review, partial | Blocked on the idle benchmark. The remaining criteria were split into `ft-47124ed7` and `ft-fb394194`. |
| `ft-47124ed7` (p1) | in progress | Parts a and d merged in `c257908b`. Part b, `walk_incomplete`, needs `liveness.rs` (S5 and dirty in main); part c, the runner refusal, needs `real_corpus_accuracy.rs` (L-Go). The daemon drain path (devmap-serve) can still carry a refused row forward. |
| `ft-3bdf1450` (new, p2, backlog, **owner**) | — | Payload identity must distinguish a budget refusal. This is a schema change. It also logs hook.rs:1187 (one refused file disables PreToolUse context) and the 50-char truncation at hook.rs:936. |
| `ft-8ed75897` (new, p1, ready) | — | Re-label the 9 Kotlin dead rows; persist the merged label files (TS and Kotlin); make the runner fail loud on missing labels. |
| `ft-fb394194` (new, p2, backlog) | — | Add a cancellation token to the build chain; today there is a 400 s uncancellable worst case. Gated on S5, because the build chain runs through S5-held files. |
| `ft-736b0b22` L-TS | review | Recall .900 [.858] at n=270, which now clears .85 at the lower bound. UniqueGlobal .794 is still an open gate. |
| `ft-c666ba7c` L-Python | in progress | Precision fails (`ft-0e6cc90f`). Recall's lower bound is under .88. The open criteria need S5 files. |
| `ft-0720fe1d` L-Kotlin | in progress | Recall .929 [.882]. Overload identity (`kg.OV1`/`kg.OV2`) needs S5's `resolver.rs`. |
| `ft-8ca270ad` L-Swift | in progress | Part 5: DevType .912 [.862]; ChronosFlow .892 [.844], still below .90. Next per Fable: `.mainContext` only on a ModelContainer receiver, or on an unbound receiver with SwiftData imported and no in-file `mainContext`, plus `chronosContext()` through its own written return; each needs attack tests. |
| `ft-0e6cc90f`, `ft-6976e8de` | review (characterized) | The fixes wait for S5 to be committed. |
| `ft-4cd88dbe` accuracy harness | review | Logged: the honest-miss floor is enforced only when the previous record held it. |
| `ft-befc1ff0` blast `--literal-pathspecs` | **deferred**; shows in progress, should be ready | `rust/dc-regress/src/blast.rs` has uncommitted edits in the main checkout from another session, and the card says not to build beside `ft-93ece913`. The call site is `blast.rs:1099`. The sweep for other call sites was not done. |
| `ft-c1e2cec7` test honesty | review | Logged two more items: the flaky `a_file_of_nothing_but_dynamic_imports_stays_linear`, and the pre-existing red `every_child_process…` test, which fails on `devmap-serve/src/blast_cancel_tests.rs:17`. |

## Benchmark deltas

The merged code adds a pure predicate, a pure shape checker and test-harness changes, and touches no hot path. The parse-budget change does touch the extraction path; its benchmark is pending, and that is why it is not merged.

## Fable's rulings, part 2

1. **S5 and L-Go:** park them. Cut `ft-7327fd12` and `ft-e007ef1b` on fresh branches, in hunks that don't overlap the held files' edits. Defer `ft-0e6cc90f` and `ft-6976e8de` until S5 is committed.
2. **The unpinned proof corpora:** pin them, don't retire them.
3. **The integration merge:** merge to main now with `--no-ff`, no push. To get past the staged file, unstage, merge and re-stage. The later approved branches were merged the same way.
4. **`ft-3018ab76`:** an owner decision. Bump once, after S5 lands, because S5 moves the extraction schema to v90.
5. **Private strings:** scrub them with a forward commit. A history rewrite is the owner's call.
6. **Model-read labels** are fine for the record and its gates. A release claim needs the Wilson lower bound, a statement of the method, and a compiler-oracle spot check of at least 30 rows where an oracle exists (Go, TS).
7. **Recall top-up:** fund it, TS first.
8. **What waits:** stop nothing. Send `ft-e007ef1b` to review as partial and file two follow-up cards. Leave the cards from older audits parked.

## Owner decisions pending

1. **Push** local main (`7f97d468`).
2. **Commit S5** (`c631ed26`). That unblocks `ft-0e6cc90f`, `ft-6976e8de`, `ft-fb394194`, Python's open criteria and Kotlin's overloads. L-Go (`c75b63d0`) needs a decision too.
3. **`ft-3018ab76`**, the vendor pin bump. Fable's advice is to do it once, after S5 lands.
4. **A history rewrite** to purge private strings from earlier commits of `record.json`. A forward scrub is already on main.
5. **An idle window** for the parse-budget benchmark, or accepting it unmeasured.
6. **The main checkout's uncommitted `dc-regress` edits**, which block `ft-befc1ff0`, and setting that card back to ready.
7. **Restart the devmap MCP.** It fails to connect with ENOENT on a test binary path under another worktree, and GitPulse's DevMap index is stale.
8. **Release wording** under ruling 6, at release time.
9. **The schema change for `ft-3bdf1450`:** give a budget refusal its own payload identity. It changes `CURRENT_SCHEMA_VERSION` and must be carried to GitPulse, Manvi and Jarvis.
10. **The extraction schema number.** Main now uses `extract-v90` (Swift r5). Ruling 4 assumed S5 would take 90, so S5 needs 91 if it changes extraction output, and the vendor pin bump `ft-3018ab76` should follow whichever lands last.
11. **Hunk-level holds.** Three disjoint-hunk exceptions to S5's holds were taken this pass under Fable's rulings. A policy would replace the case-by-case rulings.
