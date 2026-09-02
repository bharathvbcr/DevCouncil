//! Coverage invariants stated over **on-disk truth**, not the index's own view.
//!
//! This is the Class B gate from `rust-port/PLAN.md` §3.1. Two of the eight
//! kernel findings were of this shape, and neither was visible to any measure
//! that existed at the time:
//!
//! - **K2** — `.proto` and `.ps1` were dropped at *discovery*, so they never
//!   reached the extractor to be counted as missing. Extraction parity was
//!   green while two entire languages were absent from every graph.
//! - **K1** — a grammarless file was recorded in `generation_files` and
//!   contributed no node at all, so it could not be an edge target.
//!
//! The common cause is that a coverage number computed from the index can only
//! describe files the index already admits. A gate stated over the *filesystem*
//! is the only kind that can catch a file the pipeline never saw, which is why
//! these assertions walk the fixture directly instead of asking the walker.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Files planted in the fixture, each chosen for a distinct reason.
///
/// The grammarless entries are the point: `.proto` and `.ps1` have real
/// declarations and no linked grammar, `.psd1` is grammarless with nothing to
/// recover, and `.md` is prose that must never be declaration-scanned. All four
/// reached the same nodeless state before K1/K2, by different routes.
const FIXTURE: &[(&str, &str)] = &[
    ("src/service.py", "def hello():\n    return 1\n"),
    ("src/lib.rs", "pub fn process() -> u8 { 1 }\n"),
    (
        "api/user.proto",
        "syntax = \"proto3\";\npackage api.v1;\n\nmessage User {\n  string id = 1;\n}\n",
    ),
    (
        "scripts/deploy.ps1",
        "function Get-Widget {\n  param($Id)\n}\n",
    ),
    ("scripts/module.psd1", "@{ ModuleVersion = '1.0' }\n"),
    (
        "docs/design.md",
        "# Design\n\n```go\ntype Invented struct {\n}\n```\n",
    ),
    ("config/app.json", "{\n  \"port\": 8080\n}\n"),
];

fn write_fixture(root: &Path) {
    for (rel, body) in FIXTURE {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("fixture paths are nested"))
            .expect("create fixture dir");
        fs::write(&path, body).expect("write fixture file");
    }
}

/// Every file on disk that the indexer *claims* to handle, found by walking the
/// filesystem rather than by asking the indexer what it found.
fn indexable_files_on_disk(root: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("read fixture dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found
        .into_iter()
        .filter_map(|path| {
            let rel = path.strip_prefix(root).ok()?.to_str()?.to_string();
            devmap_extract::is_indexable_source(&rel).then_some(rel)
        })
        .collect()
}

fn scratch_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-coverage-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create scratch root");
    root
}

/// Class B, part one: discovery admits every file it claims to handle. (K2)
///
/// `is_indexable_source` is both the admission rule *and* the definition of
/// what should have been indexed, so a language it silently declines to name
/// disappears from both sides of the comparison at once. Asserting the two
/// agree therefore pins the weaker property — that admission is at least
/// self-consistent — while the explicit `.proto`/`.ps1` check below pins the
/// one that actually regressed.
#[test]
fn discovery_admits_every_file_it_claims_to_handle() {
    let root = scratch_root("discovery");
    write_fixture(&root);

    let on_disk = indexable_files_on_disk(&root);
    let discovered: BTreeSet<String> = devmap_extract::collect_sources(&root)
        .expect("collect sources")
        .into_iter()
        .map(|(path, _)| path)
        .collect();

    let missing: Vec<&String> = on_disk.difference(&discovered).collect();
    assert!(
        missing.is_empty(),
        "these files are on disk and indexable but were never discovered: {missing:?}"
    );

    for required in ["api/user.proto", "scripts/deploy.ps1"] {
        assert!(
            discovered.contains(required),
            "{required} is grammarless source with real declarations and must be \
             discovered; it was invisible to the graph before K2. discovered: {discovered:?}"
        );
    }
    let _ = fs::remove_dir_all(&root);
}

/// Class B, part two: every extracted file is addressable in the graph. (K1)
///
/// "Indexed" and "present in the graph" were different sets: a grammarless file
/// was recorded and contributed zero nodes, so nothing could point at it. The
/// assertion is deliberately over *every* extraction rather than over the
/// grammarless ones, because the invariant is not "fallback works" — it is that
/// no route through extraction can produce a file the graph cannot name.
#[test]
fn every_extracted_file_has_at_least_one_node() {
    let root = scratch_root("nodes");
    write_fixture(&root);

    let extractions = devmap_extract::extract_tree(&root).expect("extract tree");
    assert!(
        extractions.len() >= FIXTURE.len(),
        "expected at least the {} planted files, got {}",
        FIXTURE.len(),
        extractions.len()
    );

    let nodeless: Vec<&str> = extractions
        .iter()
        .filter(|extraction| extraction.symbols.is_empty())
        .map(|extraction| extraction.file_path.as_str())
        .collect();
    assert!(
        nodeless.is_empty(),
        "these files were indexed but contribute no node, so they cannot be an \
         edge target: {nodeless:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// The gate must not be satisfiable by inventing symbols. (K3)
///
/// Class B and Class C pull in opposite directions: the cheapest way to make
/// "every file has a node" true everywhere is to declaration-scan everything,
/// which is exactly how 457 symbols were read out of fenced code blocks in
/// Markdown design documents. This pins both halves at once — the Markdown file
/// is addressable *and* declares nothing.
#[test]
fn addressability_is_not_bought_with_invented_symbols() {
    let root = scratch_root("prose");
    write_fixture(&root);

    let extractions = devmap_extract::extract_tree(&root).expect("extract tree");
    let markdown = extractions
        .iter()
        .find(|extraction| extraction.file_path.ends_with("design.md"))
        .expect("the markdown fixture must be indexed");

    let declarations: Vec<&str> = markdown
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != devmap_extract::SymbolKind::File)
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert!(
        declarations.is_empty(),
        "prose must contribute the File node and nothing else; a type described \
         inside a fenced code block is not declared by the document: {declarations:?}"
    );
    assert!(
        markdown
            .symbols
            .iter()
            .any(|symbol| symbol.kind == devmap_extract::SymbolKind::File),
        "the markdown file must still be addressable"
    );
    let _ = fs::remove_dir_all(&root);
}
