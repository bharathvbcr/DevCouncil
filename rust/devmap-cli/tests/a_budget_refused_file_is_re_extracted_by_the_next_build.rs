//! A file refused for the wall-clock parse budget must be retried by the next
//! build even though its bytes did not change.
//!
//! `devmap build` short-circuits as "unchanged" when every content hash
//! matches, and `affected_closure` narrows a differential write to the files
//! whose hash moved. A refused file has an unchanged hash and a payload that
//! describes nothing, so both paths used to leave it refused for good, and the
//! status advice ("rebuild when the machine is idle") could never come true.
//!
//! The refusal is forced deterministically by seeding the store with a
//! generation built from a zero-budget extraction of the same bytes. There is
//! no production budget override, and inventing one would put a knob on the
//! shipped surface only a test uses.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use devmap_analyze::{analyze_with_discovery, DiscoveryCoverage};
use devmap_extract::{extract_file_with_budget, Extraction};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

const LIB: &str = "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n";

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn fixture(name: &str, files: &[(&str, &str)]) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "devmap-budget-rebuild-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    for (path, source) in files {
        std::fs::write(root.join(path), source).unwrap();
    }
    let db = root.join("index.sqlite");
    (root, db)
}

fn json(root: &Path, db: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(devmap())
        .arg("--db")
        .arg(db)
        .arg("--json")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "devmap {args:?} is not JSON ({error}): {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Replace the latest generation with one built from the same files, in which
/// `refused` was refused for the budget -- exactly what a build under load
/// stores.
fn seed_refusal(root: &Path, db: &Path, files: &[(&str, &str)], refused: &str) {
    let store = Store::open(db).unwrap();
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| {
            let budget = if *path == refused {
                Duration::ZERO
            } else {
                Duration::from_secs(60)
            };
            extract_file_with_budget(path, source, budget)
        })
        .collect();
    assert!(extractions.iter().any(|e| e.is_budget_refusal()));
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = analyze_with_discovery(&extractions, &resolution, DiscoveryCoverage::refused(0));
    let opts = GenerationWriteOpts {
        repo_root: root.to_str().map(str::to_string),
        discovery_refusals: Some(Vec::new()),
        over_limit: Some(Vec::new()),
        ..Default::default()
    };
    store
        .save_generation_with_opts(&extractions, &resolution, &analysis, opts)
        .unwrap();
}

fn names_in(db: &Path, path: &str) -> Vec<String> {
    let store = Store::open(db).unwrap();
    store
        .latest_symbol_names_by_file()
        .unwrap()
        .remove(path)
        .map(|names| names.into_iter().collect())
        .unwrap_or_default()
}

const FILES: [(&str, &str); 6] = [
    ("lib.py", LIB),
    ("a.py", "def a():\n    return 1\n"),
    ("b.py", "def b():\n    return 2\n"),
    ("c.py", "def c():\n    return 3\n"),
    ("d.py", "def d():\n    return 4\n"),
    ("e.py", "def e():\n    return 5\n"),
];

#[test]
fn an_unchanged_tree_with_a_refused_file_is_rebuilt_not_called_current() {
    let (root, db) = fixture("unchanged", &FILES);
    json(&root, &db, &["build", "."]);
    seed_refusal(&root, &db, &FILES, "lib.py");

    let before = json(&root, &db, &["status"]);
    assert_eq!(
        before["budget_refused"]["total"], 1,
        "precondition: {before}"
    );
    assert_eq!(
        names_in(&db, "lib.py"),
        ["lib.py"],
        "precondition: a refused file holds only its module node"
    );

    let build = json(&root, &db, &["build", "."]);
    assert_ne!(
        build["unchanged"], true,
        "the build called a refused file current: {build}"
    );

    let after = json(&root, &db, &["status"]);
    assert_eq!(after["budget_refused"]["total"], 0, "{after}");
    assert_eq!(names_in(&db, "lib.py"), ["alpha", "beta", "lib.py"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_differential_build_does_not_carry_the_refused_row_forward() {
    let (root, db) = fixture("differential", &FILES);
    json(&root, &db, &["build", "."]);
    seed_refusal(&root, &db, &FILES, "lib.py");
    // Another file moves, so the write is differential and lib.py's hash is
    // unchanged: the case where the refused row would be carried.
    std::fs::write(
        root.join("a.py"),
        "def a():\n    return 11\n\ndef a2():\n    return 12\n",
    )
    .unwrap();

    json(&root, &db, &["build", "."]);

    let after = json(&root, &db, &["status"]);
    assert_eq!(after["budget_refused"]["total"], 0, "{after}");
    assert_eq!(names_in(&db, "lib.py"), ["alpha", "beta", "lib.py"]);
    assert_eq!(names_in(&db, "a.py"), ["a", "a.py", "a2"]);
    let _ = std::fs::remove_dir_all(&root);
}
