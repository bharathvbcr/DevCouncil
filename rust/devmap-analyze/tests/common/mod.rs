//! Loads an accuracy fixture the way `devmap build` does, in process.
//!
//! The steps mirror `devmap-cli/src/commands/build.rs`: scan the tree with the
//! same discovery rules, read the project manifests, extract, index the
//! manifests and the extractions, mark Go directories with unindexed files,
//! resolve, analyse. A fixture measured through a shorter path (flat
//! directory, no manifests) would score a resolver the product never runs.

#![allow(dead_code)]

use devmap_analyze::{analyze, AnalysisSummary};
use devmap_extract::model::Extraction;
use devmap_extract::{
    collect_project_manifests, extract_all, go_dirs_with_unindexed_files, scan_tree, FileRef,
};
use devmap_resolve::{ResolutionResult, Resolver};
use std::path::{Path, PathBuf};

/// `rust/testdata/accuracy`.
pub fn accuracy_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("devmap-analyze sits under the workspace root")
        .join("testdata/accuracy")
}

/// Every fixture directory: a child of `testdata/accuracy` holding `edges.tsv`,
/// sorted by name so the report order is stable.
pub fn fixture_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(accuracy_root())
        .expect("testdata/accuracy is readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("edges.tsv").is_file())
        .collect();
    dirs.sort();
    dirs
}

pub struct Built {
    pub extractions: Vec<Extraction>,
    pub resolution: ResolutionResult,
    pub analysis: AnalysisSummary,
}

pub fn build(dir: &Path) -> Built {
    let scanned = scan_tree(dir).unwrap_or_else(|e| panic!("{}: scan failed: {e}", dir.display()));
    let manifests = collect_project_manifests(dir)
        .unwrap_or_else(|e| panic!("{}: manifests unreadable: {e}", dir.display()));
    let refs: Vec<FileRef> = scanned
        .sources
        .iter()
        .map(|(path, source)| FileRef {
            path: path.as_str(),
            source: source.as_str(),
        })
        .collect();
    let extractions = extract_all(&refs);
    let mut resolver = Resolver::new();
    resolver.index_project_manifests(&manifests);
    resolver.index_extractions(&extractions);
    resolver.mark_go_dirs_incomplete(go_dirs_with_unindexed_files(
        dir,
        extractions.iter().map(|ext| ext.file_path.as_str()),
    ));
    let resolution = resolver
        .resolve_all(&extractions)
        .unwrap_or_else(|e| panic!("{}: resolution refused: {e:?}", dir.display()));
    let analysis = analyze(&extractions, &resolution);
    Built {
        extractions,
        resolution,
        analysis,
    }
}
