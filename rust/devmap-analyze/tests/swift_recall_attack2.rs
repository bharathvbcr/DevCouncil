//! Second-round adversarial cases for Swift closure-parameter and receiver
//! typing (head e8cdc73f). Same rule as round one: an abstention passes, a
//! confident (>= 0.9) edge to the wrong symbol is the defect.

use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

const HIGH: i64 = 900;

fn confident(files: &[(&str, &str)], caller: &str) -> Vec<String> {
    let extractions: Vec<_> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).expect("resolve");
    let mut out: Vec<String> = resolution
        .edges
        .iter()
        .filter(|e| {
            !matches!(
                e.edge_kind,
                EdgeKind::Contains | EdgeKind::MemberOf | EdgeKind::Imports | EdgeKind::Defines
            ) && e.source_symbol.ends_with(caller)
                && e.confidence.to_millis() >= HIGH
        })
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

fn has(targets: &[String], needle: &str) -> bool {
    targets.iter().any(|t| t.ends_with(needle))
}

const ITEM: (&str, &str) = (
    "item.swift",
    "struct Item { var id: Tag\n func weight() -> Int { 1 }\n func append(_ x: Int) {}\n func contains(_ x: Int) -> Bool { true } }\n",
);
const TAG: (&str, &str) = ("tag.swift", "struct Tag { func weight() -> Int { 9 }\n func append(_ x: Int) {} }\n");
const OTHER: (&str, &str) = (
    "other.swift",
    "struct Other { func weight() -> Int { 2 }\n func append(_ x: Int) {}\n func contains(_ x: Int) -> Bool { false } }\n",
);

fn run(host: &str, extra: &[(&str, &str)]) -> Vec<String> {
    let mut files = vec![ITEM, TAG, OTHER];
    files.extend_from_slice(extra);
    files.push(("host.swift", host));
    confident(&files, "Host.run")
}

#[test]
fn control_collections_still_resolve() {
    let got = run(
        "struct Host { var items: [Item]\n func run() { items.forEach { $0.weight() } } }\n",
        &[],
    );
    assert!(has(&got, "Item.weight"), "{got:?}");
}

#[test]
fn a_superclass_member_beats_a_global_of_the_same_name() {
    let base = ("base.swift", "class Base { var items: [Item] = [] }\n");
    let got = run(
        "var items: [Other] = []\nclass Host: Base { func run() { items.forEach { $0.weight() } } }\n",
        &[base],
    );
    assert!(!has(&got, "Other.weight"), "inherited items is [Item]: {got:?}");
}

#[test]
fn a_subclass_in_another_file_member_is_not_typed_from_a_sibling_type() {
    let base = ("base.swift", "class Base { var items: [Item] = [] }\n");
    let sibling = ("sib.swift", "class Sibling { var items: [Other] = [] }\n");
    let got = run(
        "class Host: Base { func run() { items.forEach { $0.weight() } } }\n",
        &[base, sibling],
    );
    assert!(!has(&got, "Other.weight"), "{got:?}");
}

#[test]
fn reduce_accumulator_is_not_the_element() {
    let got = run(
        "struct Host { var items: [Item]\n func run() { _ = items.reduce(into: Other()) { $0.append(1) }; _ = items.reduce(Other()) { $0.append(1); return $0 } } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append"), "$0 is the accumulator: {got:?}");
}

#[test]
fn sorted_by_and_flat_map_and_compact_map_keep_one_element_type() {
    let got = run(
        "struct Host { var items: [Item]\n func run() { _ = items.sorted(by: { $0.weight() < $1.weight() }); _ = items.compactMap { $0.id }; _ = items.flatMap { [$0.id] } } }\n",
        &[],
    );
    assert!(!has(&got, "Other.weight"), "{got:?}");
}

#[test]
fn a_first_where_chain_is_not_an_element_closure() {
    let got = run(
        "struct Host { var items: [Item]\n func run() { _ = items.first(where: { $0.weight() > 1 }).map { $0.id.append(1) }; _ = items.first.map { $0.append(1) } } }\n",
        &[],
    );
    // `.map` on an Optional<Item> hands over the Item; `$0.id` is a Tag. Item.append would be wrong.
    assert!(!has(&got, "Other.append"), "{got:?}");
}

#[test]
fn for_each_after_optional_chain_is_typed_by_the_collection() {
    let got = run(
        "struct Pouch { var items: [Item] }\nstruct Host { var pouch: Pouch?\n func run() { pouch?.items.forEach { $0.weight() } } }\n",
        &[],
    );
    assert!(!has(&got, "Other.weight"), "{got:?}");
}

#[test]
fn a_nested_array_outer_closure_receives_an_array() {
    let got = run(
        "struct Host { var groups: [[Item]]\n func run() { groups.map { $0.append(1) } } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append"), "$0 is [Item], Array.append: {got:?}");
}

#[test]
fn dictionary_values_and_pairs_are_not_elements() {
    let got = run(
        "struct Host { var d: [String: [Item]]\n func run() { d.values.map { $0.append(1) }; d.map { $0.append(1) } } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append"), "{got:?}");
    let got = run(
        "struct Host { var d: [String: Item]\n func run() { d.map { $0.append(1) } } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append"), "$0 is a (key, value) pair: {got:?}");
}

#[test]
fn a_set_member_is_not_typed_as_an_array() {
    let got = run(
        "struct Host { var s: Set<Other>\n func run() { s.forEach { $0.weight() } } }\n",
        &[],
    );
    assert!(!has(&got, "Item.weight"), "{got:?}");
}

#[test]
fn a_projected_property_wrapper_value_is_not_the_collection() {
    for decl in ["@Binding var items: [Item]", "@Published var items: [Item]", "@State var items: [Item]"] {
        let src = format!(
            "struct Host {{ {decl}\n func run() {{ $items.map {{ $0.append(1) }} }} }}\n"
        );
        let got = run(&src, &[]);
        assert!(!has(&got, "Item.append"), "{decl}: {got:?}");
    }
}

#[test]
fn a_query_collection_is_typed_and_not_confused() {
    let got = run(
        "struct Host { @Query private var items: [Item]\n func run() { items.forEach { $0.weight() } } }\n",
        &[],
    );
    assert!(!has(&got, "Other.weight"), "{got:?}");
}

#[test]
fn closures_in_a_tuple_of_function_types_are_typed_by_position() {
    let got = run(
        "struct Host { func run() { let pair: (a: (Item) -> Void, b: (Other) -> Void) = ({ $0.contains(1) }, { $0.append(1) }); _ = pair } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append"), "second closure takes Other: {got:?}");
}

#[test]
fn a_two_parameter_function_type_types_dollar_one_as_the_second() {
    let got = run(
        "struct Host { func run() { let f: (Item, Other) -> Void = { _, _ in }; let g: (Item, Other) -> Void = { $1.append(1) }; _ = (f, g) } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append"), "{got:?}");
}

#[test]
fn try_await_before_a_construction_does_not_change_other_receivers() {
    let svc = ("svc.swift", "struct Svc { init(a: Int) {}\n func run() {} }\nfunc helper(a: Int) -> Other { Other() }\n");
    let got = run(
        "struct Host { func run() async throws {\n try await Svc(\n a: 1\n ).run()\n try? Svc(\n a: 1\n ).run()\n _ = try helper(\n a: 1\n ).append(1)\n } }\n",
        &[svc],
    );
    assert!(has(&got, "Svc.run"), "construction still resolves: {got:?}");
    assert!(!has(&got, "Item.append") && !has(&got, "Tag.append"), "{got:?}");
}

#[test]
fn await_of_a_method_chain_keeps_its_receiver() {
    let client = (
        "client.swift",
        "struct Client { func fetch(a: Int) async -> Other { Other() } }\n",
    );
    let got = run(
        "struct Host { var client: Client\n func run() async { _ = await client.fetch(\n a: 1\n ).append(1) } }\n",
        &[client],
    );
    assert!(!has(&got, "Item.append") && !has(&got, "Tag.append"), "{got:?}");
}

#[test]
fn deeply_nested_namesake_types_are_told_apart_by_path() {
    let src = (
        "nest.swift",
        "struct A { struct B { struct Crate { var items: [Item] } } }\n\
         struct X { struct B { struct Crate { var items: [Other] } } }\n\
         extension A.B.Crate { func runA() { items.forEach { $0.contains(1) } } }\n\
         extension X.B.Crate { func runX() { items.forEach { $0.contains(1) } } }\n",
    );
    let files = [ITEM, TAG, OTHER, src];
    let a = confident(&files, "Crate.runA");
    let x = confident(&files, "Crate.runX");
    eprintln!("runA {a:?} runX {x:?}");
    assert!(!has(&a, "Other.contains"), "A.B.Crate.items is [Item]: {a:?}");
    assert!(!has(&x, "Item.contains"), "X.B.Crate.items is [Other]: {x:?}");
}

#[test]
fn an_extension_of_a_nested_type_in_another_file_is_not_read_from_a_namesake_here() {
    let decl = ("a.swift", "struct A { struct Crate { var items: [Item] } }\n");
    let other = (
        "b.swift",
        "struct B { struct Crate { var items: [Other] } }\n",
    );
    let ext = (
        "c.swift",
        "extension A.Crate { func run() { items.forEach { $0.contains(1) } } }\n",
    );
    let got = confident(&[ITEM, TAG, OTHER, decl, other, ext], "Crate.run");
    assert!(!has(&got, "Other.contains"), "{got:?}");
}

#[test]
fn an_extension_of_a_nested_type_declared_in_an_extension_is_typed_by_its_own_path() {
    let decl = (
        "a.swift",
        "struct A {}\nextension A { struct Crate { var items: [Item] } }\n\
         struct B { struct Crate { var items: [Other] } }\n\
         extension A.Crate { func run() { items.forEach { $0.contains(1) } } }\n",
    );
    let got = confident(&[ITEM, TAG, OTHER, decl], "Crate.run");
    assert!(!has(&got, "Other.contains"), "{got:?}");
}

#[test]
fn a_typealias_declared_elsewhere_is_not_a_namesake_nested_type() {
    let alias = ("alias.swift", "typealias Row = Other\n");
    let nested = ("nested.swift", "struct Foo { struct Row { func weight() -> Int { 5 } } }\n");
    let got = run(
        "struct Host { var rows: [Row]\n func run() { rows.forEach { $0.weight() } } }\n",
        &[alias, nested],
    );
    assert!(!has(&got, "Foo.Row.weight"), "Row is Other here: {got:?}");
}

#[test]
fn a_collection_typealias_member_types_nothing_wrong() {
    let got = run(
        "struct Host { typealias Rows = [Other]\n var rows: Rows\n func run() { rows.forEach { $0.weight() } } }\n",
        &[],
    );
    assert!(!has(&got, "Item.weight"), "{got:?}");
}

#[test]
fn nil_coalescing_with_a_dictionary_default_is_not_a_collection() {
    let got = run(
        "struct Host { var table: [String: Item]?\n func run() { (table ?? [:]).map { $0.append(1) } } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append"), "$0 is a pair: {got:?}");
}

#[test]
fn nil_coalescing_with_a_named_default_keeps_the_left_type() {
    let got = run(
        "struct Host { var items: [Item]?\n var fallback: [Item]\n func run() { (items ?? fallback).forEach { $0.weight() } } }\n",
        &[],
    );
    assert!(!has(&got, "Other.weight"), "{got:?}");
}

#[test]
fn a_typealias_in_an_extension_in_another_file_is_not_the_top_level_type() {
    let entry = ("entry.swift", "struct Entry { func weight() -> Int { 3 } }\n");
    let ext = ("hostext.swift", "extension Host { typealias Entry = Item }\n");
    let got = run(
        "struct Host { var items: [Entry]\n func run() { items.forEach { $0.weight() } } }\n",
        &[entry, ext],
    );
    assert!(!has(&got, "Entry.weight"), "Entry is Item inside Host: {got:?}");
}

#[test]
fn a_generic_nesting_member_is_not_read_through_the_outer_generic() {
    let got = run(
        "struct Host { struct Box<T> { var items: [T] }\n var box: Box<Item>\n func run() { box.items.forEach { $0.weight() } } }\n",
        &[],
    );
    assert!(!has(&got, "Other.weight"), "{got:?}");
}

#[test]
fn nil_coalescing_on_a_non_collection_left_types_nothing() {
    let got = run(
        "struct Host { var tag: Tag?\n func run() { (tag ?? Tag()).append(1); (tag ?? Tag()).id } }\n",
        &[],
    );
    assert!(!has(&got, "Item.append") && !has(&got, "Other.append"), "{got:?}");
}
