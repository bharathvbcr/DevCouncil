//! Phase 1 freshness/reach characterization and adversarial coverage.
//!
//! MCP store resolution without `--db`, default fixture/grammar excludes,
//! and root-swap / store-deleted failure modes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_serve::mcp::{handle_line, StoreSlot, ROOTS_LIST_REQUEST_ID};
use devmap_serve::RootResolveInput;
use devmap_store::Store;
use serde_json::json;

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-p1-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn plant_store(root: &Path) -> PathBuf {
    let store = devmap_extract::paths::store_path(root);
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let _ = Store::open(&store).expect("create store");
    store
}

#[test]
fn resolve_error_names_roots_cwd_and_db() {
    let err = RootResolveInput {
        mcp_roots: Some(vec![]),
        client_cwd: scratch("err-cwd"),
        explicit_db: None,
    }
    .resolve()
    .expect_err("nothing planted");
    assert!(err.contains("MCP roots/list"), "{err}");
    assert!(err.contains("client cwd"), "{err}");
    assert!(err.contains("--db"), "{err}");
}

#[test]
fn db_override_wins_when_file_exists_else_discovery_continues() {
    let via_root = scratch("ovr-root");
    let via_db = scratch("ovr-db");
    plant_store(&via_root);
    let want = plant_store(&via_db);
    let got = RootResolveInput {
        mcp_roots: Some(vec![via_root.clone()]),
        client_cwd: scratch("ovr-cwd"),
        explicit_db: Some(want.clone()),
    }
    .resolve_with_db_override()
    .unwrap();
    assert_eq!(got, want);

    // Unexpanded ${CLAUDE_PROJECT_DIR} class: --db names a missing path, roots win.
    let missing = via_db.join("no-such.sqlite");
    let want_root = plant_store(&via_root);
    let recovered = RootResolveInput {
        mcp_roots: Some(vec![via_root]),
        client_cwd: scratch("ovr-cwd-2"),
        explicit_db: Some(missing),
    }
    .resolve_with_db_override()
    .unwrap();
    assert_eq!(recovered, want_root);
}

#[tokio::test]
async fn mcp_roots_list_response_rebinds_the_store() {
    let repo_a = scratch("mcp-a");
    let repo_b = scratch("mcp-b");
    let store_a = plant_store(&repo_a);
    let store_b = plant_store(&repo_b);

    let slot = Arc::new(StoreSlot::resolving(None, repo_a.clone()));
    assert_eq!(slot.db_path(), store_a);

    slot.set_client_has_roots(true);
    let response = json!({
        "jsonrpc": "2.0",
        "id": ROOTS_LIST_REQUEST_ID,
        "result": {
            "roots": [{"uri": format!("file://{}", repo_b.display())}]
        }
    });
    assert!(handle_line(&slot, &response.to_string()).await.is_none());
    let opened = slot.get().expect("store b opens");
    drop(opened);
    assert_eq!(slot.db_path(), store_b);
}

#[tokio::test]
async fn mcp_initialize_records_roots_capability() {
    let cwd = scratch("mcp-init-cwd");
    plant_store(&cwd);
    let slot = Arc::new(StoreSlot::resolving(None, cwd));
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {"roots": {"listChanged": true}},
            "clientInfo": {"name": "test", "version": "0"}
        }
    });
    let response = handle_line(&slot, &init.to_string())
        .await
        .expect("initialize answers");
    assert_eq!(response["result"]["serverInfo"]["name"], "devmap");
    assert!(
        slot.client_has_roots(),
        "initialize must record that the client advertised roots"
    );
}

#[test]
fn testdata_and_vendor_grammars_are_default_index_excludes() {
    assert!(devmap_extract::is_default_index_excluded(
        "rust-port/testdata/fixtures/x.py"
    ));
    assert!(devmap_extract::is_default_index_excluded(
        "vendor/grammars/cobol/parser.c"
    ));
    assert!(!devmap_extract::is_default_index_excluded("src/main.rs"));
    assert!(!devmap_extract::is_default_index_excluded("testdata"));
    assert!(devmap_extract::is_ignored_path(
        "rust-port/testdata/capabilities/probe.py"
    ));
}

#[test]
fn oversized_skip_is_not_a_coverage_refusal() {
    use devmap_extract::model::DiscoverySkipReason;
    assert!(!DiscoverySkipReason::Oversized {
        bytes: 2_000_000,
        limit: 1_048_576
    }
    .is_refusal());
}

#[tokio::test]
async fn concurrent_status_while_store_deleted_mid_session_fails_closed() {
    let root = scratch("race-del");
    let store_path = plant_store(&root);
    let slot = Arc::new(StoreSlot::resolving(None, root.clone()));
    let _ = slot.get().expect("open");
    std::fs::remove_file(&store_path).unwrap();
    let err = match slot.get() {
        Ok(_) => panic!("deleted store must not reopen"),
        Err(err) => err,
    };
    assert!(
        err.contains("no devmap index") || err.contains("could not"),
        "{err}"
    );
}

#[tokio::test]
async fn oversized_roots_list_is_capped_in_resolve() {
    let cwd = scratch("big-roots-cwd");
    let mut roots = Vec::new();
    for i in 0..70 {
        roots.push(scratch(&format!("big-root-{i}")));
    }
    let err = RootResolveInput {
        mcp_roots: Some(roots),
        client_cwd: cwd,
        explicit_db: None,
    }
    .resolve()
    .expect_err("no stores");
    assert!(err.contains("only the first 64"), "{err}");
}
