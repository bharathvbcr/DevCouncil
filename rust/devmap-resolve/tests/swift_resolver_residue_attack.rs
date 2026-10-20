//! Adversarial probes for lane/swift-resolver-residue (7e110127..7b6b6bce).
//! Each test states the CORRECT behaviour; a failing test is a BREAK.
//! Prefix `wrong_` = a wrong edge, `recall_` = a missing edge.
//! The `#[ignore]` dump tests are characterisation tools (run with
//! `-- --ignored`), not assertions.

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
    resolver.resolve_all(&extractions).unwrap()
}

fn one(src: &str) -> ResolutionResult {
    resolve(&[("Sources/M/A.swift", src)])
}

/// Use edges of a caller as `target|Confidence`, sorted.
fn uses(result: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = result
        .edges
        .iter()
        .filter(|e: &&ResolvedEdge| {
            matches!(e.edge_kind, EdgeKind::References | EdgeKind::Calls)
                && (e.source_symbol.ends_with(&format!("::{caller}"))
                    || e.source_symbol.ends_with(&format!(".{caller}")))
        })
        .map(|e| format!("{}|{:?}", e.target_symbol, e.confidence))
        .collect();
    out.sort();
    out
}

fn has(u: &[String], target: &str) -> bool {
    u.iter().any(|t| t.split('|').next().unwrap_or("").ends_with(target))
}

fn all_edges(files: &[(&str, &str)]) -> Vec<String> {
    let r = resolve(files);
    let mut v: Vec<String> = r
        .edges
        .iter()
        .map(|e| {
            format!(
                "{:?} {} -> {} [{:?}]",
                e.edge_kind, e.source_symbol, e.target_symbol, e.confidence
            )
        })
        .collect();
    v.sort();
    v
}

// ============================================================ 1. protocol-extension chains

#[test]
fn wrong_chain_through_self_return_keeps_the_concrete_type_override() {
    // padding() -> Self on a protocol extension: the value is a Base, whose own
    // tag() overrides the default. The edge must not be the extension default.
    let src = "protocol P {}\nextension P {\n func padding() -> Self { self }\n func tag() {}\n}\nstruct Base: P { func tag() {} }\nstruct S {\n var b: Base\n func f() { b.padding().tag() }\n}\n";
    let u = uses(&one(src), "S.f");
    assert!(!has(&u, "P.tag"), "{u:?}");
}

#[test]
fn wrong_chain_through_opaque_return_does_not_pick_default_over_override_for_concrete_receiver() {
    // Receiver Base declares its own badge: a direct call must bind it.
    let src = "protocol P {}\nextension P {\n func badge() -> some P { self }\n func tag() {}\n}\nstruct Base: P {\n func badge() -> Base { self }\n func tag() {}\n}\nstruct S {\n var b: Base\n func f() { b.badge().tag() }\n}\n";
    let u = uses(&one(src), "S.f");
    assert!(has(&u, "Base.badge"), "{u:?}");
    assert!(has(&u, "Base.tag"), "{u:?}");
    assert!(!has(&u, "P.tag") && !has(&u, "P.badge"), "{u:?}");
}

#[test]
fn wrong_two_same_named_protocols_in_two_modules_type_nothing() {
    let a = "protocol View {}\nextension View {\n func padding() -> some View { self }\n func badge() {}\n}\nstruct AB: View {}\n";
    let b = "protocol View {}\nextension View {\n func padding() -> some View { self }\n func badge() {}\n}\nstruct BB: View {}\n";
    let c = "struct S {\n var ab: AB\n func f() { ab.padding().badge() }\n}\n";
    let r = resolve(&[
        ("Sources/A/A.swift", a),
        ("Sources/B/B.swift", b),
        ("Sources/C/C.swift", c),
    ]);
    let u = uses(&r, "S.f");
    // Whatever binds, it must not be the OTHER module's protocol: S sees AB -> A.View only.
    assert!(!has(&u, "B.swift::View.badge") && !has(&u, "B.swift::View.padding"), "{u:?}");
}

#[test]
fn wrong_where_self_constraint_default_does_not_beat_own_method() {
    let src = "protocol P {}\nprotocol Marker {}\nextension P where Self: Marker {\n func special() -> some P { self }\n}\nstruct M: P, Marker {}\nstruct N: P { func special() {} }\nstruct S {\n var n: N\n func f() { n.special() }\n}\n";
    let u = uses(&one(src), "S.f");
    assert!(has(&u, "N.special"), "{u:?}");
    assert!(!has(&u, "P.special"), "{u:?}");
}

#[test]
fn wrong_where_self_constraint_default_is_not_applied_to_nonconforming_receiver() {
    // N does not satisfy Marker, so the constrained extension method does not
    // exist on N: n.special() must not bind P.special.
    let src = "protocol P {}\nprotocol Marker {}\nextension P where Self: Marker {\n func special() {}\n}\nstruct N: P {}\nstruct S {\n var n: N\n func f() { n.special() }\n}\n";
    let u = uses(&one(src), "S.f");
    assert!(!has(&u, "P.special"), "{u:?}");
}

#[test]
fn wrong_generic_parameter_return_is_not_a_declared_type_of_that_name() {
    let src = "struct Item { func tag() {} }\nstruct F { func id<Item>(_ x: Item) -> Item { x } }\nstruct S {\n var f: F\n func go() { f.id(1).tag() }\n}\n";
    let u = uses(&one(src), "S.go");
    assert!(!has(&u, "Item.tag"), "{u:?}");
}

#[test]
fn wrong_generic_struct_parameter_return_is_not_a_declared_type_of_that_name() {
    let src = "struct Element { func tag() {} }\nstruct Box<Element> { func first() -> Element { fatalError() } }\nstruct S {\n var b: Box<Int>\n func go() { b.first().tag() }\n}\n";
    let u = uses(&one(src), "S.go");
    assert!(!has(&u, "Element.tag"), "{u:?}");
}

#[test]
fn wrong_associatedtype_return_is_not_a_declared_type_of_that_name() {
    let src = "struct Item { func tag() {} }\nprotocol Store {\n associatedtype Item\n func first() -> Item\n}\nstruct S {\n var st: any Store\n func go() { st.first().tag() }\n}\n";
    let u = uses(&one(src), "S.go");
    assert!(!has(&u, "Item.tag"), "{u:?}");
}

#[test]
fn wrong_chain_return_nested_in_another_type_is_not_a_bare_namesake() {
    // First.make() returns `Inner` meaning First.Inner; a top-level Inner exists
    // elsewhere and is the only visible "declaration" if nested ones are excluded.
    let src = "struct Inner { func tag() {} }\nstruct First {\n struct Inner { func other() {} }\n func make() -> Inner { Inner() }\n}\nstruct S {\n var f: First\n func go() { f.make().tag() }\n}\n";
    let u = uses(&one(src), "S.go");
    assert!(!has(&u, "::Inner.tag"), "{u:?}");
}

// ============================================================ 2. dispatch by bare protocol name

const USER_VIEW: &str = "protocol View {}\nextension View {\n func padding() -> some View { self }\n func badge() -> some View { self }\n}\nstruct Base: View {}\nstruct Screen: View {\n var base: Base\n func body() { base.padding().badge() }\n}\n";
const SWIFTUI_FILE: &str = "import SwiftUI\nstruct RootView: View {\n var body: some View { Text(\"x\") }\n func badge() -> some View { self }\n}\n";

#[test]
fn wrong_a_swiftui_conformer_is_not_a_conformer_of_the_user_view_protocol() {
    let r = resolve(&[
        ("Sources/Lib/V.swift", USER_VIEW),
        ("Sources/App/Root.swift", SWIFTUI_FILE),
    ]);
    let u = uses(&r, "Screen.body");
    assert!(!has(&u, "RootView.badge"), "{u:?}");
}

#[test]
fn wrong_direct_protocol_typed_call_does_not_reach_a_swiftui_conformer() {
    // Pre-existing shape (no chain involved): a receiver typed by the user's View.
    let caller = "struct Host {\n var v: any View\n func go() { v.badge() }\n}\n";
    let r = resolve(&[
        ("Sources/Lib/V.swift", USER_VIEW),
        ("Sources/App/Root.swift", SWIFTUI_FILE),
        ("Sources/Lib/Host.swift", caller),
    ]);
    let u = uses(&r, "Host.go");
    assert!(!has(&u, "RootView.badge"), "{u:?}");
}

// ============================================================ 3. private / fileprivate

#[test]
fn recall_private_file_scope_type_is_visible_in_its_own_file() {
    let src = "private struct Helper {\n func go() {}\n}\nfunc run() {\n var h = Helper()\n h.go()\n}\n";
    let u = uses(&one(src), "run");
    assert!(has(&u, "Helper.go"), "{u:?}");
}

#[test]
fn recall_private_nested_type_is_visible_from_an_extension_in_the_same_file() {
    let src = "struct W {\n private struct Rng { func next() -> Int { 0 } }\n}\nextension W {\n func roll() -> Int { var r = Rng(); return r.next() }\n}\n";
    let u = uses(&one(src), "W.roll");
    assert!(has(&u, "Rng.next"), "{u:?}");
}

#[test]
fn recall_extension_member_of_a_private_type_in_the_same_file_binds() {
    let src = "private struct Helper {}\nextension Helper { func go2() {} }\nfunc run() {\n var h = Helper()\n h.go2()\n}\n";
    let u = uses(&one(src), "run");
    assert!(has(&u, "Helper.go2"), "{u:?}");
}

#[test]
fn recall_internal_extension_member_in_another_file_binds() {
    let a = "struct Rng {}\n";
    let b = "extension Rng { func extra() {} }\n";
    let c = "func run() {\n var r = Rng()\n r.extra()\n}\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(has(&u, "B.swift::Rng.extra"), "{u:?}");
}

#[test]
fn recall_private_set_property_is_readable_and_callable_from_another_file() {
    let a = "struct S {\n private(set) var handler: () -> Void\n}\n";
    let c = "func run(_ s: S) {\n s.handler()\n}\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(has(&u, "S.handler"), "{u:?}");
}

#[test]
fn recall_public_private_set_method_receiver_from_another_file() {
    let a = "struct Counter {\n public private(set) var count: Int = 0\n func inc() {}\n}\n";
    let c = "func run() {\n var c = Counter()\n c.inc()\n}\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(has(&u, "Counter.inc"), "{u:?}");
}

#[test]
fn wrong_file_scope_private_function_of_another_file_is_not_a_candidate() {
    let a = "private func helper() {}\n";
    let b = "func helper() {}\n";
    let c = "func run() { helper() }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(!has(&u, "A.swift::helper"), "{u:?}");
}

#[test]
fn wrong_private_method_in_type_is_not_reached_cross_file() {
    let a = "struct T {\n private func go() {}\n}\n";
    let c = "func run() {\n var t = T()\n t.go()\n}\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(!has(&u, "T.go"), "{u:?}");
}

#[test]
fn wrong_private_type_in_other_file_does_not_make_the_reached_member_hit_vanish() {
    // Visible Rng.next in B must still bind when A has `private struct Rng` with a same-named member and the caller is in C.
    let a = "private struct Rng { func next() {} }\n";
    let b = "struct Rng { func next() {} }\n";
    let c = "func run() {\n var r = Rng()\n r.next()\n}\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(has(&u, "B.swift::Rng.next"), "{u:?}");
    assert!(!has(&u, "A.swift::Rng.next"), "{u:?}");
}

#[test]
fn wrong_internal_member_does_not_cross_into_a_module_that_never_imports_it() {
    let a = "struct Gadget { func spin() {} }\n";
    let c = "func run() {\n var g = Gadget()\n g.spin()\n}\n";
    let r = resolve(&[("Sources/One/A.swift", a), ("Sources/Two/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(!has(&u, "Gadget.spin"), "{u:?}");
}

#[test]
fn recall_public_member_crosses_into_a_module_that_imports_it() {
    let a = "public struct Gadget { public init() {}\n public func spin() {} }\n";
    let c = "import One\nfunc run() {\n var g = Gadget()\n g.spin()\n}\n";
    let r = resolve(&[("Sources/One/A.swift", a), ("Sources/Two/C.swift", c)]);
    let u = uses(&r, "run");
    assert!(has(&u, "Gadget.spin"), "{u:?}");
}

// ============================================================ 4. closure-typed property call

#[test]
fn recall_optional_closure_property_call_binds_the_property() {
    let src = "struct S { var h: (() -> Void)? }\nstruct T { var h: (() -> Void)? }\nfunc g(_ s: S) { s.h?() }\n";
    let u = uses(&one(src), "g");
    assert!(has(&u, "S.h"), "{u:?}");
    assert!(!has(&u, "T.h"), "{u:?}");
}

#[test]
fn wrong_bare_call_of_a_local_shadowing_a_closure_property_binds_no_property() {
    let src = "struct S {\n var handler: () -> Void\n func run() {\n  let handler = { }\n  handler()\n }\n}\n";
    let u = uses(&one(src), "S.run");
    assert!(!has(&u, "S.handler"), "{u:?}");
}

#[test]
fn wrong_bare_call_of_a_parameter_shadowing_a_closure_property_binds_no_property() {
    let src = "struct S {\n var handler: () -> Void\n func run(handler: () -> Void) {\n  handler()\n }\n}\n";
    let u = uses(&one(src), "S.run");
    assert!(!has(&u, "S.handler"), "{u:?}");
}

#[test]
fn wrong_bare_call_of_a_closure_parameter_in_a_nested_closure_binds_no_property() {
    let src = "struct S {\n var handler: () -> Void\n func run() {\n  [1].forEach { handler in handler() }\n }\n}\n";
    let u = uses(&one(src), "S.run");
    assert!(!has(&u, "S.handler"), "{u:?}");
}

#[test]
fn recall_static_closure_property_called_through_the_type() {
    let src = "struct S {\n static var make: () -> S = { S() }\n}\nstruct T {\n static var make: () -> T = { T() }\n}\nfunc g() { _ = S.make() }\n";
    let u = uses(&one(src), "g");
    assert!(has(&u, "S.make"), "{u:?}");
    assert!(!has(&u, "T.make"), "{u:?}");
}

#[test]
fn recall_typealias_of_a_closure_type_property_call() {
    let src = "typealias Handler = () -> Void\nstruct S { var handler: Handler }\nstruct T { var handler: Handler }\nfunc g(_ s: S) { s.handler() }\n";
    let u = uses(&one(src), "g");
    assert!(has(&u, "S.handler"), "{u:?}");
    assert!(!has(&u, "T.handler"), "{u:?}");
}

#[test]
fn wrong_a_swiftui_method_call_on_a_view_with_a_same_named_stored_property_binds_no_property() {
    // `padding(4)` is View.padding, an unindexed SwiftUI method; Card.padding is a CGFloat.
    let src = "import SwiftUI\nstruct Card: View {\n var padding: CGFloat = 8\n var body: some View { Text(\"\") }\n}\nstruct Screen: View {\n var card: Card\n var body: some View { card.padding(4) }\n}\n";
    let u = uses(&one(src), "Screen.body");
    assert!(!has(&u, "Card.padding"), "{u:?}");
}

#[test]
fn wrong_a_stdlib_method_call_on_a_sequence_with_a_same_named_stored_property_binds_no_property() {
    let src = "struct Box: Sequence {\n var first: Int = 0\n func makeIterator() -> IndexingIterator<[Int]> { [first].makeIterator() }\n}\nfunc g(_ b: Box) { _ = b.first(where: { $0 > 1 }) }\n";
    let u = uses(&one(src), "g");
    assert!(!has(&u, "Box.first"), "{u:?}");
}

#[test]
fn wrong_a_non_closure_property_is_not_called_through() {
    // `count` is an Int; `s.count()` can only be some unindexed extension method.
    let src = "struct S { var count: Int }\nfunc g(_ s: S) { s.count() }\n";
    let u = uses(&one(src), "g");
    assert!(!has(&u, "S.count"), "{u:?}");
}

// ============================================================ 5. nested-type leak abstention recall

#[test]
fn recall_same_named_nested_types_bind_each_to_its_own_outer() {
    let src = "struct A {\n struct Inner {}\n func f() { _ = Inner() }\n}\nstruct B {\n struct Inner {}\n func g() { _ = Inner() }\n}\n";
    let r = one(src);
    let f = uses(&r, "A.f");
    let g = uses(&r, "B.g");
    assert!(has(&f, "A.Inner") && !has(&f, "B.Inner"), "{f:?}");
    assert!(has(&g, "B.Inner") && !has(&g, "A.Inner"), "{g:?}");
}

#[test]
fn recall_explicit_outer_qualified_nested_type_binds_from_outside() {
    let src = "struct A { struct Inner { func m() {} } }\nstruct B { struct Inner { func m() {} } }\nfunc free() { var x = A.Inner(); x.m() }\n";
    let u = uses(&one(src), "free");
    assert!(has(&u, "A.Inner"), "{u:?}");
    assert!(!has(&u, "B.Inner"), "{u:?}");
}

#[test]
fn recall_explicit_qualified_nested_type_in_a_type_annotation_binds() {
    let src = "struct A { struct Inner { func m() {} } }\nfunc take(_ x: A.Inner) { x.m() }\n";
    let u = uses(&one(src), "take");
    assert!(has(&u, "A.Inner"), "{u:?}");
}

#[test]
fn recall_qualified_nested_type_in_another_file_binds_when_unique() {
    let a = "struct A { struct Inner {} }\n";
    let c = "func free() { _ = A.Inner() }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "free");
    assert!(has(&u, "A.Inner"), "{u:?}");
}

#[test]
fn recall_nested_type_declared_in_an_extension_is_named_from_the_outer_type() {
    let a = "struct A {\n func f() { _ = Inner() }\n}\n";
    let b = "extension A { struct Inner {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "A.f");
    assert!(has(&u, "A.Inner"), "{u:?}");
}

#[test]
fn recall_deeply_nested_type_names_an_outer_sibling_type() {
    let src = "struct A {\n struct M {\n  struct Deep { func f() { _ = Inner() } }\n }\n struct Inner {}\n}\n";
    let u = uses(&one(src), "Deep.f");
    assert!(has(&u, "A.Inner"), "{u:?}");
}

#[test]
fn recall_property_initializer_names_a_nested_type() {
    let src = "struct A {\n struct Inner {}\n static let shared = Inner()\n var made = Inner()\n func f(x: Inner = Inner()) {}\n}\n";
    let r = one(src);
    let all = all_edges(&[("Sources/M/A.swift", src)]);
    let touched = all.iter().any(|e| e.contains("-> ") && e.contains("A.Inner") && !e.contains("Inner ->"));
    assert!(touched, "no edge reaches A.Inner from property initializers: {all:#?} {:?}", r.edges.len());
}

#[test]
fn recall_nested_type_in_a_protocol_extension_conformer_is_named_from_an_extension_of_the_outer_in_another_file() {
    let a = "class A { class Inner {} }\n";
    let b = "extension A {\n func f() { _ = Inner() }\n}\nclass Sub: A { func g() { _ = Inner() } }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    assert!(has(&uses(&r, "A.f"), "A.Inner"), "{:?}", uses(&r, "A.f"));
    assert!(has(&uses(&r, "Sub.g"), "A.Inner"), "{:?}", uses(&r, "Sub.g"));
}

// ============================================================ 6. cross-file extension member bare call

#[test]
fn wrong_bare_call_binds_no_member_of_an_unrelated_types_extension() {
    let a = "struct A {\n func f() { helper() }\n}\nstruct B {}\n";
    let b = "extension B { func helper() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "A.f");
    assert!(!has(&u, "B.helper"), "{u:?}");
}

#[test]
fn wrong_bare_call_prefers_the_free_function_over_an_unrelated_types_extension_member() {
    let a = "struct A {\n func f() { helper() }\n}\nstruct B {}\n";
    let b = "extension B { func helper() {} }\n";
    let c = "func helper() {}\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "A.f");
    assert!(!has(&u, "B.helper"), "{u:?}");
    assert!(has(&u, "C.swift::helper"), "{u:?}");
}

#[test]
fn recall_own_extension_member_in_another_file_shadows_the_free_function() {
    let a = "struct A {\n func f() { helper() }\n}\n";
    let b = "extension A { func helper() {} }\n";
    let c = "func helper() {}\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b), ("Sources/M/C.swift", c)]);
    let u = uses(&r, "A.f");
    assert!(has(&u, "B.swift::A.helper"), "{u:?}");
    assert!(!has(&u, "C.swift::helper"), "{u:?}");
}

#[test]
fn recall_supertype_extension_member_in_another_file_binds_from_a_subclass() {
    let a = "class Base {}\nclass Sub: Base {\n func f() { helper() }\n}\n";
    let b = "extension Base { func helper() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "Sub.f");
    assert!(has(&u, "B.swift::Base.helper"), "{u:?}");
}

#[test]
fn wrong_free_function_bare_call_binds_no_extension_member() {
    let a = "func run() { helper() }\nstruct B {}\n";
    let b = "extension B { func helper() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "run");
    assert!(!has(&u, "B.helper"), "{u:?}");
}

#[test]
fn wrong_nested_type_bare_call_binds_no_outer_instance_member() {
    let a = "struct A {\n struct N { func f() { helper() } }\n}\n";
    let b = "extension A { func helper() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "N.f");
    assert!(!has(&u, "A.helper"), "{u:?}");
}

#[test]
fn wrong_bare_call_of_a_closure_local_defined_in_the_method_binds_no_extension_member() {
    let a = "struct A {\n func f() {\n  func helper() {}\n  helper()\n }\n}\n";
    let b = "extension A { func helper() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "A.f");
    assert!(!has(&u, "B.swift::A.helper"), "{u:?}");
}

#[test]
fn wrong_bare_type_constructor_call_binds_no_same_named_extension_method() {
    // `Item()` inside A is the Item type's init; an extension method named Item (unusual) in another file must not capture it.
    let a = "struct Item {}\nstruct A {\n func f() { _ = Item() }\n}\n";
    let b = "extension A { func Item() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "A.f");
    // Swift would in fact pick the member; a type-init edge or the member are both defensible, but a HIGH edge to the free type is not required. Characterise only: must produce SOME edge.
    assert!(!u.is_empty(), "{u:?}");
}

#[test]
fn wrong_stdlib_bare_call_in_a_type_binds_no_unrelated_extension_member() {
    let a = "struct A {\n func f() { print(1) }\n}\nstruct Logger {}\n";
    let b = "extension Logger { func print(_ x: Int) {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "A.f");
    assert!(!has(&u, "Logger.print"), "{u:?}");
}

#[test]
fn wrong_bare_call_in_a_conformer_binds_no_protocol_extension_member_of_an_unrelated_protocol() {
    let a = "protocol P {}\nprotocol Q {}\nstruct A: P {\n func f() { helper() }\n}\n";
    let b = "extension Q { func helper() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "A.f");
    assert!(!has(&u, "Q.helper"), "{u:?}");
}

#[test]
fn wrong_bare_call_in_a_static_method_binds_no_instance_extension_member() {
    let a = "struct A {\n static func make() { helper() }\n}\n";
    let b = "extension A { func helper() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "A.make");
    assert!(!has(&u, "A.helper"), "{u:?}");
}

// ============================================================ characterisation dumps

fn dump_to(label: &str, text: String) {
    let dest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tmp/dump")
        .join(label);
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(dest, text).unwrap();
}

#[test]
#[ignore]
fn dump_inline_swift_fixtures() {
    let label = std::env::var("EDGE_LABEL").unwrap_or_else(|_| "head".into());
    let mut out = String::new();
    let sets: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("view_chain", vec![("Sources/Lib/V.swift", USER_VIEW), ("Sources/App/Root.swift", SWIFTUI_FILE)]),
        ("view_direct", vec![
            ("Sources/Lib/V.swift", USER_VIEW),
            ("Sources/App/Root.swift", SWIFTUI_FILE),
            ("Sources/Lib/Host.swift", "struct Host {\n var v: any View\n func go() { v.badge() }\n}\n"),
        ]),
        ("run_method_same_name", vec![("Sources/M/A.swift",
            "struct S {\n var run: () -> Void\n func run(_ n: Int) {}\n}\nstruct T { func run() {} }\nfunc g(_ s: S) { s.run() }\n")]),
    ];
    for (name, files) in sets {
        out.push_str(&format!("=== {name}\n"));
        for line in all_edges(&files) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    dump_to(&format!("inline-{label}.txt"), out);
}

#[test]
#[ignore]
fn dump_accuracy_fixture_edges() {
    use devmap_extract::extract_tree;
    let label = std::env::var("EDGE_LABEL").unwrap_or_else(|_| "head".into());
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy");
    let mut out = String::new();
    let mut dirs: Vec<_> = std::fs::read_dir(&fixtures)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let Ok(extractions) = extract_tree(&dir) else {
            out.push_str(&format!("=== {name}: extract failed\n"));
            continue;
        };
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let result = resolver.resolve_all(&extractions).unwrap();
        let mut lines: Vec<String> = result.edges.iter().map(|e| format!("{e:?}")).collect();
        lines.sort();
        out.push_str(&format!("=== {name}: {} files, {} edges\n", extractions.len(), lines.len()));
        for l in lines {
            out.push_str(&l);
            out.push('\n');
        }
    }
    dump_to(&format!("fixtures-{label}.txt"), out);
}
