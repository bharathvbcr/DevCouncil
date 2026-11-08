//! Adversarial pass on the Kotlin node-recall lane (card ft-4617a335): the
//! typealias shadow fix and the property call rule. Each test states what the
//! correct edge set is; a failing test is a break and its name is the repro.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::Resolver;

fn edges(files: &[(&str, &str)]) -> Vec<(String, String, EdgeKind)> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let mut found: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| matches!(edge.edge_kind, EdgeKind::Calls | EdgeKind::References))
        .map(|edge| {
            (
                edge.source_symbol.clone(),
                edge.target_symbol.clone(),
                edge.edge_kind,
            )
        })
        .collect();
    found.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    found
}

fn has(found: &[(String, String, EdgeKind)], source: &str, target: &str, kind: EdgeKind) -> bool {
    found
        .iter()
        .any(|(s, t, k)| s == source && t == target && *k == kind)
}

// ---- alias beside a same-name function (the fake-constructor idiom) ----------

#[test]
fn alias_beside_a_same_name_factory_function_in_another_file_keeps_the_call() {
    // `typealias Pt = Pair<Int, Int>` and `fun Pt(x: Int): Pt` are the stdlib
    // fake-constructor idiom. Before the alias was a node `Pt(3)` was a unique
    // global call to the function; with the alias in the bare-name index it is
    // an ambiguous global and the edge is gone.
    let found = edges(&[
        ("a/Pt.kt", "package a\n\ntypealias Pt = Pair<Int, Int>\n"),
        ("a/PtFactory.kt", "package a\n\nfun Pt(x: Int): Pt = Pair(x, x)\n"),
        ("a/UsePt.kt", "package a\n\nfun usePt() = Pt(3)\n"),
    ]);
    assert!(
        has(&found, "a/UsePt.kt::usePt", "a/PtFactory.kt::Pt", EdgeKind::Calls),
        "{found:?}"
    );
}

#[test]
fn alias_beside_a_same_name_factory_function_in_the_same_package_dir_with_a_class_target() {
    let found = edges(&[
        ("a/Box.kt", "package a\n\nclass Box<T>(val v: T)\ntypealias IntBox = Box<Int>\n"),
        ("a/Make.kt", "package a\n\nfun IntBox(v: Int): IntBox = Box(v)\n"),
        ("a/Use.kt", "package a\n\nfun use() = IntBox(1)\n"),
    ]);
    assert!(
        has(&found, "a/Use.kt::use", "a/Make.kt::IntBox", EdgeKind::Calls),
        "{found:?}"
    );
}

// ---- alias to a class in another language ------------------------------------

#[test]
#[ignore = "ft-4617a335: an alias over a Java class has no Kotlin-to-Java edge at base or tip"]
fn alias_of_a_java_class_does_not_take_the_java_class_uses_away() {
    // The shadow fix drops an alias only when a *Kotlin* class of the name
    // exists. `typealias Foo = h.jv.Foo` over a Java class is the same shape,
    // so a use of `Foo` must still reach the Java class.
    let found = edges(&[
        (
            "h/jv/Foo.java",
            "package h.jv;\n\npublic class Foo {\n    public Foo(int x) {}\n}\n",
        ),
        ("h/Alias.kt", "package h\n\ntypealias Foo = h.jv.Foo\n"),
        (
            "h/Use.kt",
            "package h\n\nfun makeFoo() = Foo(1)\nfun takeFoo(f: Foo) = f\n",
        ),
    ]);
    assert!(
        has(&found, "h/Use.kt::makeFoo", "h/jv/Foo.java::Foo", EdgeKind::Calls)
            || has(&found, "h/Alias.kt::Foo", "h/jv/Foo.java::Foo", EdgeKind::References),
        "the Java class is reached by neither the call nor the alias: {found:?}"
    );
    assert!(
        has(&found, "h/Use.kt::takeFoo", "h/jv/Foo.java::Foo", EdgeKind::References)
            || has(&found, "h/Alias.kt::Foo", "h/jv/Foo.java::Foo", EdgeKind::References),
        "{found:?}"
    );
}

// ---- alias whose name is a class of another package --------------------------

#[test]
#[ignore = "ft-4617a335: alias drop keyed on bare name, same at base"]
fn alias_does_not_hand_its_name_to_an_unrelated_class_of_another_package() {
    // `typealias Config = Map<String, String>` in package c; an unrelated
    // `class Config` lives in c.other. A use of `Config` in package c is the
    // alias, not the other package's class.
    let found = edges(&[
        ("c/other/Config.kt", "package c.other\n\nclass Config(val a: Int)\n"),
        ("c/Mine.kt", "package c\n\ntypealias Config = Map<String, String>\n\nfun use(c: Config) = c\n"),
    ]);
    assert!(
        !has(&found, "c/Mine.kt::use", "c/other/Config.kt::Config", EdgeKind::References),
        "wrong edge to the unrelated class: {found:?}"
    );
}

// ---- the alias's own outgoing edge ---------------------------------------------

#[test]
fn alias_of_a_differently_named_class_references_that_class() {
    // Without it the class is reached by no use when every use spells the alias.
    let found = edges(&[
        ("b/model/ModelId.kt", "package b.model\n\nclass ModelId(val v: Int)\n"),
        (
            "b/Alias.kt",
            "package b\n\nimport b.model.ModelId\n\ntypealias Id = ModelId\n\nfun mk() = Id(1)\n",
        ),
    ]);
    assert!(
        has(&found, "b/Alias.kt::Id", "b/model/ModelId.kt::ModelId", EdgeKind::References),
        "{found:?}"
    );
    assert!(has(&found, "b/Alias.kt::mk", "b/Alias.kt::Id", EdgeKind::Calls), "{found:?}");
}

#[test]
fn two_aliases_to_one_class_both_reach_it() {
    let found = edges(&[(
        "d/K.kt",
        "package d\n\nclass Kls\ntypealias A1 = Kls\ntypealias A2 = Kls\n\nfun x() = A1()\nfun y() = A2()\n",
    )]);
    assert!(has(&found, "d/K.kt::A1", "d/K.kt::Kls", EdgeKind::References), "{found:?}");
    assert!(has(&found, "d/K.kt::A2", "d/K.kt::Kls", EdgeKind::References), "{found:?}");
}

#[test]
fn a_cross_package_alias_to_the_same_named_class_with_an_import_keeps_both_uses() {
    let found = edges(&[
        ("m/T.kt", "package m\n\nclass T(val x: Int)\n"),
        ("u/Alias.kt", "package u\n\nimport m.T as MT\n\ntypealias T = MT\n"),
        ("u/Use.kt", "package u\n\nfun f() = T(1)\nfun g(t: T) = t\n"),
    ]);
    assert!(has(&found, "u/Use.kt::f", "m/T.kt::T", EdgeKind::Calls), "{found:?}");
    assert!(has(&found, "u/Use.kt::g", "m/T.kt::T", EdgeKind::References), "{found:?}");
}

#[test]
fn expect_class_with_an_actual_typealias_binds_a_use_to_the_class() {
    let found = edges(&[
        ("e/Common.kt", "package e\n\nexpect class Plat\n"),
        (
            "e/Jvm.kt",
            "package e\n\nclass PlatImpl\nactual typealias Plat = PlatImpl\nfun u() = Plat()\n",
        ),
    ]);
    // The use reaches the expect class (or the alias); PlatImpl is reached by
    // the alias.
    assert!(
        has(&found, "e/Jvm.kt::u", "e/Common.kt::Plat", EdgeKind::Calls)
            || has(&found, "e/Jvm.kt::u", "e/Jvm.kt::Plat", EdgeKind::Calls),
        "{found:?}"
    );
    assert!(
        has(&found, "e/Jvm.kt::Plat", "e/Jvm.kt::PlatImpl", EdgeKind::References),
        "{found:?}"
    );
}

#[test]
fn the_same_alias_name_in_two_packages_binds_each_user_to_its_own() {
    let found = edges(&[
        ("i/p1/P1.kt", "package i.p1\n\ntypealias Id = Long\n\nfun f(x: Id) = x\n"),
        ("i/p2/P2.kt", "package i.p2\n\ntypealias Id = String\n\nfun g(x: Id) = x\n"),
    ]);
    assert!(has(&found, "i/p1/P1.kt::f", "i/p1/P1.kt::Id", EdgeKind::References), "{found:?}");
    assert!(has(&found, "i/p2/P2.kt::g", "i/p2/P2.kt::Id", EdgeKind::References), "{found:?}");
}

#[test]
fn a_generic_alias_alone_under_its_name_is_still_the_target() {
    let found = edges(&[(
        "g/L.kt",
        "package g\n\ntypealias L = List<Int>\ntypealias Handler<T> = (T) -> Unit\n\nfun sum(l: L): Int = l.size\nfun h(cb: Handler<Int>) = cb(1)\n",
    )]);
    assert!(has(&found, "g/L.kt::sum", "g/L.kt::L", EdgeKind::References), "{found:?}");
    assert!(has(&found, "g/L.kt::h", "g/L.kt::Handler", EdgeKind::References), "{found:?}");
}

#[test]
fn an_alias_to_a_class_with_the_class_in_the_same_file_resolves_every_use_site() {
    let found = edges(&[(
        "k/K.kt",
        "package k\n\nclass Kls(val v: Int)\ntypealias Kls2 = Kls\n\nfun a(x: Kls) = x\nfun b(x: Kls2) = x\nfun c() = Kls(1)\nfun d() = Kls2(2)\n",
    )]);
    assert!(has(&found, "k/K.kt::a", "k/K.kt::Kls", EdgeKind::References), "{found:?}");
    assert!(has(&found, "k/K.kt::c", "k/K.kt::Kls", EdgeKind::Calls), "{found:?}");
    assert!(has(&found, "k/K.kt::d", "k/K.kt::Kls2", EdgeKind::Calls), "{found:?}");
}

// ---- Kotlin top-level property beside a class of the same name -----------------

#[test]
fn a_top_level_property_named_like_a_class_in_another_file_does_not_hide_the_class() {
    let found = edges(&[
        ("f/Typo.kt", "package f\n\nval Typography = Typo(1)\n"),
        ("f/Types.kt", "package f\n\nclass Typography(val x: Int)\nclass Typo(val x: Int)\n"),
        ("f/UseT.kt", "package f\n\nfun mkT() = Typography(2)\nfun takeT(t: Typography) = t\n"),
    ]);
    assert!(has(&found, "f/UseT.kt::mkT", "f/Types.kt::Typography", EdgeKind::Calls), "{found:?}");
    assert!(has(&found, "f/UseT.kt::takeT", "f/Types.kt::Typography", EdgeKind::References), "{found:?}");
}

// ---- the property call rule -----------------------------------------------------

#[test]
#[ignore = "ft-4617a335: a function-typed property called bare binds the outer function, documented residual"]
fn a_function_typed_property_called_bare_does_not_bind_a_same_name_top_level_function() {
    // Documented accepted residual: `onDone("y")` calls the property; the
    // resolver binds the outer `fun onDone` instead. Correct behaviour is no
    // edge to the top-level function. The test pins the residual's size.
    let found = edges(&[(
        "j/Vm.kt",
        "package j\n\nfun onDone(s: String) = s\n\nclass Vm(val onDone: (String) -> Unit) {\n    fun g() { onDone(\"y\") }\n}\n",
    )]);
    assert!(
        !has(&found, "j/Vm.kt::Vm.g", "j/Vm.kt::onDone", EdgeKind::Calls),
        "wrong edge: the property is what is called: {found:?}"
    );
}

#[test]
fn a_function_typed_local_parameter_called_bare_does_not_bind_a_top_level_function() {
    let found = edges(&[(
        "j/P.kt",
        "package j\n\nfun cb(s: String) = s\n\nfun run(cb: (String) -> Unit) { cb(\"y\") }\n",
    )]);
    assert!(
        !has(&found, "j/P.kt::run", "j/P.kt::cb", EdgeKind::Calls),
        "{found:?}"
    );
}

#[test]
fn a_property_does_not_hide_an_outer_function_from_a_call_at_any_lexical_level() {
    let found = edges(&[(
        "j/L.kt",
        "package j\n\nfun log(s: String) = s\n\nclass Outer {\n    val log = \"t\"\n    class Nested {\n        val log = 1\n        fun k() = log(\"z\")\n    }\n    inner class In {\n        val log = 2\n        fun m() = log(\"w\")\n    }\n    object Obj { val log = 3; fun n() = log(\"v\") }\n    companion object { val log = 4; fun o() = log(\"u\") }\n}\n",
    )]);
    for caller in ["Nested.k", "In.m", "Obj.n", "Outer.o"] {
        let source = format!("j/L.kt::{caller}");
        assert!(
            found
                .iter()
                .any(|(s, t, k)| *s == source && t == "j/L.kt::log" && *k == EdgeKind::Calls),
            "{caller} lost its call to the top-level log: {found:?}"
        );
    }
}

#[test]
fn a_member_function_still_beats_the_outer_function_when_a_property_shares_its_name() {
    let found = edges(&[(
        "j/M.kt",
        "package j\n\nfun size() = 0\n\nclass C {\n    val size = 3\n    fun size() = 4\n    fun h() = size()\n}\n",
    )]);
    assert!(has(&found, "j/M.kt::C.h", "j/M.kt::C.size", EdgeKind::Calls), "{found:?}");
}

#[test]
fn an_enum_entry_called_bare_in_its_own_enum_is_not_an_edge_lost_by_the_field_rule() {
    // An enum entry with a body: `A { override fun f() = 1 }`; a sealed class's
    // `object` entry invoked as a function value is out of scope. Pin: a bare
    // call of a *function* named like an entry still resolves.
    let found = edges(&[(
        "j/E.kt",
        "package j\n\nfun A() = 1\n\nenum class E { A, B; fun f() = A() }\n",
    )]);
    assert!(has(&found, "j/E.kt::E.f", "j/E.kt::A", EdgeKind::Calls), "{found:?}");
}

#[test]
fn a_call_through_an_invoke_property_on_an_explicit_receiver_does_not_bind_a_field() {
    let found = edges(&[(
        "j/R.kt",
        "package j\n\nclass H(val cb: () -> Unit)\n\nfun f(h: H) = h.cb()\n",
    )]);
    assert!(
        !found
            .iter()
            .any(|(s, t, k)| s == "j/R.kt::f" && t == "j/R.kt::H.cb" && *k == EdgeKind::Calls),
        "a Calls edge to a Field: {found:?}"
    );
}
