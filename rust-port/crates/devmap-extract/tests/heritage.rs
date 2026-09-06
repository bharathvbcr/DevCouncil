//! W1.2 — supertypes are extracted, per language, from measured node kinds.
//!
//! `ReferenceKind::Heritage`, `EdgeKind::Extends` and `EdgeKind::Implements`
//! were all declared and none produced. The consumer side was already written —
//! `resolver.rs` reads `Heritage` to set `prefer_types` — so the variant was
//! dead on the producer side only, and a superclass name arrived as
//! `ReferenceKind::Name`, indistinguishable from any other identifier. A method
//! reached only through its base type therefore had no inbound edge, which made
//! every override a candidate `extracted` false positive.
//!
//! **Every node kind here was read off a real parse**, not recalled. The
//! per-language matrix below is the record of that: it is the test that fails
//! when a grammar renames `superclass` to `extends_clause`, which is exactly
//! the silent regression an identity fixture cannot catch.

use devmap_extract::extract_file;
use devmap_extract::model::ReferenceKind;

/// `(label, path, source, expected extends, expected implements)`.
///
/// Sources are minimal on purpose: each exercises one grammar's heritage
/// clause and nothing else, so a failure names the grammar rather than
/// something incidental about the fixture.
/// One row of [`MATRIX`]: label, path, source, expected `extends`, expected
/// `implements`.
///
/// Named rather than written inline for two reasons that both apply: clippy
/// refuses a tuple this wide, and at the use site five anonymous fields — two
/// of them `&[&str]` — say nothing about which is which.
type HeritageCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

const MATRIX: &[HeritageCase] = &[
    (
        "typescript separates the two clauses",
        "W.ts",
        "class W extends Base implements IFace { m(): void {} }\n",
        &["Base"],
        &["IFace"],
    ),
    (
        "tsx behaves as typescript",
        "W.tsx",
        "class W extends Base implements IFace { m(): void {} }\n",
        &["Base"],
        &["IFace"],
    ),
    (
        "javascript has no implements clause",
        "W.js",
        "class W extends Base { m() {} }\n",
        &["Base"],
        &[],
    ),
    (
        "python states no relation, so both bases are Extends",
        "w.py",
        "class W(Base, Mixin):\n    def m(self):\n        pass\n",
        &["Base", "Mixin"],
        &[],
    ),
    (
        "java names superclass and super_interfaces apart",
        "W.java",
        "class W extends Base implements IFace { void m() {} }\n",
        &["Base"],
        &["IFace"],
    ),
    (
        "csharp uses one base_list for both",
        "W.cs",
        "class W : Base, IFace { void M() {} }\n",
        &["Base", "IFace"],
        &[],
    ),
    (
        "cpp drops the access specifier",
        "w.cpp",
        "class W : public Base { void m(); };\n",
        &["Base"],
        &[],
    ),
    (
        "ruby has a superclass node",
        "w.rb",
        "class W < Base\n  def m; end\nend\n",
        &["Base"],
        &[],
    ),
    (
        "php names base_clause and class_interface_clause apart",
        "w.php",
        "<?php\nclass W extends Base implements IFace { function m() {} }\n",
        &["Base"],
        &["IFace"],
    ),
    (
        "swift uses one inheritance_specifier per supertype",
        "w.swift",
        "class W: Base, IFace { func m() {} }\n",
        &["Base", "IFace"],
        &[],
    ),
    (
        "kotlin strips the constructor call",
        "w.kt",
        "class W : Base(), IFace { override fun m() {} }\n",
        &["Base", "IFace"],
        &[],
    ),
    (
        "scala folds `with` into extends_clause",
        "w.scala",
        "class W extends Base with IFace { def m(): Unit = () }\n",
        &["Base", "IFace"],
        &[],
    ),
    (
        "dart names superclass and interfaces apart",
        "w.dart",
        "class W extends Base implements IFace { void m() {} }\n",
        &["Base"],
        &["IFace"],
    ),
    (
        "rust attributes the impl to the type, not the block",
        "w.rs",
        "impl Doer for Thing { fn go(&self) {} }\n",
        &[],
        &["Doer"],
    ),
    (
        "objc takes the second identifier as the superclass",
        "w.m",
        "@interface W : Base <IFace>\n@end\n",
        &["Base"],
        &["IFace"],
    ),
    (
        "solidity uses one inheritance_specifier per supertype",
        "w.sol",
        "contract W is Base, IFace { function m() public {} }\n",
        &["Base", "IFace"],
        &[],
    ),
];

fn named(path: &str, source: &str, kind: ReferenceKind) -> Vec<String> {
    let mut names: Vec<String> = extract_file(path, source)
        .references
        .into_iter()
        .filter(|r| r.kind == kind)
        .map(|r| r.name)
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn every_language_extracts_its_heritage_clause() {
    for (label, path, source, extends, implements) in MATRIX {
        let mut want_extends: Vec<String> = extends.iter().map(|s| s.to_string()).collect();
        want_extends.sort();
        let mut want_implements: Vec<String> = implements.iter().map(|s| s.to_string()).collect();
        want_implements.sort();

        assert_eq!(
            named(path, source, ReferenceKind::Heritage),
            want_extends,
            "{label}: extends"
        );
        assert_eq!(
            named(path, source, ReferenceKind::HeritageInterface),
            want_implements,
            "{label}: implements"
        );
    }
}

/// The declaring type is carried, so the edge has both endpoints.
///
/// Without it the resolver falls back to the file path as the source symbol,
/// and the graph says the *file* extends `Base` — an edge whose source is not
/// a type at all.
#[test]
fn a_heritage_reference_names_the_type_that_declares_it() {
    let extraction = extract_file("W.java", "class W extends Base { void m() {} }\n");
    let heritage = extraction
        .references
        .iter()
        .find(|r| r.kind == ReferenceKind::Heritage)
        .expect("a heritage reference");
    assert_eq!(
        heritage.enclosing_symbol.as_deref(),
        Some("W.java::W"),
        "the declaring type is the edge's source endpoint"
    );
}

/// An inherent `impl` names no supertype.
///
/// `impl Thing { … }` and `impl Doer for Thing { … }` are the same node kind;
/// only the second has a `trait` field. Without that discriminator every
/// inherent impl in a Rust codebase would emit a self-edge.
#[test]
fn an_inherent_rust_impl_emits_no_heritage() {
    let extraction = extract_file("w.rs", "impl Thing { fn go(&self) {} }\n");
    assert!(
        !extraction.references.iter().any(|r| matches!(
            r.kind,
            ReferenceKind::Heritage | ReferenceKind::HeritageInterface
        )),
        "an inherent impl declares no supertype: {:?}",
        extraction.references
    );
}

/// A class with no heritage clause emits none.
///
/// The OFF direction for the whole matrix: an extractor that emitted a
/// reference for every class would satisfy every assertion above.
#[test]
fn a_class_without_a_supertype_emits_no_heritage() {
    for (path, source) in [
        ("W.ts", "class W { m(): void {} }\n"),
        ("w.py", "class W:\n    def m(self):\n        pass\n"),
        ("W.java", "class W { void m() {} }\n"),
        ("w.rb", "class W\n  def m; end\nend\n"),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.references.iter().any(|r| matches!(
                r.kind,
                ReferenceKind::Heritage | ReferenceKind::HeritageInterface
            )),
            "{path} names no supertype: {:?}",
            extraction.references
        );
    }
}

/// A type is never its own supertype.
///
/// Python's `argument_list` also carries keyword arguments such as
/// `metaclass=…`, and a self-reference would make a class extend itself — an
/// edge that turns every such class into a one-node cycle for W1.1's SCC pass.
#[test]
fn a_type_is_never_its_own_supertype() {
    let extraction = extract_file("w.py", "class W(W):\n    def m(self):\n        pass\n");
    assert!(
        !extraction
            .references
            .iter()
            .any(|r| r.kind == ReferenceKind::Heritage && r.name == "W"),
        "a self-referential base is dropped: {:?}",
        extraction.references
    );
}

/// Decoration a grammar leaves attached is stripped, and only that.
#[test]
fn generic_arguments_and_qualifiers_are_stripped_from_a_supertype() {
    assert_eq!(
        named(
            "W.java",
            "class W extends com.example.Base<String> { void m() {} }\n",
            ReferenceKind::Heritage
        ),
        vec!["Base".to_string()],
        "the resolver indexes symbols by their own name, so a qualified \
         supertype must be looked up by its last segment"
    );
}
