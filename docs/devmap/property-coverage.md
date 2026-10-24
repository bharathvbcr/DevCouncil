# DevMap property coverage (GAP-11)

The 58 property IDs that the 2026 harness audit found without an ID-named
test. Each is mapped to a test, to a new test added with this document, to a
retirement with evidence, or to an honest "uncovered".

## Two namespaces share these IDs

- **PLAN** = `docs/archive/devmap/PLAN.md` section 3: `B*`, `N*`, `D*`, `K*` and
  the Gortex-derived `G1`-`G8`.
- **AUDIT** = `docs/archive/devmap/AUDIT.md` findings against the Python
  incumbent: `G9`+, `S*`, `V*`, `X*`. AUDIT also defines `G1` (CFG builder) and
  `G2` (leader-statement lines), which collide with PLAN `G1`/`G2`; both
  meanings are listed.
- The tests in `devmap-resolve/tests/py_receiver_residue_attack*.rs` named
  `b01`..`b16` / `s08`..`s12` / `n10` are attack-bundle numbering, not these IDs.

The Python engine is deleted (`docs/devmap/DIVERGENCES.md`, header: retired in
Phase 7). `rg -uu` over `rust/` finds none of `name_index`, `interrupt_writes`,
`get_sync_coordinator`. Findings that describe only Python code are retired.

Status: **covered** (an existing test asserts it), **new** (added here, proved
to fail under mutation), **retire**, **uncovered**.

| ID | Source | Property | Status | Test or reason |
|---|---|---|---|---|
| B2 | PLAN | Back-pressure, not silent discard, on writer contention | covered | `devmap-store/tests/kernel_defects.rs::k13_writer_lock_is_exclusive_and_names_the_holder_on_timeout`; `devmap-cli/tests/kernel_defects.rs::k13_a_build_waits_for_a_held_writer_lock_and_never_reports_database_is_locked`; `devmap-store/tests/kernel_defects.rs::k1_claim_acknowledgement_survives_a_concurrent_requeue` |
| B6 | PLAN | Generation prune is a bounded delete | new | `devmap-store/tests/b_series_schema_cost_properties.rs::b6_every_prune_delete_is_an_index_search_not_a_table_scan` (+ `b6_the_plan_check_is_not_vacuous`). Cost is pinned by query plan (every per-generation prune delete is a SEARCH on `generation_id`, FTS via `nodes_fts_map`), not by wall time. The table list mirrors `prune_generations_except_latest`; a table added there must be added to the test. Correctness: `devmap-cli/tests/test_generation_retention.rs::pruned_generations_leave_no_rows_behind_in_any_table` |
| B7 | PLAN | Cache admission is O(1) | new | `devmap-store/tests/b_series_schema_cost_properties.rs::b7_admission_cost_does_not_grow_with_the_cache` (min-of-batches, 8x ratio between a near-empty and a 50,000-row cache; timing-based, so it is the one new test that load can in principle perturb). Identity: `extract_cache.rs::identical_content_never_aliases_file_identity_through_cache` |
| B8 | PLAN | Eviction follows liveness, not age | covered | `devmap-store/tests/store_hardening.rs::s4_prune_evicts_a_cache_row_no_retained_generation_can_ever_serve`; `devmap-cli/tests/test_generation_retention.rs::superseded_extraction_cache_entries_are_evicted` |
| B9 | PLAN | No redundant column in any primary key | new | `devmap-store/tests/b_series_schema_cost_properties.rs::b9_no_primary_key_carries_a_column_its_ordinal_already_determines`: every `(generation_id, ordinal)`-keyed table has exactly that key and none keys on `payload_hash`. Narrow by construction: "redundant" is decided only for ordinal-keyed tables |
| B10 | PLAN | Paths interned | covered | `devmap-store/tests/retention_churn.rs::repeated_renames_bound_paths_to_the_retained_generations`; `devmap-store/tests/adversarial_store_without_grammars.rs::audit_concurrent_path_interns_reuse_one_identity`. The "index under 60 MB" size gate is not tested |
| B11 | PLAN | Free space reclaimed | covered | `devmap-store/tests/kernel_defects.rs::k2_incremental_vacuum_frees_every_page_it_was_asked_for`; `devmap-cli/tests/kernel_defects.rs::k2_a_no_change_build_reclaims_a_large_freelist_from_the_main_file`; `devmap-store/tests/store_hardening.rs::new_stores_use_incremental_auto_vacuum_and_reclaim_without_a_full_rewrite` |
| B12 | PLAN | Extraction saturates cores | new | `devmap-store/src/extract_cache.rs::b12_n9_extraction_saturates_every_core_with_no_fixed_ceiling`: a slow-extractor build over 8x-cores files reaches every worker of the pool. Scaling is observed as workers reached, not wall time |
| B13 | PLAN | No repeated Python `Path` parsing in hot loops | retire | Python-only (1.09M `Path` calls); engine deleted, DIVERGENCES header |
| B14 | PLAN | One tree pass per file instead of regex sweeps (`routes.py`, `di.py`) | retire | Python regex sweeps; the kernel extracts with tree-sitter in one pass (AUDIT section 9.1) |
| B15 | PLAN | Language resolved once per file | retire | Python `resolve.py`; the kernel's language table is one compile-time authority (AUDIT section 9.7), and `devmap-cli/tests/test_findings_suite.rs::test_language_extractor_routing_all_specs` pins routing |
| N1 | PLAN | Queries never deserialise the whole graph | covered | `devmap-cli/tests/test_findings_suite.rs::test_explore_p95_soft_ratchet_2k` (soft ratchet at 2k files, not the 10k in the acceptance text) |
| N2 | PLAN | Deletions and renames are explicit | covered | `devmap-cli/tests/test_persisted_cli.rs::staleness_audit_add_rename_delete_and_restore_are_not_silent`; `devmap-serve/tests/daemon_drain_races.rs::ra3_deleted_sources_are_stale_even_without_a_watcher` |
| N3 | PLAN | No dual-write of a second full serialisation | covered | `devmap-cli/tests/test_code_graph_artifact.rs::the_interned_graph_is_opt_in_and_decodes_to_the_verbose_one` (the second encoding is opt-in; the store is canonical). Inferred, not a test of "one store" as such |
| N6 | PLAN | Receiver types tracked from constructors | covered | `devmap-resolve/tests/test_phase3_hardening.rs::constructor_assignment_drives_receiver_resolution_not_method_name`; `::ambiguous_constructor_types_do_not_seed_receiver_resolution` |
| N7 | PLAN | Retrieval fuses lexical and semantic ranking | retire | DIVERGENCES row N7: retired with the semantic layer; `test_findings_suite.rs::lexical_search_operates_while_rrf_remains_open` (name predates the retirement) |
| N8 | PLAN | Ingestion owned by a long-lived watcher | covered | `devmap-serve/src/daemon.rs::test_b5_s17_connect_time_sweep_enqueues_changed_new_and_deleted_files`; `devmap-serve/src/watcher.rs::test_s12_ignore_cache_reuses_rules_and_invalidates_on_rule_change` |
| N9 | PLAN | Pools sized from real cores and RAM | new (partial) | Same test as B12: after a build, `rayon::current_num_threads()` equals `available_parallelism()` (or `RAYON_NUM_THREADS`), so nothing clamps the extraction pool. The "and RAM" half is not implemented anywhere (the pool is core-sized only) and is not asserted |
| N10 | PLAN | Query surface honest about capability | covered | `devmap-query/src/cypher.rs::an_or_is_refused_because_this_engine_only_conjoins`; `::a_query_this_subset_cannot_read_is_refused_with_the_shape_it_wants`; `::a_pattern_this_subset_cannot_read_is_refused_not_answered_as_every_node` |
| D2 | PLAN | One schema, no shadow tables | new | `devmap-store/tests/one_schema_no_shadow_tables.rs::d2_a_fresh_store_and_its_reopen_carry_no_shadow_table` (+ `d2_the_shadow_detector_is_not_vacuous`) |
| D3 | PLAN | Lock status reports the true holder | covered | `devmap-store/tests/kernel_defects.rs::k13_writer_lock_is_exclusive_and_names_the_holder_on_timeout`; `devmap-store/src/db/tests/bounded_claim_tests.rs::s6_writer_lock_contention_still_polls_to_the_deadline_and_names_the_holder` |
| K8 | PLAN | Phase profiler attributes cost to the phase that incurred it | covered | `devmap-cli/tests/cli_json_contract.rs::the_unchanged_build_reports_its_timings`; `devmap-cli/tests/test_progress.rs::an_unchanged_build_reports_its_own_timings`. Inferred: they pin stage presence; the attribution regression is described in PLAN as closed with a test |
| G1 | PLAN (breadth beyond grammars) | Grammarless languages still recovered | covered | `devmap-extract/src/fallback.rs::recovers_protobuf_messages_services_and_rpcs`; `::recovers_powershell_and_visual_basic_declarations` |
| G1 | AUDIT (CFG wiring) | Branch/loop/try bodies wired into the CFG | covered | `devmap-analyze/src/pdg.rs::cfg_threads_branch_loop_try_and_terminators` |
| G2 | PLAN (lexical + semantic ranking) | An exact term ranks first | covered | `devmap-query/src/semantic.rs::ranks_a_name_the_query_does_not_literally_contain`; `::a_term_common_to_every_document_still_ranks` |
| G2 | AUDIT (leader lines) | A branch condition's use is visible to def-use | new | `devmap-analyze/src/pdg.rs::g2_a_branch_condition_use_is_a_data_edge_from_its_definition` |
| G9 | AUDIT | api_impact scans fetch sites once per query | new | `devmap-query/src/api_routes.rs::g9_api_impact_visits_each_client_file_once_per_query`: cancellation checks (one per file visit) equal the file count and `source_bytes_read` equals the bytes on disk, so a second scan doubles both |
| G10 | AUDIT | PDG query does not reload shards per path; store errors not swallowed | covered | `devmap-cli/tests/test_findings_suite.rs::extraction_cache_round_trip` (weak: DIVERGENCES G10 says it does not time a PDG batch) |
| G11 | AUDIT | Query shim does not advertise relationships that cannot match | covered | `devmap-query/src/cypher.rs::a_relationship_the_graph_cannot_emit_is_named_not_answered_empty` |
| G13 | AUDIT | Reaching definitions keep loop-carried dependencies | covered | `devmap-analyze/src/pdg.rs::reaching_definitions_keep_loop_carried_edges_and_exact_taint` |
| G15 | AUDIT | Handler return-key extraction is not tautological; line-0 handlers do not absorb every function's keys | new (half) + VIOLATED (half) | Own-span half: `devmap-query/src/api_routes.rs::g15_a_handler_returns_only_the_keys_inside_its_own_span`. Line-0 half is violated today: `handler_return_keys` maps a handler with `line` 0 and no `end_line` to the file's first 120 lines, so it returns every function's keys (`["ids","order_id","total"]`). Committed as `#[ignore = "ft-2950b25e: ..."]` `g15_a_line_zero_handler_does_not_absorb_every_functions_keys`. Whether a real route graph emits a line-0 handler with a source path is not verified |
| G17 | AUDIT | `./` stripped as a prefix, not a character set, so dot-directories keep their dot | new | `devmap-query/src/scope.rs::g17_a_dot_directory_keeps_its_leading_dot` |
| G22 | AUDIT | OKF export neighbour-area scan | retire | DIVERGENCES EXPORT-4: OKF half "Not ported"; the kernel exports GraphML only |
| G23 | AUDIT | Parameters are taint sources | covered | `devmap-analyze/src/pdg.rs::reaching_definitions_keep_loop_carried_edges_and_exact_taint` (taint flows from a `params` entry to the sink); `::a_try_body_definition_reaches_a_handler_use` |
| G24 | AUDIT | Confidence-aware budget priority | new | `devmap-query/src/engine/tests/tests.rs::g24_a_truncated_caller_list_keeps_the_proven_callers_over_ambiguous_ones` (in-memory engine) and `::g24_the_store_engine_truncates_callers_in_the_same_order` (store engine): ambiguous edges inserted first and outnumbering proven ones; a 4-item budget keeps only confidence-1.0 callers. Dead candidates: `coverage_loss_artifacts.rs::dead_symbol_candidates_rank_by_confidence_before_truncating` |
| G25 | AUDIT | `build_file_and_symbol_nodes` builds an unused `name_index` | retire | Python function; `rg -uu name_index rust` finds nothing |
| S13 | AUDIT | Migration ladder not run on every `put_extraction` | covered | `devmap-store/tests/migration_ladder.rs::a_migrated_store_reopens_without_migrating_again` (migrations run at open) |
| S15 | AUDIT | Windows lease size-probe and guard byte | retire | Python lease file; the daemon is the sole writer (AUDIT section 9.4: lease heuristics "are deleted, not ported") |
| S16 | AUDIT | Rename aliases survive across generations | retire | The Python `aliases()` rename mapping has no Rust counterpart: `rg -uu` for `rename_alias`/`aliases(`/`rename_map` over `rust/` finds none in the store (only `dc-regress` `renamed_from`, a git-diff field, and an MCP query-parameter alias), and PLAN section 7.5 states renames are "a delete plus an add ... loses rename identity, which nothing downstream consumes". The `GENERATION_RETENTION` comment in `devmap-store/src/schema.rs` that cites "rename-alias chaining" is stale |
| S17 | AUDIT | Racy-stat window: stat-read-stat with hash confirmation | covered | `devmap-serve/src/daemon.rs::stat_read_stat_never_accepts_a_concurrently_changing_file`; `::test_b5_s17_connect_time_sweep_enqueues_changed_new_and_deleted_files` |
| S19 | AUDIT | `get_sync_coordinator` kwargs check | retire | Python; `rg -uu get_sync_coordinator rust` finds nothing |
| S20 | AUDIT | `interrupt_writes` tracks every write connection | retire | Python; `rg -uu interrupt_writes rust` finds nothing |
| V2 | AUDIT | Graph page does not inline an unbounded graph | covered | `devmap-query/src/viz.rs::a_capped_view_reports_both_numbers`; `::the_cap_keeps_the_hubs_not_an_arbitrary_sample` |
| V3 | AUDIT | Symbol-mode edges consistent with what is rendered | covered | `devmap-query/src/viz.rs::no_link_survives_whose_endpoint_was_cut`; `::flags_come_from_the_graphs_own_verdicts`; `devmap-query/tests/viz_projection.rs::missing_and_ambiguous_endpoints_never_become_phantom_dependencies` |
| V4 | AUDIT | Stale dependents never served as fresh | covered | `devmap-serve/tests/attack2_daemon_verdict_on_every_part.rs::every_part_of_every_daemon_and_mcp_answer_carries_the_stale_verdict`; `devmap-serve/tests/daemon_drain_races.rs::ra3_old_analyzer_payloads_are_stale_even_when_source_matches` |
| V7 | AUDIT | Framework detection not a substring over config files | covered | `devmap-query/tests/every_emitted_field_says_whether_it_ran.rs::frameworks_are_derived_from_the_routes_that_prove_them` |
| V8 | AUDIT | Test commands are not fabricated | covered | `devmap-query/src/inventory.rs::a_mention_of_test_is_not_a_declaration_of_it` (a declared target is required). Does not name ruff/mypy specifically |
| V10 | AUDIT | JS dead-symbol scan re-splits the file per symbol | retire | Python scan; the kernel dead-symbol pass reads the graph, not source text (`devmap-query/tests/retired_python_claims_have_kernel_tests.rs::a_function_named_only_in_comments_and_strings_stays_dead`) |
| V11 | AUDIT | MCP map handlers re-parse the map per call | retire | Python handlers; the MCP server is Rust (`devmap-serve/src/mcp.rs`), answers from the store |
| V13 | AUDIT | Viz badges show N of M, not capped counts as totals | covered | `devmap-query/src/viz.rs::a_capped_view_reports_both_numbers`; `devmap-query/src/map_preview.rs::the_unresolved_list_is_capped_but_its_true_total_is_not` |
| V15 | AUDIT | `refresh_map_artifacts` serialises twice | retire | Python function; artifact writes are `devmap-query/src/artifacts.rs::test_v14_atomic_write_and_fingerprint_skip` |
| V18 | AUDIT | Source-root detection not all-or-nothing | covered | `devmap-query/tests/retired_python_claims_have_kernel_tests.rs::subsystems_on_a_foreign_tree_are_named_by_its_own_directories` (inferred: pins per-directory subsystem naming on a mixed tree) |
| V20 | AUDIT | Dead `.sqlite/.db` database-kind branch | retire | Python classifier branch; no such classifier in `devmap-query/src/inventory.rs` |
| X4 | AUDIT | Only exported module-level declarators become symbols | covered | `devmap-extract/src/treesitter/tests.rs::exported_module_level_bindings_are_symbols` (Rust/Go function-local excluded; TS function-local nesting not asserted) |
| X11 | AUDIT | LSP initialize timeout is a failure, not success | covered | `devmap-resolve/src/lsp.rs::a_silent_server_times_out_at_the_deadline_and_takes_its_children_with_it`. DIVERGENCES calls LSP "Cut"; the opt-in `--lsp` client exists and is tested, so that row is stale |
| X12 | AUDIT | LSP client cannot deadlock on full pipes | new | `devmap-resolve/src/lsp.rs::x12_a_server_that_stops_reading_cannot_wedge_the_client_writes` (server answers `initialize` then never reads; 200 x 200 KB notifications end in `TimedOut` inside the deadline, run on a thread with a 10 s guard). Mid-body stall: `lsp.rs::a_server_that_stalls_mid_body_times_out_at_the_deadline` |
| X13 | AUDIT | LSP positions in UTF-16 units | covered | `devmap-resolve/src/lsp.rs::the_line_index_matches_a_prefix_scan` (UTF-16 line index; `positionEncodings` negotiation not tested) |
| X15 | AUDIT | Embedding model name is part of the cache key | retire | Embeddings gone; `devmap-query/src/semantic.rs` is stateless TF-IDF computed per query (PLAN G2) |
| X17 | AUDIT | Import extraction sees Go block imports and Rust `use` | covered | `devmap-extract/tests/test_phase2_hardening.rs::test_x9_rust_grouped_use`; `devmap-extract/tests/rust_use_declarations.rs::an_alias_is_the_local_name_and_the_original_is_the_imported_one`. Go block imports not located by name |
| X19 | AUDIT | MCP handlers do not block the event loop | covered | `devmap-serve/src/mcp.rs::audit_an_internal_timeout_returns_a_response_instead_of_silence` (bounded work; blocking-free dispatch not separately tested) |

## Counts

58 IDs, 60 rows (G1 and G2 appear once per namespace).

| Status | Rows |
|---|---|
| covered | 33 |
| new | 10 (D2, G2 AUDIT, G17, B6, B7, B9, B12, G9, G24, X12) |
| new, partial | 2 (N9: RAM half not implemented; G15: line-0 half violated, ignored test `ft-2950b25e`) |
| retire | 15 (adds S16) |
| uncovered | 0 |

## Mutation results (lane `property-mutation`, base 44d37505)

`cargo mutants --in-place`, scoped with `--file` and `--re` to the production
functions each property exercises, and with the property's own test selected
(`--test b_series_schema_cost_properties`, or `--lib <test-name-prefix>`), so a
mutant is "caught" only if that property's test fails. No whole-crate run was
made. "Caught" counts viable mutants; unviable ones (no compiling replacement)
are listed separately. The first number of each pair is before the lane's
strengthening, the second after. Hand mutants were applied with a one-line edit,
run, and reverted (`git checkout`).

| ID | Functions exercised | cargo-mutants tried (viable / unviable) | Caught before -> after | Hand mutants (all caught) | Survivors |
|---|---|---|---|---|---|
| B6 | `Store::prune_generations_except_latest`; the DDL in `schema.rs` (SQL text, which cargo-mutants cannot mutate) | 7 / 0 | 0 -> 7 | `generation_dead_symbols` key reordered to `(ordinal, generation_id)`: the plan check fails with `SCAN generation_dead_symbols` | none |
| B7 | `Store::admit_cached_extraction` | 2 / 0 | 0 -> 2 | a full `COUNT(DISTINCT payload_json)` scan added inside admit: 12.7 s per 200 against 79 ms, the 8x bound fails | none |
| B9 | none: a schema property over DDL, no function to mutate | NOT RUN (nothing cargo-mutants can mutate) | n/a | `file_path` appended to the `generation_dead_symbols` primary key: fails. The other three ordinal-keyed tables were not mutated (NOT RUN) | n/a. Test unchanged |
| B12 | `extract_scanned_with_progress`, `extract_one_cached_with`, `extract_scanned_for_generation` | 7 / 4 | 1 -> 4 | `par_iter()` -> `iter()`: fails ("144 files ran on 1 workers of a 18-wide pool") | 3, below |
| N9 | the same test as B12; the pool is rayon's global, and no production code sizes it | NOT RUN: no production function configures the pool, so there is nothing to mutate | n/a | a pool clamp (NOT RUN, no site exists) | n/a. The RAM half stays unimplemented and unasserted |
| G9 | `scan_sites`, `graph_files`, `route_map`, `api_impact` (not `handler_return_keys`, which G15's lane owns) | 75 / 6 | 22 -> 26 | `api_impact` runs `route_map` twice: the per-file cancellation count (6, not 3) fails | 49 against G9 alone, 20 against every `api_routes` test; below |
| G24 | `budget_take`, `QueryEngine::impact`, `StoreQueryEngine::impact`, `StoreQueryEngine::traverse_walked` | 11 / 20 | 5 -> 5 (all five are `budget_take`) | the confidence comparator reversed in `in_memory.rs` (fails the in-memory test) and in `engine.rs` (fails the store-engine test): 2 of 2. The comparator is a closure argument, which cargo-mutants has no mutation for | 6, below |
| X12 | `LspClient::write_message`, `notify`, `kill`, `request`, `read_message`, `spawn_with` | 15 / 2 | 7 -> 10 | `try_send` -> a blocking `send`: fails ("the client blocked writing to a server that stopped reading") | 5, below |

Strengthening made (each fails against the mutant above, shown with the hand
mutant or the cargo-mutants run, never only by reading):

- B6: `b6_prune_removes_exactly_the_generations_outside_the_window` runs the real
  prune over four generations (keep 1, keep 2, and no-op cases) and checks the
  returned count, every per-generation row of pruned and retained generations,
  and the validity-range reclaim at the cutoff.
  `b6_the_checked_deletes_are_the_ones_prune_issues` reads the prune's own
  `DELETE` statements so the plan test cannot drift from the code.
- B7: after the timed batches, every admitted extraction is a row and the retry
  ledger is empty, so a no-op admit no longer passes as "O(1)".
- B12: the pass through `extract_scanned_for_generation` returns one extraction
  per file in scan order, and nothing is admitted to the cache before the
  generation commits.
- G9: `files_eligible`, `files_attempted`, `sites_found` and the consumer count
  are pinned beside the cancellation count and bytes read.
- X12: the server's pid must be dead after the writer gives up, checked while the
  client is still alive (dropping it would reap the server and hide a missing
  `kill`); `x12_the_payload_cap_admits_exactly_the_cap_and_refuses_one_byte_more`
  pins the `write_message` size guard exactly.

Survivors and their disposition:

| ID | Survivor | Disposition |
|---|---|---|
| B12 | `extract_cache.rs:96` delete field `cancel`; `:160` delete `!` (progress `withheld`); `:213` `cached.file_path == path` to `!=` | Outside B12/N9 (cancellation, progress accounting, path-bound cache reuse). Each is killed by the module's other tests: a run with every `extract_cache` test caught them all except `:218`, which B12 now kills |
| G9 | 20 mutants no `api_routes` test kills: `scan_sites` 377 (`-` to `+`), 385 (guard `true`), 386 (`<` to `==`, `<=`), 387 (`+=` to `-=`, `*=`), 394 (`*=`), 428 (guard `true`; `||` to `&&`), 438 (`<=`), 447 (`+=` to `-=`, `*=`); `route_map` 975 (`>` to `==`, `>=`), 978 (`-=` to `+=`, `/=`), 985 (`*=`); `api_impact` 1248 (`||` to `&&`; `==` to `!=` twice) | Not the G9 property (once per query). They are the over-size, budget-skip, site-cap and route-selection accounting. Not killed and not argued equivalent, except 377 (`remaining` only differs within 64 MB of the byte cap, unreachable with a test-sized fixture). Follow-up: a budget-accounting test, not folded into G9 |
| G24 | 6 in `traverse_walked`: `engine.rs:1443`, `1447`, `1448`, `1460` (two), `1561` (call-blind start guards) | Not confidence-ordered truncation. They decide when a start is disclosed as call-blind. No test in the G24 selection reaches them; not run against the other engine tests (NOT RUN) |
| G24 | `atomic_budget_take` 6 comparison mutants (`budget.rs:327`, `336`) | Swept in by the `budget_take` regex. It is the all-or-nothing scoped-trace packer, not the caller-list budget; not G24, left unrun against its own tests (NOT RUN) |
| X12 | `LspClient::spawn_with` `lsp.rs:1078` `delete !` and `:1079` `||` to `&&` (reader thread exit); `LspClient::request` `:1214` `Ok(Default)`, `:1219` `+=` to `-=`, `*=` | Reader-thread lifetime and request ids are not the write-side property. With all `devmap-resolve` lib tests a run still leaves `:1079` and `:1219 *=`: the first changes only when a finished reader thread exits (unobservable without a thread probe), the second repeats request id 1, which no test with two requests to one server checks. The other three are killed by the sibling lib tests. Not killed here |

Card ft-ae653b10 (`rust/devmap-store/src/db/maintenance.rs`): the nine mutants
this lane touched there (seven in the prune, two in admit) all survived the
B6/B7 test binary before the strengthening and none survive it now; with the
sibling suites (`retention_churn`, `validity_ranges`, `store_hardening`,
`adversarial_store`) all nine were already caught. The rest of the file was not
mutated (NOT RUN: whole-file sweeps are out of scope).

NOT RUN: `devmap-cli/tests/build_lsp_opt_in.rs` (the CLI LSP test) maps to none
of these IDs (X11 and X12 live in `devmap-resolve/src/lsp.rs`), so no mutants
were scoped to it. G15 and `handler_return_keys` were not touched.

Several "covered" rows are partial or inferred; the note in each row says so.
