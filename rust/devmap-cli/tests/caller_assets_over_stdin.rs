//! The two stdin contracts DevCouncil's Go host drives: `devmap skills install
//! --library-stdin` and `devmap integrate <host> --servers-stdin`.
//!
//! Driven through the binary because that is the seam: Go passes a JSON
//! document on stdin and reads the JSON report and the exit code, and nothing
//! in a unit test sees either. The cross-process lock is here for the same
//! reason — threads in one process cannot show that a second `devmap` honours
//! it, and separate processes are how the receipt is raced in the field.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const DEVMAP: &str = env!("CARGO_BIN_EXE_devmap");

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-stdin-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Run {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Run devmap with `stdin` piped in. HOME is a scratch directory so nothing
/// reaches the user's own host configs.
fn devmap(args: &[&str], stdin: &[u8], home: &Path) -> Run {
    let mut child = Command::new(DEVMAP)
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    Run {
        status: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn library(names: &[&str]) -> Vec<u8> {
    let skills: Vec<Value> = names
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "content": format!("---\nname: {name}\ndescription: Example\n---\nBody\n"),
            })
        })
        .collect();
    serde_json::to_vec(&json!({ "skills": skills })).unwrap()
}

#[test]
fn a_library_on_stdin_installs_those_skills_and_only_those() {
    let root = scratch("library");
    let home = scratch("library-home");
    let root_arg = root.to_str().unwrap();
    let run = devmap(
        &[
            "--json",
            "skills",
            "install",
            "--project-root",
            root_arg,
            "--library-stdin",
        ],
        &library(&["alpha", "beta"]),
        &home,
    );
    assert_eq!(run.status, Some(0), "{}", run.stderr);
    let report: Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(report["written"].as_array().unwrap().len(), 6, "{report}");
    assert!(root.join(".claude/skills/alpha/SKILL.md").is_file());
    assert!(
        !root.join(".claude/skills/devmap/SKILL.md").exists(),
        "a library install also wrote DevMap's own skills"
    );

    let check = devmap(
        &[
            "--json",
            "skills",
            "install",
            "--project-root",
            root_arg,
            "--library-stdin",
            "--check",
        ],
        &library(&["alpha", "beta"]),
        &home,
    );
    assert_eq!(check.status, Some(0), "{}", check.stderr);

    let missing = devmap(
        &[
            "skills",
            "install",
            "--project-root",
            root_arg,
            "--library-stdin",
            "--check",
        ],
        &library(&["alpha", "gamma"]),
        &home,
    );
    assert_ne!(
        missing.status,
        Some(0),
        "a check with a missing skill passed"
    );
    assert!(!root.join(".claude/skills/gamma").exists(), "a check wrote");

    let malformed = devmap(
        &[
            "skills",
            "install",
            "--project-root",
            root_arg,
            "--library-stdin",
        ],
        b"{\"skills\": [{\"name\": \"x\"}]}",
        &home,
    );
    assert_ne!(
        malformed.status,
        Some(0),
        "a malformed library was accepted"
    );
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn concurrent_processes_keep_one_complete_receipt() {
    let root = scratch("contention");
    let home = scratch("contention-home");
    let root_arg = root.to_str().unwrap().to_string();
    let payload = library(&["portable"]);
    let failures: Vec<String> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..16)
            .map(|_| {
                scope.spawn(|| {
                    devmap(
                        &[
                            "skills",
                            "install",
                            "--project-root",
                            &root_arg,
                            "--library-stdin",
                        ],
                        &payload,
                        &home,
                    )
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|run| run.status != Some(0))
            .map(|run| run.stderr)
            .collect()
    });
    assert!(failures.is_empty(), "{failures:?}");
    let receipt: Value =
        serde_json::from_slice(&std::fs::read(root.join(".devcouncil-skills.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["schema"], 1);
    assert_eq!(
        receipt["files"].as_object().unwrap().len(),
        3,
        "one entry per host directory: {receipt}"
    );
    assert!(
        !root.join(".devcouncil-skills.lock").exists(),
        "a lock was left behind"
    );
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn servers_on_stdin_are_registered_reported_and_checked() {
    let root = scratch("servers");
    let home = scratch("servers-home");
    // Warp's document lives under `.devcouncil/`, and DevMap resolves its
    // state directory to `.devcouncil/` only once that exists — so on a bare
    // tree the first apply writes guides naming `.devmap/` and the next one
    // rewrites them. Creating it first keeps this test about the servers.
    std::fs::create_dir_all(root.join(".devcouncil")).unwrap();
    let root_arg = root.to_str().unwrap();
    let request = serde_json::to_vec(&json!({"servers": [{
        "name": "devcouncil",
        "command": "/opt/devcouncil/bin/devcouncil",
        "args": ["mcp"],
        "env": {"DEVCOUNCIL_PROJECT_ROOT": root_arg},
    }]}))
    .unwrap();
    let integrate = |extra: &[&str], stdin: &[u8]| {
        let mut args = vec![
            "--json",
            "integrate",
            "warp",
            "--project-root",
            root_arg,
            "--servers-stdin",
        ];
        args.extend_from_slice(extra);
        devmap(&args, stdin, &home)
    };

    let check = integrate(&["--check"], &request);
    assert_ne!(check.status, Some(0), "check passed on an empty tree");
    // The report still comes out, so a caller can say which file differs. It
    // is the first document; the CLI's error envelope follows it.
    let report: Value = serde_json::Deserializer::from_str(&check.stdout)
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(report["check_ok"], false);
    assert_eq!(report["servers"][0]["changed"], true, "{report}");

    let apply = integrate(&[], &request);
    assert_eq!(apply.status, Some(0), "{}", apply.stderr);
    let doc: Value = serde_json::from_slice(
        &std::fs::read(root.join(".devcouncil/integrations/warp-mcp.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        doc["devcouncil"]["command"],
        "/opt/devcouncil/bin/devcouncil"
    );
    assert!(
        doc.get("devmap").is_some(),
        "DevMap's own entry is missing: {doc}"
    );

    let again = integrate(&["--check"], &request);
    assert_eq!(again.status, Some(0), "{}\n{}", again.stdout, again.stderr);

    // Refused before anything is written, DevMap's half included.
    let fresh = scratch("servers-refused");
    let refused = devmap(
        &[
            "integrate",
            "warp",
            "--project-root",
            fresh.to_str().unwrap(),
            "--servers-stdin",
        ],
        br#"{"servers":[{"name":"devcouncil","command":"devcouncil"}]}"#,
        &home,
    );
    assert_ne!(refused.status, Some(0), "a relative command was accepted");
    assert_eq!(
        std::fs::read_dir(&fresh).unwrap().count(),
        0,
        "a refused request wrote"
    );
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&fresh).ok();
    std::fs::remove_dir_all(&home).ok();
}
