//! `devmap status` must say when files are still refused for the wall-clock
//! parse budget after the serial retry.
//!
//! The budget is wall clock, so a refusal is a fact about the machine's load
//! as much as about the file. The build retries serially; whatever is still
//! refused is stored as `ParseOutcome::Failed` with the budget prefix and, in
//! `status`, read as an ordinary parse failure -- indistinguishable from a
//! grammar fault, though only the former is cured by running again. The status
//! carries a `budget_refused` listing (capped, with its total) and says so in
//! `degraded_reason`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use devmap_analyze::analyze;
use devmap_extract::{extract_file_with_budget, Extraction};
use devmap_resolve::Resolver;
use devmap_store::{Store, COVERAGE_GAP_SAMPLE};

const SRC: &str = "def alpha():\n    return beta()\n\ndef beta():\n    return 1\n";

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-budget-status-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn commit(store: &Store, extractions: &[Extraction]) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    store
        .save_generation(extractions, &resolution, &analysis)
        .unwrap();
}

fn refused(path: &str) -> Extraction {
    let extraction = extract_file_with_budget(path, SRC, Duration::ZERO);
    assert!(extraction.is_budget_refusal(), "fixture must be a refusal");
    extraction
}

fn ok(path: &str) -> Extraction {
    let extraction = extract_file_with_budget(path, SRC, Duration::from_secs(60));
    assert!(!extraction.is_parse_failure());
    extraction
}

fn status(dir: &Path, db: &Path) -> serde_json::Value {
    let out = Command::new(devmap())
        .arg("--db")
        .arg(db)
        .args(["--json", "status"])
        .current_dir(dir)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "status is not JSON ({error}): {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn store_at(dir: &Path) -> (Store, PathBuf) {
    let db = dir.join("index.sqlite");
    (Store::open(&db).unwrap(), db)
}

#[test]
fn a_still_refused_file_is_named_as_budget_refused_in_status() {
    let dir = scratch("one");
    let (store, db) = store_at(&dir);
    commit(&store, &[refused("slow.py"), ok("fine.py")]);
    drop(store);

    let status = status(&dir, &db);
    let listing = &status["budget_refused"];
    assert_eq!(listing["total"], 1, "{status}");
    assert_eq!(listing["truncated"], false, "{status}");
    assert_eq!(listing["paths"][0]["path"], "slow.py", "{status}");
    let reason = status["degraded_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("1 file(s) refused for the parse budget"),
        "{status}"
    );
    assert!(!reason.contains("fine.py"), "{status}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_refused_is_a_measured_zero_not_an_absent_field() {
    let dir = scratch("none");
    let (store, db) = store_at(&dir);
    commit(&store, &[ok("a.py"), ok("b.py")]);
    drop(store);
    let status = status(&dir, &db);
    assert_eq!(status["budget_refused"]["total"], 0, "{status}");
    assert!(status["budget_refused"]["paths"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        !status["degraded_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("parse budget"),
        "{status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_file_refused_is_listed_in_full_when_under_the_cap() {
    let dir = scratch("all");
    let (store, db) = store_at(&dir);
    commit(&store, &[refused("b.py"), refused("a.py")]);
    drop(store);
    let status = status(&dir, &db);
    assert_eq!(status["budget_refused"]["total"], 2, "{status}");
    let names: Vec<&str> = status["budget_refused"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["path"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a.py", "b.py"], "{status}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_refused_and_then_parsed_by_a_later_generation_leaves_the_list() {
    let dir = scratch("recovered");
    let (store, db) = store_at(&dir);
    commit(&store, &[refused("a.py")]);
    commit(&store, &[ok("a.py")]);
    drop(store);
    let status = status(&dir, &db);
    assert_eq!(
        status["budget_refused"]["total"], 0,
        "stale generation: {status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_deleted_after_refusal_leaves_the_list() {
    let dir = scratch("deleted");
    let (store, db) = store_at(&dir);
    commit(&store, &[refused("gone.py"), ok("kept.py")]);
    // The next generation no longer contains the refused file.
    commit(&store, &[ok("kept.py")]);
    drop(store);
    let status = status(&dir, &db);
    assert_eq!(status["budget_refused"]["total"], 0, "{status}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn very_many_refused_files_are_capped_and_the_total_is_carried() {
    let dir = scratch("many");
    let (store, db) = store_at(&dir);
    let total = COVERAGE_GAP_SAMPLE + 25;
    let files: Vec<Extraction> = (0..total)
        .map(|i| refused(&format!("f{i:04}.py")))
        .collect();
    commit(&store, &files);
    drop(store);
    let status = status(&dir, &db);
    let listing = &status["budget_refused"];
    assert_eq!(listing["total"], total, "{status}");
    assert_eq!(listing["shown"], COVERAGE_GAP_SAMPLE, "{status}");
    assert_eq!(listing["truncated"], true, "{status}");
    assert_eq!(
        listing["paths"].as_array().unwrap().len(),
        COVERAGE_GAP_SAMPLE
    );
    let reason = status["degraded_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains(&format!("{total} file(s) refused for the parse budget")),
        "{status}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_non_budget_failure_is_not_called_a_budget_refusal() {
    let dir = scratch("other");
    let (store, db) = store_at(&dir);
    let mut other = ok("odd.py");
    other.parse_outcome = devmap_extract::ParseOutcome::Failed {
        reason: "grammar fault".into(),
    };
    commit(&store, &[other]);
    drop(store);
    let status = status(&dir, &db);
    assert_eq!(status["budget_refused"]["total"], 0, "{status}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_store_with_no_generation_has_no_measurement() {
    let dir = scratch("empty");
    let (store, db) = store_at(&dir);
    drop(store);
    let status = status(&dir, &db);
    assert!(status["budget_refused"].is_null(), "{status}");
    let _ = std::fs::remove_dir_all(&dir);
}
