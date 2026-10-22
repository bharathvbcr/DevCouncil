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
| B6 | PLAN | Generation prune is a bounded delete | uncovered | Correctness of prune is pinned (`devmap-cli/tests/test_generation_retention.rs::pruned_generations_leave_no_rows_behind_in_any_table`); "cost independent of live row count" is not measured anywhere |
| B7 | PLAN | Cache admission is O(1) | uncovered | No cost test; `extract_cache.rs::identical_content_never_aliases_file_identity_through_cache` pins identity only |
| B8 | PLAN | Eviction follows liveness, not age | covered | `devmap-store/tests/store_hardening.rs::s4_prune_evicts_a_cache_row_no_retained_generation_can_ever_serve`; `devmap-cli/tests/test_generation_retention.rs::superseded_extraction_cache_entries_are_evicted` |
| B9 | PLAN | No redundant column in any primary key | uncovered | Acceptance was "schema review"; no test inspects primary keys |
| B10 | PLAN | Paths interned | covered | `devmap-store/tests/retention_churn.rs::repeated_renames_bound_paths_to_the_retained_generations`; `devmap-store/tests/adversarial_store_without_grammars.rs::audit_concurrent_path_interns_reuse_one_identity`. The "index under 60 MB" size gate is not tested |
| B11 | PLAN | Free space reclaimed | covered | `devmap-store/tests/kernel_defects.rs::k2_incremental_vacuum_frees_every_page_it_was_asked_for`; `devmap-cli/tests/kernel_defects.rs::k2_a_no_change_build_reclaims_a_large_freelist_from_the_main_file`; `devmap-store/tests/store_hardening.rs::new_stores_use_incremental_auto_vacuum_and_reclaim_without_a_full_rewrite` |
| B12 | PLAN | Extraction saturates cores | uncovered | Throughput property; no scaling test |
| B13 | PLAN | No repeated Python `Path` parsing in hot loops | retire | Python-only (1.09M `Path` calls); engine deleted, DIVERGENCES header |
| B14 | PLAN | One tree pass per file instead of regex sweeps (`routes.py`, `di.py`) | retire | Python regex sweeps; the kernel extracts with tree-sitter in one pass (AUDIT section 9.1) |
| B15 | PLAN | Language resolved once per file | retire | Python `resolve.py`; the kernel's language table is one compile-time authority (AUDIT section 9.7), and `devmap-cli/tests/test_findings_suite.rs::test_language_extractor_routing_all_specs` pins routing |
| N1 | PLAN | Queries never deserialise the whole graph | covered | `devmap-cli/tests/test_findings_suite.rs::test_explore_p95_soft_ratchet_2k` (soft ratchet at 2k files, not the 10k in the acceptance text) |
| N2 | PLAN | Deletions and renames are explicit | covered | `devmap-cli/tests/test_persisted_cli.rs::staleness_audit_add_rename_delete_and_restore_are_not_silent`; `devmap-serve/tests/daemon_drain_races.rs::ra3_deleted_sources_are_stale_even_without_a_watcher` |
| N3 | PLAN | No dual-write of a second full serialisation | covered | `devmap-cli/tests/test_code_graph_artifact.rs::the_interned_graph_is_opt_in_and_decodes_to_the_verbose_one` (the second encoding is opt-in; the store is canonical). Inferred, not a test of "one store" as such |
| N6 | PLAN | Receiver types tracked from constructors | covered | `devmap-resolve/tests/test_phase3_hardening.rs::constructor_assignment_drives_receiver_resolution_not_method_name`; `::ambiguous_constructor_types_do_not_seed_receiver_resolution` |
| N7 | PLAN | Retrieval fuses lexical and semantic ranking | retire | DIVERGENCES row N7: retired with the semantic layer; `test_findings_suite.rs::lexical_search_operates_while_rrf_remains_open` (name predates the retirement) |
| N8 | PLAN | Ingestion owned by a long-lived watcher | covered | `devmap-serve/src/daemon.rs::test_b5_s17_connect_time_sweep_enqueues_changed_new_and_deleted_files`; `devmap-serve/src/watcher.rs::test_s12_ignore_cache_reuses_rules_and_invalidates_on_rule_change` |
| N9 | PLAN | Pools sized from real cores and RAM | uncovered | Only `dc-grep` queries `available_parallelism`; no test pins extraction pool sizing |
| N10 | PLAN | Query surface honest about capability | covered | `devmap-query/src/cypher.rs::an_or_is_refused_because_this_engine_only_conjoins`; `::a_query_this_subset_cannot_read_is_refused_with_the_shape_it_wants`; `::a_pattern_this_subset_cannot_read_is_refused_not_answered_as_every_node` |
| D2 | PLAN | One schema, no shadow tables | new | `devmap-store/tests/one_schema_no_shadow_tables.rs::d2_a_fresh_store_and_its_reopen_carry_no_shadow_table` (+ `d2_the_shadow_detector_is_not_vacuous`) |
| D3 | PLAN | Lock status reports the true holder | covered | `devmap-store/tests/kernel_defects.rs::k13_writer_lock_is_exclusive_and_names_the_holder_on_timeout`; `devmap-store/src/db/tests/bounded_claim_tests.rs::s6_writer_lock_contention_still_polls_to_the_deadline_and_names_the_holder` |
| K8 | PLAN | Phase profiler attributes cost to the phase that incurred it | covered | `devmap-cli/tests/cli_json_contract.rs::the_unchanged_build_reports_its_timings`; `devmap-cli/tests/test_progress.rs::an_unchanged_build_reports_its_own_timings`. Inferred: they pin stage presence; the attribution regression is described in PLAN as closed with a test |
| G1 | PLAN (breadth beyond grammars) | Grammarless languages still recovered | covered | `devmap-extract/src/fallback.rs::recovers_protobuf_messages_services_and_rpcs`; `::recovers_powershell_and_visual_basic_declarations` |
| G1 | AUDIT (CFG wiring) | Branch/loop/try bodies wired into the CFG | covered | `devmap-analyze/src/pdg.rs::cfg_threads_branch_loop_try_and_terminators` |
| G2 | PLAN (lexical + semantic ranking) | An exact term ranks first | covered | `devmap-query/src/semantic.rs::ranks_a_name_the_query_does_not_literally_contain`; `::a_term_common_to_every_document_still_ranks` |
| G2 | AUDIT (leader lines) | A branch condition's use is visible to def-use | new | `devmap-analyze/src/pdg.rs::g2_a_branch_condition_use_is_a_data_edge_from_its_definition` |
| G9 | AUDIT | api_impact scans fetch sites once per query | uncovered | The scan is bounded and says when it stopped (`devmap-cli/tests/api_views_are_honest.rs::a_scan_that_stopped_early_cannot_report_a_route_as_uncalled`), but nothing counts scans per query |
| G10 | AUDIT | PDG query does not reload shards per path; store errors not swallowed | covered | `devmap-cli/tests/test_findings_suite.rs::extraction_cache_round_trip` (weak: DIVERGENCES G10 says it does not time a PDG batch) |
| G11 | AUDIT | Query shim does not advertise relationships that cannot match | covered | `devmap-query/src/cypher.rs::a_relationship_the_graph_cannot_emit_is_named_not_answered_empty` |
| G13 | AUDIT | Reaching definitions keep loop-carried dependencies | covered | `devmap-analyze/src/pdg.rs::reaching_definitions_keep_loop_carried_edges_and_exact_taint` |
| G15 | AUDIT | Handler return-key extraction is not tautological; line-0 handlers do not absorb every function's keys | uncovered | `devmap-query/src/api_routes.rs::handler_return_keys` has no direct test; `shape_check_separates_agreement_from_having_nothing_to_compare` covers the verdict only |
| G17 | AUDIT | `./` stripped as a prefix, not a character set, so dot-directories keep their dot | new | `devmap-query/src/scope.rs::g17_a_dot_directory_keeps_its_leading_dot` |
| G22 | AUDIT | OKF export neighbour-area scan | retire | DIVERGENCES EXPORT-4: OKF half "Not ported"; the kernel exports GraphML only |
| G23 | AUDIT | Parameters are taint sources | covered | `devmap-analyze/src/pdg.rs::reaching_definitions_keep_loop_carried_edges_and_exact_taint` (taint flows from a `params` entry to the sink); `::a_try_body_definition_reaches_a_handler_use` |
| G24 | AUDIT | Confidence-aware budget priority | uncovered | Dead candidates rank by confidence before truncation (`devmap-query/tests/coverage_loss_artifacts.rs::dead_symbol_candidates_rank_by_confidence_before_truncating`); caller-list truncation order is not tested |
| G25 | AUDIT | `build_file_and_symbol_nodes` builds an unused `name_index` | retire | Python function; `rg -uu name_index rust` finds nothing |
| S13 | AUDIT | Migration ladder not run on every `put_extraction` | covered | `devmap-store/tests/migration_ladder.rs::a_migrated_store_reopens_without_migrating_again` (migrations run at open) |
| S15 | AUDIT | Windows lease size-probe and guard byte | retire | Python lease file; the daemon is the sole writer (AUDIT section 9.4: lease heuristics "are deleted, not ported") |
| S16 | AUDIT | Rename aliases survive across generations | uncovered | `devmap-store/src/schema.rs` keeps two generations "for rename-alias chaining" but no test chains a rename across them; `retention_churn.rs::repeated_renames_bound_paths_to_the_retained_generations` covers path bounds only |
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
| X12 | AUDIT | LSP client cannot deadlock on full pipes | uncovered | Mid-body stall is pinned (`lsp.rs::a_server_that_stalls_mid_body_times_out_at_the_deadline`); a server that stops reading while the client writes is not |
| X13 | AUDIT | LSP positions in UTF-16 units | covered | `devmap-resolve/src/lsp.rs::the_line_index_matches_a_prefix_scan` (UTF-16 line index; `positionEncodings` negotiation not tested) |
| X15 | AUDIT | Embedding model name is part of the cache key | retire | Embeddings gone; `devmap-query/src/semantic.rs` is stateless TF-IDF computed per query (PLAN G2) |
| X17 | AUDIT | Import extraction sees Go block imports and Rust `use` | covered | `devmap-extract/tests/test_phase2_hardening.rs::test_x9_rust_grouped_use`; `devmap-extract/tests/rust_use_declarations.rs::an_alias_is_the_local_name_and_the_original_is_the_imported_one`. Go block imports not located by name |
| X19 | AUDIT | MCP handlers do not block the event loop | covered | `devmap-serve/src/mcp.rs::audit_an_internal_timeout_returns_a_response_instead_of_silence` (bounded work; blocking-free dispatch not separately tested) |

## Counts

58 IDs, 60 rows (G1 and G2 appear once per namespace).

| Status | Rows |
|---|---|
| covered | 33 |
| new | 3 (D2, G2 AUDIT, G17) |
| retire | 14 |
| uncovered | 10 (B6, B7, B9, B12, N9, G9, G15, G24, S16, X12) |

Several "covered" rows are partial or inferred; the note in each row says so.
