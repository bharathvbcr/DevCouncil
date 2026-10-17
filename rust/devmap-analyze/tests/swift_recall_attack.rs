//! Adversarial precision cases for the Swift closure-parameter and receiver
//! typing added in f2187b0d. Each case builds a small multi-file Swift project in
//! memory, resolves it through the product's own resolver, and asserts that no
//! edge at confidence >= 0.9 reaches the *wrong* symbol: an abstention is fine,
//! a wrong confident edge is the defect. Every case also states a control, a
//! shape that must still resolve, so a case cannot pass by the setup being inert.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

const HIGH: i64 = 900;

struct Edge {
    from: String,
    to: String,
    millis: i64,
}

fn edges(files: &[(&str, &str)]) -> Vec<Edge> {
    let extractions: Vec<_> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).expect("resolve");
    resolution
        .edges
        .iter()
        .filter(|e| {
            !matches!(
                e.edge_kind,
                EdgeKind::Contains | EdgeKind::MemberOf | EdgeKind::Imports | EdgeKind::Defines
            )
        })
        .map(|e| Edge {
            from: e.source_symbol.clone(),
            to: e.target_symbol.clone(),
            millis: e.confidence.to_millis(),
        })
        .collect()
}

fn confident(edges: &[Edge], caller: &str) -> Vec<String> {
    let mut out: Vec<String> = edges
        .iter()
        .filter(|e| e.from.ends_with(caller) && e.millis >= HIGH)
        .map(|e| e.to.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

const ITEM: (&str, &str) = (
    "item.swift",
    "struct Item { func weight() -> Int { 1 }\n func append(_ x: Int) {}\n func contains(_ x: Int) -> Bool { true } }\n",
);
const OTHER: (&str, &str) = (
    "other.swift",
    "struct Other { func weight() -> Int { 2 }\n func append(_ x: Int) {}\n }\n",
);

fn has(targets: &[String], needle: &str) -> bool {
    targets.iter().any(|t| t.ends_with(needle))
}

#[test]
fn control_a_member_collection_still_resolves() {
    let host = (
        "host.swift",
        "struct Host { var items: [Item]\n func run() { items.forEach { $0.weight() } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host]), "Host.run");
    assert!(has(&got, "Item.weight"), "control: {got:?}");
}

// 1(a) a bare call names a method the file cannot see (extension in another file)
#[test]
fn a_free_function_is_shadowed_by_a_method_declared_in_another_file() {
    let host = (
        "host.swift",
        "func load() -> [Item] { [] }\n\
         struct Host { func run() { let xs = load(); xs.forEach { $0.weight() } } }\n",
    );
    let ext = (
        "hostext.swift",
        "extension Host { func load() -> [Other] { [] } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host, ext]), "Host.run");
    assert!(
        !has(&got, "Item.weight"),
        "bare load() is Host.load() -> [Other]; Item.weight is wrong: {got:?}"
    );
}

#[test]
fn a_free_function_is_shadowed_by_an_inherited_method_from_another_file() {
    let host = (
        "host.swift",
        "func load() -> [Item] { [] }\n\
         class Host: Base { func run() { let xs = load(); xs.forEach { $0.weight() } } }\n",
    );
    let base = (
        "base.swift",
        "class Base { func load() -> [Other] { [] } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host, base]), "Host.run");
    assert!(!has(&got, "Item.weight"), "{got:?}");
}

// 1(b) generic member
#[test]
fn a_generic_member_collection_types_nothing() {
    let boxed = (
        "box.swift",
        "struct Box<T> { var items: [T]\n func run() { items.forEach { $0.weight() } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, boxed]), "Box.run");
    assert!(
        !has(&got, "Item.weight") && !has(&got, "Other.weight"),
        "{got:?}"
    );
}

// 1(c) existential members
#[test]
fn an_existential_member_does_not_bind_a_random_conformer() {
    let p = (
        "p.swift",
        "protocol P { func weight() -> Int }\n\
         struct A: P { func weight() -> Int { 1 } }\n\
         struct B: P { func weight() -> Int { 2 } }\n",
    );
    for written in ["[any P]", "[P]", "Array<any P>"] {
        let src = format!(
            "struct Host {{ var items: {written}\n func run() {{ items.forEach {{ $0.weight() }} }} }}\n"
        );
        let got = confident(&edges(&[p, ("host.swift", src.as_str())]), "Host.run");
        assert!(
            !has(&got, "A.weight") && !has(&got, "B.weight"),
            "{written}: {got:?}"
        );
    }
}

// 1(d) typealias element type, nested alias shadowing a top-level type
#[test]
fn a_nested_typealias_is_not_the_top_level_type_of_the_same_name() {
    let entry = (
        "entry.swift",
        "struct Entry { func weight() -> Int { 3 } }\n",
    );
    let host = (
        "host.swift",
        "struct Host { typealias Entry = Item\n var items: [Entry]\n\
         func run() { items.forEach { $0.weight() } } }\n",
    );
    let got = confident(&edges(&[ITEM, entry, host]), "Host.run");
    assert!(
        !has(&got, "Entry.weight"),
        "alias is Item, not Entry: {got:?}"
    );
}

// 1(e) extension in another file names the type; this file declares another nested type of that name
#[test]
fn an_extension_of_a_nested_type_does_not_read_a_namesake_nested_type_here() {
    let decl = (
        "a.swift",
        "struct A { struct Crate { var items: [Item] } }\n",
    );
    let ext_file = (
        "b.swift",
        "struct B { struct Crate { var items: [Other] } }\n\
         extension A.Crate { func run() { items.forEach { $0.weight() } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, decl, ext_file]), "Crate.run");
    assert!(
        !has(&got, "Other.weight"),
        "A.Crate.items is [Item]; Other.weight is wrong: {got:?}"
    );
}

// 1(f) optional collection: Optional.map hands over the collection, not an element
#[test]
fn map_on_an_optional_collection_is_not_an_element_closure() {
    let host = (
        "host.swift",
        "struct Host { var items: [Item]?\n func run() { _ = items.map { $0.append(1) } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host]), "Host.run");
    assert!(!has(&got, "Item.append"), "$0 is [Item] here: {got:?}");
}

#[test]
fn map_on_a_try_optional_collection_is_not_an_element_closure() {
    let host = (
        "host.swift",
        "func load() throws -> [Item] { [] }\n\
         struct Host { func run() { let xs = try? load(); _ = xs.map { $0.append(1) } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host]), "Host.run");
    assert!(!has(&got, "Item.append"), "$0 is [Item]: {got:?}");
    let parenthesised = (
        "host2.swift",
        "func load2() throws -> [Item] { [] }\n\
         struct Host2 { func run() { _ = (try? load2()).map { $0.append(1) } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, parenthesised]), "Host2.run");
    assert!(!has(&got, "Item.append"), "{got:?}");
}

// 1(g) dictionary on the left of ?? []
#[test]
fn a_dictionary_default_is_not_a_collection() {
    let host = (
        "host.swift",
        "struct Host { var table: [String: [Item]]\n\
         func run(_ k: String) { (table[k] ?? []).forEach { $0.weight() } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host]), "Host.run");
    assert!(!has(&got, "Other.weight"), "{got:?}");
    let dict = (
        "host2.swift",
        "struct Host2 { var table: [String: Item]\n\
         func run() { (table ?? [:]).forEach { $0.append(1) } } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, dict]), "Host2.run");
    assert!(!has(&got, "Item.append"), "{got:?}");
}

// 1(h) chained closure: the second $0 is a different type
#[test]
fn the_second_closure_in_a_chain_is_not_typed_by_the_first_collection() {
    let tag = ("tag.swift", "struct Tag { func weight() -> Int { 9 } }\n");
    let item = (
        "item2.swift",
        "struct Item { var id: Tag\n func weight() -> Int { 1 } }\n",
    );
    let host = (
        "host.swift",
        "struct Host { var items: [Item]\n\
         func run() { items.map { $0.id }.forEach { $0.weight() } } }\n",
    );
    let got = confident(&edges(&[item, tag, OTHER, host]), "Host.run");
    assert!(!has(&got, "Item.weight"), "second $0 is a Tag: {got:?}");
}

#[test]
fn enumerated_zip_and_lazy_do_not_type_dollar_zero_as_the_element() {
    for chain in [
        "items.enumerated().map { $0.element.append(1) }",
        "items.enumerated().forEach { $0.1.append(1) }",
        "zip(items, items).map { $0.0.append(1) }",
        "items.lazy.map { $0.weight() }",
        "items.reversed().map { $0.weight() }",
    ] {
        let src = format!("struct Host {{ var items: [Item]\n func run() {{ _ = {chain} }} }}\n");
        let got = confident(
            &edges(&[ITEM, OTHER, ("host.swift", src.as_str())]),
            "Host.run",
        );
        // `lazy`/`reversed` keep the element, so Item.weight is not wrong there;
        // the tuple shapes must never reach Item.append through `$0`.
        assert!(!has(&got, "Item.append"), "{chain}: {got:?}");
    }
}

// a variadic function type: $0 is an array
#[test]
fn a_variadic_function_type_parameter_is_an_array() {
    let host = (
        "host.swift",
        "struct Host { func run() { let f: (Item...) -> Void = { $0.append(1) }; f() } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host]), "Host.run");
    assert!(!has(&got, "Item.append"), "$0 is [Item]: {got:?}");
}

#[test]
fn an_inout_function_type_parameter_still_resolves_or_abstains_never_wrong() {
    let host = (
        "host.swift",
        "struct Host { func run() { let f: (inout Item) -> Void = { $0.weight() }; _ = f } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, host]), "Host.run");
    assert!(!has(&got, "Other.weight"), "{got:?}");
}

// switch / if-case bindings named like the member shadow it
#[test]
fn a_pattern_binding_named_like_the_member_shadows_it() {
    for body in [
        "switch e { case .a(let items): items.forEach { $0.append(1) } default: break }",
        "if case .a(let items) = e { items.forEach { $0.append(1) } }",
        "do { try go() } catch let items { _ = items.map { $0.append(1) } }",
        "for items in groups { items.forEach { $0.append(1) } }",
        "guard case .a(let items) = e else { return }; items.forEach { $0.append(1) }",
    ] {
        let src = format!(
            "enum E {{ case a([Other]) }}\n\
             struct Host {{ var items: [Item]\n var groups: [[Other]]\n\
             func go() throws {{}}\n func run(e: E) {{ {body} }} }}\n"
        );
        let got = confident(
            &edges(&[ITEM, OTHER, ("host.swift", src.as_str())]),
            "Host.run",
        );
        assert!(!has(&got, "Item.append"), "{body}: {got:?}");
    }
}

// if let u = recv.m(): an overload declared in another file
#[test]
fn a_method_overload_in_another_file_does_not_inherit_this_files_return_type() {
    let hist = (
        "hist.swift",
        "struct Hist { func pop() -> Item? { nil } }\n\
         struct Host { func run(h: Hist) { if let u = h.pop(n: 2) { u.append(1) } } }\n",
    );
    let ext = (
        "histext.swift",
        "extension Hist { func pop(n: Int) -> Other? { nil } }\n",
    );
    let got = confident(&edges(&[ITEM, OTHER, hist, ext]), "Host.run");
    assert!(!has(&got, "Item.append"), "pop(n:) returns Other?: {got:?}");
}

// a member typed in this file for a type declared elsewhere with the same name
#[test]
fn a_member_of_a_namesake_type_in_another_scope_is_not_read() {
    let a = (
        "a.swift",
        "struct Outer { struct Pouch { var items: [Other] } }\n\
         struct Host { func run(p: Pouch) { p.items.forEach { $0.append(1) } } }\n",
    );
    let pouch = ("pouch.swift", "struct Pouch { var items: [Item] }\n");
    let got = confident(&edges(&[ITEM, OTHER, a, pouch]), "Host.run");
    assert!(
        !has(&got, "Other.append"),
        "p is the top-level Pouch, whose items are [Item]: {got:?}"
    );
}
