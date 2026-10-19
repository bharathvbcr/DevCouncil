//! rustlabel on the committed Rust accuracy fixture, held equal to
//! `rust/testdata/accuracy/rust.labels.json`. It needs rust-analyzer and the
//! `rust-src` component, so it is `#[ignore]`d: a default run reports it as
//! not run, never as passed. Asked to run (`-- --ignored`) without either, it
//! fails; an ignored test that was requested and could not execute must not
//! pass.
//!
//! `RUSTLABEL_UPDATE=1` rewrites the committed file.

use std::path::{Path, PathBuf};

use rustlabel::server::{label_checkout, Prerequisites, ServerConfig};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/accuracy/rust")
}

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/accuracy/rust.labels.json")
}

/// The fixture copied out of the repository: rust-analyzer's `cargo metadata`
/// writes a `Cargo.lock` the fixture does not carry, and the tree must stay
/// clean.
fn scratch_copy() -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }
    let dir = std::env::temp_dir().join(format!("rustlabel-golden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy(&fixture(), &dir);
    dir
}

#[test]
#[ignore = "needs rust-analyzer and rust-src: cargo test -p devmap-rustlabel --test it golden -- --ignored"]
fn the_fixture_labels_match_the_committed_file() {
    let dir = scratch_copy();
    // The copy runs under whatever toolchain rustup picks outside this
    // repository, so the pinned one is carried over with it.
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../rust-toolchain.toml"),
        dir.join("rust-toolchain.toml"),
    )
    .unwrap();
    if let Err(missing) = Prerequisites::check_in(&dir) {
        let _ = std::fs::remove_dir_all(&dir);
        panic!(
            "NOT RUN: {missing}. The golden test was asked to run and could not, which is not a pass."
        );
    }
    let outcome =
        label_checkout(&dir, &ServerConfig::for_tests()).unwrap_or_else(|e| panic!("{e}"));
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("rustlabel golden report: {:#?}", outcome.report);
    assert!(
        outcome.report.session_failure.is_none(),
        "{:?}",
        outcome.report
    );
    let got = outcome.labels.encode().unwrap();
    if std::env::var_os("RUSTLABEL_UPDATE").is_some() {
        std::fs::write(golden(), &got).unwrap();
        return;
    }
    let want = std::fs::read(golden()).unwrap_or_else(|e| panic!("{}: {e}", golden().display()));
    assert!(
        got == want,
        "{} is not the labeller's output; regenerate with RUSTLABEL_UPDATE=1 cargo test -p devmap-rustlabel --test it golden -- --ignored (and re-run the scorer test in devmap-analyze)",
        golden().display()
    );
}
