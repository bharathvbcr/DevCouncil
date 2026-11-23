//! `devmap build --lsp` anchors every path it hands a language server.
//!
//! The pass has no scratch directory of its own: the server runs with the build
//! root as its working directory and is told that root as `rootUri`. Both must
//! be the absolute root `root_parser` anchored at parse, and must not depend on
//! the directory `devmap` was started in. A stub `rust-analyzer` on PATH records
//! its working directory and the `initialize` frame it was sent.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-lsp-anchor-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

#[test]
fn a_relative_root_and_db_reach_the_server_as_the_one_absolute_root() {
    let base = scratch("rel");
    let repo = base.join("repo");
    let elsewhere = base.join("elsewhere");
    let bin = base.join("bin");
    let record = base.join("record");
    for dir in [&repo, &elsewhere, &bin, &record] {
        fs::create_dir_all(dir).unwrap();
    }
    fs::write(
        repo.join("lib.rs"),
        "use other::thing;\nfn a(x: Foo) { x.frob(); thing(); }\n",
    )
    .unwrap();

    // The unresolved calls above are what ask the pass to start a server.
    // Reads the Content-Length header and the initialize body, records them,
    // then exits: the pass reports did-not-run, which is all this test needs.
    let stub = bin.join("rust-analyzer");
    fs::write(
        &stub,
        "#!/bin/sh\npwd -P > \"$RECORD/cwd\"\nread -r header\nn=$(printf %s \"$header\" | tr -dc 0-9)\nread -r blank\nhead -c \"$n\" > \"$RECORD/init\"\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    let path = format!("{}:/usr/bin:/bin", bin.display());
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args([
            "--json",
            "--progress",
            "never",
            "build",
            "--lsp",
            "--db",
            "../store.sqlite",
            "../repo",
        ])
        .current_dir(&elsewhere)
        .env("PATH", path)
        .env("RECORD", &record)
        .env("DEVMAP_AUTOSPAWN", "0")
        .env_remove("DEVMAP_HOME")
        .output()
        .expect("spawn build --lsp");
    assert!(
        output.status.success(),
        "build --lsp failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let cwd = fs::read_to_string(record.join("cwd")).expect("the stub server ran");
    assert_eq!(
        Path::new(cwd.trim()),
        repo,
        "server cwd is the anchored build root"
    );
    let init = fs::read_to_string(record.join("init")).expect("the server received initialize");
    let root_uri = format!("file://{}", repo.display());
    assert!(
        init.contains(&root_uri),
        "initialize must name the anchored root {root_uri}: {init}"
    );
    assert!(
        base.join("store.sqlite").exists(),
        "the relative --db landed beside the invocation directory's parent"
    );
    assert!(
        !elsewhere.join("store.sqlite").exists(),
        "no store under the invocation directory"
    );
}
