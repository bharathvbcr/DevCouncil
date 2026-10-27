//! Resolver-level probes for the Swift residue lane (extract 9661bb6a). Each test
//! states the CORRECT behaviour; a failing test is a BREAK. The extractor's
//! wrong shape or type is only a finding when an edge follows from it.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::Resolver;

fn calls(files: &[(&str, &str)], from: &str) -> Vec<String> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let mut found: Vec<String> = resolver
        .resolve_all(&extractions)
        .expect("resolution")
        .edges
        .into_iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .filter(|edge| {
            edge.source_symbol.ends_with(&format!("::{from}"))
                || edge.source_symbol.ends_with(&format!(".{from}"))
        })
        .map(|edge| edge.target_symbol)
        .collect();
    found.sort();
    found
}

fn reaches(found: &[String], suffix: &str) -> bool {
    found.iter().any(|target| target.ends_with(suffix))
}

const STORE: &str = "struct Snap { func go() {} }\n\
     struct Store {\n    func snapshot(_ p: Int) -> Snap { Snap() }\n\
     \x20   func map(_ p: Int) -> Snap { Snap() }\n}\n";

/// `Mod.Store` is another module's `Store`; this corpus's `Store` is not it.
#[test]
fn a_module_qualified_parameter_reaches_no_edge_through_the_corpus_store() {
    let found = calls(
        &[
            ("A.swift", STORE),
            (
                "B.swift",
                "func t(_ store: Mod.Store) { let s = store.snapshot(1); s.go() }",
            ),
        ],
        "t",
    );
    assert!(
        !reaches(&found, "Snap.go"),
        "Mod.Store.snapshot is not Store.snapshot: {found:?}"
    );
}

/// `store.map(1)` on an `Optional<Store>` is `Optional.map`.
#[test]
fn an_optional_parameter_map_reaches_no_edge_through_the_wrapped_type() {
    let found = calls(
        &[
            ("A.swift", STORE),
            (
                "B.swift",
                "func t(_ store: Store?) { let s = store.map(1); s.go() }",
            ),
        ],
        "t",
    );
    assert!(
        !reaches(&found, "Snap.go"),
        "Optional.map is not Store.map: {found:?}"
    );
}

/// `case .failure(let store)` hides the parameter `store`.
#[test]
fn a_case_let_that_hides_a_parameter_reaches_no_edge_through_the_parameter_type() {
    let found = calls(
        &[
            ("A.swift", STORE),
            (
                "B.swift",
                "func t(_ store: Store, _ r: Res) {\n    switch r {\n\
                 \x20   case .failure(let store):\n        let s = store.snapshot(1)\n        s.go()\n\
                 \x20   default: break\n    }\n}",
            ),
        ],
        "t",
    );
    assert!(
        !reaches(&found, "Snap.go"),
        "the case payload is not the parameter: {found:?}"
    );
}

/// `H` inherits `makePair()` from `Base` (another file); inside `H` the member
/// is found before the free function in `A.swift`.
#[test]
fn a_subclass_destructure_does_not_call_through_the_file_level_function() {
    let found = calls(
        &[
            (
                "A.swift",
                "struct Store { func go() {} }\nstruct Svc {}\n\
                 func makePair() -> (Store, Svc) { (Store(), Svc()) }\n\
                 class H: Base { func t() { let (a, b) = makePair(); a.go() } }",
            ),
            (
                "Base.swift",
                "struct Other { func go() {} }\nstruct Svc2 {}\n\
                 class Base { func makePair() -> (Other, Svc2) { (Other(), Svc2()) } }",
            ),
        ],
        "t",
    );
    assert!(
        !reaches(&found, "Store.go"),
        "Base.makePair returns an Other: {found:?}"
    );
}

/// A destructure in an `if` block binds `a` for the whole function; the member
/// `a` used after the block is an `Other`, not the destructured `Store`.
#[test]
fn a_block_scoped_destructure_does_not_capture_a_member_used_after_the_block() {
    let found = calls(
        &[(
            "A.swift",
            "struct Store { func run() {} }\nstruct Svc {}\nstruct Other { func run() {} }\n\
             func makePair() -> (Store, Svc) { (Store(), Svc()) }\n\
             final class H {\n    var a: Other = Other()\n\
             \x20   func go(_ flag: Bool) {\n        if flag { let (a, b) = makePair(); print(b) }\n\
             \x20       a.run()\n    }\n}",
        )],
        "go",
    );
    assert!(
        !reaches(&found, "Store.run"),
        "H.a is an Other; no Store is in scope at a.run(): {found:?}"
    );
}

/// A cross-file `typealias Store = Real`: the element `Store` is a `Real`.
/// Held: the resolver refuses or follows the alias; it must not pick `Other`.
#[test]
fn a_tuple_element_aliased_in_another_file_is_not_a_different_type() {
    let found = calls(
        &[
            (
                "A.swift",
                "typealias Store = Real\nstruct Real { func keep() {} }\nstruct Other { func keep() {} }",
            ),
            (
                "B.swift",
                "struct Svc {}\nfunc makePair() -> (Store, Svc) { fatalError() }\n\
                 func t() { let (a, b) = makePair(); a.keep() }",
            ),
        ],
        "t",
    );
    assert!(!reaches(&found, "Other.keep"), "{found:?}");
}
