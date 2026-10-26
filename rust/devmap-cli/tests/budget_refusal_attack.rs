//! Adversarial probes: the budget-refused wording must reach every surface and
//! "the next build re-extracts them" must be true for a plain CLI build.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use devmap_analyze::{analyze_with_discovery, DiscoveryCoverage};
use devmap_extract::{extract_file_with_budget, Extraction};
use devmap_resolve::Resolver;
use devmap_serve::protocol::{daemon_status_reason, store_degraded_reason, UnappliedEdits};
use devmap_store::{GenerationWriteOpts, Store};

const CLAUSE: &str = "of the parse failure(s) are budget-refused";

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
        "devmap-budget-attack-{name}-{}",
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
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "devmap {args:?} not JSON ({e}): {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn seed_refusal(root: &Path, db: &Path, files: &[(&str, &str)], refused: &[&str]) {
    let store = Store::open(db).unwrap();
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| {
            let budget = if refused.contains(path) {
                Duration::ZERO
            } else {
                Duration::from_secs(60)
            };
            extract_file_with_budget(path, source, budget)
        })
        .collect();
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

const FILES: [(&str, &str); 4] = [
    (
        "lib.py",
        "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n",
    ),
    ("a.py", "def a():\n    return 1\n"),
    ("b.go", "package p\n\nfunc B() int { return 2 }\n"),
    ("c.rs", "fn c() -> i32 { 3 }\n"),
];

#[test]
fn every_surface_carries_the_cause_and_count_clause() {
    let (root, db) = fixture("surfaces", &FILES);
    json(&root, &db, &["build", "."]);
    seed_refusal(&root, &db, &FILES, &["lib.py", "b.go"]);

    // CLI status.
    let status = json(&root, &db, &["status"]);
    let cli = status["degraded_reason"].as_str().unwrap_or("").to_string();
    assert!(
        cli.contains(&format!("2 {CLAUSE}")),
        "CLI status lacks the clause: {cli}"
    );

    // Shared join (MCP status path) and daemon status.
    let store = Store::open_existing(&db).unwrap().unwrap();
    let st = store.status("probe").unwrap();
    let budget = store.latest_budget_refusals().unwrap();
    let joined = store_degraded_reason(&store, &st, budget.as_ref())
        .unwrap()
        .unwrap();
    assert!(
        joined.contains(&format!("2 {CLAUSE}")),
        "join lacks it: {joined}"
    );
    let daemon = daemon_status_reason(&store, &st, &UnappliedEdits::default())
        .unwrap()
        .unwrap();
    assert_eq!(daemon, joined, "daemon and CLI disagree");
    assert_eq!(cli, joined, "CLI status and the shared join disagree");

    // Query walk_incomplete: a dead-code answer over the stored generation.
    let dead = json(&root, &db, &["dead"]);
    let walk = dead["walk_incomplete"].as_str().unwrap_or("").to_string();
    if walk.contains("failed to parse") {
        assert!(
            walk.contains(&format!("2 {CLAUSE}")),
            "walk_incomplete says failed-to-parse without the budget clause: {walk}"
        );
    } else {
        panic!("dead answer over a refused corpus carries no parse-failure sentence: {dead}");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_clause_vanishes_when_the_next_plain_build_re_extracts() {
    let (root, db) = fixture("rebuild", &FILES);
    json(&root, &db, &["build", "."]);
    seed_refusal(&root, &db, &FILES, &["lib.py", "c.rs"]);
    let before = json(&root, &db, &["status"]);
    assert!(
        before["degraded_reason"]
            .as_str()
            .unwrap_or("")
            .contains(CLAUSE),
        "{before}"
    );

    // Plain `devmap build`, not --full, nothing on disk changed.
    json(&root, &db, &["build", "."]);
    let after = json(&root, &db, &["status"]);
    let reason = after["degraded_reason"].as_str().unwrap_or("");
    assert!(
        !reason.contains(CLAUSE) && !reason.contains("failed to parse"),
        "the sentence said the next build re-extracts them; it did not: {after}"
    );
    let dead = json(&root, &db, &["dead"]);
    let walk = dead["walk_incomplete"].as_str().unwrap_or("");
    assert!(!walk.contains("budget-refused"), "{walk}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_second_plain_build_is_unchanged_once_the_refusal_is_cured() {
    let (root, db) = fixture("settled", &FILES);
    json(&root, &db, &["build", "."]);
    seed_refusal(&root, &db, &FILES, &["lib.py"]);
    json(&root, &db, &["build", "."]);
    let again = json(&root, &db, &["build", "."]);
    assert_eq!(
        again["unchanged"], true,
        "cured corpus keeps rebuilding: {again}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
