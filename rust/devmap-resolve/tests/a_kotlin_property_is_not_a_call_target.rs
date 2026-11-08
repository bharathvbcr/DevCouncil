//! A bare call never binds to a Kotlin property (card ft-4617a335).
//!
//! Kotlin properties became `Field` symbols. `val size` and a call `size()` are
//! different things: a property is reached through a receiver or a
//! `Type.NAME` reference, and the bare-name rungs would otherwise bind any unique
//! namesake in the module (the unique-global defect, on the property surface).

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

fn call_targets(files: &[(&str, &str)]) -> Vec<String> {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let mut targets: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| edge.target_symbol.clone())
        .collect();
    targets.sort();
    targets
}

#[test]
fn a_bare_call_in_the_same_file_does_not_bind_to_a_member_property() {
    let targets = call_targets(&[(
        "A.kt",
        "class Box { val size = 1 }\nfun use() { size() }\n",
    )]);
    assert!(
        !targets.iter().any(|target| target.ends_with("Box.size")),
        "{targets:?}"
    );
}

#[test]
fn a_bare_call_in_another_file_does_not_bind_to_a_top_level_property() {
    let targets = call_targets(&[
        ("a/Config.kt", "package a\nval limit = 1\n"),
        ("a/Use.kt", "package a\nfun use() { limit() }\n"),
    ]);
    assert!(
        !targets.iter().any(|target| target.ends_with("::limit")),
        "{targets:?}"
    );
}

#[test]
fn a_function_of_the_same_name_is_still_the_target() {
    let targets = call_targets(&[(
        "A.kt",
        "fun size() = 2\nfun use() { size() }\nval size2 = 1\n",
    )]);
    assert_eq!(targets, ["A.kt::size"]);
}
