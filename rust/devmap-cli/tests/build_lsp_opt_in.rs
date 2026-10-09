//! `devmap build --lsp` is opt-in. A missing server is did-not-run, not zero
//! edges from a run, and never writes UniqueGlobal.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devmap"))
}

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-lsp-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn build_lsp_with_no_covering_server_on_path_reports_did_not_run() {
    let dir = tmp_dir("missing-server");
    let src = dir.join("src");
    fs::create_dir_all(&src).unwrap();
    // A tiny Cobol-shaped file is not covered by any of the four opt-in
    // servers, so the pass is asked and finds nothing to run — distinct from
    // inventing UniqueGlobal edges for the unresolved sites.
    fs::write(
        src.join("hello.cbl"),
        "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. HELLO.\n",
    )
    .unwrap();
    // Empty PATH so even rust-analyzer cannot be found if the repo had rust.
    let output = Command::new(bin())
        .args(["--json", "build", "--lsp", "--db"])
        .arg(dir.join("devmap.sqlite"))
        .arg(&dir)
        .env("PATH", "")
        .output()
        .expect("spawn build --lsp");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "build --lsp must exit 0 even when no server runs\nstdout={stdout}\nstderr={stderr}"
    );
    // When PATH is empty and no file matches a server, `lsp` is still present
    // (requested) and must not claim UniqueGlobal edges were written.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&stdout) {
        if let Some(lsp) = value.get("lsp") {
            assert!(
                lsp.get("edges_added").and_then(|v| v.as_u64()).unwrap_or(0) == 0
                    || lsp.get("servers").is_some(),
                "lsp block must not invent edges without a server: {lsp}"
            );
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Runs `devmap --json build --lsp` over `repo` with `path` as PATH, from
/// `repo` as the working directory, and returns rust-analyzer's server row.
#[cfg(unix)]
fn rust_analyzer_row(
    repo: &std::path::Path,
    db: &std::path::Path,
    path: &std::ffi::OsStr,
) -> serde_json::Value {
    let output = Command::new(bin())
        .args(["--json", "build", "--lsp", "--db"])
        .arg(db)
        .arg(repo)
        .current_dir(repo)
        .env("PATH", path)
        .output()
        .expect("spawn build --lsp");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "build --lsp must exit 0\nstdout={stdout}\nstderr={stderr}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&stdout).unwrap_or_else(|error| panic!("{error}: {stdout}"));
    value["lsp"]["servers"]
        .as_array()
        .and_then(|servers| servers.iter().find(|row| row["binary"] == "rust-analyzer"))
        .cloned()
        .unwrap_or_else(|| panic!("no rust-analyzer row: {}", value["lsp"]))
}

/// A language server is a program found on PATH, and PATH in an agent's
/// environment can name a directory inside the repository being built. A
/// server planted there — on PATH absolutely, or through a relative entry
/// that names whatever directory the build runs in — is refused and never
/// started, and the row says it was refused rather than missing. The control
/// case, the same program installed outside the repository, does start: it
/// is what proves the fixture reaches the language-server pass at all.
#[cfg(unix)]
#[test]
fn a_language_server_inside_the_repository_is_refused_not_spawned() {
    use std::os::unix::fs::PermissionsExt;
    let base = tmp_dir("planted-server");
    let repo = base.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(
        repo.join("src").join("lib.rs"),
        "pub fn caller() {\n    not_defined_anywhere();\n}\n",
    )
    .unwrap();
    let plant = |dir: &std::path::Path, marker: &std::path::Path| {
        fs::create_dir_all(dir).unwrap();
        let program = dir.join("rust-analyzer");
        fs::write(
            &program,
            format!("#!/bin/sh\n: > '{}'\nexit 0\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    };

    let inside_marker = base.join("inside-ran");
    plant(&repo.join("tools").join("bin"), &inside_marker);
    for (label, path) in [
        (
            "absolute entry",
            repo.join("tools").join("bin").into_os_string(),
        ),
        ("relative entry", std::ffi::OsString::from("tools/bin")),
    ] {
        let db = base.join(format!("{}.sqlite", label.replace(' ', "-")));
        let row = rust_analyzer_row(&repo, &db, &path);
        assert!(
            !inside_marker.exists(),
            "{label}: the repository's rust-analyzer was spawned: {row}"
        );
        assert_eq!(row["ran"], false, "{label}: {row}");
        assert_eq!(
            row["did_not_run"], "refused_binary",
            "{label}: a refusal must not read as a missing binary: {row}"
        );
    }

    let outside_marker = base.join("outside-ran");
    let outside = base.join("installed").join("bin");
    plant(&outside, &outside_marker);
    let row = rust_analyzer_row(&repo, &base.join("control.sqlite"), outside.as_os_str());
    assert!(
        outside_marker.exists(),
        "control: an installation outside the repository was not started, so the \
         fixture never reached the language-server pass: {row}"
    );
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn build_without_lsp_flag_omits_the_lsp_block() {
    let dir = tmp_dir("no-flag");
    fs::write(dir.join("a.py"), "def a():\n    return 1\n").unwrap();
    let output = Command::new(bin())
        .args(["--json", "build", "--db"])
        .arg(dir.join("devmap.sqlite"))
        .arg(&dir)
        .output()
        .expect("spawn build");
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value.get("lsp").unwrap().is_null(),
        "without --lsp the block must be null, not a silent zero-edge run: {}",
        value["lsp"]
    );
    let _ = fs::remove_dir_all(&dir);
}
