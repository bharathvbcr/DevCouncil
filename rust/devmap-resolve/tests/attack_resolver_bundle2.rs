//! Second wave of adversarial probes (lane D). Each asserts CORRECT behaviour.
use devmap_extract::extract_file;
use devmap_extract::model::EdgeKind;
use devmap_resolve::Resolver;

type E = Vec<(String, String)>;

fn calls(files: &[(&str, &str)]) -> E {
    let extractions: Vec<_> = files.iter().map(|(p, s)| extract_file(p, s)).collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    for e in result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls)
    {
        println!(
            "EDGE {} -> {} conf={:?}",
            e.source_symbol, e.target_symbol, e.confidence
        );
    }
    let mut v: E = result
        .edges
        .iter()
        .filter(|e| e.edge_kind == EdgeKind::Calls)
        .map(|e| (e.source_symbol.clone(), e.target_symbol.clone()))
        .collect();
    v.sort();
    v
}
fn has(e: &E, f: &str, t: &str) -> bool {
    e.iter().any(|(a, b)| a == f && b == t)
}

const T5: &str = "package p\n\ninterface Repo {\n    fun find(id: Int): String\n}\n\nclass Impl1 : Repo {\n    override fun find(id: Int): String = \"1\"\n}\n\nclass Unrelated {\n    fun find(id: Int): String = \"u\"\n}\n\nfun <reified T> mock(): T = throw IllegalStateException()\n\n";

#[test]
fn z5_cast_to_unrelated_type_is_not_repo() {
    for init in [
        "mock<Repo>() as Unrelated",
        "(mock<Repo>() as Unrelated)",
        "mock<Repo>() ?: Unrelated()",
        "mock<Repo>().let { Unrelated() }",
    ] {
        let s = format!(
            "{T5}fun use(): String {{\n    val repo = {init}\n    return repo.find(1)\n}}\n"
        );
        let e = calls(&[("p/Z.kt", &s)]);
        assert!(
            !has(&e, "p/Z.kt::use", "p/Z.kt::Repo.find"),
            "{init}: {e:?}"
        );
    }
}

#[test]
fn z5_name_of_returns_string() {
    let s = "package p\n\nclass Foo {\n    override fun toString(): String = \"foo\"\n    fun trim(): Int = 1\n}\n\ninline fun <reified T> nameOf(): String = T::class.simpleName!!\n\nfun use(): String {\n    val n = nameOf<Foo>()\n    return n.trim().toString()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(
        !has(&e, "p/Z.kt::use", "p/Z.kt::Foo.trim"),
        "String-typed local bound to Foo: {e:?}"
    );
}

#[test]
fn z5_boolean_int_returns() {
    let s = "package p\n\nclass Foo {\n    fun not(): Boolean = true\n    fun toInt(): Int = 1\n}\n\ninline fun <reified T> has(): Boolean = true\n\nfun use(): Boolean {\n    val h = has<Foo>()\n    return h.not()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Foo.not"), "{e:?}");
}

#[test]
fn z3_qualified_param_control() {
    let own = "package q\n\nclass Repo<T> {\n    fun find(): Int = 1\n}\n";
    let user = "package p\n\nfun use(r: ext.Repo<String>): Int {\n    return r.find()\n}\n";
    let e = calls(&[("q/R.kt", own), ("p/Z.kt", user)]);
    println!("control(param ext.Repo<String>): {e:?}");
}

#[test]
fn z3_qualified_nongeneric_return_control() {
    let own = "package q\n\nclass Repo {\n    fun find(): Int = 1\n}\n";
    let user = "package p\n\nfun get(): ext.Repo = TODO()\n\nfun use(): Int {\n    val r = get()\n    return r.find()\n}\n";
    let e = calls(&[("q/R.kt", own), ("p/Z.kt", user)]);
    println!("control(nongeneric qualified return): {e:?}");
}

#[test]
fn z3_generic_return_of_class_declared_in_corpus_but_other_pkg_imported() {
    let own = "package q\n\nclass Page<T> {\n    fun next(): Int = 1\n}\n";
    let other = "package r\n\nclass Page<T> {\n    fun next(): Int = 2\n}\n";
    let user = "package p\n\nimport r.Page\n\nfun load(): Page<String> = TODO()\n\nfun use(): Int {\n    val pg = load()\n    return pg.next()\n}\n";
    let e = calls(&[("q/P.kt", own), ("r/P.kt", other), ("p/Z.kt", user)]);
    assert!(!has(&e, "p/Z.kt::use", "q/P.kt::Page.next"), "{e:?}");
}

#[test]
fn z3_generic_return_unique_class_correct() {
    let own = "package q\n\nclass Page<T> {\n    fun next(): Int = 1\n}\n";
    let user = "package p\n\nimport q.Page\n\nfun load(): Page<String> = TODO()\n\nfun use(): Int {\n    val pg = load()\n    return pg.next()\n}\n";
    let e = calls(&[("q/P.kt", own), ("p/Z.kt", user)]);
    println!("same-file factory unique class: {e:?}");
}

#[test]
fn z3_function_type_and_suspend_returns() {
    for ret in [
        "(Int) -> List<String>",
        "List<String>.() -> Unit",
        "Map<String, Int>?",
        "Array<String>",
        "Pair<List<A>, B>",
    ] {
        let s = format!("package p\n\nclass A\nclass B\n\nclass Pair<X, Y> {{\n    fun first(): Int = 1\n}}\n\nclass Array<X> {{\n    fun first(): Int = 1\n}}\n\nfun names(): {ret} = TODO()\n\nfun use(): Int {{\n    val v = names()\n    return v.first()\n}}\n");
        let e = calls(&[("p/Z.kt", &s)]);
        println!(
            "{ret}: {:?}",
            e.iter()
                .filter(|(a, _)| a == "p/Z.kt::use")
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn z3_type_parameter_named_like_a_class() {
    // `fun <Pair> f(): Pair<A>`: silly; but class type parameter in generic class
    let s = "package p\n\nclass Box<T> {\n    fun total(): Int = 1\n}\n\nclass Holder<Box> {\n    fun make(): Box = TODO()\n}\n\nclass Other {\n    fun total(): Int = 2\n}\n\nfun use(h: Holder<Other>): Int {\n    val b = h.make()\n    return b.total()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(!has(&e, "p/Z.kt::use", "p/Z.kt::Box.total"), "{e:?}");
}

#[test]
fn z3_extension_receiver_generic_function_returning_generic_class() {
    // callee is a method of a generic class returning its own generic type: List<T>
    let s = "package p\n\nclass Repo<T> {\n    fun all(): List<T> = TODO()\n}\n\nclass Item {\n    fun total(): Int = 1\n}\n\nclass List<X> {\n    fun total(): Int = 2\n}\n\nfun use(r: Repo<Item>): Int {\n    val xs = r.all()\n    return xs.total()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{:?}", e);
}

#[test]
fn ov_swift_two_classes_one_file_each_with_init_same_name() {
    let a = "class A { func go(_ x: Int) -> Int { return go(x - 1) } }\nclass B { func go(_ x: Int) -> Int { return go(x - 1) } }\n";
    let e = calls(&[("a/AB.swift", a)]);
    assert!(has(&e, "a/AB.swift::A.go", "a/AB.swift::A.go"), "{e:?}");
    assert!(has(&e, "a/AB.swift::B.go", "a/AB.swift::B.go"), "{e:?}");
}

#[test]
fn ov_swift_extension_in_same_file_with_same_member_name() {
    // `extension Calc { func add }` in the same file as `class Calc { func add }`
    let a = "class Calc {\n    func add(_ x: Int) -> Int { return x }\n}\nextension Calc {\n    func add(_ s: String) -> Int { return add(s.count) }\n    func count(_ n: Int) -> Int { return n > 0 ? count(n - 1) : 0 }\n}\n";
    let e = calls(&[("a/C.swift", a)]);
    println!("{e:?}");
    assert!(
        !has(&e, "a/C.swift::Calc.add", "a/C.swift::Calc.add"),
        "{e:?}"
    );
    assert!(
        has(&e, "a/C.swift::Calc.count", "a/C.swift::Calc.count"),
        "{e:?}"
    );
}

#[test]
fn ov_kotlin_property_getter_and_function_same_name() {
    let s = "package p\n\nclass C {\n    val size: Int get() = 1\n    fun size(): Int = 2\n    fun use(): Int = size()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
}

#[test]
fn ov_kotlin_secondary_constructors() {
    let s = "package p\n\nclass A(val x: Int) {\n    constructor(s: String) : this(s.length)\n    constructor(s: String, t: String) : this(s + t)\n    fun go(n: Int): Int = if (n > 0) go(n - 1) else 0\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    println!("{e:?}");
    assert!(has(&e, "p/Z.kt::A.go", "p/Z.kt::A.go"), "{e:?}");
}

#[test]
fn ov_kotlin_recursion_through_this_and_receiver() {
    let s = "package p\n\nclass N(val next: N?) {\n    fun depth(): Int = if (next == null) 0 else next.depth() + 1\n    fun again(): Int = this.again()\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::N.depth", "p/Z.kt::N.depth"), "{e:?}");
    assert!(has(&e, "p/Z.kt::N.again", "p/Z.kt::N.again"), "{e:?}");
}

#[test]
fn ov_kotlin_nested_class_same_method_name_as_outer() {
    let s = "package p\n\nclass Outer {\n    fun run(n: Int): Int = if (n > 0) run(n - 1) else 0\n\n    class Inner {\n        fun run(n: Int): Int = if (n > 0) run(n - 1) else 0\n    }\n}\n";
    let e = calls(&[("p/Z.kt", s)]);
    assert!(has(&e, "p/Z.kt::Outer.run", "p/Z.kt::Outer.run"), "{e:?}");
    // The extractor names a nested class's member `Inner.run`, not `Outer.Inner.run`.
    assert!(has(&e, "p/Z.kt::Inner.run", "p/Z.kt::Inner.run"), "{e:?}");
}

#[test]
fn ov_kotlin_same_class_name_reopened_in_two_files_receiver_binds() {
    // identical hits only dedupe within one file+symbol; two files must not bind
    let a = "package p\n\nclass Calc {\n    fun add(x: Int): Int = x\n}\n";
    let b = "package p\n\nfun Calc.add(s: String): Int = s.length\n";
    let u = "package p\n\nfun use(c: Calc): Int = c.add(1)\n";
    let e = calls(&[("p/A.kt", a), ("p/B.kt", b), ("p/U.kt", u)]);
    println!("{e:?}");
}
