//! A `Calls` edge names something that runs.
//!
//! Measured against `go/types` and rust-analyzer on this repository, 1,304 of
//! 8,634 Go `Calls` pairs and 53 Rust ones targeted a `Struct`: composite
//! literals (`args{}`), tuple-struct construction (`Error(..)`) and conversions.
//! The extractor records them as call sites on purpose, so the resolver must
//! decide the edge kind: a type is *used*, not called, and is emitted as the
//! existing `References` kind (detail `Constructor`). The type keeps an inbound
//! edge, so liveness and impact still see it.
//!
//! The reclassification is deliberately asymmetric across languages. In Go
//! and Rust a type is not callable: `args{}` and `Error(..)` build a value
//! without running any code the type declares, so the edge is a reference. In
//! Python a class *is* callable: `Engine()` runs `Engine.__new__` and
//! `Engine.__init__`, so a class call keeps its `Calls` edge
//! (`a_python_class_call_keeps_its_calls_edge` below). A Calls-to-References
//! change on the same (caller, type) pair is its own corpus-diff bucket, not a
//! lost edge (Fable ruling 3, 2026-10-29).

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn rows(result: &ResolutionResult, kind: EdgeKind, from: &str) -> Vec<String> {
    let mut rows: Vec<String> = result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == kind && edge.source_symbol.ends_with(from))
        .map(|edge| {
            format!(
                "{} {}",
                edge.target_symbol,
                edge.details.as_deref().unwrap_or("")
            )
        })
        .collect();
    rows.sort();
    rows
}

const GO_PKG: &str = "package p\n\
type args struct{ a int }\n\
type Level int\n\
func helper() int { return 1 }\n\
func parse() (args, error) {\n\
\tout := args{}\n\
\t_ = Level(3)\n\
\t_ = helper()\n\
\treturn out, nil\n\
}\n";

#[test]
fn a_go_composite_literal_and_conversion_are_references_not_calls() {
    let result = resolve(&[("p/a.go", GO_PKG)]);
    assert_eq!(
        rows(&result, EdgeKind::Calls, "::parse"),
        ["p/a.go::helper "],
        "only the function call is a Calls edge"
    );
    let uses = rows(&result, EdgeKind::References, "::parse");
    for ty in ["p/a.go::args Constructor", "p/a.go::Level Constructor"] {
        assert!(
            uses.iter().any(|row| row == ty),
            "{ty:?} must be kept as a References edge, got {uses:?}"
        );
    }
}

/// A type declared inside a function is a symbol named `fn.type`; the check
/// must read it by its bare name like every other, or the 14 Go and 1 Rust
/// function-local types of this repository stay `Calls` targets.
#[test]
fn a_function_local_type_is_a_reference_not_a_call() {
    let go = "package p\nfunc list() {\n\ttype row struct{ n int }\n\t_ = row{n: 1}\n}\n";
    let result = resolve(&[("p/a.go", go)]);
    assert!(
        rows(&result, EdgeKind::Calls, "::list")
            .iter()
            .all(|row| !row.contains("row")),
        "{:?}",
        rows(&result, EdgeKind::Calls, "::list")
    );
    assert!(rows(&result, EdgeKind::References, "::list")
        .iter()
        .any(|row| row.contains("row") && row.ends_with("Constructor")));

    let rust = "fn run() -> u8 { struct Exit(u8); Exit(3).0 }\n";
    let result = resolve(&[("src/lib.rs", rust)]);
    assert!(
        rows(&result, EdgeKind::Calls, "::run")
            .iter()
            .all(|row| !row.contains("Exit")),
        "{:?}",
        rows(&result, EdgeKind::Calls, "::run")
    );
}

#[test]
fn a_go_struct_in_another_file_of_the_package_is_still_a_reference() {
    let result = resolve(&[
        ("p/a.go", "package p\ntype Options struct{ n int }\n"),
        (
            "p/b.go",
            "package p\nfunc run() Options { return Options{n: 1} }\n",
        ),
    ]);
    assert!(rows(&result, EdgeKind::Calls, "::run").is_empty());
    assert!(
        rows(&result, EdgeKind::References, "::run")
            .iter()
            .any(|row| row == "p/a.go::Options Constructor"),
        "the literal must survive as a Constructor reference to the struct"
    );
}

#[test]
fn a_rust_tuple_struct_construction_is_a_reference_not_a_call() {
    let src = "pub struct Error(pub String);\n\
fn make() -> Error { Error(String::new()) }\n\
fn plain() {}\n\
fn caller() { plain(); let _ = make(); }\n";
    let result = resolve(&[("src/lib.rs", src)]);
    assert!(
        !rows(&result, EdgeKind::Calls, "::make")
            .iter()
            .any(|row| row.starts_with("src/lib.rs::Error")),
        "Error(..) must not be a Calls edge: {:?}",
        rows(&result, EdgeKind::Calls, "::make")
    );
    assert!(
        rows(&result, EdgeKind::References, "::make")
            .iter()
            .any(|row| row.starts_with("src/lib.rs::Error")),
        "Error(..) must stay a use of the type"
    );
    let callee_rows = rows(&result, EdgeKind::Calls, "::caller");
    assert!(callee_rows
        .iter()
        .any(|row| row.starts_with("src/lib.rs::plain")));
    assert!(callee_rows
        .iter()
        .any(|row| row.starts_with("src/lib.rs::make")));
}

/// The asymmetry the module doc states: a Python class is callable, so
/// `Engine()` runs code (`__new__`, `__init__`) and stays a `Calls` edge,
/// while a Go composite literal or a Rust tuple-struct constructor builds a
/// value without running anything the type declares and becomes
/// `References` (detail `Constructor`). The resolver reclassifies only Go and
/// Rust type targets.
#[test]
fn a_python_class_call_keeps_its_calls_edge() {
    let result = resolve(&[(
        "a.py",
        "class Engine:\n    pass\n\ndef build():\n    return Engine()\n",
    )]);
    assert!(
        rows(&result, EdgeKind::Calls, "::build")
            .iter()
            .any(|row| row.starts_with("a.py::Engine")),
        "a class call runs its constructor; only Go and Rust types are reclassified"
    );
}

#[test]
fn no_calls_edge_in_a_go_or_rust_graph_targets_a_type_declaration() {
    let result = resolve(&[
        ("p/a.go", GO_PKG),
        (
            "src/lib.rs",
            "pub struct S { pub n: i32 }\nenum E { A(i32) }\nfn f() { let _ = S { n: 1 }; let _ = E::A(1); }\n",
        ),
    ]);
    let types = [
        "p/a.go::args",
        "p/a.go::Level",
        "src/lib.rs::S",
        "src/lib.rs::E",
    ];
    for edge in result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls)
    {
        assert!(
            !types.contains(&edge.target_symbol.as_str()),
            "Calls edge to a type: {} -> {}",
            edge.source_symbol,
            edge.target_symbol
        );
    }
}

/// Rust gives a type and a function of one name separate namespaces, so
/// `struct Foo {..}` and `fn Foo()` can coexist. The index cannot tell which
/// the call names, so it must not reclassify: a call may be real.
#[test]
fn a_name_shared_by_a_type_and_a_function_keeps_its_call() {
    let src = "struct Foo { n: i32 }\nfn Foo() -> i32 { 1 }\nfn caller() -> i32 { Foo() }\n";
    let result = resolve(&[("src/lib.rs", src)]);
    let calls = rows(&result, EdgeKind::Calls, "::caller");
    let uses = rows(&result, EdgeKind::References, "::caller");
    assert!(
        calls.iter().any(|row| row.starts_with("src/lib.rs::Foo")) || uses.is_empty(),
        "an ambiguous type/function name must not be turned into a use: calls={calls:?} uses={uses:?}"
    );
}
