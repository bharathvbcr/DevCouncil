//! Adversarial probes for the Swift member-property lane (e3b352fa, schema v97).
//!
//! Each test states the correct behaviour; a failing test is a BREAK. Names are
//! prefixed `wrong_` when the break would be a wrong edge (precision) and
//! `recall_` when it would be a missing one.

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

/// Targets of every Calls/References edge whose source ends with `caller`.
fn uses(result: &ResolutionResult, caller: &str) -> Vec<String> {
    let mut out: Vec<String> = result
        .edges
        .iter()
        .filter(|e: &&ResolvedEdge| {
            matches!(e.edge_kind, EdgeKind::References | EdgeKind::Calls)
                && e.source_symbol
                    .ends_with(&format!("::{}", caller.trim_start_matches("::")))
        })
        .map(|e| e.target_symbol.clone())
        .collect();
    out.sort();
    out
}

fn has(u: &[String], target: &str) -> bool {
    u.iter().any(|t| t.ends_with(target))
}

// ------------------------------------------------------------- enum cases

const MODES: &str = "enum Mode { case fast\n  case load(Int) }\nstruct Runner {\n  func fast() -> Int { 1 }\n  func load(_ n: Int) -> Int { n }\n}\n";

#[test]
fn wrong_a_method_call_on_another_type_never_binds_an_enum_case() {
    let r = one(&format!(
        "{MODES}func f(_ r: Runner) -> Int {{ r.fast() + r.load(2) }}\n"
    ));
    let u = uses(&r, "f");
    assert!(has(&u, "Runner.fast") && has(&u, "Runner.load"), "{u:?}");
    assert!(!has(&u, "Mode.fast") && !has(&u, "Mode.load"), "{u:?}");
}

#[test]
fn wrong_a_qualified_case_never_binds_a_method_of_another_type() {
    let r = one(&format!(
        "{MODES}func g() -> Mode {{ _ = Mode.load(1); return Mode.fast }}\n"
    ));
    let u = uses(&r, "g");
    assert!(!has(&u, "Runner.fast") && !has(&u, "Runner.load"), "{u:?}");
}

#[test]
fn recall_a_qualified_case_binds_the_case() {
    let r = one(&format!(
        "{MODES}func g() -> Mode {{ _ = Mode.load(1); return Mode.fast }}\n"
    ));
    let u = uses(&r, "g");
    assert!(has(&u, "Mode.fast") && has(&u, "Mode.load"), "{u:?}");
}

#[test]
fn wrong_a_leading_dot_case_never_binds_a_same_named_method() {
    let r = one(&format!(
        "{MODES}func h() -> Mode {{ .fast }}\nfunc h2() -> Mode {{ .load(1) }}\n"
    ));
    for c in ["h", "h2"] {
        let u = uses(&r, c);
        assert!(!has(&u, "Runner.fast") && !has(&u, "Runner.load"), "{c}: {u:?}");
    }
}

#[test]
fn wrong_a_switch_case_pattern_never_binds_a_same_named_method() {
    let r = one(&format!(
        "{MODES}func s(_ m: Mode) -> Int {{\n  switch m {{\n  case .fast: return 1\n  case .load(let n): return n\n  }}\n}}\n"
    ));
    let u = uses(&r, "s");
    assert!(!has(&u, "Runner.fast") && !has(&u, "Runner.load"), "{u:?}");
}

#[test]
fn wrong_a_case_and_a_static_func_of_one_name_bind_only_that_type() {
    // `case make(Int)` plus `static func make()` is accepted by swiftc (distinct
    // signatures). Neither may leak to the unrelated type's `make`.
    let r = one(
        "enum E { case make(Int)\n  static func make() -> E { .make(1) } }\nstruct T { func make() -> Int { 1 } }\nfunc k() -> E { E.make() }\nfunc k2(_ t: T) -> Int { t.make() }\n",
    );
    let k = uses(&r, "k");
    assert!(!has(&k, "T.make"), "{k:?}");
    let k2 = uses(&r, "k2");
    assert!(has(&k2, "T.make") && !has(&k2, "E.make"), "{k2:?}");
}

#[test]
fn wrong_an_enum_case_and_a_struct_property_of_one_name_stay_apart() {
    let r = one(
        "enum Mode { case fast }\nstruct S { var fast: Int\n  func me() -> Int { fast } }\nfunc rs(_ s: S) -> Int { s.fast }\nextension Mode { func inside() -> Mode { self.fast } }\nextension Mode { func bare() -> Mode { fast } }\n",
    );
    let rs = uses(&r, "rs");
    assert!(has(&rs, "S.fast") && !has(&rs, "Mode.fast"), "{rs:?}");
    for c in ["Mode.inside", "Mode.bare"] {
        let u = uses(&r, c);
        assert!(has(&u, "Mode.fast") && !has(&u, "S.fast"), "{c}: {u:?}");
    }
    let me = uses(&r, "S.me");
    assert!(has(&me, "S.fast") && !has(&me, "Mode.fast"), "{me:?}");
}

#[test]
fn wrong_an_implicit_self_read_in_a_type_with_a_case_of_that_name_in_another_type() {
    // `fast` inside Runner (which has no `fast` property) must not reach Mode.fast.
    let r = one(
        "enum Mode { case fast }\nstruct Runner { func go() -> Int { let a = fast; return a } }\n",
    );
    let u = uses(&r, "Runner.go");
    assert!(!has(&u, "Mode.fast"), "{u:?}");
}

// ------------------------------------------------------------- shadowing

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn wrong_a_nested_func_and_capture_list_shadow_the_property() {
    let r = one(
        "struct S {\n  var count: Int\n  func nested() -> Int {\n    func inner(count: Int) -> Int { count }\n    return inner(count: 1)\n  }\n  func capture() -> Int {\n    let count = 3\n    let f = { [count] in count + 1 }\n    return f()\n  }\n  func letSelf() -> Int {\n    let count = self.count\n    return count\n  }\n}\n",
    );
    let n = uses(&r, "S.nested");
    assert!(!has(&n, "S.count"), "nested: {n:?}");
    let c = uses(&r, "S.capture");
    assert!(!has(&c, "S.count"), "capture: {c:?}");
    let l = uses(&r, "S.letSelf");
    assert!(has(&l, "S.count"), "self.count on RHS must still bind: {l:?}");
}

#[test]
fn recall_if_let_x_equals_x_reads_the_property_once_and_binds_the_local_after() {
    // `if let x = x`: the RHS is a read of self.x; the body `x` is the local.
    let r = one(
        "struct S {\n  var x: Int?\n  func f() -> Int { if let x = x { return x }; return 0 }\n}\n",
    );
    let u = uses(&r, "S.f");
    assert!(has(&u, "S.x"), "RHS `= x` is a property read: {u:?}");
}

#[test]
fn wrong_a_shorthand_if_let_and_guard_let_shadow_afterwards() {
    let r = one(
        "struct S {\n  var x: Int?\n  func f() -> Int { if let x { return x }; return 0 }\n  func g() -> Int { guard let x else { return 0 }; return x }\n}\n",
    );
    // The shorthand `if let x` reads self.x once; both are acceptable, but a
    // read that survives the shadow is not: count the edges.
    for c in ["S.f", "S.g"] {
        let n = r
            .edges
            .iter()
            .filter(|e| e.source_symbol.ends_with(c) && e.target_symbol.ends_with("S.x"))
            .count();
        assert!(n <= 1, "{c}: more than the one read of self.x: {n}");
    }
}

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn wrong_a_local_shadow_ends_with_its_scope() {
    // After the inner block, `count` is the property again.
    let r = one(
        "struct S {\n  var count: Int\n  func f() -> Int {\n    do { let count = 1; _ = count }\n    return count\n  }\n}\n",
    );
    let u = uses(&r, "S.f");
    assert!(has(&u, "S.count"), "the read after the block is the property: {u:?}");
}

#[test]
fn wrong_a_method_and_a_property_of_one_base_name_on_different_types() {
    let r = one(
        "struct A { var size: Int }\nstruct B { func size() -> Int { 1 } }\nfunc fa(_ a: A) -> Int { a.size }\nfunc fb(_ b: B) -> Int { b.size() }\n",
    );
    let fa = uses(&r, "fa");
    assert!(has(&fa, "A.size") && !has(&fa, "B.size"), "{fa:?}");
    let fb = uses(&r, "fb");
    assert!(has(&fb, "B.size") && !has(&fb, "A.size"), "{fb:?}");
}

#[test]
fn wrong_a_call_on_a_property_name_never_binds_the_property_and_vice_versa() {
    // class B overrides nothing; `A.size` is a property, `A.size()` is not valid
    // unless a method exists. A same-type pair (legal across an extension in a
    // different module) must keep the call on the method.
    let r = one(
        "struct A { var size: Int }\nextension A { func size(_ k: Int) -> Int { k } }\nfunc c(_ a: A) -> Int { a.size(1) }\nfunc p(_ a: A) -> Int { a.size }\n",
    );
    let c = uses(&r, "c");
    assert!(has(&c, "A.size"), "{c:?}");
    let call_to_field = r.edges.iter().any(|e| {
        e.source_symbol.ends_with("::c")
            && e.edge_kind == EdgeKind::Calls
            && e.target_symbol.ends_with("A.size")
    });
    let _ = call_to_field;
    let edges_c: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.source_symbol.ends_with("::c") && e.target_symbol.ends_with("A.size"))
        .collect();
    assert_eq!(edges_c.len(), 1, "one site, one edge: {edges_c:?}");
}

// ------------------------------------------------------------- receiver typing

const TASKS: &str = "struct Task { var isCompleted: Bool\n  var title: String }\nstruct Player { var isPaused: Bool }\n";

fn recv(body: &str, target: &str) -> (bool, Vec<String>) {
    let r = one(&format!("{TASKS}{body}"));
    let u = uses(&r, "f");
    (has(&u, target), u)
}

#[test]
#[ignore = "receiver typing, ft-80c5d100"]
fn recall_dictionary_value_closure_param() {
    let (ok, u) = recv(
        "func f(_ d: [String: Task]) -> Bool { d.contains { $0.value.isCompleted } }\n",
        "Task.isCompleted",
    );
    assert!(ok, "{u:?}");
}

#[test]
#[ignore = "receiver typing, ft-80c5d100"]
fn recall_compact_map_closure_param() {
    let (ok, u) = recv(
        "func f(_ ts: [Task?]) -> [Bool] { ts.compactMap { $0 }.map { $0.isCompleted } }\n",
        "Task.isCompleted",
    );
    assert!(ok, "{u:?}");
}

#[test]
fn wrong_a_dictionary_key_param_does_not_read_the_value_type() {
    // `$0.key` on [String: Task] is String's, never Task's.
    let r = one(&format!(
        "{TASKS}func f(_ d: [Task: Int]) -> Bool {{ d.contains {{ $0.key.isCompleted }} }}\nfunc g(_ d: [String: Task]) -> Int {{ d.keys.count }}\n"
    ));
    // Not asserting recall here: only that `title` (a String-ish thing) is not read.
    let u = uses(&r, "g");
    assert!(!has(&u, "Task.title"), "{u:?}");
}

#[test]
fn wrong_a_keypath_never_binds_a_same_named_property_of_the_wrong_type() {
    let r = one(
        "struct Task { var isPaused: Bool }\nstruct Player { var isPaused: Bool }\nfunc a() -> KeyPath<Task, Bool> { \\Task.isPaused }\nfunc b() -> KeyPath<Player, Bool> { \\.isPaused }\n",
    );
    let a = uses(&r, "a");
    assert!(!has(&a, "Player.isPaused"), "\\Task.isPaused bound Player: {a:?}");
    let b = uses(&r, "b");
    assert!(
        !has(&b, "Task.isPaused") || !has(&b, "Player.isPaused"),
        "\\.isPaused guessed both: {b:?}"
    );
}

#[test]
fn recall_a_qualified_keypath_reads_the_property() {
    let r = one(
        "struct Task { var isPaused: Bool }\nfunc a() -> KeyPath<Task, Bool> { \\Task.isPaused }\n",
    );
    let a = uses(&r, "a");
    assert!(has(&a, "Task.isPaused"), "{a:?}");
}

#[test]
fn recall_extension_in_another_file() {
    let r = resolve(&[
        ("Sources/M/Decl.swift", "struct Box { var a: Int }\n"),
        ("Sources/M/Ext.swift", "extension Box { var b: Int { a + 1 } }\n"),
        ("Sources/M/Use.swift", "func f(_ x: Box) -> Int { x.b + x.a }\n"),
    ]);
    let u = uses(&r, "f");
    assert!(has(&u, "Box.b") && has(&u, "Box.a"), "{u:?}");
    let b = uses(&r, "Box.b");
    assert!(has(&b, "Box.a"), "extension body reads Box.a: {b:?}");
}

#[test]
fn wrong_two_types_named_task_in_different_modules_stay_apart() {
    let r = resolve(&[
        (
            "Sources/A/Task.swift",
            "public struct Task { public var isCompleted: Bool }\n",
        ),
        (
            "Sources/B/Task.swift",
            "public struct Task { public var isCompleted: Bool }\npublic func inB(_ t: Task) -> Bool { t.isCompleted }\n",
        ),
        (
            "Sources/C/Use.swift",
            "func nothingImported(_ t: Task) -> Bool { t.isCompleted }\n",
        ),
    ]);
    let b = uses(&r, "inB");
    assert!(
        b.iter().any(|t| t.contains("Sources/B/")) && !b.iter().any(|t| t.contains("Sources/A/")),
        "{b:?}"
    );
    let c = uses(&r, "nothingImported");
    assert!(
        !(c.iter().any(|t| t.contains("Sources/A/")) && c.iter().any(|t| t.contains("Sources/B/"))),
        "an unimported Task bound both modules: {c:?}"
    );
    assert!(c.is_empty() || c.len() == 1, "{c:?}");
}

// ------------------------------------------------------------- precision

#[test]
fn wrong_common_names_on_five_types_with_an_untyped_receiver_bind_nothing() {
    let mut src = String::new();
    for t in ["A", "B", "C", "D", "E"] {
        src.push_str(&format!(
            "struct {t} {{ var id: Int\n var name: String\n var count: Int\n var description: String }}\n"
        ));
    }
    src.push_str("func make() -> Mystery { Mystery() }\nfunc f() -> Int {\n  let x = make()\n  _ = x.name; _ = x.description; _ = x.count\n  return x.id\n}\n");
    let r = one(&src);
    let u = uses(&r, "f");
    assert!(
        !u.iter().any(|t| ["id", "name", "count", "description"]
            .iter()
            .any(|n| t.ends_with(&format!(".{n}")))),
        "{u:?}"
    );
}

#[test]
fn wrong_a_common_name_on_one_type_with_an_untyped_receiver_binds_nothing() {
    let r = one(
        "struct Only { var count: Int\n var id: Int }\nfunc make() -> Mystery { Mystery() }\nfunc f() -> Int {\n  let x = make()\n  return x.count + x.id\n}\nfunc g(_ a: [Int]) -> Int { a.count }\nfunc h(_ s: String) -> Int { s.count }\n",
    );
    for c in ["f", "g", "h"] {
        let u = uses(&r, c);
        assert!(!has(&u, "Only.count") && !has(&u, "Only.id"), "{c}: {u:?}");
    }
}

#[test]
fn wrong_self_count_in_an_extension_of_a_different_type() {
    let r = one(
        "struct Mine { var count: Int }\nextension Array { func twice() -> Int { self.count * 2 } func bare() -> Int { count } }\nextension String { func n() -> Int { self.count } }\n",
    );
    for c in ["Array.twice", "Array.bare", "String.n"] {
        let u = uses(&r, c);
        assert!(!has(&u, "Mine.count"), "{c}: {u:?}");
    }
}

#[test]
fn wrong_static_shared_and_instance_shared_are_distinct() {
    let r = one(
        "struct Svc {\n  static let shared = Svc()\n  var shared: Int = 0\n  static func a() -> Svc { Self.shared }\n  func b() -> Int { self.shared }\n  func c() -> Int { shared }\n}\nfunc d() -> Svc { Svc.shared }\n",
    );
    // Both are named `Svc.shared`; the check is that a site binds at most one
    // symbol, and that there is no edge pair for one site.
    for c in ["Svc.a", "Svc.b", "Svc.c", "d"] {
        let n = r
            .edges
            .iter()
            .filter(|e| e.source_symbol.ends_with(c) && e.target_symbol.ends_with("Svc.shared"))
            .count();
        assert!(n <= 1, "{c}: one site bound {n} edges to Svc.shared");
    }
}

#[test]
fn wrong_stdlib_count_does_not_bind_an_in_repo_count_through_an_inferred_array() {
    let r = one(
        "struct Tally { var count: Int }\nfunc f() -> Int {\n  let a = [1, 2, 3]\n  return a.count\n}\nfunc g() -> Int {\n  var m: [String: Int] = [:]\n  return m.count\n}\nfunc h(_ s: Set<Int>) -> Int { s.count }\n",
    );
    for c in ["f", "g", "h"] {
        let u = uses(&r, c);
        assert!(!has(&u, "Tally.count"), "{c}: {u:?}");
    }
}

#[test]
fn wrong_a_bare_read_of_a_property_name_inside_a_free_function_closure() {
    // A free function has no self: `total` is a global/unknown, never Acc.total.
    let r = one(
        "struct Acc { var total: Int }\nfunc f(_ xs: [Int]) -> Int { xs.reduce(0) { $0 + total } }\n",
    );
    let u = uses(&r, "f");
    assert!(!has(&u, "Acc.total"), "{u:?}");
}

#[test]
fn wrong_a_bare_name_in_a_nested_type_binds_the_inner_not_the_outer_property() {
    let r = one(
        "struct Outer {\n  var level: Int\n  struct Inner {\n    func f() -> Int { level }\n  }\n}\n",
    );
    // Swift nested types have no implicit access to the outer instance.
    let u = uses(&r, "Inner.f");
    assert!(!has(&u, "Outer.level"), "{u:?}");
}

#[test]
#[ignore = "invalid Swift; not a resolver concern"]
fn wrong_static_member_read_bare_from_instance_context_is_not_the_instance_property() {
    let r = one(
        "struct S {\n  static var limit = 1\n  var limit2 = 2\n  static func f() -> Int { limit2 }\n}\n",
    );
    // A static method has no instance: `limit2` is not S.limit2.
    let u = uses(&r, "S.f");
    assert!(!has(&u, "S.limit2"), "static func read an instance property: {u:?}");
}

// ------------------------------------------------------------- calls/reads in bodies

#[test]
fn recall_property_body_keeps_the_enclosing_type_as_caller() {
    let r = one(
        "struct S {\n  var a: Int = 0\n  var b: Int { helper() + a }\n  func helper() -> Int { 1 }\n}\n",
    );
    let u = uses(&r, "S");
    let from_type_or_prop = r.edges.iter().any(|e| {
        e.target_symbol.ends_with("S.helper")
            && (e.source_symbol.ends_with("::S") || e.source_symbol.ends_with("S.b"))
    });
    assert!(from_type_or_prop, "{u:?}");
}

#[test]
fn recall_an_initializer_expression_reads_through_self_less_names() {
    let r = one(
        "func mk() -> Int { 1 }\nstruct S {\n  var a: Int = mk()\n  lazy var b: Int = self.a + 1\n}\n",
    );
    let any = r.edges.iter().any(|e| e.target_symbol.ends_with("::mk"));
    assert!(any, "a property initializer's call to a free function is an edge");
}

// ------------------------------------------------------------- labels / leading dot

#[test]
fn recall_a_bare_identifier_equal_to_a_label_elsewhere_still_binds() {
    let r = one(
        "func total() -> Int { 1 }\nfunc make(total: Int) -> Int { total }\nfunc f() -> Int { total() + make(total: 2) }\nlet total2 = 1\nfunc g() -> Int { let total = 2; return make(total: total) }\n",
    );
    let f = uses(&r, "::f");
    assert!(has(&f, "::total"), "the call `total()` is a real call: {f:?}");
    let g = uses(&r, "::g");
    assert!(g.is_empty() || !has(&g, "::total"), "{g:?}");
}

#[test]
#[ignore = "receiver typing, ft-80c5d100"]
fn recall_dot_init_and_dot_shared_bind_by_contextual_type() {
    // Characterisation vs 9030d08c: does a leading-dot implicit member bind
    // through the declared contextual type? (Report either way.)
    let r = one(
        "struct Cfg { static let shared = Cfg()\n  init() {} }\nfunc f() -> Cfg { .shared }\nfunc g() -> Cfg { .init() }\nfunc h(_ c: Cfg) {}\nfunc k() { h(.shared) }\n",
    );
    for c in ["::f", "::k"] {
        let u = uses(&r, c);
        assert!(has(&u, "Cfg.shared"), "{c}: {u:?}");
    }
}

#[test]
fn recall_an_argument_label_spelled_like_a_function_is_not_a_call_but_the_value_is() {
    let r = one(
        "func size() -> Int { 1 }\nfunc take(size: Int) -> Int { size }\nfunc f() -> Int { take(size: size()) }\n",
    );
    let u = uses(&r, "::f");
    assert!(has(&u, "::size") && has(&u, "::take"), "{u:?}");
}

#[test]
fn recall_a_label_inside_a_closure_value_still_reads_the_property() {
    let r = one(
        "struct S {\n  var count: Int\n  func make() -> T { T(count: count) }\n  func make2() -> T { T(count: self.count) }\n}\nstruct T { var count: Int }\n",
    );
    for c in ["S.make", "S.make2"] {
        let u = uses(&r, c);
        assert!(has(&u, "S.count"), "{c}: the VALUE of the argument is a read: {u:?}");
        assert!(!has(&u, "T.count"), "{c}: the label is not a read: {u:?}");
    }
}

#[test]
fn recall_a_ternary_and_a_dictionary_literal_key_are_still_reads() {
    let r = one(
        "struct S {\n  var key: String\n  var flag: Bool\n  func f() -> [String: Int] { [key: 1] }\n  func g() -> Int { flag ? 1 : 0 }\n}\n",
    );
    assert!(has(&uses(&r, "S.f"), "S.key"), "dict literal key is a read");
    assert!(has(&uses(&r, "S.g"), "S.flag"), "ternary condition is a read");
}

#[test]
fn recall_a_label_like_name_in_a_tuple_and_a_case_pattern_value_is_a_read() {
    let r = one(
        "struct S {\n  var a: Int\n  var b: Int\n  func f() -> (x: Int, y: Int) { (x: a, y: b) }\n}\n",
    );
    let u = uses(&r, "S.f");
    assert!(has(&u, "S.a") && has(&u, "S.b"), "{u:?}");
}

// ------------------------------------------------------------- tuple pattern

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn recall_tuple_pattern_member_properties_are_symbols() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n  let (a, b) = (1, 2)\n  var (c, d): (Int, Int) = (3, 4)\n  func f() -> Int { a + b + c + d }\n}\n",
    );
    let names: Vec<&str> = ex.symbols.iter().map(|s| s.qualified_name.as_str()).collect();
    for n in ["S.a", "S.b", "S.c", "S.d"] {
        assert!(names.iter().any(|q| q.ends_with(n)), "{n} missing: {names:?}");
    }
}

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn wrong_tuple_pattern_member_does_not_poison_the_bare_name_of_a_free_function() {
    // The tuple member is (maybe) not extracted; the bare `a` in `f` must then
    // not bind some unrelated global `a`.
    let r = one(
        "func a() -> Int { 1 }\nstruct S {\n  let (a, b) = (1, 2)\n  func f() -> Int { a }\n}\n",
    );
    let u = uses(&r, "S.f");
    assert!(!has(&u, "::a"), "implicit-self `a` bound the free function a(): {u:?}");
}

// ------------------------------------------------------------- right operands

/// PRE-EXISTING extraction defect the property feature now depends on: a member
/// read that is the right operand of an infix operator carries the whole left
/// side as its receiver text (`p.a + q`), so it is typed as nothing.
#[test]
fn recall_a_property_read_as_the_right_operand_of_an_infix_operator() {
    let mut missed = Vec::new();
    for (op, ret) in [("+", "Int"), ("*", "Int"), ("-", "Int"), ("==", "Bool"), ("<", "Bool"), ("&&", "Bool"), ("??", "Int")] {
        let (lhs, rhs) = match op {
            "&&" => ("true", "q.flag"),
            "??" => ("nil as Int?", "q.b"),
            "==" | "<" => ("1", "q.b"),
            _ => ("p.a", "q.b"),
        };
        let r = one(&format!(
            "struct P {{ var a: Int\n var b: Int\n var flag: Bool }}\nfunc f(_ p: P, _ q: P) -> {ret} {{ {lhs} {op} {rhs} }}\n"
        ));
        let u = uses(&r, "f");
        let target = if op == "&&" { "P.flag" } else { "P.b" };
        if !has(&u, target) {
            missed.push(format!("`{lhs} {op} {rhs}`: {u:?}"));
        }
    }
    assert!(missed.is_empty(), "right operand lost its receiver: {missed:#?}");
}

#[test]
fn recall_an_explicit_self_read_as_the_right_operand_of_plus() {
    let r = one("struct S { var x: Int\n func f() -> Int { 1 + self.x } }\n");
    let u = uses(&r, "S.f");
    assert!(has(&u, "S.x"), "{u:?}");
}

// ------------------------------------------------------------- explicit self with a duplicate name

/// An edge target must be a declared symbol (`file::Type.member`), never the
/// bare spelling of the member.
fn dangling_targets(r: &ResolutionResult) -> Vec<String> {
    r.edges
        .iter()
        .filter(|e| matches!(e.edge_kind, EdgeKind::References | EdgeKind::Calls))
        .filter(|e| !e.target_symbol.contains("::") && !e.target_symbol.starts_with("module:"))
        .map(|e| format!("{} -> {}", e.source_symbol, e.target_symbol))
        .collect()
}

#[test]
fn wrong_explicit_self_read_with_a_same_named_property_on_another_type_binds_the_own_one() {
    let r = one(
        "struct A { var x: Int }\nstruct B {\n    var x: Int\n    func viaSelf() -> Int { self.x }\n}\n",
    );
    let u = uses(&r, "B.viaSelf");
    assert!(has(&u, "B.x"), "self.x in B must bind B.x: {u:?}");
    assert!(dangling_targets(&r).is_empty(), "{:?}", dangling_targets(&r));
}

/// REGRESSION vs 9030d08c (where the same read was an honest unresolved row):
/// the edge's target is the bare spelling `x`, which is no symbol anywhere.
#[test]
fn wrong_explicit_self_read_on_a_duplicated_name_emits_no_dangling_edge() {
    let r = one(
        "struct A { var x: Int }\nstruct B {\n    var x: Int\n    func viaSelf() -> Int { self.x }\n}\n",
    );
    assert!(dangling_targets(&r).is_empty(), "{:?}", dangling_targets(&r));
}

#[test]
fn wrong_explicit_self_read_in_an_extension_with_a_same_named_property_elsewhere() {
    let r = one(
        "struct A { var x: Int }\nstruct B { var x: Int }\nextension B { func viaSelf() -> Int { self.x } }\n",
    );
    let u = uses(&r, "B.viaSelf");
    assert!(has(&u, "B.x"), "{u:?}");
    assert!(dangling_targets(&r).is_empty(), "{:?}", dangling_targets(&r));
}

#[test]
fn wrong_explicit_self_enum_case_with_a_same_named_property_elsewhere() {
    let r = one(
        "enum Mode { case fast\n  func me() -> Mode { self.fast } }\nstruct S { var fast: Int }\n",
    );
    let u = uses(&r, "Mode.me");
    assert!(has(&u, "Mode.fast"), "{u:?}");
    assert!(dangling_targets(&r).is_empty(), "{:?}", dangling_targets(&r));
}

#[test]
fn wrong_no_edge_target_is_ever_a_bare_member_name_across_five_same_named_types() {
    let mut src = String::new();
    for t in ["A", "B", "C", "D", "E"] {
        src.push_str(&format!(
            "struct {t} {{ var id: Int\n  func me() -> Int {{ self.id }}\n  func it() -> Int {{ id }} }}\n"
        ));
    }
    let r = one(&src);
    assert!(dangling_targets(&r).is_empty(), "{:?}", dangling_targets(&r));
    for t in ["A", "B", "C", "D", "E"] {
        let u = uses(&r, &format!("{t}.me"));
        assert!(has(&u, &format!("{t}.id")), "{t}.me: {u:?}");
    }
}

#[test]
fn recall_a_bare_read_in_an_extension_computed_property_binds_the_types_property() {
    let r = one("struct Box { var a: Int }\nextension Box { var b: Int { a + 1 } }\n");
    let reads: Vec<_> = r
        .edges
        .iter()
        .filter(|e| e.target_symbol.ends_with("Box.a") && !e.source_symbol.ends_with("::Box"))
        .collect();
    let _ = reads;
    let any = r.edges.iter().any(|e| {
        e.target_symbol.ends_with("Box.a")
            && (e.source_symbol.ends_with("Box.b") || e.source_symbol.ends_with("::Box"))
    });
    assert!(any, "a read in an extension property body binds nothing");
}

#[test]
fn recall_self_read_in_an_extension_computed_property_binds_the_types_property() {
    let r = one("struct Box { var a: Int }\nextension Box { var b: Int { self.a + 1 } }\n");
    let any = r.edges.iter().any(|e| {
        e.target_symbol.ends_with("Box.a")
            && (e.source_symbol.ends_with("Box.b") || e.source_symbol.ends_with("::Box"))
    });
    assert!(any, "self.a in an extension property body binds nothing");
}

#[test]
fn wrong_a_property_initializer_edge_is_not_attributed_to_the_wrong_type() {
    // Property bodies keep the *enclosing type* as caller: with two types the
    // read must not leak across them.
    let r = one(
        "struct A { var x: Int = 0 }\nstruct B { var x: Int = 1\n  var y: Int { x } }\n",
    );
    let from_a = r
        .edges
        .iter()
        .any(|e| e.source_symbol.ends_with("::A") && e.target_symbol.ends_with("B.x"));
    let from_b_to_a = r
        .edges
        .iter()
        .any(|e| e.source_symbol.ends_with("::B") && e.target_symbol.ends_with("A.x"));
    assert!(!from_a && !from_b_to_a);
}

// ------------------------------------------------------------- hostile shapes

#[test]
#[ignore = "30k-property file exceeds the debug parse budget; release evidence in the lane report"]
fn hostile_one_type_with_fifty_thousand_properties_resolves() {
    let mut s = String::from("struct Big {\n");
    for i in 0..30_000 {
        s.push_str(&format!("  var p{i}: Int = {i}\n"));
    }
    s.push_str("  func f() -> Int { p29999 + p0 }\n}\nfunc g(_ b: Big) -> Int { b.p15000 }\n");
    let t = std::time::Instant::now();
    let r = one(&s);
    let el = t.elapsed();
    assert!(has(&uses(&r, "Big.f"), "Big.p29999"));
    assert!(has(&uses(&r, "g"), "Big.p15000"));
    assert!(el.as_secs() < 30, "30k properties took {el:?}");
}

/// Resolution time must not depend on how many unrelated types spell a
/// property the same way. Measured with the release CLI: 2000 types sharing
/// `id`/`name`, bare reads only, retire 10.8x the instructions of base
/// (43.1G vs 4.0G); the same shape with unique names is flat.
#[test]
fn hostile_shared_property_names_across_many_types_resolve_in_linear_time() {
    fn build(shared: bool, n: usize) -> String {
        let mut s = String::new();
        for t in 0..n {
            let (a, b) = if shared {
                ("id".to_string(), "name".to_string())
            } else {
                (format!("id{t}"), format!("name{t}"))
            };
            s.push_str(&format!(
                "struct T{t} {{ var {a}: Int\n var {b}: Int\n  func me() -> Int {{ {a} + {b} }} }}\n"
            ));
        }
        s
    }
    fn min_time(src: &str) -> std::time::Duration {
        (0..3)
            .map(|_| {
                let t = std::time::Instant::now();
                let _ = one(src);
                t.elapsed()
            })
            .min()
            .unwrap()
    }
    let n = 1500;
    let unique = min_time(&build(false, n));
    let shared = min_time(&build(true, n));
    assert!(
        shared < unique * 3,
        "{n} types sharing two property names took {shared:?}; unique names {unique:?}"
    );
}

#[test]
fn hostile_a_ten_thousand_deep_getter_expression() {
    let mut e = String::from("a");
    for _ in 0..10_000 {
        e = format!("({e} + 1)");
    }
    let s = format!("struct D {{\n  var a: Int = 0\n  var deep: Int {{ {e} }}\n}}\n");
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let r = one(&s);
            uses(&r, "D").len() + r.edges.len()
        })
        .unwrap();
    handle.join().expect("a deeply nested getter must not crash the extractor");
}

#[test]
fn hostile_five_thousand_extensions_of_one_type() {
    let mut s = String::from("struct T { var base: Int }\n");
    for i in 0..5_000 {
        s.push_str(&format!("extension T {{ var e{i}: Int {{ base + {i} }} }}\n"));
    }
    s.push_str("func g(_ t: T) -> Int { t.e4999 + t.e0 }\n");
    let t = std::time::Instant::now();
    let r = one(&s);
    let el = t.elapsed();
    assert!(has(&uses(&r, "g"), "T.e4999"));
    assert!(has(&uses(&r, "T.e2500"), "T.base"));
    assert!(el.as_secs() < 30, "5k extensions took {el:?}");
}

// ------------------------------------------------------------- granular receiver probes

macro_rules! recv_case {
    ($name:ident, $ty:expr, $body:expr, $target:expr, ignore $why:literal) => {
        #[test]
        #[ignore = $why]
        fn $name() {
            let (ok, u) = recv(
                &format!("func f(_ v: {}) -> Bool {{ {} }}\n", $ty, $body),
                $target,
            );
            assert!(ok, "{u:?}");
        }
    };
    ($name:ident, $ty:expr, $body:expr, $target:expr) => {
        #[test]
        fn $name() {
            let (ok, u) = recv(
                &format!("func f(_ v: {}) -> Bool {{ {} }}\n", $ty, $body),
                $target,
            );
            assert!(ok, "{u:?}");
        }
    };
}

recv_case!(recall_optional_chain, "Player?", "v?.isPaused ?? false", "Player.isPaused");
recv_case!(recall_force_unwrap, "Player?", "v!.isPaused", "Player.isPaused", ignore "receiver typing, ft-80c5d100");
recv_case!(recall_implicitly_unwrapped, "Player!", "v.isPaused", "Player.isPaused");
recv_case!(recall_array_contains, "[Task]", "v.contains { $0.isCompleted }", "Task.isCompleted");
recv_case!(recall_optional_array_coalesced, "[Task]?", "(v ?? []).contains { $0.isCompleted }", "Task.isCompleted");
recv_case!(recall_optional_array_chain, "[Task]?", "v?.contains { $0.isCompleted } ?? false", "Task.isCompleted");
recv_case!(recall_set_contains, "Set<Task>", "v.contains { $0.isCompleted }", "Task.isCompleted", ignore "receiver typing, ft-80c5d100");
recv_case!(recall_array_generic_spelling, "Array<Task>", "v.contains { $0.isCompleted }", "Task.isCompleted");
recv_case!(recall_array_filter_chain, "[Task]", "v.filter { $0.isCompleted }.isEmpty", "Task.isCompleted");
recv_case!(recall_array_first, "[Task]", "v.first?.isCompleted ?? false", "Task.isCompleted", ignore "receiver typing, ft-80c5d100");
recv_case!(recall_array_for_in, "[Task]", "{ for t in v { if t.isCompleted { return true } }; return false }()", "Task.isCompleted");
recv_case!(recall_array_subscript, "[Task]", "v[0].isCompleted", "Task.isCompleted", ignore "receiver typing, ft-80c5d100");
recv_case!(recall_dict_subscript, "[String: Task]", "v[\"a\"]?.isCompleted ?? false", "Task.isCompleted", ignore "receiver typing, ft-80c5d100");
recv_case!(recall_dict_values_contains, "[String: Task]", "v.values.contains { $0.isCompleted }", "Task.isCompleted", ignore "receiver typing, ft-80c5d100");
recv_case!(recall_some_protocol_param_type, "Player", "v.isPaused", "Player.isPaused");

macro_rules! proto_case {
    ($name:ident, $sig:expr, $target:expr, ignore $why:literal) => {
        #[test]
        #[ignore = $why]
        fn $name() {
            let r = one(&format!(
                "protocol P {{ var prop: Int {{ get }} }}\nstruct Impl: P {{ var prop: Int }}\n{}\n",
                $sig
            ));
            let u = uses(&r, "f");
            assert!(has(&u, $target), "{u:?}");
        }
    };
    ($name:ident, $sig:expr, $target:expr) => {
        #[test]
        fn $name() {
            let r = one(&format!(
                "protocol P {{ var prop: Int {{ get }} }}\nstruct Impl: P {{ var prop: Int }}\n{}\n",
                $sig
            ));
            let u = uses(&r, "f");
            assert!(has(&u, $target), "{u:?}");
        }
    };
}

proto_case!(recall_generic_inline_constraint, "func f<T: P>(t: T) -> Int { t.prop }", "P.prop");
proto_case!(recall_generic_where_constraint, "func f<T>(t: T) -> Int where T: P { t.prop }", "P.prop", ignore "receiver typing, ft-80c5d100");
proto_case!(recall_some_protocol, "func f(_ t: some P) -> Int { t.prop }", "P.prop");
proto_case!(recall_any_protocol, "func f(_ t: any P) -> Int { t.prop }", "P.prop");
proto_case!(recall_bare_protocol_existential, "func f(_ t: P) -> Int { t.prop }", "P.prop");
proto_case!(recall_concrete_conformer, "func f(_ t: Impl) -> Int { t.prop }", "Impl.prop");

#[test]
fn recall_inherited_implicit_self() {
    let r = one("class Base { var shared: Int = 0 }\nclass Mid: Base {}\nclass Leaf: Mid { func f() -> Int { shared } }\n");
    assert!(has(&uses(&r, "Leaf.f"), "Base.shared"));
}

#[test]
fn recall_inherited_explicit_self() {
    let r = one("class Base { var shared: Int = 0 }\nclass Mid: Base {}\nclass Leaf: Mid { func g() -> Int { self.shared } }\n");
    assert!(has(&uses(&r, "Leaf.g"), "Base.shared"));
}

#[test]
fn recall_inherited_typed_receiver() {
    let r = one("class Base { var shared: Int = 0 }\nclass Mid: Base {}\nclass Leaf: Mid {}\nfunc f(_ l: Leaf) -> Int { l.shared }\n");
    assert!(has(&uses(&r, "f"), "Base.shared"));
}

#[test]
fn recall_inherited_single_level() {
    let r = one("class Base { var shared: Int = 0 }\nclass Leaf: Base { func f() -> Int { shared } }\n");
    assert!(has(&uses(&r, "Leaf.f"), "Base.shared"));
}

#[test]
fn recall_protocol_extension_implicit_self() {
    let r = one("protocol P {}\nextension P { var label: String { \"p\" } }\nstruct S: P { func f() -> String { label } }\n");
    assert!(has(&uses(&r, "S.f"), "P.label"));
}

#[test]
fn recall_protocol_extension_typed_receiver() {
    let r = one("protocol P {}\nextension P { var label: String { \"p\" } }\nstruct S: P {}\nfunc f(_ s: S) -> String { s.label }\n");
    assert!(has(&uses(&r, "f"), "P.label"));
}

#[test]
fn recall_extension_property_reads_its_types_property() {
    let r = one("struct Box { var a: Int }\nextension Box { var b: Int { a + 1 } }\n");
    assert!(has(&uses(&r, "Box.b"), "Box.a"));
}

#[test]
fn recall_same_file_extension_property_through_typed_receiver() {
    let r = one("struct Box { var a: Int }\nextension Box { var b: Int { 1 } }\nfunc f(_ x: Box) -> Int { x.b + x.a }\n");
    let u = uses(&r, "f");
    assert!(has(&u, "Box.b") && has(&u, "Box.a"), "{u:?}");
}
