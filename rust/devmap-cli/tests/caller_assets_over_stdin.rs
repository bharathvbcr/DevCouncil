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
    // Absolute on the host: `/opt/...` is rooted but not absolute on Windows.
    let command = if cfg!(windows) {
        r"C:\opt\devcouncil\bin\devcouncil"
    } else {
        "/opt/devcouncil/bin/devcouncil"
    };
    let request = serde_json::to_vec(&json!({"servers": [{
        "name": "devcouncil",
        "command": command,
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
    assert_eq!(doc["devcouncil"]["command"], command);
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

/// A document devmap will not rewrite must stop an apply before it writes
/// anything. A refused Cursor apply (JSONC `~/.cursor/mcp.json`) used to have
/// already written the guides, the skills and their receipt, and then printed
/// no receipt of any of it.
#[test]
fn a_refused_document_stops_an_apply_before_it_writes_anything() {
    let root = scratch("preflight");
    let home = scratch("preflight-home");
    std::fs::create_dir_all(home.join(".cursor")).unwrap();
    let jsonc = "// my servers\n{\"mcpServers\":{}}\n";
    std::fs::write(home.join(".cursor/mcp.json"), jsonc).unwrap();
    let root_arg = root.to_str().unwrap();

    let apply = devmap(
        &["--json", "integrate", "cursor", "--project-root", root_arg],
        b"",
        &home,
    );
    assert_ne!(apply.status, Some(0), "{}", apply.stdout);
    assert!(
        apply.stdout.contains("mcp.json") || apply.stderr.contains("mcp.json"),
        "the refusal must name the file:\n{}\n{}",
        apply.stdout,
        apply.stderr
    );
    let written: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        written.is_empty(),
        "a refused apply left files behind: {written:?}"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap(),
        jsonc,
        "the user's document was touched"
    );
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

fn servers_request(root: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({"servers": [{
        "name": "devcouncil",
        "command": "/opt/devcouncil/bin/devcouncil",
        "args": ["mcp"],
        "env": {"DEVCOUNCIL_PROJECT_ROOT": root.to_str().unwrap()},
    }]}))
    .unwrap()
}

/// `integrate <host> --remove` takes back exactly the server entries an apply
/// registered, from exactly the documents it is pointed at, and leaves every
/// other byte of the user's documents alone. Nothing could undo an apply before.
#[test]
fn remove_takes_back_what_apply_registered_and_nothing_else() {
    let root = scratch("remove");
    let home = scratch("remove-home");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    let user_config = "# my codex config\nmodel = \"o5\"   # trailing comment\n\n\
                       [mcp_servers.other]\ncommand = \"x\"   # theirs\nargs = []\n";
    std::fs::write(home.join(".codex/config.toml"), user_config).unwrap();
    let root_arg = root.to_str().unwrap();
    let request = servers_request(&root);

    let apply = devmap(
        &[
            "--json",
            "integrate",
            "codex",
            "--project-root",
            root_arg,
            "--servers-stdin",
        ],
        &request,
        &home,
    );
    assert_eq!(apply.status, Some(0), "{}", apply.stderr);
    let after_apply = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    assert!(
        after_apply.contains("[mcp_servers.devmap]"),
        "{after_apply}"
    );
    assert!(root.join(".codex/config.toml").is_file());

    let user_path = home.join(".codex/config.toml");
    let project_path = root.join(".codex/config.toml");
    let remove = |extra: &[&str]| {
        let mut args = vec![
            "--json",
            "integrate",
            "codex",
            "--project-root",
            root_arg,
            "--servers-stdin",
            "--remove",
        ];
        args.extend_from_slice(extra);
        devmap(&args, &request, &home)
    };

    // Pointed at no document it refuses: an undo with no receipt to follow
    // would have to guess, and it does not.
    let none = remove(&[]);
    assert_eq!(none.status, Some(2), "{}", none.stdout);
    assert_eq!(std::fs::read_to_string(&user_path).unwrap(), after_apply);

    // A check names what is still registered and fails.
    let check = remove(&["--check", "--only-path", user_path.to_str().unwrap()]);
    assert_ne!(check.status, Some(0), "{}", check.stdout);
    assert_eq!(std::fs::read_to_string(&user_path).unwrap(), after_apply);

    let removed = remove(&[
        "--only-path",
        user_path.to_str().unwrap(),
        "--only-path",
        project_path.to_str().unwrap(),
    ]);
    assert_eq!(removed.status, Some(0), "{}", removed.stderr);
    let report: Value = serde_json::from_str(&removed.stdout).unwrap();
    assert_eq!(report["check_ok"], true, "{report}");
    assert_eq!(
        std::fs::read_to_string(&user_path).unwrap(),
        user_config,
        "the user's own lines must come back byte for byte"
    );
    assert!(
        !project_path.exists(),
        "a document that held only our entry is deleted, not left empty"
    );

    // Idempotent: nothing left to remove.
    let again = remove(&["--check", "--only-path", user_path.to_str().unwrap()]);
    assert_eq!(again.status, Some(0), "{}\n{}", again.stdout, again.stderr);
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn remove_keeps_other_servers_and_keys_in_a_json_document() {
    let root = scratch("remove-json");
    let home = scratch("remove-json-home");
    let root_arg = root.to_str().unwrap();
    let doc = root.join(".agents/mcp_config.json");
    std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
    let theirs =
        json!({"mcpServers": {"other": {"command": "x", "args": ["serve"]}}, "theme": "dark"});
    std::fs::write(&doc, serde_json::to_string_pretty(&theirs).unwrap() + "\n").unwrap();
    let request = servers_request(&root);

    let apply = devmap(
        &[
            "--json",
            "integrate",
            "antigravity",
            "--project-root",
            root_arg,
            "--servers-stdin",
        ],
        &request,
        &home,
    );
    assert_eq!(apply.status, Some(0), "{}", apply.stderr);
    let registered: Value = serde_json::from_slice(&std::fs::read(&doc).unwrap()).unwrap();
    assert!(
        registered["mcpServers"].get("devcouncil").is_some(),
        "{registered}"
    );

    let removed = devmap(
        &[
            "--json",
            "integrate",
            "antigravity",
            "--project-root",
            root_arg,
            "--servers-stdin",
            "--remove",
            "--only-path",
            doc.to_str().unwrap(),
        ],
        &request,
        &home,
    );
    assert_eq!(removed.status, Some(0), "{}", removed.stderr);
    let left: Value = serde_json::from_slice(&std::fs::read(&doc).unwrap()).unwrap();
    assert_eq!(left, theirs, "only our entries may go");

    // A document the tool will not rewrite stops the removal untouched.
    let jsonc = "// mine\n{\"mcpServers\":{\"devmap\":{\"command\":\"d\",\"args\":[\"mcp\"]}}}\n";
    std::fs::write(&doc, jsonc).unwrap();
    let refused = devmap(
        &[
            "--json",
            "integrate",
            "antigravity",
            "--project-root",
            root_arg,
            "--remove",
            "--only-path",
            doc.to_str().unwrap(),
        ],
        b"",
        &home,
    );
    assert_ne!(refused.status, Some(0));
    assert_eq!(std::fs::read_to_string(&doc).unwrap(), jsonc);
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&home).ok();
}
