//! An extraction written before `rust_scope_facts` existed carries none. Absent
//! is not empty: such a file's scopes are unknown, and unknown never yields a
//! confident edge - and never an edge the base resolver would not have made.
//!
//! Each case resolves the same corpus twice: with fresh facts (the edge the model
//! expects) and with the facts removed from every extraction, as a stale cache
//! serves them.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)], strip: bool) -> ResolutionResult {
    let mut extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    if strip {
        for extraction in &mut extractions {
            extraction.rust_scope_facts = None;
        }
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// `(target_symbol, tier name)` of each Calls edge out of `caller`.
fn calls_from(result: &ResolutionResult, caller: &str) -> Vec<(String, &'static str)> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.source_symbol == caller)
        .map(|edge| {
            let tier = match edge.resolution.as_deref() {
                Some(Resolution::ImportScoped { .. }) => "import_scoped",
                Some(Resolution::SameFile { .. }) => "same_file",
                Some(Resolution::ReceiverType { .. }) => "receiver_type",
                Some(Resolution::UniqueGlobal { .. }) => "unique_global",
                _ => "other",
            };
            (edge.target_symbol.clone(), tier)
        })
        .collect()
}

fn confident(calls: &[(String, &'static str)]) -> Vec<(String, &'static str)> {
    calls
        .iter()
        .filter(|(_, tier)| matches!(*tier, "import_scoped" | "same_file" | "receiver_type"))
        .cloned()
        .collect()
}

#[test]
fn a_block_level_use_is_not_guessed_without_scope_facts() {
    let files = [
        ("src/lib.rs", "pub mod gt;\npub mod vs;\n"),
        ("src/gt.rs", "pub fn record(a: u8, b: u8, c: u8) -> u8 { a + b + c }\n"),
        (
            "src/vs.rs",
            "fn record(a: u8) -> u16 { a as u16 }\npub fn evidence() -> u8 {\n    use crate::gt::record;\n    record(1, 2, 3)\n}\n",
        ),
    ];
    let fresh = calls_from(&resolve(&files, false), "src/vs.rs::evidence");
    assert_eq!(
        fresh,
        vec![("src/gt.rs::record".to_string(), "import_scoped")]
    );
    let stale = calls_from(&resolve(&files, true), "src/vs.rs::evidence");
    assert!(
        confident(&stale).is_empty(),
        "unknown scopes must not give a confident edge: {stale:?}"
    );
}

#[test]
fn a_glob_over_a_restricted_item_is_not_bound_without_scope_facts() {
    let files = [
        ("src/lib.rs", "pub mod a;\npub mod other;\npub mod run;\n"),
        (
            "src/a.rs",
            "pub(super) fn sup_only() -> u16 { 2 }\npub fn open() -> u8 { 1 }\n",
        ),
        ("src/other.rs", "pub fn sup_only() -> u8 { 1 }\n"),
        (
            "src/run.rs",
            "use crate::a::*;\nuse crate::other::*;\npub fn go() -> u8 { sup_only() + open() }\n",
        ),
    ];
    let stale = calls_from(&resolve(&files, true), "src/run.rs::go");
    assert!(
        confident(&stale).is_empty(),
        "restricted visibility is unknown without facts, so nothing is bound at 1.0: {stale:?}"
    );
}

#[test]
fn a_chain_is_opaque_without_scope_facts() {
    let files = [
        ("src/lib.rs", "pub mod m;\npub mod chain;\npub mod user;\n"),
        (
            "src/m.rs",
            "macro_rules! mk { () => { pub fn made() -> u8 { 1 } }; }\nmk!();\n",
        ),
        ("src/chain.rs", "pub use crate::m::*;\n"),
        (
            "src/user.rs",
            "use crate::chain::*;\npub fn call() -> u8 { made() }\n",
        ),
    ];
    let stale = calls_from(&resolve(&files, true), "src/user.rs::call");
    assert!(confident(&stale).is_empty(), "{stale:?}");
}

#[test]
fn a_type_receiver_is_not_narrowed_without_scope_facts() {
    let files = [
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
            "use crate::da::Dup;\npub fn fl() -> u8 {\n    use crate::db::Dup;\n    Dup::mk()\n}\n",
        ),
    ];
    let fresh = calls_from(&resolve(&files, false), "src/user.rs::fl");
    assert_eq!(
        fresh,
        vec![("src/db.rs::Dup.mk".to_string(), "receiver_type")]
    );
    let stale = calls_from(&resolve(&files, true), "src/user.rs::fl");
    assert!(confident(&stale).is_empty(), "{stale:?}");
}
