//! Closing adversarial probes for lane/swift-resolver-residue @ 5bdbc2f0.
//! Each test states the CORRECT behaviour; a failing test is a BREAK.
//! Prefix `wrong_` = a wrong edge, `recall_` = a missing edge, `cost_` = a
//! recorded false abstention.  `dump_swift_fixture_edges` is a characterisation
//! tool (run with `-- --ignored`, `SWR2_DUMP=<file>`), not an assertion.

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

// ============================================================ 1. inferred / computed / static properties

#[test]
fn wrong_inferred_stored_properties_are_not_callable() {
    let src = "struct C {\n var padding = 4.0\n let opacity = 0.5\n}\nfunc f(c: C) {\n c.padding(4)\n c.opacity(0.5)\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.padding"), "{u:?}");
    assert!(!has(&u, "C.opacity"), "{u:?}");
}

#[test]
fn wrong_inferred_int_count_property_is_not_callable() {
    let src = "struct S {\n var count = 0\n}\nfunc f(s: S) {\n s.count()\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "S.count"), "{u:?}");
}

#[test]
fn wrong_lazy_inferred_property_is_not_callable() {
    let src = "struct X {\n var items: [Int] = []\n lazy var first = items.first\n}\nfunc f(x: X) {\n x.first(where: { _ in true })\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "X.first"), "{u:?}");
}

#[test]
fn wrong_computed_some_view_property_is_not_callable() {
    let src = "struct C {\n var padding: some View { Text(\"\") }\n}\nfunc f(c: C) {\n c.padding(4)\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.padding"), "{u:?}");
}

#[test]
fn wrong_computed_string_property_is_not_callable() {
    let src = "struct C {\n var tag: String { \"\" }\n}\nfunc f(c: C) {\n c.tag()\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.tag"), "{u:?}");
}

#[test]
fn wrong_static_inferred_singleton_is_not_callable() {
    let src = "struct Foo {\n static let shared = Foo()\n}\nfunc f() {\n Foo.shared()\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "Foo.shared"), "{u:?}");
}

// ============================================================ 2. typealias / collection / tuple / optional

#[test]
fn wrong_typealias_of_a_non_function_property_is_not_callable() {
    let src = "typealias Pad = CGFloat\nstruct C {\n var padding: Pad\n}\nfunc f(c: C) {\n c.padding(4)\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.padding"), "{u:?}");
}

#[test]
fn wrong_array_property_is_not_callable() {
    let src = "struct C {\n var rows: [Int]\n}\nfunc f(c: C) {\n c.rows(1)\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.rows"), "{u:?}");
}

#[test]
fn wrong_dictionary_property_is_not_callable() {
    let src = "struct C {\n var table: [String: Int]\n}\nfunc f(c: C) {\n c.table(\"a\")\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.table"), "{u:?}");
}

#[test]
fn wrong_tuple_property_is_not_callable() {
    let src = "struct C {\n var origin: (Int, Int)\n}\nfunc f(c: C) {\n c.origin(1)\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.origin"), "{u:?}");
}

#[test]
fn wrong_optional_nominal_property_is_not_callable() {
    let src = "struct C {\n var padding: CGFloat?\n}\nfunc f(c: C) {\n c.padding(4)\n}\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "C.padding"), "{u:?}");
}

// ============================================================ 3. recall that must hold

#[test]
fn recall_plain_closure_property() {
    let src = "struct C {\n var onTap: () -> Void\n}\nfunc f(c: C) {\n c.onTap()\n}\n";
    assert!(has(&uses(&one(src), "f"), "C.onTap"));
}

#[test]
fn recall_optional_closure_property() {
    let src = "struct C {\n var cb: (() -> Void)?\n}\nfunc f(c: C) {\n c.cb?()\n}\n";
    assert!(has(&uses(&one(src), "f"), "C.cb"));
}

#[test]
fn recall_typealias_of_closure_property() {
    let src = "typealias Handler = () -> Void\nstruct C {\n var h: Handler\n}\nfunc f(c: C) {\n c.h()\n}\n";
    assert!(has(&uses(&one(src), "f"), "C.h"));
}

#[test]
fn recall_escaping_stored_closure_property() {
    let src = "struct C {\n let h: () -> Void\n init(h: @escaping () -> Void) { self.h = h }\n}\nfunc f(c: C) {\n c.h()\n}\n";
    assert!(has(&uses(&one(src), "f"), "C.h"));
}

#[test]
fn recall_protocol_requirement_closure_property() {
    let src = "protocol P {\n var make: () -> Int { get }\n}\nfunc f(p: P) {\n _ = p.make()\n}\n";
    assert!(has(&uses(&one(src), "f"), "P.make"));
}

#[test]
fn recall_unannotated_closure_literal_property() {
    // `var go = { print(1) }` is callable; no type is written.
    let src = "struct C {\n var go = { print(1) }\n}\nfunc f(c: C) {\n c.go()\n}\n";
    assert!(has(&uses(&one(src), "f"), "C.go"));
}

// ============================================================ 4. -> Self

const SELF_P: &str = "protocol P {}\nextension P {\n func a() -> Self { self }\n func tag() {}\n}\n";

#[test]
fn recall_self_chain_without_override_binds_the_extension_default() {
    let src = format!("{SELF_P}struct B: P {{}}\nfunc f(b: B) {{ b.a().tag() }}\n");
    assert!(has(&uses(&one(&src), "f"), "P.tag"));
}

#[test]
fn wrong_self_chain_on_an_unknown_receiver_binds_nothing() {
    let src = format!("{SELF_P}func f(x: Mystery) {{ x.a().tag() }}\n");
    let u = uses(&one(&src), "f");
    assert!(!has(&u, "P.tag"), "{u:?}");
}

#[test]
fn wrong_two_self_steps_keep_the_concrete_override() {
    let src = format!("{SELF_P}struct B: P {{ func tag() {{}} }}\nfunc f(b: B) {{ b.a().a().tag() }}\n");
    let u = uses(&one(&src), "f");
    assert!(has(&u, "B.tag"), "{u:?}");
    assert!(!has(&u, "P.tag"), "{u:?}");
}

#[test]
fn wrong_three_self_steps_keep_the_concrete_override() {
    let src = format!("{SELF_P}struct B: P {{ func tag() {{}} }}\nfunc f(b: B) {{ b.a().a().a().tag() }}\n");
    let u = uses(&one(&src), "f");
    assert!(has(&u, "B.tag"), "{u:?}");
    assert!(!has(&u, "P.tag"), "{u:?}");
}

#[test]
fn wrong_self_in_a_class_hierarchy_is_the_subclass() {
    let src = format!(
        "{SELF_P}class Base: P {{ func tag() {{}} }}\nclass Sub: Base {{ override func tag() {{}} }}\nfunc f(s: Sub) {{ s.a().tag() }}\n"
    );
    let u = uses(&one(&src), "f");
    assert!(has(&u, "Sub.tag"), "{u:?}");
    assert!(!has(&u, "Base.tag"), "{u:?}");
    assert!(!has(&u, "P.tag"), "{u:?}");
}

#[test]
fn wrong_self_from_a_class_extension_is_the_subclass() {
    let src = "class Base {\n func tag() {}\n}\nextension Base {\n func me() -> Self { self }\n}\nclass Sub: Base {\n override func tag() {}\n}\nfunc f(s: Sub) { s.me().tag() }\n";
    let u = uses(&one(src), "f");
    assert!(has(&u, "Sub.tag"), "{u:?}");
    assert!(!has(&u, "Base.tag"), "{u:?}");
}

#[test]
fn wrong_self_chain_through_an_optional_receiver_keeps_the_override() {
    let src = format!("{SELF_P}struct B: P {{ func tag() {{}} }}\nfunc f(b: B?) {{ b?.a().tag() }}\n");
    let u = uses(&one(&src), "f");
    assert!(!has(&u, "P.tag"), "{u:?}");
}

// ============================================================ 5. generic returns

#[test]
fn wrong_generic_type_param_return_does_not_bind_a_same_named_type_in_another_file() {
    let a = "struct Box<T> {\n func get() -> T { fatalError() }\n}\nfunc f(b: Box<Int>) { b.get().m() }\n";
    let b = "struct T { func m() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "f");
    assert!(!has(&u, "T.m"), "{u:?}");
}

#[test]
fn wrong_generic_method_param_return_does_not_bind_a_same_named_type_in_another_file() {
    let a = "struct S {\n func id<Item>(_ x: Item) -> Item { x }\n}\nfunc f(s: S) { s.id(3).m() }\n";
    let b = "struct Item { func m() {} }\n";
    let r = resolve(&[("Sources/M/A.swift", a), ("Sources/M/B.swift", b)]);
    let u = uses(&r, "f");
    assert!(!has(&u, "Item.m"), "{u:?}");
}

#[test]
fn wrong_generic_param_shadowing_a_type_in_the_same_file_binds_nothing() {
    let src = "struct Item { func m() {} }\nstruct S {\n func id<Item>(_ x: Item) -> Item { x }\n}\nfunc f(s: S) { s.id(3).m() }\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "Item.m"), "{u:?}");
}

#[test]
fn cost_generic_container_return_still_binds_the_container() {
    // LOW: record the false-abstention cost; `-> Box<T>` names the real type Box.
    let src = "struct Box<T> {\n func me() -> Box<T> { self }\n func m() {}\n}\nfunc f(b: Box<Int>) { b.me().m() }\n";
    let u = uses(&one(src), "f");
    assert!(has(&u, "Box.m"), "{u:?}");
}

#[test]
fn cost_generic_method_returning_a_real_wrapper_still_binds() {
    let src = "struct Box<T> {\n func m() {}\n}\nstruct S {\n func wrap<U>(_ u: U) -> Box<U> { Box() }\n}\nfunc f(s: S) { s.wrap(1).m() }\n";
    let u = uses(&one(src), "f");
    assert!(has(&u, "Box.m"), "{u:?}");
}

#[test]
fn cost_non_generic_return_of_a_real_type_still_binds_next_to_a_generic_sibling() {
    let src = "struct Snap { func m() {} }\nstruct S<T> {\n func snap() -> Snap { Snap() }\n}\nfunc f(s: S<Int>) { s.snap().m() }\n";
    let u = uses(&one(src), "f");
    assert!(has(&u, "Snap.m"), "{u:?}");
}

// ============================================================ 6. qualified nested types

#[test]
fn recall_a_inner_and_b_inner_each_bind_their_own() {
    let src = "struct A { struct Inner { func m() {} } }\nstruct B { struct Inner { func m() {} } }\nfunc fa() { var x = A.Inner(); x.m() }\nfunc fb() { var x = B.Inner(); x.m() }\n";
    let r = one(src);
    let ua = uses(&r, "fa");
    let ub = uses(&r, "fb");
    assert!(has(&ua, "A.Inner.m") && !has(&ua, "B.Inner.m"), "{ua:?}");
    assert!(has(&ub, "B.Inner.m") && !has(&ub, "A.Inner.m"), "{ub:?}");
}

#[test]
fn recall_qualified_nested_annotation_binds_its_own() {
    let src = "struct A { struct Inner { func m() {} } }\nstruct B { struct Inner { func m() {} } }\nfunc f(x: B.Inner) { x.m() }\n";
    let u = uses(&one(src), "f");
    assert!(has(&u, "B.Inner.m") && !has(&u, "A.Inner.m"), "{u:?}");
}

#[test]
fn wrong_qualified_through_a_type_that_does_not_nest_it_binds_nothing() {
    // A has no Inner; B.Inner must not answer for A.Inner.
    let src = "struct A {}\nstruct B { struct Inner { func m() {} } }\nfunc f() { var x = A.Inner(); x.m() }\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "B.Inner.m"), "{u:?}");
}

#[test]
fn wrong_bare_inner_outside_any_outer_binds_nothing() {
    let src = "struct A { struct Inner { func m() {} } }\nstruct B { struct Inner { func m() {} } }\nfunc f() { var x = Inner(); x.m() }\n";
    let u = uses(&one(src), "f");
    assert!(!has(&u, "A.Inner.m") && !has(&u, "B.Inner.m"), "{u:?}");
}

#[test]
fn recall_bare_inner_inside_its_outer_binds_that_outer() {
    let src = "struct A {\n struct Inner { func m() {} }\n func g() { var x = Inner(); x.m() }\n}\nstruct B { struct Inner { func m() {} } }\n";
    let u = uses(&one(src), "A.g");
    assert!(has(&u, "A.Inner.m") && !has(&u, "B.Inner.m"), "{u:?}");
}

#[test]
fn recall_deeply_nested_qualified_climbs_to_its_outer() {
    let src = "struct O { struct Mid { struct Deep { func f() {} } } }\nstruct Q { struct Deep { func f() {} } }\nfunc g() { var d = O.Mid.Deep(); d.f() }\n";
    let u = uses(&one(src), "g");
    assert!(has(&u, "Deep.f") && !has(&u, "Q.Deep.f"), "{u:?}");
}

#[test]
fn wrong_bare_deep_outside_binds_nothing() {
    let src = "struct O { struct Mid { struct Deep { func f() {} } } }\nstruct Q { struct Deep { func f() {} } }\nfunc g() { var d = Deep(); d.f() }\n";
    let u = uses(&one(src), "g");
    assert!(!has(&u, "Deep.f"), "{u:?}");
}

// ============================================================ 7. chain receiver: no bare-name fan-out

#[test]
fn wrong_chain_receiver_never_fans_out_to_unrelated_protocol_conformers() {
    let src = "protocol View {}\nextension View {\n func badge() {}\n}\nstruct Mine: View {}\nstruct Other {\n func badge() {}\n func pad() -> Other { self }\n}\nfunc f(o: Other) { o.pad().badge() }\n";
    let u = uses(&one(src), "f");
    assert!(has(&u, "Other.badge"), "{u:?}");
    assert!(!has(&u, "View.badge"), "{u:?}");
}

// ============================================================ characterisation

#[test]
#[ignore]
fn dump_swift_fixture_edges() {
    use std::fmt::Write as _;
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut ents: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        ents.sort_by_key(|e| e.path());
        for e in ents {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().map(|x| x == "swift").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/accuracy/swift");
    let mut text = String::new();
    for sub in ["micro", "spm", "xcode"] {
        let mut files = Vec::new();
        walk(&root.join(sub), &mut files);
        let owned: Vec<(String, String)> = files
            .iter()
            .map(|p| {
                let rel = p.strip_prefix(&root).unwrap().to_string_lossy().to_string();
                (rel, std::fs::read_to_string(p).unwrap())
            })
            .collect();
        let refs: Vec<(&str, &str)> = owned.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let r = resolve(&refs);
        for e in &r.edges {
            let kind = e
                .resolution
                .as_ref()
                .map(|x| format!("{:?}", x).chars().take(40).collect::<String>())
                .unwrap_or_default();
            writeln!(
                text,
                "{:?} {} -> {} [{:?}] {}",
                e.edge_kind, e.source_symbol, e.target_symbol, e.confidence, kind
            )
            .unwrap();
        }
    }
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort();
    std::fs::write(std::env::var("SWR2_DUMP").unwrap(), lines.join("\n")).unwrap();
}
