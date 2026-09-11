//! Phase 1 CLI surfaces: status rebuild_required, doctor binaries, mcp --print-config.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-p1-cli-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|e| panic!("devmap {args:?}: {e}"))
}

fn one_json(output: &Output, what: &str) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or_else(|| {
        panic!(
            "{what}: empty stdout\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    serde_json::from_str(line).unwrap_or_else(|e| panic!("{what}: {e}: {line}"))
}

#[test]
fn status_json_carries_rebuild_required_distinct_from_is_fresh() {
    let root = scratch("status-rebuild");
    std::fs::write(root.join("a.py"), "def a():\n    return 1\n").unwrap();
    let build = run(&root, &["--json", "build", "."]);
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let status = one_json(&run(&root, &["--json", "status"]), "status");
    assert_eq!(status["rebuild_required"], false, "{status}");
    assert!(status["rebuild_reason"].is_null(), "{status}");
    assert_eq!(status["is_fresh"], true, "{status}");
}

#[test]
fn doctor_json_lists_binaries() {
    let root = scratch("doctor-bins");
    let out = run(&root, &["--json", "doctor"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let bins = payload["binaries"]
        .as_array()
        .unwrap_or_else(|| panic!("binaries missing: {payload}"));
    assert!(!bins.is_empty(), "{payload}");
    assert!(payload.get("binary_skew_warning").is_some(), "{payload}");
}

#[test]
fn mcp_print_config_omits_db_by_default_and_uses_absolute_command() {
    let root = scratch("mcp-print");
    let out = run(&root, &["--json", "mcp", "--print-config"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "mcp --print-config");
    let args = payload["mcpServers"]["devmap"]["args"]
        .as_array()
        .expect("args");
    assert_eq!(args, &vec![json!("mcp")], "{payload}");
    let command = payload["mcpServers"]["devmap"]["command"]
        .as_str()
        .expect("command");
    assert!(
        Path::new(command).is_absolute(),
        "must write an absolute path: {command}"
    );
}

#[test]
fn mcp_print_config_keeps_explicit_db_override() {
    let root = scratch("mcp-print-db");
    let db = root.join("custom.sqlite");
    let out = run(
        &root,
        &[
            "--json",
            "--db",
            db.to_str().unwrap(),
            "mcp",
            "--print-config",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "mcp --print-config with --db");
    let args = payload["mcpServers"]["devmap"]["args"]
        .as_array()
        .expect("args");
    assert_eq!(args[0], "--db", "{payload}");
    assert_eq!(args[2], "mcp", "{payload}");
}
