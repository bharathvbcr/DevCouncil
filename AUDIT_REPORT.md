# DevMap accuracy audit — 2026-10-17

Read-only audit of DevMap on local `main` at `9030d08c`. No source, config, or dependency edits. The previous audit file was copied to `AUDIT_REPORT-2026-10-13.md` before this one was written.

Index at the start of the audit: generation **4530**. While the resolution-rate check was running, `generation_id` moved to **4531**. This session did not run `devmap build`. Counts did not move: 20914 symbols, 78076 edges, schema 27, `is_fresh` true, `pending_count` 0, `budget_refused.total` 0. MCP `repository.root` is `/Users/bharath/Code/devtools/DevCouncil`. The corpus is still partial: 1 file failed to parse (`scripts/install.ps1`, no PowerShell grammar), 4 HTML files recovered by pattern, `coverage_gaps.partial_parse.total` 56 (shown 50), 2 paths never indexed because they exceed 1 MiB. CLI `degraded_reason` carries the partial sentence. MCP `devmap_status.degraded_reason` is null on generation 4531. Empty DevMap answers on this index are incomplete, not proof that a symbol is absent. The five bugs filed against 4530 were rechecked on 4531 and still reproduce.

GitPulse git facets (`insights`, `collision_risk`, `active_changes`) returned `ok: false` with “git worktree deferred under load”. That is unscanned, not clean. Local `git worktree list` showed six other worktrees: swift lanes `attack/swift-properties` and `lane/swift-properties`, python lanes `attack4/py-wave8`, `lane/py-attack3-fixes`, and `attack3/py-followup`, and `accuracy/rustlabel`. Their uncommitted dirt was not scanned (`git -C` is blocked here). `treesitter.rs` is in the committed diffs of the swift and python lanes. `resolver.rs` is on the swift and rustlabel lanes. Findings that need those files are flagged in-flight.

Board before the first five cards: 122 tasks, `has_more` false (backlog 29, review 73, done 14, inbox 3, ready 1, in_progress 2). Re-pulled again before the sixth: 127 tasks, `has_more` false, which is those five and nothing else. In-progress cards were not touched: `ft-6a46dddd32f7be98090c2ffcd6401c0f` (Swift properties) and `ft-3018ab7669e7105678cfc313b0026c37` (extract-v95 re-vendor).

## Severity × dimension

Confirmed findings only. Rejected probes are below and are not in this table.

| Dimension | Critical | High | Medium | Low |
| --- | ---: | ---: | ---: | ---: |
| Modular architecture | 0 | 0 | 0 | 0 |
| Strict typing | 0 | 0 | 0 | 0 |
| Defensive validation at trust boundaries | 0 | 3 | 0 | 0 |
| Async error boundaries | 0 | 0 | 0 | 0 |
| Deterministic testability | 0 | 1 | 0 | 0 |
| Performance and benchmarks | 0 | 0 | 0 | 0 |
| Correctness (all platforms; no OS-specific bug confirmed) | 0 | 3 | 1 | 0 |
| Security | 0 | 0 | 0 | 0 |

Validation: search explains a miss by naming the wrong files; MCP status omits the analysis partial reason; an impact with no start says the generation is not persisted. Testability: the dead-list caveat says the confident tier is closed while a Go row is published at 0.9. Correctness, high: charged partial parses drop real declarations; a used Go type is confident-dead; `devmap_dependencies` returns the wrong direction and rejects a symbol name. Correctness, medium: the per-language resolution rate scores call-blind HTML at 1000 and scores vendored grammar C as this repository's C.

No performance finding was confirmed, so no benchmark-baseline card was filed. `ft-93ece913` already owns the blast/suspects double-read cost and was not re-measured.

## Top 10 risks

1. **Search sends the reader to files that were never indexed.** `DEFAULT_PARSE_BUDGET` and `is_metal_path` are absent because their bytes sit in charged ERROR ranges of an indexed file. Unscoped search says they may live in a 30 MB COBOL grammar or a competition JSON blob. Card `ft-75ddb66b`.
2. **A live Go type is confident-dead.** `toolsCallParams` (`server.go:95`, used at `:630`) is the only 0.9 row, with a null exemption reason. Deleting 0.9 rows removes the `tools/call` params type. Card `ft-19604c23`.
3. **MCP status looks clean.** `is_fresh` true and `degraded_reason` null, while the CLI on the same generation says the analysis is partial and 56 files have ERROR/MISSING regions. Card `ft-ce979db3`.
4. **Dependencies are callers.** The first 80 of 1892 edges for `rust/devmap-extract/src/lib.rs` are inbound. A symbol query for `RETRY_BUDGET_FACTOR` says the symbol is not indexed. Card `ft-c7d548e1`.
5. **Impact denies the generation it just read.** `devmap impact does_not_exist_audit_probe` returns `unresolved_namesakes.generation_id` 4531 and `source_freshness.reason` “no persisted generation is available”. Card `ft-b79f84e2`.
6. **The resolution rate is not call accuracy.** HTML is net 1000 with `extracts_calls` false (126 resolved, 0 unresolved). C is net 9 (130 resolved / 14065 unresolved) and all 13 indexed C files sit under `rust/vendor/grammars`. Card `ft-b7362179`.
7. **`treesitter.rs` is in flight on five lanes.** The extract half of risk 1 can move under those branches before this card is built. The search-explanation half does not need that file.
8. **Existing, not re-measured this session:** `ft-f9665721` (L-Rust method recall) and `ft-e221de3c` (L-Go). Eight sampled call sites on those paths were bound correctly. The corpus recall figures were not re-run.
9. **Existing, not re-attacked:** Python precision cards `ft-0e6cc90f` (UniqueGlobal) and `ft-d69f8f97` (field type leak at confidence 1.0), both in review. This audit did not open their fixtures.
10. **Fresh and partial are true together.** An agent that stops at `is_fresh: true` on MCP misses the partial corpus. CLI status already says partial. The two surfaces disagree, which is risk 3.

## New cards

Five are backlog, kind bug, priority 1, severity high. The sixth is backlog, kind bug, priority 2, severity medium. All carry `audit-2026-10-17`. None were folded. `related_tasks_exist` matched on shared title words plus the `devmap` label. The five were judged from the refusal titles and, for the close ones, from a full brief. The sixth's refusal listed 10 cards; each brief was read and none owns this rate. `reviewed_related` named every id. The related scan on the sixth was complete (113 open tasks compared). No user card was deleted or merged.

| Item | Concern | Wave | Files |
| --- | --- | --- | --- |
| `ft-75ddb66b0275405cb45dc1fbca839fdd` | Charged partial-parse holes drop `DEFAULT_PARSE_BUDGET` and `is_metal_path`; search blames the two never-indexed paths; a scoped search returns Available + 0 | 1 | `gaps.rs`, `treesitter.rs` (in flight), query tests |
| `ft-ce979db3138552e1128635f140b423b0` | MCP `degraded_reason` is null while CLI status joins `AnalysisStatus::Partial` | 1 | `protocol.rs`, `daemon.rs` |
| `ft-19604c23ec81ad9dbd830a28af76d9e8` | `toolsCallParams` at 0.9, and the dead caveat claims that tier is shut | 1 | `liveness.rs`, Go fixture test |
| `ft-c7d548e17501f99275dd1e6d2f23eed3` | `devmap_dependencies` is bidirectional and symbol-blind | 2 | `engine.rs`, `query.rs`, `mcp.rs` |
| `ft-b79f84e286e6df54d0b6c0e973aa47ef` | No-start impact and depth-exhausted `trace_between` stamp “no persisted generation” | 3 | `engine.rs`, `budget.rs` |
| `ft-b7362179279015b377706808156a5370` | Call-blind HTML publishes net 1000; the C rate is 13 vendored grammar files | 1 | `resolution_rate.rs`, `build.rs` (`print_resolution_rate` only) |

### Wave order

Wave 1 is at the cap of four, with no shared files: partial-search, MCP status, confident-dead, and the resolution-rate card. The search half of partial-search (`gaps.rs`) can start while `treesitter.rs` is held by the swift and python lanes. The grammar recovery waits. The rate card edits `print_resolution_rate` in `build.rs`. `ft-c13da1b7` and `ft-2e510bf3` also name `build.rs`; both logged those edits as already landed and are in review. Do not start the rate card in a wave that is still editing that function.

Wave 2 is the dependencies card. It edits `engine.rs`.

Wave 3 is the traversal-freshness card. It also edits `engine.rs`, so it starts after wave 2 leaves that file.

## Folded cards

None. Close calls that stayed separate:

- `ft-47124ed7` is the budget-refused status kind. This checkout’s `budget_refused.total` is 0, and the partial sentence is the analysis sentence.
- `ft-cc6c12cb` is the over-1 MiB drop. `treesitter.rs` is 731208 bytes.
- `ft-e007ef1b` is the 5 s wall-clock refusal. No file was budget-refused in generation 4530.
- `ft-c13da1b7` is a long-lived `fresh: true` with no age. Its brief says a one-shot CLI is unaffected. The freshness bug reproduced here is a one-shot CLI saying no generation exists.
- `ft-7327fd12` is two copies of the confident-tier predicate and the runner scoring capped rows as 0.4. The new dead bug is a live type at 0.9.
- `ft-90cef8de` only splits `treesitter.rs`. A dropped declaration is a different fix.

## Verified-stale cards

`ft-47124ed7` (revision 3, backlog) already carries a 2026-10-17 log that marks criterion (a) and (d) met and moves (b)/(c) to `ft-09f2437e` and `ft-292df47e`. This pass re-read `status.rs:347-364`: the CLI joins a `budget-degraded:` clause only when `budget_refused.total > 0`, and live status has total 0. The tests named in that log were not re-run. The card was left as written. Nothing was moved to done.

`ft-c13da1b7` criteria 1 and 2 are recorded done in its review log (merged locally, not pushed). Criteria 3 and 4 (freshness age) are still open. Not re-tested here.

## What was verified

Source reads are from this checkout. Live rows were taken on generation 4530 and rechecked on 4531 after the index moved. The recheck is the number cited where the two could differ.

**Charged holes.** `treesitter.rs` charged bytes 3543..3594 are line 77, `pub const DEFAULT_PARSE_BUDGET`. Bytes 1513..1803 and 1831..1837 are lines 41–48, the doc comment and signature of `is_metal_path` (`> bool`). `RETRY_BUDGET_FACTOR` at line 85 is indexed. Unscoped search for `DEFAULT_PARSE_BUDGET` is Unavailable and names `gortex-source-tree.json` and `rust/vendor/grammars/cobol/parser.c`. That blame still holds on 4531. Scoped search of that file, kind function, is Available, total 0, 345 functions in the file. `outside_index_reason` (`gaps.rs:241-247`) reads only discovery refusals and over-limit paths. The liveness tests at `liveness.rs:3677` and `:3750` require the hole to be listed and charged. They do not require the dropped name to be searchable. Listing works: status shows the charged ranges. Search does not use them.

**MCP status.** CLI `status.rs:358-364` joins the analysis partial reason. Daemon `protocol.rs:1274` sets `degraded_reason` from `daemon_degraded_reason` (`protocol.rs:1227-1233`), freshness plus unapplied edits. `daemon.rs:5166` expects null on a clean fixture. A clean fixture must stay null. This generation is not that fixture: the analysis is partial.

**Dependencies.** `mcp.rs:1197-1204` says outbound edges and accepts a symbol name. `engine.rs:311-315` looks up `file_edges` on the raw string. Live on 4531: `devmap deps RETRY_BUDGET_FACTOR` is Unavailable, “is not indexed”. `query.rs:1016` matches either endpoint, confidence descending. The first page of `lib.rs` was 80 of 1892, all inbound, on 4530. The recheck page was 100 of 1892, still all inbound. The hidden remainder was not classified.

**Freshness.** `unavailable_response` (`budget.rs:111-124`) stamps the no-generation sentence. `traverse` (`engine.rs:906-925`) does not call `finish`. The no-start arm (`engine.rs:1246-1264`) returns that response, then impact attaches namesakes for the loaded generation. Recheck on 4531: impact of `does_not_exist_audit_probe` has resolution “has no indexed traversal start”, freshness “no persisted generation”, and `unresolved_namesakes.generation_id` 4531. `query_source_freshness` uses that sentence only when there is no generation (`generation.rs:899-902`). A found `trace_between` does call `finish` and says freshness was not checked, which is the honest empty-cache sentence.

**Dead.** One row at 0.9: `toolsCallParams`, `server.go:95`, used at `:630`. Still the only row at or above 0.89 on the 4531 dead page (total 80, shown 80). `degraded_reason` (`liveness.rs:597-601`) always says the confident tier is closed when `is_complete()` is false. `cap_for` (`liveness.rs:835-837`) and the 0.9 arm (`liveness.rs:3574-3575`) leave Go uncapped. `HIGHEST_DEGRADED_CONFIDENCE` is 0.89 (`liveness.rs:1057`). Where the type reference is dropped, extract versus resolve, was not re-extracted.

**Resolution rate.** `resolution_rate.rs:12-14` says a language with no call extractor sits at zero. `resolution_rate.rs:285-299` divides every non-structural edge and does not look at `extracts_calls`. HTML's capability is REFERENCES only (`languages.rs:873`). MCP status on 4531: html net 1000, resolved 126, unresolved 0, `extracts_calls` false, `blind_to` includes calls. The test at `rust/devmap-analyze/tests/resolution_rate.rs:42-68` only allows None or 0 when nothing resolved, so HTML escapes it. `build.rs:1335-1357` prints a percent for `Some(net)` and then strips the word "calls". The human cap is 8 (`build.rs:1382`). HTML is 9th, so the build summary does not print that 100% line; the JSON field does. C is 2nd: net 9, 130 resolved, 14065 unresolved. `scope.rs:159-182` counts files admitted by language and path together. Unscoped `--language c` and `--path rust/vendor/grammars` are both 13 files and 187 symbols. The 13 paths are cobol, liquid, and vue grammar sources under `rust/vendor/grammars`. `resolution_rate.rs:210-229` has no vendored filter. Liveness skips a vendored partial parse (`liveness.rs:2503-2507`) because that file cannot call this repository. The 30 MB cobol `parser.c` is the over-limit drop, not one of the 13.

## Rejected findings

- **HTML 1000 as "already disclosed, so not a bug."** Withdrawn. `blind_to` does sit beside the number, and a careful JSON reader can see `extracts_calls: false`. The number the module comment tells that reader to trust is still 1000, and the printer strips "calls" before it speaks. Filed as `ft-b7362179`. The human summary on this corpus does not print the HTML line, because HTML sorts 9th and the cap is 8. That limit is in the card. It is not a reason to drop the JSON field.
- **Eight resolver call sites** (Go import resolution, Rust qualified calls, `impact` → `traverse`, in-memory impact, a Ruby send, `runGateSet`, `runGussetCheck`, `console.Errorf`) bind the callees the source names, at confidence 1.0, on the rungs those calls deserve. No new false edge. L-Rust and L-Go stay as they are.
- **`min_rung` histogram on an empty trace.** The resolver slice read a depth-cap message as a hidden rung filter. `trace_between` does not apply `min_rung`. A found trace of `read_artifact` → `MemoryProvider.load_artifact` returned the edge. The freshness half of that slice is the card above. The histogram half is dropped.
- **0.4 dead rows.** Fourteen sampled live symbols sit at 0.4 with an explicit reason (unresolved namesake or `exported_unreferenced`). Two `#[allow(dead_code)]` symbols (`shell_command`, `PROJECT_DIR`) are true dead and stay at 0.4. The list is doing what it says for those rows.
- **Go function-value registry.** `runStatus` / `runDoctor` are not in this tree. Not re-filed.
- **Empty `dead_clusters`.** The incomplete flag is unset, which the model treats as “the pass ran and found none”. No abandoned mutual-call pair was shown.
- **SQL `work_events` search miss.** The name sits in a charged range of `schema.sql` and receives the same wrong outside-index blame. Whether a clean SQL parse would have emitted the table was not proven, so it is evidence for the search card, not a separate extractor card.
- **C at 9 permille as "not measured."** Withdrawn. Measured on 4531: the language scope and the `rust/vendor/grammars` scope are the same 13 files and 187 symbols. Same card as the HTML row. The 30 MB parser that never indexed stays on `ft-cc6c12cb`.

## Decisions made without asking

- Subagent model could not be pinned. `spawn_subagent` has no model argument in this session. Audit slices ran as `code-reader`. Resolver and dead returned before the first five cards. The extraction slice returned after that write. Its two new findings (the HTML printer and the vendored C rate) were re-read in source and rechecked live, then filed as one card. Its other probes were the same partial-parse and over-limit holes already on `ft-75ddb66b` and `ft-cc6c12cb`, and were not re-filed. Query-honesty was still running when this report was updated (about 39 minutes, 460 tool calls). Findings from that slice are not on the board yet.
- Call-blind `net_permille` becomes None. A second reference-rate field was the other option and was not chosen. The published HTML field therefore changes from 1000 to null. Override that before wave 1 if the field has to stay a number.
- The index generation moved from 4530 to 4531 during the audit. This session did not rebuild it. The recheck is what the report cites for the live rows.
- The dead-caveat fix is to change the sentence so it names languages still eligible for 0.9. Go is not capped down to 0.4. Capping Go would hide true Go deads and collide with `ft-b812a97c`.
- The search explanation lands without waiting on grammar recovery in `treesitter.rs`.
- Freshness was not folded into `ft-c13da1b7`.
- `ft-47124ed7` was not rewritten. Its log already records the split, and the tests were not re-run here.
- The 2026-10-13 `AUDIT_REPORT.md` was copied to `AUDIT_REPORT-2026-10-13.md` rather than overwritten in place.
- GitPulse git facets were treated as unknown after they deferred. Local worktree names were used only as a collision flag.
- No benchmark card. No performance finding was confirmed.
- Briefs were read in full for `ft-47124ed7` and `ft-c13da1b7`. Other related-task refusals were judged from the titles the refusal printed, plus the prior full read of `ft-f9665721` (truncated at 76 KB in the earlier session). 108 open briefs were not each opened.

## Owner decisions needed before building

One override, not a blocker: if HTML `net_permille` must stay a number, say so before wave 1. Otherwise it becomes None, which is what the card says. Do not edit `treesitter.rs` or `resolver.rs` until the swift, python, and `accuracy/rustlabel` lanes land, and do not touch the two in-progress cards. Wave 1 is full. The rate card shares `build.rs` only at file level with two review cards whose edits are logged as landed; it does not share a function with the other three wave-1 cards.

Stopped here. No implementation until you say to build.
