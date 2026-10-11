//! The one integration-test target of `devmap-cli`.
//!
//! Each file under `tests/` used to be its own target, and every target linked
//! the whole dependency graph, including all the tree-sitter grammars, into a
//! binary of its own (about 70 MiB each). They are modules of this one target
//! instead, so the link is paid once. Test names are now prefixed with their
//! file's name (`cargo test -p devmap-cli --test it -- <file>::`).
//!
//! * A file that needs `parse` is declared `#[cfg(feature = "parse")]` here;
//!   `every_test_target_declares_its_grammar_need` reads these lines.
//! * A file that must own its process (it changes the process environment, or
//!   counts the process's threads) stays a target of its own: it is listed in
//!   `ISOLATED` and has a `[[test]]` entry in Cargo.toml.
//! * `autotests = false`, so a new file under `tests/` is not silently built
//!   as a separate target or silently skipped: `every_test_file_is_a_module`
//!   fails until it is declared here or isolated.

mod a_message_literal_has_no_run_of_spaces;
mod a_named_root_must_be_a_directory;
mod a_one_file_edit_writes_one_file_of_rows;
mod a_python_index_is_named_not_migrated;
mod a_read_arm_does_not_run_git_log;
mod a_refusal_is_printed_once;
mod a_root_anchor_replaces_a_baked_store_path;
mod an_ignore_file_binds_the_daemon;
mod api_views_are_honest;
mod ast_reports_its_own_totals;
mod binary_digests_are_memoised;
mod blast_names_what_a_change_reaches;
mod build_lsp_opt_in;
mod caller_assets_over_stdin;
mod claude_integration;
mod cli_json_contract;
mod closed_pipe_is_not_a_panic;
mod coverage_invariants;
mod crash_consistency;
mod daemon_storm_soak;
mod dead_tier_eligibility;
mod discovery_refusal_is_coverage_loss;
mod doctor_gaps;
mod doctor_reports_edge_confidence;
mod every_command_is_in_the_presentation_stress;
mod every_test_target_declares_its_grammar_need;
mod export_write_security;
mod failure_class_gates;
mod file_liveness_reaches_every_artifact;
mod head_stamp_skip;
mod hook_conformance;
mod hotspots_are_churn_times_coupling;
mod incremental_equivalence;
mod kernel_defects;
mod license_declarations_agree;
mod manifest_is_written_once;
mod mcp_over_real_stdio;
mod min_rung_flag;
mod outside_index_and_leaf_answers;
mod over_limit_daemon_agrees_with_build;
mod over_limit_files_are_listed_not_lost;
mod partial_parses_are_a_coverage_bucket;
mod passive_diagnostics;
mod paths_answers_about_one_repository;
mod paths_answers_without_opening_a_store;
mod phase1_status_doctor_mcp;
mod redirected_stdout_has_no_terminal_control_bytes;
mod repo_scope_hardening;
mod secret_paths_stay_out_of_answers_and_artifacts;
mod small_native_stack;
mod standalone_namespace;
mod state_discovery_contract;
mod status_names_what_it_could_not_read;
mod suspects_names_the_commit;
mod test_code_graph_artifact;
mod test_concurrency;
mod test_extraction;
mod test_fanout_metric;
mod test_findings_suite;
mod test_generation_retention;
mod test_hardening;
mod test_persisted_cli;
mod test_preview;
mod test_process_recovery;
mod test_progress;
mod test_query;
mod test_scholarlm_findings;
mod test_socket_path;
mod test_store;
mod test_stress;
mod test_workspace;
mod the_churn_window_is_an_artifact_input;
mod the_map_reads_the_repository_inventory;
mod the_workspace_registry_stays_with_the_repository;
mod tsconfig_path_mapping;
mod unchanged_skip;
mod worktree_stress_support;

/// Files that keep a process of their own, and why (see the file's header).
const ISOLATED: &[&str] = &["global_excludes_bind_the_daemon", "test_time_scale"];

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
            declared.len() + isolated.len() >= 78,
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
