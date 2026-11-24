//! Embed a build identifier: git hash, dirty flag, and UTC time.
//!
//! `--version` and `doctor --json` must name the binary that is running, not
//! just the crate version string. Two installs with the same version and
//! different git hashes is the skew doctor used to miss.

mod build_git;

use build_git::{bounded_git, dirty_state, GIT_BOUND};
use std::ffi::OsStr;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let repo = Path::new(&manifest).join("../..");
    println!(
        "cargo:rerun-if-changed={}",
        repo.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        repo.join(".git/index").display()
    );

    let git = OsStr::new("git");
    let git_hash = bounded_git(git, &repo, &["rev-parse", "--short=12", "HEAD"], GIT_BOUND)
        .unwrap_or_else(|| "unknown".to_string());
    // `None` is "could not be read in time" (a hung fsmonitor daemon, say), which
    // is not the same claim as clean: the id says so rather than guessing.
    let dirty = dirty_state(git, &repo, GIT_BOUND);
    let build_id = match dirty {
        Some(true) => format!("{git_hash}-dirty"),
        Some(false) => git_hash.clone(),
        None => format!("{git_hash}-dirty-unknown"),
    };
    let build_time = unix_utc();

    println!("cargo:rustc-env=DEVMAP_GIT_HASH={git_hash}");
    println!(
        "cargo:rustc-env=DEVMAP_GIT_DIRTY={}",
        if dirty == Some(true) { "1" } else { "0" }
    );
    println!("cargo:rustc-env=DEVMAP_BUILD_TIME={build_time}");
    println!("cargo:rustc-env=DEVMAP_BUILD_ID={build_id}");
}

fn unix_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
