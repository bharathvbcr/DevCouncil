//! A resolution budget that is exceeded caps the affected candidate lists and
//! says so; it no longer aborts the whole build.
//!
//! The reproduction (Linux `c339f401`, 74,042 files): resolve refused with
//! `ambiguity evidence bytes requires 134218520 units, exceeding the 134217728
//! limit … no partial graph was produced`, after 124 s, and the store was
//! empty. Here the same budget is lowered so a handful of files trips it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = run(root, args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "devmap {args:?} stdout is not JSON ({error}): {stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-resolve-budget-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// Forty files each defining `leaf`, and one caller that names it.
fn repository(name: &str) -> PathBuf {
    let root = scratch(name);
    for index in 0..40 {
        std::fs::write(root.join(format!("p{index}.py")), "def leaf():\n    pass\n").unwrap();
    }
    std::fs::write(
        root.join("caller.py"),
        "def run():\n    leaf()\n    leaf()\n    leaf()\n",
    )
    .unwrap();
    root
}

const TIGHT: &[&str] = &["--resolve-ambiguity-evidence-bytes", "1024"];

fn build_args(extra: &[&'static str]) -> Vec<&'static str> {
    let mut args = vec!["--json", "build", "."];
    args.extend_from_slice(extra);
    args
}

#[test]
fn an_exceeded_budget_still_builds_and_the_gap_is_in_the_result_and_in_status() {
    let root = repository("degrade");
    let build = json(&root, &build_args(TIGHT));

    assert_eq!(
        build["files_indexed"], 41,
        "the whole tree was indexed: {build}"
    );
    let gaps = build["resolution_budget"].as_array().unwrap();
    assert_eq!(gaps.len(), 1, "{build}");
    assert_eq!(gaps[0]["limit"], "ambiguity evidence bytes");
    assert_eq!(gaps[0]["names_truncated"], 1);
    assert_eq!(gaps[0]["candidates_dropped"], 32);
    assert_eq!(gaps[0]["sites"], 3);

    let status = json(&root, &["--json", "status"]);
    let bucket = &status["coverage_gaps"]["resolution_budget"];
    assert_eq!(bucket["total"], 1, "{status}");
    let reason = bucket["paths"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("32 candidate(s) dropped") && reason.contains("leaf"),
        "the gap says what was cut and by how much: {reason}"
    );
    assert_eq!(
        bucket["paths"][0]["path"],
        "resolve:ambiguity evidence bytes"
    );
}

#[test]
fn a_build_inside_its_budget_claims_no_gap() {
    let root = repository("control");
    let build = json(&root, &build_args(&[]));
    assert_eq!(
        build["resolution_budget"].as_array().unwrap().len(),
        0,
        "{build}"
    );
    let status = json(&root, &["--json", "status"]);
    assert_eq!(
        status["coverage_gaps"]["resolution_budget"]["total"], 0,
        "{status}"
    );
}

#[test]
fn the_gap_is_rederived_on_every_write_and_never_carried_forward() {
    let root = repository("stale");
    let _ = json(&root, &build_args(TIGHT));
    // A differential write: one file changed, and this time the budget holds.
    std::fs::write(
        root.join("caller.py"),
        "def run():\n    leaf()\n    leaf()\n    leaf()\n    leaf()\n",
    )
    .unwrap();
    let rebuilt = json(&root, &build_args(&[]));
    assert_eq!(
        rebuilt["resolution_budget"].as_array().unwrap().len(),
        0,
        "{rebuilt}"
    );
    let status = json(&root, &["--json", "status"]);
    assert_eq!(
        status["coverage_gaps"]["resolution_budget"]["total"], 0,
        "a stale cap must not outlive the resolution that made it: {status}"
    );
}

#[test]
fn the_refuse_flag_keeps_the_old_abort() {
    let root = repository("refuse");
    let out = run(
        &root,
        &[
            "--json",
            "build",
            ".",
            "--resolve-ambiguity-evidence-bytes",
            "1024",
            "--refuse-on-resolve-limit",
        ],
    );
    assert!(
        !out.status.success(),
        "a refused resolution is a failed build"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("resolution refused"), "{stderr}");
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path:?}: {e}")))
        .unwrap()
}

/// The exported artifacts were the one place a capped build said nothing: the
/// stderr line, the build JSON, the coverage-gap row and `status` all named the
/// cap while `repo_map.json` said `graph_degraded: false` and `code_graph.json`
/// said `analysis_status: ok`. Both now read the same `AnalysisStatus` a partial
/// parse does.
#[test]
fn a_capped_build_marks_both_exported_maps_degraded_and_names_the_budget() {
    let root = repository("exported-capped");
    let mut args = build_args(TIGHT);
    args.extend_from_slice(&["--manifest"]);
    let _ = json(&root, &args);
    let _ = run(
        &root,
        &[
            "manifest",
            ".",
            "--output",
            "repo_map.json",
            "--graph-output",
            "code_graph.json",
        ],
    );

    let map = read_json(&root.join("repo_map.json"));
    assert_eq!(
        map["graph_degraded"], true,
        "a capped resolution is not a complete graph: {}",
        map["graph_degraded_reason"]
    );
    let reason = map["graph_degraded_reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("ambiguity evidence bytes budget exceeded")
            && reason.contains("32 candidate(s) dropped"),
        "the degradation names the budget and what it cost: {reason:?}"
    );

    let graph = read_json(&root.join("code_graph.json"));
    let status = graph["meta"]["devmap_rust"]["analysis_status"]
        .as_str()
        .unwrap_or("");
    assert!(
        status.starts_with("partial:") && status.contains("ambiguity evidence bytes budget"),
        "code_graph.json carries the same verdict: {status:?}"
    );

    // The build's own default artifacts agree with the explicit export.
    let default_map = read_json(&root.join(".devmap").join("repo_map.json"));
    assert_eq!(default_map["graph_degraded"], true, "{default_map}");
}

#[test]
fn an_uncapped_build_keeps_both_exported_maps_clean() {
    let root = repository("exported-control");
    let _ = json(&root, &build_args(&[]));
    let _ = run(
        &root,
        &[
            "manifest",
            ".",
            "--output",
            "repo_map.json",
            "--graph-output",
            "code_graph.json",
        ],
    );
    let map = read_json(&root.join("repo_map.json"));
    assert_eq!(map["graph_degraded"], false, "{map}");
    assert_eq!(map["graph_degraded_reason"], "");
    let graph = read_json(&root.join("code_graph.json"));
    assert_eq!(graph["meta"]["devmap_rust"]["analysis_status"], "ok");
}
