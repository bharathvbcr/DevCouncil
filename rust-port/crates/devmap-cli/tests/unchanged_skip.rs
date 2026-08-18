use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn build(root: &std::path::Path) -> String {
    let out = Command::new(devmap())
        .args(["build", "."])
        .current_dir(root)
        .output()
        .expect("devmap build");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn edge_count(root: &std::path::Path) -> usize {
    let store = devmap_store::Store::open(root.join(".devcouncil/codeintel/index.sqlite")).unwrap();
    store.status("x").unwrap().edge_count
}

/// A rebuild skips resolve/analyze only when the tree is genuinely unchanged.
///
/// B3/SC2: re-resolving an unchanged tree costs 54% of a build (measured:
/// resolve 2,145 ms + analyze 835 ms of 5.5 s over 1,610 files) to reproduce a
/// graph that is already stored. Skipping it is safe *only* because identical
/// inputs give an identical graph, which the determinism gate proves — so the
/// risk is not the skip itself but skipping when something did change. Each
/// mutation below must force a real rebuild; a rename with byte-identical
/// content is the case a file-count check alone would miss.
#[test]
fn a_rebuild_skips_only_when_the_tree_is_unchanged() {
    let root = std::env::temp_dir().join(format!("devmap-skip-{}", std::process::id()));
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.py"), "def a():\n    return helper()\n").unwrap();
    std::fs::write(src.join("b.py"), "def helper():\n    return 1\n").unwrap();

    build(&root);
    let baseline = edge_count(&root);
    assert!(
        baseline > 0,
        "fixture precondition: the cold build produced edges"
    );

    // Unchanged: skipped, and the stored graph is untouched.
    let output = build(&root);
    assert!(
        output.contains("No source changes"),
        "an unchanged tree must skip the rebuild: {output}"
    );
    assert_eq!(
        edge_count(&root),
        baseline,
        "a skipped build must not alter the graph"
    );

    // Every one of these must produce a real rebuild.
    std::fs::write(
        src.join("b.py"),
        "def helper():\n    return 1\n\ndef added():\n    return 2\n",
    )
    .unwrap();
    assert!(
        !build(&root).contains("No source changes"),
        "a modified file rebuilds"
    );

    std::fs::write(src.join("c.py"), "def c():\n    return helper()\n").unwrap();
    assert!(
        !build(&root).contains("No source changes"),
        "an added file rebuilds"
    );

    std::fs::remove_file(src.join("c.py")).unwrap();
    assert!(
        !build(&root).contains("No source changes"),
        "a deleted file rebuilds"
    );

    // Same content, same file count, different path: only a per-path hash
    // comparison catches this.
    std::fs::rename(src.join("b.py"), src.join("renamed.py")).unwrap();
    assert!(
        !build(&root).contains("No source changes"),
        "a rename with identical content rebuilds"
    );

    let _ = std::fs::remove_dir_all(&root);
}
