//! X40 — a reference in **type position** is classified by type-position
//! evidence, not left in the tier reserved for probable defects.
//!
//! `UnresolvedClass::Unresolved` is documented as "the only tier that indicates
//! a defect … the tier to read when hunting bugs". Measured on this repository
//! before this test existed, **8,795 of its 9,790 rows were type references**,
//! and the top ten names in it were `String`, `Vec`, `Option`, `Node`, `Path`,
//! `Value`, `_`, `T`, `Result` and `PathBuf`. Four separate facts were being
//! thrown away at the classification step, each of which is written down in the
//! source the extractor already read:
//!
//! 1. **Rust's standard prelude declares types, not only macros.** `String` and
//!    `Vec` are as much language-declared as `Some` and `println!`, which the
//!    builtin table already holds. 4,581 rows on this repository.
//! 2. **`_` in type position is the inferred-type placeholder.** It is not a
//!    reference to anything, in any indexed language, so there is nothing to
//!    attribute and no row to file. 275 rows, every one of them a turbofish
//!    (`row.get::<_, f64>(1)`).
//! 3. **A qualified type keeps its qualifier**, in a `TypeQualifier` sibling
//!    reference. `t *testing.T` in Go binds `t@mod = testing`, and `testing` is
//!    an import that resolved to no indexed file — which is exactly the
//!    evidence `UnresolvedClass::External` is defined by. The `Type` half was
//!    reduced to the bare `T` and then classified as though nothing were known
//!    about it. 273 rows.
//! 4. **A generic parameter is declared by the enclosing item.** `fn f<T>(x: T)`
//!    binds `T` in its own signature, which is `LocalBinding`'s definition.
//!
//! The counter-direction is asserted too, and it is the half that matters: a
//! prelude name the corpus *itself* declares more than once must stay
//! `Unresolved`, because there the resolver abstained on ambiguity and calling
//! it "the language declares this" would hide a real one.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{ResolutionResult, UnresolvedClass, UnresolvedKind};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

/// Every unresolved row for `callee_name`, as `(kind, class label)`.
fn rows_for(result: &ResolutionResult, name: &str) -> Vec<(UnresolvedKind, String)> {
    result
        .unresolved
        .iter()
        .filter(|row| row.callee_name == name)
        .map(|row| (row.kind, row.class.label().to_string()))
        .collect()
}

const RUST_PRELUDE_USER: &str = "\
use std::collections::BTreeMap;

pub struct Holder {
    pub items: Vec<String>,
}

pub fn build(names: Vec<String>, limit: Option<usize>) -> Result<Holder, String> {
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (position, name) in names.iter().enumerate() {
        index.insert(name.clone(), position);
    }
    let _ = limit;
    Ok(Holder { items: names })
}
";

/// Rust's prelude types are language-declared names, and nothing in the corpus
/// declares them. That is the same evidence `Some` and `println` already have.
#[test]
fn rust_prelude_types_are_language_declared_not_probable_defects() {
    let (_, result) = resolve(&[("lib.rs", RUST_PRELUDE_USER)]);

    for name in ["String", "Vec", "Option", "Result"] {
        let rows = rows_for(&result, name);
        assert!(
            !rows.is_empty(),
            "{name} must still be recorded — the ledger covers every rung that ran"
        );
        assert!(
            rows.iter().all(|(_, class)| class == "builtin"),
            "{name} is in Rust's standard prelude and nothing here declares it, \
             so it cannot be a resolution defect; got {rows:?}"
        );
    }
}

/// The OFF direction. A prelude name that two files of the corpus declare is a
/// name the resolver *abstained* on, and abstention is not "the language
/// declares this".
#[test]
fn a_prelude_name_the_corpus_declares_twice_stays_a_defect() {
    let (_, result) = resolve(&[
        ("one.rs", "pub struct Default;\n"),
        ("two.rs", "pub struct Default;\n"),
        (
            "use.rs",
            "pub fn take(value: Default) -> Default {\n    value\n}\n",
        ),
    ]);

    let rows = rows_for(&result, "Default");
    assert!(
        !rows.is_empty() && rows.iter().all(|(_, class)| class == "unresolved"),
        "two files declare `Default`, so the resolver abstained on ambiguity; \
         labelling it a language builtin would hide that. got {rows:?}"
    );
}

/// `_` in type position is the inferred-type placeholder. There is nothing to
/// attribute, so there must be no row at all — a row would put a non-reference
/// into the tier that means "possible defect".
#[test]
fn a_wildcard_type_placeholder_is_not_a_reference() {
    let (extractions, result) = resolve(&[(
        "lib.rs",
        "pub fn read(row: usize) -> usize {\n    \
         let value = get::<_, u64>(row);\n    value as usize\n}\n",
    )]);

    assert!(
        !extractions[0]
            .references
            .iter()
            .any(|reference| reference.name == "_"),
        "`_` names nothing in type position; extracting it as a reference is \
         what put 275 turbofish placeholders into the defect tier"
    );
    assert!(
        rows_for(&result, "_").is_empty(),
        "no ledger row may exist for a placeholder that references nothing"
    );
}

/// A Go parameter written `t *testing.T` carries its package in a
/// `TypeQualifier` sibling. That qualifier is an import that resolved to no
/// indexed file, which is the definition of `External`.
#[test]
fn a_qualified_type_is_classified_by_the_module_that_qualifies_it() {
    let (_, result) = resolve(&[(
        "pkg/thing_test.go",
        "package pkg\n\nimport \"testing\"\n\n\
         func TestThing(t *testing.T) {\n\tt.Helper()\n}\n",
    )]);

    let rows = rows_for(&result, "T");
    assert!(
        !rows.is_empty(),
        "the bare type name is still recorded — this asserts its class, not its absence"
    );
    assert!(
        rows.iter().all(|(_, class)| class == "external"),
        "`T` here is `testing.T`; the `testing` import proves it comes from \
         outside the corpus, so it is not a resolution defect. got {rows:?}"
    );
}

/// A generic parameter is bound by the enclosing item's own signature, which is
/// what `LocalBinding` means.
#[test]
fn a_generic_parameter_is_bound_by_the_item_that_declares_it() {
    let (_, result) = resolve(&[(
        "lib.rs",
        "pub fn first<T>(items: Vec<T>, spare: T) -> T {\n    spare\n}\n",
    )]);

    let rows = rows_for(&result, "T");
    assert!(
        !rows.is_empty() && rows.iter().all(|(_, class)| class == "local_binding"),
        "`T` is declared by `first`'s own type-parameter list, so the ladder \
         failing to find a cross-file symbol is correct, not a defect. got {rows:?}"
    );
}

/// The whole point of the change, stated as the property rather than per name:
/// after it, no row in the defect tier of this fixture is a language-declared
/// prelude type or a placeholder.
#[test]
fn the_defect_tier_holds_no_prelude_type_and_no_placeholder() {
    let (_, result) = resolve(&[("lib.rs", RUST_PRELUDE_USER)]);

    let offenders: Vec<_> = result
        .unresolved
        .iter()
        .filter(|row| row.class == UnresolvedClass::Unresolved)
        .filter(|row| {
            matches!(
                row.callee_name.as_str(),
                "String" | "Vec" | "Option" | "Result" | "Box" | "_"
            )
        })
        .map(|row| format!("{} in {}", row.callee_name, row.source_file))
        .collect();

    assert!(
        offenders.is_empty(),
        "the tier documented as the one that indicates a defect still holds \
         names the language itself declares: {offenders:?}"
    );
}
