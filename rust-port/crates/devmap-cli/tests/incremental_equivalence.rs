use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn build(root: &std::path::Path) {
    let out = Command::new(devmap())
        .args(["build", "."])
        .current_dir(root)
        .output()
        .expect("devmap build");
    assert!(
        out.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every edge in the committed generation, as a comparable set.
fn graph(root: &std::path::Path) -> Vec<String> {
    let store = devmap_store::Store::open(root.join(".devcouncil/codeintel/index.sqlite")).unwrap();
    let mut rows = store.latest_edges_for_test().unwrap();
    rows.sort();
    rows
}

/// An incremental build must produce the same graph as a cold build.
///
/// B3/SC2 resolves only the files a change can reach: the changed files plus
/// every file mentioning a name whose definition moved. That is sound because
/// the resolver's two global indexes are keyed by symbol name — but "sound
/// because of an argument" is exactly what SC16 was, where an incremental build
/// diverged permanently and no gate could see it. So the property is asserted
/// directly: edit a file, build incrementally, then build the identical tree
/// cold, and require the graphs to be equal.
#[test]
fn an_incremental_build_equals_a_cold_build() {
    let root = std::env::temp_dir().join(format!("devmap-incr-{}", std::process::id()));
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.py"), "def a():\n    return helper()\n").unwrap();
    std::fs::write(src.join("b.py"), "def helper():\n    return 1\n").unwrap();
    std::fs::write(
        src.join("c.py"),
        "from b import helper\n\ndef c():\n    return helper()\n",
    )
    .unwrap();

    build(&root);

    // Change a definition other files depend on: `helper` gains a sibling, so
    // the symbol index changes for a name two other files call.
    std::fs::write(
        src.join("b.py"),
        "def helper():\n    return 1\n\ndef helper_two():\n    return 2\n",
    )
    .unwrap();
    build(&root);
    let incremental = graph(&root);

    // Same tree, no history.
    std::fs::remove_dir_all(root.join(".devcouncil")).unwrap();
    build(&root);
    let cold = graph(&root);

    assert_eq!(
        incremental, cold,
        "an incremental build must produce exactly the cold graph"
    );
    assert!(
        !cold.is_empty(),
        "fixture precondition: the graph is non-empty"
    );

    let _ = std::fs::remove_dir_all(&root);
}
