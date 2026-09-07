//! `code_graph.json`'s third Intel panel must be filled or say why not.
//!
//! `viz.py:521` reads `meta.hotspots`, and the key was absent from every graph
//! the Rust kernel has ever written — so the panel rendered "(no churn data —
//! needs git history)" on a repository with three months of history, and
//! `hotspots_computed: false` was the only thing distinguishing that from a
//! repository that genuinely has none.
//!
//! Churn is the half the extraction pass cannot see, for the same reason lock
//! files are: it is repository *history*, not repository content. The kernel
//! already shells out to `git` for `HEAD`; one bounded `git log` answers this.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn temp_root(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-hotspots-{tag}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    root
}

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(path, body).expect("write fixture file");
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "-c",
            "user.email=fixture@example.com",
            "-c",
            "user.name=fixture",
        ])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// One file rewritten on every commit and imported by four others; one file
/// written once and imported by nobody. The first must outrank the second on
/// both halves of the score.
fn write_repository_with_history(root: &Path) {
    write(root, "src/hot.py", "def hot():\n    return 0\n");
    write(root, "src/cold.py", "def cold():\n    return 0\n");
    for index in 0..4 {
        write(
            root,
            &format!("src/importer{index}.py"),
            "from hot import hot\n\n\ndef use():\n    return hot()\n",
        );
    }
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial"]);
    for revision in 1..6 {
        write(
            root,
            "src/hot.py",
            &format!("def hot():\n    return {revision}\n"),
        );
        git(root, &["add", "src/hot.py"]);
        git(root, &["commit", "-qm", &format!("touch hot {revision}")]);
    }
}

fn build_graph(root: &Path) -> Value {
    let db = root.join("index.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--db"])
        .arg(&db)
        .arg("build")
        .arg(root)
        .output()
        .expect("run build");
    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .args(["--json", "--db"])
        .arg(&db)
        .arg("manifest")
        .arg(root)
        .output()
        .expect("run manifest");
    assert!(
        manifest.status.success(),
        "manifest failed: stdout={} stderr={}",
        String::from_utf8_lossy(&manifest.stdout),
        String::from_utf8_lossy(&manifest.stderr)
    );
    let graph_path = devmap_extract::paths::code_graph_path(root);
    serde_json::from_str(&fs::read_to_string(&graph_path).expect("read code_graph.json"))
        .expect("code_graph.json parses")
}

#[test]
fn the_file_the_repository_keeps_rewriting_is_a_hotspot() {
    let root = temp_root("history");
    write_repository_with_history(&root);
    let graph = build_graph(&root);

    let hotspots = graph["meta"]["hotspots"]
        .as_array()
        .unwrap_or_else(|| panic!("meta.hotspots is not an array: {}", graph["meta"]));
    assert!(
        !hotspots.is_empty(),
        "six commits in the last ten minutes and no hotspot: {hotspots:?}"
    );
    let hot = hotspots
        .iter()
        .find(|entry| entry["path"] == "src/hot.py")
        .unwrap_or_else(|| panic!("src/hot.py is missing from {hotspots:?}"));
    assert_eq!(hot["churn"], 6, "one initial commit plus five rewrites");
    assert!(
        hot["fan_in"].as_u64().unwrap_or(0) >= 1,
        "four files import src/hot.py: {hot}"
    );
    assert!(
        hot["score"].as_f64().unwrap_or(0.0) > 0.0,
        "a hotspot with no score is a row of nulls: {hot}"
    );

    // Ranked, then cut. The list is capped, so a cut that did not rank first is
    // the R7 defect `dead_symbol_candidates` was fixed for.
    let scores: Vec<f64> = hotspots
        .iter()
        .map(|entry| entry["score"].as_f64().unwrap_or(0.0))
        .collect();
    let mut descending = scores.clone();
    descending.sort_by(|left, right| right.partial_cmp(left).expect("no NaN scores"));
    assert_eq!(scores, descending, "hotspots are not ranked: {hotspots:?}");
    assert!(
        scores[0]
            >= hotspots
                .iter()
                .find(|entry| entry["path"] == "src/cold.py")
                .and_then(|entry| entry["score"].as_f64())
                .unwrap_or(0.0),
        "the file rewritten six times must outrank the one written once"
    );
}

#[test]
fn the_hotspot_panel_says_it_was_computed_and_how_far_it_got() {
    let root = temp_root("provenance");
    write_repository_with_history(&root);
    let graph = build_graph(&root);
    let meta = &graph["meta"]["devmap_rust"];

    assert_eq!(
        meta["hotspots_computed"],
        serde_json::json!(true),
        "the churn pass ran over a repository with history: {meta}"
    );
    let shown = meta["hotspots_shown"].as_u64().expect("hotspots_shown");
    let total = meta["hotspots_total"].as_u64().expect("hotspots_total");
    assert!(shown <= total, "shown {shown} exceeds total {total}");
    assert_eq!(
        meta["hotspots_truncated"],
        serde_json::json!(shown < total),
        "the truncation flag must follow the two counts it summarises"
    );
    assert_eq!(
        shown as usize,
        graph["meta"]["hotspots"].as_array().expect("array").len(),
        "the count and the list must be counting the same thing"
    );
}

/// The distinction the whole marker exists for: a tree with no history at all
/// produces an empty list *and* a reason, never an empty list that reads as
/// "this repository has no hotspots".
#[test]
fn a_tree_with_no_git_history_says_so_rather_than_reporting_none() {
    let root = temp_root("no-git");
    write(root.as_path(), "src/app.py", "def app():\n    return 1\n");
    let graph = build_graph(&root);
    let meta = &graph["meta"]["devmap_rust"];

    assert_eq!(
        meta["hotspots_computed"],
        serde_json::json!(false),
        "there is no git repository here, so nothing can have been read: {meta}"
    );
    let reason = meta["hotspots_unavailable_reason"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "an uncomputed field must name why, or `false` carries no information"
    );
    assert!(
        graph["meta"]["hotspots"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "the value has to agree with the marker: {}",
        graph["meta"]["hotspots"]
    );
}

/// Determinism (R4): two renderings of one generation are the same bytes.
#[test]
fn the_ranking_is_the_same_on_two_renderings() {
    let root = temp_root("determinism");
    write_repository_with_history(&root);
    let first = build_graph(&root);
    // A second `manifest` over the same store and the same tree.
    let second = build_graph(&root);
    assert_eq!(first["meta"]["hotspots"], second["meta"]["hotspots"]);
}
