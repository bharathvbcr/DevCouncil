//! Shared by the tests that run the real `soak` binary against a stand-in
//! `devmap`.

#![cfg(unix)]
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soak-bin-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A corpus with one Python file and a stand-in `devmap` with `body` as its
/// script. Returns (corpus root, stand-in path, state directory).
pub fn fixture(name: &str, body: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = scratch(name);
    let root = base.join("corpus");
    fs::create_dir_all(root.join("pkg")).expect("corpus");
    fs::write(root.join("pkg/a.py"), "x = 1\n").expect("target");
    let state = base.join("state");
    fs::create_dir_all(&state).expect("state");
    let stub = base.join("devmap");
    fs::write(
        &stub,
        format!("#!/bin/sh\nSTATE='{}'\n{body}\n", state.display()),
    )
    .expect("stub");
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");
    (root, stub, state)
}

/// The `soak` binary, five cycles over `root`, driving `stub`, stdout piped.
pub fn soak(root: &Path, stub: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_soak"));
    command
        .arg(root)
        .arg("5")
        .env("DEVMAP_BIN", stub)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}
