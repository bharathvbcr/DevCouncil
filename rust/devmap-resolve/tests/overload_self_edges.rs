//! Same-name overloads (ft-eda9b3a6, resolver half; fixture rows `kg.OV1` /
//! `kg.OV2`).
//!
//! The extractor mints one id per qualified name, so `add(Int)` and
//! `add(String)` are two declarations of `Calc.add`. Two resolver defects
//! followed from that, and both are fixed here for every language that
//! overloads by signature:
//!
//! * a call from one overload to its sibling bound to the caller's own id, a
//!   `Calc.add -> Calc.add` self-edge, because nothing recorded about the call
//!   (no arity) tells it from recursion. It is now an abstention that names
//!   its reason; a function with ONE declaration still self-edges on real
//!   recursion.
//! * `c.add(1)` on a `Calc`-typed receiver found two identical hits, and the
//!   receiver rung refuses several hits, so the call was never bound.
//!
//! Distinct ids per overload, and "impact on one overload does not list the
//! other's callers", need extractor facts (extract-v98) and are out of scope.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(path: &str, source: &str) -> ResolutionResult {
    let extraction = extract_file(path, source);
    let mut resolver = Resolver::new();
    let extractions = vec![extraction];
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn call_edges(result: &ResolutionResult) -> Vec<(String, String)> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls)
        .map(|edge| (edge.source_symbol.clone(), edge.target_symbol.clone()))
        .collect()
}

/// The abstention reason of every unresolved call named `callee`.
fn abstentions(result: &ResolutionResult, callee: &str) -> Vec<String> {
    result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == callee)
        .filter_map(|row| match &row.resolution {
            Resolution::Unresolved { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn assert_overloads_never_self_edge(path: &str, source: &str, owner: &str, user: &str) {
    let result = resolve(path, source);
    let calls = call_edges(&result);
    let target = format!("{path}::{owner}.add");
    assert!(
        calls.iter().all(|(from, to)| from != to),
        "{path}: an overload calling its sibling is not a self-edge: {calls:?}"
    );
    assert!(
        calls.contains(&(format!("{path}::{user}"), target)),
        "{path}: c.add(..) on a {owner}-typed receiver reaches {owner}.add: {calls:?}"
    );
    let reasons = abstentions(&result, "add");
    assert!(
        reasons.iter().any(|reason| reason.contains("overloads share one id")),
        "{path}: the dropped sibling call is a labelled abstention, not silence: {reasons:?}"
    );
}

#[test]
fn kotlin_overloads_never_self_edge_and_the_receiver_call_binds() {
    assert_overloads_never_self_edge(
        "gaps/Overloads.kt",
        "package gaps\n\nclass Calc {\n    fun add(a: Int): Int = a\n\n    fun add(a: String): Int = add(a.length)\n}\n\nfun useCalc(c: Calc): Int = c.add(1) + c.add(\"two\")\n",
        "Calc",
        "useCalc",
    );
}

const JAVA_OVERLOADS: &str = "class Calc {\n  int add(int a) { return a; }\n  int add(String a) { return add(a.length()); }\n}\nclass U { int use(Calc c) { return c.add(1) + c.add(\"x\"); } }\n";

#[test]
fn java_overloads_never_self_edge() {
    let result = resolve("Calc.java", JAVA_OVERLOADS);
    let calls = call_edges(&result);
    assert!(
        calls.iter().all(|(from, to)| from != to),
        "an overload calling its sibling is not a self-edge: {calls:?}"
    );
    let reasons = abstentions(&result, "add");
    assert!(
        reasons.iter().any(|reason| reason.contains("overloads share one id")),
        "the dropped sibling call is a labelled abstention: {reasons:?}"
    );
}

/// Java's `Calc c` parameter extracts a local binding with `declared_type:
/// None` (verified), so the receiver is never typed, with or without overloads:
/// a single `add` misses `c.add(1)` too. Blocked on extract-v98 (the Java
/// extractor must record a parameter's declared type); not an overload defect.
#[test]
#[ignore = "blocked on extract-v98: the Java extractor records no declared type for a parameter"]
fn java_typed_parameter_receiver_binds() {
    let result = resolve("Calc.java", JAVA_OVERLOADS);
    let calls = call_edges(&result);
    assert!(
        calls.contains(&("Calc.java::U.use".to_string(), "Calc.java::Calc.add".to_string())),
        "{calls:?}"
    );
}

#[test]
fn swift_overloads_never_self_edge_and_the_receiver_call_binds() {
    assert_overloads_never_self_edge(
        "Calc.swift",
        "class Calc {\n  func add(_ a: Int) -> Int { return a }\n  func add(_ a: String) -> Int { return add(a.count) }\n}\nfunc useCalc(_ c: Calc) -> Int { return c.add(1) + c.add(\"x\") }\n",
        "Calc",
        "useCalc",
    );
}

/// TypeScript overload signatures have no bodies, so they never call one
/// another, and the extractor folds them into the one implementation symbol.
/// MET today: the typed-receiver call binds and nothing needed fixing.
#[test]
fn typescript_overload_signatures_bind_the_receiver_call() {
    let path = "calc.ts";
    let result = resolve(
        path,
        "class Calc {\n  add(a: number): number;\n  add(a: string): number;\n  add(a: any): number { return a; }\n}\nfunction useCalc(c: Calc): number { return c.add(1) + c.add('x'); }\n",
    );
    let calls = call_edges(&result);
    assert!(
        calls.contains(&("calc.ts::useCalc".to_string(), "calc.ts::Calc.add".to_string())),
        "{calls:?}"
    );
    assert!(calls.iter().all(|(from, to)| from != to), "{calls:?}");
}

/// The guard is keyed on a *shared* identity, so a function with one
/// declaration keeps its genuine recursion edge.
#[test]
fn real_recursion_of_a_single_declaration_still_self_edges() {
    let result = resolve(
        "rec.kt",
        "fun f(n: Int): Int { return f(n - 1) }\nclass R { fun g(n: Int): Int = g(n - 1) }\n",
    );
    let calls = call_edges(&result);
    assert!(calls.contains(&("rec.kt::f".into(), "rec.kt::f".into())), "{calls:?}");
    assert!(calls.contains(&("rec.kt::R.g".into(), "rec.kt::R.g".into())), "{calls:?}");
}

/// Same-named methods of two different classes are different identities: the
/// guard must not swallow a call between them.
#[test]
fn same_method_name_on_two_classes_is_not_an_overload() {
    let result = resolve(
        "two.kt",
        "class A {\n    fun go(): Int = 1\n}\n\nclass B {\n    fun go(a: A): Int = a.go()\n}\n",
    );
    let calls = call_edges(&result);
    assert!(calls.contains(&("two.kt::B.go".into(), "two.kt::A.go".into())), "{calls:?}");
}
