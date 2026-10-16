//! A typed receiver whose type does not declare the member is
//! `unresolved_member`, not a bare-name guess and not `uninferred_receiver`.

use devmap_extract::{extract_file, Extraction};
use devmap_resolve::model::ResolutionResult;
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

fn classes_for(result: &ResolutionResult, callee: &str) -> Vec<String> {
    result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == callee)
        .map(|row| row.class.label().to_string())
        .collect()
}

fn assert_unresolved_member(source_path: &str, source: &str, callee: &str) {
    let result = resolve(&[(source_path, source)]);
    let classes = classes_for(&result, callee);
    assert_eq!(
        classes,
        vec!["unresolved_member".to_string()],
        "{source_path} {callee}: {:?}",
        result
            .unresolved
            .iter()
            .map(|row| (&row.source_symbol, &row.callee_name, row.class.label()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_go_typed_receiver_missing_the_member_is_unresolved_member() {
    assert_unresolved_member(
        "use.go",
        "package p\n\ntype Reader struct{}\n\nfunc (r *Reader) Read() string { return \"\" }\n\nfunc use(r *Reader) {\n\tr.Missing()\n}\n",
        "Missing",
    );
}

#[test]
fn a_swift_typed_receiver_missing_the_member_is_unresolved_member() {
    assert_unresolved_member(
        "use.swift",
        "struct Box { func size() -> Int { 1 } }\nfunc use(b: Box) { _ = b.missing() }\n",
        "missing",
    );
}

#[test]
fn a_kotlin_typed_receiver_missing_the_member_is_unresolved_member() {
    assert_unresolved_member(
        "use.kt",
        "class Box { fun size(): Int = 1 }\nfun use(b: Box) { b.missing() }\n",
        "missing",
    );
}

#[test]
fn a_rust_typed_receiver_missing_the_member_is_unresolved_member() {
    assert_unresolved_member(
        "use.rs",
        "struct Box;\nimpl Box { fn size(&self) -> i32 { 1 } }\nfn use_box(b: &Box) { b.missing(); }\n",
        "missing",
    );
}

#[test]
fn a_typescript_typed_receiver_missing_the_member_is_unresolved_member() {
    assert_unresolved_member(
        "use.ts",
        "class Box { size(): number { return 1 } }\nfunction use(b: Box) { return b.missing() }\n",
        "missing",
    );
}

#[test]
fn a_python_typed_receiver_missing_the_member_is_unresolved_member() {
    assert_unresolved_member(
        "use.py",
        "class Box:\n    def size(self):\n        return 1\n\ndef use(b: Box):\n    return b.missing()\n",
        "missing",
    );
}

#[test]
fn an_untyped_receiver_is_not_relabelled_unresolved_member() {
    let result = resolve(&[("use.ts", "function use(b) { return b.missing() }\n")]);
    let classes = classes_for(&result, "missing");
    assert!(
        !classes.iter().any(|class| class == "unresolved_member"),
        "no indexed type was named: {classes:?}"
    );
}
