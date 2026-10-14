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

/// Whether `text` names `path` in a spelling the platform gives it: as
/// written, or canonical. Windows canonicalises to a `\\?\` verbatim path
/// with long names where the temp dir used 8.3 short ones (`RUNNER~1`), and
/// macOS to `/private/var` for `/var`; every one is the same directory.
fn names(text: &str, path: &std::path::Path) -> bool {
    if text.contains(&path.display().to_string()) {
        return true;
    }
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    let shown = canonical.display().to_string();
    text.contains(shown.strip_prefix(r"\\?\").unwrap_or(&shown))
}

/// `path` as a `file://` URI in this platform's form: `file:///C:/…` on
/// Windows. `format!("file://{}", path.display())` gives `file://C:\…` there,
/// which is not a file URI at all.
fn file_uri(path: &std::path::Path) -> String {
    let shown = path.display().to_string();
    if cfg!(windows) {
        let bare = shown.strip_prefix(r"\\?\").unwrap_or(&shown);
        format!("file:///{}", bare.replace('\\', "/"))
    } else {
        format!("file://{shown}")
    }
}

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
    assert_eq!(
        got.canonicalize().unwrap_or(got),
        want.canonicalize().unwrap_or(want)
    );

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
    assert_eq!(
        recovered.canonicalize().unwrap_or(recovered),
        want_root.canonicalize().unwrap_or(want_root)
    );
}

#[tokio::test]
async fn mcp_roots_list_does_not_silently_replace_a_cwd_store() {
    let repo_a = scratch("mcp-a");
    let repo_b = scratch("mcp-b");
    plant_store(&repo_a);
    plant_store(&repo_b);

    let slot = Arc::new(StoreSlot::resolving(None, repo_a.clone(), None));
    assert!(
        slot.db_path().as_os_str().is_empty(),
        "an unresolved resolving slot must not preview store_path(cwd)"
    );

    slot.set_client_has_roots(true);
    let response = json!({
        "jsonrpc": "2.0",
        "id": ROOTS_LIST_REQUEST_ID,
        "result": {
            "roots": [{"uri": file_uri(&repo_b)}]
        }
    });
    assert!(handle_line(&slot, &response.to_string()).await.is_none());
    let err = match slot.get() {
        Ok(_) => panic!("cwd store and a different MCP root must not first-wins"),
        Err(err) => err,
    };
    assert!(
        names(&err, &repo_a) && names(&err, &repo_b),
        "must name both repositories: {err}"
    );
}

#[tokio::test]
async fn mcp_roots_list_opens_when_cwd_has_no_store() {
    let cwd = scratch("mcp-empty-cwd");
    let repo = scratch("mcp-root-only");
    let store = plant_store(&repo);
    let slot = Arc::new(StoreSlot::resolving(None, cwd, None));
    slot.set_client_has_roots(true);
    let response = json!({
        "jsonrpc": "2.0",
        "id": ROOTS_LIST_REQUEST_ID,
        "result": {
            "roots": [{"uri": file_uri(&repo)}]
        }
    });
    assert!(handle_line(&slot, &response.to_string()).await.is_none());
    let opened = slot.get().expect("a unique MCP-root store still opens");
    drop(opened);
    assert_eq!(
        slot.db_path(),
        store.canonicalize().unwrap_or_else(|_| store.clone())
    );
}

#[tokio::test]
async fn mcp_initialize_records_roots_capability() {
    let cwd = scratch("mcp-init-cwd");
    plant_store(&cwd);
    let slot = Arc::new(StoreSlot::resolving(None, cwd, None));
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
        "rust/testdata/fixtures/x.py"
    ));
    assert!(devmap_extract::is_default_index_excluded(
        "vendor/grammars/cobol/parser.c"
    ));
    assert!(!devmap_extract::is_default_index_excluded("src/main.rs"));
    assert!(!devmap_extract::is_default_index_excluded("testdata"));
    assert!(devmap_extract::is_ignored_path(
        "rust/testdata/capabilities/probe.py"
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
    let slot = Arc::new(StoreSlot::resolving(None, root.clone(), None));
    let _ = slot.get().expect("open");
    if let Err(error) = std::fs::remove_file(&store_path) {
        // Windows refuses to delete a file an open handle holds without
        // FILE_SHARE_DELETE (os error 32), so the mid-session deletion this
        // test exercises cannot happen there: the store stays whole and keeps
        // serving. Anything else is a real failure.
        assert!(
            cfg!(windows) && error.raw_os_error() == Some(32),
            "removing the store failed: {error}"
        );
        assert!(store_path.is_file(), "a refused delete leaves the store");
        slot.get()
            .expect("the store the delete could not touch still serves");
        return;
    }
    let err = match slot.get() {
        Ok(_) => panic!("deleted store must not reopen"),
        Err(err) => err,
    };
    assert!(
        err.contains("no devmap index") || err.contains("could not"),
        "{err}"
    );
}

/// Write `count` generations into a fresh store at `root`'s store path, so a
/// store can be told apart from its replacement by its latest generation id.
fn build_store_with_generations(root: &Path, count: u32) -> PathBuf {
    let store_path = devmap_extract::paths::store_path(root);
    std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
    let extraction = devmap_extract::extract_file("core.py", "def helper():\n    return 1\n");
    let extractions = vec![extraction];
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open(&store_path).expect("create store");
    for _ in 0..count {
        store
            .save_generation(&extractions, &resolution, &analysis)
            .expect("generation");
    }
    store_path
}

/// The slot modes a store can be reached through, each with how one MCP call
/// opens it. Canonical roots so the cache key is the path itself — the
/// spelling under which the stale-hit was reachable.
fn slot_modes(root: &Path) -> Vec<(&'static str, Arc<StoreSlot>, Option<String>)> {
    let store_path = devmap_extract::paths::store_path(root);
    vec![
        ("pinned", Arc::new(StoreSlot::new(store_path)), None),
        (
            "cwd",
            Arc::new(StoreSlot::resolving(None, root.to_path_buf(), None)),
            None,
        ),
        (
            "root pin",
            Arc::new(StoreSlot::resolving(
                None,
                scratch("rebuilt-elsewhere"),
                Some(root.to_path_buf()),
            )),
            None,
        ),
        (
            "repo_path",
            Arc::new(StoreSlot::resolving(None, scratch("rebuilt-cwd"), None)),
            Some(root.display().to_string()),
        ),
    ]
}

fn open_generation(slot: &StoreSlot, repo_path: Option<&str>) -> Result<Option<u32>, String> {
    let (store, _) = slot.open_for_call(repo_path)?;
    store
        .latest_generation_id()
        .map_err(|error| error.to_string())
}

/// `rm -rf .devmap && devmap build` is the documented quarantine remedy. A
/// long-lived MCP process kept its connection to the unlinked inode and went
/// on answering from that store's last generation on every slot mode.
#[test]
#[cfg(unix)]
fn a_store_rebuilt_at_the_same_path_is_reopened_on_every_slot_mode() {
    let root = scratch("rebuilt").canonicalize().unwrap();
    for (mode, slot, repo_path) in slot_modes(&root) {
        let store_path = build_store_with_generations(&root, 3);
        assert_eq!(
            open_generation(&slot, repo_path.as_deref()),
            Ok(Some(3)),
            "{mode}: the original store"
        );
        let state_dir = store_path.parent().unwrap().parent().unwrap();
        std::fs::remove_dir_all(state_dir).unwrap();
        build_store_with_generations(&root, 1);
        assert_eq!(
            open_generation(&slot, repo_path.as_deref()),
            Ok(Some(1)),
            "{mode}: the next call must answer from the rebuilt store, not the unlinked one"
        );
        std::fs::remove_dir_all(state_dir).unwrap();
    }
}

/// `--db` pins a path, and nothing on the way to the cache looked at it, so a
/// store deleted under a pinned server kept answering from the unlinked file.
#[test]
#[cfg(unix)]
fn a_pinned_store_unlinked_with_no_replacement_explains_its_absence() {
    let root = scratch("pinned-gone").canonicalize().unwrap();
    let store_path = build_store_with_generations(&root, 1);
    let slot = StoreSlot::new(store_path.clone());
    assert_eq!(open_generation(&slot, None), Ok(Some(1)));
    std::fs::remove_file(&store_path).unwrap();
    let error = open_generation(&slot, None).expect_err("a vanished store must not answer");
    assert!(error.contains("no devmap index"), "{error}");
}

/// The identity check must not turn into a reopen per call: an unchanged
/// store is served from the same open handle.
#[test]
fn an_unchanged_store_is_served_from_the_same_handle() {
    let root = scratch("steady").canonicalize().unwrap();
    build_store_with_generations(&root, 1);
    for (mode, slot, repo_path) in slot_modes(&root) {
        let (first, _) = slot.open_for_call(repo_path.as_deref()).unwrap();
        let (second, _) = slot.open_for_call(repo_path.as_deref()).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "{mode}: reopened an unchanged store"
        );
    }
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
