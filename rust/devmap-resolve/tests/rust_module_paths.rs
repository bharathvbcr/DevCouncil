//! X41, resolver half — a Rust `use` path names the file the module system
//! says it names.
//!
//! Three rungs were wrong in ways that only showed up once `use` started
//! arriving as a real path (see `devmap-extract/tests/rust_use_declarations.rs`
//! for the extractor half):
//!
//! * `crate::` was probed against a literal `src/…` from the **repository**
//!   root. A workspace member lives at `crates/<name>/src/…`, so every
//!   `use crate::model::…` in this repository's own kernel resolved to nothing.
//! * `super::x` walked one directory too far. `src/deep/leaf.rs` is the module
//!   `deep::leaf`, so its `super` is `deep`, whose children are in `src/deep/`
//!   — not in `src/`. The old rung was right only for a `mod.rs`, which is the
//!   one shape the pre-existing test used.
//! * Bare `self`, `super` and `crate` — which `use super::*;` and `use crate::*;`
//!   reduce to — matched no rung at all, because every arm tested for a `::`
//!   suffix.
//!
//! And the shape the whole item exists for: `mod tests { use super::*; }`
//! names the enclosing *file*. It must resolve, so the ledger stops calling it
//! an unresolved relative import — and it must not emit an `Imports` edge,
//! because an edge from a file to itself is not a dependency. `langimports/
//! rust.rs` already declines to emit one for the `mod` half of the same fact.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{ResolutionResult, UnresolvedKind};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

fn import_edges(result: &ResolutionResult) -> Vec<(String, String)> {
    let mut rows: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Imports)
        .map(|edge| (edge.source_file.clone(), edge.target_file.clone()))
        .collect();
    rows.sort();
    rows.dedup();
    rows
}

fn unresolved_imports(result: &ResolutionResult) -> Vec<String> {
    result
        .unresolved
        .iter()
        .filter(|row| row.kind == UnresolvedKind::Import)
        .map(|row| format!("{} in {}", row.callee_name, row.source_file))
        .collect()
}

/// A workspace member. `crate::` is relative to the crate, and the crate root
/// is the innermost `src` above the file.
#[test]
fn a_crate_path_resolves_against_its_own_crate_not_the_repository_root() {
    let (_, result) = resolve(&[
        ("crates/thing/src/lib.rs", "pub mod model;\npub mod work;\n"),
        ("crates/thing/src/model.rs", "pub struct Widget;\n"),
        (
            "crates/thing/src/work.rs",
            "use crate::model::Widget;\n\npub fn build() -> Widget {\n    Widget\n}\n",
        ),
    ]);

    assert!(
        import_edges(&result).contains(&(
            "crates/thing/src/work.rs".to_string(),
            "crates/thing/src/model.rs".to_string()
        )),
        "`use crate::model::Widget` names a file this crate contains; got {:?}",
        import_edges(&result)
    );
}

/// `super::x` from a leaf module stays in the leaf's own directory.
#[test]
fn a_super_path_from_a_leaf_module_stays_in_its_directory() {
    let (_, result) = resolve(&[
        ("src/deep/mod.rs", "pub mod leaf;\npub mod sibling;\n"),
        (
            "src/deep/leaf.rs",
            "use super::sibling::help;\n\npub fn l() {\n    help();\n}\n",
        ),
        ("src/deep/sibling.rs", "pub fn help() {}\n"),
        ("src/sibling.rs", "pub fn help() {}\n"),
    ]);

    let edges = import_edges(&result);
    assert!(
        edges.contains(&(
            "src/deep/leaf.rs".to_string(),
            "src/deep/sibling.rs".to_string()
        )),
        "`deep::leaf`'s `super` is `deep`, whose children are in src/deep/; got {edges:?}"
    );
    assert!(
        !edges.contains(&("src/deep/leaf.rs".to_string(), "src/sibling.rs".to_string())),
        "src/sibling.rs is `crate::sibling`, one module further out than the \
         statement names; got {edges:?}"
    );
}

/// The pre-existing shape, kept: from a `mod.rs`, `super::x` really is one
/// directory up. Same answer as before this change, for the stated reason
/// rather than by accident.
#[test]
fn a_super_path_from_a_directory_module_still_walks_up() {
    let (_, result) = resolve(&[
        (
            "src/deep/mod.rs",
            "use super::sibling::help;\npub mod leaf;\n",
        ),
        ("src/deep/leaf.rs", "pub fn l() {}\n"),
        ("src/sibling.rs", "pub fn help() {}\n"),
    ]);

    assert!(
        import_edges(&result)
            .contains(&("src/deep/mod.rs".to_string(), "src/sibling.rs".to_string())),
        "a `mod.rs` *is* its module, so its `super` is the directory above it"
    );
}

const INLINE_TEST_MODULE: &str = "\
pub fn helper(value: u32) -> u32 {
    value + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_adds() {
        assert_eq!(helper(1), 2);
    }
}
";

/// The 87-row shape on this repository. `super` inside an inline module is the
/// file itself: resolvable, so not an index gap — and not an edge, because a
/// file does not import itself.
#[test]
fn a_test_modules_super_glob_is_not_an_unresolved_relative_import() {
    let (_, result) = resolve(&[("crates/thing/src/work.rs", INLINE_TEST_MODULE)]);

    assert!(
        unresolved_imports(&result).is_empty(),
        "`use super::*` in an inline module names this very file, which is \
         indexed — recording it as a relative import that found nothing calls \
         an index gap where there is none. got {:?}",
        unresolved_imports(&result)
    );
    assert!(
        import_edges(&result).is_empty(),
        "a file does not import itself; got {:?}",
        import_edges(&result)
    );
}

/// The call through the glob is a `SameFile` edge, which it must have been all
/// along — asserted so the change above cannot have cost it.
#[test]
fn the_call_through_an_inline_test_glob_is_a_same_file_edge() {
    let (_, result) = resolve(&[("crates/thing/src/work.rs", INLINE_TEST_MODULE)]);

    let call = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("::helper"))
        .expect("the test module calls `helper`");
    assert_eq!(
        call.source_file, "crates/thing/src/work.rs",
        "the caller is in the same file"
    );
    assert_eq!(
        format!("{:?}", call.evidence.expect("resolver evidence").kind),
        "SameFile",
        "the declaration is in this very file"
    );
}

/// A `use` of a crate that is not in the corpus is the evidence that makes a
/// later failure expected rather than a defect. Rust produced **no** `External`
/// row at all before this change, across 33,395 unresolved sites on this
/// repository, while Python produced 8,734 from the same ladder.
#[test]
fn an_external_crate_import_classifies_its_names_as_external() {
    let (_, result) = resolve(&[(
        "crates/thing/src/work.rs",
        "use serde_json::Value;\n\npub fn read(raw: Value) -> Value {\n    raw\n}\n",
    )]);

    let rows: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == "Value")
        .map(|row| row.class.label().to_string())
        .collect();
    assert!(
        !rows.is_empty() && rows.iter().all(|class| class == "external"),
        "`use serde_json::Value` proves `Value` comes from outside the corpus; \
         got {rows:?}"
    );
}

/// A glob binds every name the target module exports, so a call through it
/// resolves on import evidence rather than falling to the global tier.
#[test]
fn a_glob_import_binds_the_modules_whole_surface() {
    let (_, result) = resolve(&[
        (
            "crates/thing/src/lib.rs",
            "pub mod prelude;\npub mod work;\n",
        ),
        (
            "crates/thing/src/prelude.rs",
            "pub fn shared() -> u32 {\n    1\n}\n",
        ),
        (
            "crates/thing/src/work.rs",
            "use crate::prelude::*;\n\npub fn run() -> u32 {\n    shared()\n}\n",
        ),
    ]);

    let call = result
        .edges
        .iter()
        .find(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol.ends_with("::shared"))
        .expect("`shared()` is called through the glob");
    assert_eq!(
        call.target_file, "crates/thing/src/prelude.rs",
        "the glob is what names where `shared` lives"
    );
    assert_eq!(
        format!("{:?}", call.evidence.expect("resolver evidence").kind),
        "ImportScoped",
        "an import in this file is what bound the name, and that is the rung \
         its evidence entitles it to"
    );
}
