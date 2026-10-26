//! Swift resolver-side residue (ft-915b0a03): a private nested namesake, a
//! protocol-extension method chained after another call, a closure stored in a
//! property and called through it, and a nested type named from outside its
//! type. Each positive case has the adversarial neighbour that must abstain.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let dangling = resolver.dangling_targets(&result);
    assert!(dangling.is_empty(), "dangling: {dangling:?}");
    result
}

/// Every use edge (call or reference) whose source ends with `caller`.
fn uses(result: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = result
        .edges
        .iter()
        .filter(|e: &&ResolvedEdge| {
            matches!(e.edge_kind, EdgeKind::References | EdgeKind::Calls)
                && (e.source_symbol.ends_with(&format!("::{caller}"))
                    || e.source_symbol.ends_with(&format!(".{caller}")))
        })
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out
}

fn has(u: &[String], target: &str) -> bool {
    u.iter().any(|t| t.ends_with(target))
}

// ------------------------------------------------- T7: private nested namesake

const RNG: &str = "struct Rng {\n var seed: Int\n mutating func next() -> Int { seed += 1; return seed }\n}\nfunc roll() -> Int {\n var r = Rng(seed: 1)\n return r.next()\n}\n";
const PRIVATE_RNG: &str =
    "struct Wrapper {\n private struct Rng {\n  func next() -> Int { 0 }\n }\n func inner() -> Int { Rng().next() }\n}\n";

#[test]
fn a_private_nested_namesake_in_another_file_does_not_hide_the_visible_type() {
    let r = resolve(&[
        ("Sources/M/Q6.swift", RNG),
        ("Sources/M/Q7.swift", PRIVATE_RNG),
    ]);
    let u = uses(&r, "roll");
    assert!(has(&u, "Q6.swift::Rng.next"), "{u:?}");
    assert!(!has(&u, "Q7.swift::Rng.next"), "{u:?}");
}

#[test]
fn a_private_nested_type_alone_in_another_file_is_not_the_receivers_type() {
    let caller = "func roll() -> Int {\n var r = Rng()\n return r.next()\n}\n";
    let r = resolve(&[
        ("Sources/M/Q7.swift", PRIVATE_RNG),
        ("Sources/M/Q8.swift", caller),
    ]);
    let u = uses(&r, "roll");
    assert!(!has(&u, "Rng.next"), "{u:?}");
}

#[test]
fn two_visible_namesakes_still_abstain() {
    let other = "struct Rng {\n func next() -> Int { 0 }\n}\n";
    let r = resolve(&[("Sources/M/Q6.swift", RNG), ("Sources/M/Q9.swift", other)]);
    let u = uses(&r, "roll");
    assert!(!has(&u, "Rng.next"), "{u:?}");
}

#[test]
fn a_fileprivate_member_of_another_file_is_not_a_candidate() {
    let hidden = "extension Rng {\n fileprivate func next() -> Int { 0 }\n}\n";
    let r = resolve(&[("Sources/M/Q6.swift", RNG), ("Sources/M/Q9.swift", hidden)]);
    let u = uses(&r, "roll");
    assert!(has(&u, "Q6.swift::Rng.next"), "{u:?}");
    assert!(!has(&u, "Q9.swift::Rng.next"), "{u:?}");
}

// ------------------------------------------ O3: a chain through a return type

const VIEW: &str = "protocol View {}\nextension View {\n func padding() -> some View { self }\n func badge() -> some View { self }\n}\nstruct Other { func badge() {} }\nstruct Base: View {}\nstruct Screen: View {\n var base: Base\n func body() { base.padding().badge() }\n}\n";

#[test]
fn a_protocol_extension_method_chained_after_another_is_the_protocols() {
    let r = resolve(&[("Sources/M/Q3.swift", VIEW)]);
    let u = uses(&r, "Screen.body");
    assert!(has(&u, "View.padding"), "{u:?}");
    assert!(has(&u, "View.badge"), "{u:?}");
    assert!(!has(&u, "Other.badge"), "{u:?}");
}

#[test]
fn a_chain_through_a_written_concrete_return_type_binds_the_next_call() {
    let src = "struct Cmd { func inverse() {} }\nstruct Hist { func top() -> Cmd { Cmd() } }\nstruct Runner {\n var hist: Hist\n func go() { hist.top().inverse() }\n}\nstruct Decoy { func inverse() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "Runner.go");
    assert!(has(&u, "Cmd.inverse"), "{u:?}");
    assert!(!has(&u, "Decoy.inverse"), "{u:?}");
}

#[test]
fn a_chain_through_an_overload_set_types_nothing() {
    let src = "struct Cmd { func inverse() {} }\nstruct Other { func inverse() {} }\nstruct Hist {\n func top() -> Cmd { Cmd() }\n func top(_ n: Int) -> Other { Other() }\n}\nstruct Runner {\n var hist: Hist\n func go() { hist.top().inverse() }\n}\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "Runner.go");
    assert!(!has(&u, "inverse"), "{u:?}");
}

#[test]
fn a_chain_through_an_optional_or_collection_result_types_nothing() {
    let src = "struct Cmd { func inverse() {} }\nstruct Hist {\n func top() -> Cmd? { nil }\n func all() -> [Cmd] { [] }\n}\nstruct Runner {\n var hist: Hist\n func go() { hist.top().inverse(); hist.all().inverse() }\n}\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "Runner.go");
    assert!(!has(&u, "Cmd.inverse"), "{u:?}");
}

#[test]
fn a_chain_whose_result_type_is_declared_twice_types_nothing() {
    let a = "struct Cmd { func inverse() {} }\nstruct Hist { func top() -> Cmd { Cmd() } }\nstruct Runner {\n var hist: Hist\n func go() { hist.top().inverse() }\n}\n";
    let b = "struct Cmd { func inverse() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "Runner.go");
    assert!(!has(&u, "Cmd.inverse"), "{u:?}");
}

#[test]
fn a_chain_on_an_untyped_root_types_nothing() {
    let src = "struct Cmd { func inverse() {} }\nstruct Hist { func top() -> Cmd { Cmd() } }\nfunc go(_ mystery: Foo) { mystery.top().inverse() }\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "go");
    assert!(!has(&u, "Cmd.inverse"), "{u:?}");
}

// --------------------------------------- a closure stored in a property, called

#[test]
#[ignore = "needs extractor function-type facts (schema frozen at 97), ft-915b0a03 follow-up"]
fn a_closure_property_called_through_its_name_is_a_use_of_the_property() {
    let src = "struct S { var handler: () -> Void }\nstruct T { var handler: () -> Void }\nfunc g(_ s: S) { s.handler() }\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "g");
    assert!(has(&u, "S.handler"), "{u:?}");
    assert!(!has(&u, "T.handler"), "{u:?}");
}

#[test]
fn a_method_of_the_name_wins_over_a_property_call() {
    let src = "struct S {\n var size: Int\n func size(_ n: Int) -> Int { n }\n}\nfunc g(_ s: S) -> Int { s.size(1) }\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "g");
    assert!(has(&u, "S.size"), "{u:?}");
}

#[test]
fn a_closure_property_call_on_an_unknown_receiver_binds_nothing() {
    let src = "struct S { var handler: () -> Void }\nfunc g(_ s: Mystery) { s.handler() }\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "g");
    assert!(!has(&u, "S.handler"), "{u:?}");
}

#[test]
#[ignore = "needs extractor function-type facts (schema frozen at 97), ft-915b0a03 follow-up"]
fn a_closure_property_inherited_from_a_protocol_requirement_is_reached() {
    let src = "protocol P { var handler: () -> Void { get } }\nfunc g(_ p: P) { p.handler() }\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    let u = uses(&r, "g");
    assert!(has(&u, "P.handler"), "{u:?}");
}

// ------------------------------ honest rows the extractor's output cannot lift
//
// sw.O1c, sw.T4 and sw.Y1 stay in known_failing.tsv because the resolver is
// handed too little (see the notes there). What it must do meanwhile is stay
// silent: no edge to a decoy of the same member name.

#[test]
fn honest_row_a_local_typed_by_a_free_function_returning_a_collection_binds_no_decoy() {
    let src = "struct Parcel { func weight() -> Int { 1 } }\nstruct Decoy { func weight() -> Int { 2 } }\nfunc loadItems() -> [Parcel] { [] }\nstruct Crate {\n func fromFree() { let xs = loadItems(); _ = xs.map { $0.weight() } }\n}\n";
    let r = resolve(&[("Sources/M/Q1.swift", src)]);
    let u = uses(&r, "Crate.fromFree");
    assert!(!has(&u, "Decoy.weight"), "{u:?}");
}

#[test]
fn honest_row_a_member_initialised_from_a_shared_singleton_binds_no_decoy() {
    let src = "struct Mgr { static let shared = Mgr(); func run() {} }\nstruct Decoy { func run() {} }\nfinal class Runner {\n private let m = Mgr.shared\n func t4() { m.run() }\n}\n";
    let r = resolve(&[("Sources/M/Q5.swift", src)]);
    let u = uses(&r, "Runner.t4");
    assert!(!has(&u, "Decoy.run"), "{u:?}");
}

#[test]
fn honest_row_a_shadowed_singleton_binds_the_local_not_the_type() {
    let src = "struct Mgr { static let shared = Mgr(); func run() {} }\nstruct Other { func run() {} }\nfinal class Runner {\n private let m = Mgr.shared\n func t() { let m = Other(); m.run() }\n}\n";
    let r = resolve(&[("Sources/M/Q5.swift", src)]);
    let u = uses(&r, "Runner.t");
    assert!(has(&u, "Other.run"), "{u:?}");
    assert!(!has(&u, "Mgr.run"), "{u:?}");
}

#[test]
fn honest_row_a_cross_file_extension_typealias_never_binds_the_top_level_type() {
    let depot = "struct Lot { func mass() -> Int { 1 } }\nstruct Stock { func mass() -> Int { 2 } }\nstruct Depot {\n var lots: [Stock]\n func tally() { lots.forEach { _ = $0.mass() } }\n}\n";
    let ext = "extension Depot { typealias Stock = Lot }\n";
    let r = resolve(&[("Sources/M/Q9.swift", depot), ("Sources/M/Q10.swift", ext)]);
    let u = uses(&r, "Depot.tally");
    assert!(!has(&u, "Stock.mass"), "{u:?}");
}

#[test]
fn two_modules_declaring_one_nested_type_name_bind_neither_from_outside() {
    let a = "struct A { struct Item { func weigh() {} } }\n";
    let b = "struct B { struct Item { func weigh() {} } }\n";
    let caller = "func go() { let i = Item(); i.weigh() }\n";
    let r = resolve(&[
        ("Sources/One/A.swift", a),
        ("Sources/Two/B.swift", b),
        ("Sources/Three/C.swift", caller),
    ]);
    let u = uses(&r, "go");
    assert!(!has(&u, "Item.weigh") && !has(&u, "Item"), "{u:?}");
}

// ------------------------- X42: deinit and extension members of another file

#[test]
fn a_deinit_body_reaches_an_extension_member_declared_in_another_file() {
    let class = "final class Session {\n deinit {\n  cleanup()\n  self.drain()\n }\n}\nclass Other { func cleanup() {}\n func drain() {} }\n";
    let ext = "extension Session {\n func cleanup() {}\n func drain() {}\n}\n";
    let r = resolve(&[
        ("Sources/M/Session.swift", class),
        ("Sources/M/SessionExt.swift", ext),
    ]);
    let u = uses(&r, "Session");
    assert!(has(&u, "SessionExt.swift::Session.cleanup"), "{u:?}");
    assert!(has(&u, "SessionExt.swift::Session.drain"), "{u:?}");
    assert!(
        !has(&u, "Other.cleanup") && !has(&u, "Other.drain"),
        "{u:?}"
    );
}

// ------------------------------------------- a nested type is named from inside

const OUTER: &str = "struct A {\n struct Inner {}\n func f() { _ = Inner() }\n}\n";

#[test]
fn a_nested_type_does_not_leak_into_an_unrelated_type_or_function() {
    let other = "struct B { func g() { _ = Inner() } }\nfunc free() { _ = Inner() }\n";
    let r = resolve(&[("Sources/M/A.swift", OUTER), ("Sources/M/B.swift", other)]);
    assert!(
        has(&uses(&r, "f"), "A.Inner"),
        "control: the outer type still binds"
    );
    assert!(!has(&uses(&r, "B.g"), "A.Inner"), "{:?}", uses(&r, "B.g"));
    assert!(!has(&uses(&r, "free"), "A.Inner"), "{:?}", uses(&r, "free"));
}

#[test]
fn a_nested_type_is_still_named_from_an_extension_in_another_file() {
    let ext = "extension A { func h() { _ = Inner() } }\n";
    let r = resolve(&[("Sources/M/A.swift", OUTER), ("Sources/M/E.swift", ext)]);
    assert!(has(&uses(&r, "A.h"), "A.Inner"), "{:?}", uses(&r, "A.h"));
}

#[test]
fn a_nested_type_is_still_named_from_a_sibling_nested_type_and_a_subclass() {
    let src = "class A {\n class Inner {}\n class Other { func k() { _ = Inner() } }\n}\nclass Sub: A { func m() { _ = Inner() } }\n";
    let r = resolve(&[("Sources/M/A.swift", src)]);
    assert!(
        has(&uses(&r, "Sub.m"), "A.Inner"),
        "{:?}",
        uses(&r, "Sub.m")
    );
    assert!(has(&uses(&r, "k"), "A.Inner"), "{:?}", uses(&r, "k"));
}

#[test]
fn a_typealias_naming_the_nested_type_keeps_the_binding() {
    let alias = "typealias Inner = A.Inner\nfunc free() { _ = Inner() }\n";
    let r = resolve(&[("Sources/M/A.swift", OUTER), ("Sources/M/B.swift", alias)]);
    assert!(has(&uses(&r, "free"), "A.Inner"), "{:?}", uses(&r, "free"));
}
