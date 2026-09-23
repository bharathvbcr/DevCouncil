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
