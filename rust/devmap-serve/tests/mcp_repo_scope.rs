//! Repository scoping for the shared MCP process.
//!
//! Cursor runs one `devmap mcp` for every workspace tab. These tests pin the
//! class that makes that safe: per-call `repo_path`, no first-wins across
//! MCP roots, roots/list_changed re-queries without clearing, and every
//! success names the repository it answered from.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_serve::mcp::{
    handle_line, serve_streams, to_ipc_command, tool_specs, StoreSlot, ROOTS_LIST_REQUEST_ID,
};
use devmap_store::Store;
use serde_json::{json, Value};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-mcp-scope-{name}-{}-{}",
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

async fn call(store: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    handle_line(store, &frame.to_string())
        .await
        .expect("a request must produce a response frame")
}

fn tool_error_text(response: &Value) -> String {
    let result = &response["result"];
    assert_eq!(result["isError"], json!(true), "{response}");
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn structured(response: &Value) -> &Value {
    &response["result"]["structuredContent"]
}

#[test]
fn every_tool_declares_optional_repo_path() {
    for spec in tool_specs() {
        let name = spec["name"].as_str().unwrap();
        let properties = spec["inputSchema"]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} has no properties object"));
        assert!(
            properties.contains_key("repo_path"),
            "{name} must accept repo_path"
        );
        assert!(
            properties.contains_key("root"),
            "{name} must accept root as an alias of repo_path"
        );
        let required = spec["inputSchema"]["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !required.iter().any(|v| v.as_str() == Some("repo_path")),
            "{name}: repo_path is optional so a single-root client still works"
        );
    }
}

#[test]
fn to_ipc_command_accepts_repo_path_without_forwarding_it() {
    to_ipc_command(
        "devmap_status",
        Some(&json!({"repo_path": "/tmp/some-repo"})),
    )
    .unwrap_or_else(|err| panic!("repo_path must not be an unknown argument: {err:?}"));
}

#[tokio::test]
async fn tools_call_with_repo_path_selects_that_store_not_the_mcp_root() {
    let via_roots = scratch("call-roots");
    let via_arg = scratch("call-arg");
    plant_store(&via_roots);
    plant_store(&via_arg);

    let slot = Arc::new(StoreSlot::resolving(None, scratch("call-cwd")));
    slot.set_mcp_roots(vec![via_roots.clone()]);

    let response = call(
        &slot,
        "devmap_status",
        json!({"repo_path": via_arg.display().to_string()}),
    )
    .await;
    assert_eq!(
        response["result"]["isError"],
        json!(false),
        "repo_path must open the named store: {response}"
    );
    let structured = structured(&response);
    let reported = structured["repository"]["root"]
        .as_str()
        .unwrap_or_default();
    let canonical = via_arg.canonicalize().unwrap();
    assert_eq!(
        Path::new(reported),
        canonical.as_path(),
        "answered from the repo_path store, not the MCP root: {structured}"
    );
    assert_eq!(
        structured["repository"]["resolved_from"],
        json!("repo_path"),
        "{structured}"
    );
}

#[tokio::test]
async fn invalid_repo_path_is_a_tool_error_not_a_fallback() {
    let via_roots = scratch("inv-roots");
    plant_store(&via_roots);
    let slot = Arc::new(StoreSlot::resolving(None, via_roots.clone()));
    slot.set_mcp_roots(vec![via_roots.clone()]);

    let missing = scratch("inv-missing");
    let _ = std::fs::remove_dir_all(&missing);
    for (label, value) in [
        ("missing directory", missing.display().to_string()),
        ("relative path", "not/absolute".to_string()),
    ] {
        let response = call(&slot, "devmap_status", json!({"repo_path": value})).await;
        let text = tool_error_text(&response);
        assert!(
            text.contains("repo_path")
                || text.to_lowercase().contains("relative")
                || text.to_lowercase().contains("invalid"),
            "{label}: must name the refused argument: {text}"
        );
        if response["result"]["isError"] != json!(true) {
            panic!("{label}: invalid repo_path must be a tool error, not a fallback: {response}");
        }
        let reported = response["result"]
            .pointer("/structuredContent/repository/root")
            .and_then(Value::as_str);
        if let Some(reported) = reported {
            let fallback = via_roots.canonicalize().unwrap();
            assert_ne!(
                Path::new(reported),
                fallback.as_path(),
                "{label}: must not answer from the MCP root: {response}"
            );
        }
    }
}

#[tokio::test]
async fn list_changed_triggers_a_new_roots_list_and_does_not_clear() {
    let first = scratch("lc-a");
    plant_store(&first);

    let slot = Arc::new(StoreSlot::resolving(None, scratch("lc-cwd")));
    slot.set_client_has_roots(true);
    slot.set_mcp_roots(vec![first.clone()]);
    assert!(slot.get().is_ok(), "precondition: first root opens");

    let (client, server) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(serve_streams(
        Arc::clone(&slot),
        tokio::io::BufReader::new(server_read),
        server_write,
    ));

    let (client_read, mut client_write) = tokio::io::split(client);
    {
        use tokio::io::AsyncWriteExt;
        let notify = json!({
            "jsonrpc": "2.0",
            "method": "notifications/roots/list_changed"
        });
        client_write
            .write_all(format!("{notify}\n").as_bytes())
            .await
            .unwrap();
        client_write.flush().await.unwrap();
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        slot.get().is_ok(),
        "list_changed must keep the previous roots list until the new roots/list answer arrives"
    );

    {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(client_read).lines();
        let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("roots/list must be re-sent after list_changed")
            .unwrap()
            .expect("a frame");
        let frame: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(frame["method"], json!("roots/list"), "{frame}");
        assert_ne!(frame["id"], json!(null), "roots/list is a request: {frame}");
        drop(client_write);
    }

    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
}

#[tokio::test]
async fn every_success_payload_carries_repository_root() {
    let root = scratch("attr-root");
    plant_store(&root);
    let slot = Arc::new(StoreSlot::resolving(None, root.clone()));
    slot.set_mcp_roots(vec![root.clone()]);

    let response = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    let structured = structured(&response);
    let reported = structured["repository"]["root"]
        .as_str()
        .expect("repository.root is required on success");
    assert_eq!(
        Path::new(reported),
        root.canonicalize().unwrap().as_path(),
        "{structured}"
    );
    assert!(
        structured["repository"]["store"].as_str().is_some(),
        "{structured}"
    );
    let from = structured["repository"]["resolved_from"]
        .as_str()
        .unwrap_or_default();
    assert!(
        matches!(from, "repo_path" | "mcp_roots" | "cwd" | "db"),
        "resolved_from must be a known source: {from}"
    );
}

#[tokio::test]
async fn tool_success_schema_enforces_repository() {
    let root = scratch("schema-root");
    plant_store(&root);
    let slot = Arc::new(StoreSlot::resolving(None, root.clone()));
    let response = call(&slot, "devmap_status", json!({})).await;
    let structured = structured(&response);
    assert!(
        devmap_serve::mcp::structured_content_violation("devmap_status", structured).is_none(),
        "{:?}",
        devmap_serve::mcp::structured_content_violation("devmap_status", structured)
    );
    for spec in tool_specs() {
        let name = spec["name"].as_str().unwrap();
        let required = spec["outputSchema"]["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            required.iter().any(|v| v.as_str() == Some("repository")),
            "{name} outputSchema must require repository"
        );
    }
}

#[tokio::test]
async fn preview_without_repo_path_and_two_candidate_roots_is_refused() {
    let a = scratch("prev-a");
    let b = scratch("prev-b");
    plant_store(&a);
    plant_store(&b);
    let slot = Arc::new(StoreSlot::resolving(None, scratch("prev-cwd")));
    slot.set_mcp_roots(vec![a.clone(), b.clone()]);

    let response = call(
        &slot,
        "devmap_preview",
        json!({"file": "a.py", "content": "def x():\n    return 1\n"}),
    )
    .await;
    let text = tool_error_text(&response);
    assert!(
        text.contains(&a.display().to_string()) && text.contains(&b.display().to_string()),
        "preview must name both candidate roots, not judge the first: {text}"
    );
}

#[tokio::test]
async fn unresolved_slot_does_not_write_a_session_log_under_cwd() {
    let cwd = scratch("log-cwd");
    let slot = Arc::new(StoreSlot::resolving(None, cwd.clone()));
    let _ = call(&slot, "devmap_status", json!({})).await;
    let stray = devmap_extract::paths::store_path(&cwd)
        .parent()
        .unwrap()
        .join("sessions")
        .join("live.jsonl");
    assert!(
        !stray.exists(),
        "session log must not be created from an unresolved slot at {}",
        stray.display()
    );
}

#[test]
fn opening_an_older_schema_does_not_change_the_file_and_reports_rebuild_required() {
    let root = scratch("schema-old");
    let db = plant_store(&root);
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("PRAGMA user_version = 11", []).unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    let slot = StoreSlot::new(&db);
    let err = match slot.get() {
        Ok(_) => panic!("schema-behind must not open"),
        Err(err) => err,
    };
    assert!(
        err.contains("rebuild_required") || err.contains("schema"),
        "{err}"
    );
    let after = std::fs::read(&db).unwrap();
    assert_eq!(
        before, after,
        "a read-only MCP open must never migrate the store"
    );
}

#[tokio::test]
async fn two_spellings_of_one_case_insensitive_path_open_one_canonical_root() {
    let root = scratch("CaseRoot");
    plant_store(&root);
    let parent = root.parent().unwrap();
    let name = root.file_name().unwrap().to_str().unwrap();
    let flipped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else {
                c.to_ascii_uppercase()
            }
        })
        .collect();
    let alt = parent.join(&flipped);
    if !alt.exists() {
        return;
    }
    let slot = Arc::new(StoreSlot::resolving(None, scratch("case-cwd")));
    let first = call(
        &slot,
        "devmap_status",
        json!({"repo_path": root.display().to_string()}),
    )
    .await;
    let second = call(
        &slot,
        "devmap_status",
        json!({"repo_path": alt.display().to_string()}),
    )
    .await;
    assert_eq!(first["result"]["isError"], json!(false), "{first}");
    assert_eq!(second["result"]["isError"], json!(false), "{second}");
    assert_eq!(
        structured(&first)["repository"]["root"],
        structured(&second)["repository"]["root"],
        "canonical repository.root must match across spellings"
    );
}

#[tokio::test]
async fn windows_file_uri_is_rejected_not_reinterpreted() {
    let unix = scratch("uri-unix");
    plant_store(&unix);
    let slot = Arc::new(StoreSlot::resolving(None, unix.clone()));
    let response = json!({
        "jsonrpc": "2.0",
        "id": ROOTS_LIST_REQUEST_ID,
        "result": {
            "roots": [{"uri": "file:///C:/Users/nobody/project"}]
        }
    });
    assert!(handle_line(&slot, &response.to_string()).await.is_none());
    let err = match slot.get() {
        Ok(_) => panic!("a Windows file URI is not a unix root"),
        Err(err) => err,
    };
    assert!(
        !err.contains("/C:/Users/nobody"),
        "file:///C:/ must not become /C:/…: {err}"
    );
}

#[tokio::test]
async fn status_reports_candidate_roots_when_more_than_one_store_exists() {
    let a = scratch("cand-a");
    let b = scratch("cand-b");
    plant_store(&a);
    plant_store(&b);
    let slot = Arc::new(StoreSlot::resolving(None, scratch("cand-cwd")));
    slot.set_mcp_roots(vec![a.clone(), b.clone()]);
    let response = call(
        &slot,
        "devmap_status",
        json!({"repo_path": a.display().to_string()}),
    )
    .await;
    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    let structured = structured(&response);
    assert_eq!(structured["ambiguous"], json!(true), "{structured}");
    let candidates = structured["candidate_roots"]
        .as_array()
        .expect("candidate_roots");
    assert!(candidates.len() >= 2, "{structured}");
}
