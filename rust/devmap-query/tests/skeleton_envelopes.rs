//! Skeleton query: empty vs not-in-index, missing signatures, truncation.

#![cfg(feature = "parse")]

use std::path::{Path, PathBuf};

use devmap_extract::extract_file;
use devmap_query::{SkeletonPresence, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-skel-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn store_of(root: &Path, files: &[(&str, &str)]) -> Store {
    let mut extractions = Vec::new();
    for (path, body) in files {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(root.join(parent)).unwrap();
            }
        }
        std::fs::write(root.join(path), body).unwrap();
        extractions.push(extract_file(path, body));
    }
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
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    store
}

#[test]
fn not_in_index_and_empty_are_different_envelopes() {
    let root = scratch("presence");
    let store = store_of(
        &root,
        &[
            ("has.py", "def a():\n    return 1\n"),
            ("empty.py", "# comment only\n"),
        ],
    );
    let engine = StoreQueryEngine::new(&store);

    let missing = engine.skeleton("nope.py", 2000).unwrap();
    assert_eq!(missing.presence, SkeletonPresence::NotInIndex);
    assert_eq!(missing.shown, 0);
    assert_eq!(missing.total, 0);
    assert!(!missing.truncated);

    let empty = engine.skeleton("empty.py", 2000).unwrap();
    assert_ne!(
        empty.presence,
        SkeletonPresence::NotInIndex,
        "an indexed comment-only file must not claim not_in_index: {empty:?}"
    );

    let has = engine.skeleton("has.py", 2000).unwrap();
    assert_eq!(has.presence, SkeletonPresence::Indexed);
    assert!(has.total >= 1);
    for item in &has.items {
        if item.signature.is_none() {
            assert_eq!(
                item.signature_note.as_deref(),
                Some("not extracted"),
                "missing signature must say so: {item:?}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tiny_budget_truncates_and_reports_shown_total() {
    let root = scratch("budget");
    let mut body = String::new();
    for i in 0..200 {
        body.push_str(&format!("def f{i}():\n    return {i}\n"));
    }
    let store = store_of(&root, &[("many.py", body.as_str())]);
    let report = StoreQueryEngine::new(&store)
        .skeleton("many.py", 80)
        .unwrap();
    assert_eq!(report.presence, SkeletonPresence::Indexed);
    assert!(report.truncated, "{report:?}");
    assert!(report.shown < report.total, "{report:?}");
    assert_eq!(report.items.len() as u32, report.shown);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn notebook_path_is_not_in_index_envelope() {
    let root = scratch("notebook");
    let store = store_of(&root, &[("a.py", "def a():\n    return 1\n")]);
    let report = StoreQueryEngine::new(&store)
        .skeleton("analysis.ipynb", 2000)
        .unwrap();
    assert_eq!(report.presence, SkeletonPresence::NotInIndex);
    assert_eq!(report.total, 0);
    assert!(!report.truncated);
    let _ = std::fs::remove_dir_all(&root);
}
