//! A retired `codeintel/base_store_ref.json` pointer on a lane with no store and
//! no lease is refused by name, with the command that replaces it.
//!
//! Before this, `devmap status` answered a plain "no devmap store at this path"
//! for such a lane: the stale pointer was invisible, so the operator was told to
//! `devmap build` a full index when the lane should have been re-seeded from its
//! donor. The refusal reads only; it never creates a lease, a `stack/`, a delta
//! or a store.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-legacy-ref-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tree_listing(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.push((path, bytes));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn status_names_the_retired_pointer_and_the_upgrade_command() {
    let lane = scratch("status");
    let pointer = lane.join(".devmap/codeintel/base_store_ref.json");
    std::fs::create_dir_all(pointer.parent().unwrap()).unwrap();
    std::fs::write(&pointer, br#"{"anchor_repo_root":"/nowhere/anchor"}"#).unwrap();
    let before = tree_listing(&lane);

    let out = Command::new(devmap())
        .env_remove("DEVMAP_HOME")
        .arg("--json")
        .arg("--root")
        .arg(&lane)
        .arg("status")
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let payload: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|err| panic!("status JSON: {err}\nstdout: {stdout}"));
    let reason = payload["degraded_reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("retired legacy worktree pointer"),
        "degraded_reason must name the retired pointer, got {reason:?}"
    );
    assert!(
        reason.contains("devmap seed --stacked --from"),
        "degraded_reason must name the replacement command, got {reason:?}"
    );
    assert_eq!(
        tree_listing(&lane),
        before,
        "the refusal must not write a single byte into the lane"
    );
    let _ = std::fs::remove_dir_all(&lane);
}
