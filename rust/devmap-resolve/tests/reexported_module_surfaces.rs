//! A module surface republished under another name — Rust's `pub use`, and the
//! glob form of it in every language that has one.
//!
//! `reexport_chains.rs` established the rule this file extends: a chain is
//! recorded where the export *names the module it came from*, and nowhere else.
//! `export { x } from './m'` carries that evidence; Python's
//! `from .impl import thing` does not, because the extractor records it as an
//! import. Two shapes carry it and were still dropped.
//!
//! **Rust `pub use`.** The extractor emitted no `ExtractedExport` at all for a
//! `use_declaration`, public or not, so `compute_reexport_chains` never saw a
//! single Rust re-export. That is the canonical Rust library layout: `lib.rs`
//! republishes the crate surface and every consumer imports through the crate
//! root. Measured on this repository — `devmap-store/src/lib.rs` carries
//! `pub use schema::*;`, and `tests/migration_ladder.rs` imports
//! `CURRENT_SCHEMA_VERSION` through it. The graph gave that constant six
//! callers, all in `db.rs`, which names `crate::schema::` directly. The test
//! that guards the migration ladder against a schema change had no edge to the
//! schema at all.
//!
//! **The glob.** `export * from './impl'` already produced an export record
//! with `exported_name: "*"`, so the hop map held `src/index.js::*` pointing at
//! `("src/impl.js", "*")`. No file declares a symbol literally named `*`, so the
//! walk could never terminate and every such chain was silently dropped. The
//! extractor cannot expand a glob — it sees one file and a glob names whatever
//! *another* file declares — so it states the glob and the resolver, which
//! holds the whole-corpus symbol table, turns it into one hop per name.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn chain<'a>(resolution: &'a ResolutionResult, key: &str) -> Option<&'a str> {
    resolution.reexport_chains.get(key).map(String::as_str)
}

/// The named case: `pub use schema::stamp;` states where `stamp` is declared.
#[test]
fn a_rust_pub_use_names_where_a_symbol_comes_from() {
    let (_, resolution) = resolve(&[
        ("src/schema.rs", "pub fn stamp() -> i32 {\n    22\n}\n"),
        ("src/lib.rs", "pub mod schema;\npub use schema::stamp;\n"),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::stamp"),
        Some("src/schema.rs::stamp"),
        "`pub use schema::stamp` names the declaring module: {:?}",
        resolution.reexport_chains
    );
}

/// A private `use` is not a re-export and must record nothing.
///
/// `use schema::stamp;` brings the name into scope for *this file only*. A
/// consumer importing it from here would not compile, so a chain through it
/// would be a route the language does not have. This is the boundary that
/// keeps the map evidence-bound: the visibility is the evidence, and it is
/// read rather than assumed.
#[test]
fn a_private_rust_use_is_not_a_reexport() {
    let (_, resolution) = resolve(&[
        ("src/schema.rs", "pub fn stamp() -> i32 {\n    22\n}\n"),
        ("src/lib.rs", "pub mod schema;\nuse schema::stamp;\n"),
    ]);
    assert!(
        resolution.reexport_chains.is_empty(),
        "a private `use` publishes nothing: {:?}",
        resolution.reexport_chains
    );
}

/// `pub(crate) use` is likewise not a public surface.
///
/// It is still a re-export *within* the crate, but the name it publishes is not
/// reachable from outside it. Recording it would be the same overreach as
/// recording a private `use`, one crate wider.
#[test]
fn a_crate_visible_use_is_not_a_public_reexport() {
    let (_, resolution) = resolve(&[
        ("src/schema.rs", "pub fn stamp() -> i32 {\n    22\n}\n"),
        (
            "src/lib.rs",
            "pub mod schema;\npub(crate) use schema::stamp;\n",
        ),
    ]);
    assert!(
        resolution.reexport_chains.is_empty(),
        "`pub(crate)` does not republish across the crate boundary: {:?}",
        resolution.reexport_chains
    );
}

/// A rename republishes under the new name and asks the module for the old one.
#[test]
fn a_renaming_rust_pub_use_follows_the_source_name() {
    let (_, resolution) = resolve(&[
        ("src/schema.rs", "pub fn stamp() -> i32 {\n    22\n}\n"),
        (
            "src/lib.rs",
            "pub mod schema;\npub use schema::stamp as version_stamp;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::version_stamp"),
        Some("src/schema.rs::stamp"),
        "the published name is the alias, the asked-for name is the original: {:?}",
        resolution.reexport_chains
    );
}

/// `pub use schema::*;` — the shape `devmap-store/src/lib.rs` actually uses.
#[test]
fn a_rust_glob_reexport_expands_to_every_declared_name() {
    let (_, resolution) = resolve(&[
        (
            "src/schema.rs",
            "pub fn stamp() -> i32 {\n    22\n}\n\npub fn ladder() -> i32 {\n    5\n}\n",
        ),
        ("src/lib.rs", "pub mod schema;\npub use schema::*;\n"),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::stamp"),
        Some("src/schema.rs::stamp"),
        "a glob republishes every name the module declares: {:?}",
        resolution.reexport_chains
    );
    assert_eq!(
        chain(&resolution, "src/lib.rs::ladder"),
        Some("src/schema.rs::ladder"),
        "including the second one: {:?}",
        resolution.reexport_chains
    );
}

/// The same defect in JavaScript, which is how it was found to be a class
/// rather than one language's case. Both spellings reach the same map, so one
/// expansion fixes both.
#[test]
fn a_javascript_star_reexport_expands_to_every_declared_name() {
    let (_, resolution) = resolve(&[
        ("src/impl.js", "export function thing() {\n  return 1;\n}\n"),
        ("src/index.js", "export * from './impl';\n"),
    ]);
    assert_eq!(
        chain(&resolution, "src/index.js::thing"),
        Some("src/impl.js::thing"),
        "`export *` republishes every declared name: {:?}",
        resolution.reexport_chains
    );
}

/// An explicit re-export outranks a glob that also covers the name.
///
/// `pub use a::thing; pub use b::*;` publishes `a`'s `thing` — that is the
/// language's rule, and it is the one that matters here because binding the
/// consumer to `b` would be a *confident* edge to the wrong file, which is
/// worse than the ambiguity this map exists to remove.
#[test]
fn an_explicit_reexport_outranks_a_glob_covering_the_same_name() {
    let (_, resolution) = resolve(&[
        ("src/a.rs", "pub fn thing() -> i32 {\n    1\n}\n"),
        ("src/b.rs", "pub fn thing() -> i32 {\n    2\n}\n"),
        (
            "src/lib.rs",
            "pub mod a;\npub mod b;\npub use a::thing;\npub use b::*;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::thing"),
        Some("src/a.rs::thing"),
        "the name the author wrote wins over the one a glob swept in: {:?}",
        resolution.reexport_chains
    );
}

/// A glob through a glob, which is what a crate root republishing a module that
/// itself republishes looks like. The expansion reaches a fixpoint rather than
/// stopping after one round.
#[test]
fn a_glob_through_a_glob_still_reaches_the_declaring_file() {
    let (_, resolution) = resolve(&[
        ("src/deep.rs", "pub fn thing() -> i32 {\n    1\n}\n"),
        ("src/mid.rs", "pub mod deep;\npub use deep::*;\n"),
        ("src/lib.rs", "pub mod mid;\npub use mid::*;\n"),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::thing"),
        Some("src/deep.rs::thing"),
        "two globs still name the declaring file: {:?}",
        resolution.reexport_chains
    );
    assert_eq!(
        chain(&resolution, "src/mid.rs::thing"),
        Some("src/deep.rs::thing"),
        "and the intermediate surface is published too: {:?}",
        resolution.reexport_chains
    );
}

/// Two files globbing each other must record nothing, the same rule the named
/// cycle already follows. A glob makes the cycle easy to write by accident, so
/// it is pinned rather than assumed to inherit the behaviour.
#[test]
fn mutually_globbing_files_produce_no_chain() {
    let (_, resolution) = resolve(&[
        ("src/a.rs", "pub mod b;\npub use b::*;\n"),
        ("src/b.rs", "pub mod a;\npub use a::*;\n"),
    ]);
    assert!(
        resolution.reexport_chains.is_empty(),
        "neither file declares anything, so there is no terminal to record: {:?}",
        resolution.reexport_chains
    );
}

/// A glob naming a module outside the corpus records nothing.
///
/// `pub use serde::*;` cannot be expanded — the declaring file is not indexed —
/// and inventing an endpoint for it would manufacture graph structure. This is
/// the same refusal `a_reexport_from_outside_the_corpus_records_no_chain`
/// already pins for the named form.
#[test]
fn a_glob_from_outside_the_corpus_records_no_chain() {
    let (_, resolution) = resolve(&[("src/lib.rs", "pub use serde::*;\n")]);
    assert!(
        resolution.reexport_chains.is_empty(),
        "an unindexed module has no declared names to republish: {:?}",
        resolution.reexport_chains
    );
}

/// The case this whole file was opened for, in the layout it actually occurs
/// in: an integration test importing a constant through the crate root.
///
/// `rust/devmap-store/tests/migration_ladder.rs` writes
/// `use devmap_store::{Store, CURRENT_SCHEMA_VERSION};`, and
/// `devmap-store/src/lib.rs` publishes that constant with `pub use schema::*;`.
/// The graph gave `CURRENT_SCHEMA_VERSION` six callers, every one of them in
/// `db.rs` — the single file naming `crate::schema::` directly — so the test
/// that guards the migration ladder against a schema change had no edge to the
/// schema, and `devmap_affected_tests` could not name it at any depth.
///
/// Four things had to hold for that edge to exist, and none of them did: the
/// `pub use` had to be recorded as an export, the glob had to expand, a bare
/// module path had to resolve, and the *crate name* had to name its own root.
/// A `tests/` file has no `src` ancestor, so the crate-relative rung fell back
/// to a literal `src` and answered nothing.
#[test]
fn an_integration_test_reaches_a_constant_through_the_crate_root() {
    let (_, resolution) = resolve(&[
        (
            "rust/devmap-store/src/schema.rs",
            "pub const CURRENT_SCHEMA_VERSION: i32 = 22;\n",
        ),
        (
            "rust/devmap-store/src/lib.rs",
            "pub mod schema;\npub use schema::*;\n",
        ),
        (
            "rust/devmap-store/tests/migration_ladder.rs",
            "use devmap_store::CURRENT_SCHEMA_VERSION;\n\n#[test]\nfn ladder() {\n    assert_eq!(CURRENT_SCHEMA_VERSION, 22);\n}\n",
        ),
    ]);
    let targets: Vec<&str> = resolution
        .edges
        .iter()
        .filter(|edge| edge.source_file == "rust/devmap-store/tests/migration_ladder.rs")
        .filter(|edge| edge.target_symbol.ends_with("::CURRENT_SCHEMA_VERSION"))
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert!(
        targets.contains(&"rust/devmap-store/src/schema.rs"),
        "the test that guards the schema must have an edge to it; got {targets:?}\n\
         chains: {:?}",
        resolution.reexport_chains
    );
}

/// A deep cross-crate path lands on the module that declares the name, not on
/// the crate root — the same walk `crate::` already does.
#[test]
fn a_deep_cross_crate_path_lands_on_the_declaring_module() {
    let (_, resolution) = resolve(&[
        (
            "rust/devmap-store/src/schema.rs",
            "pub const CURRENT_SCHEMA_VERSION: i32 = 22;\n",
        ),
        ("rust/devmap-store/src/lib.rs", "pub mod schema;\n"),
        (
            "rust/devmap-store/tests/ladder.rs",
            "use devmap_store::schema::CURRENT_SCHEMA_VERSION;\n\nfn go() -> i32 {\n    CURRENT_SCHEMA_VERSION\n}\n",
        ),
    ]);
    let targets: Vec<&str> = resolution
        .edges
        .iter()
        .filter(|edge| edge.source_file == "rust/devmap-store/tests/ladder.rs")
        .filter(|edge| edge.target_symbol.ends_with("::CURRENT_SCHEMA_VERSION"))
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert!(
        targets.contains(&"rust/devmap-store/src/schema.rs"),
        "the named module answers directly, with no chain needed; got {targets:?}"
    );
}

/// Two indexed directories claiming one crate name abstain rather than pick.
///
/// A vendored copy beside its original is the common way this happens, and
/// picking either would be a confident edge decided by sort order. This is the
/// same rule the unique-basename rung follows, and the reason either can be
/// trusted at all: neither ever picks a winner.
#[test]
fn two_crates_of_the_same_name_resolve_to_neither() {
    let (_, resolution) = resolve(&[
        (
            "a/thing/src/lib.rs",
            "pub fn only_here() -> i32 {\n    1\n}\n",
        ),
        (
            "b/thing/src/lib.rs",
            "pub fn only_here() -> i32 {\n    2\n}\n",
        ),
        (
            "c/user/src/lib.rs",
            "use thing::only_here;\n\npub fn go() -> i32 {\n    only_here()\n}\n",
        ),
    ]);
    // The ambiguity ladder may still fan out on the bare name at low
    // confidence — that is its job, and it says so in the confidence. What must
    // not happen is the *import* picking one crate and presenting it as
    // settled.
    let named: Vec<(&str, f32)> = resolution
        .edges
        .iter()
        .filter(|edge| edge.source_file == "c/user/src/lib.rs")
        .filter(|edge| edge.target_symbol.ends_with("::only_here"))
        .map(|edge| (edge.target_file.as_str(), edge.confidence.0))
        .collect();
    assert!(
        !named.is_empty(),
        "the call is still reported, or this asserts nothing"
    );
    assert!(
        named.iter().all(|(_, confidence)| *confidence < 1.0),
        "an ambiguous crate name must not produce a settled edge; got {named:?}"
    );
}

/// A crate carrying both a `lib.rs` and a `main.rs` is one crate with one root,
/// not two claimants of the same name. Pinned because the abstention above is
/// keyed on the *root*, and keying it on the file would have made every binary
/// crate with a library ambiguous with itself.
#[test]
fn a_crate_with_both_a_lib_and_a_main_is_not_ambiguous_with_itself() {
    let (_, resolution) = resolve(&[
        (
            "rust/tool/src/lib.rs",
            "pub mod helper;\npub use helper::*;\n",
        ),
        (
            "rust/tool/src/helper.rs",
            "pub fn work() -> i32 {\n    1\n}\n",
        ),
        ("rust/tool/src/main.rs", "fn main() {}\n"),
        (
            "rust/tool/tests/it.rs",
            "use tool::work;\n\n#[test]\nfn t() {\n    work();\n}\n",
        ),
    ]);
    let targets: Vec<&str> = resolution
        .edges
        .iter()
        .filter(|edge| edge.source_file == "rust/tool/tests/it.rs")
        .filter(|edge| edge.target_symbol.ends_with("::work"))
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert!(
        targets.contains(&"rust/tool/src/helper.rs"),
        "the crate still resolves, through its glob re-export; got {targets:?}\n\
         chains: {:?}",
        resolution.reexport_chains
    );
}

/// The guard on reading a bare Rust path as `self::`-relative.
///
/// Uniform paths make `use schema::stamp;` and `use serde::Serialize;`
/// syntactically identical — one names a child module, the other an external
/// crate — and only the corpus can tell them apart. The arm therefore accepts a
/// candidate solely when it is an **indexed file next to the importer**, so an
/// external crate resolves to nothing exactly as it did before. Pinned here
/// because the failure mode is silent: a wrong binding is a confident edge to
/// an unrelated file, which is worse than the missing one it replaced.
#[test]
fn an_external_crate_path_binds_to_nothing() {
    let (_, resolution) = resolve(&[
        ("src/schema.rs", "pub fn stamp() -> i32 {\n    22\n}\n"),
        (
            "src/lib.rs",
            "pub mod schema;\nuse serde::Serialize;\npub use schema::stamp;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::stamp"),
        Some("src/schema.rs::stamp"),
        "the local module still resolves: {:?}",
        resolution.reexport_chains
    );
    assert!(
        !resolution
            .edges
            .iter()
            .any(|edge| edge.target_symbol.ends_with("::Serialize")),
        "`serde` names no indexed file, so it binds to none"
    );
}

/// A name a glob would sweep in from a module that does not declare it is not
/// published by that module, and the expansion must not invent it.
///
/// This is the boundary between "expand what the target publishes" and "expand
/// every name in the corpus". A glob over an *empty* module publishes nothing,
/// and a consumer of an unrelated name gets no chain through it.
#[test]
fn a_glob_over_a_module_that_declares_nothing_publishes_nothing() {
    let (_, resolution) = resolve(&[
        ("src/empty.rs", "// nothing here\n"),
        ("src/other.rs", "pub fn thing() -> i32 {\n    1\n}\n"),
        (
            "src/lib.rs",
            "pub mod empty;\npub mod other;\npub use empty::*;\n",
        ),
    ]);
    assert_eq!(
        chain(&resolution, "src/lib.rs::thing"),
        None,
        "`other` was never re-exported, so the glob over `empty` must not \
         publish its name: {:?}",
        resolution.reexport_chains
    );
}

/// The property the whole thing exists for, end to end: a consumer that reaches
/// a symbol *only* through a glob re-export gets an edge to the file that
/// declares it, not to the surface it came through.
#[test]
fn a_consumer_through_a_glob_binds_to_the_declaring_file() {
    let (_, resolution) = resolve(&[
        ("src/impl.js", "export function thing() {\n  return 1;\n}\n"),
        ("src/index.js", "export * from './impl';\n"),
        (
            "src/app.js",
            "import { thing } from './index';\nexport function go() {\n  return thing();\n}\n",
        ),
    ]);
    let bound: Vec<&str> = resolution
        .edges
        .iter()
        .filter(|edge| edge.source_file == "src/app.js")
        .filter(|edge| edge.target_symbol.ends_with("::thing"))
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert!(
        bound.contains(&"src/impl.js"),
        "the call lands on the declaring file, not the barrel: {bound:?}"
    );
    assert!(
        !bound.contains(&"src/index.js"),
        "and not on the surface it came through: {bound:?}"
    );
}
