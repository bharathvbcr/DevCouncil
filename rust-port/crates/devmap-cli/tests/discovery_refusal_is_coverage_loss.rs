//! K-A2, discovery half: a file discovery **refused** costs the graph exactly
//! as much coverage as a file that failed to parse, and must be reported the
//! same way — through every surface a consumer reads, not just stderr.
//!
//! The owner is `DiscoverySkipReason::is_refusal` plus
//! `DiscoveryCoverage`, threaded into `analyze_with_discovery` so the refusal
//! count reaches `ExtractionCoverage` before the confidence cap is applied.
//! This file is the end-to-end pin on that owner through the real binary: the
//! artifacts, the dead-code tier, the build result and `status` must all agree
//! that the corpus was not read in full.
//!
//! The measured consequence is the fixture below. `app.py` is `helper`'s only
//! caller and is one line over the ceiling, so an unfolded refusal reported
//! `helper` dead at 0.9 with `resolution: Available` and `truncated: false` —
//! a confident finding drawn from a scan that never read the file holding the
//! answer.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> (String, String) {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let (stdout, stderr) = run(root, args);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!("devmap {args:?} stdout is not JSON ({error}): {stdout}\n--- stderr ---\n{stderr}")
    })
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-refusal-{name}-{}-{}",
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

/// `helper`'s only caller, written one line past the source ceiling.
fn write_oversized_caller(root: &Path) {
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    let head = "from lib import helper\n\n\ndef main():\n    return helper()\n";
    let pad = format!("# {}\n", "x".repeat(78));
    let mut body = String::from(head);
    while body.len() as u64 <= devmap_extract::MAX_SOURCE_BYTES {
        body.push_str(&pad);
    }
    assert!(
        body.len() as u64 > devmap_extract::MAX_SOURCE_BYTES,
        "fixture precondition: the caller must be past the ceiling"
    );
    std::fs::write(root.join("app.py"), body).unwrap();
}

/// The audit's own reproduction, end to end through the real binary.
#[test]
fn a_refused_file_degrades_the_graph_and_demotes_the_findings_it_could_not_check() {
    let root = fixture("audit");
    write_oversized_caller(&root);

    let build = json(&root, &["--json", "build", "."]);
    assert_eq!(
        build["discovery_refused_files"], 1,
        "the build result must carry the refused count as its own number — a \
         reader of `files_indexed` alone cannot tell a corpus that was read \
         from one that was refused: {build}"
    );
    assert_eq!(
        build["files_indexed"], 1,
        "a refused file is not an indexed one: {build}"
    );
    // Every file discovery admitted is in exactly one of the two numbers. This
    // is the property that survives a change of owner: whatever the refusal is
    // spelled as internally, nothing may fall out of both counts.
    assert_eq!(
        build["files_indexed"].as_u64().unwrap()
            + build["discovery_refused_files"].as_u64().unwrap(),
        2,
        "the two source files must be accounted for between them: {build}"
    );

    // The finding the audit caught: `helper` is only called from the file that
    // was refused, so "nothing calls it" is not a fact this run established.
    let dead = json(&root, &["--json", "dead"]);
    let helper = dead["items"]
        .as_array()
        .expect("dead items")
        .iter()
        .find(|item| item["symbol_name"] == "helper")
        .unwrap_or_else(|| panic!("helper must still be reported, demoted: {dead}"));
    let confidence = helper["confidence"].as_f64().expect("confidence");
    assert!(
        confidence <= 0.35 + 1e-6,
        "a dead-code finding made against a corpus with a refused file must be \
         capped below the confident tier, got {confidence}: {dead}"
    );

    // Both artifacts of the build must say the graph is degraded, and say why.
    let (_, _) = run(
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
    let repo_map: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("repo_map.json")).unwrap())
            .unwrap();
    assert_eq!(
        repo_map["graph_degraded"], true,
        "a build that could not read every file is not a complete graph: {}",
        repo_map["graph_degraded_reason"]
    );
    let reason = repo_map["graph_degraded_reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("call extraction did not cover the whole corpus"),
        "the degradation must name the coverage loss: {reason:?}"
    );
    assert!(
        reason.contains("refused by discovery"),
        "and must distinguish a refusal from a parse failure — the two have \
         different fixes: {reason:?}"
    );

    let graph: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("code_graph.json")).unwrap())
            .unwrap();
    let provenance = &graph["meta"]["devmap_rust"];
    let status = provenance["analysis_status"].as_str().unwrap_or("");
    assert!(
        status.starts_with("partial:"),
        "code_graph.json must agree with repo_map.json: {provenance}"
    );

    // `status` is what a health check reads, and it said nothing at all: the
    // store was undamaged, so `freshness_degraded_reason` had nothing to
    // report, and the *graph's* degradation reached no transport.
    let status = json(&root, &["--json", "status"]);
    let degraded = status["degraded_reason"].as_str().unwrap_or("");
    assert!(
        degraded.contains("call extraction did not cover the whole corpus"),
        "status must report the degradation the artifacts carry: {status}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `NonSource` is the ordinary case and must never become a coverage loss.
///
/// A README beside the code is not a gap in the graph. Counting it would put
/// every real repository permanently in the degraded state, and a flag that is
/// always on carries no information.
#[test]
fn an_ordinary_non_source_file_is_not_a_refusal() {
    let root = fixture("nonsource");
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    std::fs::write(
        root.join("app.py"),
        "from lib import helper\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    std::fs::write(root.join("notes.bin"), [0u8, 1, 2, 0xff]).unwrap();

    let build = json(&root, &["--json", "build", "."]);
    assert_eq!(
        build["discovery_refused_files"], 0,
        "a non-source file is not a refusal: {build}"
    );
    let status = json(&root, &["--json", "status"]);
    assert_eq!(
        status["degraded_reason"],
        serde_json::Value::Null,
        "a repository whose every source file parsed must report clean: {status}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A refusal on both sides of a rebuild is still "unchanged"; a file that
/// shrinks back under the ceiling is not.
///
/// The early return is the most frequent build in the system, and it is also
/// the one that must keep reporting the refusal it just re-checked — a warm
/// build that answers `discovery_refused_files: 0` where the cold build said
/// `1` would read as the degradation having lifted.
#[test]
fn a_refusal_is_stable_across_rebuilds_and_a_shrunken_file_is_re_extracted() {
    let root = fixture("unchanged");
    write_oversized_caller(&root);

    let first = json(&root, &["--json", "build", "."]);
    assert_eq!(first["discovery_refused_files"], 1, "{first}");

    let second = json(&root, &["--json", "build", "."]);
    assert_eq!(
        second["unchanged"], true,
        "a tree whose refusals are identical has not changed: {second}"
    );
    assert_eq!(
        second["discovery_refused_files"], 1,
        "the unchanged result reports the refusals it re-checked: {second}"
    );

    // Shrink the caller back under the ceiling: it is admissible again, so the
    // graph must be rebuilt and the degradation must lift.
    std::fs::write(
        root.join("app.py"),
        "from lib import helper\n\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    let third = json(&root, &["--json", "build", "."]);
    assert_eq!(
        third["unchanged"],
        serde_json::Value::Null,
        "a file that shrank under the ceiling must be re-extracted: {third}"
    );
    assert_eq!(third["discovery_refused_files"], 0, "{third}");

    let status = json(&root, &["--json", "status"]);
    assert_eq!(
        status["degraded_reason"],
        serde_json::Value::Null,
        "the degradation must lift once the file is readable: {status}"
    );
    let dead = json(&root, &["--json", "dead"]);
    assert!(
        !dead["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["symbol_name"] == "helper"),
        "helper is called from a file that is now indexed: {dead}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The rule that decides which skips are coverage loss has exactly one
/// spelling, and `NonSource` is not one of them.
///
/// The CLI, the daemon and the report all ask `DiscoverySkipReason::is_refusal`.
/// A second copy of the predicate is how the CLI and the daemon came to
/// disagree about the same discovery report in the first place.
#[test]
fn only_genuine_refusals_are_charged_against_coverage() {
    use devmap_extract::model::DiscoverySkipReason;

    for reason in [
        DiscoverySkipReason::Oversized {
            bytes: 2_000_000,
            limit: devmap_extract::MAX_SOURCE_BYTES,
        },
        DiscoverySkipReason::Unreadable {
            reason: "permission denied".to_string(),
        },
        DiscoverySkipReason::NonUtf8Path,
    ] {
        assert!(
            reason.is_refusal(),
            "a file the indexer turned away is coverage the graph lost: {reason:?}"
        );
    }
    assert!(
        !DiscoverySkipReason::NonSource.is_refusal(),
        "NonSource is the ordinary case, not a refusal"
    );

    // And the charge reaches the analysis: an unmeasured discovery contributes
    // nothing, a measured one contributes what it refused.
    assert_eq!(devmap_analyze::DiscoveryCoverage::none().charged(), 0);
    assert_eq!(
        devmap_analyze::DiscoveryCoverage::none().refused_files(),
        None
    );
    assert_eq!(devmap_analyze::DiscoveryCoverage::refused(3).charged(), 3);
    assert_eq!(
        devmap_analyze::DiscoveryCoverage::refused(0).refused_files(),
        Some(0),
        "a walk that refused nothing is not the same as no walk"
    );
}

/// A build pointed at a regular file is refused, and leaves no generation.
///
/// Measured with the release binary before the fix: `devmap build <file>`
/// exited 0, reported `files_indexed: 0` and wrote a generation — a check that
/// could not run reporting as one that ran — while `devmap build <missing>`
/// exited 1. The owner is `devmap_extract::scan_tree`; this pins that the
/// build arm propagates its refusal rather than swallowing it.
#[test]
fn a_build_whose_root_is_a_regular_file_is_refused_and_writes_nothing() {
    let root = fixture("fileroot");
    let file = root.join("notes.txt");
    std::fs::write(&file, "not a repository\n").unwrap();
    let db = root.join("store").join("devmap.sqlite");
    let db_arg = db.to_string_lossy().into_owned();
    let file_arg = file.to_string_lossy().into_owned();
    let out = Command::new(devmap())
        .args([
            "--json",
            "--db",
            &db_arg,
            "build",
            "--progress",
            "never",
            &file_arg,
        ])
        .current_dir(&root)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(
        out.status.code(),
        Some(0),
        "a root that is a file must be refused, not built as empty: {stdout}"
    );
    assert!(
        stderr.contains("not a directory") && stderr.contains("notes.txt"),
        "the refusal must name the root and say why: {stderr}"
    );
    let status = json(&root, &["--json", "--db", &db_arg, "status"]);
    assert_eq!(
        status["generation_id"],
        serde_json::Value::Null,
        "no generation may be written for a root that was never walked: {status}"
    );
}
