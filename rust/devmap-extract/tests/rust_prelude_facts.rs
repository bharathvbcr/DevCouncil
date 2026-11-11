//! The Rust facts that say what a crate's macro prelude holds and where an import
//! or a `#[macro_use] mod` sits (round 8 of the Rust caller audit): `#![no_std]`,
//! `#![no_implicit_prelude]`, the inline-module chain of a `#[macro_use] mod`, an
//! inner attribute that is not an attribute macro, the cfg spans of a `use`, and
//! the version marker a reader checks before trusting what is absent.

use devmap_extract::extract_file;
use devmap_extract::model::{RustMacroFacts, RUST_MACRO_FACTS_VERSION};

fn facts(source: &str) -> RustMacroFacts {
    extract_file("src/lib.rs", source)
        .rust_scope_facts
        .expect("scope facts")
        .macros
        .expect("macro facts")
}

#[test]
fn no_std_is_recorded_when_written_or_possibly_written() {
    assert!(facts("#![no_std]\nfn f() {}\n").no_std);
    assert!(facts("#![cfg_attr(not(feature = \"std\"), no_std)]\nfn f() {}\n").no_std);
    assert!(!facts("fn f() {}\n").no_std);
    assert!(!facts("#![allow(dead_code)]\nfn f() {}\n").no_std);
}

#[test]
fn no_implicit_prelude_is_recorded_on_the_crate_or_a_module() {
    assert!(facts("#![no_implicit_prelude]\nfn f() {}\n").no_implicit_prelude);
    assert!(facts("#[no_implicit_prelude]\nmod m {}\n").no_implicit_prelude);
    assert!(!facts("mod m {}\n").no_implicit_prelude);
}

#[test]
fn a_macro_use_mod_is_named_by_its_inline_module_chain() {
    assert_eq!(facts("#[macro_use]\nmod m;\n").macro_use_mods, vec!["m"]);
    assert_eq!(
        facts("mod outer {\n    #[macro_use]\n    mod m;\n}\n").macro_use_mods,
        vec!["outer/m"]
    );
    assert_eq!(
        facts("mod a {\n    mod b {\n        #[macro_use]\n        mod m;\n    }\n}\n")
            .macro_use_mods,
        vec!["a/b/m"]
    );
    // A module in a block follows no file layout: not understood.
    let in_block = facts("fn f() {\n    #[macro_use]\n    mod m;\n}\n");
    assert!(in_block.macro_use_mods.is_empty());
    assert!(in_block.macro_use);
}

#[test]
fn an_inner_attribute_is_not_an_attribute_macro_on_the_next_item() {
    // `#![..]` belongs to the crate or module that holds it; stable Rust has no
    // inner attribute macro. wry opens its lib.rs with this one.
    let inner = facts("#![cfg_attr(docsrs, feature(doc_cfg))]\n\n#[cfg(unix)]\nmod m;\n");
    assert!(inner.attrs.is_empty(), "{:?}", inner.attrs);
    // Control: an outer attribute macro on the same item is one.
    let outer = facts("#[pm::with_use]\nmod m;\n");
    assert_eq!(outer.attrs.len(), 1, "{:?}", outer.attrs);
}

#[test]
fn a_use_under_cfg_has_a_cfg_span() {
    let source = "#[cfg(feature = \"x\")]\nuse a::b;\nuse c::d;\n";
    let spans = extract_file("src/lib.rs", source)
        .rust_scope_facts
        .expect("scope facts")
        .cfg_spans;
    let gated = source.find("use a::b").unwrap();
    let plain = source.find("use c::d").unwrap();
    assert!(spans.iter().any(|(from, to)| *from <= gated && gated < *to));
    assert!(!spans.iter().any(|(from, to)| *from <= plain && plain < *to));
}

#[test]
fn a_foreign_derive_in_a_braced_use_is_told_from_a_std_one() {
    let attrs = |source: &str| facts(source).attrs.len();
    // A derive counts where an attribute macro would: on an item in a block.
    let item = "\nfn f() {\n    #[derive(Clone)]\n    struct S;\n}\n";
    assert_eq!(
        attrs(&format!("use {{std::fmt::Debug, dep::Clone}};{item}")),
        1
    );
    assert_eq!(
        attrs(&format!("use std::{{fmt::Debug, clone::Clone}};{item}")),
        0
    );
    assert_eq!(attrs(&format!("use dep::{{Debug, Clone as C}};{item}")), 1);
}

#[test]
fn the_extractor_writes_the_current_facts_version_and_an_old_payload_reads_as_zero() {
    let written = facts("fn f() {}\n");
    assert_eq!(written.facts_version, RUST_MACRO_FACTS_VERSION);
    // A payload written before the marker existed carries no such key.
    let mut json = serde_json::to_value(&written).unwrap();
    json.as_object_mut().unwrap().remove("facts_version");
    let old: RustMacroFacts = serde_json::from_value(json).unwrap();
    assert_eq!(old.facts_version, 0);
    assert!(old.facts_version < RUST_MACRO_FACTS_VERSION);
}
