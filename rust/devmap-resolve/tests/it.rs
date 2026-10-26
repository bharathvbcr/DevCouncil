//! The one integration-test target of `devmap-resolve`.
//!
//! Each file under `tests/` used to be its own target, and every target linked
//! the whole dependency graph, including all the tree-sitter grammars, into a
//! binary of its own (about 70 MiB each). They are modules of this one target
//! instead, so the link is paid once. Test names are now prefixed with their
//! file's name (`cargo test -p devmap-resolve --test it -- <file>::`).
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
mod a_field_types_its_receiver;
#[cfg(feature = "parse")]
mod a_prelude_type_receiver_is_not_a_value;
#[cfg(feature = "parse")]
mod a_receiver_call_never_binds_by_bare_name;
#[cfg(feature = "parse")]
mod a_receiver_member_nothing_declares;
#[cfg(feature = "parse")]
mod adversarial_resolve;
#[cfg(feature = "parse")]
mod ambiguity_budget;
#[cfg(feature = "parse")]
mod audit_regressions;
#[cfg(feature = "parse")]
mod captured_receiver_attribution;
#[cfg(feature = "parse")]
mod commonjs_require_bindings;
#[cfg(feature = "parse")]
mod dead_hardening_characterization;
#[cfg(feature = "parse")]
mod deref_transparent_receivers;
#[cfg(feature = "parse")]
mod embedded_script_families;
#[cfg(feature = "parse")]
mod every_site_is_counted_once;
#[cfg(feature = "parse")]
mod factory_return_types;
#[cfg(feature = "parse")]
mod function_values_and_platform_targets;
#[cfg(feature = "parse")]
mod glob_reexport_poison;
#[cfg(feature = "parse")]
mod global_rung_requires_no_other_evidence;
#[cfg(feature = "parse")]
mod go_own_package_receiver;
#[cfg(feature = "parse")]
mod go_package_scope;
#[cfg(feature = "parse")]
mod go_qualifier_scope;
#[cfg(feature = "parse")]
mod go_unexported_selector;
#[cfg(feature = "parse")]
mod host_global_objects;
#[cfg(feature = "parse")]
mod implicit_receiver_dispatch;
#[cfg(feature = "parse")]
mod implicit_receiver_reference;
#[cfg(feature = "parse")]
mod import_paths_resolve_to_real_files;
#[cfg(feature = "parse")]
mod imports_stay_in_the_named_tree;
#[cfg(feature = "parse")]
mod kotlin_receivers_and_packages;
#[cfg(feature = "parse")]
mod kotlin_written_types;
#[cfg(feature = "parse")]
mod language_server_edges;
#[cfg(feature = "parse")]
mod markup_selectors_resolve;
#[cfg(feature = "parse")]
mod module_singleton_receivers;
#[cfg(feature = "parse")]
mod non_swift_edge_compare;
#[cfg(feature = "parse")]
mod overload_self_edges;
#[cfg(feature = "parse")]
mod parallel_determinism;
#[cfg(feature = "parse")]
mod primary_languages;
#[cfg(feature = "parse")]
mod py_receiver_residue_attack;
#[cfg(feature = "parse")]
mod py_receiver_residue_attack2;
#[cfg(feature = "parse")]
mod py_relative_imports_attack;
#[cfg(feature = "parse")]
mod python_class_identity;
#[cfg(feature = "parse")]
mod python_edge_dump_attack4;
#[cfg(feature = "parse")]
mod python_edge_dump_attack5;
#[cfg(feature = "parse")]
mod python_field_super_attack;
#[cfg(feature = "parse")]
mod python_field_super_attack2;
#[cfg(feature = "parse")]
mod python_field_super_attack3;
#[cfg(feature = "parse")]
mod python_field_super_attack4;
#[cfg(feature = "parse")]
mod python_field_super_attack5;
#[cfg(feature = "parse")]
mod python_heritage_residue;
#[cfg(feature = "parse")]
mod python_import_module_literal;
#[cfg(feature = "parse")]
mod python_module_constants;
#[cfg(feature = "parse")]
mod python_modules_loaded_by_path;
#[cfg(feature = "parse")]
mod python_path_constants_and_loaders;
#[cfg(feature = "parse")]
mod python_receiver_residue;
#[cfg(feature = "parse")]
mod python_residue_attack;
#[cfg(feature = "parse")]
mod python_residue_attack2;
#[cfg(feature = "parse")]
mod python_sys_path_imports;
#[cfg(feature = "parse")]
mod qualified_module_paths;
#[cfg(feature = "parse")]
mod qualifier_scope_attack;
#[cfg(feature = "parse")]
mod qualifier_scope_other_families;
#[cfg(feature = "parse")]
mod real_corpus_resolution;
#[cfg(feature = "parse")]
mod reexport_chains;
#[cfg(feature = "parse")]
mod reexported_module_surfaces;
#[cfg(feature = "parse")]
mod resolution_honesty;
mod resolution_kind_is_one_owner;
#[cfg(feature = "parse")]
mod resolution_walks_terminate;
#[cfg(feature = "parse")]
mod ruby_attributes_are_methods;
#[cfg(feature = "parse")]
mod rust_accuracy_core;
#[cfg(feature = "parse")]
mod rust_module_paths;
#[cfg(feature = "parse")]
mod rust_qualified_path_calls;
#[cfg(feature = "parse")]
mod rust_qualified_super_calls;
#[cfg(feature = "parse")]
mod rust_self_type;
#[cfg(feature = "parse")]
mod rust_trait_dispatch;
#[cfg(feature = "parse")]
mod same_file_type_position;
#[cfg(feature = "parse")]
mod self_in_a_type_is_that_type;
#[cfg(feature = "parse")]
mod semantic_scope_and_identity;
#[cfg(feature = "parse")]
mod shell_source_resolves;
#[cfg(feature = "parse")]
mod swift_close_attack;
#[cfg(feature = "parse")]
mod swift_modules;
#[cfg(feature = "parse")]
mod swift_properties_attack;
#[cfg(feature = "parse")]
mod swift_property_reads;
#[cfg(feature = "parse")]
mod swift_protocol_dispatch;
#[cfg(feature = "parse")]
mod swift_resolver_residue;
#[cfg(feature = "parse")]
mod swift_resolver_residue_attack;
#[cfg(feature = "parse")]
mod swift_resolver_residue_attack2;
#[cfg(feature = "parse")]
mod test_phase3_hardening;
#[cfg(feature = "parse")]
mod type_aliases_are_not_call_targets;
#[cfg(feature = "parse")]
mod type_position_classification;
#[cfg(feature = "parse")]
mod typed_receivers;
#[cfg(feature = "parse")]
mod typescript_core_shapes;
#[cfg(feature = "parse")]
mod typescript_interface_dispatch;
#[cfg(feature = "parse")]
mod typescript_object_spread;
#[cfg(feature = "parse")]
mod typescript_object_spread_attack;
#[cfg(feature = "parse")]
mod unresolved_member;

/// Files that keep a process of their own, and why (see the file's header).
const ISOLATED: &[&str] = &[];

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
            declared.len() + isolated.len() >= 50,
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
#[cfg(feature = "parse")]
mod attack_resolver_bundle;
#[cfg(feature = "parse")]
mod attack_resolver_bundle2;
#[cfg(feature = "parse")]
mod attack_resolver_bundle3;
#[cfg(feature = "parse")]
mod attack_resolver_bundle4;
#[cfg(feature = "parse")]
mod attack_resolver_bundle5;
#[cfg(feature = "parse")]
mod corpus_dump_attack;
