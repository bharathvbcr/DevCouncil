//! `build --manifest` after a small edit rewrites only the blocks of
//! `code_graph.json` whose content changed.
//!
//! A linked worktree's export is a copy-on-write clone of the main checkout's,
//! so it shares disk blocks with it only for as long as those blocks are not
//! rewritten. The writer used to emit compact JSON through write-temp-and-rename:
//! one inserted node moved every later byte, so every block of a 71 MB graph
//! was new after a one-file edit. These tests hold the end-to-end behaviour:
//! the file a real build writes keeps its unchanged blocks byte-identical at
//! the same offsets, and still parses to the value a fresh write produces.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BLOCK: usize = 4096;
const FILES: usize = 160;
const FUNCTIONS_PER_FILE: usize = 20;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    let output = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    assert!(
        output.status.success(),
        "devmap {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn module_source(file: usize) -> String {
    let mut text = String::new();
    for function in 0..FUNCTIONS_PER_FILE {
        text.push_str(&format!(
            "def f{file}_{function}(value):\n    return value + {function}\n\n\n"
        ));
    }
    text
}

/// A git repository big enough that its verbose graph passes the size below
/// which the writer does not pad.
fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-manifest-blocks-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    for file in 0..FILES {
        std::fs::write(root.join(format!("src/m{file}.py")), module_source(file)).unwrap();
    }
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "t@example.invalid"]);
    git(&root, &["config", "user.name", "t"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "fixture"]);
    root
}

fn graph_path(root: &Path) -> PathBuf {
    devmap_extract::paths::code_graph_path(root)
}

fn differing_blocks(old: &[u8], new: &[u8]) -> usize {
    new.chunks(BLOCK)
        .enumerate()
        .filter(|(index, block)| old.chunks(BLOCK).nth(*index) != Some(*block))
        .count()
}

#[test]
fn a_small_edit_rewrites_a_bounded_number_of_graph_blocks() {
    let root = fixture("edit");
    run(&root, &["build", ".", "--manifest", "--force"]);
    let before = std::fs::read(graph_path(&root)).unwrap();
    assert!(
        before.len() > 1 << 20,
        "the fixture graph must be large enough to be laid out: {} bytes",
        before.len()
    );

    // Five files each gain one function: a few new nodes and edges, scattered.
    for file in [3, 40, 77, 110, 150] {
        let path = root.join(format!("src/m{file}.py"));
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str(&format!("def added_{file}(value):\n    return value\n"));
        std::fs::write(path, text).unwrap();
    }
    run(&root, &["build", ".", "--manifest", "--force"]);
    let after = std::fs::read(graph_path(&root)).unwrap();

    let total = after.len().div_ceil(BLOCK);
    let differing = differing_blocks(&before, &after);
    // What the compact writer's bytes would have cost for the same two graphs.
    let compact = |bytes: &[u8]| {
        serde_json::to_string(&serde_json::from_slice::<serde_json::Value>(bytes).unwrap())
            .unwrap()
            .into_bytes()
    };
    let shifted = differing_blocks(&compact(&before), &compact(&after));
    assert!(
        shifted > total / 2,
        "the premise: compact JSON shifts most of the file ({shifted} of {total} blocks)"
    );
    // Five files, each adding a node, an edge, a dead-code row and a name in the
    // meta lists: a few blocks apiece, wherever in the file they land.
    assert!(
        differing <= 5 * 4 * 4,
        "{differing} of {total} blocks of the graph changed after a five-file edit"
    );
    assert!(
        differing * 5 < shifted,
        "{differing} blocks changed against {shifted} for compact JSON"
    );
    assert!(
        after.len().abs_diff(before.len()) < 8 * BLOCK,
        "the file's length moved by {} bytes: its tail shifted",
        after.len().abs_diff(before.len())
    );
    let graph: serde_json::Value = serde_json::from_slice(&after).expect("valid JSON");

    // The same generation written afresh, to a path nothing was laid out at: the
    // rewrite over the previous layout must parse to the same value.
    let fresh_path = root.join("fresh_graph.json");
    run(
        &root,
        &[
            "manifest",
            "--graph-output",
            fresh_path.to_str().unwrap(),
            "--force",
        ],
    );
    let fresh: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fresh_path).unwrap()).unwrap();
    assert_eq!(
        graph, fresh,
        "a rewrite over the previous layout must not change the value"
    );

    let nodes = graph["nodes"].as_array().unwrap();
    assert!(
        nodes.iter().any(|node| node["id"]
            .as_str()
            .is_some_and(|id| id.ends_with("::added_77"))),
        "the rewritten graph carries the edit"
    );
    let _ = std::fs::remove_dir_all(&root);
}
