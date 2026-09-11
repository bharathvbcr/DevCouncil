//! Reliability audit RA1/RA2: semantic oracles independent of the resolver's labels.
use std::collections::BTreeSet;

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, EdgeKind, Extraction};
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let files: Vec<Extraction> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    resolver.resolve_all(&files)
}

fn calls_from(result: &ResolutionResult, caller: &str) -> BTreeSet<String> {
    result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls && e.source_symbol == caller)
        .map(|e| e.target_symbol.clone())
        .collect()
}

#[test]
fn notebook_cross_cell_calls_resolve_after_independent_cell_parsing() {
    let notebook = r#"{"metadata":{"language_info":{"name":"python"}}, "cells":[
        {"cell_type":"code", "source":"def helper():\n    return 1\n"},
        {"cell_type":"code", "source":"def run():\n    return helper()\n"}
    ]}"#;
    let result = resolve(&[("calls.ipynb", notebook)]);
    assert!(calls_from(&result, "calls.ipynb::run").contains("calls.ipynb::helper"));
}

#[test]
fn ra1_python_parameter_shadows_the_module_declaration() {
    let r = resolve(&[(
        "a.py",
        "def target(): return 1\ndef run(target): return target()\n",
    )]);
    assert!(
        !calls_from(&r, "a.py::run").contains("a.py::target"),
        "{:?}",
        r.edges
    );
    assert!(
        r.unresolved
            .iter()
            .any(|u| u.source_symbol == "a.py::run" && u.callee_name == "target"),
        "the unbound call must remain accounted for"
    );
}

#[test]
fn ra1_typed_and_default_parameters_shadow_just_like_plain_parameters() {
    for signature in [
        "target: object",
        "target=None",
        "target: object=None",
        "*target",
        "**target",
    ] {
        let source = format!("def target(): return 1\ndef run({signature}): return target()\n");
        let r = resolve(&[("a.py", &source)]);
        assert!(
            !calls_from(&r, "a.py::run").contains("a.py::target"),
            "{signature}: {:?}",
            r.edges
        );
    }
}

#[test]
fn ra1_a_parameter_shadows_an_import_before_any_resolution_rung() {
    let r = resolve(&[
        ("lib.py", "def target(): return 1\n"),
        (
            "a.py",
            "from lib import target\ndef run(target): return target()\n",
        ),
    ]);
    assert!(
        !calls_from(&r, "a.py::run").contains("lib.py::target"),
        "{:?}",
        r.edges
    );
}

#[test]
fn ra1_shadowing_is_not_python_specific() {
    for (path, source) in [
        ("a.js", "function target() { return 1; } function run(target) { return target(); }"),
        ("a.ts", "function target() { return 1; } function run(target: () => number) { return target(); }"),
        ("a.go", "package a\nfunc target() int { return 1 }; func run(target func() int) int { return target() }"),
        ("a.rs", "fn target() -> i32 { 1 } fn run(target: fn() -> i32) -> i32 { target() }"),
    ] {
        let r = resolve(&[(path, source)]);
        assert!(!calls_from(&r, &format!("{path}::run")).contains(&format!("{path}::target")), "{path}: {:?}", r.edges);
    }
}

#[test]
fn ra1_local_assignment_cannot_resolve_to_an_unrelated_global() {
    let r = resolve(&[(
        "a.py",
        "def target(): return 1\ndef run(callback):\n target = callback\n return target()\n",
    )]);
    assert!(
        !calls_from(&r, "a.py::run").contains("a.py::target"),
        "{:?}",
        r.edges
    );
}

/// A nested `const walk = () => …; walk()` is a call of that arrow.
///
/// RA1 used to treat every local callee as an unknown value, so the extracted
/// nested function had no inbound edge and became inferred-dead, while the
/// `walk()` in the same function was the only caller it has. The parameter
/// case above must stay unresolved: a parameter is not an extracted nested
/// function.
#[test]
fn a_nested_const_arrow_call_resolves_to_that_arrow() {
    let r = resolve(&[(
        "a.js",
        "function collapseAll() {\n  const walk = (folders) => { walk(folders); };\n  walk([]);\n}\nfunction walk() {}\n",
    )]);
    let from_collapse = calls_from(&r, "a.js::collapseAll");
    assert!(
        from_collapse.contains("a.js::collapseAll.walk"),
        "the outer `walk([])` is the nested arrow, not the module function: {from_collapse:?}"
    );
    assert!(
        !from_collapse.contains("a.js::walk"),
        "the module-level `walk` must not absorb the nested call: {:?}",
        r.edges
    );
    let from_nested = calls_from(&r, "a.js::collapseAll.walk");
    assert!(
        from_nested.contains("a.js::collapseAll.walk"),
        "the recursive `walk(folders)` is the same arrow: {from_nested:?}"
    );
}

#[test]
fn ra1_receiver_members_cannot_bind_a_bare_import() {
    let r = resolve(&[
        ("lib.py", "def target(): return 1\n"),
        (
            "a.py",
            "from lib import target\ndef run(obj): return obj.target()\n",
        ),
    ]);
    assert!(
        !r.edges.iter().any(|e| e.source_symbol == "a.py::run"
            && e.target_symbol == "lib.py::target"
            && e.confidence == Confidence::DETERMINISTIC),
        "{:?}",
        r.edges
    );
}

#[test]
fn ra1_receiver_types_do_not_leak_from_a_sibling_function() {
    let r = resolve(&[("a.py", "class Service:\n def ping(self): return 1\ndef known():\n x = Service()\n return x.ping()\ndef unknown(x): return x.ping()\n")]);
    assert!(
        !r.edges.iter().any(|e| e.source_symbol == "a.py::unknown"
            && e.target_symbol == "a.py::Service.ping"
            && e.confidence == Confidence::DETERMINISTIC),
        "{:?}",
        r.edges
    );
    assert!(r.edges.iter().any(|e| e.source_symbol == "a.py::known"
        && e.target_symbol == "a.py::Service.ping"
        && e.confidence == Confidence::DETERMINISTIC));
}

#[test]
fn ra1_shadowing_does_not_erase_an_unshadowed_call_in_another_scope() {
    let r = resolve(&[("a.py", "def target(): return 1\ndef shadow(target): return target()\ndef control(): return target()\n")]);
    assert!(calls_from(&r, "a.py::control").contains("a.py::target"));
}

#[test]
fn ra1_a_default_expression_executes_outside_the_parameter_scope() {
    let r = resolve(&[(
        "a.py",
        "def target(): return 1\ndef run(target=target()): return target\n",
    )]);
    assert!(
        r.edges
            .iter()
            .any(|e| e.edge_kind == EdgeKind::Calls && e.target_symbol == "a.py::target"),
        "{:?}",
        r.edges
    );
}

#[test]
fn ra2_both_same_named_methods_remain_distinct_ambiguous_candidates() {
    let r = resolve(&[("a.py", "class Alpha:\n def ping(self): return 1\nclass Beta:\n def ping(self): return 2\ndef run(x): return x.ping()\n")]);
    assert_eq!(
        calls_from(&r, "a.py::run"),
        BTreeSet::from(["a.py::Alpha.ping".into(), "a.py::Beta.ping".into()])
    );
    for e in r
        .edges
        .iter()
        .filter(|e| e.source_symbol == "a.py::run" && e.edge_kind == EdgeKind::Calls)
    {
        assert_eq!(e.confidence, Confidence::SPECULATIVE);
        if let Some(Resolution::AmbiguousGlobal { candidates, .. }) = e.resolution.as_deref() {
            assert_eq!(
                candidates.iter().collect::<BTreeSet<_>>().len(),
                2,
                "candidate identities must be distinct"
            );
        } else {
            panic!("missing ambiguity evidence: {e:?}");
        }
    }
}

#[test]
fn ra2_ambiguity_caps_count_distinct_identities_in_one_file() {
    let mut source = String::new();
    for i in 0..40 {
        source.push_str(&format!("class C{i:02}:\n def ping(self): return {i}\n"));
    }
    source.push_str("def run(x): return x.ping()\n");
    let r = resolve(&[("a.py", &source)]);
    assert_eq!(
        calls_from(&r, "a.py::run").len(),
        devmap_resolve::model::AMBIGUOUS_FANOUT_CAP
    );
    for e in r
        .edges
        .iter()
        .filter(|e| e.source_symbol == "a.py::run" && e.edge_kind == EdgeKind::Calls)
    {
        assert!(
            e.details.as_deref().is_some_and(|d| d.contains("40")),
            "{e:?}"
        );
        let Some(Resolution::AmbiguousGlobal { candidates, .. }) = e.resolution.as_deref() else {
            panic!("{e:?}")
        };
        assert_eq!(candidates.iter().collect::<BTreeSet<_>>().len(), 40);
    }
}

#[test]
fn ra2_nested_declarations_resolve_in_their_lexical_scope() {
    let r = resolve(&[("a.py", "def first():\n def helper(): return 1\n return helper()\ndef second():\n def helper(): return 2\n return helper()\n")]);
    assert_eq!(
        calls_from(&r, "a.py::first"),
        BTreeSet::from(["a.py::first.helper".into()])
    );
    assert_eq!(
        calls_from(&r, "a.py::second"),
        BTreeSet::from(["a.py::second.helper".into()])
    );
}

#[test]
fn ra2_java_implicit_receivers_keep_their_real_method() {
    let source = "class A { void f() { g(); } void g() {} }";
    let ext = extract_file("A.java", source);
    let r = resolve(&[("A.java", source)]);
    assert!(
        calls_from(&r, "A.java::A.f").contains("A.java::A.g"),
        "symbols={:?}, calls={:?}, locals={:?}, result={r:?}",
        ext.symbols,
        ext.calls,
        ext.scope_locals
    );
}

#[test]
fn ra1_a_callback_parameter_does_not_shadow_the_outer_call() {
    let r = resolve(&[("a.js", "function target() { return 1; } function run(items) { items.map(target => target()); return target(); }")]);
    assert!(
        calls_from(&r, "a.js::run").contains("a.js::target"),
        "{r:?}"
    );
    let calls: Vec<_> = r
        .edges
        .iter()
        .filter(|e| {
            e.edge_kind == EdgeKind::Calls
                && e.source_symbol == "a.js::run"
                && e.target_symbol == "a.js::target"
        })
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "only the outer call reaches the declaration: {calls:?}"
    );
}

#[test]
fn ra1_a_nested_function_captures_the_outer_parameter() {
    let r = resolve(&[("a.py", "def target(): return 1\ndef outer(target):\n def inner(): return target()\n return inner()\n")]);
    assert!(
        !calls_from(&r, "a.py::outer.inner").contains("a.py::target"),
        "{r:?}"
    );
}

#[test]
fn ra1_using_a_parameter_as_a_value_does_not_reference_a_global() {
    let r = resolve(&[(
        "a.py",
        "def target(): return 1\ndef run(target): return target\n",
    )]);
    assert!(
        !r.edges.iter().any(|e| e.source_symbol == "a.py::run"
            && e.target_symbol == "a.py::target"
            && matches!(e.edge_kind, EdgeKind::References | EdgeKind::Calls)),
        "{r:?}"
    );
}

#[test]
fn ra2_nested_functions_passed_as_values_keep_their_lexical_identity() {
    let r = resolve(&[("a.py", "def first():\n def helper(): return 1\n register(helper)\ndef second():\n def helper(): return 2\n register(helper)\n")]);
    for scope in ["first", "second"] {
        assert!(
            r.edges.iter().any(|e| e.edge_kind == EdgeKind::References
                && e.source_symbol == format!("a.py::{scope}")
                && e.target_symbol == format!("a.py::{scope}.helper")),
            "{scope}: {r:?}"
        );
    }
}

#[test]
fn ra2_an_import_cannot_export_a_class_member_by_its_bare_name() {
    let r = resolve(&[
        ("lib.py", "class A:\n def ping(self): return 1\n"),
        ("a.py", "from lib import ping\ndef run(): return ping()\n"),
    ]);
    assert!(
        calls_from(&r, "a.py::run").is_empty(),
        "the module declares no exported ping: {r:?}"
    );
}

#[test]
fn ra1_pytest_fixture_requests_survive_without_treating_parameters_as_calls() {
    let source = "import pytest\n@pytest.fixture\ndef resource(): return 1\ndef test_uses(resource): return resource\ndef ordinary(resource): return resource()\n";
    let r = resolve(&[("test_a.py", source)]);
    assert!(
        r.edges.iter().any(|e| e.edge_kind == EdgeKind::References
            && e.source_symbol == "test_a.py::test_uses"
            && e.target_symbol == "test_a.py::resource"),
        "the signature requests a declared fixture: {r:?}"
    );
    assert!(calls_from(&r, "test_a.py::ordinary").is_empty());
    assert!(calls_from(&r, "test_a.py::test_uses").is_empty());
}

#[test]
fn ra1_captured_module_parameters_do_not_rebind_to_imports() {
    let r = resolve(&[
        ("lib.py", "def ping(): return 1\n"),
        (
            "a.py",
            "import lib\ndef outer(lib):\n def inner(): return lib.ping()\n return inner()\n",
        ),
    ]);
    assert!(
        !r.edges
            .iter()
            .any(|e| e.source_symbol == "a.py::outer.inner"
                && e.target_symbol == "lib.py::ping"
                && e.confidence == Confidence::DETERMINISTIC),
        "{r:?}"
    );
}

#[test]
fn ra1_anonymous_receiver_parameters_do_not_inherit_the_outer_type() {
    let r = resolve(&[("a.js", "class Service { ping() {} } function run(items) { const x = new Service(); items.map(x => x.ping()); }")]);
    assert!(
        !r.edges.iter().any(|e| e.source_symbol == "a.js::run"
            && e.target_symbol == "a.js::Service.ping"
            && e.confidence == Confidence::DETERMINISTIC),
        "{r:?}"
    );
}

#[test]
fn ra1_anonymous_parameters_do_not_erase_the_outer_import_binding() {
    let r = resolve(&[
        ("lib.js", "export function ping() {}"),
        ("a.js", "import * as lib from './lib'; function run(items) { items.map(lib => lib.ping()); return lib.ping(); }"),
    ]);
    assert!(
        r.edges.iter().any(|e| e.source_symbol == "a.js::run"
            && e.target_symbol == "lib.js::ping"
            && e.edge_kind == EdgeKind::Calls
            && e.confidence == Confidence::DETERMINISTIC),
        "{r:?}"
    );
}
