//! `dcstore` can be asked which build it is.
//!
//! It could not before: `--version` was an unknown flag, so the one question
//! an operator asks a deployed helper had no answer, and a `dcstore` three
//! releases old was indistinguishable from the current one at the boundary the
//! Go host actually talks to. The package version said 0.2.3 the whole time —
//! the program simply could not be asked.
//!
//! The expected string is built from `CARGO_PKG_VERSION` rather than spelled
//! out, so a release does not have to edit this file; a literal here would be
//! one more place to forget, which is the defect this test exists to prevent.
//! That the crate takes its version from the workspace rather than declaring
//! its own is a separate property, checked once for every crate by
//! `scripts/check-release.mjs`.

use std::process::Command;

#[test]
fn version_flag_reports_this_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcstore"))
        .arg("--version")
        .output()
        .expect("dcstore must run");

    assert!(
        output.status.success(),
        "--version must exit 0, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
    let want = format!(
        "{{\"ok\":true,\"component\":\"dc-store\",\"version\":\"{}\"}}",
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        stdout.trim(),
        want,
        "every outcome on this boundary is one JSON object, --version included"
    );
}

/// Asking for the version must not need a database.
///
/// `run` requires `--db` before it will do anything, and a version check that
/// only worked once a store had been opened would be useless to the caller
/// that needs it most: an installer confirming what it just placed on disk,
/// with no repository in hand.
#[test]
fn version_flag_needs_no_database() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcstore"))
        .arg("--version")
        .current_dir(std::env::temp_dir())
        .output()
        .expect("dcstore must run");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("--db is required"),
        "version is build identity, not a store query: {stdout}"
    );
}
