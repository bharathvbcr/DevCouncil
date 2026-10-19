//! A Go variable's package qualifier belongs to the function that wrote it.
//!
//! `type_qualifier_of` fell back to a file-wide `{file}:{typed}@mod` slot when
//! the reference's own scope held no qualifier. Every `Type` reference binds
//! that file-wide slot too, so one function's `params json.RawMessage` became
//! the qualifier of a same-named `var params t` in another function: the bare
//! `t` was routed down the qualified-type path under `json`, never bound, and
//! `t` was reported dead at 0.9.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{ResolutionResult, ResolvedEdge, UnresolvedClass};
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

fn edges_to<'a>(result: &'a ResolutionResult, source: &str, target: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|e| {
            e.source_symbol.rsplit("::").next() == Some(source)
                && e.target_symbol.rsplit(['.', ':']).next() == Some(target)
        })
        .collect()
}

fn all_edges(result: &ResolutionResult) -> Vec<(&str, &str)> {
    result
        .edges
        .iter()
        .map(|e| (e.source_symbol.as_str(), e.target_symbol.as_str()))
        .collect()
}

fn raw_message_classes(result: &ResolutionResult) -> Vec<UnresolvedClass> {
    result
        .unresolved
        .iter()
        .filter(|r| r.callee_name == "RawMessage")
        .map(|r| r.class.clone())
        .collect()
}

const LEAK: &str = "package p\n\nimport \"encoding/json\"\n\ntype t struct{}\n\n\
func budget(params json.RawMessage) int { return 0 }\n\n\
func call() { var params t; _ = params }\n";

#[test]
fn a_qualified_param_in_one_function_does_not_qualify_a_bare_local_in_another() {
    let result = resolve(&[("p/p.go", LEAK)]);
    assert!(
        !edges_to(&result, "call", "t").is_empty(),
        "`var params t` in `call` is a bare type; got edges {:?}",
        all_edges(&result)
    );
}

#[test]
fn the_order_of_the_two_functions_does_not_matter() {
    let src = "package p\n\nimport \"encoding/json\"\n\ntype t struct{}\n\n\
func call() { var params t; _ = params }\n\n\
func budget(params json.RawMessage) int { return 0 }\n";
    let result = resolve(&[("p/p.go", src)]);
    assert!(
        !edges_to(&result, "call", "t").is_empty(),
        "{:?}",
        all_edges(&result)
    );
}

#[test]
fn the_qualified_binding_still_classifies_as_external_in_its_own_function() {
    let result = resolve(&[("p/p.go", LEAK)]);
    let classes = raw_message_classes(&result);
    assert!(
        !classes.is_empty()
            && classes
                .iter()
                .all(|c| matches!(c, UnresolvedClass::External { .. })),
        "got {classes:?}"
    );
}

#[test]
fn a_package_level_qualified_var_feeds_no_qualifier_slot_and_is_unchanged() {
    let src = "package p\n\nimport \"encoding/json\"\n\nvar x json.RawMessage\n\n\
func use() int { return len(x) }\n";
    // The Go extractor emits no `Type` reference for a package-level var, so
    // there is no file-wide qualifier slot for it to feed; users of the var are
    // untouched by the scope rule.
    let ex = extract_file("p/p.go", src);
    assert!(!ex.references.iter().any(|r| r.name == "RawMessage"));
    let result = resolve(&[("p/p.go", src)]);
    assert!(
        edges_to(&result, "use", "x").is_empty() && raw_message_classes(&result).is_empty(),
        "{:?}",
        all_edges(&result)
    );
}

#[test]
fn a_package_level_qualified_var_does_not_qualify_a_local_bare_type_of_the_same_name() {
    let src = "package p\n\nimport \"encoding/json\"\n\ntype t struct{}\n\n\
var x json.RawMessage\n\nfunc call() { var x t; _ = x }\n";
    let result = resolve(&[("p/p.go", src)]);
    assert!(
        !edges_to(&result, "call", "t").is_empty(),
        "{:?}",
        all_edges(&result)
    );
}
