//! Round-3 adversarial probes against lane D's round-2 changes (JVM import
//! package rung, star-vs-explicit imports, receiver abstain on a same-named
//! import, enclosing-member abstain, split-type overload ids, python
//! function-only imports). Each test asserts the CORRECT behaviour; a failure
//! is a finding.
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
        .map(|e| {
            println!(
                "EDGE {:?} {} -> {} conf={:?}",
                kind, e.source_symbol, e.target_symbol, e.confidence
            );
            (e.source_symbol.clone(), e.target_symbol.clone())
        })
        .collect();
    v.sort();
    v
}
fn calls(files: &[(&str, &str)]) -> E {
    run(files, EdgeKind::Calls)
}
fn imports(files: &[(&str, &str)]) -> E {
    run(files, EdgeKind::Imports)
}
/// `file` is `path` or `path::Qualified.member`; the qualified form is matched
/// on the path prefix and the member's last segment (nested ids drop their outer).
fn to_file(e: &E, from: &str, file: &str) -> bool {
    let (path, member) = match file.split_once("::") {
        Some((p, q)) => (p, Some(q.rsplit('.').next().unwrap_or(q))),
        None => (file, None),
    };
    e.iter().any(|(a, b)| {
        a.starts_with(from)
            && b.starts_with(path)
            && member.is_none_or(|m| b.rsplit(['.', ':']).next() == Some(m))
    })
}

// ------------------------------ (1) JVM import rung ------------------------------

#[test]
fn j1_scala_sbt_multimodule_import_binds() {
    let foo = "package com.x\n\nobject Foo {\n  def f(): Int = 1\n}\n";
    let main = "package com.y\n\nimport com.x.Foo\n\nobject Main {\n  def run(): Int = Foo.f()\n}\n";
    let files = [
        ("core/src/main/scala/com/x/Foo.scala", foo),
        ("app/src/main/scala/com/y/Main.scala", main),
    ];
    let i = imports(&files);
    assert!(
        to_file(&i, "app/src/main/scala/com/y/Main.scala", "core/src/main/scala/com/x/Foo.scala"),
        "scala import lost: {i:?}"
    );
    let c = calls(&files);
    assert!(to_file(&c, "app/src/main/scala/com/y/Main.scala::Main.run", "core/src/main/scala/com/x/Foo.scala::Foo.f"), "{c:?}");
}

#[test]
fn j2_kotlin_object_member_import_dir_mismatch() {
    let u = "package a\n\nobject Util {\n    fun go(): Int = 1\n}\n";
    let m = "package b\n\nimport a.Util.go\n\nfun run(): Int = go()\n";
    let files = [("lib/Util.kt", u), ("app/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "app/M.kt::run", "lib/Util.kt::Util.go"), "{c:?}");
}

#[test]
fn j3_java_static_import_dir_mismatch() {
    let u = "package a;\n\npublic class B {\n    public static int c() { return 1; }\n}\n";
    let m = "package b;\n\nimport static a.B.c;\n\npublic class M {\n    int run() { return c(); }\n}\n";
    let files = [("lib/B.java", u), ("app/M.java", m)];
    let c = calls(&files);
    assert!(to_file(&c, "app/M.java::M.run", "lib/B.java::B.c"), "{c:?}");
}

#[test]
fn j4_kotlin_nested_class_import_dir_mismatch() {
    let u = "package a\n\nclass Outer {\n    class Inner {\n        fun go(): Int = 1\n    }\n}\n";
    let m = "package b\n\nimport a.Outer.Inner\n\nfun run(i: Inner): Int = i.go()\n";
    let files = [("lib/Outer.kt", u), ("app/M.kt", m)];
    let i = imports(&files);
    assert!(to_file(&i, "app/M.kt", "lib/Outer.kt"), "import lost {i:?}");
    let c = calls(&files);
    assert!(to_file(&c, "app/M.kt::run", "lib/Outer.kt::Outer.Inner.go"), "{c:?}");
}

#[test]
fn j5_kotlin_alias_import_dir_mismatch() {
    let u = "package a\n\nclass Foo {\n    fun go(): Int = 1\n}\n";
    let m = "package b\n\nimport a.Foo as F\n\nfun run(f: F): Int = f.go()\n";
    let files = [("lib/Foo.kt", u), ("app/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "app/M.kt::run", "lib/Foo.kt::Foo.go"), "{c:?}");
}

#[test]
fn j6_java_import_of_kotlin_file_dir_mismatch() {
    let u = "package a\n\nclass Foo {\n    fun go(): Int = 1\n}\n";
    let m = "package b;\n\nimport a.Foo;\n\npublic class M {\n    int run(Foo f) { return f.go(); }\n}\n";
    let files = [("lib/Foo.kt", u), ("app/M.java", m)];
    let c = calls(&files);
    assert!(to_file(&c, "app/M.java::M.run", "lib/Foo.kt::Foo.go"), "{c:?}");
}

#[test]
fn j7_kotlin_file_with_file_annotation_before_package() {
    let u = "@file:Suppress(\"unused\")\npackage a\n\nclass Foo {\n    fun go(): Int = 1\n}\n";
    let m = "package b\n\nimport a.Foo\n\nfun run(f: Foo): Int = f.go()\n";
    let files = [("lib/Foo.kt", u), ("app/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "app/M.kt::run", "lib/Foo.kt::Foo.go"), "{c:?}");
}

#[test]
fn j8_scala_nested_package_blocks_import() {
    let u = "package a {\n  package b {\n    object Foo {\n      def f(): Int = 1\n    }\n  }\n}\n";
    let m = "package z\n\nimport a.b.Foo\n\nobject M {\n  def run(): Int = Foo.f()\n}\n";
    let files = [("lib/Foo.scala", u), ("app/M.scala", m)];
    let c = calls(&files);
    assert!(to_file(&c, "app/M.scala::M.run", "lib/Foo.scala::Foo.f"), "{c:?}");
}

#[test]
fn j9_library_import_does_not_bind_repo_stem() {
    // The guarantee itself: library package never binds.
    let u = "package a\n\nclass Repo {\n    fun find(): Int = 1\n}\n";
    let m = "package b\n\nimport ext.Repo\n\nfun run(r: Repo): Int = r.find()\n";
    let files = [("lib/Repo.kt", u), ("app/M.kt", m)];
    let c = calls(&files);
    assert!(!to_file(&c, "app/M.kt::run", "lib/Repo.kt"), "{c:?}");
}

#[test]
fn j10_kotlin_import_of_package_declared_only_by_other_file_binds_stem() {
    // typealias in package `a` file Aliases.kt; Foo.kt declares package `a.impl`.
    let al = "package a\n\ntypealias Foo = a.impl.Foo\n";
    let foo = "package a.impl\n\nclass Foo {\n    fun go(): Int = 1\n}\n";
    let m = "package b\n\nimport a.Foo\n\nfun run(f: Foo): Int = f.go()\n";
    let files = [("lib/Aliases.kt", al), ("impl/Foo.kt", foo), ("app/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "app/M.kt::run", "impl/Foo.kt::Foo.go"), "{c:?}");
}

// ------------------------------ (2) explicit vs star ------------------------------

#[test]
fn s1_explicit_repo_import_beats_star_of_other_repo_pkg_either_order() {
    let a = "package a\n\nclass Repo {\n    fun find(): Int = 1\n}\n";
    let b = "package b\n\nclass Repo {\n    fun find(): Int = 2\n}\n";
    for order in 0..2 {
        let m = if order == 0 {
            "package c\n\nimport a.Repo\nimport b.*\n\nfun run(r: Repo): Int = r.find()\n"
        } else {
            "package c\n\nimport b.*\nimport a.Repo\n\nfun run(r: Repo): Int = r.find()\n"
        };
        let files = [("a/Repo.kt", a), ("b/Repo.kt", b), ("c/M.kt", m)];
        let c = calls(&files);
        assert!(to_file(&c, "c/M.kt::run", "a/Repo.kt::Repo.find"), "order {order}: {c:?}");
        assert!(!to_file(&c, "c/M.kt::run", "b/Repo.kt"), "order {order}: {c:?}");
    }
}

#[test]
fn s2_star_import_kept_when_unrelated_external_name_imported() {
    let a = "package a\n\nclass Repo {\n    fun find(): Int = 1\n}\n";
    let m = "package c\n\nimport a.*\nimport ext.Other\n\nfun run(r: Repo, o: Other): Int = r.find()\n";
    let files = [("a/Repo.kt", a), ("c/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "c/M.kt::run", "a/Repo.kt::Repo.find"), "{c:?}");
}

#[test]
fn s3_java_star_import_plus_external_same_name_function_static() {
    // Java static import of an external `verify` beside a star import of a repo
    // class holding a *method* named verify: different namespaces for types, but
    // the star binding of the TYPE `Verify` must stay.
    let a = "package a;\n\npublic class Helper {\n    public static int help() { return 1; }\n}\n";
    let m = "package c;\n\nimport a.*;\nimport static org.junit.Assert.help;\n\npublic class M {\n    int run() { return Helper.help(); }\n}\n";
    let files = [("a/Helper.java", a), ("c/M.java", m)];
    let c = calls(&files);
    assert!(to_file(&c, "c/M.java::M.run", "a/Helper.java::Helper.help"), "{c:?}");
}

#[test]
fn s4_kotlin_star_import_class_vs_external_function_same_name() {
    // `import icons.filled.Person` (a property) beside `import a.*` where a has
    // class Person: the class is still reachable as a constructor/type.
    let a = "package a\n\nclass Person {\n    fun greet(): Int = 1\n}\n";
    let m = "package c\n\nimport a.*\nimport icons.filled.Person\n\nfun run(p: Person): Int = p.greet()\n";
    let files = [("a/Person.kt", a), ("c/M.kt", m)];
    let c = calls(&files);
    // ambiguous: abstain is acceptable; a binding is acceptable; document.
    println!("s4 {c:?}");
}

#[test]
fn s5_own_package_star_import_with_external_same_name_keeps_nothing_wrong() {
    // `import a.*` in a file of package a, plus explicit `import ext.Repo`: Java
    // and Kotlin both say the explicit import wins over the same package.
    let a = "package a\n\nclass Repo {\n    fun find(): Int = 1\n}\n";
    let m = "package a\n\nimport a.*\nimport ext.Repo\n\nfun run(r: Repo): Int = r.find()\n";
    let files = [("a/Repo.kt", a), ("a/M.kt", m)];
    let c = calls(&files);
    assert!(!to_file(&c, "a/M.kt::run", "a/Repo.kt::Repo.find"), "{c:?}");
}

// ------------------------------ (3) receiver abstain ------------------------------

#[test]
fn r1_explicit_repo_import_of_same_named_class_still_binds() {
    for name in ["Result", "Task", "Event", "User", "Config"] {
        let a = format!("package a\n\nclass {name} {{\n    fun find(): Int = 1\n}}\n");
        let m = format!("package b\n\nimport a.{name}\n\nfun run(r: {name}): Int = r.find()\n");
        let path_a = format!("a/{name}.kt");
        let files = [(path_a.as_str(), a.as_str()), ("b/M.kt", m.as_str())];
        let c = calls(&files);
        assert!(
            to_file(&c, "b/M.kt::run", &format!("a/{name}.kt::{name}.find")),
            "{name}: {c:?}"
        );
    }
}

#[test]
fn r2_repo_import_with_dir_mismatch_still_binds() {
    for name in ["Result", "Task"] {
        let a = format!("package a\n\nclass {name} {{\n    fun find(): Int = 1\n}}\n");
        let m = format!("package b\n\nimport a.{name}\n\nfun run(r: {name}): Int = r.find()\n");
        let path_a = format!("lib/{name}.kt");
        let files = [(path_a.as_str(), a.as_str()), ("app/M.kt", m.as_str())];
        let c = calls(&files);
        assert!(
            to_file(&c, "app/M.kt::run", &format!("lib/{name}.kt::{name}.find")),
            "{name}: {c:?}"
        );
    }
}

#[test]
fn r3_java_repo_import_same_name_as_library_class_binds() {
    let a = "package a;\n\npublic class Task {\n    public int find() { return 1; }\n}\n";
    let m = "package b;\n\nimport a.Task;\n\npublic class M {\n    int run(Task t) { return t.find(); }\n}\n";
    let files = [("a/Task.java", a), ("b/M.java", m)];
    let c = calls(&files);
    assert!(to_file(&c, "b/M.java::M.run", "a/Task.java::Task.find"), "{c:?}");
}

#[test]
fn r4_scala_repo_import_same_name_binds() {
    let a = "package a\n\nclass Task {\n  def find(): Int = 1\n}\n";
    let m = "package b\n\nimport a.Task\n\nobject M {\n  def run(t: Task): Int = t.find()\n}\n";
    let files = [("a/Task.scala", a), ("b/M.scala", m)];
    let c = calls(&files);
    assert!(to_file(&c, "b/M.scala::M.run", "a/Task.scala::Task.find"), "{c:?}");
}

#[test]
fn r5_repo_class_nested_in_object_imported() {
    let a = "package a\n\nobject Api {\n    class Result {\n        fun find(): Int = 1\n    }\n}\n";
    let m = "package b\n\nimport a.Api.Result\n\nfun run(r: Result): Int = r.find()\n";
    let files = [("a/Api.kt", a), ("b/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "b/M.kt::run", "a/Api.kt::Api.Result.find"), "{c:?}");
}

#[test]
fn r6_import_of_repo_class_through_typealias_or_two_same_names() {
    // two repo classes named Result in different packages, explicit import of one
    let a = "package a\n\nclass Result {\n    fun find(): Int = 1\n}\n";
    let b = "package b\n\nclass Result {\n    fun find(): Int = 2\n}\n";
    let m = "package c\n\nimport b.Result\n\nfun run(r: Result): Int = r.find()\n";
    let files = [("a/Result.kt", a), ("b/Result.kt", b), ("c/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "c/M.kt::run", "b/Result.kt::Result.find"), "{c:?}");
}

#[test]
fn r7_kotlin_import_resolved_but_same_basename_in_two_modules() {
    // module-local duplicates: the importer's module's `Result` must win.
    let a = "package a\n\nclass Result {\n    fun find(): Int = 1\n}\n";
    let m = "package a\n\nimport a.Result\n\nfun run(r: Result): Int = r.find()\n";
    let files = [
        ("m1/src/main/kotlin/a/Result.kt", a),
        ("m2/src/main/kotlin/a/Result.kt", a),
        ("m1/src/main/kotlin/a/M.kt", m),
    ];
    let c = calls(&files);
    assert!(to_file(&c, "m1/src/main/kotlin/a/M.kt::run", "m1/src/main/kotlin/a/Result.kt::Result.find"), "{c:?}");
}

#[test]
fn r8_receiver_with_import_of_same_named_repo_class_in_same_package_file() {
    // Class declared in the same package, imported explicitly (redundant) via its own FQN.
    let a = "package a\n\nclass Event {\n    fun find(): Int = 1\n}\n";
    let m = "package a\n\nimport a.Event\n\nfun run(r: Event): Int = r.find()\n";
    let files = [("src/Event.kt", a), ("src/M.kt", m)];
    let c = calls(&files);
    assert!(to_file(&c, "src/M.kt::run", "src/Event.kt::Event.find"), "{c:?}");
}

// ------------------------------ (4) enclosing member ------------------------------

#[test]
fn e1_inherited_member_is_not_declared_so_factory_binds() {
    // Base declares make(): Other; Sub extends Base, calls make() which is
    // actually the inherited one (returns Other) -> must not bind top-level make
    // typed as Repo. Abstain or bind to Other, never Repo.
    let src = "package p\n\nclass Repo { fun find(): Int = 1 }\nclass Other { fun find(): Int = 2 }\n\nfun make(): Repo = Repo()\n\nopen class Base {\n    fun make(): Other = Other()\n}\n\nclass Sub : Base() {\n    fun use(): Int {\n        val r = make()\n        return r.find()\n    }\n}\n";
    let c = calls(&[("p/Z.kt", src)]);
    assert!(!to_file(&c, "p/Z.kt::Sub.use", "p/Z.kt::Repo.find"), "{c:?}");
}

#[test]
fn e2_inheriting_class_with_unrelated_toplevel_factory_binds() {
    // The enclosing class has no `make`; top-level make() is the only candidate.
    let src = "package p\n\nclass Repo { fun find(): Int = 1 }\n\nfun make(): Repo = Repo()\n\nclass Sub {\n    fun use(): Int {\n        val r = make()\n        return r.find()\n    }\n}\n";
    let c = calls(&[("p/Z.kt", src)]);
    assert!(to_file(&c, "p/Z.kt::Sub.use", "p/Z.kt::Repo.find"), "{c:?}");
}

#[test]
fn e3_same_simple_type_name_in_other_file_does_not_block() {
    // `Sub` in another file declares make(); this file's `Sub` does not.
    let a = "package q\n\nclass Sub {\n    fun make(): Int = 1\n}\n";
    let src = "package p\n\nclass Repo { fun find(): Int = 1 }\n\nfun make(): Repo = Repo()\n\nclass Sub {\n    fun use(): Int {\n        val r = make()\n        return r.find()\n    }\n}\n";
    let c = calls(&[("q/Sub.kt", a), ("p/Z.kt", src)]);
    assert!(to_file(&c, "p/Z.kt::Sub.use", "p/Z.kt::Repo.find"), "{c:?}");
}

#[test]
fn e4_nested_class_member_of_inner_blocks_factory() {
    let src = "package p\n\nclass Repo { fun find(): Int = 1 }\nclass Other { fun find(): Int = 2 }\n\nfun make(): Repo = Repo()\n\nclass Outer {\n    class Inner {\n        fun make(): Other = Other()\n        fun use(): Int {\n            val r = make()\n            return r.find()\n        }\n    }\n}\n";
    let c = calls(&[("p/Z.kt", src)]);
    assert!(!to_file(&c, "p/Z.kt::Outer.Inner.use", "p/Z.kt::Repo.find"), "{c:?}");
}

#[test]
fn e5_companion_member_vs_toplevel() {
    let src = "package p\n\nclass Repo { fun find(): Int = 1 }\nclass Other { fun find(): Int = 2 }\n\nfun make(): Repo = Repo()\n\nclass Host {\n    companion object {\n        fun make(): Other = Other()\n    }\n    fun use(): Int {\n        val r = make()\n        return r.find()\n    }\n}\n";
    let c = calls(&[("p/Z.kt", src)]);
    assert!(!to_file(&c, "p/Z.kt::Host.use", "p/Z.kt::Repo.find"), "{c:?}");
}

#[test]
fn e6_swift_extension_member_blocks_toplevel() {
    let a = "struct Repo { func find() -> Int { 1 } }\nstruct Other { func find() -> Int { 2 } }\n\nfunc make() -> Repo { Repo() }\n\nstruct Host {}\n";
    let b = "extension Host {\n    func make() -> Other { Other() }\n    func use() -> Int {\n        let r = make()\n        return r.find()\n    }\n}\n";
    let c = calls(&[("A.swift", a), ("B.swift", b)]);
    assert!(!to_file(&c, "B.swift::Host.use", "A.swift::Repo.find"), "{c:?}");
}

#[test]
fn e7_java_method_vs_static_import_factory() {
    let a = "package p;\n\npublic class Repo { public int find() { return 1; } }\n";
    let u = "package p;\n\npublic class U {\n    public static Repo make() { return new Repo(); }\n}\n";
    let m = "package q;\n\nimport p.Repo;\nimport static p.U.make;\n\npublic class M {\n    int use() {\n        Repo r = make();\n        return r.find();\n    }\n}\n";
    let c = calls(&[("p/Repo.java", a), ("p/U.java", u), ("q/M.java", m)]);
    assert!(to_file(&c, "q/M.java::M.use", "p/Repo.java::Repo.find"), "{c:?}");
}

// ------------------------------ (5) split_types ------------------------------

#[test]
fn t1_swift_two_distinct_types_same_name_keep_their_recursion() {
    let a = "struct Node {\n    func walk(_ n: Int) -> Int {\n        if n == 0 { return 0 }\n        return walk(n - 1)\n    }\n}\n";
    let b = "struct Node {\n    func walk(_ n: Int) -> Int {\n        if n == 0 { return 0 }\n        return walk(n - 1)\n    }\n}\n";
    let c = calls(&[("ModA/Node.swift", a), ("ModB/Node.swift", b)]);
    assert!(
        c.iter().any(|(f, t)| f == "ModA/Node.swift::Node.walk" && t == "ModA/Node.swift::Node.walk"),
        "A recursion lost: {c:?}"
    );
    assert!(
        c.iter().any(|(f, t)| f == "ModB/Node.swift::Node.walk" && t == "ModB/Node.swift::Node.walk"),
        "B recursion lost: {c:?}"
    );
}

#[test]
fn t2_swift_toplevel_functions_same_name_two_files_keep_their_recursion() {
    let a = "func walk(_ n: Int) -> Int {\n    if n == 0 { return 0 }\n    return walk(n - 1)\n}\n";
    let c = calls(&[("A.swift", a), ("B.swift", a)]);
    assert!(
        c.iter().any(|(f, t)| f == "A.swift::walk" && t == "A.swift::walk"),
        "recursion lost: {c:?}"
    );
}

#[test]
fn t3_csharp_partials_in_different_namespaces_do_not_merge() {
    let a = "namespace A {\n    partial class X {\n        int Walk(int n) {\n            if (n == 0) return 0;\n            return Walk(n - 1);\n        }\n    }\n}\n";
    let b = "namespace B {\n    partial class X {\n        int Walk(int n) {\n            if (n == 0) return 0;\n            return Walk(n - 1);\n        }\n    }\n}\n";
    let c = calls(&[("A/X.cs", a), ("B/X.cs", b)]);
    assert!(
        c.iter().any(|(f, t)| f.starts_with("A/X.cs") && t.starts_with("A/X.cs")),
        "A recursion lost: {c:?}"
    );
}

#[test]
fn t4_swift_true_extension_overload_still_abstains() {
    // baseline: extension across files with overloads; self-edge abstains.
    let a = "struct Foo {\n    func add(_ x: Int) -> Int { x }\n}\n";
    let b = "extension Foo {\n    func add(_ x: String) -> Int {\n        return add(1)\n    }\n}\n";
    let c = calls(&[("A.swift", a), ("B.swift", b)]);
    println!("t4 {c:?}");
}

// ------------------------------ (6) python function-only imports ------------------------------

#[test]
fn p1_function_imports_x_locally_other_function_uses_module_level_x() {
    let helper = "def x():\n    return 1\n";
    let user = "from helper import x as _unused_alias\n\ndef x():\n    return 2\n\ndef a():\n    from helper import x\n    return x()\n\ndef b():\n    return x()\n";
    let c = calls(&[("helper.py", helper), ("user.py", user)]);
    assert!(
        c.iter().any(|(f, t)| f == "user.py::b" && t == "user.py::x"),
        "module-level x not bound: {c:?}"
    );
}

#[test]
fn p2_function_imports_x_locally_other_function_uses_same_file_x() {
    let helper = "def x():\n    return 1\n";
    let user = "def x():\n    return 2\n\ndef a():\n    from helper import x\n    return x()\n\ndef b():\n    return x()\n";
    let c = calls(&[("helper.py", helper), ("user.py", user)]);
    assert!(
        c.iter().any(|(f, t)| f == "user.py::b" && t == "user.py::x"),
        "same-file x not bound in b: {c:?}"
    );
}

#[test]
fn p3_function_imports_x_locally_other_function_uses_global_unique_x() {
    // b has no import and the file has no x: the global unique-name rung is the
    // only candidate (and a bare call with no import is legitimately global? it
    // would be a NameError in Python; abstain is right).
    let helper = "def x():\n    return 1\n";
    let user = "def a():\n    from helper import x\n    return x()\n\ndef b():\n    return x()\n";
    let c = calls(&[("helper.py", helper), ("user.py", user)]);
    assert!(
        c.iter().any(|(f, t)| f == "user.py::a" && t == "helper.py::x"),
        "a lost: {c:?}"
    );
    assert!(!c.iter().any(|(f, t)| f == "user.py::b" && t == "helper.py::x"), "{c:?}");
}

#[test]
fn p4_function_local_import_shadows_builtin_name_other_function_uses_builtin() {
    // `open` imported in one function; other function calls the builtin open
    // while the repo declares `open` too in another file.
    let other = "def open(p):\n    return p\n";
    let user = "def a():\n    from other import open\n    return open('x')\n\ndef b():\n    return open('y')\n";
    let c = calls(&[("other.py", other), ("user.py", user)]);
    assert!(
        !c.iter().any(|(f, t)| f == "user.py::b" && t == "other.py::open"),
        "builtin open mis-bound: {c:?}"
    );
    assert!(c.iter().any(|(f, t)| f == "user.py::a" && t == "other.py::open"), "a lost {c:?}");
}

#[test]
fn p5_function_local_import_in_method_class_attr_x_elsewhere() {
    // A method imports x locally; a sibling method calls self.x? not a bare name.
    let helper = "def x():\n    return 1\n";
    let user = "class C:\n    def a(self):\n        from helper import x\n        return x()\n\n    def b(self):\n        return x()\n";
    let c = calls(&[("helper.py", helper), ("user.py", user)]);
    assert!(c.iter().any(|(f, t)| f == "user.py::C.a" && t == "helper.py::x"), "{c:?}");
    assert!(!c.iter().any(|(f, t)| f == "user.py::C.b" && t == "helper.py::x"), "{c:?}");
}

#[test]
fn p6_python_local_import_by_try_except_toplevel_in_function() {
    // typical lazy import in `main` used by `main` only, and elsewhere a
    // *parameter* named x: the unique global rung must not bind it.
    let helper = "def x():\n    return 1\n";
    let user = "def a():\n    from helper import x\n    return x()\n\ndef b(x):\n    return x()\n";
    let c = calls(&[("helper.py", helper), ("user.py", user)]);
    assert!(!c.iter().any(|(f, t)| f == "user.py::b" && t == "helper.py::x"), "{c:?}");
}

#[test]
fn t0_baselines_single_file_recursion_binds() {
    let a = "struct Node {\n    func walk(_ n: Int) -> Int {\n        if n == 0 { return 0 }\n        return walk(n - 1)\n    }\n}\n";
    let c = calls(&[("ModA/Node.swift", a)]);
    assert!(c.iter().any(|(f, t)| f == t), "swift baseline: {c:?}");
    let x = "namespace A {\n    partial class X {\n        int Walk(int n) {\n            if (n == 0) return 0;\n            return Walk(n - 1);\n        }\n    }\n}\n";
    let c = calls(&[("A/X.cs", x)]);
    assert!(c.iter().any(|(f, t)| f == t), "csharp baseline: {c:?}");
    let k = "package p\n\nfun walk(n: Int): Int {\n    if (n == 0) return 0\n    return walk(n - 1)\n}\n";
    let c = calls(&[("p/W.kt", k)]);
    assert!(c.iter().any(|(f, t)| f == t), "kotlin baseline: {c:?}");
}

// ------------------------------ round-3 additions ------------------------------

#[test]
fn x1_extension_on_imported_external_type_binds_despite_unrelated_repo_class_of_same_name() {
    let other = "package other\n\nclass Task {\n    fun toItem(): Int = 9\n}\n";
    let f = "package f\n\nimport ext.Task\n\nprivate fun Task.toItem(): Int = 1\n\nfun build(t: Task): Int = t.toItem()\n";
    let c = calls(&[("other/Task.kt", other), ("f/F.kt", f)]);
    assert!(to_file(&c, "f/F.kt::build", "f/F.kt::Task.toItem"), "{c:?}");
}

#[test]
fn x2_extension_on_imported_external_type_via_lambda_param() {
    let other = "package other\n\nclass Task {\n    val id: Int = 1\n}\n";
    let f = "package f\n\nimport ext.Task\n\nprivate fun Task.toItem(d: Int): Int? = d\n\nfun build(tasks: List<Task>): List<Int> = tasks.mapNotNull { task -> task.toItem(1) }\n";
    let c = calls(&[("other/Task.kt", other), ("f/F.kt", f)]);
    assert!(to_file(&c, "f/F.kt::build", "f/F.kt::Task.toItem"), "{c:?}");
}

#[test]
fn x3_external_import_member_call_never_binds_to_unrelated_repo_class() {
    let other = "package other\n\nclass Task {\n    fun foo(): Int = 9\n}\n";
    let f = "package f\n\nimport ext.Task\n\nfun build(t: Task): Int = t.foo()\n";
    let c = calls(&[("other/Task.kt", other), ("f/F.kt", f)]);
    assert!(!to_file(&c, "f/F.kt::build", "other/Task.kt"), "{c:?}");
}

#[test]
fn s6_external_explicit_beats_star_of_repo_package() {
    let a = "package a\n\nclass Repo {\n    fun find(): Int = 1\n}\n";
    let m = "package c\n\nimport a.*\nimport ext.Repo\n\nfun run(r: Repo): Int = r.find()\n";
    let c = calls(&[("a/Repo.kt", a), ("c/M.kt", m)]);
    assert!(!to_file(&c, "c/M.kt::run", "a/Repo.kt"), "{c:?}");
}

#[test]
fn s7_aliased_external_import_does_not_shadow_star_class() {
    let a = "package a\n\nclass Repo {\n    fun find(): Int = 1\n}\n";
    let m = "package c\n\nimport a.*\nimport ext.Repo as ExtRepo\n\nfun run(r: Repo, e: ExtRepo): Int = r.find()\n";
    let c = calls(&[("a/Repo.kt", a), ("c/M.kt", m)]);
    assert!(to_file(&c, "c/M.kt::run", "a/Repo.kt::Repo.find"), "{c:?}");
}

#[test]
fn p9_star_import_binding_kept_when_another_function_imports_same_name_locally() {
    let helper = "def x():\n    return 1\n";
    let other = "def x():\n    return 2\n";
    let user = "from helper import *\n\ndef a():\n    from other import x\n    return x()\n\ndef b():\n    return x()\n";
    let c = calls(&[("helper.py", helper), ("other.py", other), ("user.py", user)]);
    assert!(c.iter().any(|(f, t)| f == "user.py::b" && t == "helper.py::x"), "{c:?}");
}

#[test]
fn p10_module_level_conditional_import_other_function_local_import_same_name() {
    let helper = "def x():\n    return 1\n";
    let user = "try:\n    from helper import x\nexcept ImportError:\n    x = None\n\ndef a():\n    from helper import x\n    return x()\n\ndef b():\n    return x()\n";
    let c = calls(&[("helper.py", helper), ("user.py", user)]);
    assert!(c.iter().any(|(f, t)| f == "user.py::b" && t == "helper.py::x"), "{c:?}");
}
