//! `devmap doctor` carries the edges check the Go `dcmap doctor` was retired
//! without: stored edges whose confidence contradicts their resolution kind.
//!
//! Three readings, never two. Zero is a measurement and passes; a count above
//! zero warns; and a store that could not be measured reports `null` with a
//! warning saying the check is unknown — an unmeasured count must not read as a
//! clean one. The count comes from `Store::edge_confidence_mismatches`, the same
//! SQL owner `devmap status` reports, and doctor must still create no store.

use std::path::{Path, PathBuf};
use std::process::Command;

use devmap_store::Store;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "devmap {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("devmap {args:?} stdout is not JSON ({error}): {stdout}"))
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-doctor-edges-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

#[test]
fn doctor_counts_edges_whose_confidence_contradicts_their_evidence() {
    let root = fixture("tamper");
    std::fs::write(root.join("lib.py"), "def helper():\n    return 42\n").unwrap();
    std::fs::write(
        root.join("app.py"),
        "from lib import helper\n\n\ndef main():\n    return helper()\n",
    )
    .unwrap();
    let build = Command::new(devmap())
        .args(["build", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let clean = json(&root, &["--json", "doctor"]);
    assert_eq!(
        clean["edge_confidence_mismatches"], 0,
        "a clean generation is a measured zero: {clean}"
    );
    assert!(
        clean["edge_confidence_warning"].is_null(),
        "zero is the passing reading and carries no warning: {clean}"
    );

    let db = devmap_extract::paths::store_path(&root);
    // `edge_rows`, not the `generation_edges` view, which is not updatable.
    // Every writer routes through `ResolvedEdge::resolved`, so SQL is the only
    // way such a row can exist.
    let changed = rusqlite::Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE edge_rows SET confidence = 0.2
             WHERE valid_to IS NULL
               AND edge_kind = 'Calls'
               AND resolution IN ('ImportScoped', 'SameFile', 'UniqueGlobal', 'ReceiverType')",
            [],
        )
        .unwrap();
    assert!(changed >= 1, "the fixture must hold a resolved call edge");

    let tampered = json(&root, &["--json", "doctor"]);
    assert_eq!(
        tampered["edge_confidence_mismatches"],
        serde_json::json!(changed),
        "every contradicted row is counted: {tampered}"
    );
    let warning = tampered["edge_confidence_warning"]
        .as_str()
        .unwrap_or_default();
    assert!(
        warning.contains(&changed.to_string()),
        "a count above zero warns and names the count: {tampered}"
    );
    assert_eq!(
        tampered["edge_confidence_mismatches"],
        serde_json::json!(Store::open_read_only(&db)
            .unwrap()
            .edge_confidence_mismatches()
            .unwrap()),
        "doctor reports the SQL owner `status` uses, not a recount"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn doctor_without_a_store_reports_the_edges_check_as_unknown_and_creates_nothing() {
    let root = fixture("no-store");
    let db = devmap_extract::paths::store_path(&root);

    let payload = json(&root, &["--json", "doctor"]);
    assert!(
        payload["edge_confidence_mismatches"].is_null(),
        "nothing was measured, so the count is null, never 0: {payload}"
    );
    let warning = payload["edge_confidence_warning"]
        .as_str()
        .unwrap_or_default();
    assert!(
        warning.contains("unknown"),
        "an unmeasured check must say it is unknown rather than pass silently: {payload}"
    );
    assert!(
        !db.exists(),
        "doctor is a probe and must not create a store at {}",
        db.display()
    );

    let _ = std::fs::remove_dir_all(&root);
}
