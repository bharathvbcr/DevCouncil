//! X41 — a Rust `use` declaration binds the names it imports.
//!
//! The `use_declaration` arm did string surgery on the statement's source text
//! and stored the remainder whole: `use tree_sitter::{Language, Node, Parser};`
//! became one import whose `module_specifier` was the literal
//! `"tree_sitter::{Language, Node, Parser}"`, with `imported_names` empty. Every
//! consumer of an import reads one of those two fields, so the consequence was
//! total rather than partial — measured on this repository, **not one `.rs` file
//! of 700 produced a single `Imports` edge**, and `UnresolvedClass::External`
//! never once fired for Rust across 33,395 unresolved sites, while Python
//! produced 8,734 and Go 2,619 from the same ladder.
//!
//! What that cost, in the tier reserved for probable defects: `Node` (633, from
//! `use tree_sitter::Node`), `Path` (502), `Value` (374, `serde_json`),
//! `PathBuf` (173), `BTreeMap` (170) — each one an import the file states
//! outright.
//!
//! The second half is `super`. Rust's `super` is relative to the *module*, and
//! `mod tests { use super::*; }` is one module deeper than the file, so its
//! `super` names the file itself — not the parent directory the file-level
//! rung walks to. Both spellings appear in this repository and only one of them
//! was ever right.

use devmap_extract::extract_file;
use devmap_extract::model::ExtractedImport;

fn imports(source: &str) -> Vec<ExtractedImport> {
    extract_file("crates/thing/src/lib.rs", source).imports
}

/// `(module_specifier, imported_names, local_names, alias)` — the four fields
/// every consumer of an import reads.
type ImportShape = (String, Vec<String>, Vec<String>, Option<String>);

/// The import set, sorted, so an assertion reads as the statement it describes.
fn shape(imports: &[ExtractedImport]) -> Vec<ImportShape> {
    let mut out: Vec<_> = imports
        .iter()
        .map(|imp| {
            (
                imp.module_specifier.clone(),
                imp.imported_names.clone(),
                imp.local_names.clone(),
                imp.alias.clone(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_braced_use_names_its_module_and_each_name_it_imports() {
    let found = shape(&imports("use tree_sitter::{Language, Node, Parser};\n"));
    assert_eq!(
        found,
        vec![(
            "tree_sitter".to_string(),
            vec![
                "Language".to_string(),
                "Node".to_string(),
                "Parser".to_string()
            ],
            vec![
                "Language".to_string(),
                "Node".to_string(),
                "Parser".to_string()
            ],
            None,
        )],
        "the brace group is the import list, not part of the module path"
    );
}

#[test]
fn a_plain_path_import_splits_module_from_name() {
    let found = shape(&imports("use crate::model::ExtractedImport;\n"));
    assert_eq!(
        found,
        vec![(
            "crate::model".to_string(),
            vec!["ExtractedImport".to_string()],
            vec!["ExtractedImport".to_string()],
            None,
        )],
        "the last segment of a `use` path is the name; the rest is the module"
    );
}

#[test]
fn an_alias_is_the_local_name_and_the_original_is_the_imported_one() {
    let found = shape(&imports("use std::collections::HashMap as Map;\n"));
    assert_eq!(
        found,
        vec![(
            "std::collections".to_string(),
            vec!["HashMap".to_string()],
            vec!["Map".to_string()],
            None,
        )],
        "`as Map` renames the binding; the module still exports `HashMap`"
    );
}

#[test]
fn a_nested_group_yields_one_import_per_module_it_names() {
    let found = shape(&imports(
        "use std::{collections::BTreeMap, sync::{Arc, Mutex}, fmt};\n",
    ));
    assert_eq!(
        found,
        vec![
            (
                "std".to_string(),
                vec!["fmt".to_string()],
                vec!["fmt".to_string()],
                None
            ),
            (
                "std::collections".to_string(),
                vec!["BTreeMap".to_string()],
                vec!["BTreeMap".to_string()],
                None
            ),
            (
                "std::sync".to_string(),
                vec!["Arc".to_string(), "Mutex".to_string()],
                vec!["Arc".to_string(), "Mutex".to_string()],
                None
            ),
        ],
        "each leaf of the group belongs to the module its own prefix names"
    );
}

/// A glob brings every name of the module into scope, which is what the
/// resolver's `.` alias already means — the Go dot-import. Reusing it rather
/// than inventing a second spelling is deliberate: one owner for "bind
/// everything this module exports".
#[test]
fn a_glob_import_is_a_bind_everything_import() {
    let found = shape(&imports("use crate::prelude::*;\n"));
    assert_eq!(
        found,
        vec![(
            "crate::prelude".to_string(),
            vec![],
            vec![],
            Some(".".to_string())
        )],
        "a glob names no individual symbol; it binds the module's whole surface"
    );
}

/// `use super::*` written at file level names the parent module — a real
/// directory hop.
#[test]
fn a_file_level_super_names_the_parent_module() {
    let found = shape(&imports("use super::helper;\n"));
    assert_eq!(
        found,
        vec![(
            "super".to_string(),
            vec!["helper".to_string()],
            vec!["helper".to_string()],
            None
        )],
        "at file level `super` is the module above this file"
    );
}

/// The case this repository is full of. `mod tests { use super::*; }` sits one
/// module below the file, so its `super` **is** the file — resolving it to the
/// parent directory names a module the statement does not mention.
#[test]
fn a_super_inside_an_inline_module_names_the_enclosing_file() {
    let found = shape(&imports(
        "pub fn helper() -> u32 {\n    1\n}\n\n\
         #[cfg(test)]\nmod tests {\n    use super::*;\n\n    \
         #[test]\n    fn it_works() {\n        assert_eq!(helper(), 1);\n    }\n}\n",
    ));
    assert_eq!(
        found,
        vec![("self".to_string(), vec![], vec![], Some(".".to_string()))],
        "one inline `mod` deep, `super` is this file; `super` would be the \
         parent directory only at file level"
    );
}

/// Two levels of inline module, and one `super`: still inside the file.
#[test]
fn super_is_counted_against_the_inline_module_depth() {
    let found = shape(&imports(
        "mod outer {\n    mod inner {\n        use super::super::helper;\n    }\n}\n",
    ));
    assert_eq!(
        found,
        vec![(
            "self".to_string(),
            vec!["helper".to_string()],
            vec!["helper".to_string()],
            None
        )],
        "two `super`s from two levels of inline module land back at the file"
    );
}

/// One `super` more than there are inline modules is a real hop out of the file.
#[test]
fn a_super_deeper_than_the_inline_nesting_still_leaves_the_file() {
    let found = shape(&imports(
        "mod inner {\n    use super::super::sibling::thing;\n}\n",
    ));
    assert_eq!(
        found,
        vec![(
            "super::sibling".to_string(),
            vec!["thing".to_string()],
            vec!["thing".to_string()],
            None
        )],
        "one `super` is spent on the inline module; the rest still walks up"
    );
}

/// `use x as _;` imports a trait for its methods and binds no name. The
/// resolver already skips that alias; the extractor must still produce the
/// import so the `Imports` edge exists.
#[test]
fn an_underscore_alias_is_preserved_rather_than_dropped() {
    let found = shape(&imports("use std::io::Write as _;\n"));
    assert_eq!(
        found,
        vec![(
            "std::io".to_string(),
            vec!["Write".to_string()],
            vec!["_".to_string()],
            None
        )],
        "the trait is imported for its methods; the binding is anonymous"
    );
}

/// A `use` with a visibility modifier is still a `use`.
#[test]
fn a_reexport_is_read_the_same_way_as_a_private_use() {
    let found = shape(&imports("pub use crate::model::Thing;\n"));
    assert_eq!(
        found,
        vec![(
            "crate::model".to_string(),
            vec!["Thing".to_string()],
            vec!["Thing".to_string()],
            None
        )],
        "`pub` changes who can see the binding, not what it binds"
    );
}

/// The whole statement stays available as the edge's detail, exactly as it was.
#[test]
fn every_import_still_carries_the_statement_that_produced_it() {
    for imp in imports("use std::{fmt, io::Write};\n") {
        assert_eq!(
            imp.raw_import, "use std::{fmt, io::Write};",
            "the raw statement is what an `Imports` edge reports as its detail"
        );
    }
}
