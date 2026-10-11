//! Every walk the resolver makes over a type graph or a chain of names ends on
//! a cycle, and a repository can write one in every language: a protocol that
//! refines itself, two classes that extend each other, a `Deref` that points
//! back, an embedded struct that embeds its embedder, a type that is its own
//! alias, a barrel that re-exports itself.
//!
//! Each case runs extraction and resolution on a thread with a small stack, so
//! a walk that recurses or loops without a bound kills this test process
//! instead of passing on a roomy main-thread stack, and asserts the resolver
//! returned rather than guessed.

use devmap_extract::extract_file;
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve_on_a_small_stack(files: Vec<(&'static str, String)>) -> ResolutionResult {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let extractions: Vec<_> = files
                .iter()
                .map(|(path, source)| extract_file(path, source))
                .collect();
            let mut resolver = Resolver::new();
            resolver.index_extractions(&extractions);
            resolver.resolve_all(&extractions).unwrap()
        })
        .expect("spawn")
        .join()
        .expect("resolution must terminate without overflowing the stack")
}

fn terminates(path: &'static str, source: &str) -> ResolutionResult {
    resolve_on_a_small_stack(vec![(path, source.to_string())])
}

fn binds_nothing_to(result: &ResolutionResult, callee: &str) {
    let hits: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.target_symbol.ends_with(callee) && edge.resolution.is_some())
        .filter(|edge| {
            matches!(
                edge.resolution.as_deref().map(|r| r.kind()),
                Some(devmap_resolve::model::ResolutionKind::ReceiverType)
            )
        })
        .collect();
    assert!(
        hits.is_empty(),
        "a cycle must not invent a target: {hits:?}"
    );
}

#[test]
fn a_swift_protocol_that_refines_itself_through_an_extension_terminates() {
    let result = terminates(
        "Sources/App/P.swift",
        "protocol P: P { func run() }\n\
         extension P: P { func extra() {} }\n\
         protocol Q: R { func q() }\nprotocol R: Q { func r() }\n\
         struct S: Q { func q() {} }\n\
         func use(p: P, s: S, q: Q) { p.run(); p.missing(); s.r(); q.nothing() }\n",
    );
    binds_nothing_to(&result, "missing");
}

#[test]
fn a_swift_typealias_loop_and_self_conformance_terminate() {
    terminates(
        "Sources/App/T.swift",
        "typealias A = B\ntypealias B = A\n\
         protocol Box { associatedtype Item: Box\n func open() -> Item }\n\
         struct S: Box { typealias Item = S\n func open() -> S { self } }\n\
         func use(a: A, s: S) { a.go(); s.open().open().open().go() }\n",
    );
}

#[test]
fn a_python_class_cycle_terminates() {
    let result = terminates(
        "m.py",
        "class A(B):\n    def f(self): pass\nclass B(A):\n    def g(self): pass\n\
         class C(C):\n    pass\n\
         def use(a: A, c: C):\n    a.nothing()\n    c.nothing()\n    a.g()\n",
    );
    binds_nothing_to(&result, "nothing");
}

#[test]
fn a_kotlin_supertype_cycle_terminates() {
    let result = terminates(
        "m.kt",
        "interface A : B { fun f() }\ninterface B : A { fun g() }\n\
         class C : A { fun h() { nothing(); f() } }\n\
         fun use(c: C) { c.nothing() }\n",
    );
    binds_nothing_to(&result, "nothing");
}

#[test]
fn a_java_inheritance_cycle_terminates() {
    let result = terminates(
        "A.java",
        "class A extends B { void f() {} }\nclass B extends A { void g() {} }\n\
         class U { void use(A a) { a.nothing(); } }\n",
    );
    binds_nothing_to(&result, "nothing");
}

#[test]
fn a_rust_deref_cycle_terminates() {
    let result = terminates(
        "lib.rs",
        "use std::ops::Deref;\nstruct A; struct B;\n\
         impl Deref for A { type Target = B; fn deref(&self) -> &B { loop {} } }\n\
         impl Deref for B { type Target = A; fn deref(&self) -> &A { loop {} } }\n\
         impl Deref for A { type Target = A; fn deref(&self) -> &A { self } }\n\
         fn use_it(a: A) { a.nothing(); }\n",
    );
    binds_nothing_to(&result, "nothing");
}

#[test]
fn a_go_embedding_and_type_cycle_terminates() {
    let result = terminates(
        "a.go",
        "package p\ntype A struct{ B }\ntype B struct{ A }\n\
         type T []T\ntype U *U\ntype V = W\ntype W = V\n\
         type I interface{ J }\ntype J interface{ I }\n\
         func use(a A, t T, u U, v V, i I) {\n a.Nothing()\n t[0][0][0].Nothing()\n \
         (*u).Nothing()\n v.Nothing()\n i.Nothing()\n x := x\n x.Nothing()\n}\n",
    );
    binds_nothing_to(&result, "Nothing");
}

#[test]
fn a_typescript_reexport_cycle_and_self_barrel_terminate() {
    terminates(
        "a.ts",
        "export * from './a';\nexport { x } from './a';\nexport { y } from './b';\n",
    );
    resolve_on_a_small_stack(vec![
        (
            "a.ts",
            "export * from './b';\nexport { y } from './b';\n".into(),
        ),
        (
            "b.ts",
            "export * from './a';\nexport { y } from './a';\n".into(),
        ),
        ("c.ts", "import { y } from './a';\ny();\n".into()),
    ]);
}

/// The extractor's own walks, on a tree as deep as a hostile file can make it.
#[test]
fn a_kotlin_scope_function_nest_a_thousand_deep_terminates() {
    let depth = 1000;
    let mut source = String::from("fun f(x: Door) {\n");
    for _ in 0..depth {
        source.push_str("x.apply {\n");
    }
    source.push_str("open()\n");
    for _ in 0..depth {
        source.push_str("}\n");
    }
    source.push_str("}\n");
    terminates("m.kt", &source);
}

#[test]
fn a_swift_scope_nest_a_thousand_deep_terminates() {
    let depth = 1000;
    let mut source = String::from("func f(xs: [Door]) {\n");
    for i in 0..depth {
        source.push_str(&format!("for x{i} in xs {{\n"));
    }
    source.push_str("x0.open()\n");
    for _ in 0..depth {
        source.push_str("}\n");
    }
    source.push_str("}\n");
    terminates("a.swift", &source);
}
