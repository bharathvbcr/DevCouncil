#![cfg(feature = "parse")]
//! S2. A call written with a receiver expression, in any syntactic shape,
//! never carries a `UniqueGlobal` or `AmbiguousGlobal` resolution: the bare-name
//! rung answers only receiver-less calls. The fixture holds chains, macro
//! arguments, calls on call results and subscripts, each beside a repository
//! function of the member's name, and the test scans every stored edge and
//! ledger row of those call sites.

use devmap_extract::extract_file;
use devmap_resolve::model::{Resolution, ResolutionResult};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| (path, extract_file(path, source)))
        .map(|(_, extraction)| extraction)
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

fn name_only(resolution: &Resolution) -> bool {
    matches!(
        resolution,
        Resolution::UniqueGlobal { .. } | Resolution::AmbiguousGlobal { .. }
    )
}

#[test]
fn rust_chains_macro_arguments_and_calls_on_calls() {
    let result = resolve(&[
        (
            "src/lib.rs",
            "pub fn count() -> usize { 0 }\npub fn trim() -> usize { 0 }\npub fn len() -> usize { 0 }\n\
             pub fn chain(s: &str) -> usize {\n    s.chars().count()\n}\n\
             pub fn macro_arg(s: &str) {\n    assert!(s.trim().len() > 0);\n    println!(\"{}\", s.chars().count());\n}\n\
             pub fn on_a_call(s: &str) -> usize {\n    make(s).trim().len()\n}\n\
             fn make(s: &str) -> String { s.to_string() }\n",
        ),
    ]);
    for edge in &result.edges {
        let bound_by_name = edge.resolution.as_deref().is_some_and(name_only);
        assert!(
            !(bound_by_name
                && matches!(
                    edge.source_symbol.as_str(),
                    "src/lib.rs::chain" | "src/lib.rs::macro_arg" | "src/lib.rs::on_a_call"
                )),
            "a receiver call carries a name-only resolution: {edge:?}"
        );
    }
    for row in &result.unresolved {
        if row.receiver.is_some() {
            assert!(
                !name_only(&row.resolution),
                "a receiver call's ledger row carries a name-only resolution: {row:?}"
            );
        }
    }
}

#[test]
fn python_chains_subscripts_and_calls_on_calls() {
    let result = resolve(&[
        (
            "lib.py",
            "def strip():\n    return 1\n\ndef get():\n    return 2\n\ndef load():\n    return 3\n",
        ),
        (
            "app.py",
            "def chain(x):\n    return x.a.b.strip()\n\n\
             def on_a_call(x):\n    return fetch(x).strip().get()\n\n\
             def subscript(x):\n    return x[0].load()\n",
        ),
    ]);
    for edge in &result.edges {
        assert!(
            !(edge.source_file == "app.py" && edge.resolution.as_deref().is_some_and(name_only)),
            "a receiver call carries a name-only resolution: {edge:?}"
        );
    }
    for row in result
        .unresolved
        .iter()
        .filter(|row| row.receiver.is_some())
    {
        assert!(!name_only(&row.resolution), "{row:?}");
    }
}

#[test]
fn typescript_chains_and_optional_calls() {
    let result = resolve(&[
        (
            "lib.ts",
            "export function trim() { return 1; }\nexport function map() { return 2; }\n",
        ),
        (
            "app.ts",
            "export function chain(s: string) { return s.trim().length; }\n\
             export function opt(x: any) { return x?.items.map(f => f).trim(); }\n",
        ),
    ]);
    for edge in &result.edges {
        assert!(
            !(edge.source_file == "app.ts" && edge.resolution.as_deref().is_some_and(name_only)),
            "{edge:?}"
        );
    }
    for row in result
        .unresolved
        .iter()
        .filter(|row| row.receiver.is_some())
    {
        assert!(!name_only(&row.resolution), "{row:?}");
    }
}
