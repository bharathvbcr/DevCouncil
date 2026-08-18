use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Unique per call, not merely per instant: `SystemTime` ticks every 1 us here,
/// so same-microsecond callers would otherwise share one fixture directory.
fn temp_root() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-persisted-cli-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    root
}

#[test]
fn query_commands_do_not_rebuild_or_require_sources() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    fs::write(
        root.join("src/target.py"),
        "def durable_symbol():\n    return 1\n",
    )
    .expect("write fixture");
    fs::write(
        root.join("src/caller.py"),
        "from target import durable_symbol\n\ndef caller():\n    return durable_symbol()\n",
    )
    .expect("write caller fixture");

    let binary = env!("CARGO_BIN_EXE_devmap");
    let build = Command::new(binary)
        .args(["--json", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .output()
        .expect("run build");
    assert!(
        build.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    fs::remove_dir_all(root.join("src")).expect("remove source tree after persistence");
    let query = Command::new(binary)
        .current_dir(&root)
        .args(["--json", "--db"])
        .arg(&db)
        .args(["search", "durable_symbol"])
        .output()
        .expect("run persisted query");
    assert!(
        query.status.success(),
        "query failed: stdout={} stderr={}",
        String::from_utf8_lossy(&query.stdout),
        String::from_utf8_lossy(&query.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&query.stdout).expect("query returns JSON");
    assert_eq!(payload["total"], 1);
    assert_eq!(payload["items"][0]["symbol_name"], "durable_symbol");

    for (command, expected_available) in [
        (vec!["deps", "src/caller.py"], true),
        (vec!["impact", "src/target.py"], true),
        (vec!["trace", "src/caller.py"], true),
        (vec!["dead"], true),
        (vec!["snapshots", "src/target.py"], true),
    ] {
        let output = Command::new(binary)
            .current_dir(&root)
            .args(["--json", "--db"])
            .arg(&db)
            .args(&command)
            .output()
            .expect("run persisted command");
        assert!(
            output.status.success(),
            "persisted {:?} failed: stdout={} stderr={}",
            command,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("command returns JSON");
        if expected_available {
            assert_eq!(value["resolution"], "Available", "command={command:?}");
        }
    }

    let manifest_path = root.join("manifest.json");
    let manifest = Command::new(binary)
        .current_dir(&root)
        .args(["--json", "--db"])
        .arg(&db)
        .arg("manifest")
        .arg(&root)
        .arg("--output")
        .arg(&manifest_path)
        .output()
        .expect("run persisted manifest");
    assert!(
        manifest.status.success(),
        "manifest failed: {}",
        String::from_utf8_lossy(&manifest.stderr)
    );
    assert!(manifest_path.is_file());

    let scoped_trace = Command::new(binary)
        .current_dir(&root)
        .args(["--json", "--db"])
        .arg(&db)
        .args(["trace", "src/caller.py", "durable_symbol"])
        .output()
        .expect("run scoped trace");
    assert!(
        scoped_trace.status.success(),
        "scoped trace failed: stdout={} stderr={}",
        String::from_utf8_lossy(&scoped_trace.stdout),
        String::from_utf8_lossy(&scoped_trace.stderr)
    );
    let scoped: serde_json::Value =
        serde_json::from_slice(&scoped_trace.stdout).expect("scoped trace returns JSON");
    assert_eq!(scoped["resolution"], "Available");
    assert_eq!(scoped["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(scoped["items"][0]["source_file"], "src/caller.py");
    // Edge endpoints are graph identities, not bare words, so a trace item can
    // be joined back to the node it names.
    assert_eq!(
        scoped["items"][0]["target_symbol"],
        "src/target.py::durable_symbol"
    );

    fs::remove_dir_all(&root).expect("remove fixture tree");
}
