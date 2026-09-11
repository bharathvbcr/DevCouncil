//! `devmap status` names the files behind its coverage numbers.
//!
//! `degraded_reason` has always carried three numbers — "2 file(s) failed to
//! parse, 1 recovered by pattern (no calls extracted), 1 refused by discovery
//! and never read at all" — and no surface anywhere carried a path. On this
//! repository a real refusal is an escaping symlink (or an unreadable /
//! non-UTF-8 path); oversized sources are an ordinary skip under the 1 MiB
//! ceiling (`DiscoverySkipReason::Oversized::is_refusal` is false), so they
//! must not be used as the refusal fixture here.
//!
//! Each list is capped at `devmap_store::COVERAGE_GAP_SAMPLE` and carries
//! `{shown, total, truncated}`, because a list that stops at fifty without
//! saying so reads exactly like a corpus with fifty holes in it.

use std::path::{Path, PathBuf};
use std::process::Command;

use devmap_store::Store;

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
        "devmap-status-names-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`); canonicalize
    // so the escaping-symlink refusal is measured against the same root the
    // indexer walks.
    root.canonicalize().unwrap()
}

/// Plant a source-shaped symlink whose target sits outside the repository.
fn plant_escaping_source(root: &Path, link_name: &str) -> PathBuf {
    let outside = root.with_file_name(format!(
        "{}-outside",
        root.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("outside")
    ));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).unwrap();
    let target = outside.join("secret.py");
    std::fs::write(&target, "def secret():\n    return 0\n").unwrap();
    let link = root.join(link_name);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &link).unwrap();
    outside
}

/// One escaping-symlink refusal, one with no linked grammar, one ordinary.
fn write_a_corpus_with_two_holes(root: &Path) -> PathBuf {
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    let outside = plant_escaping_source(root, "app.py");
    std::fs::write(
        root.join("svc.proto"),
        "syntax = \"proto3\";\nmessage Ping { string id = 1; }\n",
    )
    .unwrap();
    outside
}

#[test]
fn status_names_the_refused_and_the_unparsed_paths() {
    let root = fixture("holes");
    let outside = write_a_corpus_with_two_holes(&root);

    let build = json(&root, &["--json", "build", "."]);
    assert_eq!(
        build["discovery_refused_files"], 1,
        "fixture precondition: the escaping symlink is refused: {build}"
    );

    let status = json(&root, &["--json", "status"]);
    let gaps = &status["coverage_gaps"];
    assert!(
        !gaps.is_null(),
        "`degraded_reason` states three numbers and nothing stated the paths: {status}"
    );

    let refused = &gaps["discovery_refused"];
    assert_eq!(refused["total"], 1, "{status}");
    assert_eq!(refused["shown"], 1, "{status}");
    assert_eq!(refused["truncated"], false, "{status}");
    assert_eq!(
        refused["paths"][0]["path"], "app.py",
        "the path is the whole point of the list: {status}"
    );
    assert!(
        refused["paths"][0]["reason"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("escapes")
            || refused["paths"][0]["reason"]
                .as_str()
                .unwrap()
                .to_ascii_lowercase()
                .contains("outside"),
        "the verdict is what distinguishes a correct refusal from a broken \
         indexer: {status}"
    );

    let recovered = &gaps["pattern_recovered"];
    assert_eq!(recovered["total"], 1, "{status}");
    assert_eq!(
        recovered["paths"][0]["path"], "svc.proto",
        "a `.proto` has no linked grammar here, so its declarations are \
         recovered by pattern and it contributes no calls: {status}"
    );

    // The off direction. `lib.py` parsed, so it appears in no list — a marker
    // that names every file is worth as much as one that names none.
    let named: Vec<&str> = ["discovery_refused", "parse_failed", "pattern_recovered"]
        .iter()
        .flat_map(|key| gaps[*key]["paths"].as_array().unwrap())
        .map(|row| row["path"].as_str().unwrap())
        .collect();
    assert!(!named.contains(&"lib.py"), "{named:?}");

    // The number in `degraded_reason` and the length of the list are one
    // measurement, not two that happen to agree.
    let reason = status["degraded_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("1 refused by discovery"),
        "the prose and the inventory must describe the same corpus: {status}"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
}

/// A corpus with no holes reports empty lists, not absent ones.
///
/// `null` here would mean "not measured" and is the answer a store with no
/// generation gets. A built generation that read everything has a measurement,
/// and it is three empty lists.
#[test]
fn a_corpus_with_nothing_to_report_reports_nothing_rather_than_saying_nothing() {
    let root = fixture("clean");
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    std::fs::write(
        root.join("app.py"),
        "from lib import helper\n\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    // Skipped as `NonSource` on every walk. If that counted, no repository
    // would ever report a clean corpus again.
    std::fs::write(root.join("README.md"), "# notes\n").unwrap();

    json(&root, &["--json", "build", "."]);
    let status = json(&root, &["--json", "status"]);
    for key in ["discovery_refused", "parse_failed", "pattern_recovered"] {
        assert_eq!(status["coverage_gaps"][key]["total"], 0, "{key}: {status}");
        assert_eq!(
            status["coverage_gaps"][key]["paths"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "{key}: {status}"
        );
    }
    assert!(status["degraded_reason"].is_null(), "{status}");

    // Item 2's disclosure, on the same surface: this generation's edges carry
    // the evidence the resolver recorded rather than a reconstruction of it.
    assert_eq!(
        status["edge_resolution_source"], "stored",
        "an edge re-read from a generation this kernel wrote must not be \
         indistinguishable from one whose tier was guessed back out of its file \
         layout: {status}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// `edge_confidence_mismatches` on the CLI surface, both directions: a clean
/// generation reports 0 as a measurement, and a row whose confidence no longer
/// matches its stored kind is counted. The tamper is done through SQL because
/// that is the only way such a row can exist — every writer routes through
/// `ResolvedEdge::resolved`.
#[test]
fn status_counts_stored_edges_whose_confidence_contradicts_their_evidence() {
    let root = fixture("confidence-mismatch");
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    std::fs::write(
        root.join("app.py"),
        "from lib import helper\n\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    let (stdout, stderr) = run(&root, &["build", "."]);
    assert!(
        stderr.is_empty() || !stdout.is_empty(),
        "build failed: {stderr}"
    );

    let before = json(&root, &["--json", "status"]);
    assert_eq!(
        before["edge_resolution_source"], "stored",
        "a generation written by this kernel stores its evidence: {before}"
    );
    assert_eq!(
        before["edge_confidence_mismatches"], 0,
        "a clean generation reports zero as a measurement: {before}"
    );

    let db = devmap_extract::paths::store_path(&root);
    let changed = {
        let conn = rusqlite::Connection::open(&db).unwrap();
        // Every `Calls` edge with a stored deterministic-or-high kind is moved
        // to the floor; the count of rows touched is what status must report.
        //
        // `edge_rows`, not `generation_edges`: since v18 the latter is a view
        // over the validity ranges and is not updatable. Only the currently
        // valid rows are tampered with, which for a store with one generation
        // is all of them — a closed row belongs to a generation status is not
        // reporting on.
        conn.execute(
            "UPDATE edge_rows SET confidence = 0.2
             WHERE valid_to IS NULL
               AND edge_kind = 'Calls'
               AND resolution IN ('ImportScoped', 'SameFile', 'UniqueGlobal', 'ReceiverType')",
            [],
        )
        .unwrap()
    };
    assert!(
        changed >= 1,
        "the fixture must hold a resolved call edge to tamper with"
    );

    let after = json(&root, &["--json", "status"]);
    assert_eq!(
        after["edge_confidence_mismatches"],
        serde_json::json!(changed),
        "every tampered row must be counted: {after}"
    );

    // CLI and IPC share one owner (`Store::edge_confidence_mismatches`). IPC is
    // pinned the same way in `ipc_status_edge_confidence_mismatches_matches_sql_owner`.
    let store = Store::open_read_only(&db).unwrap();
    assert_eq!(
        after["edge_confidence_mismatches"],
        serde_json::json!(store.edge_confidence_mismatches().unwrap()),
        "CLI status must report the SQL owner, not an in-memory recount"
    );

    let _ = std::fs::remove_dir_all(&root);
}
