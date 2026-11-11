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
    // `bail` is in no prelude, so a glob can bring in another one. (With
    // `println!` this case is a compile error: rustc reports E0659, a glob import
    // against the std prelude, so a crate that builds has no such glob. See
    // `r6a_*` below.)
    let result = Case {
        user_head: "use serde::*;",
        run_body: "bail!(crate::db::Dup);",
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

// ------------------------------------------------------------- round 7

#[test]
fn r3_a_pathed_call_ignores_a_glob_but_a_bare_call_does_not() {
    let pathed = Case {
        deps: "dep = { path = \"../dep\" }\nlog = \"0.4\"\n",
        user_head: "use serde::*;",
        run_body: "log::info!(\"x\");",
        ..Case::default()
    }
    .resolve("r3a");
    assert!(
        binds_da(&pathed),
        "{:?}",
        targets_from(&pathed, "app/src/user.rs::run")
    );
    let bare = Case {
        deps: "dep = { path = \"../dep\" }\nlog = \"0.4\"\n",
        user_head: "use serde::*;",
        run_body: "bail!(\"x\");",
        ..Case::default()
    }
    .resolve("r3b");
    assert!(
        !binds_da(&bare),
        "{:?}",
        targets_from(&bare, "app/src/user.rs::run")
    );
}

#[test]
fn r4_macro_use_on_a_module_whose_file_is_not_indexed_disables_silence() {
    for (tag, header) in [
        ("r4a", "#[macro_use]\nmod gen;\n"),
        (
            "r4b",
            "#[macro_use]\n#[path = \"gen/macros.rs\"]\nmod gen;\n",
        ),
    ] {
        let result = Case {
            lib_header: header,
            run_body: "println!(\"x\");",
            ..Case::default()
        }
        .resolve(tag);
        assert!(
            !binds_da(&result),
            "{header}: {:?}",
            targets_from(&result, "app/src/user.rs::run")
        );
    }
    // Control: the same declaration with the file indexed is understood.
    let result = Case {
        lib_header: "#[macro_use]\nmod gen;\n",
        run_body: "println!(\"x\");",
        extra: vec![("app/src/gen.rs", "macro_rules! helper { () => { 1u8 }; }\n")],
        ..Case::default()
    }
    .resolve("r4c");
    assert!(
        binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn r5_two_manifests_claiming_one_package_name_abstain() {
    let result = Case {
        dep_lib: "#[macro_export]\nmacro_rules! helper { () => { 1u8 }; }\n",
        lib_header: "#[macro_use]\nextern crate dep;\n",
        root_cargo: "[workspace]\nmembers = [\"app\", \"dep\", \"other/dep\"]\n",
        run_body: "println!(crate::db::Dup);",
        extra: vec![
            (
                "other/dep/Cargo.toml",
                "[package]\nname = \"dep\"\nversion = \"0.1.0\"\n",
            ),
            (
                "other/dep/src/lib.rs",
                "#[macro_export]\nmacro_rules! println { ($($t:tt)+) => { use $($t)+; }; }\n",
            ),
        ],
        ..Case::default()
    }
    .resolve("r5");
    assert!(
        !binds_da(&result),
        "{:?}",
        targets_from(&result, "app/src/user.rs::run")
    );
}

#[test]
fn r6c_a_crate_exporting_the_name_as_a_proc_macro_or_a_decl_macro_is_not_silent() {
    for (tag, dep_lib) in [
        (
            "r6c1",
            "#[proc_macro]\npub fn bail(input: TokenStream) -> TokenStream { input }\n",
        ),
        ("r6c2", "pub macro bail($($t:tt)+) { use $($t)+; }\n"),
    ] {
        let result = Case {
            dep_lib,
            user_head: "use dep::*;",
            run_body: "bail!(crate::db::Dup);",
            ..Case::default()
        }
        .resolve(tag);
        assert!(
            !binds_da(&result),
            "{dep_lib}: {:?}",
            targets_from(&result, "app/src/user.rs::run")
        );
    }
}

#[test]
fn r7_a_bare_std_derive_rebound_by_a_use_of_another_crate_is_not_inert() {
    let derive = |tag: &str, import: &str| {
        let result = Case {
            user_head: import,
            run_body: "#[derive(Clone)] struct _S;",
            ..Case::default()
        }
        .resolve(tag);
        binds_da(&result)
    };
    assert!(!derive("r7a", "use dep::Clone;"), "a derive of dep");
    assert!(
        !derive("r7b", "use dep::{Debug, Clone as C};"),
        "a braced import of dep"
    );
    assert!(
        derive("r7c", "use std::clone::Clone;"),
        "the std derive imported"
    );
    assert!(derive("r7d", ""), "no import at all");
}

#[test]
fn r6a_a_glob_cannot_rebind_a_std_prelude_macro_but_can_rebind_another() {
    // rustc 1.99.1: `use dep::*;` with `dep` exporting `vec` or `println` and a
    // use of `vec!`/`println!` is "error[E0659]: `vec` is ambiguous ... conflict
    // between a name from a glob import and an outer scope".
    let std_name = Case {
        user_head: "use serde::*;",
        run_body: "println!(\"x\");",
        ..Case::default()
    }
    .resolve("r6a1");
    assert!(
        binds_da(&std_name),
        "{:?}",
        targets_from(&std_name, "app/src/user.rs::run")
    );
    let other = Case {
        user_head: "use serde::*;",
        run_body: "bail!(\"x\");",
        ..Case::default()
    }
    .resolve("r6a2");
    assert!(
        !binds_da(&other),
        "{:?}",
        targets_from(&other, "app/src/user.rs::run")
    );
}

#[test]
fn r6b_a_name_imported_explicitly_in_the_chain_is_not_rebound_by_a_glob() {
    // rustc 1.99.1: `use anyhow::bail; use dep::*;` compiles and the glob is
    // reported unused (the explicit import wins whatever the order); a glob in an
    // inner scope against an outer explicit import is E0659 (probe `pb3`), so
    // that shape does not compile and is not a case here (an unproven block glob
    // would leave `Dup` itself unplaced anyway).
    for (tag, head, body) in [
        ("r6b1", "use anyhow::bail;\nuse serde::*;", "bail!(\"x\");"),
        ("r6b2", "use serde::*;\nuse anyhow::bail;", "bail!(\"x\");"),
    ] {
        let result = Case {
            deps: "dep = { path = \"../dep\" }\nanyhow = \"1\"\n",
            user_head: head,
            run_body: body,
            ..Case::default()
        }
        .resolve(tag);
        assert!(
            binds_da(&result),
            "{tag}: {:?}",
            targets_from(&result, "app/src/user.rs::run")
        );
    }
}

// ------------------------------------------------------------- round 8

fn binds_da_from(result: &ResolutionResult, caller: &str) -> bool {
    targets_from(result, caller)
        .iter()
        .any(|target| target == "app/src/da.rs::Dup.mk")
}

/// A dependency that exports each of `names` as a macro that emits a `use`.
fn dep_exporting(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| {
            format!(
                "#[macro_export]\nmacro_rules! {name} {{ ($($t:tt)+) => {{ use $($t)+; }}; }}\n"
            )
        })
        .collect()
}

#[test]
fn s1_an_explicit_import_under_cfg_is_not_an_import_when_the_cfg_is_off() {
    // rustc 1.99.1: with the feature off, `use anyhow::bail;` is not there, so
    // `bail!` is the glob's and emits `use crate::db::Dup;`. The import counts only
    // when its cfg also encloses the call (then the call exists only if the import
    // does).
    let user = |head: &str, body: &str| {
        format!("use crate::da::Dup;\n{head}\npub fn run() -> u8 {{\n    {body}\n    Dup::mk()\n}}\n")
    };
    let cases: [(&str, String, bool); 3] = [
        (
            "s1a",
            user(
                "use dep::*;\n#[cfg(feature = \"anyhow\")]\nuse anyhow::bail;",
                "bail!(crate::db::Dup);",
            ),
            false,
        ),
        (
            "s1b",
            user(
                "use dep::*;\nuse anyhow::bail;",
                "bail!(crate::db::Dup);",
            ),
            true,
        ),
        (
            "s1c",
            // The import and the call sit in one cfg'd function: the call is
            // there only when the import is.
            "use crate::da::Dup;\nuse dep::*;\n#[cfg(feature = \"anyhow\")]\npub fn run() -> u8 {\n    use anyhow::bail;\n    bail!(crate::db::Dup);\n    Dup::mk()\n}\n"
                .to_string(),
            true,
        ),
    ];
    for (tag, source, expect_edge) in cases {
        let mut files = Case {
            deps: "dep = { path = \"../dep\" }\nanyhow = \"1\"\n",
            lib_header: "pub mod other;\n",
            ..Case::default()
        }
        .files();
        files.push(("app/src/other.rs".into(), source));
        let result = resolve_workspace(tag, &files);
        assert_eq!(
            binds_da_from(&result, "app/src/other.rs::run"),
            expect_edge,
            "{tag}: {:?}",
            targets_from(&result, "app/src/other.rs::run")
        );
    }
}

#[test]
fn s2_without_the_implicit_prelude_a_glob_can_bring_a_std_named_macro() {
    // rustc 1.99.1, `use dep::*;` with `dep` exporting the name:
    //   #![no_std]: print/println/eprint/eprintln/dbg/thread_local/vec/format
    //     compile and the glob's macro is the one called; assert/assert_eq/
    //     assert_ne/debug_assert*/panic/matches/write/writeln/todo/unreachable/
    //     unimplemented/format_args are E0659 ("`assert` is ambiguous").
    //     `#[macro_use] extern crate alloc;` brings vec/format back (E0659),
    //     `#[macro_use] extern crate std;` all of them.
    //   #![no_implicit_prelude]: only assert/panic/unreachable/format_args are
    //     E0659; every other std macro name compiles with the glob's winning.
    // (record: rust/testdata/accuracy/rust/compile_fail/check.sh)
    let edge = |tag: &str, header: &str, name: &str| {
        let result = Case {
            dep_lib: Box::leak(dep_exporting(&[name]).into_boxed_str()),
            lib_header: Box::leak(header.to_string().into_boxed_str()),
            user_head: "use dep::*;",
            run_body: Box::leak(format!("{name}!(crate::db::Dup);").into_boxed_str()),
            ..Case::default()
        }
        .resolve(tag);
        binds_da(&result)
    };
    let no_std = "#![no_std]\n";
    for name in [
        "print", "println", "eprint", "eprintln", "dbg", "thread_local", "vec", "format",
    ] {
        assert!(
            !edge(&format!("s2a-{name}"), no_std, name),
            "no_std: a glob can bring `{name}!`"
        );
    }
    for name in [
        "assert", "assert_eq", "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne",
        "panic", "matches", "write", "writeln", "todo", "unreachable", "unimplemented",
        "format_args",
    ] {
        assert!(
            edge(&format!("s2b-{name}"), no_std, name),
            "no_std: `{name}!` is a core prelude macro"
        );
    }
    let alloc = "#![no_std]\n#[macro_use]\nextern crate alloc;\n";
    assert!(edge("s2c-vec", alloc, "vec"), "macro_use alloc brings vec");
    assert!(
        !edge("s2c-println", alloc, "println"),
        "macro_use alloc does not bring println"
    );
    let std_extern = "#![no_std]\n#[macro_use]\nextern crate std;\n";
    assert!(edge("s2d-println", std_extern, "println"));
    let no_prelude = "#![no_implicit_prelude]\n";
    for name in ["println", "vec", "matches", "write", "todo", "dbg"] {
        assert!(
            !edge(&format!("s2e-{name}"), no_prelude, name),
            "no_implicit_prelude: a glob can bring `{name}!`"
        );
    }
    for name in ["assert", "panic", "unreachable", "format_args"] {
        assert!(
            edge(&format!("s2f-{name}"), no_prelude, name),
            "no_implicit_prelude: `{name}!` is a builtin"
        );
    }
    // Control: with the implicit prelude, nothing changes.
    assert!(edge("s2g", "", "println"));
    // A conditional `no_std` may be on, so it counts as on.
    assert!(!edge(
        "s2h",
        "#![cfg_attr(not(feature = \"std\"), no_std)]\n",
        "println"
    ));
}

#[test]
fn s2_a_pathed_root_in_a_no_implicit_prelude_crate_needs_an_extern_crate_item() {
    // Without the extern prelude, `log` is whatever the module binds; only an
    // `extern crate` item makes it the crate.
    let pathed = |tag: &str, header: &'static str| {
        let result = Case {
            deps: "dep = { path = \"../dep\" }\nlog = \"0.4\"\n",
            lib_header: header,
            run_body: "log::info!(\"x\");",
            ..Case::default()
        }
        .resolve(tag);
        binds_da(&result)
    };
    assert!(!pathed("s2i", "#![no_implicit_prelude]\n"));
    assert!(pathed("s2j", "#![no_implicit_prelude]\nextern crate log;\n"));
    assert!(pathed("s2k", ""));
}

#[test]
fn s3_a_glob_cannot_rebind_the_root_of_a_pathed_call() {
    // rustc 1.99.1: a glob that brings a module `log` while `log` is a dependency
    // is "error[E0659]: `log` is ambiguous ... a name from a glob import and an
    // outer scope" (crate vs glob), in a block glob as well, so a crate that
    // builds has no such glob and the pathed call stays the crate's.
    let result = Case {
        deps: "dep = { path = \"../dep\" }\nlog = \"0.4\"\n",
        user_head: "use serde::*;",
        run_body: "log::info!(\"x\");",
        ..Case::default()
    }
    .resolve("s3");
    assert!(binds_da(&result));
}

#[test]
fn s4_a_payload_that_predates_the_current_facts_reads_as_unknown() {
    // A payload written before `facts_version` existed has no such key; it reads
    // as version 0, and absence of `macro_use_mods`, `no_std` and
    // `no_implicit_prelude` in it is not "none".
    let files = Case {
        run_body: "println!(\"x\");",
        ..Case::default()
    }
    .files();
    let extractions = |stale: bool| -> Vec<Extraction> {
        files
            .iter()
            .filter(|(path, _)| path.ends_with(".rs"))
            .map(|(path, source)| {
                let mut ext = extract_file(path, source);
                if stale {
                    if let Some(macros) = ext
                        .rust_scope_facts
                        .as_mut()
                        .and_then(|facts| facts.macros.as_mut())
                    {
                        macros.facts_version = 0;
                    }
                }
                ext
            })
            .collect()
    };
    for (stale, expect_edge) in [(false, true), (true, false)] {
        let extractions = extractions(stale);
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let result = resolver.resolve_all(&extractions).unwrap();
        assert_eq!(
            binds_da(&result),
            expect_edge,
            "stale={stale}: {:?}",
            targets_from(&result, "app/src/user.rs::run")
        );
    }
}

#[test]
fn s6_small_gaps() {
    // (a) `use {std::fmt::Debug, dep::Clone};` imports dep's `Clone`.
    let result = Case {
        user_head: "use {std::fmt::Debug, dep::Clone};",
        run_body: "#[derive(Clone)] struct _S;",
        ..Case::default()
    }
    .resolve("s6a");
    assert!(!binds_da(&result), "a braced import of std and dep");
    let result = Case {
        user_head: "use std::{fmt::Debug, clone::Clone};",
        run_body: "#[derive(Clone)] struct _S;",
        ..Case::default()
    }
    .resolve("s6a2");
    assert!(binds_da(&result), "a braced import that is all std");
    // (b) `#[macro_use] mod m;` inside an inline module reads `outer/m.rs`.
    for (tag, extra, expect_edge) in [
        (
            "s6b1",
            vec![("app/src/m.rs", "macro_rules! helper { () => { 1u8 }; }\n")],
            false,
        ),
        (
            "s6b2",
            vec![("app/src/outer/m.rs", "macro_rules! helper { () => { 1u8 }; }\n")],
            true,
        ),
    ] {
        let result = Case {
            lib_header: "mod outer {\n    #[macro_use]\n    mod m;\n}\n",
            run_body: "println!(\"x\");",
            extra,
            ..Case::default()
        }
        .resolve(tag);
        assert_eq!(
            binds_da(&result),
            expect_edge,
            "{tag}: {:?}",
            targets_from(&result, "app/src/user.rs::run")
        );
    }
    // (c) an out-of-line `mod log;` whose file is not indexed can stand for `log`.
    let result = Case {
        deps: "dep = { path = \"../dep\" }\nlog = \"0.4\"\n",
        lib_header: "mod log;\n",
        run_body: "log::info!(\"x\");",
        ..Case::default()
    }
    .resolve("s6c");
    assert!(!binds_da(&result), "an unindexed `mod log;`");
}
