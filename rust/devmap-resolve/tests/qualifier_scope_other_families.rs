//! Probe: the Go qualifier leak (`go_qualifier_scope`) for every other family
//! whose extractor emits `TypeQualifier`. A qualified parameter type in one
//! function must not qualify a same-named, bare-typed parameter in another.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

fn bare_param_binds_its_type(path: &str, source: &str, user: &str, target: &str) {
    let extractions: Vec<Extraction> = vec![extract_file(path, source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).unwrap();
    let hit = result.edges.iter().any(|e| {
        e.source_symbol.rsplit(['.', ':']).next() == Some(user)
            && e.target_symbol.rsplit(['.', ':']).next() == Some(target)
    });
    assert!(
        hit,
        "{path}: `{user}` has a bare `{target}` parameter; edges {:?}",
        result
            .edges
            .iter()
            .map(|e| (e.source_symbol.as_str(), e.target_symbol.as_str()))
            .collect::<Vec<_>>()
    );
}

#[test]
fn rust() {
    bare_param_binds_its_type(
        "lib.rs",
        "use std::fmt;\n\npub struct Thing;\n\n\
         pub fn first(params: fmt::Formatter) {}\n\n\
         pub fn second(params: Thing) {}\n",
        "second",
        "Thing",
    );
}

#[test]
fn typescript() {
    bare_param_binds_its_type(
        "m.ts",
        "import * as j from 'json';\n\nexport class Thing {}\n\n\
         export function first(params: j.Raw) {}\n\n\
         export function second(params: Thing) {}\n",
        "second",
        "Thing",
    );
}

#[test]
fn python() {
    bare_param_binds_its_type(
        "m.py",
        "import json\n\n\nclass Thing:\n    pass\n\n\n\
         def first(params: json.Raw):\n    pass\n\n\n\
         def second(params: Thing):\n    pass\n",
        "second",
        "Thing",
    );
}

#[test]
fn kotlin() {
    bare_param_binds_its_type(
        "m.kt",
        "import kotlinx.json\n\nclass Thing\n\n\
         fun first(params: json.Raw) {}\n\n\
         fun second(params: Thing) {}\n",
        "second",
        "Thing",
    );
}

#[test]
fn swift() {
    bare_param_binds_its_type(
        "m.swift",
        "import Foundation\n\nstruct Thing {}\n\n\
         func first(params: Foundation.Data) {}\n\n\
         func second(params: Thing) {}\n",
        "second",
        "Thing",
    );
}

#[test]
fn java() {
    bare_param_binds_its_type(
        "M.java",
        "import java.util.List;\n\nclass Thing {}\n\nclass M {\n\
         void first(java.util.List params) {}\n\
         void second(Thing params) {}\n}\n",
        "second",
        "Thing",
    );
}
