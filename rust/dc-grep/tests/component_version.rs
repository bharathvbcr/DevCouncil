//! `dcgrep` can be asked which build it is.
//!
//! It could not before: `--version` was rejected as an unknown command. The
//! `health` command already answered with an identity — searcher, schema
//! version, engines — but never with the product version, so a deployed
//! searcher's build was not observable. The package version said 0.2.3 the
//! whole time; the program simply could not be asked.
//!
//! The expected string is built from `CARGO_PKG_VERSION` rather than spelled
//! out, so a release does not have to edit this file. That the crate takes its
//! version from the workspace rather than declaring its own is a separate
//! property, checked once for every crate by `scripts/check-release.mjs`.

use std::process::Command;

#[test]
fn version_flag_reports_this_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcgrep"))
        .arg("--version")
        .output()
        .expect("dcgrep must run");

    assert!(
        output.status.success(),
        "--version must exit 0, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
    let want = format!(
        "{{\"ok\":true,\"component\":\"dc-grep\",\"version\":\"{}\"}}",
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        stdout.trim(),
        want,
        "every outcome on this boundary is one JSON object, --version included"
    );
}

/// `--version` must not read a request from stdin.
///
/// With no leading command this binary searches, and searching blocks on stdin
/// until EOF. A `--version` that fell through to that arm would hang for any
/// caller holding the pipe open — an installer probing what it placed on disk
/// being exactly that caller. Asserted with stdin left inheritable and a
/// closed-pipe-free path: the test would deadlock rather than fail if the arm
/// were missing, so the bound is the harness timeout and the assertion below
/// is what distinguishes a right answer from a lucky one.
#[test]
fn version_flag_does_not_wait_for_a_request() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcgrep"))
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("dcgrep must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("no request on stdin"),
        "--version fell through to the search path: {stdout}"
    );
    assert!(
        stdout.contains("\"version\""),
        "--version must answer with the build identity: {stdout}"
    );
}

/// The unknown-command help must name every command it accepts.
///
/// The list is hand-maintained beside the match, so a new arm that is not
/// added to it leaves callers told the command does not exist.
#[test]
fn the_command_list_names_the_version_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcgrep"))
        .arg("no-such-command")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("dcgrep must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--version"),
        "the command list omits an accepted command: {stdout}"
    );
}
