//! A name the walk outward from a call site cannot place is `Unknown`, and
//! Unknown never becomes an outer or a default origin.
//!
//! * a glob nearer than the declaration that would answer provides the name when
//!   the index can prove it, is set aside when the index can prove it does not,
//!   and leaves the name unplaced otherwise (external module, a module holding an
//!   item macro, an enum's variants);
//! * a macro in a scope that could emit the name leaves it unplaced, except the
//!   std/core/log/tracing macros that emit nothing, and only while the crate
//!   defines no `macro_rules!` of the same name.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// `(target_symbol, tier)` of each Calls edge out of `caller`.
fn calls_from(result: &ResolutionResult, caller: &str) -> Vec<(String, &'static str)> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == caller)
        .map(|edge| {
            let kind = match edge.resolution.as_deref() {
                Some(Resolution::ImportScoped { .. }) => "import_scoped",
                Some(Resolution::UniqueGlobal { .. }) => "unique_global",
                Some(Resolution::SameFile { .. }) => "same_file",
                Some(Resolution::ReceiverType { .. }) => "receiver_type",
                _ => "other",
            };
            (edge.target_symbol.clone(), kind)
        })
        .collect()
}

const LIB: &str = "pub mod m;\npub mod m2;\npub mod user;\n";

#[test]
fn a_block_glob_that_provably_provides_the_name_binds_it_over_the_modules_own_item() {
    let result = resolve(&[
        ("src/lib.rs", LIB),
        ("src/m.rs", "pub fn h() -> u8 { 1 }\n"),
        ("src/m2.rs", "pub fn other() -> u8 { 1 }\n"),
        (
            "src/user.rs",
            "pub fn h() -> u16 { 2 }\npub fn run() -> u8 {\n    use crate::m::*;\n    h()\n}\n",
        ),
    ]);
    assert_eq!(
        calls_from(&result, "src/user.rs::run"),
        vec![("src/m.rs::h".to_string(), "import_scoped")]
    );
}

#[test]
fn a_block_glob_that_provably_lacks_the_name_leaves_the_outer_item_answering() {
    let result = resolve(&[
        ("src/lib.rs", LIB),
        ("src/m.rs", "pub fn other() -> u8 { 1 }\n"),
        ("src/m2.rs", "pub fn h() -> u8 { 1 }\n"),
        (
            "src/user.rs",
            "pub fn h() -> u16 { 2 }\npub fn run() -> u16 {\n    use crate::m::*;\n    h()\n}\n",
        ),
    ]);
    assert_eq!(
        calls_from(&result, "src/user.rs::run"),
        vec![("src/user.rs::h".to_string(), "same_file")]
    );
}

#[test]
fn a_nearer_glob_the_index_cannot_read_leaves_the_name_unplaced() {
    // std::mem is not indexed; a module with an item macro may make `gm`; an
    // enum's variants are not a module's items.
    let result = resolve(&[
        ("src/lib.rs", "pub mod mm;\npub mod user;\n"),
        (
            "src/mm.rs",
            "macro_rules! mk { () => { pub fn gm() -> u8 { 1 } }; }\nmk!();\n",
        ),
        (
            "src/user.rs",
            "pub enum E { Mk(u8) }\n\
             pub fn gm() -> u16 { 2 }\n\
             pub fn swap() -> u16 { 2 }\n\
             pub fn Mk(x: u8) -> u16 { x as u16 }\n\
             pub fn via_macro_module() -> u8 {\n    use crate::mm::*;\n    gm()\n}\n\
             pub fn via_std() -> u8 {\n    use std::mem::*;\n    swap(&mut 1, &mut 2);\n    0\n}\n\
             pub fn via_enum() -> E {\n    use E::*;\n    Mk(1)\n}\n",
        ),
    ]);
    for caller in ["via_macro_module", "via_std", "via_enum"] {
        let calls = calls_from(&result, &format!("src/user.rs::{caller}"));
        assert!(
            calls.iter().all(|(target, tier)| {
                !target.starts_with("src/user.rs::") && *tier != "same_file"
            }),
            "{caller} must not bind the module's own namesake: {calls:?}"
        );
    }
}

#[test]
fn a_macro_in_the_block_leaves_an_outer_namesake_unplaced_unless_it_is_known_silent() {
    let result = resolve(&[
        ("src/lib.rs", "pub mod user;\n"),
        (
            "src/user.rs",
            "macro_rules! emit { () => { fn made() -> u8 { 1 } }; }\n\
             pub fn made() -> u16 { 2 }\n\
             pub fn quiet() -> u16 { 2 }\n\
             pub fn emits() -> u8 {\n    emit!();\n    made()\n}\n\
             pub fn prints() -> u16 {\n    println!(\"x\");\n    let _v = vec![1u8];\n    quiet()\n}\n",
        ),
    ]);
    let emits = calls_from(&result, "src/user.rs::emits");
    assert!(
        emits.is_empty(),
        "a crate macro in the block may declare `made`: {emits:?}"
    );
    assert_eq!(
        calls_from(&result, "src/user.rs::prints"),
        vec![("src/user.rs::quiet".to_string(), "same_file")],
        "println! and vec! emit nothing"
    );
}

#[test]
fn a_silent_macro_name_the_crate_redefines_is_not_silent() {
    let result = resolve(&[
        ("src/lib.rs", "pub mod user;\n"),
        (
            "src/user.rs",
            "macro_rules! dbg { ($($t:tt)*) => { fn h() -> u8 { 1 } }; }\n\
             pub fn h() -> u16 { 2 }\n\
             pub fn run() -> u8 {\n    dbg!(1);\n    h()\n}\n",
        ),
    ]);
    let calls = calls_from(&result, "src/user.rs::run");
    assert!(calls.is_empty(), "dbg! is the crate's own macro: {calls:?}");
}

#[test]
fn a_macro_emitted_import_does_not_let_the_module_import_answer_a_type_path() {
    let result = resolve(&[
        ("src/lib.rs", "pub mod da;\npub mod db;\npub mod user;\n"),
        (
            "src/da.rs",
            "pub struct Dup;\nimpl Dup { pub fn mk() -> u16 { 2 } }\n",
        ),
        (
            "src/db.rs",
            "pub struct Dup;\nimpl Dup { pub fn mk() -> u8 { 1 } }\n",
        ),
        (
            "src/user.rs",
            "use crate::da::Dup;\n\
             macro_rules! use_db { () => { use crate::db::Dup; }; }\n\
             pub fn run() -> u8 {\n    use_db!();\n    Dup::mk()\n}\n",
        ),
    ]);
    let calls = calls_from(&result, "src/user.rs::run");
    assert!(
        calls
            .iter()
            .all(|(target, _)| target != "src/da.rs::Dup.mk"),
        "the block's own `use` may have replaced the module's: {calls:?}"
    );
}

/// Two crates' worth of `Dup` and a caller that imports the first at module
/// level; `body` runs inside `pub fn run() -> u8 { .. Dup::mk() }`.
fn dup_caller(header: &str, before_call: &str) -> Vec<(String, &'static str)> {
    let lib = if header.contains("#[macro_use]") {
        "#[macro_use]\nextern crate dep;\npub mod da;\npub mod db;\npub mod user;\n"
    } else {
        "pub mod da;\npub mod db;\npub mod user;\n"
    };
    let user = format!(
        "use crate::da::Dup;\n{}\npub fn run() -> u8 {{\n    {}\n    Dup::mk()\n}}\n",
        header.replace("#[macro_use]", ""),
        before_call
    );
    let result = resolve(&[
        ("src/lib.rs", lib),
        (
            "src/da.rs",
            "pub struct Dup;\nimpl Dup { pub fn mk() -> u16 { 2 } }\n",
        ),
        (
            "src/db.rs",
            "pub struct Dup;\nimpl Dup { pub fn mk() -> u8 { 1 } }\n",
        ),
        ("src/user.rs", user.as_str()),
    ]);
    calls_from(&result, "src/user.rs::run")
}

fn binds_da(calls: &[(String, &'static str)]) -> bool {
    calls
        .iter()
        .any(|(target, _)| target == "src/da.rs::Dup.mk")
}

#[test]
fn a_macro_imported_by_name_is_not_the_std_one() {
    let calls = dup_caller("use dep::info;", "info!(crate::db::Dup);");
    assert!(!binds_da(&calls), "{calls:?}");
}

#[test]
fn a_macro_use_extern_crate_leaves_no_macro_silent() {
    let calls = dup_caller("#[macro_use]", "vec!(crate::db::Dup);");
    assert!(!binds_da(&calls), "{calls:?}");
}

#[test]
fn a_crate_alias_is_not_the_crate_its_name_says() {
    let calls = dup_caller("extern crate dep as log;", "log::info!(crate::db::Dup);");
    assert!(!binds_da(&calls), "{calls:?}");
}

#[test]
fn an_attribute_macro_on_a_block_item_leaves_the_block_unplaced() {
    let calls = dup_caller("", "#[pm::with_use(crate::db::Dup)] fn _x() {}");
    assert!(!binds_da(&calls), "{calls:?}");
}

#[test]
fn an_inert_attribute_on_a_block_item_places_nothing_in_doubt() {
    let calls = dup_caller("", "#[allow(dead_code)] #[inline] fn _x() {}");
    assert_eq!(
        calls,
        vec![("src/da.rs::Dup.mk".to_string(), "receiver_type")]
    );
}

#[test]
fn a_derive_of_a_dependency_trait_is_not_inert() {
    let calls = dup_caller("", "#[derive(Debug, Serialize)] struct _S;");
    assert!(!binds_da(&calls), "{calls:?}");
    let calls = dup_caller("", "#[derive(Debug, Clone)] struct _S;");
    assert!(binds_da(&calls), "{calls:?}");
}

#[test]
fn a_import_of_the_real_logging_macro_stays_silent() {
    let calls = dup_caller("use log::info;", "info!(\"x\");");
    assert!(binds_da(&calls), "{calls:?}");
}

#[test]
fn thread_local_declares_only_the_statics_it_names() {
    let result = resolve(&[
        ("src/lib.rs", "pub mod user;\n"),
        (
            "src/user.rs",
            "thread_local! {\n    static T: u8 = const { 1 };\n}\n\
             pub fn h() -> u16 { 2 }\n\
             pub fn run() -> u16 { h() }\n",
        ),
    ]);
    assert_eq!(
        calls_from(&result, "src/user.rs::run"),
        vec![("src/user.rs::h".to_string(), "same_file")]
    );
}

#[test]
fn a_crate_thread_local_macro_is_not_the_std_one() {
    let body = |defined: &str| {
        let user = format!(
            "{defined}pub fn h() -> u16 {{ 2 }}\npub fn run() -> u16 {{\n    thread_local!();\n    h()\n}}\n"
        );
        let result = resolve(&[
            ("src/lib.rs", "pub mod user;\n"),
            ("src/user.rs", user.as_str()),
        ]);
        calls_from(&result, "src/user.rs::run")
    };
    assert!(body("macro_rules! thread_local { () => { fn h() -> u8 { 1 } }; }\n").is_empty());
    assert_eq!(
        body(""),
        vec![("src/user.rs::h".to_string(), "same_file")],
        "the std macro declares only the statics it names"
    );
}

#[test]
fn bail_declares_nothing_when_it_is_anyhows() {
    let body = |root: &str| {
        let user = format!(
            "pub fn h() -> u16 {{ 2 }}\npub fn run(f: bool) -> u16 {{\n    if f {{\n        {root}::bail!(\"no\");\n        return h();\n    }}\n    0\n}}\n"
        );
        let result = resolve(&[
            ("src/lib.rs", "pub mod user;\n"),
            ("src/user.rs", user.as_str()),
        ]);
        calls_from(&result, "src/user.rs::run")
    };
    assert_eq!(
        body("anyhow"),
        vec![("src/user.rs::h".to_string(), "same_file")]
    );
    assert!(
        body("other").is_empty(),
        "a path root that is not anyhow or eyre is not a silent macro"
    );
}
