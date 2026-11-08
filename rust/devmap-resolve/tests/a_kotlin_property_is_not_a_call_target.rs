//! A bare call never binds to a Kotlin property, and a property never hides
//! the function a bare call names (card ft-4617a335).
//!
//! Kotlin properties became `Field` symbols. `val size` and a call `size()` are
//! different things: a property is reached through a receiver or a
//! `Type.NAME` reference. Two failure shapes follow from that:
//!
//! * the bare-name rungs would bind any unique namesake in the module to the
//!   property (the unique-global defect, on the property surface);
//! * the lexical walk stopped at the first scope holding the name, so a
//!   property of the enclosing class hid a same-named top-level function and
//!   the call lost the edge it had before properties were nodes.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction, SymbolKind};
use devmap_resolve::Resolver;

struct Resolved {
    extractions: Vec<Extraction>,
    calls: Vec<(String, String)>,
}

fn resolve(files: &[(&str, &str)]) -> Resolved {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let mut calls: Vec<(String, String)> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect();
    calls.sort();
    Resolved { extractions, calls }
}

impl Resolved {
    fn has_field(&self, qualified: &str) -> bool {
        self.extractions.iter().any(|ext| {
            ext.symbols.iter().any(|symbol| {
                symbol.qualified_name == qualified && symbol.kind == SymbolKind::Field
            })
        })
    }

    fn targets(&self) -> Vec<&str> {
        self.calls
            .iter()
            .map(|(_, target)| target.as_str())
            .collect()
    }
}

#[test]
fn a_bare_call_in_the_same_file_does_not_bind_to_a_member_property() {
    let resolved = resolve(&[("A.kt", "class Box { val size = 1 }\nfun use() { size() }\n")]);
    assert!(
        resolved.has_field("A.kt::Box.size"),
        "the property is a node"
    );
    assert!(
        !resolved
            .targets()
            .iter()
            .any(|target| target.ends_with("Box.size")),
        "{:?}",
        resolved.calls
    );
}

#[test]
fn a_bare_call_in_another_file_does_not_bind_to_a_top_level_property() {
    let resolved = resolve(&[
        ("a/Config.kt", "package a\nval limit = 1\n"),
        ("a/Use.kt", "package a\nfun use() { limit() }\n"),
    ]);
    assert!(
        resolved.has_field("a/Config.kt::limit"),
        "the property is a node"
    );
    assert!(
        !resolved
            .targets()
            .iter()
            .any(|target| target.ends_with("::limit")),
        "{:?}",
        resolved.calls
    );
}

/// The reviewer's fixture. `String` has no `invoke`, so `log("x")` is the
/// top-level function; on the lane tip before the fix the lexical walk stopped
/// at `Svc.log`, the call filter refused it, and the edge to `log` was lost.
#[test]
fn a_property_of_the_class_does_not_hide_the_top_level_function_a_call_names() {
    let resolved = resolve(&[(
        "A.kt",
        "fun log(msg: String) {}\nclass Svc {\n    val log = \"tag\"\n    fun run() { log(\"x\") }\n}\n",
    )]);
    assert!(
        resolved.has_field("A.kt::Svc.log"),
        "the property is a node"
    );
    assert!(
        resolved
            .calls
            .contains(&("A.kt::Svc.run".to_string(), "A.kt::log".to_string())),
        "{:?}",
        resolved.calls
    );
}

/// The dead-code consequence: a `private` helper that another Kotlin `log`
/// elsewhere makes ambiguous for the global rung. Only the lexical edge keeps
/// the helper from being a confident dead-code finding.
#[test]
fn a_private_helper_behind_a_same_named_property_keeps_its_caller() {
    let resolved = resolve(&[
        (
            "a/A.kt",
            "package a\nprivate fun log(msg: String) {}\nclass Svc {\n    val log = \"tag\"\n    fun run() { log(\"x\") }\n}\n",
        ),
        ("b/B.kt", "package b\nfun log(msg: String) {}\n"),
        ("c/C.kt", "package c\nfun log(msg: String) {}\n"),
    ]);
    assert!(
        resolved.has_field("a/A.kt::Svc.log"),
        "the property is a node"
    );
    assert!(
        resolved
            .calls
            .contains(&("a/A.kt::Svc.run".to_string(), "a/A.kt::log".to_string())),
        "{:?}",
        resolved.calls
    );
}

#[test]
fn a_function_of_the_same_name_is_still_the_target_when_no_property_is_in_the_way() {
    let resolved = resolve(&[(
        "A.kt",
        "fun size() = 2\nfun use() { size() }\nval size2 = 1\n",
    )]);
    assert_eq!(resolved.targets(), ["A.kt::size"]);
}
