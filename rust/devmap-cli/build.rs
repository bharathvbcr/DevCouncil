//! Embed a build identifier: git hash, dirty flag, and UTC time.
//!
//! `--version` and `doctor --json` must name the binary that is running, not
//! just the crate version string. Two installs with the same version and
//! different git hashes is the skew doctor used to miss.

mod build_git;

use build_git::{bounded_git, build_id, dirty_env, dirty_state, GIT_BOUND};
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
    let hash = bounded_git(git, &repo, &["rev-parse", "--short=12", "HEAD"], GIT_BOUND);
    // `None` is "could not be read in time" (a hung fsmonitor daemon, say), which
    // is not the same claim as clean. With no hash there is no checkout to be
    // dirty or clean, so the state is unknown too.
    let dirty = match hash {
        Some(_) => dirty_state(git, &repo, GIT_BOUND),
        None => None,
    };
    let build_id = build_id(hash.as_deref(), dirty);
    let git_hash = hash.unwrap_or_else(|| "unknown".to_string());
    let build_time = unix_utc();

    println!("cargo:rustc-env=DEVMAP_GIT_HASH={git_hash}");
    println!(
        "cargo:rustc-env=DEVMAP_GIT_DIRTY={}",
        dirty_env(dirty)
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
