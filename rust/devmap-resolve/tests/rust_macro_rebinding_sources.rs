//! What can rebind the name of a macro the resolver would otherwise take for a
//! std/log/tracing/anyhow one that emits nothing (round 6 of the Rust caller
//! audit). A name no scope can prove is `Unknown`, never an outer or default
//! origin, so each case below must leave `Dup::mk` without an edge to the module
//! level import, and each control must keep it:
//!
//! * a glob import (direct, through a prelude chain, from an unindexed crate);
//! * the manifest: an `extern crate .. as` alias, a Cargo `package =` rename, a
//!   workspace-inherited rename, and a crate no manifest governs;
//! * `cfg_attr(pred, ..)`, which stands for the attributes inside it, and a
//!   derive reached through a path;
//! * `#[macro_use]`, which on a module publishes only that module's macros.
//!
//! The crates are written to a scratch directory and read through the product's
//! own manifest pass, so crate membership and dependency names come from the
//! `Cargo.toml` files and not from a path guess.

use std::path::PathBuf;

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

const DA: &str = "pub struct Dup;\nimpl Dup { pub fn mk() -> u16 { 2 } }\n";
const DB: &str = "pub struct Dup;\nimpl Dup { pub fn mk() -> u8 { 1 } }\n";
const DEP_MACROS: &str = "#[macro_export]\nmacro_rules! bail { ($($t:tt)+) => { use $($t)+; }; }\n\
     #[macro_export]\nmacro_rules! trace { ($($t:tt)+) => { use $($t)+; }; }\n";

fn scratch(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-macro-sources-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

/// Resolve `files`, the manifests among them read by the manifest pass.
fn resolve_workspace(tag: &str, files: &[(String, String)]) -> ResolutionResult {
    let root = scratch(tag);
    for (path, contents) in files {
        let full = root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, contents).unwrap();
    }
    let manifests = devmap_extract::collect_project_manifests(&root).unwrap();
    let extractions: Vec<Extraction> = files
        .iter()
        .filter(|(path, _)| path.ends_with(".rs"))
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_project_manifests(&manifests);
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let _ = std::fs::remove_dir_all(&root);
    result
}

/// The same files with no manifest at all.
fn resolve_without_manifests(files: &[(String, String)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .filter(|(path, _)| path.ends_with(".rs"))
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn targets_from(result: &ResolutionResult, caller: &str) -> Vec<String> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == caller)
        .map(|edge| edge.target_symbol.clone())
        .collect()
}

fn binds_da(result: &ResolutionResult) -> bool {
    targets_from(result, "app/src/user.rs::run")
        .iter()
        .any(|target| target == "app/src/da.rs::Dup.mk")
}

struct Case<'a> {
    /// `[dependencies]` lines of the app crate.
    deps: &'a str,
    /// What the dependency crate `dep` holds.
    dep_lib: &'a str,
    /// Lines at the top of `app/src/lib.rs`.
    lib_header: &'a str,
    /// The body of `app/src/user.rs`, before `pub fn run`.
    user_head: &'a str,
    /// The first statements of `run`.
    run_body: &'a str,
    /// Extra files `(path, contents)`.
    extra: Vec<(&'a str, &'a str)>,
    /// `Cargo.toml` of the workspace root.
    root_cargo: &'a str,
}

impl<'a> Default for Case<'a> {
    fn default() -> Self {
        Case {
            deps: "dep = { path = \"../dep\" }\n",
            dep_lib: DEP_MACROS,
            lib_header: "",
            user_head: "",
            run_body: "",
            extra: Vec::new(),
            root_cargo: "[workspace]\nmembers = [\"app\", \"dep\"]\n",
        }
    }
}

impl Case<'_> {
    fn files(&self) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = vec![
            ("Cargo.toml".into(), self.root_cargo.into()),
            (
                "dep/Cargo.toml".into(),
                "[package]\nname = \"dep\"\nversion = \"0.1.0\"\n".into(),
            ),
            ("dep/src/lib.rs".into(), self.dep_lib.into()),
            (
                "app/Cargo.toml".into(),
                format!(
                    "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\n{}",
                    self.deps
                ),
            ),
            (
                "app/src/lib.rs".into(),
                format!(
                    "{}pub mod da;\npub mod db;\npub mod user;\n",
                    self.lib_header
                ),
            ),
            ("app/src/da.rs".into(), DA.into()),
            ("app/src/db.rs".into(), DB.into()),
            (
                "app/src/user.rs".into(),
                format!(
                    "use crate::da::Dup;\n{}\npub fn run() -> u8 {{\n    {}\n    Dup::mk()\n}}\n",
                    self.user_head, self.run_body
                ),
            ),
        ];
        files.extend(
            self.extra
                .iter()
                .map(|(path, contents)| (path.to_string(), contents.to_string())),
        );
        files
    }

    fn resolve(&self, tag: &str) -> ResolutionResult {
        resolve_workspace(tag, &self.files())
    }
}

// ---------------------------------------------------------------- F1: globs

#[test]
fn f1_a_glob_of_an_indexed_crate_that_exports_the_macro_makes_it_not_silent() {
    let result = Case {
        user_head: "use dep::*;",
        run_body: "bail!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f1a");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f1_a_glob_reached_through_a_prelude_chain_counts_the_same() {
    let result = Case {
        user_head: "use crate::prelude::*;",
        run_body: "bail!(crate::db::Dup);",
        lib_header: "pub mod prelude;\n",
        extra: vec![("app/src/prelude.rs", "pub use dep::*;\n")],
        ..Case::default()
    }
    .resolve("f1b");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f1_a_glob_of_a_crate_that_is_not_indexed_cannot_be_proven() {
    let result = Case {
        user_head: "use serde::*;",
        run_body: "println!(\"x\");",
        ..Case::default()
    }
    .resolve("f1c");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f1_a_glob_of_std_or_of_an_indexed_crate_without_the_macro_keeps_it_silent() {
    for (tag, head) in [("f1d", "use std::collections::*;"), ("f1e", "use dep::*;")] {
        // `dep` here exports `bail` and `trace` only; `println!` is not among them.
        let result = Case {
            user_head: head,
            run_body: "println!(\"x\");",
            ..Case::default()
        }
        .resolve(tag);
        assert!(
            binds_da(&result),
            "{head}: {:?}",
            targets_from(&result, "app/src/user.rs::run")
        );
    }
}

#[test]
fn f1_a_glob_of_a_block_reaches_the_call_but_one_in_another_module_does_not() {
    // The glob sits in a sibling module that the call's module does not import.
    let result = Case {
        run_body: "println!(\"x\");",
        lib_header: "pub mod other;\n",
        extra: vec![("app/src/other.rs", "use serde::*;\npub fn f() {}\n")],
        ..Case::default()
    }
    .resolve("f1f");
    assert!(
        binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

// ------------------------------------------------- F2: crate names, manifest

#[test]
fn f2_an_extern_crate_alias_is_honoured_for_a_macro_imported_through_it() {
    let result = Case {
        user_head: "extern crate dep as anyhow;\nuse anyhow::bail;",
        run_body: "bail!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f2a");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f2_a_cargo_package_rename_is_honoured_for_the_qualified_form() {
    let result = Case {
        deps: "log = { package = \"dep\", path = \"../dep\" }\n",
        run_body: "log::trace!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f2b");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f2_a_cargo_package_rename_is_honoured_for_the_bare_name_through_use() {
    let result = Case {
        deps: "log = { package = \"dep\", path = \"../dep\" }\n",
        user_head: "use log::trace;",
        run_body: "trace!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f2c");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f2_a_workspace_inherited_rename_is_followed() {
    let result = Case {
        deps: "log = { workspace = true }\n",
        root_cargo: "[workspace]\nmembers = [\"app\", \"dep\"]\n\n[workspace.dependencies]\n\
                     log = { package = \"dep\", path = \"dep\" }\n",
        run_body: "log::trace!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f2d");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f2_an_inherited_dependency_no_workspace_table_answers_is_unknown() {
    let result = Case {
        deps: "log = { workspace = true }\n",
        run_body: "log::trace!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f2e");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f2_the_real_log_crate_through_its_own_name_stays_silent() {
    let result = Case {
        deps: "log = \"0.4\"\n",
        user_head: "use log::trace;",
        run_body: "trace!(\"x\");",
        ..Case::default()
    }
    .resolve("f2f");
    assert!(
        binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f2_with_no_manifest_a_dependency_macro_is_not_silent_but_a_std_one_is() {
    let files = Case {
        user_head: "use log::info;",
        run_body: "info!(\"x\");",
        ..Case::default()
    }
    .files();
    let manifest_free: Vec<(String, String)> = files
        .into_iter()
        .filter(|(path, _)| path.ends_with(".rs"))
        .collect();
    let result = resolve_without_manifests(&manifest_free);
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );

    let std_files: Vec<(String, String)> = Case {
        run_body: "println!(\"x\");",
        ..Case::default()
    }
    .files()
    .into_iter()
    .filter(|(path, _)| path.ends_with(".rs"))
    .collect();
    let result = resolve_without_manifests(&std_files);
    assert!(
        binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

// ------------------------------------------------------- F3: cfg_attr, derive

#[test]
fn f3_cfg_attr_macro_use_is_macro_use() {
    let result = Case {
        dep_lib: "#[macro_export]\nmacro_rules! vec { ($($t:tt)+) => { use $($t)+; }; }\n",
        lib_header: "#[cfg_attr(all(), macro_use)]\nextern crate dep;\n",
        run_body: "vec!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f3a");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f3_an_attribute_macro_written_through_cfg_attr_leaves_the_block_unplaced() {
    let result = Case {
        run_body: "#[cfg_attr(all(), pm::with_use(crate::db::Dup))] fn _x() {}",
        ..Case::default()
    }
    .resolve("f3b");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f3_a_derive_with_a_path_is_std_only_when_the_path_is_std() {
    let unplaced = |tag: &str, item: &str| {
        let result = Case {
            run_body: item,
            ..Case::default()
        }
        .resolve(tag);
        !binds_da(&result)
    };
    assert!(
        unplaced("f3c", "#[derive(dep::Clone)] struct _S;"),
        "dep::Clone"
    );
    assert!(
        unplaced("f3d", "#[derive(Debug, crate::Clone)] struct _S;"),
        "crate::Clone"
    );
    assert!(
        !unplaced("f3e", "#[derive(Clone, Debug)] struct _S;"),
        "bare std derives"
    );
    assert!(
        !unplaced(
            "f3f",
            "#[derive(core::clone::Clone, ::std::fmt::Debug)] struct _S;"
        ),
        "std-rooted derives"
    );
    assert!(
        !unplaced("f3g", "#[cfg_attr(all(), derive(Debug))] struct _S;"),
        "an inert attribute inside cfg_attr"
    );
}

// ------------------------------------------------------------ F7: macro_use

#[test]
fn f7_macro_use_on_an_internal_module_does_not_disable_a_std_macro() {
    let result = Case {
        lib_header: "#[macro_use]\nmod macros;\n",
        run_body: "println!(\"x\");",
        extra: vec![(
            "app/src/macros.rs",
            "macro_rules! helper { () => { 1u8 }; }\n",
        )],
        ..Case::default()
    }
    .resolve("f7a");
    assert!(
        binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f7_macro_use_extern_crate_of_an_indexed_crate_without_the_macro_keeps_it_silent() {
    let result = Case {
        dep_lib: "#[macro_export]\nmacro_rules! helper { () => { 1u8 }; }\n",
        lib_header: "#[macro_use]\nextern crate dep;\n",
        run_body: "println!(\"x\");",
        ..Case::default()
    }
    .resolve("f7b");
    assert!(
        binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f7_macro_use_extern_crate_that_exports_the_macro_still_disables_it() {
    let result = Case {
        dep_lib: "#[macro_export]\nmacro_rules! println { ($($t:tt)+) => { use $($t)+; }; }\n",
        lib_header: "#[macro_use]\nextern crate dep;\n",
        run_body: "println!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f7c");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f7_macro_use_extern_crate_that_re_exports_the_macro_under_the_name_still_disables_it() {
    let result = Case {
        dep_lib: "#[macro_export]\nmacro_rules! __emit_use { ($($t:tt)+) => { use $($t)+; }; }\n\
                  pub use __emit_use as vec;\n",
        lib_header: "#![allow(dead_code)]\n#[macro_use]\nextern crate dep;\n",
        run_body: "vec!(crate::db::Dup);",
        ..Case::default()
    }
    .resolve("f7e");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn f7_macro_use_extern_crate_that_is_not_indexed_still_disables_it() {
    let result = Case {
        deps: "dep = { path = \"../dep\" }\nserde = \"1\"\n",
        lib_header: "#[macro_use]\nextern crate serde;\n",
        run_body: "println!(\"x\");",
        ..Case::default()
    }
    .resolve("f7d");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}
