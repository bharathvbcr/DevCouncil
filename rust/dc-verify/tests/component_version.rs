//! `dcverify` can be asked which build it is.
//!
//! It could not before: `--version` was rejected as an unknown flag, so a
//! deployed verifier's version was not observable at all. The package version
//! said 0.2.3 the whole time — the program simply could not be asked, which is
//! the same failure mode this binary's own contract exists to prevent: a check
//! that could not run must not be indistinguishable from one that ran.
//!
//! The expected string is built from `CARGO_PKG_VERSION` rather than spelled
//! out, so a release does not have to edit this file. That the crate takes its
//! version from the workspace rather than declaring its own is a separate
//! property, checked once for every crate by `scripts/check-release.mjs`.

use std::process::Command;

#[test]
fn version_flag_reports_this_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .arg("--version")
        .output()
        .expect("dcverify must run");

    assert!(
        output.status.success(),
        "--version must exit 0, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout must be UTF-8");
    let want = format!(
        "{{\"ok\":true,\"component\":\"dc-verify\",\"version\":\"{}\"}}",
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        stdout.trim(),
        want,
        "every outcome on this boundary is one JSON object, --version included"
    );
}

/// Asking for the version must not be mistaken for an empty verification.
///
/// This binary reads a diff on stdin and, given none, legitimately answers
/// `{"ok":true,"files":0,...,"findings":[]}`. A `--version` that fell through
/// to that path would exit 0 with a plausible-looking verdict, so the two
/// answers are asserted to be different documents rather than merely both
/// successful.
#[test]
fn version_flag_is_not_an_empty_verdict() {
    let output = Command::new(env!("CARGO_BIN_EXE_dcverify"))
        .arg("--version")
        .output()
        .expect("dcverify must run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\"findings\""),
        "--version answered with a verification result: {stdout}"
    );
    assert!(
        stdout.contains("\"version\""),
        "--version must answer with the build identity: {stdout}"
    );
}
