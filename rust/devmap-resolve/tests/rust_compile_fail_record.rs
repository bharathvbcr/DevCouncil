//! Runs `rust/testdata/rust-compile-fail/check.sh`, the record of the rustc
//! facts the Rust macro-silence rules rely on (which names a glob import can
//! and cannot rebind, by prelude configuration). The script compiles small
//! crates with plain `rustc` and prints PASS or FAIL per case, so it needs a
//! toolchain and takes a few seconds: `#[ignore]`d, and run by
//! `.github/workflows/ignored-tests.yml`:
//!
//! ```text
//! cargo test --locked -p devmap-resolve --test it -- rust_compile_fail_record:: --ignored --nocapture
//! ```
//!
//! A FAIL means rustc no longer agrees with the record, so the table in
//! `rust_site_scope.rs` (`macro_providers`, `prelude_binds`) must be re-read
//! against the compiler before it is trusted again.

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore = "needs rustc; run by ignored-tests.yml"]
fn rustc_still_agrees_with_the_compile_fail_record() {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata/rust-compile-fail/check.sh")
        .canonicalize()
        .expect("check.sh exists");
    let output = Command::new("sh")
        .arg(&script)
        .output()
        .expect("sh runs the record");
    let text = String::from_utf8_lossy(&output.stdout);
    let passes = text
        .lines()
        .filter(|line| line.starts_with("PASS "))
        .count();
    let fails: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("FAIL "))
        .collect();
    println!("{}", text.lines().next().unwrap_or("rustc: unknown"));
    println!("{passes} PASS, {} FAIL", fails.len());
    assert!(
        output.status.success() && fails.is_empty(),
        "exit {:?}; FAIL lines: {fails:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        passes >= 77,
        "only {passes} cases ran: a case was dropped from the record"
    );
}
