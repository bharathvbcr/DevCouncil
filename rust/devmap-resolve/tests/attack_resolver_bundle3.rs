//! Round-2 adversarial probes against lane D (Z3 hardened, python function-only
//! imports, overload abstain scope, M1 guard). Each test asserts the CORRECT
//! behaviour; a failure is a finding.
use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

type E = Vec<(String, String)>;

fn run(files: &[(&str, &str)], kind: EdgeKind) -> E {
    let extractions: Vec<_> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    for e in result.edges.iter().filter(|e| e.edge_kind == kind) {
        println!(
            "EDGE {} -> {} conf={:?}",
            e.source_symbol, e.target_symbol, e.confidence
        );
    }
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
fn has(e: &E, f: &str, t: &str) -> bool {
    e.iter().any(|(a, b)| a == f && b == t)
}

const REPO_A: (&str, &str) = (
    "a/Repo.kt",
    "package a\n\nclass Repo<T> {\n    fun find(): Int = 1\n}\n",
);
const FIND: &str = "a/Repo.kt::Repo.find";

// ------------------------------ Z3 ------------------------------

#[test]
fn z3_alias_named_like_a_repo_type_is_not_the_repo_type() {
    let callee = "package a\n\nimport ext.Thing as Repo\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, ("a/F.kt", callee)]);
    assert!(
        !has(&e, "a/F.kt::use", FIND),
        "alias bound to repo type: {e:?}"
    );
}

#[test]
fn z3_nested_class_is_not_a_top_level_name() {
    let outer =
        "package a\n\nclass Outer {\n    class Repo<T> {\n        fun find(): Int = 1\n    }\n}\n";
    let callee = "package a\n\nimport ext.*\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[("a/Outer.kt", outer), ("a/F.kt", callee)]);
    assert!(
        !e.iter()
            .any(|(f, t)| f == "a/F.kt::use" && t.contains("Repo.find")),
        "{e:?}"
    );
}

#[test]
fn z3_overloads_with_different_returns_do_not_borrow_the_generic_return() {
    let src = "package p\n\nprivate fun List<String>.total(): Int = size\n\nfun names(): List<String> = listOf(\"a\")\n\nfun names(x: Int): Int = x\n\nfun use(): Int {\n    val n = names(1)\n    return n.total()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "{e:?}");
}

#[test]
fn z3_overloads_nominal_baseline() {
    let src = "package p\n\nclass Repo { fun find(): Int = 1 }\n\nfun make(): Repo = Repo()\n\nfun make(x: Int): Int = x\n\nfun use(): Int {\n    val n = make(1)\n    return n.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z3_chain_does_not_type_by_the_head_call() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1; fun next(): Int = 2 }\n\nfun make(): Repo<String> = Repo()\n\nfun use(): Int {\n    val n = make().next()\n    return n.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z3_extension_with_same_name_as_generic_toplevel() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1 }\nclass Other\n\nfun names(): Repo<String> = Repo()\n\nfun Other.names(): Int = 1\n\nfun use(o: Other): Int {\n    val n = o.names()\n    return n.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z3_member_shadows_toplevel_generic_factory() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1 }\n\nfun make(): Repo<String> = Repo()\n\nclass C {\n    fun make(): Int = 1\n    fun use(): Int {\n        val n = make()\n        return n.find()\n    }\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::C.use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn z3_default_import_name_repo_declared_and_explicitly_imported_binds() {
    let res = (
        "a/Result.kt",
        "package a\n\nclass Result<T> {\n    fun get(): Int = 1\n}\n",
    );
    let f = "package b\n\nimport a.Result\n\nfun make(): Result<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.get()\n}\n";
    let e = calls(&[res, ("b/F.kt", f)]);
    assert!(
        has(&e, "b/F.kt::use", "a/Result.kt::Result.get"),
        "recall: {e:?}"
    );
}

#[test]
fn z3_default_import_name_not_imported_does_not_bind() {
    let res = (
        "a/Result.kt",
        "package a\n\nclass Result<T> {\n    fun get(): Int = 1\n}\n",
    );
    let f = "package b\n\nfun make(): Result<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.get()\n}\n";
    let e = calls(&[res, ("b/F.kt", f)]);
    assert!(!has(&e, "b/F.kt::use", "a/Result.kt::Result.get"), "{e:?}");
}

#[test]
fn z3_explicit_ext_import_with_star_import_of_repo_package() {
    let callee = "package b\n\nimport ext.Repo\nimport a.*\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, ("b/F.kt", callee)]);
    assert!(!has(&e, "b/F.kt::use", FIND), "{e:?}");
}

#[test]
fn z3_two_files_different_directories_same_package_recall() {
    let callee = "package a\n\nfun make(): Repo<String> = TODO()\n";
    let user = "package a\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, ("x/y/F.kt", callee), ("z/U.kt", user)]);
    // Recall only (LOW 7, recorded): `lookup_in_package` finds a callee by
    // directory, not by Kotlin package, so a same-package callee in another
    // directory is not reached for nominal and generic factories alike. No
    // wrong edge may appear.
    assert!(
        !e.iter()
            .any(|(f, t)| f == "z/U.kt::use" && t != "x/y/F.kt::make"),
        "{e:?}"
    );
}

#[test]
fn z3_use_file_imports_other_repo_type_of_same_name() {
    let other = (
        "q2/Repo.kt",
        "package q2\n\nclass Repo<T> {\n    fun find(): Int = 1\n}\n",
    );
    let callee = "package a\n\nfun make(): Repo<String> = TODO()\n";
    let user = "package q\n\nimport q2.Repo\nimport a.make\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, other, ("a/F.kt", callee), ("q/U.kt", user)]);
    // `use -> make` is the legitimate call; only a bound `find` would be wrong.
    assert!(
        !e.iter()
            .any(|(f, t)| f == "q/U.kt::use" && t.ends_with("find")),
        "{e:?}"
    );
}

#[test]
fn z3_two_libs_same_stdlib_name_two_files() {
    let callee = "package b\n\nimport lib1.Foo\n\nfun make(): Foo<String> = TODO()\n";
    let user = "package c\n\nimport b.make\nimport lib2.Foo\n\nprivate fun Foo<String>.ping(): Int = 1\n\nfun use(): Int {\n    val r = make()\n    return r.ping()\n}\n";
    let e = calls(&[("b/F.kt", callee), ("c/U.kt", user)]);
    assert!(
        !has(&e, "c/U.kt::use", "c/U.kt::Foo.ping"),
        "lib1.Foo is not lib2.Foo: {e:?}"
    );
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn z3_same_file_local_class_list_recall() {
    let src = "package p\n\nclass List<T> { fun total(): Int = 1 }\n\nfun names(): List<String> = List()\n\nfun use(): Int {\n    val n = names()\n    return n.total()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(
        has(&e, "p/Z.kt::use", "p/Z.kt::List.total"),
        "recall: {e:?}"
    );
}

#[test]
fn z3_use_file_local_class_list_callee_file_stdlib() {
    let callee = "package b\n\nfun names(): List<String> = listOf()\n";
    let user = "package c\n\nimport b.names\n\nclass List<T> { fun total(): Int = 1 }\n\nfun use(): Int {\n    val n = names()\n    return n.total()\n}\n";
    let e = calls(&[("b/F.kt", callee), ("c/U.kt", user)]);
    assert!(!has(&e, "c/U.kt::use", "c/U.kt::List.total"), "{e:?}");
}

#[test]
fn z3_when_three_branches_one_null() {
    let src = "package p\n\nprivate fun List<String>.total(): Int = size\n\nfun a(): List<String> = listOf()\nfun b(): List<String> = listOf()\n\nfun use(k: Int): Int {\n    val v = when (k) {\n        1 -> a()\n        2 -> b()\n        else -> null\n    }\n    return v?.total() ?: 0\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    println!("when3null: {e:?}");
}

#[test]
fn z3_when_branches_disagree() {
    let src = "package p\n\nprivate fun List<String>.total(): Int = size\n\nfun a(): List<String> = listOf()\nfun b(): Set<String> = setOf()\n\nfun use(k: Int): Int {\n    val v = when (k) {\n        1 -> a()\n        2 -> b()\n        else -> a()\n    }\n    return v.total()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "{e:?}");
}

#[test]
fn z3_when_disagree_first_is_the_odd_one() {
    let src = "package p\n\nprivate fun List<String>.total(): Int = size\n\nfun a(): List<String> = listOf()\nfun b(): Set<String> = setOf()\n\nfun use(k: Int): Int {\n    val v = when (k) {\n        1 -> b()\n        2 -> a()\n        else -> a()\n    }\n    return v.total()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "{e:?}");
}

#[test]
fn z3_listof_mixed_with_repo_factory() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1 }\n\nfun make(): Repo<String> = Repo()\n\nfun use(c: Boolean): Int {\n    val v = if (c) make() else listOf(1)\n    return v.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z3_emptylist_redeclared_by_repo() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1 }\n\nfun emptyList(): Repo<String> = Repo()\n\nfun make(): List<String> = listOf()\n\nprivate fun List<String>.total(): Int = size\n\nfun use(c: Boolean): Int {\n    val v = if (c) make() else emptyList()\n    return v.total()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::List.total"), "{e:?}");
}

#[test]
fn z3_nested_call_in_argument_of_other_type() {
    let src = "package p\n\nclass Box<T> { fun find(): Int = 1 }\n\nfun wrap(x: Int): Box<Int> = Box()\n\nfun raw(): Set<String> = setOf()\n\nfun use(): Int {\n    val v = wrap(raw().size)\n    return v.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    println!("nested arg: {e:?}");
}

#[test]
fn z3_argument_call_of_other_type_abstains() {
    // outer wrap(...) returns Box<Int>; but initializer is `id(wrap2())` where
    // id() returns Other<..> and wrap2() returns Box: shape=id, other callee Box != Other.
    let src = "package p\n\nclass Box<T> { fun find(): Int = 1 }\nclass Other<T> { fun find(): Int = 2 }\n\nfun id(x: Any): Other<Int> = Other()\n\nfun wrap2(): Box<Int> = Box()\n\nfun use(): Int {\n    val v = id(wrap2())\n    return v.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Box.find"), "{e:?}");
}

#[test]
fn z3_typealias_generic_return() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1 }\n\ntypealias Rs<T> = Repo<T>\n\nfun make(): Rs<String> = Repo()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    println!("typealias: {e:?}");
}

#[test]
fn z3_typealias_shadows_stdlib_name() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1 }\n\nclass Other<T> { fun total(): Int = 1 }\n\ntypealias List<T> = Other<T>\n\nfun make(): List<String> = Other()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
fn z3_no_package_clause_two_files_same_dir() {
    let repo = ("Repo.kt", "class Repo<T> {\n    fun find(): Int = 1\n}\n");
    let callee = ("F.kt", "fun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n");
    let e = calls(&[repo, callee]);
    println!("no pkg: {e:?}");
}

#[test]
fn z3_no_package_clause_vs_packaged_repo() {
    let callee = ("F.kt", "fun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n");
    let e = calls(&[REPO_A, callee]);
    assert!(!has(&e, "F.kt::use", FIND), "{e:?}");
}

#[test]
fn z3_java_class_same_name_imported_explicitly() {
    let java = (
        "ext/Repo.java",
        "package ext;\n\npublic class Repo<T> {\n    public int find() { return 1; }\n}\n",
    );
    let callee = "package a\n\nimport ext.Repo\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, java, ("a/F.kt", callee)]);
    assert!(!has(&e, "a/F.kt::use", FIND), "{e:?}");
}

#[test]
fn z3_explicit_import_of_nested_type() {
    let outer = (
        "a/Outer.kt",
        "package a\n\nclass Outer {\n    class Repo<T> { fun find(): Int = 1 }\n}\n",
    );
    let callee = "package b\n\nimport a.Outer.Repo\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, outer, ("b/F.kt", callee)]);
    assert!(!has(&e, "b/F.kt::use", FIND), "{e:?}");
}

#[test]
fn z3_extension_returning_generic() {
    let src = "package p\n\nclass Repo<T> { fun find(): Int = 1 }\nclass Src\n\nfun Src.items(): Repo<String> = Repo()\n\nfun use(s: Src): Int {\n    val r = s.items()\n    return r.find()\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    println!("ext generic: {e:?}");
}

// ------------------------------ Overloads ------------------------------

#[test]
fn ov_java_inner_classes_same_name_methods_keep_recursion() {
    let a = "package p;\n\nclass A {\n    static class Inner {\n        int run(int n) { return n > 0 ? run(n - 1) : 0; }\n    }\n}\n";
    let b = "package p;\n\nclass B {\n    static class Inner {\n        int run(int n) { return n > 0 ? run(n - 2) : 0; }\n    }\n}\n";
    let e = calls(&[("p/A.java", a), ("p/B.java", b)]);
    println!("java inner 2 files: {e:?}");
    let one = "package p;\n\nclass A {\n    static class Inner {\n        int run(int n) { return n > 0 ? run(n - 1) : 0; }\n    }\n    static class Other {\n        int run(int n) { return n > 0 ? run(n - 1) : 0; }\n    }\n}\n";
    let e = calls(&[("p/A.java", one)]);
    assert!(
        e.iter().any(|(f, t)| f == t && f.contains("Inner")),
        "Inner.run recursion lost: {e:?}"
    );
    assert!(
        e.iter().any(|(f, t)| f == t && f.contains("Other")),
        "Other.run recursion lost: {e:?}"
    );
}

/// Recorded for extract-v98 (the extractor mints one id for `A.Inner.run` and
/// `B.Inner.run`): the two declarations are siblings of one id, so the call
/// inside either cannot be told from a call to the other and the resolver
/// abstains. The loss is the recursion edge; no wrong edge is asserted away.
#[test]
fn ov_java_two_outer_each_with_inner_same_name_one_file() {
    let one = "package p;\n\nclass A {\n    static class Inner {\n        int run(int n) { return n > 0 ? run(n - 1) : 0; }\n    }\n}\n\nclass B {\n    static class Inner {\n        int run(int n) { return n > 0 ? run(n - 2) : 0; }\n    }\n}\n";
    let e = calls(&[("p/A.java", one)]);
    println!("two Inner one file: {e:?}");
    assert!(
        e.iter().all(|(f, t)| f != t || f.contains("Inner")),
        "{e:?}"
    );
}

#[test]
fn ov_java_instance_and_static_nested_overload_recursion_true_pair() {
    let src = "package p;\n\nclass C {\n    int add(int a) { return a; }\n    int add(String s) { return add(s.length()); }\n}\n";
    let e = calls(&[("p/C.java", src)]);
    assert!(!e.iter().any(|(f, t)| f == t), "overload self edge: {e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn ov_csharp_partial_classes_across_files_sibling_call_not_self() {
    let a = "namespace N {\n  partial class C {\n    int F(int a) { return a; }\n  }\n}\n";
    let b =
        "namespace N {\n  partial class C {\n    int F(string s) { return F(s.Length); }\n  }\n}\n";
    let e = calls(&[("N/A.cs", a), ("N/B.cs", b)]);
    println!("partial: {e:?}");
    assert!(
        !e.iter().any(|(f, t)| f == t),
        "self edge for a call to the other partial's F(int): {e:?}"
    );
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn ov_swift_extensions_across_files_sibling_call_not_self() {
    let a = "struct T {}\nextension T {\n  func f(_ a: Int) -> Int { return a }\n}\n";
    let b = "extension T {\n  func f(_ s: String) -> Int { return f(s.count) }\n}\n";
    let e = calls(&[("A.swift", a), ("B.swift", b)]);
    println!("swift ext files: {e:?}");
    assert!(
        !e.iter().any(|(f, t)| f == t),
        "self edge for call to the other file's f(Int): {e:?}"
    );
}

#[test]
fn ov_swift_extensions_same_file_overload_siblings() {
    let a = "struct T {}\nextension T {\n  func f(_ a: Int) -> Int { return a }\n}\nextension T {\n  func f(_ s: String) -> Int { return f(s.count) }\n}\n";
    let e = calls(&[("A.swift", a)]);
    assert!(!e.iter().any(|(f, t)| f == t), "{e:?}");
}

#[test]
fn ov_swift_extension_real_recursion_single_decl() {
    let a = "extension Int {\n  func fact() -> Int { return self <= 1 ? 1 : self * (self - 1).fact() }\n}\nstruct S { func g(_ n: Int) -> Int { return n > 0 ? g(n - 1) : 0 } }\n";
    let e = calls(&[("A.swift", a)]);
    assert!(
        e.iter().any(|(f, t)| f == t && f.contains("g")),
        "S.g recursion lost: {e:?}"
    );
}

#[test]
fn ov_swift_protocol_requirement_plus_default_impl_recursion_elsewhere() {
    let a = "protocol P {\n  func f()\n  func g()\n}\nextension P {\n  func f() { g() }\n  func g() { f() }\n}\nstruct S: P {\n  func f() {}\n  func g() {}\n}\n";
    let e = calls(&[("A.swift", a)]);
    println!("proto default: {e:?}");
}

#[test]
fn ov_swift_protocol_default_impl_and_requirement_same_name_recursion_through_other() {
    // P.f requirement + P.f default impl => same id twice, not overloads.
    // A default impl `h` calling `f()` must still fan out to conformers.
    let a = "protocol P {\n  func f()\n}\nextension P {\n  func f() {}\n  func h() { f() }\n}\nstruct S: P {\n  func f() {}\n}\n";
    let e = calls(&[("A.swift", a)]);
    println!("proto req+default: {e:?}");
    assert!(
        e.iter()
            .any(|(f, t)| f.ends_with("P.h") && t.ends_with("S.f")),
        "fan-out lost: {e:?}"
    );
}

#[test]
fn ov_scala_def_overloads_vs_companion_object() {
    let src = "class C {\n  def add(a: Int): Int = a\n}\n\nobject C {\n  def make(n: Int): C = new C()\n  def make(s: String): C = make(s.length)\n}\n";
    let e = calls(&[("C.scala", src)]);
    assert!(!e.iter().any(|(f, t)| f == t), "{e:?}");
}

#[test]
fn ov_scala_true_recursion_in_distinct_objects() {
    let src = "object A {\n  def go(n: Int): Int = if (n > 0) go(n - 1) else 0\n}\nobject B {\n  def go(n: Int): Int = if (n > 0) go(n - 1) else 0\n}\n";
    let e = calls(&[("C.scala", src)]);
    assert!(e.iter().filter(|(f, t)| f == t).count() == 2, "{e:?}");
}

#[test]
fn ov_kotlin_expect_actual_same_file() {
    let src = "package p\n\nexpect fun plat(a: Int): Int\n\nactual fun plat(a: Int): Int = if (a > 0) plat(a - 1) else 0\n";
    let e = calls(&[("p/Z.kt", src)]);
    println!("expect/actual: {e:?}");
}

#[test]
fn ov_kotlin_member_and_extension_pair_call_member_not_abstained() {
    let src = "package p\n\nclass C {\n    fun add(a: Int): Int = a\n}\n\nfun C.add(s: String): Int = add(s.length)\n\nfun use(c: C): Int = c.add(1)\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(has(&e, "p/Z.kt::use", "p/Z.kt::C.add"), "{e:?}");
}

#[test]
fn ov_kotlin_two_extension_overloads_on_different_receivers_recursion() {
    // fun A.go() / fun B.go() have distinct qualified names? if shared, true recursion lost.
    let src = "package p\n\nclass A\nclass B\n\nfun A.go(n: Int): Int = if (n > 0) go(n - 1) else 0\n\nfun B.go(n: Int): Int = if (n > 0) go(n - 1) else 0\n";
    let e = calls(&[("p/Z.kt", src)]);
    println!("two ext receivers: {e:?}");
    assert!(
        e.iter().filter(|(f, t)| f == t).count() == 2,
        "true recursion lost on distinct receivers: {e:?}"
    );
}

#[test]
fn ov_kotlin_local_funs_in_one_function_siblings() {
    let src = "package p\n\nfun a(): Int {\n    fun helper(n: Int): Int = n\n    fun helper(s: String): Int = helper(s.length)\n    return helper(3)\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!e.iter().any(|(f, t)| f == t), "{e:?}");
}

#[test]
fn ov_kotlin_member_and_toplevel_same_qn() {
    // top-level `fun run` and class member `run` are distinct ids; recursion in each kept.
    let src = "package p\n\nfun run(n: Int): Int = if (n > 0) run(n - 1) else 0\n\nclass C {\n    fun run(n: Int): Int = if (n > 0) run(n - 1) else 0\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(e.iter().filter(|(f, t)| f == t).count() == 2, "{e:?}");
}

#[test]
fn ov_swift_class_and_nested_type_same_method_name() {
    let src = "class Outer {\n  func go(_ n: Int) -> Int { return n > 0 ? go(n - 1) : 0 }\n  class Inner {\n    func go(_ n: Int) -> Int { return n > 0 ? go(n - 1) : 0 }\n  }\n}\n";
    let e = calls(&[("A.swift", src)]);
    println!("swift nested: {e:?}");
    assert!(e.iter().filter(|(f, t)| f == t).count() == 2, "{e:?}");
}

#[test]
fn ov_swift_init_overloads_and_convenience() {
    let src = "class C {\n  init(_ a: Int) {}\n  convenience init(_ s: String) { self.init(s.count) }\n}\n";
    let e = calls(&[("A.swift", src)]);
    println!("swift init: {e:?}");
    assert!(!e.iter().any(|(f, t)| f == t), "init self-edge: {e:?}");
}

#[test]
fn m1_protocol_extension_overload_delegating_to_other_conformers() {
    let a = "protocol Adder {\n  func add(_ a: Int) -> Int\n  func add(_ a: String) -> Int\n}\n\nextension Adder {\n  func add(_ a: String) -> Int { return add(a.count) }\n}\n";
    let b = "class Calc: Adder {\n  func add(_ a: Int) -> Int { return a }\n}\n\nclass Calc2: Adder {\n  func add(_ a: Int) -> Int { return a + 1 }\n}\n";
    let e = calls(&[("A.swift", a), ("B.swift", b)]);
    println!("m1 conformers: {e:?}");
    // The call `add(a.count)` is add(Int): a true edge to each conformer's add exists.
    assert!(
        e.iter()
            .any(|(f, t)| f.ends_with("Adder.add") && t.ends_with("Calc.add")),
        "cross-conformer fan-out lost: {e:?}"
    );
}

#[test]
fn m1_protocol_extension_nonoverloaded_fan_out_kept() {
    let a = "protocol Greeter {\n  func name() -> String\n}\n\nextension Greeter {\n  func greet() -> String { return name() }\n}\n\nclass A: Greeter {\n  func name() -> String { return \"a\" }\n}\n\nclass B: Greeter {\n  func name() -> String { return \"b\" }\n}\n";
    let e = calls(&[("A.swift", a)]);
    assert!(
        e.iter()
            .any(|(f, t)| f.ends_with("Greeter.greet") && t.ends_with("A.name")),
        "{e:?}"
    );
    assert!(
        e.iter()
            .any(|(f, t)| f.ends_with("Greeter.greet") && t.ends_with("B.name")),
        "{e:?}"
    );
}

#[test]
fn m1_conformer_overloads_call_each_other() {
    let a = "protocol Adder {\n  func add(_ a: Int) -> Int\n}\nclass Calc: Adder {\n  func add(_ a: Int) -> Int { return a }\n  func add(_ s: String) -> Int { return add(s.count) }\n}\nfunc go(_ c: Calc) -> Int { return c.add(1) }\n";
    let e = calls(&[("A.swift", a)]);
    println!("m1 conformer: {e:?}");
    assert!(!e.iter().any(|(f, t)| f == t), "{e:?}");
    assert!(
        e.iter()
            .any(|(f, t)| f.ends_with("::go") && t.ends_with("Calc.add")),
        "{e:?}"
    );
}

// ---- baselines / blame ----

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn base_member_shadows_toplevel_nominal_factory() {
    let src = "package p\n\nclass Repo { fun find(): Int = 1 }\n\nfun make(): Repo = Repo()\n\nclass C {\n    fun make(): Int = 1\n    fun use(): Int {\n        val n = make()\n        return n.find()\n    }\n}\n";
    let e = calls(&[("p/Z.kt", src)]);
    assert!(!has(&e, "p/Z.kt::C.use", "p/Z.kt::Repo.find"), "{e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn base_star_import_after_explicit_nominal() {
    let repo = (
        "a/Repo.kt",
        "package a\n\nclass Repo {\n    fun find(): Int = 1\n}\n",
    );
    let src = "package b\n\nimport ext.Repo\nimport a.*\n\nfun use(r: Repo): Int {\n    return r.find()\n}\n";
    let e = calls(&[repo, ("b/F.kt", src)]);
    assert!(!has(&e, "b/F.kt::use", "a/Repo.kt::Repo.find"), "{e:?}");
}

#[test]
fn base_star_import_before_explicit_generic() {
    let src = "package b\n\nimport a.*\nimport ext.Repo\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, ("b/F.kt", src)]);
    assert!(!has(&e, "b/F.kt::use", FIND), "{e:?}");
}

#[test]
#[ignore = "ft-2c800986: pre-existing, reverted per Fable 2026-10-21"]
fn base_nested_class_nominal_factory() {
    let outer = "package a\n\nclass Outer {\n    class Repo { fun find(): Int = 1 }\n}\n";
    let callee = "package a\n\nimport ext.*\n\nfun make(): Repo = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[("a/Outer.kt", outer), ("a/F.kt", callee)]);
    assert!(
        !e.iter()
            .any(|(f, t)| f == "a/F.kt::use" && t.contains("Repo.find")),
        "{e:?}"
    );
}

#[test]
fn z3_star_import_of_repo_pkg_only_no_explicit_recall_or_precision() {
    // `import a.*` + generic return `Repo<String>`: truly a.Repo. Either abstain or bind.
    let src = "package b\n\nimport a.*\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, ("b/F.kt", src)]);
    println!("star only: {e:?}");
}

#[test]
fn z3_star_ext_and_star_repo_ambiguous_no_bind() {
    let src = "package b\n\nimport a.*\nimport ext.*\n\nfun make(): Repo<String> = TODO()\n\nfun use(): Int {\n    val r = make()\n    return r.find()\n}\n";
    let e = calls(&[REPO_A, ("b/F.kt", src)]);
    println!("two stars: {e:?}");
}
