//! A one-shot command whose reader goes away must end quietly.
//!
//! `devmap export -o - | head` is the ordinary Unix shape for looking at a
//! large answer, and every other CLI on the machine (`git`, `sqlite3`, `rg`)
//! ends silently when `head` closes the pipe. This kernel instead printed a
//! panic — `failed printing to stdout: Broken pipe (os error 32)` with a
//! backtrace hint — because Rust's runtime ignores SIGPIPE at startup so that
//! `println!` sees `EPIPE` as an error, and `println!` answers an error with a
//! panic. Observed on 2026-09-06 against this repository's own store.
//!
//! The check reads a few bytes of a multi-megabyte export, closes its end of
//! the pipe, and requires that stderr carries no panic. Whether the process
//! then reports a signal or a status is the platform's business; a panic is
//! not.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-closed-pipe-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// A corpus whose GraphML export is comfortably larger than a pipe buffer
/// (64 KiB on Linux and macOS), so the writer is still writing when the
/// reader has gone.
fn write_corpus(root: &Path) {
    let pkg = root.join("pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    for file in 0..120 {
        let mut body = String::new();
        for function in 0..25 {
            body.push_str(&format!(
                "def function_{file}_{function}(value):\n    return helper_{file}(value)\n\n"
            ));
        }
        body.push_str(&format!("def helper_{file}(value):\n    return value\n"));
        std::fs::write(pkg.join(format!("module_{file}.py")), body).unwrap();
    }
}

#[test]
fn a_closed_stdout_pipe_ends_the_export_without_a_panic() {
    let root = scratch("export");
    write_corpus(&root);
    let db = root.join("store.sqlite");
    let db = db.to_string_lossy().into_owned();
    let root_s = root.to_string_lossy().into_owned();

    let build = Command::new(devmap())
        .args(["--db", &db, "--progress", "never", "build", &root_s])
        .output()
        .expect("spawn devmap build");
    assert!(
        build.status.success(),
        "the fixture build must succeed:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let mut child = Command::new(devmap())
        .args([
            "--db",
            &db,
            "--progress",
            "never",
            "export",
            &root_s,
            "-o",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn devmap export");
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 64];
    let read = stdout
        .read(&mut first)
        .expect("read the head of the export");
    assert!(
        read > 0,
        "the export must start streaming before the pipe closes"
    );
    // `head` has seen enough.
    drop(stdout);

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .expect("read stderr");
    let status = child.wait().expect("wait for devmap export");
    std::io::stdout().flush().ok();
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        !stderr.contains("panicked") && !stderr.contains("Broken pipe"),
        "a reader that closed the pipe must not be answered with a panic; \
         exit {status:?}, stderr:\n{stderr}"
    );
}
