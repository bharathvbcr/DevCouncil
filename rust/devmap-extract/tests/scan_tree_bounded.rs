//! A bounded scan refuses a tree whose source text is larger than the caller
//! allows it to hold, and returns the same tree as an unbounded one otherwise.

use devmap_extract::{scan_tree_bounded, scan_tree_cancellable, ScanBudgetExceeded};
use std::sync::atomic::AtomicBool;

fn tree(label: &str, files: usize, bytes_each: usize) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-bounded-scan-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    for n in 0..files {
        let body = format!("def f{n}():\n    return 1\n#{}\n", "x".repeat(bytes_each));
        std::fs::write(root.join(format!("m{n}.py")), body).expect("source");
    }
    root
}

#[test]
fn a_tree_over_the_limit_is_refused_with_the_typed_error_and_no_partial_tree() {
    let root = tree("over", 16, 4096);
    let cancel = AtomicBool::new(false);
    let error = scan_tree_bounded(&root, &cancel, 8 * 1024).expect_err("over the limit");
    let exceeded = error
        .downcast_ref::<ScanBudgetExceeded>()
        .unwrap_or_else(|| panic!("not the budget error: {error:#}"));
    assert_eq!(exceeded.limit_bytes, 8 * 1024);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_tree_within_the_limit_scans_exactly_as_the_unbounded_scan_does() {
    let root = tree("within", 4, 64);
    let cancel = AtomicBool::new(false);
    let bounded = scan_tree_bounded(&root, &cancel, 1024 * 1024).expect("within the limit");
    let plain = scan_tree_cancellable(&root, &cancel).expect("plain");
    assert_eq!(bounded.sources, plain.sources);
    assert_eq!(bounded.sources.len(), 4);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_empty_tree_is_within_a_zero_limit() {
    let root = tree("empty", 0, 0);
    let cancel = AtomicBool::new(false);
    let scanned = scan_tree_bounded(&root, &cancel, 0).expect("nothing to hold");
    assert!(scanned.sources.is_empty());
    let _ = std::fs::remove_dir_all(root);
}
