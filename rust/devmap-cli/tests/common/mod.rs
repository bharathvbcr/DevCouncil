//! Fixture support: a speculative edge population.
//!
//! The resolver records an ambiguous bare name on the unresolved ledger and
//! emits no edge for it, so the speculative rung (`LanguageServerDispatch`) is
//! populated only by the opt-in language-server pass. These fixtures need that
//! population to exist, so they apply the same decision that pass makes, through
//! the same entry point, to every ambiguous ledger row.
#![allow(dead_code)]

use devmap_resolve::lsp::apply_language_server_resolutions;
use devmap_resolve::model::{Resolution, ResolutionResult};

/// Turn every ambiguous-site ledger row into a language-server dispatch set
/// over the same candidates, returning how many edges that added.
pub fn dispatch_ambiguous_sites(resolution: &mut ResolutionResult) -> usize {
    let decisions: Vec<(usize, Resolution)> = resolution
        .unresolved
        .iter()
        .enumerate()
        .filter_map(|(index, row)| match &row.resolution {
            Resolution::AmbiguousGlobal { candidates, .. } => Some((
                index,
                Resolution::LanguageServerDispatch {
                    candidates: std::sync::Arc::clone(candidates),
                    server: "fixture-server".to_string(),
                    server_version: "0".to_string(),
                },
            )),
            _ => None,
        })
        .collect();
    apply_language_server_resolutions(resolution, &decisions)
}

/// Rewrite the store `devmap build` left at `db` as a new generation that also
/// carries the speculative edges a language server's dispatch set adds.
///
/// A binary-level test cannot reach the speculative rung by building a corpus:
/// the resolver keeps an ambiguous bare name on the ledger and emits no edge
/// for it, and the dispatch rung needs a running language server. This reads
/// the files the build read, resolves them, applies the dispatch decision to
/// every ambiguous ledger row (one per distinct site, as the resolver's own
/// edge dedup would have collapsed repeats), and saves the result through the
/// store's own write path, so the CLI queries a real generation.
pub fn add_dispatch_edges(root: &std::path::Path, db: &std::path::Path, files: &[&str]) {
    let extractions: Vec<devmap_extract::model::Extraction> = files
        .iter()
        .map(|rel| {
            let source = std::fs::read_to_string(root.join(rel)).expect("fixture file is readable");
            devmap_extract::extract_file(rel, &source)
        })
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let mut resolution = resolver
        .resolve_all(&extractions)
        .expect("fixture resolves");
    let mut seen = std::collections::BTreeSet::new();
    resolution.unresolved.retain(|row| {
        !matches!(row.resolution, Resolution::AmbiguousGlobal { .. })
            || seen.insert((
                row.source_file.clone(),
                row.source_symbol.clone(),
                row.callee_name.clone(),
            ))
    });
    assert!(
        dispatch_ambiguous_sites(&mut resolution) > 0,
        "the fixture must hold an ambiguous site to turn into dispatch edges"
    );
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = devmap_store::Store::open(db).expect("the build's store opens");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("the dispatch generation writes");
}
