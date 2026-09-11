//! Embed a build identifier: git hash, dirty flag, and UTC time.
//!
//! `--version` and `doctor --json` must name the binary that is running, not
//! just the crate version string. Two installs with the same version and
//! different git hashes is the skew doctor used to miss.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let repo = Path::new(&manifest).join("../..");
    println!("cargo:rerun-if-changed={}", repo.join(".git/HEAD").display());
    println!("cargo:rerun-if-changed={}", repo.join(".git/index").display());

    let git_hash = git_output(&repo, &["rev-parse", "--short=12", "HEAD"])
        .unwrap_or_else(|| "unknown".to_string());
    let dirty = git_output(&repo, &["status", "--porcelain"])
        .map(|text| !text.trim().is_empty())
        .unwrap_or(false);
    let build_id = if dirty {
        format!("{git_hash}-dirty")
    } else {
        git_hash.clone()
    };
    let build_time = unix_utc();

    println!("cargo:rustc-env=DEVMAP_GIT_HASH={git_hash}");
    println!(
        "cargo:rustc-env=DEVMAP_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );
    println!("cargo:rustc-env=DEVMAP_BUILD_TIME={build_time}");
    println!("cargo:rustc-env=DEVMAP_BUILD_ID={build_id}");
}

fn git_output(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.trim().to_string())
}

fn unix_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}
