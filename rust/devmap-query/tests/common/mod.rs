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
