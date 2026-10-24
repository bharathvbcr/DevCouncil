# Build report: DevMap backlog run, 2026-10-20 to 2026-10-21

This run worked the open DevMap cards on the GitPulse board, as the owner asked ("focus on devmap improvements"). Each lane went through four steps: an Opus brief, a Sonnet implementer in its own worktree, one or more adversarial Sonnet passes, and an Opus review. Fable ruled on sequencing and scope.

**Update, 2026-10-21:** the owner said "merge all and for others go with fable".
- Every lane is merged. Local `main` moved `aacffb85` → `b888043d` (lanes), then → `99d41a1f` (the ft-2c800986 design doc), then → `500a55c8` (the GAP-13 harness).
- **Nothing is pushed.** Fable ruled that main pushes after the consumer sweep.
- See "After the merge" at the end of this report.

No card reached done: done is gated on Manvi's verification, and that has not run. The previous report is kept as `BUILD_REPORT-2026-10-19.md`.

AUDIT_REPORT.md's waves were already all in review before this run, so the order came from priority, severity and Fable's rulings.

## Rules in force

These were carried over from the owner's 2026-10-17 adoption of Fable's advice.

- At most two lanes ran at once. Load stayed between 8 and 70.
- No merge before a lane's adversarial round closes.
- `EXTRACTION_SCHEMA_VERSION` stays frozen at 97, so no lane changed extraction output.
- Cards stay in review until Manvi verifies them.

## Cards moved to review

| Card | Branch @ head | What landed | Adversarial |
|---|---|---|---|
| ft-47124ed7 (load-degraded index) | none (bookkeeping) | Every criterion is either met on main or moved to ft-09f2437e or ft-292df47e. Evidence tests were re-run: status 8/8, contention 4/4, traversal hole 6/6. | n/a |
| ft-fb394194 (build cancellation) | `lane/build-cancel` @ `d6915e19` | One token, `devmap_query::Cancel`, flows from SIGINT in `devmap build` and from daemon shutdown or signals into extraction and the budget retry. A single phase CAS decides cancel versus exit 130. A cancelled build writes no generation. Daemon paths stay queued and uncharged. | 1 MEDIUM and 2 LOW, plus 2 inferred issues, all fixed with probes. Non-cancelled output is byte-identical to main (node, edge and payload hashes). 200 random-point cancels and an 11-delay real SIGINT sweep held. |
| ft-5ad86836 (Kotlin declaration drops) | `lane/kotlin-decl-proof` @ `b392f0a5` | Tests only. AC3 and AC4 are met by test. **AC1 found a silent loss:** `@A\n@B\nprivate fun f() {}` parses Clean as an `infix_expression`, so `f` disappears with no partial flag. AC2: a one-line `class C { companion object { … } }` is an ERROR node. Both fixes are extractor output (v98), and their tests are ignored with the reason. | Opus returned the AC4 found-check (it matched the whole JSON) and it was fixed. |
| ft-a43bfbc6 (parent_dir) | `lane/liveness-script` @ `93af32be` | Splits on `/` or `\`. Indexed paths are already normalised, so this is boundary hardening. | Attacked with lane C; held. |
| ft-1d03bb95 (Python script exemption, getattr) | `lane/liveness-script` @ `93af32be` | Part 2: a Python, JS or TS ScriptEntry file claim no longer exempts **module-level constants and variables**. Functions, classes and methods keep it (Fable's scope). Part 1 (getattr) is **blocked on extract-v98**: no call record exists for the outer call. | Narrowing every symbol exposed about 20 live callables at 0.4 in real repos (scholarlm 16/16 live), so Fable cut the scope to constants. A 141-probe attack corpus is kept as tests. |
| ft-8a85f117 (KNOWN_GAPS) | `lane/known-gaps` @ `1d83a9c6` | See the item breakdown below the table. | Byte-identity evidence (pure dead-code deletion) |
| ft-eda9b3a6 (Kotlin overloads) | `lane/resolver-bundle` @ `1fd736bd`, also `lane/resolver-bundle-on-lane-c` @ `bb7933fc` | Same-file sibling overloads (Kotlin, Swift, Java, C#, Scala) no longer self-edge; they get a labelled abstain. Identical receiver hits count as one target, so kg.OV2 binds. The Swift protocol fan-out keeps the other conformers. `Resolver.parent_dir` is fixed. kg.OV1 and kg.OV2 leave known_failing. **AC4 (distinct ids) is blocked on v98.** | 3 rounds; see below |
| ft-87b28bb6 (Kotlin Z1–Z6) | — | **Nothing landed.** Z3 and Z5 were implemented, attacked and reverted on Fable's ruling. Z3 now waits on ft-2c800986; Z1, Z2, Z4, Z5 and Z6 wait on v98. Recommendation: close this card in favour of those. | — |
| ft-260158ce (TS namespace dead at 0.9, new) | `lane/ts-namespace` @ `53ad3ef5` | The root cause is verified: `scope_locals.rs` has no `internal_module` arm, so a namespace shadows itself and file-scope reads are dropped. The real fix is v98. Meanwhile, unexported TS and JS namespaces with no inbound evidence are capped at 0.4 with a named reason. | Demote-only change; Opus confirmed `Module` comes only from namespaces in TS and JS |

The ft-8a85f117 items:
- **GAP-21, closed.** The `async_function_definition` arms were dead in all 33 linked grammars and are deleted. Output is byte-identical on `rust/` (17,123 nodes, 68,109 edges) and on MLSystemsLab.
- **rayon dev-dependency, kept.** The `worktree_stress` example uses it.
- **`manifest` path, used.** It is honoured, with no change.
- **GAP-10, measured, still open.** A one-file `--affected` build costs about 18% of a cold build: scan and resolve stay whole-tree.
- **GAP-8 and GAP-9, re-measured** on local corpora. They are not the original 12,821-file corpus.

### Lane D's adversarial rounds (resolver bundle)

- **Round 1** found wrong edges at 1.0:
  - Z5 accepted any bare return type and ignored casts.
  - Z3 dropped qualifiers.
  - Python alias reads bound through `for`, `with`, walrus and comprehension rebinding.
  - Some true recursion was lost.

  Z5 and the alias bind were reverted to v98. Z3 was hardened and the overload abstain was scoped to siblings.
- **Round 2** found three pre-existing wrong edges at 1.0 in the Kotlin factory path, widened by Z3:
  - an explicit import loses to a star import;
  - a nested class is read as a top-level name;
  - a member `make()` is mistaken for the top-level one.

  The fixes touched six resolver seams.
- **Round 3** found that those fixes caused 4 HIGH regressions against main:
  - ChronosFlow `Task.toDayQuickItem` and `Habit.statusLabel` were lost;
  - Scala never stem-binds;
  - a function-local import suppresses a star import;
  - cross-file `split_types` drops real recursion.
- **Fable chose option B:** keep only the same-file overload abstain and `parent_dir`, revert every round-2 change and Z3, and file the pre-existing bugs as ft-2c800986.
- **Close condition, met:** a corpus diff of the cut branch against main on 12 repos (AppLock, Blitztree, ChronosFlow, Curio, DevType, Graft, M5Blade, MarkDev, Meridian, scholarlm, Strait, Vani). Every removed edge is a same-file overload self-edge, and no non-self edge was lost. The added edges are overload receiver binds and the 0.40 protocol fan-out, and spot checks were right. 45 probes are ignored, citing ft-2c800986.

## Benchmarks on touched paths

Each figure is `devmap build rust --full`, release build, instructions retired, min of 3, interleaved against that lane's base.

| Lane | Base → new | Delta |
|---|---|---|
| build-cancel | 188.511G → 188.376G | −0.07% |
| liveness-script | 190.37G → 187.86G | −1.3% |
| known-gaps | 189.60G → 187.23G | −1.25% (peak RSS +2%, likely noise) |
| resolver-bundle (cut) | 190.48G → 192.68G | +1.2% |
| ts-namespace | 191.24G → 191.43G | +0.10% |

None exceeds the 5% gate.

## New and folded cards

- **ft-260158ce** (new, p1, high): a TS namespace is confident-dead. It is now in review with the cap; the extractor fix is v98.
- **ft-f0af68af** (new, p2, medium): recall gaps that keep script callables exempt. These are JS object-literal members, `T[k]()` dispatch, Python attribute-receiver methods, `nn.Module` hooks, string annotations and `global X; X += 1`. Most are v98.
- **ft-2c800986** (new, p1, high): the pre-existing Kotlin and JVM factory-path wrong edges at 1.0, and Scala stem binding. It also covers the Python 0.9 global-unique bind. It carries the round-3 regression guards as constraints.
- **Not filed as a separate card:** an extract-v98 bundle. The board's duplicate check flagged it. Each v98 item stays on its own card; the list is below.

## Fable's decisions

1. Resolver-only and liveness changes are allowed under the v97 freeze. They land at the first build that sees an edit, or with `devmap build --full`; a warm, unchanged store stays stale until then. (Fable's ruling named `--force`, but `--force` only replaces a foreign map; this was verified 2026-10-21.) Each lane proves its change lands under a narrowed write. This revised Fable's first ruling, which had blocked them.
2. Every extractor-output fix waits for one v98 bump, after the consumer sweep (ft-3018ab76).
3. Lane C narrows constants and variables only, for Python, JS and TS. A 0.4 label does not make a false row free.
4. Lane C is merge-eligible despite 12 live 0.4 constants on MLSystemsLab. That gap is pre-existing and resolver-owned, and it is pinned as known_failing `py.AR1`.
5. Lane D: option B scope cut after round 3. Lost true edges outrank removed self-edges.
6. ft-2c800986 needs a scope and import model designed before code. Stop after wave 4 rather than stack a fifth unmerged branch.
7. Defer the competition benchmark (ft-c86df5d4). Reasons:
   - none of the five competitor tools is installed, and installing them is a new third-party dependency;
   - the idle-machine guard cannot be met;
   - it should measure the merged build.

## Waiting on extract-v98 (one bump, after ft-3018ab76)

- **Kotlin:**
  - Stacked annotations plus a modifier plus a block body are silently dropped on a Clean parse (ft-5ad86836 AC1; HIGH, since a Clean file hides declarations).
  - One-line companion object (ft-5ad86836 AC2).
  - Distinct overload ids (ft-eda9b3a6 AC4), and same-name inner classes under different outers.
  - Z1, Z2, Z4 and Z6 extractor facts, and the Z5 callee type-parameter list (ft-87b28bb6).
- **Java:** parameter declared types (`declared_type: None`), needed for receiver binding.
- **Python:**
  - a getattr outer-call record (ft-1d03bb95 part 1);
  - function-local alias rebinding facts (`py.AR1`);
  - the items carried from the previous report.
- **TS:** namespace self-shadowing in `scope_locals.rs` (ft-260158ce).
- **tsconfig.rs:** `parent_dir` splits only on `/`.
- **Swift:** extract-side residue (ft-80c5d100) and function-type property facts.
- **Scala:** package facts for `jvm_package_by_file` (ft-2c800986).

## Deferred or blocked, with reasons

- **ft-c86df5d4**, the competition benchmark: see Fable's decision 7.
- **ft-f6abefd9, ft-3bdf1450 and ft-38ef7393**: store schema changes, which need the owner.
- **ft-9465b250**, Windows Job Object: a decision, and it cannot be tested on this Mac.
- **ft-3018ab76**: in progress under an owner gate, and was not touched.
- **ft-2c800986**: needs a design first (Fable).
- **ft-f0af68af** and **ft-80c5d100**: mostly v98.
- **ft-8a85f117 GAP-7**: a VB.NET grammar is a new dependency. **GAP-11, 12 and 13**: run them or retire them.

## Owner decisions pending

1. **Merge to main.** Proposed order:
   - first the independent lanes, `lane/build-cancel`, `lane/known-gaps` and `lane/kotlin-decl-proof`;
   - then the stack, `lane/ts-namespace`, which contains `lane/liveness-script` and then `lane/resolver-bundle-on-lane-c`. Merge it as one fast-forward of the stack.

   `lane/resolver-bundle` is the same resolver change based on main, and is redundant if the stack is merged. Pairwise `git merge-tree` of the four tips is clean, but the **combined tree has not been built or tested**. After merging, run the full Rust suites on it, reinstall `devmap`, and run `devmap build --full` so warm stores pick up the resolver and liveness changes. **Done 2026-10-21; see below.**
2. Manvi verification for the cards in review.
3. The consumer sweep and the v98 unfreeze (ft-3018ab76). `devcouncil install devmap` reports the following consumers stale:
   - GitPulse vendored crates, `gitpulse-mcp` and GitPulse.app;
   - `dcstore`;
   - the Manvi and Jarvis pins (devmap-extract 94 against 97, dc-store 9 against 10, workbench 13 against 14).
4. ft-2c800986 design: one JVM/Kotlin scope and import model, so explicit-over-star, nested-versus-top-level and member-versus-top-level are decided at one owner.
5. ft-87b28bb6: close it in favour of ft-2c800986 and v98.
6. ft-8a85f117: GAP-7, VB.NET, either a pattern-only decision or a grammar dependency. GAP-11, 12 and 13: run or retire.
7. ft-c86df5d4: approve installing the pinned competitor binaries, and schedule an idle machine.
8. **The devmap MCP server is misconfigured.** It points at `tmp/fix-wb/rust/target/devcouncil-testbin/…/devmap`, which no longer exists, so every lane navigated with the CLI.
9. **Cleanup:**
   - Agent worktrees under `.claude/worktrees/agent-*` hold the lane and attack branches.
   - Three detached probe worktrees, under `agent-a1ed67124fcd39a2b/{probe-c,probe-main,.claude/worktrees/probe-main}`, plus an untracked `corpus-out/`.
   - Survey ignored files before removal: `git worktree remove` deletes ignored files.
   - The attack branches are `attack/build-cancel`, `attack/liveness-script`, `attack/resolver-bundle`, `attack2/resolver-bundle` and `attack3/resolver-bundle`.

## Not verified

- The combined merge tree was not built or tested; only each lane's own suites ran.
- Wall-clock tests failed under load in several suites: `s01_thousands_of_factories_bounded_and_correct`, `crash_consistency::a_build_killed_mid_persist_leaves_a_whole_generation`, `build_lsp_opt_in`, and others listed in the card logs. Each passed when rerun alone. Not every one was checked against main.
- Windows `cfg` arms (lane A's `CtrlC` stream) were not compiled.
- No workspace-wide `cargo test`, and no Go host tests.
- Corpus classification of added edges was by sampling. Removed edges were checked in full where the counts were small.
- GitPulse insights git facets were deferred under load at the start and the end of the run (unscanned, not clean). `collision_risk` was clean once, with 2 of 2 worktrees scanned.
- The DevMap MCP was unavailable for the whole run. Lanes used the `devmap` CLI, extraction dumps and source reads, and DevMap answers were often `walk_incomplete`.
- The installed `devmap` was rebuilt from main at the start: it had been 7aa6bf51 and was missing the freshness fix. It now reports build `aacffb85`.

## After the merge (2026-10-21)

The owner said: "merge all and for others go with fable".

### How the merge was done and verified

- I built the merge commits with `merge-tree` and `commit-tree` on top of the stack `lane/ts-namespace`, in this order:
  - `7787808c` merges build-cancel;
  - `1e5eefd6` merges known-gaps;
  - `b888043d` merges kotlin-decl-proof.
- Combined-tree test: the full suites of devmap-extract, -store, -query, -resolve, -analyze, -cli and -serve ran on `b888043d` in a scratch worktree. 139 suite binaries passed and 8 tests failed.
  - 7 of the 8 passed when rerun alone at load 30: `test_adversarial_inputs`, `the_under_limit_fixture…`, `a_file_of_nothing_but_dynamic_imports…`, two `python_extract_attack4` timing tests, `p4_a_five_thousand_edge…` and `hostile_five_thousand_extensions…`.
  - The 8th, `s01_thousands_of_factories_bounded_and_correct`, fails the same way on main: "0/3000 resolved". Under load the 5 s parse budget refuses its generated file. It predates this run.
- Main then fast-forwarded `aacffb85` → `b888043d`, confirmed by rev-parse.
- I reinstalled `devmap` (build `99d41a1f`, reported as `-dirty` only because BUILD_REPORT.md was uncommitted) and `dcstore` (0.2.5, workbench 14; now a match). Then `devmap build --full --manifest` ran: generation 4564, fresh, 0 budget refusals.
- **Correction:** `devmap build --force` does **not** bypass the unchanged early-return; `--full` does. Earlier notes that say `--force` mean `--full`.

### Fable's rulings on the delegated decisions

1. **ft-2c800986:** design-first.
   - The doc `docs/devmap/design/jvm-scope-import-model.md` was written, approved with changes, and merged (`99d41a1f`).
   - Phase 0 ran: 39 of 45 probes failed as the doc predicted.
     - e6 was a false probe (a loose assertion).
     - r3, j6 and e7 need Java declared-type facts (V6, v98) and stay ignored.
     - x1 and x2 are model targets, not guards.
     - j10 is ignored, citing V5.
   - **Commit 1** (`lane/jvm-scope-model` @ `c9a6e127`) adds the bench, the FQN/container-chain declaration tables, owner-keyed members and the `resolve_name` seam.
     - It changes no behaviour: edge dumps are byte-identical to main on Curio, Meridian, MarkDev, DevType and ChronosFlow.
     - The probe-assertion sweep made about 52 assertions symbol-exact and fixed two wrong expected ids (j4, r5). It added the guards x4 and x5 (an extension on an external `Task` beside an unrelated same-named member).
     - **Not merged on its own:** `resolve_name` answers `Unmodelled` and nothing reads the tables yet. It merges with the commit that wires it in.
   - The tables attack (3 HIGH, 6 MEDIUM) was fixed. Fable ruled that Uncertain is a ledger label, never a verdict, and dropped the weak triggers.
   - **Commit 2** adds `resolve_name` ranks 1–8. Its fixes after round 1 (35 of 50 probes failing) covered rank 8, `MaybeExternal`, and the removed fact scan.
   - **Round 2** (36 of 82 failing) showed that nearly every confident wrong verdict comes from a missing extractor fact.
   - **Fable chose to pause:** `lane/jvm-scope-model` @ `a247f398` stays **unmerged**, because the unwired tables are placeholders. It waits for an extract-v98 "JVM declaration facts" concern: V1, V2, V3 type parameters, V5–V8, V9 inner/static, V10 companions, V11 header/body spans, V12 local extents, V13 parse-drop and V14 visibility.
   - 45 ignored probes are its acceptance suite.
   - The doc, marked paused, is on main. The card is in review, recorded as paused.
   - Bench baseline: +1.2–1.9% quiet on ChronosFlow, and +12% on the 30k-method file.
2. **ft-87b28bb6:** left in review with a note; Z3/Z5 ride on ft-2c800986 and v98.
3. **ft-8a85f117:** GAP-7 is VB.NET pattern-only, by decision. GAP-11/12/13 were each run once and all are measured-open; see the next section.
4. **ft-c86df5d4 competition benchmark:** on hold. Installing the pinned competitor binaries (CodeGraph, codebase-memory-mcp, Gortex, Graphify, GitNexus) needs your explicit OK, plus an idle machine.
5. **Consumer sweep (ft-3018ab76):** this session did the DevCouncil half. The rest is handed over, below.
6. **v98 unfreeze:** after the sweep.
7. **Cleanup** (the owner then asked to delete old and stale branches, 2026-10-22). All merged and patch-equivalent branches and their worktrees are deleted: 30+ branches, every lane/attack/agent worktree and the nested probe worktrees. Left in place:
   - `main`;
   - `lane/jvm-scope-model` (paused, unmerged);
   - `claude/literals-substring-b3546e` and its worktree (another session's; Fable said to leave it);
   - the remote `origin/dependabot/...` (deleting it would be a push);
   - two old stashes.

   The earlier cleanup notes:
   - Removed: the integration worktree and the known-gaps, liveness-script, resolver-bundle, attack/liveness-script and attack/build-cancel agent worktrees.
   - Left, because removal needs `--force`:
     - the agent-locked worktrees for kotlin-decl-proof, build-cancel, ts-namespace and attack/resolver-bundle;
     - the nested probe worktrees under `agent-a1ed67124fcd39a2b`, which hold untracked files;
     - `agent-a9bb175e705f145d9`, where deletion was refused with "Permission denied".
8. **Push main:** after the sweep. Not pushed.

### GAP-11/12/13, measured once (`measure/known-gaps-11-13` @ `6478b0f1`, merged as `500a55c8`)

- **GAP-11:** 62 of 120 audit property IDs have an ID-named test (heuristic).
- **GAP-12:** package-scope mutation of devmap-store `maintenance.rs`: 80 mutants, 63 caught, 13 missed (12 in `checkpoint_wal`), 4 unviable. The weekly `mutants.yml` job fails because `rust/mutants-baseline.txt` was never committed. Workspace-scope mutation is unviable because `include_str!` reaches outside `rust/`.
- **GAP-13:** a 40-cycle daemon soak was clean, and a 200-cycle CLI freshness loop passed after making its cycle count configurable and adding an assertion. Daemon-mode envelope freshness is still unchecked. The soak's default corpus fails because `main` is ambiguous.
- All of this is filed as **ft-ae653b10** (DevMap test-harness debt).

### ft-ae653b10 harness debt (merged, main `8381c35c`)

Fable scoped this narrowly.
- The 13 surviving `maintenance.rs` mutants are now killed: 80 mutants, 76 caught, 0 missed, 4 unviable.
- The soak now targets the unique symbol `<file>::_soak_<i>`; the 5- and 20-cycle runs pass.
- `mutants.yml` documents its scope.
- **Still open, and the owner's decision:** the weekly job cannot produce `mutants-baseline.txt`. The full scope is 4,509 mutants, which cannot fit the 350-minute budget. Either shard it with a `--shard k/n` matrix, or shrink the scope.
- GAP-11 property coverage and daemon envelope freshness stay out of scope.

### Wave of 2026-10-22 (owner: "Next wave"; plan by Fable)

All of this is merged to main, now `1d8a3505`.

**Records honesty, ft-0ed8604:**
- Docs only, with every claim checked against code.
- Version 0.2.5.
- The v0.2.4 notes are marked as never published.
- New DIVERGENCES rows X63–X66 cover this run's DevMap changes.
- The Kotlin confident-tier claim is corrected.
- The LSP row is corrected to opt-in.

**extract-v98 spec, `docs/devmap/design/extract-v98.md`:** a 28-row fact inventory with a probe map, a schema plan and two lanes.

**Quiet benchmark re-run, `benchmarks/results/2026-10-22-quiet-rerun/REPORT.md`:**
- Main vs pre-run main: +1.2% instructions on a cold build, with no difference on a one-file edit, `dead` or `impact`.
- The JVM tables add +0.46% per resolve round.

**Store write cost, `docs/devmap/design/store-write-cost-proposal.md` (ft-f6abefd9, measurement half):**
- The `generation_literals` carry is 52% of an edit's write.
- The card's two-relation prototype would save only about 4%, so it is a NO-GO.
- The non-schema wins come first. A v28 payload-scoped unit is projected at 27–29%, and that is the owner's call.

**Benchmark data and hygiene, ft-bc87301a and ft-7364b2c8:**
- 3,741 raw benchmark captures are untracked: tracked `benchmarks/` fell from 36.3 MB to 8.7 MB. Local ignored copies were restored.
- Two dead Python scripts are gone.
- 22 `z`-prefixed Go test files are renamed. Opus checked that the `init()` order still holds.
- The skills receipt is untracked, and `devmap doctor` warns when installed skills drift.
- `scripts/__pycache__` was removed.
- These are still yours:
  - `${CLAUDE_PROJECT_DIR}/`, which the gate cannot address;
  - `.venv`, which a system process holds open;
  - the `.devcouncil/*` files.

**Harness coverage, ft-2950b25e:**
- The mutants job is sharded per crate, a timed-out shard reports NOT RUN, and the `dc-glob` baseline is committed.
- `docs/devmap/property-coverage.md` maps all 58 IDs: 33 covered, 3 new tests, 14 retired, 10 uncovered.
- A new 20-cycle daemon test found a **real bug** and fixed it. Query envelopes said `fresh:true` while queued edits made the tree differ. They now report unverified while `pending_paths` is non-empty.

**Still open from this wave:**
- The `devmap-resolve` and `devmap-analyze` mutant baselines, from the first CI run.
- Envelopes read `fresh:null` after a rebuild until `status` runs.
- The non-schema store wins.

### Second wave of 2026-10-22 (merged; main `812ae141`)

**Store write wins (ft-f6abefd9, non-schema half):**
- `prepare_cached` for the fresh-row INSERTs.
- A one-statement literal carry.
- Three new `persist:write` spans (serialize, payloads, literals). `other` drops from about 47% to under 0.5% on a cold build, which meets acceptance #1.
- The six store tables are byte-identical to main's.
- Instructions: −2.4% on a cold build and −3.9% on a one-file edit. The Swift 30k `nodes` span is about 2.4× faster.

**Daemon freshness (ft-2950b25e follow-up):**
- After each successful drain, the daemon verifies freshness with a `status` walk, so query answers read `fresh:true` without a separate `status` call. Settled samples read true or null at 0/40 before the fix and 40/0 after. A race test passes.
- Cost: one tree walk per drain, about 2.2 s wall and 0.23 s user CPU on this repo at load 51. The third wave bounded it to one walk per quiet period.
- The liveness corpus-count comment and the `verify_task` description are corrected.

### Third wave of 2026-10-22 (merged)

**Property tests (GAP-11, merged `4ec5cfab`):**
- ID-named tests for B6, B7, B9, B12, N9, G9, G15, G24 and X12; S16 is retired.
- G15 is pinned as an ignored test, not fixed. `handler_return_keys` maps a handler with line 0 and no end line to the first 120 lines, so it absorbs every function's keys.
- The new `b_series_schema_cost_properties` target failed devmap-cli's classification gate on main. It is now declared `required-features = ["parse"]` (`7e554bd6`).
- The mutation check for these tests was NOT RUN.

**Bounded post-drain walk (ft-2950b25e follow-up):**
- The status walk after a drain is now a deferred job.
  - Every drain re-arms a 400 ms settle window, which `with_freshness_settle` can change.
  - The walk starts only when the store queue is empty and the watcher holds no undelivered event (`WatchBacklog`).
  - A queue that cannot be read counts as busy.
- The walk can now be cancelled (`scan_tree_cancellable`, `Store::status_cancellable`). A shutdown no longer waits for it, and a cancelled walk is an error that records no verdict.
- Truth guards are unchanged: `status` and `query_source_freshness` still refuse to certify over a non-empty queue.
- Tests: three new tests failed against the inline per-drain walk and pass now.
  - Storm test: 31 walks over 30 drains before, at most 3 after.
  - Shutdown test: before, the loop was still blocked 3 s after shutdown.
  - Pending test: no walk runs while an edit is queued or held.
  - A fourth test covers a cancelled walk.
- Measured with real-watcher storms of 50 edits at 1 s spacing; CPU is user+sys, n=2 per arm, load 7–52.
  - The watcher already coalesces such a storm into 2 drains, so walks fell from 2 to 1.
  - On a 20k-file synthetic tree, daemon CPU was about 9% lower (32.7/35.7 s before, 29.4/32.1 s after).
  - On `rust/` (1,682 files) the difference was inside noise.
  - The clear gains are the walk count and a shutdown that no longer blocks.
- Limitation: a drain that starts during a walk does not cancel it. The walk's verdict is still guarded by the generation re-check.

### Consumer sweep handover (run from sessions rooted in each repo)

`devcouncil install` reports these consumers stale against DevCouncil main (now `8381c35c`; use the HEAD at sweep time). The schemas are devmap-extract 97, dc-store 10 and workbench 14.

1. **GitPulse** (`/Users/bharath/Code/devtools/GitPulse`):
   - re-vendor: `node scripts/vendor-crates.mjs && bun run check:vendor-schema`, then commit;
   - copy Manvi's `contracts/serve.ops.schema.json` over GitPulse's copy and update the Rust side;
   - `bun run mcp:install && bun run mcp:doctor` (all three verdicts OK);
   - `bun run tauri build` and reinstall GitPulse.app.
2. **Manvi:** bump the DevCouncil pin in `scripts/module-pins.txt` from `1ae3bc15` to the DevCouncil HEAD at sweep time (devmap-extract 94→97, dc-store 9→10, workbench 13→14), then `manvi doctor`.
3. **Jarvis:** bump the DevCouncil pin in `upstream.lock.json` and `go.mod` from `1ae3bc15` to the DevCouncil HEAD at sweep time. Gusset must agree in all three places. Then `dev check-pins` and doctor.
4. **Confirm from the GitPulse checkout:** `bun run check:schema-parity` passes with no stale or unverifiable rows.
5. **Then:** push DevCouncil main once, and only then unfreeze extract-v98.
