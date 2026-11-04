//! `devmap agents install --agents-stdin` through the binary: the JSON report
//! Go reads, including `check_ok`, and a dry-run that writes nothing.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const DEVMAP: &str = env!("CARGO_BIN_EXE_devmap");

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-agents-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The report is the first JSON value. A failing `--check` prints the error
/// envelope after it, which is what the Go client skips by decoding once.
fn first_report(stdout: &str) -> Value {
    let line = stdout.lines().next().unwrap_or(stdout);
    serde_json::from_str(line).unwrap_or_else(|err| panic!("{err}: {stdout}"))
}

fn document() -> Vec<u8> {
    let agents = [
        "devcouncil",
        "devcouncil-implementer",
        "devcouncil-reviewer",
        "devcouncil-verifier",
    ]
    .into_iter()
    .map(|name| {
        json!({
            "name": name,
            "content": format!("---\nname: {name}\ndescription: d\n---\n{name}\n"),
        })
    })
    .collect::<Vec<_>>();
    serde_json::to_vec(&json!({
        "agents": agents,
        "rules": [{
            "name": "devcouncil",
            "content": "---\ndescription: d\ntrigger: always_on\n---\n# DevCouncil\n",
        }],
    }))
    .unwrap()
}

fn devmap(root: &std::path::Path, extra: &[&str], stdin: &[u8]) -> (Option<i32>, String, String) {
    let mut child = Command::new(DEVMAP)
        .arg("--json")
        .arg("agents")
        .arg("install")
        .arg("--project-root")
        .arg(root)
        .arg("--agents-stdin")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn the_json_report_has_check_ok_and_dry_run_writes_nothing() {
    let root = scratch("cli");
    let (status, stdout, stderr) = devmap(&root, &["--dry-run"], &document());
    assert_eq!(status, Some(0), "{stderr}");
    let report = first_report(&stdout);
    assert_eq!(report["check_ok"], false, "{report}");
    assert_eq!(report["differing"].as_array().unwrap().len(), 5, "{report}");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert!(!root.join(".devcouncil-agents.json").exists());

    let (status, stdout, stderr) = devmap(&root, &["--check"], &document());
    assert_ne!(
        status,
        Some(0),
        "a missing file reported as current: {stdout}"
    );
    let report = first_report(&stdout);
    assert_eq!(report["check_ok"], false, "{report} {stderr}");
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0, "{stderr}");

    let (status, stdout, stderr) = devmap(&root, &[], &document());
    assert_eq!(status, Some(0), "{stderr}");
    let report = first_report(&stdout);
    assert_eq!(report["check_ok"], true, "{report}");
    assert!(root.join(".agents/agents/devcouncil/agent.md").is_file());
    assert!(root.join(".agents/rules/devcouncil.md").is_file());
    assert!(root.join(".devcouncil-agents.json").is_file());

    let (status, stdout, stderr) = devmap(&root, &["--check"], &document());
    assert_eq!(status, Some(0), "{stderr}{stdout}");
    let report = first_report(&stdout);
    assert_eq!(report["check_ok"], true, "{report}");
    let _ = std::fs::remove_dir_all(&root);
}
