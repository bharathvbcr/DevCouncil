//! `Vec::new()`'s receiver is not a value whose type could not be inferred.
//!
//! `UninferredReceiver` says one thing: "the receiver is a value, and naming
//! its owner needs type inference this resolver does not do." `Vec` is not a
//! value. The language's own standard library declares it, so no indexed file
//! can ever own `new` here and no amount of extractor work will bind it —
//! which is the definition of `Builtin`, and the sentence the Swift rung
//! already makes for `s.count` where `s` is a `String`.
//!
//! Measured on the DevCouncil repository, these were **1,493** of the Rust
//! `uninferred_receiver` rows: `Vec` 776, `String` 513, `Default` 169, `Option`
//! 35, and a tail. Every one sat in the half of the ledger a reader is told to
//! treat as work still to do.
//!
//! The veto is the half of this worth testing. A repository that declares its
//! own `Vec`, or imports one from elsewhere, is the authority on what `Vec`
//! means in its own files — the prelude is not — and the classification must
//! fall back to the attribution gaps there rather than claim the language
//! declared something the corpus demonstrably does.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{ResolutionResult, UnresolvedClass};
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

fn classes_for<'a>(
    resolution: &'a ResolutionResult,
    callee: &str,
    receiver: &str,
) -> Vec<&'a UnresolvedClass> {
    resolution
        .unresolved
        .iter()
        .filter(|row| row.callee_name == callee && row.receiver.as_deref() == Some(receiver))
        .map(|row| &row.class)
        .collect()
}

/// The prelude types this repository's own code reaches for most.
#[test]
fn a_prelude_type_receiver_is_a_builtin() {
    let resolution = resolve(&[(
        "a.rs",
        r#"
fn drive() {
    let v = Vec::new();
    let s = String::from("x");
    let d = Default::default();
    let b = Box::new(1);
    println!("{v:?}{s}{d:?}{b}");
}
"#,
    )]);
    for (callee, receiver) in [
        ("new", "Vec"),
        ("from", "String"),
        ("default", "Default"),
        ("new", "Box"),
    ] {
        let classes = classes_for(&resolution, callee, receiver);
        assert!(
            !classes.is_empty(),
            "{receiver}::{callee} produced no unresolved row at all — the \
             fixture stopped exercising the classification"
        );
        for class in classes {
            assert!(
                matches!(class, UnresolvedClass::Builtin),
                "{receiver}::{callee} classified {class:?}, not Builtin"
            );
        }
    }
}

/// ...and a corpus that declares the name is the authority, not the prelude.
#[test]
fn a_corpus_declared_type_of_the_same_name_vetoes_the_prelude() {
    let resolution = resolve(&[
        ("mine.rs", "pub struct Vec;\n"),
        ("a.rs", "fn drive() { let v = Vec::brand_new(); }\n"),
    ]);
    let classes = classes_for(&resolution, "brand_new", "Vec");
    assert!(
        !classes.is_empty(),
        "Vec::brand_new produced no unresolved row — the corpus `Vec` has no \
         `brand_new`, so the call must still miss"
    );
    for class in classes {
        assert!(
            !matches!(class, UnresolvedClass::Builtin),
            "a corpus that declares its own `Vec` must not have `Vec::brand_new` \
             explained as a language builtin; got {class:?}"
        );
    }
}

/// ...and a local bound to that spelling is a value, not the prelude type.
///
/// This is the `root_is_a_value_here` veto, and it is the one this rung leans
/// on hardest: without it, any repository that happens to name a value `Vec` or
/// `Default` would have its own method calls explained away as the language's.
/// Written with a **path** receiver (`Default::whatever()`) because that is the
/// shape the rung sees — `Default.whatever()` is a field access on a value and
/// never reaches it, so a fixture using the dot form asserts over an empty set
/// and passes whatever the rung does.
#[test]
fn a_local_named_like_a_prelude_type_is_not_the_prelude_type() {
    let resolution = resolve(&[(
        "a.rs",
        "fn drive() { let Default = make(); Default::whatever(); }\n",
    )]);
    let classes = classes_for(&resolution, "whatever", "Default");
    assert!(
        !classes.is_empty(),
        "no unresolved row for Default::whatever — the fixture asserts over \
         nothing and would pass however the rung behaves"
    );
    for class in classes {
        assert!(
            !matches!(class, UnresolvedClass::Builtin),
            "a scope that binds `Default` as a value states what it is; got {class:?}"
        );
    }
}

/// ...and so does a value whose type the file states by assignment.
///
/// The `root_is_a_value_here` test has four clauses and this is the fourth:
/// `receiver_types`, filled where an assignment names a unique indexed type
/// (`Vec = Holder::make()`). Separated from the local-binding case above
/// because they are different evidence, and a conjunction of the four would
/// still pass a test that only ever exercises one.
#[test]
fn an_assignment_that_types_the_spelling_is_also_a_value() {
    let resolution = resolve(&[
        ("holder.rs", "pub struct Holder;\n"),
        (
            "a.rs",
            "fn drive() { let Vec: Holder = Holder::make(); Vec::whatever(); }\n",
        ),
    ]);
    let classes = classes_for(&resolution, "whatever", "Vec");
    assert!(
        !classes.is_empty(),
        "no unresolved row for Vec::whatever — the fixture asserts over nothing"
    );
    for class in classes {
        assert!(
            !matches!(class, UnresolvedClass::Builtin),
            "the file states what its `Vec` is; the prelude does not get to \
             answer for it. got {class:?}"
        );
    }
}

/// ...and an import of that spelling is file-specific evidence that outranks
/// the prelude table, the same way it outranks `is_builtin` for a bare name.
#[test]
fn an_import_of_the_spelling_outranks_the_prelude_table() {
    let resolution = resolve(&[(
        "a.rs",
        "use other_crate::Vec;\nfn drive() { let v = Vec::new(); }\n",
    )]);
    let classes = classes_for(&resolution, "new", "Vec");
    assert!(
        !classes.is_empty(),
        "no unresolved row for Vec::new — the fixture asserts over nothing"
    );
    for class in classes {
        assert!(
            matches!(class, UnresolvedClass::External { .. }),
            "`use other_crate::Vec` says where this file's `Vec` comes from; \
             got {class:?}"
        );
    }
}

/// A prelude type is not a prelude type in another language's file.
///
/// `is_prelude_type` is keyed by family, and a table consulted without that key
/// is how a Go `String` or a Python `Vec` would be explained by Rust's prelude.
#[test]
fn the_prelude_table_is_keyed_by_language() {
    let resolution = resolve(&[(
        "a.py",
        "def drive():\n    v = Vec.new()\n    s = String.from_parts()\n",
    )]);
    for (callee, receiver) in [("new", "Vec"), ("from_parts", "String")] {
        let classes = classes_for(&resolution, callee, receiver);
        assert!(
            !classes.is_empty(),
            "no unresolved row for {receiver}.{callee} — the fixture asserts \
             over nothing"
        );
        for class in classes {
            assert!(
                !matches!(class, UnresolvedClass::Builtin),
                "Python has no Rust prelude; {receiver}.{callee} got {class:?}"
            );
        }
    }
}
