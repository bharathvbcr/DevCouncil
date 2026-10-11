//! The one integration-test target of `devmap-query`.
//!
//! Each file under `tests/` used to be its own target, and every target linked
//! the whole dependency graph, including all the tree-sitter grammars, into a
//! binary of its own (about 70 MiB each). They are modules of this one target
//! instead, so the link is paid once. Test names are now prefixed with their
//! file's name (`cargo test -p devmap-query --test it -- <file>::`).
//!
//! * A file that needs `parse` is declared `#[cfg(feature = "parse")]` here;
//!   `every_test_target_declares_its_grammar_need` reads these lines.
//! * A file that must own its process (it changes the process environment, or
//!   counts the process's threads) stays a target of its own: it is listed in
//!   `ISOLATED` and has a `[[test]]` entry in Cargo.toml.
//! * `autotests = false`, so a new file under `tests/` is not silently built
//!   as a separate target or silently skipped: `every_test_file_is_a_module`
//!   fails until it is declared here or isolated.

#[cfg(feature = "parse")]
mod a_bare_ruby_send_has_a_caller;
#[cfg(feature = "parse")]
mod a_call_blind_target_is_unavailable_not_empty;
#[cfg(feature = "parse")]
mod a_kernel_is_reached_from_its_host_by_name;
mod a_nan_floor_is_refused;
mod a_query_only_build_can_call_a_store_current;
#[cfg(feature = "parse")]
mod a_rendered_component_has_a_caller;
#[cfg(feature = "parse")]
mod a_traversal_over_a_hole_says_so;
#[cfg(feature = "parse")]
mod a_walk_names_only_the_gaps_it_touches;
#[cfg(feature = "parse")]
mod a_wrapped_callback_is_a_function;
#[cfg(feature = "parse")]
mod affected_counts_inline_tests;
#[cfg(feature = "parse")]
mod ambiguous_impact_seeds;
#[cfg(feature = "parse")]
mod answers_agree_with_each_other;
mod artifacts_sidecar_adversarial;
#[cfg(feature = "parse")]
mod ask_evidence;
#[cfg(feature = "parse")]
mod ask_ranking;
#[cfg(feature = "parse")]
mod ask_scope;
#[cfg(feature = "parse")]
mod coupling_spans_inheritance;
#[cfg(feature = "parse")]
mod coverage_loss_artifacts;
#[cfg(feature = "parse")]
mod dead_clusters_reach_a_query;
mod documentation_visibility;
#[cfg(feature = "parse")]
mod dynamic_references_clear_unwired;
#[cfg(feature = "parse")]
mod edge_index_under_writes;
#[cfg(feature = "parse")]
mod every_emitted_field_says_whether_it_ran;
#[cfg(feature = "parse")]
mod explore_and_affected;
#[cfg(feature = "parse")]
mod file_liveness_characterization;
mod git_is_bounded_on_the_artifact_path;
#[cfg(feature = "parse")]
mod graph_intel_is_emitted;
mod guide_write_security;
mod host_artifact_provider;
#[cfg(feature = "parse")]
mod impact_layers_come_from_the_kernel;
#[cfg(feature = "parse")]
mod impact_names_its_unresolved_callers;
#[cfg(feature = "parse")]
mod incomplete_answers_say_so;
#[cfg(feature = "parse")]
mod literal_sites;
#[cfg(feature = "parse")]
mod manifest_truncation_counts;
#[cfg(feature = "parse")]
mod name_query_filters;
#[cfg(feature = "parse")]
mod neighbors_adversarial;
#[cfg(feature = "parse")]
mod neighbors_composition;
#[cfg(feature = "parse")]
mod paired_reads_are_one_generation;
#[cfg(feature = "parse")]
mod persisted_engine_answers;
#[cfg(feature = "parse")]
mod preview_containment;
#[cfg(feature = "parse")]
mod preview_unreadable_file;
#[cfg(feature = "parse")]
mod query_cancellation;
mod repository_git_config;
#[cfg(feature = "parse")]
mod retired_python_claims_have_kernel_tests;
#[cfg(feature = "parse")]
mod routes_under_hostile_input;
#[cfg(feature = "parse")]
mod rung_filter;
#[cfg(feature = "parse")]
mod rung_filter_under_stress;
#[cfg(feature = "parse")]
mod search_counts_under_concurrent_writes;
#[cfg(feature = "parse")]
mod search_hostile_queries;
#[cfg(feature = "parse")]
mod search_ranks_before_paging;
#[cfg(feature = "parse")]
mod search_zero_is_not_absence;
#[cfg(feature = "parse")]
mod secret_paths_are_withheld;
#[cfg(feature = "parse")]
mod skeleton_envelopes;
#[cfg(feature = "parse")]
mod snapshot_budget;
#[cfg(feature = "parse")]
mod source_read_security;
mod stat_memo_spawns_are_test_only;
#[cfg(feature = "parse")]
mod stored_edge_resolution_is_read_not_guessed;
#[cfg(feature = "parse")]
mod stored_edges_keep_the_confidence_their_evidence_earned;
mod subsystem_handoff_paths_are_computed;
#[cfg(feature = "parse")]
mod subsystem_neighbours_are_computed;
mod subsystem_roles_and_file_kinds_are_computed;
#[cfg(feature = "parse")]
mod the_core_graph_is_the_artifacts_nodes_and_edges;
mod the_freshness_inventory_counts_only_what_discovery_can_index;
mod the_repository_inventory_bounds_are_honest;
mod tooltip_security;
#[cfg(feature = "parse")]
mod unwired_excludes_prose_on_its_own_grounds;
#[cfg(feature = "parse")]
mod unwired_is_not_import_blind;
#[cfg(feature = "parse")]
mod unwired_never_names_a_non_code_file;
mod viz_projection;
mod workspace_registry_concurrency;
#[cfg(feature = "parse")]
mod workspace_search_totals;

/// Files that keep a process of their own, and why (see the file's header).
const ISOLATED: &[&str] = &[
    "artifact_writer_security",
    "a_cold_query_pays_for_its_answer_not_the_generation",
    "a_delete_this_list_reads_for_its_answer",
    "a_fan_out_pays_for_its_index_once_per_direction",
    "query_work_is_bounded_by_the_answer",
];

mod every_test_file_is_a_module {
    use std::collections::BTreeSet;

    #[test]
    fn every_test_file_is_a_module_or_isolated_and_declared() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
            .expect("tests/ is readable")
            .filter_map(|entry| {
                let path = entry.expect("a directory entry").path();
                (path.extension()? == "rs")
                    .then(|| path.file_stem().unwrap().to_string_lossy().into_owned())
            })
            .filter(|stem| stem != "it")
            .collect();
        let declared: BTreeSet<String> = include_str!("it.rs")
            .lines()
            .filter_map(|line| line.trim().strip_prefix("mod ")?.strip_suffix(';'))
            .map(str::to_string)
            .collect();
        let isolated: BTreeSet<String> = super::ISOLATED
            .iter()
            .map(|name| name.to_string())
            .collect();
        let manifest =
            std::fs::read_to_string(dir.join("../Cargo.toml")).expect("Cargo.toml is readable");
        let targets: BTreeSet<String> = manifest
            .split("[[test]]")
            .skip(1)
            .filter_map(|block| {
                let line = block.lines().find(|l| l.trim_start().starts_with("name"))?;
                Some(line.split('"').nth(1)?.to_string())
            })
            .filter(|name| name != "it")
            .collect();
        // A parse that found nothing would make this vacuous.
        assert!(
            declared.len() + isolated.len() >= 76,
            "found {} modules",
            declared.len()
        );
        let expected: BTreeSet<String> = declared.union(&isolated).cloned().collect();
        assert_eq!(
            on_disk, expected,
            "tests/*.rs and the modules declared in tests/it.rs disagree: add a `mod` line, or list the file in ISOLATED and give it a [[test]] entry"
        );
        assert_eq!(
            targets, isolated,
            "ISOLATED and the [[test]] entries in Cargo.toml disagree"
        );
    }
}
