//! Adversarial probes against lane D (resolver bundle). Each test asserts the
//! CORRECT behaviour; a failure is a finding.
use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

type E = Vec<(String, String)>;

fn run(files: &[(&str, &str)], kind: EdgeKind) -> E {
    let extractions: Vec<_> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let mut v: E = result
        .edges
        .iter()
        .filter(|e| e.edge_kind == kind)
        .map(|e| (e.source_symbol.clone(), e.target_symbol.clone()))
        .collect();
    v.sort();
    v
}
fn calls(files: &[(&str, &str)]) -> E {
    run(files, EdgeKind::Calls)
}
fn reads(files: &[(&str, &str)]) -> E {
    run(files, EdgeKind::References)
}
fn has(e: &E, f: &str, t: &str) -> bool {
    e.iter().any(|(a, b)| a == f && b == t)
}
fn from(e: &E, f: &str) -> Vec<String> {
    e.iter().filter(|(a, _)| a == f).map(|(_, b)| b.clone()).collect()
}

// ---------------- overloads ----------------

#[test]
fn ov_default_param_self_recursion_keeps_self_edge() {
    let s = "package p\n\nfun f(a: Int = 0): Int = if (a > 0) f(a - 1) else 0\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::f", "p/Z.kt::f"), "{e:?}");
}

#[test]
fn ov_member_recursion_keeps_self_edge() {
    let s = "package p\n\nclass C {\n    fun f(a: Int): Int = if (a > 0) f(a - 1) else 0\n    fun g(a: Int): Int = a\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::C.f", "p/Z.kt::C.f"), "{e:?}");
}

#[test]
fn ov_property_and_function_same_name_recursion() {
    let s = "package p\n\nclass C {\n    val f: Int = 1\n    fun f(a: Int): Int = if (a > 0) f(a - 1) else 0\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::C.f", "p/Z.kt::C.f"), "property+fun same name lost true self-edge: {e:?}");
}

#[test]
fn ov_local_functions_same_name_in_two_functions_keep_recursion() {
    let s = "package p\n\nfun a(): Int {\n    fun helper(n: Int): Int = if (n > 0) helper(n - 1) else 0\n    return helper(3)\n}\n\nfun b(): Int {\n    fun helper(n: Int): Int = if (n > 0) helper(n - 2) else 1\n    return helper(4)\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("local fn edges: {e:?}");
    assert!(e.iter().any(|(a, b)| a == b && a.contains("helper")), "local helper recursion lost: {e:?}");
}

#[test]
fn ov_extension_and_member_sibling_never_self_edge() {
    let s = "package p\n\nclass C {\n    fun add(a: Int): Int = a\n}\n\nfun C.add(s: String): Int = add(s.length)\n\nfun use(c: C): Int = c.add(1) + c.add(\"x\")\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
    assert!(!e.iter().any(|(a, b)| a == b), "self edge: {e:?}");
    assert!(has(&e, "p/Z.kt::use", "p/Z.kt::C.add"), "{e:?}");
}

#[test]
fn ov_top_level_pair_true_recursion_in_one_overload() {
    let s = "package p\n\nfun walk(a: Int): Int = if (a > 0) walk(a - 1) else 0\n\nfun walk(s: String): Int = walk(s.length)\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("top-level pair: {e:?}");
    assert!(has(&e, "p/Z.kt::walk", "p/Z.kt::walk"), "true recursion in walk(Int) lost (shared id): {e:?}");
}

#[test]
fn ov_same_name_in_different_classes_keep_cross_edges() {
    let s = "package p\n\nclass A {\n    fun go(x: Int): Int = B().go(\"s\")\n}\n\nclass B {\n    fun go(s: String): Int = A().go(1)\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::A.go", "p/Z.kt::B.go"), "{e:?}");
    assert!(has(&e, "p/Z.kt::B.go", "p/Z.kt::A.go"), "{e:?}");
}

#[test]
fn ov_same_name_in_different_classes_recursion_kept() {
    let s = "package p\n\nclass A {\n    fun go(x: Int): Int = if (x > 0) go(x - 1) else 0\n}\n\nclass B {\n    fun go(x: Int): Int = if (x > 0) go(x - 1) else 0\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::A.go", "p/Z.kt::A.go"), "{e:?}");
    assert!(has(&e, "p/Z.kt::B.go", "p/Z.kt::B.go"), "{e:?}");
}

#[test]
fn ov_companion_overload_cross_calls() {
    let s = "package p\n\nclass C {\n    fun add(a: Int): Int = a\n\n    companion object {\n        fun add(a: String): Int = C().add(1)\n    }\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("companion: {e:?}");
    assert!(e.iter().any(|(a, b)| a != b && b.ends_with("add")), "member add reached from companion add: {e:?}");
}

#[test]
fn ov_companion_recursion_kept() {
    let s = "package p\n\nclass C {\n    companion object {\n        fun make(a: Int): Int = if (a > 0) make(a - 1) else 0\n    }\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(e.iter().any(|(a, b)| a == b && a.ends_with("make")), "{e:?}");
}

#[test]
fn ov_swift_init_overloads_and_recursion() {
    let s = "class Pt {\n    var x = 0\n    init(x: Int) { self.x = x }\n    init(s: String) { self.init(x: s.count) }\n    func fact(_ n: Int) -> Int { return n <= 1 ? 1 : n * fact(n - 1) }\n    subscript(i: Int) -> Int { return i }\n    subscript(s: String) -> Int { return self[s.count] }\n}\n";
    let e = calls(&[("a/Pt.swift", s)]);
    println!("swift: {e:?}");
    assert!(has(&e, "a/Pt.swift::Pt.fact", "a/Pt.swift::Pt.fact"), "{e:?}");
}

#[test]
fn ov_swift_generic_overloads_no_self_edge() {
    let s = "struct S {\n    func f<T>(_ x: T) { f(1) }\n    func f(_ x: Int) { }\n    func g<T>(_ x: T) { if false { g(x) } }\n}\nfunc use(s: S) { s.f(\"a\") }\n";
    let e = calls(&[("a/S.swift", s)]);
    println!("swift gen: {e:?}");
    assert!(!has(&e, "a/S.swift::S.f", "a/S.swift::S.f"), "{e:?}");
    assert!(has(&e, "a/S.swift::S.g", "a/S.swift::S.g"), "{e:?}");
}

#[test]
fn ov_swift_same_name_class_two_files_not_merged() {
    let a = "class Calc { func add(_ a: Int) -> Int { a } }\n";
    let b = "class Calc { func add(_ a: Int) -> Int { a } }\n";
    let u = "func use(c: Calc) -> Int { c.add(1) }\n";
    let e = calls(&[("a/A.swift", a), ("b/B.swift", b), ("c/U.swift", u)]);
    println!("{e:?}");
    assert!(from(&e, "c/U.swift::use").len() <= 1, "{e:?}");
}

#[test]
fn ov_kotlin_two_packages_same_class_not_merged() {
    let a = "package a\n\nclass Calc {\n    fun add(x: Int): Int = x\n}\n";
    let b = "package b\n\nclass Calc {\n    fun add(x: Int): Int = x\n}\n";
    let u = "package c\n\nimport a.Calc\n\nfun use(c: Calc): Int = c.add(1)\n";
    let e = calls(&[("a/A.kt", a), ("b/B.kt", b), ("c/U.kt", u)]);
    println!("{e:?}");
    assert!(!has(&e, "c/U.kt::use", "b/B.kt::Calc.add"), "wrong package: {e:?}");
}

#[test]
fn ov_kotlin_same_file_similar_classes() {
    let s = "package p\n\nclass Calc {\n    fun add(a: Int): Int = a\n}\n\nclass Calc2 {\n    fun add(a: Int): Int = a\n}\n\nfun use(c: Calc2): Int = c.add(1)\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::use", "p/Z.kt::Calc2.add"), "{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Calc.add"), "{e:?}");
}

#[test]
fn ov_java_recursion_and_overload() {
    let s = "class A {\n    int f(int a) { return a > 0 ? f(a - 1) : 0; }\n    int g(int a) { return a; }\n    int g(String s) { return g(s.length()); }\n    int h(int a) { return a > 0 ? h(a-1) : 0; }\n}\n";
    let e = calls(&[("a/A.java", s)]);
    println!("java: {e:?}");
    assert!(has(&e, "a/A.java::A.f", "a/A.java::A.f"), "{e:?}");
    assert!(has(&e, "a/A.java::A.h", "a/A.java::A.h"), "{e:?}");
    assert!(!has(&e, "a/A.java::A.g", "a/A.java::A.g"), "{e:?}");
}

#[test]
fn ov_ts_overload_signatures_keep_impl_recursion() {
    let s = "export function f(a: string): void;\nexport function f(a: number): void;\nexport function f(a: any): void {\n  if (a) { f(a - 1); }\n}\n";
    let e = calls(&[("a/f.ts", s)]);
    println!("ts: {e:?}");
    assert!(has(&e, "a/f.ts::f", "a/f.ts::f"), "TS overload-impl recursion lost: {e:?}");
}

#[test]
fn ov_rust_two_trait_impls_same_method_recursion() {
    let s = "struct X(Option<Box<X>>);\ntrait T1 { fn walk(&self) -> u32; }\ntrait T2 { fn walk(&self) -> u32; }\nimpl T1 for X {\n    fn walk(&self) -> u32 { match &self.0 { Some(c) => c.walk() + 1, None => 0 } }\n}\nimpl T2 for X {\n    fn walk(&self) -> u32 { 2 }\n}\n";
    let e = calls(&[("a/x.rs", s)]);
    println!("rust: {e:?}");
    assert!(e.iter().any(|(a, b)| a == b && a.ends_with("walk")), "rust recursion w/ two impls lost: {e:?}");
}

#[test]
fn ov_python_redefined_function_recursion() {
    let s = "def f(n):\n    return f(n - 1) if n else 0\n\nif True:\n    def g(n):\n        return g(n - 1) if n else 0\nelse:\n    def g(n):\n        return 1\n";
    let e = calls(&[("a/f.py", s)]);
    println!("py: {e:?}");
    assert!(has(&e, "a/f.py::f", "a/f.py::f"), "{e:?}");
    assert!(has(&e, "a/f.py::g", "a/f.py::g"), "py conditional def recursion lost: {e:?}");
}

#[test]
fn ov_go_init_multiple_and_recursion() {
    let s = "package a\n\nfunc init() {}\nfunc init() {}\nfunc f(n int) int { if n > 0 { return f(n-1) }; return 0 }\n";
    let e = calls(&[("a/f.go", s)]);
    assert!(has(&e, "a/f.go::f", "a/f.go::f"), "{e:?}");
}

// ---------------- Z3 / Z5 ----------------

#[test]
fn z3_repo_declares_own_list_class_with_same_member_wrong_edge() {
    let own = "package q\n\nclass List<T> {\n    fun total(): Int = 1\n}\n";
    let user = "package p\n\nfun names(): List<String> = listOf(\"a\")\n\nfun use(): Int {\n    val files = names()\n    return files.total()\n}\n";
    let e = calls(&[("q/L.kt", own), ("p/Z.kt", user)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "q/L.kt::List.total"), "stdlib List bound to repo List: {e:?}");
}

#[test]
fn z3_control_declared_param_type_vs_imported_other_list() {
    let own = "package q\n\nclass List<T> {\n    fun total(): Int = 1\n}\n";
    let user = "package p\n\nfun use(files: List<String>): Int {\n    return files.total()\n}\n";
    let e = calls(&[("q/L.kt", own), ("p/Z.kt", user)]);
    println!("control: {e:?}");
}

#[test]
fn z3_cross_file_factory_matching_repo_class() {
    let own = "package q\n\nclass List<T> {\n    fun total(): Int = 1\n}\n";
    let prov = "package p\n\nfun names(): List<String> = listOf(\"a\")\n";
    let user = "package p\n\nfun use(): Int {\n    val files = names()\n    return files.total()\n}\n";
    let e = calls(&[("q/L.kt", own), ("p/N.kt", prov), ("p/Z.kt", user)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "q/L.kt::List.total"), "{e:?}");
}

#[test]
fn z3_imported_list_from_other_package_vs_same_file_decl() {
    let own = "package q\n\nclass List<T> {\n    fun total(): Int = 1\n}\n";
    let user = "package p\n\nimport java.awt.List\n\nfun names(): List<String> = throw Exception()\n\nfun use(): Int {\n    val files = names()\n    return files.total()\n}\n";
    let e = calls(&[("q/L.kt", own), ("p/Z.kt", user)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "q/L.kt::List.total"), "{e:?}");
}

#[test]
fn z3_nested_qualified_type_takes_last_segment() {
    let own = "package q\n\nclass Entry {\n    fun getValue(): Int = 1\n}\n";
    let user = "package p\n\nfun first(): Map.Entry<String, Int> = mapOf(\"a\" to 1).entries.first()\n\nfun use(): Int {\n    val e = first()\n    return e.getValue()\n}\n";
    let e = calls(&[("q/E.kt", own), ("p/Z.kt", user)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "q/E.kt::Entry.getValue"), "Map.Entry -> repo Entry: {e:?}");
}

#[test]
fn z3_qualified_package_prefix_other_package_class() {
    let own = "package q\n\nclass Repo<T> {\n    fun find(): Int = 1\n}\n";
    let user = "package p\n\nfun get(): ext.Repo<String> = TODO()\n\nfun use(): Int {\n    val r = get()\n    return r.find()\n}\n";
    let e = calls(&[("q/R.kt", own), ("p/Z.kt", user)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "q/R.kt::Repo.find"), "ext.Repo bound to q.Repo: {e:?}");
}

#[test]
fn z3_typealias_generic() {
    let user = "package p\n\nclass Bar<T> {\n    fun bar(): Int = 1\n}\n\nclass Other {\n    fun bar(): Int = 2\n}\n\ntypealias Foo<T> = Other\n\nfun make(): Foo<Int> = Other()\n\nfun use(): Int {\n    val f = make()\n    return f.bar()\n}\n";
    let e = calls(&[("p/Z.kt", user)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Bar.bar"), "{e:?}");
}

#[test]
fn z3_typealias_name_equals_other_class_elsewhere() {
    let other = "package q\n\nclass Foo<T> {\n    fun bar(): Int = 1\n}\n";
    let user = "package p\n\nclass Real {\n    fun bar(): Int = 2\n}\n\ntypealias Foo<T> = Real\n\nfun make(): Foo<Int> = Real()\n\nfun use(): Int {\n    val f = make()\n    return f.bar()\n}\n";
    let e = calls(&[("q/F.kt", other), ("p/Z.kt", user)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "q/F.kt::Foo.bar"), "alias Foo bound to q.Foo: {e:?}");
}

#[test]
fn z3_generic_class_box_returning_t() {
    let s = "package p\n\nclass Box<T>(val v: T) {\n    fun get(): T = v\n    fun total(): Int = 1\n}\n\nclass Other {\n    fun total(): Int = 2\n}\n\nfun use(b: Box<Other>): Int {\n    val o = b.get()\n    return o.total()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Box.total"), "T return typed as Box: {e:?}");
}

#[test]
fn z3_two_overloads_different_returns() {
    for order in [0, 1] {
        let a = "fun load(): List<String> = listOf()\n";
        let b = "fun load(id: Int): Other = Other()\n";
        let (x, y) = if order == 0 { (a, b) } else { (b, a) };
        let s = format!("package p\n\nprivate fun List<String>.total(): Int = 1\n\nclass Other {{\n    fun total(): Int = 2\n}}\n\n{x}\n{y}\nfun use(): Int {{\n    val v = load(1)\n    return v.total()\n}}\n");
        let e = calls(&[("p/Z.kt", &s)]);
        println!("order {order}: {e:?}");
        assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "order {order}: {e:?}");
    }
}

#[test]
fn z3_declared_type_wins_over_factory() {
    let s = "package p\n\nprivate fun List<String>.total(): Int = 1\n\nclass Other {\n    fun total(): Int = 2\n}\n\nfun names(): List<String> = listOf()\n\nfun use(): Int {\n    val v: Other = names() as Other\n    return v.total()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "{e:?}");
}

#[test]
fn z3_lambda_param_shadows_local() {
    let s = "package p\n\nprivate fun List<String>.total(): Int = 1\n\nclass Other {\n    fun total(): Int = 2\n}\n\nfun names(): List<String> = listOf()\n\nfun use(xs: List<Other>): Int {\n    val files = names()\n    return xs.map { files -> files.total() }.size\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "lambda param shadow: {e:?}");
}

#[test]
fn z3_smart_cast_is_check() {
    let s = "package p\n\nprivate fun List<String>.total(): Int = 1\n\nclass Other {\n    fun total(): Int = 2\n}\n\nfun names(): List<String> = listOf()\n\nfun use(): Int {\n    val files: Any = names()\n    if (files is Other) return files.total()\n    return 0\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "{e:?}");
}

#[test]
fn z3_if_branches_two_types() {
    let s = "package p\n\nprivate fun List<String>.total(): Int = 1\n\nclass Other {\n    fun total(): Int = 2\n}\n\nfun names(): List<String> = listOf()\nfun other(): Other = Other()\n\nfun use(f: Boolean): Int {\n    val v = if (f) names() else other()\n    return v.total()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
    assert!(!(has(&e, "p/Z.kt::use", "p/Z.kt::List.total") && !has(&e, "p/Z.kt::use", "p/Z.kt::Other.total")), "one-branch typing: {e:?}");
}

#[test]
fn z3_mutable_list_vs_list_ext_on_list() {
    let s = "package p\n\nprivate fun List<String>.total(): Int = 1\n\nfun names(): MutableList<String> = mutableListOf()\n\nfun use(): Int {\n    val v = names()\n    return v.total()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
}

#[test]
fn z3_extension_in_other_file_not_imported() {
    let ext = "package q\n\nfun List<String>.total(): Int = 1\n";
    let user = "package p\n\nfun names(): List<String> = listOf()\n\nfun use(): Int {\n    val v = names()\n    return v.total()\n}\n";
    let e = calls(&[("q/E.kt", ext), ("p/Z.kt", user)]);
    println!("unimported ext: {e:?}");
}

const T5: &str = "package p\n\ninterface Repo {\n    fun find(id: Int): String\n}\n\nclass Impl1 : Repo {\n    override fun find(id: Int): String = \"1\"\n}\n\nclass Impl2 : Repo {\n    override fun find(id: Int): String = \"2\"\n}\n\nfun <reified T> mock(): T = throw IllegalStateException()\n\n";

#[test]
fn z5_interface_with_two_impls_goes_to_interface() {
    let s = format!("{T5}fun use(): String {{\n    val repo = mock<Repo>()\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    assert!(has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Impl1.find"), "{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Impl2.find"), "{e:?}");
}

#[test]
fn z5_cast_after_mock() {
    for init in ["mock<Repo>() as Impl1", "(mock<Repo>() as Impl1)", "mock<Repo>().also { }"] {
        let s = format!("{T5}fun use(): String {{\n    val repo = {init}\n    return repo.find(1)\n}}\n");
        let e = calls(&[("p/Z.kt", &s)]);
        println!("{init}: {:?}", from(&e, "p/Z.kt::use"));
        assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{init}: {e:?}");
    }
}

#[test]
fn z5_declared_other_type_wins() {
    let s = format!("{T5}fun use(): String {{\n    val repo: Impl1 = mock<Repo>() as Impl1\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z5_callee_returns_external_class_not_type_param() {
    let s = "package p\n\nclass Repo {\n    fun info(msg: String): Int = 1\n}\n\ninline fun <reified T> logger(): Logger = Logger()\n\nfun use(): Int {\n    val log = logger<Repo>()\n    return log.info(\"x\")\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.info"), "external return type treated as type param: {e:?}");
}

#[test]
fn z5_callee_returns_unit_or_class_type_param() {
    let s = "package p\n\nclass Repo {\n    fun launch(): Int = 1\n}\n\nfun <T> register(): Job = Job()\n\nfun use(): Int {\n    val j = register<Repo>()\n    return j.launch()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.launch"), "{e:?}");
}

#[test]
fn z5_return_is_other_type_parameter_of_enclosing_class() {
    let s = "package p\n\nclass Repo {\n    fun find(): Int = 1\n}\n\nclass Holder<R> {\n    fun <T> make(): R = TODO()\n    fun use(): Int {\n        val x = make<Repo>()\n        return x.find()\n    }\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
    assert!(!has(&e, "p/Z.kt::Holder.use", "p/Z.kt::Repo.find"), "class type param R instantiated by <Repo>: {e:?}");
}

#[test]
fn z5_imported_external_repo_same_name_as_corpus_class() {
    let a = "package a\n\nclass Repo {\n    fun find(id: Int): String = \"a\"\n}\n";
    let s = "package p\n\nimport com.ext.Repo\n\nfun <reified T> mock(): T = throw IllegalStateException()\n\nfun use(): String {\n    val repo = mock<Repo>()\n    return repo.find(1)\n}\n";
    let e = calls(&[("a/R.kt", a), ("p/Z.kt", s)]);
    println!("{e:?}");
    let s2 = "package p\n\nimport com.ext.Repo\n\nfun use(repo: Repo): String {\n    return repo.find(1)\n}\n";
    let c = calls(&[("a/R.kt", a), ("p/Z.kt", s2)]);
    println!("control: {c:?}");
    assert!(!has(&e, "p/Z.kt::use", "a/R.kt::Repo.find"), "z5 bound across a conflicting import: {e:?} control={c:?}");
}

#[test]
fn z5_argument_declared_in_other_file_visible() {
    let a = "package a\n\ninterface Repo {\n    fun find(id: Int): String\n}\n";
    let m = "package p\n\nfun <reified T> mock(): T = throw IllegalStateException()\n";
    let s = "package p\n\nimport a.Repo\n\nfun use(): String {\n    val repo = mock<Repo>()\n    return repo.find(1)\n}\n";
    let e = calls(&[("a/R.kt", a), ("p/M.kt", m), ("p/Z.kt", s)]);
    println!("cross-file: {e:?}");
}

#[test]
fn z5_lambda_param_shadow() {
    let s = format!("{T5}class Other {{\n    fun find(id: Int): String = \"o\"\n}}\n\nfun use(xs: List<Other>): List<String> {{\n    val repo = mock<Repo>()\n    return xs.map {{ repo -> repo.find(2) }}\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    println!("{:?}", from(&e, "p/Z.kt::use"));
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "lambda param named repo bound to outer Repo: {e:?}");
}

#[test]
fn z5_it_shadow() {
    let s = format!("{T5}class Other {{\n    fun find(id: Int): String = \"o\"\n}}\n\nfun use(xs: List<Other>): List<String> {{\n    val it = mock<Repo>()\n    return xs.map {{ it.find(2) }}\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    println!("{:?}", from(&e, "p/Z.kt::use"));
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "implicit it bound to outer val it: {e:?}");
}

#[test]
fn z5_same_name_in_two_functions_independent() {
    let s = format!("{T5}class Other {{\n    fun find(id: Int): String = \"o\"\n}}\n\nfun a(): String {{\n    val repo = mock<Repo>()\n    return repo.find(1)\n}}\n\nfun b(): String {{\n    val repo = mock<Other>()\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    assert!(has(&e, "p/Z.kt::a", "p/Z.kt::Repo.find"), "{e:?}");
    assert!(has(&e, "p/Z.kt::b", "p/Z.kt::Other.find"), "{e:?}");
    assert!(!has(&e, "p/Z.kt::a", "p/Z.kt::Other.find"), "{e:?}");
    assert!(!has(&e, "p/Z.kt::b", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z5_two_locals_one_scope_other_call_inside_lambda() {
    let s = format!("{T5}class Other {{\n    fun find(id: Int): String = \"o\"\n}}\n\nfun use(): String {{\n    val repo = mock<Repo>()\n    run {{\n        val repo = mock<Other>()\n        repo.find(2)\n    }}\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    println!("{:?}", from(&e, "p/Z.kt::use"));
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find") || !has(&e, "p/Z.kt::use", "p/Z.kt::Other.find"), "{e:?}");
}

#[test]
fn z5_nested_class_named_repo_and_toplevel() {
    let s = format!("{T5}class Outer {{\n    class Repo {{\n        fun find(id: Int): String = \"n\"\n    }}\n}}\n\nfun use(): String {{\n    val repo = mock<Repo>()\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    println!("{:?}", from(&e, "p/Z.kt::use"));
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Outer.Repo.find"), "{e:?}");
}

#[test]
fn z5_callee_overloaded_with_value_arg() {
    let s = format!("{T5}class Other {{\n    fun find(id: Int): String = \"o\"\n}}\nfun mock(x: Int): Other = Other()\n\nfun use(): String {{\n    val repo = mock<Repo>()\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    println!("overloaded callee: {:?}", from(&e, "p/Z.kt::use"));
}

#[test]
fn z5_generic_class_arg() {
    let s = format!("{T5}class Box<X> {{\n    fun find(id: Int): String = \"b\"\n}}\n\nfun use(): String {{\n    val repo = mock<Box<Repo>>()\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z5_trailing_lambda_arg() {
    let s = format!("{T5}fun use(): String {{\n    val repo = mock<Repo> {{ }}\n    return repo.find(1)\n}}\n");
    let e = calls(&[("p/Z.kt", &s)]);
    println!("lambda arg: {:?}", from(&e, "p/Z.kt::use"));
}

// ---------------- Python ----------------

const M: &str = "CONST = 1\nOTHER = 2\n";

fn py_one(user: &str, extra: &[(&str, &str)]) -> E {
    let mut files = vec![("pkg/__init__.py", ""), ("pkg/m.py", M), ("pkg/use.py", user)];
    files.extend_from_slice(extra);
    reads(&files)
}

#[test]
fn py_sibling_package_same_module_name() {
    let e = reads(&[
        ("pkg/__init__.py", ""),
        ("m.py", "CONST = 0\n"),
        ("pkg/sub/__init__.py", ""),
        ("pkg/sub/m.py", "CONST = 1\n"),
        ("pkg/m.py", "CONST = 2\n"),
        ("pkg/sub/use.py", "def check():\n    from . import m as cr\n    return cr.CONST\n"),
    ]);
    assert!(has(&e, "pkg/sub/use.py::check", "pkg/sub/m.py::CONST"), "{e:?}");
    assert_eq!(from(&e, "pkg/sub/use.py::check").len(), 1, "{e:?}");
}

#[test]
fn py_module_and_package_same_name() {
    for import in ["import a.b as cr", "from a import b as cr"] {
        let user = format!("def check():\n    {import}\n    return cr.CONST\n");
        let files = |u: &str| -> E {
            reads(&[
                ("a/__init__.py", ""),
                ("a/b.py", "CONST = 1\n"),
                ("a/b/__init__.py", "CONST = 2\n"),
                ("use.py", u),
            ])
        };
        let e = files(&user);
        let c = files(&format!("{import}\n\ndef check():\n    return cr.CONST\n"));
        println!("{import}: scoped {:?} control {:?}", from(&e, "use.py::check"), from(&c, "use.py::check"));
        assert_eq!(from(&e, "use.py::check"), from(&c, "use.py::check"), "scoped disagrees with module-level");
    }
}

#[test]
fn py_conditional_imports() {
    let e = py_one(
        "def check(flag):\n    if flag:\n        from . import m as cr\n    else:\n        from . import n as cr\n    return cr.CONST\n",
        &[("pkg/n.py", "CONST = 3\n")],
    );
    println!("{e:?}");
    assert!(from(&e, "pkg/use.py::check").is_empty(), "conditional alias bound: {e:?}");
}

#[test]
fn py_try_except_import_fallback() {
    let e = py_one(
        "def check():\n    try:\n        from . import fast as cr\n    except ImportError:\n        from . import m as cr\n    return cr.CONST\n",
        &[("pkg/fast.py", "CONST = 3\n")],
    );
    println!("{e:?}");
    assert!(from(&e, "pkg/use.py::check").is_empty(), "try/except alias bound: {e:?}");
}

#[test]
fn py_try_except_import_none_fallback() {
    let e = py_one(
        "def check():\n    try:\n        from . import m as cr\n    except ImportError:\n        cr = None\n    return cr.CONST if cr else 0\n",
        &[],
    );
    println!("{e:?}");
    assert!(from(&e, "pkg/use.py::check").is_empty(), "{e:?}");
}

#[test]
fn py_rebinding_forms_abstain() {
    let forms = [
        "    for cr in range(3):\n        pass\n    return cr.CONST\n",
        "    with open('f') as cr:\n        pass\n    return cr.CONST\n",
        "    cr, other = pair()\n    return cr.CONST\n",
        "    (cr := other())\n    return cr.CONST\n",
        "    try:\n        pass\n    except Exception as cr:\n        pass\n    return cr.CONST\n",
        "    def cr():\n        pass\n    return cr.CONST\n",
        "    class cr:\n        CONST = 1\n    return cr.CONST\n",
        "    return [cr.CONST for cr in mods]\n",
        "    return list(map(lambda cr: cr.CONST, mods))\n",
        "    return {cr.CONST for cr in mods}\n",
        "    import os as cr\n    return cr.CONST\n",
        "    cr += 1\n    return cr.CONST\n",
        "    cr: int = 3\n    return cr.CONST\n",
        "    del cr\n    return cr.CONST\n",
        "    for x in range(3):\n        cr = x\n    return cr.CONST\n",
        "    [cr] = pair()\n    return cr.CONST\n",
        "    *cr, y = pair()\n    return cr.CONST\n",
        "    match pair():\n        case cr:\n            pass\n    return cr.CONST\n",
        "    global cr\n    return cr.CONST\n",
    ];
    let mut bad = vec![];
    for (i, body) in forms.iter().enumerate() {
        let user = format!("def check():\n    from . import m as cr\n{body}");
        let e = py_one(&user, &[]);
        if has(&e, "pkg/use.py::check", "pkg/m.py::CONST") {
            bad.push(i);
            println!("BOUND form {i}:\n{user}");
        }
    }
    assert!(bad.is_empty(), "forms that rebind cr but still bound: {bad:?}");
}

#[test]
fn py_global_decl_before_import() {
    let e = py_one(
        "cr = None\n\ndef check():\n    global cr\n    from . import m as cr\n    return cr.CONST\n\ndef other():\n    return cr.CONST\n",
        &[],
    );
    println!("{e:?}");
    assert!(!has(&e, "pkg/use.py::other", "pkg/m.py::CONST"), "{e:?}");
}

#[test]
fn py_nested_function_reads_outer_import() {
    let e = py_one(
        "def check():\n    from . import m as cr\n    def inner():\n        return cr.CONST\n    return inner\n",
        &[],
    );
    println!("{e:?}");
}

#[test]
fn py_nested_function_param_shadows_outer_import() {
    let e = py_one(
        "def check():\n    from . import m as cr\n    def inner(cr):\n        return cr.CONST\n    return inner\n",
        &[],
    );
    println!("{e:?}");
    assert!(!has(&e, "pkg/use.py::check.inner", "pkg/m.py::CONST"), "param shadows import: {e:?}");
}

#[test]
fn py_inner_import_not_visible_in_outer_with_module_alias() {
    let e = py_one(
        "from . import n as cr\n\ndef check():\n    def inner():\n        from . import m as cr\n        return cr.CONST\n    return cr.CONST\n",
        &[("pkg/n.py", "CONST = 9\n")],
    );
    println!("{e:?}");
    assert!(!has(&e, "pkg/use.py::check", "pkg/m.py::CONST"), "outer bound to inner's import: {e:?}");
}

#[test]
fn py_other_function_param_named_like_alias() {
    let e = py_one(
        "def a():\n    from . import m as cr\n    return cr.CONST\n\ndef b(cr):\n    return cr.CONST\n\ndef c():\n    cr = make()\n    return cr.CONST\n\ndef d():\n    return cr.CONST\n",
        &[],
    );
    println!("{e:?}");
    for f in ["b", "c", "d"] {
        assert!(!has(&e, &format!("pkg/use.py::{f}"), "pkg/m.py::CONST"), "{f} wrongly bound: {e:?}");
    }
}

#[test]
fn py_method_reads_function_alias_from_other_method() {
    let e = py_one(
        "class K:\n    def a(self):\n        from . import m as cr\n        return cr.CONST\n\n    def b(self):\n        return cr.CONST\n\n    def c(self, cr):\n        return cr.CONST\n",
        &[],
    );
    println!("{e:?}");
    assert!(has(&e, "pkg/use.py::K.a", "pkg/m.py::CONST"), "{e:?}");
    assert!(!has(&e, "pkg/use.py::K.b", "pkg/m.py::CONST"), "{e:?}");
    assert!(!has(&e, "pkg/use.py::K.c", "pkg/m.py::CONST"), "{e:?}");
}

#[test]
fn py_class_body_reads() {
    let e = py_one(
        "def check():\n    from . import m as cr\n    class K:\n        X = cr.CONST\n        cr = 5\n        Y = cr.CONST\n    return K\n",
        &[],
    );
    println!("{e:?}");
}

#[test]
fn py_star_import_target() {
    let files = |u: &str| -> E {
        reads(&[
            ("pkg/__init__.py", ""),
            ("pkg/x.py", "CONST = 1\n"),
            ("pkg/m.py", "from .x import *\n"),
            ("pkg/use.py", u),
        ])
    };
    let e = files("def check():\n    from . import m as cr\n    return cr.CONST\n");
    let c = files("from . import m as cr\n\ndef check():\n    return cr.CONST\n");
    println!("star scoped {:?} control {:?}", from(&e, "pkg/use.py::check"), from(&c, "pkg/use.py::check"));
    for t in from(&e, "pkg/use.py::check") {
        assert_eq!(t, "pkg/x.py::CONST", "{e:?}");
    }
}

#[test]
fn py_reexport_const() {
    let files = |u: &str| -> E {
        reads(&[
            ("pkg/__init__.py", ""),
            ("pkg/x.py", "CONST = 1\n"),
            ("pkg/m.py", "from .x import CONST\n"),
            ("pkg/use.py", u),
        ])
    };
    let e = files("def check():\n    from . import m as cr\n    return cr.CONST\n");
    let c = files("from . import m as cr\n\ndef check():\n    return cr.CONST\n");
    println!("reexport scoped {:?} control {:?}", from(&e, "pkg/use.py::check"), from(&c, "pkg/use.py::check"));
    assert_eq!(from(&e, "pkg/use.py::check"), from(&c, "pkg/use.py::check"));
}

#[test]
fn py_function_local_alias_missing_attr() {
    let e = py_one("def check():\n    from . import m as cr\n    return cr.MISSING\n", &[]);
    assert!(from(&e, "pkg/use.py::check").is_empty(), "{e:?}");
}

#[test]
fn py_alias_package_attr_shadows_submodule() {
    let files = |u: &str| -> E {
        reads(&[
            ("pkg/__init__.py", "m = object()\nCONST = 7\n"),
            ("pkg/m.py", "CONST = 1\n"),
            ("pkg/use.py", u),
        ])
    };
    let e = files("def check():\n    from . import m as cr\n    return cr.CONST\n");
    let c = files("from . import m as cr\n\ndef check():\n    return cr.CONST\n");
    println!("init-shadow scoped {:?} control {:?}", from(&e, "pkg/use.py::check"), from(&c, "pkg/use.py::check"));
    assert_eq!(from(&e, "pkg/use.py::check"), from(&c, "pkg/use.py::check"));
}

#[test]
fn py_module_level_alias_to_other_module_other_function_reads_module() {
    let e = py_one(
        "from . import n as cr\n\ndef a():\n    from . import m as cr\n    return cr.CONST\n\ndef b():\n    return cr.CONST\n",
        &[("pkg/n.py", "CONST = 5\n")],
    );
    println!("{e:?}");
    assert!(!has(&e, "pkg/use.py::b", "pkg/m.py::CONST"), "{e:?}");
}

#[test]
fn py_import_after_read_in_function() {
    let e = py_one(
        "def check():\n    x = cr.CONST\n    from . import m as cr\n    return x\n",
        &[],
    );
    println!("read-before-import {e:?}");
}

#[test]
fn py_import_in_loop() {
    let e = py_one(
        "def check(names):\n    for n in names:\n        from . import m as cr\n    return cr.CONST\n",
        &[],
    );
    println!("{e:?}");
}

#[test]
fn parent_dir_backslash_paths_resolve_relative_imports() {
    let e = reads(&[
        ("pkg\\__init__.py", ""),
        ("pkg\\m.py", "CONST = 1\n"),
        ("pkg\\use.py", "from . import m as cr\n\ndef check():\n    return cr.CONST\n"),
    ]);
    println!("backslash: {e:?}");
}
