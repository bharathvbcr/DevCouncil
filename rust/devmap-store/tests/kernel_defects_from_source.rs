//! Store-level regression gates for kernel defects K1 and K5 whose fixtures
//! are extracted from source.
//!
//! Split from `kernel_defects.rs` so that target runs without grammars: these
//! three build their generations with `extract_file`, so they need `parse` and
//! are declared with it. The rest of the K-series store gates are grammar-free
//! and stay there.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::Store;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// K5: prose and data formats are not parse failures.
///
/// `history.parse_failed` counted every `ParseOutcome::Failed`, and a file with
/// no linked grammar reports `Failed` — so on this repository 294 of 1,310
/// files were reported as parse failures and every one of them was Markdown,
/// JSON, YAML, config or HTML. Nothing was broken; there is simply no grammar
/// for prose, and there never will be. The count that mattered — the 16 files
/// tree-sitter parsed and flagged errors in — was invisible underneath it.
///
/// The distinguishing case is the notebook. `"notebook"` is a non-declarative
/// language, so a classifier keyed on the language alone would call a malformed
/// `.ipynb` "not applicable" and lose a genuine failure. The classifier asks
/// the *engine*, which records whether a grammar was expected at all.
#[test]
fn k5_prose_formats_are_not_parse_failures_but_broken_files_still_are() {
    use devmap_extract::extract_file;

    let prose = [
        ("notes.md", "# Title\n\nSome prose.\n"),
        ("data.json", "{\"a\": 1}\n"),
        ("conf.yaml", "a: 1\n"),
        ("page.html", "<html><body>hi</body></html>\n"),
    ];
    for (path, source) in prose {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.is_parse_failure(),
            "{path} has no grammar by design and must not count as a parse failure \
             (outcome {:?}, engine {:?})",
            extraction.parse_outcome,
            extraction.engine
        );
        assert!(
            extraction
                .symbols
                .iter()
                .any(|symbol| symbol.kind == devmap_extract::SymbolKind::File),
            "{path} must still be addressable as a File node"
        );
    }

    // A notebook that is not valid JSON *is* a failure, and shares its language
    // with the prose formats above.
    let broken_notebook = extract_file("broken.ipynb", "{not json");
    assert!(
        broken_notebook.is_parse_failure(),
        "a malformed notebook is a real failure: {:?} / {:?}",
        broken_notebook.parse_outcome,
        broken_notebook.engine
    );

    // A syntactically broken Python file is `Partial`, never `Failed` —
    // tree-sitter is error-tolerant, so it reports error ranges rather than
    // refusing the file. It was never part of `parse_failed` and must not
    // become part of it: `Partial` is the tier this fix exists to make visible.
    let broken_python = extract_file("broken.py", "def a(:\n  return\n");
    assert!(
        matches!(
            broken_python.parse_outcome,
            devmap_extract::ParseOutcome::Partial { .. }
        ),
        "a broken .py parses partially, it does not fail: {:?}",
        broken_python.parse_outcome
    );
    assert!(!broken_python.is_parse_failure());
}

/// K5: the history row a build writes must carry the corrected count.
///
/// Store-level rather than extractor-level: `parse_failed` is computed where
/// the generation is persisted, so the classifier being right is only half of
/// it — the writer has to ask.
#[test]
fn k5_build_history_parse_failed_excludes_grammarless_prose() {
    use devmap_analyze::analyze;
    use devmap_extract::extract_file;
    use devmap_resolve::Resolver;

    let extractions = vec![
        extract_file("a.py", "def a():\n    return 1\n"),
        extract_file("README.md", "# hi\n"),
        extract_file("data.json", "{\"a\": 1}\n"),
        extract_file("broken.ipynb", "{not json"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = analyze(&extractions, &resolution);

    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let row = store.build_history(1).unwrap().pop().unwrap();
    assert_eq!(
        row.parse_failed, 1,
        "only the malformed notebook is a parse failure; the Markdown and JSON \
         files have no grammar by design"
    );
}

/// K1(b): an absent path the graph still claims is a *deletion*, not garbage.
///
/// Dropping it would leave the stored generation asserting a file that is gone,
/// which is the failure the reconcile must not cause while fixing the one it
/// exists for.
#[test]
fn k1_reconcile_keeps_deletions_the_drain_still_has_to_process() {
    use devmap_analyze::analyze;
    use devmap_extract::extract_file;
    use devmap_resolve::Resolver;

    let dir = tmp_dir("k1-deletion");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("pkg")).unwrap();
    let store = Store::open(dir.join("devmap.sqlite")).unwrap();

    let extractions = vec![
        extract_file("pkg/removed.py", "def removed(): pass\n"),
        extract_file("kept.py", "def kept(): pass\n"),
    ];
    fs::write(root.join("kept.py"), "def kept(): pass\n").unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    store
        .enqueue_pending_paths(&[
            "pkg/removed.py".to_string(),
            "pkg".to_string(),
            "never_indexed.py".to_string(),
        ])
        .unwrap();
    // The directory is removed too, so both the file and its parent are absent.
    fs::remove_dir_all(root.join("pkg")).unwrap();

    let outcome = store.reconcile_pending_paths(&root).unwrap();
    let remaining = store.get_pending_paths().unwrap();
    assert!(
        remaining.contains(&"pkg/removed.py".to_string()),
        "a deleted file the graph still claims must stay queued: {remaining:?} \
         (dropped {:?})",
        outcome.dropped
    );
    assert!(
        remaining.contains(&"pkg".to_string()),
        "a deleted directory with indexed descendants must stay queued: {remaining:?}"
    );
    assert!(
        !remaining.contains(&"never_indexed.py".to_string()),
        "an absent path with nothing indexed under it is garbage: {remaining:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}
