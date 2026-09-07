//! The four read-only surfaces that answer from the graph's nodes and edges —
//! `cypher`, `routes`, `shape-check`, `api-impact` — used to build the whole
//! export artifact in memory first, `git log` for the churn panel included.
//! Measured on this repository's store (release binary, n=7): `cypher` 643 ms
//! with git on PATH, 522 ms without; `search` 8 ms.
//!
//! A `git` shim on PATH records every invocation. The read arms must leave it
//! untouched; `export`, which writes the panel, must still reach it — that half
//! is the proof the shim was consulted at all.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-nogit-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_corpus(root: &Path) {
    let app = root.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("routes.py"),
        "from flask import Flask\napp = Flask(__name__)\n\n\n@app.route(\"/x\", methods=[\"GET\"])\ndef x():\n    return helper()\n\n\ndef helper():\n    return \"x\"\n",
    )
    .unwrap();
    std::fs::write(
        app.join("other.py"),
        "from app.routes import helper\n\n\ndef other():\n    return helper()\n",
    )
    .unwrap();
}

/// A `git` that records its arguments and answers nothing.
fn git_shim(dir: &Path, log: &Path) -> PathBuf {
    let bin = dir.join("shim-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("git");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            log.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    bin
}

fn run(db: &Path, root: &Path, shim: &Path, args: &[&str]) -> String {
    let path = format!(
        "{}:{}",
        shim.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(devmap())
        .env("PATH", path)
        .arg("--json")
        .arg("--db")
        .arg(db)
        .args(args)
        .current_dir(root)
        .output()
        .expect("devmap runs");
    assert!(
        out.status.success(),
        "{args:?} must answer: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The `git log` invocations the shim saw. The runner spells every call
/// `-C <root> <subcommand> …`, so the subcommand is matched as a token, not
/// as a prefix — a prefix match sees nothing and passes vacuously.
fn git_log_calls(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.split_whitespace().any(|token| token == "log"))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_read_arms_never_run_git_log_and_export_still_does() {
    let dir = scratch("read");
    let root = dir.join("repo");
    write_corpus(&root);
    let db = dir.join("devmap.sqlite");
    let log = dir.join("git-calls.log");
    let shim = git_shim(&dir, &log);

    let build = Command::new(devmap())
        .args(["--json", "--db"])
        .arg(&db)
        .args(["build", "--progress", "never"])
        .arg(&root)
        .output()
        .expect("devmap builds");
    assert!(
        build.status.success(),
        "the fixture must build: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let root_s = root.to_string_lossy().into_owned();
    let read_arms: &[&[&str]] = &[
        &["cypher", "MATCH (a) RETURN a.name LIMIT 5"],
        &["routes", &root_s],
        &["shape-check", &root_s],
        &["api-impact", "GET /x", &root_s],
    ];
    for args in read_arms {
        let _ = std::fs::remove_file(&log);
        let answer = run(&db, &root, &shim, args);
        assert!(
            !answer.contains("\"error\""),
            "{args:?} must answer from the store: {answer}"
        );
        let calls = git_log_calls(&log);
        assert!(
            calls.is_empty(),
            "{args:?} reads nodes and edges and must not run git log: {calls:?}"
        );
    }

    let _ = std::fs::remove_file(&log);
    let export = run(&db, &root, &shim, &["export", "-o", "-"]);
    assert!(
        export.contains("\"nodes\""),
        "export writes the artifact: {}",
        &export[..export.len().min(200)]
    );
    let calls = git_log_calls(&log);
    assert_eq!(
        calls.len(),
        1,
        "export writes the churn panel and must reach the shim exactly once: {calls:?}"
    );
}
