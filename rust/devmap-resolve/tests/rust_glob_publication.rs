//! A Rust glob import brings in what the source module makes visible, and only
//! what the index can say is the whole set.
//!
//! * a private item is not published (`fn stdin` beside `pub fn open`), and an
//!   explicit `use std::io::stdin;` wins over any glob;
//! * a name published through a chain that holds an item-position macro, which
//!   may declare names the index never saw, is not bound at the import tier.

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

/// `(target_symbol, resolution kind)` of each Calls edge out of `caller`.
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
                _ => "other",
            };
            (edge.target_symbol.clone(), kind)
        })
        .collect()
}

#[test]
fn a_private_item_is_not_published_and_an_explicit_import_wins() {
    let result = resolve(&[
        (
            "src/lib.rs",
            "pub mod inner;\npub mod chain;\npub mod user;\n",
        ),
        (
            "src/inner.rs",
            "fn stdin() -> u32 { 1 }\npub fn open() -> u32 { stdin() }\n",
        ),
        ("src/chain.rs", "pub use crate::inner::*;\n"),
        (
            "src/user.rs",
            "use crate::chain::*;\nuse std::io::stdin;\npub fn read() -> u32 {\n    let _ = stdin();\n    open()\n}\n",
        ),
    ]);
    let calls = calls_from(&result, "src/user.rs::read");
    assert!(
        calls.contains(&("src/inner.rs::open".to_string(), "import_scoped")),
        "the pub item the chain publishes still binds: {calls:?}"
    );
    assert!(
        !calls
            .iter()
            .any(|(target, _)| target == "src/inner.rs::stdin"),
        "a private fn is not published, and the named std import beats the glob: {calls:?}"
    );
}

#[test]
fn a_chain_that_holds_an_item_macro_binds_below_the_import_tier() {
    let result = resolve(&[
        (
            "src/lib.rs",
            "pub mod m;\npub mod v;\npub mod chain;\npub mod user;\n",
        ),
        (
            "src/m.rs",
            "macro_rules! mk { () => { pub fn made() -> u32 { 1 } }; }\nmk!();\n",
        ),
        ("src/v.rs", "pub fn made() -> u32 { 2 }\n"),
        (
            "src/chain.rs",
            "pub use crate::m::*;\npub use crate::v::*;\n",
        ),
        (
            "src/user.rs",
            "use crate::chain::*;\npub fn call() -> u32 { made() }\n",
        ),
    ]);
    let calls = calls_from(&result, "src/user.rs::call");
    assert!(
        !calls.iter().any(|(_, kind)| *kind == "import_scoped"),
        "an unseen macro may publish `made` too, so the glob cannot claim it: {calls:?}"
    );
}

#[test]
fn a_complete_chain_still_binds_at_the_import_tier() {
    let result = resolve(&[
        ("src/lib.rs", "pub mod v;\npub mod chain;\npub mod user;\n"),
        ("src/v.rs", "pub fn made() -> u32 { 2 }\n"),
        ("src/chain.rs", "pub use crate::v::*;\n"),
        (
            "src/user.rs",
            "use crate::chain::*;\npub fn call() -> u32 { made() }\n",
        ),
    ]);
    let calls = calls_from(&result, "src/user.rs::call");
    assert_eq!(
        calls,
        vec![("src/v.rs::made".to_string(), "import_scoped")],
        "nothing hides a second publisher, so the one declaration binds"
    );
}
