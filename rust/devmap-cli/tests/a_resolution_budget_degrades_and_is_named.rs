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
    env!("CARGO_BIN_EXE_devmap").to_string()
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

fn manifest_exports(root: &Path) {
    let _ = run(
        root,
        &[
            "manifest",
            ".",
            "--output",
            "repo_map.json",
            "--graph-output",
            "code_graph.json",
        ],
    );
}

/// A cap sets `AnalysisStatus::Partial`, and `Partial` flips
/// `liveness_unreachable_unreliable` — deliberately, by main's own rule
/// (`code_graph.rs`: "a coverage hole means a missing edge into a component
/// could invalidate every cluster in the result"). A capped candidate list is
/// exactly a missing edge, so reachability findings over a capped graph are as
/// unreliable as over a graph with a refused file. The query surface says so
/// (`walk_incomplete` on `dead`) and both exported maps carry the flag.
#[test]
fn a_cap_reaches_the_query_surface_and_flags_reachability_unreliable() {
    let root = repository("unreliable-capped");
    let _ = json(&root, &build_args(TIGHT));

    let dead = json(&root, &["--json", "dead"]);
    let walk = dead["walk_incomplete"].as_str().unwrap_or("");
    assert!(
        walk.contains("ambiguity evidence bytes budget exceeded")
            && walk.contains("32 candidate(s) dropped"),
        "`dead` over a capped graph says the walk is incomplete and why: {dead}"
    );

    manifest_exports(&root);
    let map = read_json(&root.join("repo_map.json"));
    assert_eq!(map["liveness_unreachable_unreliable"], true, "{map}");
    let graph = read_json(&root.join("code_graph.json"));
    assert_eq!(
        graph["meta"]["liveness_unreachable_unreliable"], true,
        "{graph}"
    );
}

#[test]
fn an_uncapped_build_leaves_reachability_reliable_and_the_walk_complete() {
    let root = repository("unreliable-control");
    let _ = json(&root, &build_args(&[]));
    let dead = json(&root, &["--json", "dead"]);
    // The ambiguous `leaf` sites still earn their own unresolved-attribution
    // note; what must be absent is any claim that a budget cut something.
    assert!(
        !dead["walk_incomplete"]
            .as_str()
            .unwrap_or("")
            .contains("budget"),
        "nothing capped, so no budget is blamed: {dead}"
    );
    manifest_exports(&root);
    assert_eq!(
        read_json(&root.join("repo_map.json"))["liveness_unreachable_unreliable"],
        false
    );
    assert_eq!(
        read_json(&root.join("code_graph.json"))["meta"]["liveness_unreachable_unreliable"],
        false
    );
}

/// The daemon has no `--resolve-*` flags, so the way it reports a cap is by
/// reading the stored analysis status of the generation it serves. A store the
/// CLI built under a tight budget, opened the way the daemon opens it, carries
/// the budget text in `degraded_reason`, identical to `devmap status`.
#[test]
fn a_store_built_under_a_tight_budget_reports_it_in_the_daemons_status() {
    use devmap_serve::protocol::{daemon_status_reason, UnappliedEdits};
    let root = repository("daemon-status");
    let db = root.join("index.sqlite");
    let db_arg = db.to_str().unwrap().to_string();
    let _ = json(
        &root,
        &[
            "--db",
            &db_arg,
            "--json",
            "build",
            ".",
            "--resolve-ambiguity-evidence-bytes",
            "1024",
        ],
    );
    let cli = json(&root, &["--db", &db_arg, "--json", "status"]);
    let cli_reason = cli["degraded_reason"].as_str().unwrap_or("").to_string();

    let store = devmap_store::Store::open_existing(&db).unwrap().unwrap();
    let status = store.status("probe").unwrap();
    let daemon = daemon_status_reason(&store, &status, &UnappliedEdits::default())
        .unwrap()
        .unwrap_or_default();
    assert!(
        daemon.contains("ambiguity evidence bytes budget exceeded")
            && daemon.contains("32 candidate(s) dropped"),
        "the daemon's status names the cap: {daemon:?}"
    );
    assert_eq!(daemon, cli_reason, "daemon and CLI status disagree");
}

/// A budget that trips on a list short enough to keep whole drops nothing, so it
/// is no gap: the build, `status` and both exported maps stay clean. (The
/// shape that reported "N candidate list(s) cut to a retained prefix, 0
/// candidate(s) dropped" and marked the graph degraded.)
#[test]
fn a_tripped_budget_that_drops_no_candidate_degrades_nothing() {
    let root = scratch("no-op-cut");
    for index in 0..3 {
        std::fs::write(root.join(format!("p{index}.py")), "def leaf():\n    pass\n").unwrap();
    }
    std::fs::write(
        root.join("caller.py"),
        "def run():\n    leaf()\n    leaf()\n",
    )
    .unwrap();
    let mut args = build_args(&["--resolve-ambiguity-evidence-bytes", "1"]);
    args.extend_from_slice(&["--manifest"]);
    let build = json(&root, &args);
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
    manifest_exports(&root);
    let map = read_json(&root.join("repo_map.json"));
    assert_eq!(
        map["graph_degraded"], false,
        "{}",
        map["graph_degraded_reason"]
    );
    assert_eq!(
        read_json(&root.join("code_graph.json"))["meta"]["devmap_rust"]["analysis_status"],
        "ok"
    );
}
