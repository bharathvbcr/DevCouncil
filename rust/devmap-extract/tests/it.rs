//! The one integration-test target of `devmap-extract`.
//!
//! Each file under `tests/` used to be its own target, and every target linked
//! the whole dependency graph, including all the tree-sitter grammars, into a
//! binary of its own (about 70 MiB each). They are modules of this one target
//! instead, so the link is paid once. Test names are now prefixed with their
//! file's name (`cargo test -p devmap-extract --test it -- <file>::`).
//!
//! * A file that needs `parse` is declared `#[cfg(feature = "parse")]` here;
//!   `every_test_target_declares_its_grammar_need` reads these lines.
//! * A file that must own its process (it changes the process environment, or
//!   counts the process's threads) stays a target of its own: it is listed in
//!   `ISOLATED` and has a `[[test]]` entry in Cargo.toml.
//! * `autotests = false`, so a new file under `tests/` is not silently built
//!   as a separate target or silently skipped: `every_test_file_is_a_module`
//!   fails until it is declared here or isolated.

mod a_cache_verdict_answers_for_the_root_it_was_asked;
#[cfg(feature = "parse")]
mod a_call_is_one_site_hardening;
#[cfg(feature = "parse")]
mod a_call_is_one_site_not_two;
#[cfg(feature = "parse")]
mod a_declared_field_has_a_type;
#[cfg(feature = "parse")]
mod a_minified_bundle_is_not_parsed;
#[cfg(feature = "parse")]
mod a_receiver_is_not_its_operator;
mod a_subprocess_is_bounded_in_time_and_bytes;
#[cfg(feature = "parse")]
mod adversarial_corpus;
#[cfg(feature = "parse")]
mod allow_unwired_and_dynamic_imports;
mod an_unreadable_subtree_is_a_hole_not_a_dead_build;
#[cfg(feature = "parse")]
mod budget_is_a_real_bound;
#[cfg(feature = "parse")]
mod c_family;
mod cache_tag_security;
#[cfg(feature = "parse")]
mod capped_reads_say_so;
#[cfg(feature = "parse")]
mod cargo_target_roots_are_wired;
#[cfg(feature = "parse")]
mod cprobe;
#[cfg(feature = "parse")]
mod declarations;
mod discovery_stays_inside_the_root;
#[cfg(feature = "parse")]
mod embedded_scripts;
#[cfg(feature = "parse")]
mod extract_bindings;
#[cfg(feature = "parse")]
mod file_liveness_eligibility;
#[cfg(feature = "parse")]
mod grammar_matrix;
#[cfg(feature = "parse")]
mod heritage;
mod ignore_rule_tolerance;
#[cfg(feature = "parse")]
mod langcalls_erlang_pascal;
#[cfg(feature = "parse")]
mod langcalls_jvm_dotnet;
#[cfg(feature = "parse")]
mod langcalls_lua_r;
#[cfg(feature = "parse")]
mod langcalls_nix;
#[cfg(feature = "parse")]
mod langcalls_ruby_php;
#[cfg(feature = "parse")]
mod langcalls_shell_sql_solidity;
#[cfg(feature = "parse")]
mod langcalls_swift_scala;
#[cfg(feature = "parse")]
mod langimports_specifier_names_a_file;
#[cfg(feature = "parse")]
mod language_capabilities;
#[cfg(feature = "parse")]
mod liquid_script_scanner_limits;
#[cfg(feature = "parse")]
mod markup_and_style;
#[cfg(feature = "parse")]
mod markup_under_stress;
#[cfg(feature = "parse")]
mod metal_coverage;
#[cfg(feature = "parse")]
mod module_bindings_and_import_names;
#[cfg(feature = "parse")]
mod mutation_fuzz;
#[cfg(feature = "parse")]
mod nested_source_limits;
#[cfg(feature = "parse")]
mod notebook_completeness;
mod one_ignore_owner;
#[cfg(feature = "parse")]
mod powershell_stays_pattern_recovered;
#[cfg(feature = "parse")]
mod python_path_loads;
#[cfg(feature = "parse")]
mod python_path_loads_under_stress;
mod python_source_roots_collect;
#[cfg(feature = "parse")]
mod receiver_is_an_identity;
mod reliability_source_reads;
#[cfg(feature = "parse")]
mod rust_use_declarations;
mod safe_fs_controls;
#[cfg(feature = "parse")]
mod sql_check_constraint_parse;
#[cfg(feature = "parse")]
mod stress_hardening;
#[cfg(feature = "parse")]
mod test_phase2_hardening;
mod the_bounded_runner_under_hostile_children;
#[cfg(feature = "parse")]
mod ts_import_export_fabrication;
#[cfg(feature = "parse")]
mod wiring_port_under_stress;
mod workspaces_collect;

/// Files that keep a process of their own, and why (see the file's header).
const ISOLATED: &[&str] = &[
    "global_excludes_are_one_rule",
    "a_deadline_reaches_the_children_the_child_started",
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
            declared.len() + isolated.len() >= 57,
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
