//! A source file over `MAX_SOURCE_BYTES` is a coverage category of its own:
//! never a refusal, never retried, and never silent.
//!
//! The audit's reproduction (2026-10-09, `big`): `lib.py` defines
//! `helper_only_big`, and a committed 1.7 MB `gen.py` calls it forty thousand
//! times. After `devmap build --manifest --full`, `dead` reported the helper at
//! 0.9 with no caveat, `status` said fresh with every coverage total at 0, and
//! `gen.py` appeared in no artifact. Growing an indexed `gen.py` past the
//! limit was reported as `removed: 1`, the next build printed `-1`, and
//! `impact helper_only_big` answered zero callers — fresh again, still with no
//! gap.
//!
//! Driven through the real binary, because the defect was the surfaces
//! disagreeing with the graph, not any one function.

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
        "devmap-over-limit-{name}-{}-{}",
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

const LIMIT: u64 = devmap_extract::MAX_SOURCE_BYTES;

/// A Python caller of `helper_only_big` of at least `bytes` bytes.
fn caller_of_at_least(bytes: u64) -> String {
    let mut body = String::from("from lib import helper_only_big\n\n\ndef run():\n");
    while (body.len() as u64) < bytes {
        body.push_str("    helper_only_big()\n");
    }
    body
}

fn write_big(root: &Path, gen_bytes: u64) {
    std::fs::write(
        root.join("lib.py"),
        "def helper_only_big():\n    return 42\n",
    )
    .unwrap();
    std::fs::write(root.join("gen.py"), caller_of_at_least(gen_bytes)).unwrap();
}

fn over_limit_paths(status: &serde_json::Value) -> Vec<(String, Option<u64>, Option<u64>)> {
    status["coverage_gaps"]["over_limit"]["paths"]
        .as_array()
        .unwrap_or_else(|| panic!("status must carry an over_limit bucket: {status}"))
        .iter()
        .map(|row| {
            (
                row["path"].as_str().unwrap().to_string(),
                row["bytes"].as_u64(),
                row["limit"].as_u64(),
            )
        })
        .collect()
}

fn helper_finding(dead: &serde_json::Value) -> Option<serde_json::Value> {
    dead["items"]
        .as_array()
        .expect("dead items")
        .iter()
        .find(|item| {
            item["symbol_name"]
                .as_str()
                .is_some_and(|name| name.ends_with("helper_only_big"))
        })
        .cloned()
}

/// The audit's `big` fixture: an over-limit caller is listed everywhere, and
/// `helper_only_big` is not reported at the confident tier with no caveat.
#[test]
fn the_big_fixture_lists_the_skipped_caller_and_caveats_dead_and_impact() {
    let root = fixture("big");
    write_big(&root, LIMIT + 600_000);
    let gen_bytes = std::fs::metadata(root.join("gen.py")).unwrap().len();

    let build = json(&root, &["--json", "build", ".", "--full", "--manifest"]);
    assert_eq!(
        build["discovery_refused_files"], 0,
        "over the limit is not a refusal: {build}"
    );
    let listed = &build["over_limit_files"];
    assert_eq!(
        listed,
        &serde_json::json!([{ "path": "gen.py", "bytes": gen_bytes, "limit": LIMIT }]),
        "the build result names the skipped file, its size and the limit: {build}"
    );

    let dead = json(&root, &["--json", "dead"]);
    let finding = helper_finding(&dead)
        .unwrap_or_else(|| panic!("helper_only_big is still a candidate, demoted: {dead}"));
    let confidence = finding["confidence"].as_f64().expect("confidence");
    let caveat = dead["walk_incomplete"].as_str().unwrap_or("");
    assert!(
        confidence < 0.9 && !caveat.is_empty(),
        "helper_only_big's only caller was never read, so it must be neither at \
         the confident tier nor uncaveated (confidence {confidence}, caveat \
         {caveat:?}): {dead}"
    );
    assert!(
        caveat.contains("exceeding the source size limit"),
        "the caveat names the cause: {caveat:?}"
    );

    let impact = json(&root, &["--json", "impact", "helper_only_big"]);
    assert!(
        impact["walk_incomplete"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "an empty blast radius over a corpus with an unread caller is a lower \
         bound and must say so: {impact}"
    );

    let status = json(&root, &["--json", "status"]);
    assert_eq!(status["is_fresh"], true, "{status}");
    assert_eq!(
        status["coverage_gaps"]["over_limit"]["total"], 1,
        "{status}"
    );
    assert_eq!(
        over_limit_paths(&status),
        vec![("gen.py".to_string(), Some(gen_bytes), Some(LIMIT))],
        "status lists path, bytes and limit: {status}"
    );
    assert_eq!(
        status["coverage_gaps"]["discovery_refused"]["total"], 0,
        "and not as a refusal: {status}"
    );

    let repo_map: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(devmap_extract::paths::repo_map_path(&root))
            .expect("--manifest wrote repo_map.json"),
    )
    .unwrap();
    assert_eq!(
        repo_map["over_limit_files"]["count"], 1,
        "repo_map.json counts the skipped files: {}",
        repo_map["over_limit_files"]
    );
    assert_eq!(
        repo_map["over_limit_files"]["files"][0]["path"], "gen.py",
        "and names them: {}",
        repo_map["over_limit_files"]
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A file that grows past the limit is "skipped: over limit", not "removed" —
/// in `status.delta` before the rebuild and in the build's own summary.
#[test]
fn a_file_crossing_the_limit_is_reported_as_skipped_not_removed() {
    let root = fixture("crossing");
    write_big(&root, 848_958);
    let build = json(&root, &["--json", "build", "."]);
    assert_eq!(
        build["over_limit_files"],
        serde_json::json!([]),
        "under the limit, gen.py is indexed: {build}"
    );
    let impact = json(&root, &["--json", "impact", "helper_only_big"]);
    assert!(
        impact["total"].as_u64().unwrap_or(0) > 0,
        "precondition: gen.py's calls reach the helper while it is indexed: {impact}"
    );

    std::fs::write(root.join("gen.py"), caller_of_at_least(LIMIT + 700_000)).unwrap();

    let status = json(&root, &["--json", "status"]);
    assert_eq!(status["is_fresh"], false, "the tree moved: {status}");
    let delta = &status["delta"];
    assert_eq!(delta["removed"], 0, "gen.py was not removed: {status}");
    assert_eq!(
        delta["skipped_over_limit"], 1,
        "it crossed the size limit, and says so: {status}"
    );
    assert_eq!(
        delta["sample_paths"],
        serde_json::json!(["gen.py"]),
        "{status}"
    );

    let build = json(&root, &["--json", "build", "."]);
    assert_eq!(build["file_progress"]["delta"]["removed"], 0, "{build}");
    assert_eq!(
        build["file_progress"]["delta"]["skipped_over_limit"], 1,
        "{build}"
    );

    // The human summary of the same build shape.
    std::fs::write(root.join("gen.py"), caller_of_at_least(848_958)).unwrap();
    let _ = json(&root, &["--json", "build", "."]);
    std::fs::write(root.join("gen.py"), caller_of_at_least(LIMIT + 700_000)).unwrap();
    let (stdout, stderr) = run(&root, &["build", "."]);
    let human = format!("{stdout}{stderr}");
    assert!(
        human.contains("-0 · 1 skipped: over limit"),
        "the Changes line names the skip rather than counting a removal: {human}"
    );

    let status = json(&root, &["--json", "status"]);
    assert_eq!(status["is_fresh"], true, "rebuilt: {status}");
    assert_eq!(
        over_limit_paths(&status)
            .into_iter()
            .map(|(path, _, _)| path)
            .collect::<Vec<_>>(),
        vec!["gen.py"],
        "and still listed: {status}"
    );
    let impact = json(&root, &["--json", "impact", "helper_only_big"]);
    assert!(
        impact["walk_incomplete"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "the callers that vanished with gen.py leave a caveat behind: {impact}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// A vendored or call-free over-limit file is listed but does not degrade the
/// graph: it cannot hold a caller, and charging it is what left every
/// repository with a vendored blob permanently `partial`.
#[test]
fn a_vendored_or_data_file_over_the_limit_is_listed_without_degrading() {
    let root = fixture("vendored");
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    std::fs::create_dir_all(root.join("vendor")).unwrap();
    std::fs::write(root.join("vendor/blob.py"), caller_of_at_least(LIMIT + 10)).unwrap();
    let mut data = String::from("{\"rows\": [");
    while (data.len() as u64) <= LIMIT {
        data.push_str("\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\",");
    }
    data.push_str("\"end\"]}");
    std::fs::write(root.join("fixture.json"), data).unwrap();

    let _ = json(&root, &["--json", "build", "."]);
    let status = json(&root, &["--json", "status"]);
    assert_eq!(
        status["degraded_reason"],
        serde_json::Value::Null,
        "a vendored file and a data file cannot hide a caller: {status}"
    );
    let mut listed: Vec<String> = over_limit_paths(&status)
        .into_iter()
        .map(|(path, _, _)| path)
        .collect();
    listed.sort();
    let indexable_json = devmap_extract::is_indexable_source("fixture.json");
    let mut expected = vec!["vendor/blob.py".to_string()];
    if indexable_json {
        expected.insert(0, "fixture.json".to_string());
    }
    assert_eq!(listed, expected, "but both are listed: {status}");
    assert_eq!(status["is_fresh"], true, "{status}");

    let _ = std::fs::remove_dir_all(&root);
}
