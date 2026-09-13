//! Doctor / paths / build refusals for shared MCP registration and unsafe roots.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-cli-scope-{name}-{}-{}",
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

fn run_in(root: &Path, args: &[&str], home: Option<&Path>) -> Output {
    let mut cmd = Command::new(devmap());
    cmd.args(args).current_dir(root).env_remove("DEVMAP_HOME");
    if let Some(home) = home {
        cmd.env("HOME", home);
    }
    cmd.output()
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

fn write_mcp(path: &Path, command: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        path,
        format!(
            r#"{{"mcpServers":{{"devmap":{{"type":"stdio","command":"{command}","args":["mcp"]}}}}}}"#
        ),
    )
    .unwrap();
}

#[test]
fn doctor_reports_duplicate_mcp_registrations_including_plugin_cache() {
    let home = scratch("doc-home");
    let cwd = scratch("doc-cwd");
    std::fs::create_dir_all(cwd.join(".devcouncil").join("codeintel")).unwrap();
    write_mcp(&home.join(".cursor").join("mcp.json"), "devmap");
    write_mcp(&home.join(".claude.json"), "devmap");
    write_mcp(
        &home
            .join(".claude")
            .join("plugins")
            .join("cache")
            .join("devmap-local")
            .join("devmap")
            .join("0.1.1")
            .join(".mcp.json"),
        "devmap",
    );
    let out = run_in(&cwd, &["--json", "doctor"], Some(&home));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let warning = payload
        .get("duplicate_mcp_registration_warning")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !warning.is_empty(),
        "duplicate registrations must be reported: {payload}"
    );
    assert!(
        warning.contains("mcp.json") || warning.contains("plugin"),
        "must mention the plugin-cache or mcp.json: {warning}"
    );
}

#[test]
fn doctor_warns_when_home_devcouncil_exists_without_a_store() {
    let home = scratch("doc-empty-home");
    std::fs::create_dir_all(home.join(".devcouncil")).unwrap();
    let cwd = scratch("doc-empty-cwd");
    let out = run_in(&cwd, &["--json", "doctor"], Some(&home));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let warning = payload
        .get("stray_state_warning")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        warning.contains(".devcouncil"),
        "must warn about ~/.devcouncil without a store: {payload}"
    );
}

#[test]
fn doctor_json_embeds_a_build_identifier() {
    let cwd = scratch("doc-build");
    let out = run_in(&cwd, &["--json", "doctor"], None);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let build = payload.get("build").expect("build object");
    assert!(
        build.get("git").and_then(Value::as_str).is_some()
            || build.get("id").and_then(Value::as_str).is_some(),
        "doctor --json must embed a build identifier: {payload}"
    );
}

#[test]
fn version_line_embeds_a_build_identifier() {
    let cwd = scratch("ver-build");
    let out = run_in(&cwd, &["--version"], None);
    assert!(out.status.success());
    let text = output_stdout(&out);
    assert!(
        text.contains("build "),
        "--version must embed a build identifier: {text:?}"
    );
}

fn output_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn build_from_home_refuses_instead_of_indexing() {
    let home = scratch("build-home");
    std::fs::create_dir_all(home.join(".devcouncil")).unwrap();
    std::fs::write(home.join("notes.md"), "not a repository\n").unwrap();
    let out = run_in(&home, &["--json", "build"], Some(&home));
    assert!(
        !out.status.success(),
        "devmap build in $HOME must refuse: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains(&home.display().to_string()),
        "refusal must name the resolved root: {combined}"
    );
    let store = home
        .join(".devcouncil")
        .join("codeintel")
        .join("devmap.sqlite");
    assert!(
        !store.exists(),
        "must not create a store under $HOME: {}",
        store.display()
    );
}

#[test]
fn doctor_warns_when_a_host_config_names_a_missing_binary() {
    let home = scratch("doc-missing-bin");
    let cwd = scratch("doc-missing-cwd");
    let missing = home.join("no-such-devmap-binary");
    write_mcp(&home.join(".claude.json"), &missing.display().to_string());
    let out = run_in(&cwd, &["--json", "doctor"], Some(&home));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let warning = payload
        .get("missing_binary_warning")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        !warning.is_empty(),
        "a host config pointing at a path that is not a file must not look like a clean doctor: {payload}"
    );
    assert!(
        warning.contains(&missing.display().to_string()) || warning.contains("no-such-devmap"),
        "warning must name the missing path: {warning}"
    );
    let skew = payload.get("binary_skew_warning");
    assert!(
        skew.is_some(),
        "binary_skew_warning remains a distinct field (null when versions match): {payload}"
    );
}

/// Given a fixture rather than the developer's own machine, for two reasons.
///
/// It used to run against the real `HOME` and assert that *some* listed binary
/// carried a hash — which held only because hashing was unbounded and would
/// therefore pay any price to finish. Now that the binary inventory has a
/// budget, "a hash is always present" is no longer a property of the diagnostic
/// but of how large the binaries on the machine are and how much CPU the
/// machine has to spare; on a loaded host an unoptimised 61 MiB binary does not
/// fit, and reporting that honestly is the fix, not a regression.
///
/// So the binary this test hashes is one it puts there itself, small enough
/// that it fits any budget, and the assertions are strengthened rather than
/// relaxed: *every* listed path must be absolute (it was *some*), and the bare
/// `devmap` named by the host config must resolve onto `PATH` and come back
/// with a real digest.
#[test]
fn doctor_resolves_bare_devmap_and_hashes_binaries() {
    let cwd = scratch("doc-hash");
    let home = scratch("doc-hash-home");
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let discovered = bin.join("devmap");
    std::fs::write(&discovered, b"#!/bin/sh\nexit 0\n").unwrap();
    // A bare command name is the resolution this test is named for: doctor must
    // report where it lands, never echo back "devmap".
    write_mcp(&home.join(".cursor/mcp.json"), "devmap");

    let out = Command::new(devmap())
        .args(["--json", "doctor"])
        .current_dir(&cwd)
        .env("HOME", &home)
        .env("PATH", &bin)
        .env_remove("DEVMAP_HOME")
        .output()
        .expect("devmap --json doctor");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload = one_json(&out, "doctor");
    let binaries = payload["binaries"].as_array().expect("binaries");
    assert!(
        binaries.iter().all(|b| {
            b.get("path")
                .and_then(Value::as_str)
                .is_some_and(|p| Path::new(p).is_absolute())
        }),
        "every listed binary must be a resolved absolute path: {payload}"
    );
    let resolved = discovered.canonicalize().unwrap().display().to_string();
    let row = binaries
        .iter()
        .find(|b| b.get("path").and_then(Value::as_str) == Some(resolved.as_str()))
        .unwrap_or_else(|| panic!("bare `devmap` was not resolved onto PATH: {payload}"));
    assert_eq!(
        row.get("sha256").and_then(Value::as_str).map(str::len),
        Some(64),
        "a discovered binary that fits the budget must be hashed: {row}"
    );
    assert_eq!(row["sha256_status"], "hashed", "{row}");
}
