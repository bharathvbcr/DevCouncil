//! A Kotlin `typealias` beside the class it spells does not take that class's
//! uses away (card ft-4617a335, AC7 corpus diff on ChronosFlow).
//!
//! `typealias T = pkg.model.T` is a `Struct` node. Left in the bare-name index
//! next to `class T` it made `T(..)` an ambiguous global, a `T`-typed binding
//! unresolved and the alias's own right-hand `T` a reference to itself: 55
//! true edges lost on ChronosFlow. An alias alone under its name keeps the
//! candidate.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::Resolver;

const CLASS: &str = "package a.model\n\nclass T(val x: Int)\n";
const ALIAS: &str = "package a\n\ntypealias T = a.model.T\ntypealias Handler = (Int) -> Unit\n";
const USER: &str = "package a\n\nfun f() = T(1)\nfun g(t: T) = t\nfun h(cb: Handler) = cb(1)\n";

/// `(source, target, kind)` of every use edge.
fn edges(files: &[(&str, &str)]) -> Vec<(String, String, EdgeKind)> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let mut found: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References))
        .map(|edge| {
            (
                edge.source_symbol.clone(),
                edge.target_symbol.clone(),
                edge.edge_kind,
            )
        })
        .collect();
    found.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    found
}

fn has(found: &[(String, String, EdgeKind)], source: &str, target: &str, kind: EdgeKind) -> bool {
    found
        .iter()
        .any(|(s, t, k)| s == source && t == target && *k == kind)
}

fn fixture() -> Vec<(String, String, EdgeKind)> {
    edges(&[
        ("a/model/T.kt", CLASS),
        ("a/alias.kt", ALIAS),
        ("a/user.kt", USER),
    ])
}

#[test]
fn a_call_to_the_alias_name_reaches_the_class() {
    let found = fixture();
    assert!(
        has(&found, "a/user.kt::f", "a/model/T.kt::T", EdgeKind::Calls),
        "{found:?}"
    );
}

#[test]
fn a_binding_typed_by_the_alias_name_references_the_class() {
    let found = fixture();
    assert!(
        has(
            &found,
            "a/user.kt::g",
            "a/model/T.kt::T",
            EdgeKind::References
        ),
        "{found:?}"
    );
}

#[test]
fn the_alias_right_hand_side_is_not_a_reference_to_the_alias_itself() {
    let found = fixture();
    assert!(
        !found
            .iter()
            .any(|(s, t, _)| s == "a/alias.kt::T" && t == "a/alias.kt::T"),
        "{found:?}"
    );
}

/// An alias that is the only declaration of its name is still the target.
#[test]
fn an_alias_alone_under_its_name_keeps_its_references() {
    let found = fixture();
    assert!(
        has(
            &found,
            "a/user.kt::h",
            "a/alias.kt::Handler",
            EdgeKind::References
        ),
        "{found:?}"
    );
}

/// The alias and its user in one file: the lexical walk finds the alias first
/// and must not stop there.
#[test]
fn a_same_file_alias_does_not_hide_the_class_from_its_user() {
    let found = edges(&[
        ("a/model/T.kt", CLASS),
        (
            "a/user.kt",
            "package a\n\ntypealias T = a.model.T\n\nfun g(t: T) = t\n",
        ),
    ]);
    assert!(
        has(
            &found,
            "a/user.kt::g",
            "a/model/T.kt::T",
            EdgeKind::References
        ),
        "{found:?}"
    );
    assert!(
        !found
            .iter()
            .any(|(s, t, _)| s == "a/user.kt::g" && t == "a/user.kt::T"),
        "{found:?}"
    );
}
