//! Closing adversarial probes for 30a5eade (lane/swift-properties fix delta
//! 4f457e0e..30a5eade). Each test states the CORRECT behaviour; a failing test
//! is a BREAK. Prefix `wrong_` = a wrong edge, `recall_` = a missing edge,
//! `crash_` = a debug-build panic.

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
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out
}

fn has(u: &[String], target: &str) -> bool {
    u.iter().any(|t| t.ends_with(target))
}

// ---------------------------------------------------------------- #2 enum cases

#[test]
fn wrong_a_real_local_named_like_a_case_binds_nothing() {
    let r = one("enum E { case a }\nstruct S { var a = 1 }\nfunc f() -> Int { let a = 2; return a }\n");
    let u = uses(&r, "f");
    assert!(!has(&u, "S.a") && !has(&u, "E.a"), "{u:?}");
}

#[test]
fn wrong_a_bare_case_spelling_in_a_free_function_binds_no_case() {
    let r = one("enum E { case a }\nfunc g() -> Int { return a }\n");
    let u = uses(&r, "g");
    assert!(!has(&u, "E.a"), "{u:?}");
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97): a top-level `let` emits no symbol for the read to reach, ft-915b0a03"]
fn recall_a_global_named_like_a_case_still_binds_the_global() {
    let r = one("let circle = 1\nenum Shape { case circle }\nfunc g() -> Int { circle }\n");
    let u = uses(&r, "g");
    assert!(has(&u, "circle") && !has(&u, "Shape.circle"), "{u:?}");
}

#[test]
fn wrong_a_property_initializer_reading_a_name_shared_with_a_case_binds_the_property() {
    let r = one("enum E { case a }\nstruct S { var a = 1\n var b = a }\n");
    let u = uses(&r, "S.b");
    let u2 = uses(&r, "b");
    assert!(!has(&u, "E.a") && !has(&u2, "E.a"), "{u:?} {u2:?}");
}

#[test]
fn wrong_a_local_in_a_property_initializer_closure_named_like_a_case() {
    let r = one("enum E { case a }\nstruct S { var a = 1\n var b: Int = { let a = 3; return a }() }\n");
    let u = uses(&r, "b");
    assert!(!has(&u, "E.a") && !has(&u, "S.a"), "{u:?}");
}

// ---------------------------------------------------------------- #3 dangling

#[test]
fn crash_dangling_target_shapes_do_not_panic() {
    let shapes: &[(&str, &[(&str, &str)])] = &[
        ("two types same property one file", &[("Sources/M/A.swift", "struct A { var x = 1\n func f() -> Int { x } }\nstruct B { var x = 2\n func g() -> Int { x } }\n")]),
        ("two types same property two files", &[("Sources/M/A.swift", "struct A { var x = 1\n func f() -> Int { x } }\n"), ("Sources/M/B.swift", "struct B { var x = 2\n func g() -> Int { x } }\n")]),
        ("extension in another file", &[("Sources/M/A.swift", "struct A { var x = 1 }\nstruct B { var x = 2 }\n"), ("Sources/M/C.swift", "extension A { func f() -> Int { x } }\nextension B { func g() -> Int { x } }\n")]),
        ("inherited same name two classes", &[("Sources/M/A.swift", "class P { var x = 1 }\nclass Q { var x = 2 }\nclass A: P { func f() -> Int { x } }\nclass B: Q { func g() -> Int { x } }\n")]),
        ("protocol extension default", &[("Sources/M/A.swift", "protocol P { var x: Int { get } }\nprotocol Q { var x: Int { get } }\nextension P { func f() -> Int { x } }\nextension Q { func g() -> Int { x } }\nstruct S: P, Q { var x = 1 }\n")]),
        ("closure capture self", &[("Sources/M/A.swift", "class A { var x = 1\n func f() -> () -> Int { { [self] in x } } }\nclass B { var x = 2\n func g() -> () -> Int { { [self] in x } } }\n")]),
        ("nested same member", &[("Sources/M/A.swift", "struct O { var x = 1\n struct I { var x = 2\n func f() -> Int { x } }\n func g() -> Int { x } }\n")]),
        ("generic boxes", &[("Sources/M/A.swift", "struct Box<T> { var v: T\n func f() -> T { v } }\nstruct Bag<T> { var v: T\n func g() -> T { v } }\n")]),
        ("global plus members", &[("Sources/M/A.swift", "let x = 0\nstruct A { var x = 1 }\nstruct B { var x = 2 }\nfunc h() -> Int { x }\n")]),
        ("method vs property read as value", &[("Sources/M/A.swift", "struct A { func size() -> Int { 1 } }\nstruct B { var size = 2 }\nfunc h(_ a: A, _ b: B) { let f = a.size; let g = b.size; _ = (f, g) }\n")]),
        ("unique global from other file", &[("Sources/M/A.swift", "var counter = 0\n"), ("Sources/M/B.swift", "struct A { var counter = 1 }\nstruct B { var counter = 2 }\n"), ("Sources/M/C.swift", "func h() -> Int { counter }\n")]),
        ("actor and enum members", &[("Sources/M/A.swift", "actor A { var x = 1\n func f() -> Int { x } }\nenum E { case a\n var x: Int { 1 }\n func g() -> Int { x } }\n")]),
        ("static members", &[("Sources/M/A.swift", "struct A { static var x = 1\n static func f() -> Int { x } }\nstruct B { static var x = 2\n static func g() -> Int { x } }\n")]),
        ("where clause extension", &[("Sources/M/A.swift", "struct A<T> { var x = 1 }\nextension A where T == Int { func f() -> Int { x } }\nstruct B { var x = 2\n func g() -> Int { x } }\n")]),
        ("overloaded methods same name as property elsewhere", &[("Sources/M/A.swift", "struct A { func x(_ i: Int) -> Int { i }\n func x(_ s: String) -> Int { 1 }\n func f() -> Int { x(1) } }\nstruct B { var x = 2\n func g() -> Int { x } }\n")]),
        ("same type declared in two files", &[("Sources/M/A.swift", "struct A { var x = 1 }\n"), ("Sources/N/A.swift", "struct A { var x = 1\n func f() -> Int { x } }\n")]),
        ("class hierarchy chain", &[("Sources/M/A.swift", "class R { var x = 1 }\nclass M: R {}\nclass L: M { func f() -> Int { x } }\nclass R2 { var x = 2 }\nclass L2: R2 { func g() -> Int { x } }\n")]),
        ("subscript and init", &[("Sources/M/A.swift", "struct A { var x = 1\n init() { self.x = x }\n subscript(i: Int) -> Int { x } }\nstruct B { var x = 2\n init() { x = 3 } }\n")]),
        ("lazy and observers", &[("Sources/M/A.swift", "struct A { lazy var y: Int = x\n var x = 1 { didSet { _ = x } } }\nstruct B { var x = 2 { willSet { _ = x } } }\n")]),
        ("typealias and extension same name", &[("Sources/M/A.swift", "typealias T = A\nstruct A { var x = 1 }\nextension T { func f() -> Int { x } }\nstruct B { var x = 1 }\n")]),
        ("enum case and property of different enums", &[("Sources/M/A.swift", "enum E { case circle }\nstruct F { static var circle = 1 }\nfunc h() { _ = E.circle; _ = F.circle }\n")]),
    ];
    let mut panicked = Vec::new();
    for (name, files) in shapes {
        let files: Vec<(&str, &str)> = files.to_vec();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = resolve(&files);
        }));
        if let Err(payload) = outcome {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            panicked.push(format!("{name}: {message}"));
        }
    }
    assert!(panicked.is_empty(), "{panicked:#?}");
}

#[test]
fn crash_more_dangling_target_shapes_do_not_panic() {
    let shapes: &[(&str, &[(&str, &str)])] = &[
        ("override property and super", &[("Sources/M/A.swift", "class A { var x = 1 }\nclass B: A { override var x: Int { get { super.x + 1 } set {} }\n func f() -> Int { self.x + super.x + x } }\nclass C { var x = 3\n func g() -> Int { x } }\n")]),
        ("Self and type(of:)", &[("Sources/M/A.swift", "struct A { static var x = 1\n var y = 2\n func f() -> Int { Self.x + type(of: self).x + y } }\nstruct B { static var x = 1\n var y = 2\n func g() -> Int { Self.x + y } }\n")]),
        ("property wrappers", &[("Sources/M/A.swift", "final class VM { @Published var x = 1\n var y: Int { x } }\nfinal class W { @Published var x = 2\n var y: Int { x } }\n")]),
        ("protocol extension default plus conformer", &[("Sources/M/A.swift", "protocol P { var x: Int { get } }\nextension P { var y: Int { x } }\nstruct A: P { var x = 1 }\nstruct B: P { var x = 2\n func f() -> Int { y + x } }\n")]),
        ("extension adds computed with same name in two types across files", &[("Sources/M/A.swift", "struct A {}\nstruct B {}\n"), ("Sources/M/B.swift", "extension A { var x: Int { 1 }\n func f() -> Int { x } }\n"), ("Sources/N/C.swift", "extension B { var x: Int { 2 }\n func g() -> Int { x } }\n")]),
        ("enum with associated cases and same-named property", &[("Sources/M/A.swift", "enum E { case a(Int)\n var a2: Int { if case .a(let v) = self { return v }; return 0 } }\nenum F { case a(Int)\n var a2: Int { 0 } }\nfunc h() -> E { .a(1) }\n")]),
        ("class var and static let same name", &[("Sources/M/A.swift", "class A { class var x: Int { 1 }\n static let y = 1\n func f() -> Int { Self.x + A.y } }\nclass B { static let x = 2\n static var y: Int { 1 }\n func g() -> Int { B.x + Self.y } }\n")]),
        ("local type shadows", &[("Sources/M/A.swift", "struct A { var x = 1 }\nfunc f() -> Int { struct A { var x = 2 }; return A().x }\n")]),
        ("operators and subscripts", &[("Sources/M/A.swift", "struct V { var x = 1\n static func + (l: V, r: V) -> V { V(x: l.x + r.x) }\n subscript(i: Int) -> Int { x } }\nstruct W { var x = 2\n static func + (l: W, r: W) -> W { W(x: l.x + r.x) } }\n")]),
        ("nested generics extension", &[("Sources/M/A.swift", "struct Outer<T> { struct Inner { var x = 1 }\n func f(_ i: Inner) -> Int { i.x } }\nstruct Other { struct Inner { var x = 2 }\n func g(_ i: Inner) -> Int { i.x } }\n")]),
    ];
    let mut panicked = Vec::new();
    for (name, files) in shapes {
        let files: Vec<(&str, &str)> = files.to_vec();
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = resolve(&files);
        })) {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            panicked.push(format!("{name}: {message}"));
        }
    }
    assert!(panicked.is_empty(), "{panicked:#?}");
}

// ---------------------------------------------------------------- #4 member keys

#[test]
fn recall_a_method_read_as_a_value_binds_the_method() {
    let r = one("struct O { func size() -> Int { 1 } }\nfunc g(_ o: O) { let f = o.size; _ = f }\n");
    let u = uses(&r, "g");
    assert!(has(&u, "O.size"), "{u:?}");
}

#[test]
fn recall_a_property_and_a_method_of_one_name_are_both_reached() {
    let r = one("struct O { var size = 1\n func size(_ n: Int) -> Int { n } }\nfunc g(_ o: O) -> Int { o.size + o.size(2) }\n");
    let edges: Vec<(EdgeKind, String)> = r
        .edges
        .iter()
        .filter(|e| e.source_symbol.ends_with("::g") && e.target_symbol.ends_with("O.size"))
        .map(|e| (e.edge_kind, e.target_symbol.clone()))
        .collect();
    assert!(
        edges.iter().any(|(k, _)| *k == EdgeKind::Calls)
            && edges.iter().any(|(k, _)| *k == EdgeKind::References),
        "{edges:?}"
    );
}

#[test]
fn wrong_a_call_never_binds_a_same_named_property_when_a_method_exists() {
    let r = one("struct O { var size = 1\n func size(_ n: Int) -> Int { n } }\nfunc g(_ o: O) -> Int { o.size(2) }\n");
    let kinds: Vec<EdgeKind> = r
        .edges
        .iter()
        .filter(|e| e.source_symbol.ends_with("::g") && e.target_symbol.ends_with("O.size"))
        .map(|e| e.edge_kind)
        .collect();
    assert!(kinds.contains(&EdgeKind::Calls), "{kinds:?}");
    assert!(!kinds.contains(&EdgeKind::References), "{kinds:?}");
}

#[test]
fn recall_a_closure_property_called_through_its_name_is_a_use_of_the_property() {
    let r = one("struct S { var handler: () -> Void }\nfunc g(_ s: S) { s.handler() }\n");
    let u = uses(&r, "g");
    assert!(has(&u, "S.handler"), "{u:?}");
}

#[test]
fn recall_a_protocol_requirement_mixing_var_and_func_of_one_name() {
    let r = one("protocol P { var size: Int { get }\n func size(_ n: Int) -> Int }\nfunc g(_ p: P) -> Int { p.size + p.size(1) }\n");
    let u = uses(&r, "g");
    assert!(has(&u, "P.size"), "{u:?}");
}

#[test]
fn wrong_a_qualified_case_and_a_static_property_of_the_same_name_bind_their_own_types() {
    let r = one("enum E { case circle }\nstruct F { static var circle = 1 }\nfunc h() { _ = E.circle; _ = F.circle }\n");
    let u = uses(&r, "h");
    assert!(has(&u, "E.circle") && has(&u, "F.circle"), "{u:?}");
}

// ---------------------------------------------------------------- #5 lexical scopes

#[test]
fn wrong_a_local_function_does_not_leak_into_a_sibling_method() {
    let r = one("struct S {\n func m() { func helper() {}; helper() }\n func n() { helper() }\n}\n");
    let u = uses(&r, "n");
    assert!(!has(&u, "helper"), "{u:?}");
}

#[test]
fn wrong_a_nested_type_does_not_leak_into_a_sibling_type() {
    let r = one("struct A { struct Inner {}\n func f() { _ = Inner() } }\nstruct B { func g() { _ = Inner() } }\n");
    let u = uses(&r, "g");
    assert!(!has(&u, "A.Inner"), "{u:?}");
}

#[test]
fn wrong_same_named_nested_types_in_siblings_each_bind_their_own() {
    let r = one("struct A { struct Inner {}\n func f() { _ = Inner() } }\nstruct B { struct Inner {}\n func g() { _ = Inner() } }\n");
    let f = uses(&r, "f");
    let g = uses(&r, "g");
    assert!(!has(&f, "B.Inner") && !has(&g, "A.Inner"), "{f:?} {g:?}");
}

// ---------------------------------------------------------------- #6 leading dot (resolve level)

#[test]
#[ignore = "needs extractor change (schema frozen at 97): a leading-dot member through an optional result emits no reference, ft-915b0a03"]
fn recall_an_optional_result_leading_dot_reaches_the_case() {
    let r = one("enum Shape { case circle }\nfunc f() -> Shape? { .circle }\n");
    let u = uses(&r, "f");
    assert!(has(&u, "Shape.circle"), "{u:?}");
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97): a leading-dot member inside an array result emits no reference, ft-915b0a03"]
fn recall_an_array_result_leading_dot_reaches_the_case() {
    let r = one("enum Shape { case circle }\nfunc f() -> [Shape] { [.circle] }\n");
    let u = uses(&r, "f");
    assert!(has(&u, "Shape.circle"), "{u:?}");
}

#[test]
fn recall_a_self_result_leading_dot_reaches_the_case() {
    let r = one("enum E { case a\n static func make() -> Self { .a } }\n");
    let u = uses(&r, "make");
    assert!(has(&u, "E.a"), "{u:?}");
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97): a leading-dot member in a ternary branch emits no reference, ft-915b0a03"]
fn recall_a_ternary_leading_dot_reaches_the_case() {
    let r = one("enum E { case a, b }\nfunc f(_ c: Bool) -> E { c ? .a : .b }\n");
    let u = uses(&r, "f");
    assert!(has(&u, "E.a") && has(&u, "E.b"), "{u:?}");
}

#[test]
fn wrong_a_generic_parameter_named_like_a_real_type_does_not_bind_that_types_case() {
    let r = one("protocol P {}\nenum T { case a }\nfunc f<T: P>() -> T { .a }\n");
    let u = uses(&r, "f");
    assert!(!has(&u, "T.a"), "{u:?}");
}

// ---------------------------------------------------------------- #8 key paths

#[test]
fn wrong_a_keypath_without_a_root_outside_its_type_binds_nothing() {
    let r = one("struct S { var secret = 1 }\nfunc f(_ xs: [S]) -> [Int] { xs.map(\\.secret) }\n");
    let u = uses(&r, "f");
    assert!(!has(&u, "S.secret"), "{u:?}");
}

#[test]
#[ignore = "needs extractor change (schema frozen at 97): `\\.count` is emitted as a bare `count` read with no key-path marker, indistinguishable from an implicit-self read, ft-915b0a03"]
fn wrong_a_contextual_keypath_inside_another_type_does_not_bind_that_types_member() {
    let r = one("struct Foo { var count = 0\n func f(_ xs: [String]) -> [Int] { xs.map(\\.count) } }\n");
    let u = uses(&r, "f");
    assert!(!has(&u, "Foo.count"), "{u:?}");
}

#[test]
fn wrong_a_chained_keypath_does_not_bind_the_tail_to_the_root() {
    let r = one("struct S { var secret = [1]\n var count = 5 }\nfunc f() { _ = \\S.secret.count }\n");
    let u = uses(&r, "f");
    assert!(has(&u, "S.secret"), "recall: {u:?}");
    assert!(!has(&u, "S.count"), "wrong: {u:?}");
}

#[test]
fn wrong_an_array_root_keypath_binds_nothing_of_the_element() {
    let r = one("struct S { var first = 1 }\nfunc f() { _ = \\[S].first }\n");
    let u = uses(&r, "f");
    assert!(!has(&u, "S.first"), "{u:?}");
}

#[test]
fn wrong_a_generic_root_keypath_binds_nothing_of_the_argument() {
    let r = one("struct S { var count = 1 }\nfunc f() { _ = \\Array<S>.count }\n");
    let u = uses(&r, "f");
    assert!(!has(&u, "S.count"), "{u:?}");
}

// ---------------------------------------------------------------- #10 implicit self

#[test]
fn wrong_a_bare_read_in_a_free_function_of_an_extension_file_binds_nothing() {
    let r = resolve(&[
        ("Sources/M/A.swift", "struct T { var count = 1 }\n"),
        ("Sources/M/B.swift", "extension T { func m() -> Int { count } }\nfunc free() -> Int { count }\n"),
    ]);
    let m = uses(&r, "T.m");
    let m2 = uses(&r, "m");
    let free = uses(&r, "free");
    assert!(has(&m, "T.count") || has(&m2, "T.count"), "recall: {m:?} {m2:?}");
    assert!(!has(&free, "T.count"), "wrong: {free:?}");
}

#[test]
fn wrong_a_global_sharing_a_name_with_an_inherited_property_binds_by_scope() {
    let r = one("let name = \"g\"\nclass Base { var name = \"b\" }\nclass Derived: Base { func m() -> String { name } }\nfunc free() -> String { name }\n");
    let m = uses(&r, "m");
    let free = uses(&r, "free");
    assert!(has(&m, "Base.name"), "member wins in a method: {m:?}");
    assert!(!has(&free, "Base.name"), "global outside: {free:?}");
}

#[test]
fn control_an_inherited_property_read_without_a_global_binds() {
    let r = one("class Base { var name = \"b\" }\nclass Derived: Base { func m() -> String { name } }\n");
    let m = uses(&r, "m");
    assert!(has(&m, "Base.name"), "{m:?}");
}

#[test]
fn recall_an_own_property_read_beside_a_same_named_global_binds_the_member() {
    let r = one("let name = \"g\"\nclass A { var name = \"b\"\n func m() -> String { name } }\n");
    let m = uses(&r, "m");
    assert!(has(&m, "A.name"), "{m:?}");
}

#[test]
fn recall_an_inherited_property_read_beside_a_same_named_global_binds_the_member() {
    let r = one("let name = \"g\"\nclass Base { var name = \"b\" }\nclass D: Base { func m() -> String { name } }\n");
    let m = uses(&r, "m");
    assert!(has(&m, "Base.name"), "{m:?}");
}

#[test]
fn wrong_a_free_function_read_beside_a_member_of_the_same_name_binds_no_member() {
    let r = one("let name = \"g\"\nclass A { var name = \"b\" }\nfunc free() -> String { name }\n");
    let u = uses(&r, "free");
    assert!(!has(&u, "A.name"), "{u:?}");
}

#[test]
fn wrong_a_static_function_local_named_like_an_instance_property_binds_nothing() {
    let r = one("struct S { var n = 1\n static func f() -> Int { let n = 5; return n } }\n");
    let u = uses(&r, "f");
    assert!(!has(&u, "S.n"), "{u:?}");
}

#[test]
fn wrong_a_parameter_named_like_an_inherited_property_binds_nothing() {
    let r = one("class Base { var name = \"b\" }\nclass D: Base { func m(name: String) -> String { name } }\n");
    let u = uses(&r, "m");
    assert!(!has(&u, "Base.name"), "{u:?}");
}

#[test]
fn wrong_a_closure_parameter_named_like_an_inherited_property_binds_nothing() {
    let r = one("class Base { var name = \"b\" }\nclass D: Base { func m() -> [String] { [\"a\"].map { name in name } } }\n");
    let u = uses(&r, "m");
    assert!(!has(&u, "Base.name"), "{u:?}");
}

#[test]
fn wrong_a_local_declared_after_use_scope_named_like_inherited_property() {
    let r = one("class Base { var name = \"b\" }\nclass D: Base { func m() -> String { if true { let name = \"x\"; return name }; return \"\" } }\n");
    let u = uses(&r, "m");
    assert!(!has(&u, "Base.name"), "{u:?}");
}

#[test]
fn wrong_a_for_loop_variable_named_like_an_inherited_property() {
    let r = one("class Base { var item = 1 }\nclass D: Base { func m(_ xs: [Int]) -> Int { var t = 0; for item in xs { t += item }; return t } }\n");
    let u = uses(&r, "m");
    assert!(!has(&u, "Base.item"), "{u:?}");
}

#[test]
fn wrong_a_guard_let_binding_named_like_an_inherited_property() {
    let r = one("class Base { var item: Int? = 1 }\nclass D: Base { func m(_ o: Int?) -> Int { guard let item = o else { return 0 }; return item } }\n");
    let u = uses(&r, "m");
    assert!(!has(&u, "Base.item"), "{u:?}");
}

#[test]
fn wrong_a_switch_case_binding_named_like_an_inherited_property() {
    let r = one("class Base { var v = 1 }\nclass D: Base { func m(_ o: Int?) -> Int { switch o { case let v?: return v; default: return 0 } } }\n");
    let u = uses(&r, "m");
    assert!(!has(&u, "Base.v"), "{u:?}");
}
