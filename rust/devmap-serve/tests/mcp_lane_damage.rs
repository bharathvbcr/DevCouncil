#![cfg(unix)]

//! Damage met in a stacked lane is attributed to the lane file that has it.
//!
//! A lane reads two SQLite files: the shared, immutable `base.sqlite` and the
//! worktree's own `delta.sqlite`. A damaged delta used to fail the lane's open
//! before any naming could happen; the server then evicted and reopened the
//! healthy base, found the lane failing again and remembered the base as
//! damaged, with the wrong file and remedy named, until the server restarted.
//! `devmap seed --sync-delta`, which repairs the delta, did not clear it.
//!
//! The lane is built the way `devmap seed --stacked` builds one, through
//! `provision_stacked_worktree`, and the damage is real: pages of
//! `delta.sqlite` are overwritten in place.
//!
//! There is no real-file test for a damaged *base*: the base is sealed by the
//! lease (length, modification time, change time, inode, digest), so any write
//! to it is refused by the seal check as "the base changed" before SQLite can
//! report corruption. The attribution of a corrupt error that does not carry
//! the delta's tag to the base is pinned by a unit test in `mcp.rs`
//! (`damage_recovery_tests`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_serve::repo_scope::{open_mcp_store, MCP_READER_MEMORY};
use serde_json::{json, Value};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-lane-damage-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(["-c", "user.name=T", "-c", "user.email=t@example.invalid"])
        .args(["-c", "commit.gpgSign=false", "-c", "core.hooksPath="])
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

struct Lane {
    root: PathBuf,
    anchor: PathBuf,
    worktree: PathBuf,
    delta: PathBuf,
}

/// An anchor checkout with a store, and a stacked worktree over it whose delta
/// holds one synced edit.
fn lane(name: &str) -> Lane {
    let root = scratch(name);
    let anchor = root.join("anchor");
    let worktree = root.join("lane-wt");
    std::fs::create_dir_all(&anchor).unwrap();
    git(&anchor, &["init", "-q", "-b", "main"]);
    std::fs::write(
        anchor.join("lib.rs"),
        "pub fn shared_target() -> i32 { 1 }\npub fn base_caller() -> i32 { shared_target() }\n",
    )
    .unwrap();
    git(&anchor, &["add", "lib.rs"]);
    git(&anchor, &["commit", "-qm", "init"]);
    git(
        &anchor,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree.to_str().unwrap(),
        ],
    );
    let head = git(&anchor, &["rev-parse", "HEAD"]);

    let anchor_db = devmap_extract::paths::store_path(&anchor);
    std::fs::create_dir_all(anchor_db.parent().unwrap()).unwrap();
    {
        let store = devmap_store::Store::open(&anchor_db).unwrap();
        store
            .bind_repo_root(&anchor.canonicalize().unwrap())
            .unwrap();
        let src = std::fs::read_to_string(anchor.join("lib.rs")).unwrap();
        let extractions = vec![devmap_extract::extract_file("lib.rs", &src)];
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions).unwrap();
        let analysis = devmap_analyze::analyze(&extractions, &resolution);
        store
            .save_generation_with_metadata(
                &extractions,
                &resolution,
                &analysis,
                devmap_store::GenerationWriteOpts::default(),
                &head,
            )
            .unwrap();
        let _ = store.write_latest_csr_sidecar().unwrap();
    }
    devmap_query::stacked::provision_stacked_worktree(&anchor, &worktree).unwrap();
    std::fs::write(
        worktree.join("lib.rs"),
        "pub fn shared_target() -> i32 { 1 }\npub fn delta_caller() -> i32 { shared_target() }\n",
    )
    .unwrap();
    {
        let mut stacked = devmap_query::stacked::StackedStore::open(&worktree).unwrap();
        stacked.sync_file_from_disk("lib.rs").unwrap();
    }
    let delta = devmap_extract::paths::stack_delta_path(&worktree);
    assert!(delta.is_file(), "the fixture must hold a delta");
    Lane {
        root,
        anchor,
        worktree,
        delta,
    }
}

/// Remove a fixture. The lane's sealed base and its directory are read-only,
/// so write access is restored first.
fn remove(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    fn unlock(path: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if meta.is_symlink() {
            return;
        }
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
        if meta.is_dir() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    unlock(&entry.path());
                }
            }
        }
    }
    unlock(root);
    std::fs::remove_dir_all(root).unwrap();
}

async fn call(slot: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    handle_line(slot, &frame.to_string())
        .await
        .expect("a response frame")
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], json!(true), "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Overwrite every page after the first, in place. Returns the original bytes.
fn damage_in_place(file: &Path) -> Vec<u8> {
    use std::io::{Seek, SeekFrom, Write};
    let original = std::fs::read(file).unwrap();
    let raw = u16::from_be_bytes([original[16], original[17]]);
    let page = if raw == 1 { 65_536 } else { raw as usize };
    assert!(
        original.len() > 2 * page,
        "precondition: the file has pages beyond the first"
    );
    let mut handle = std::fs::OpenOptions::new().write(true).open(file).unwrap();
    handle.seek(SeekFrom::Start(page as u64)).unwrap();
    handle
        .write_all(&vec![0xFF; original.len() - page])
        .unwrap();
    handle.sync_all().unwrap();
    original
}

#[tokio::test]
async fn a_damaged_delta_is_named_and_the_healthy_base_is_never_remembered_as_damaged() {
    let lane = lane("delta");
    let slot = Arc::new(StoreSlot::resolving(
        None,
        lane.worktree.clone(),
        Some(lane.anchor.clone()),
    ));
    let repo = lane.worktree.display().to_string();
    let args = json!({"repo_path": repo, "target": "shared_target"});

    let healthy = call(&slot, "devmap_impact", args.clone()).await;
    assert_eq!(healthy["result"]["isError"], json!(false), "{healthy}");

    let original = damage_in_place(&lane.delta);

    for attempt in 0..3 {
        let text = error_text(&call(&slot, "devmap_impact", args.clone()).await);
        assert!(
            text.contains(&lane.delta.display().to_string()),
            "attempt {attempt}: must name the delta file {}: {text}",
            lane.delta.display()
        );
        assert!(
            text.contains("devmap seed --sync-delta"),
            "attempt {attempt}: must give the delta's remedy: {text}"
        );
        assert!(
            !text.contains("base.sqlite") && !text.contains("has not changed since"),
            "attempt {attempt}: the healthy base must not be blamed or remembered: {text}"
        );
    }

    // `devmap seed --sync-delta` replaces the delta. The very next call answers:
    // nothing about the lane was remembered.
    std::fs::write(&lane.delta, &original).unwrap();
    let healed = call(&slot, "devmap_impact", args).await;
    assert_eq!(
        healed["result"]["isError"],
        json!(false),
        "a repaired delta is answered from at once: {healed}"
    );
    remove(&lane.root);
}

#[test]
fn a_lane_base_opened_for_the_server_uses_the_servers_smaller_cache() {
    let lane = lane("memory");
    let base_path = devmap_query::stacked::StackedStore::open(&lane.worktree)
        .unwrap()
        .lease()
        .base_sqlite_path();
    let base = open_mcp_store(&base_path).expect("the lane base opens through the server's opener");
    assert_eq!(
        base.memory_settings().unwrap(),
        MCP_READER_MEMORY,
        "the lane base is held by the server like any other store"
    );
    remove(&lane.root);
}
