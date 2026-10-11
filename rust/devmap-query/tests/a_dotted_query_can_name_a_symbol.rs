//! `impact Thing.go` and `impact go` start from the method `go` of `Thing`.
//!
//! `go` is a source extension, so `Thing.go` read as a file path; no such file
//! is indexed and the answer was "no indexed traversal start" even though the
//! symbol is. The extension guard that keeps `impact go` off every Go file
//! also rejected the qualified tail `a.ts::Thing.go`.

use devmap_extract::extract_file;
use devmap_query::{Request, ResolutionAvailability, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn store(files: &[(&str, &str)]) -> Store {
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

fn callers(store: &Store, query: &str) -> anyhow::Result<(Vec<String>, bool)> {
    let answer = StoreQueryEngine::new(store).impact(Request {
        query: query.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth: 2,
    })?;
    let available = matches!(answer.resolution, ResolutionAvailability::Available);
    Ok((
        answer
            .items
            .iter()
            .map(|e| e.source_symbol.clone())
            .collect(),
        available,
    ))
}

const SOURCE: &str = "export class Thing { go() { return 1; } }\n\
                      export function use() { return new Thing().go(); }\n";

#[test]
fn a_dotted_name_that_is_no_file_starts_from_the_symbol() {
    let store = store(&[("a.ts", SOURCE)]);
    for query in ["Thing.go", "go"] {
        let (found, available) = callers(&store, query).unwrap();
        assert!(available, "{query} must find a start");
        assert_eq!(found, ["a.ts::use"], "{query}");
    }
}

#[test]
fn a_real_file_of_that_name_still_wins() {
    // `Thing.go` the file is a file: the fallback is only for a name that
    // matches none.
    let store = store(&[("Thing.go", "package p\nfunc Run() {}\n"), ("a.ts", SOURCE)]);
    let (_, available) = callers(&store, "Thing.go").unwrap();
    assert!(available);
}

#[test]
fn a_dotted_name_that_is_neither_file_nor_symbol_has_no_start() {
    let store = store(&[("a.ts", SOURCE)]);
    let (found, available) = callers(&store, "Other.go").unwrap();
    assert!(!available);
    assert!(found.is_empty());
}
