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
        let description = spec["description"].as_str().unwrap_or("");
        assert!(
            description.contains("repo_path"),
            "{name} description must mention repo_path so a host that cached an empty schema still sees the argument: {description}"
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

    let slot = Arc::new(StoreSlot::resolving(None, scratch("call-cwd"), None));
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
    let slot = Arc::new(StoreSlot::resolving(None, via_roots.clone(), None));
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

    let slot = Arc::new(StoreSlot::resolving(None, scratch("lc-cwd"), None));
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
    let slot = Arc::new(StoreSlot::resolving(None, root.clone(), None));
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
    let slot = Arc::new(StoreSlot::resolving(None, root.clone(), None));
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
async fn cwd_store_and_different_mcp_root_without_repo_path_is_refused() {
    let via_roots = scratch("cwd-roots");
    let via_cwd = scratch("cwd-named");
    plant_store(&via_roots);
    plant_store(&via_cwd);
    let slot = Arc::new(StoreSlot::resolving(None, via_cwd.clone(), None));
    slot.set_mcp_roots(vec![via_roots.clone()]);

    let response = call(&slot, "devmap_status", json!({})).await;
    let text = tool_error_text(&response);
    assert!(
        text.contains(&via_roots.display().to_string())
            && text.contains(&via_cwd.display().to_string()),
        "must name both the MCP root and the cwd/--root repository: {text}"
    );

    let via_arg = call(
        &slot,
        "devmap_status",
        json!({"repo_path": via_cwd.display().to_string()}),
    )
    .await;
    assert_eq!(via_arg["result"]["isError"], json!(false), "{via_arg}");
    let reported = structured(&via_arg)["repository"]["root"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        Path::new(reported),
        via_cwd.canonicalize().unwrap().as_path(),
        "repo_path must still select the named store: {via_arg}"
    );
}

#[tokio::test]
async fn initialize_advertises_tools_list_changed() {
    let slot = Arc::new(StoreSlot::resolving(None, scratch("init-cwd"), None));
    let response = handle_line(
        &slot,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "scope-test", "version": "0"}
            }
        })
        .to_string(),
    )
    .await
    .expect("initialize returns a result");
    assert_eq!(
        response["result"]["capabilities"]["tools"]["listChanged"],
        json!(true),
        "hosts cache tools/list; listChanged must be true so a schema change can invalidate that cache: {response}"
    );
}

#[tokio::test]
async fn initialized_emits_tools_list_changed_so_hosts_refetch_schemas() {
    let slot = Arc::new(StoreSlot::resolving(None, scratch("notify-cwd"), None));
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
            "method": "notifications/initialized"
        });
        client_write
            .write_all(format!("{notify}\n").as_bytes())
            .await
            .unwrap();
        client_write.flush().await.unwrap();
    }

    {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(client_read).lines();
        let line = tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
            .await
            .expect("tools/list_changed must be sent after initialized")
            .unwrap()
            .expect("a frame");
        let frame: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(
            frame["method"],
            json!("notifications/tools/list_changed"),
            "stale host schema caches only invalidate on this notification: {frame}"
        );
        assert!(
            frame.get("id").is_none() || frame["id"].is_null(),
            "list_changed is a notification: {frame}"
        );
        drop(client_write);
    }

    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
}

#[tokio::test]
async fn preview_without_repo_path_and_two_candidate_roots_is_refused() {
    let a = scratch("prev-a");
    let b = scratch("prev-b");
    plant_store(&a);
    plant_store(&b);
    let slot = Arc::new(StoreSlot::resolving(None, scratch("prev-cwd"), None));
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
    let slot = Arc::new(StoreSlot::resolving(None, cwd.clone(), None));
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

/// MCP with no `--root` and a non-repository cwd ($HOME / temp) must refuse
/// with the `repo_path` fix named, and must not create `.devmap` state there.
#[tokio::test]
async fn unsafe_cwd_without_repo_path_refuses_and_creates_no_state() {
    let cwd = std::env::temp_dir();
    let marker = cwd.join(format!(
        ".devmap-mcp-refuse-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // Preflight: the temp dir itself is the banned cwd; we only assert that
    // DevMap does not create its store/session tree under it for this call.
    let store_before = devmap_extract::paths::store_path(&cwd);
    let parent_before = store_before.parent().map(|p| p.exists()).unwrap_or(false);

    let slot = Arc::new(StoreSlot::resolving(None, cwd.clone(), None));
    let response = call(&slot, "devmap_status", json!({})).await;
    let text = tool_error_text(&response);
    assert!(
        text.contains("repo_path")
            || text.contains("--root")
            || text.contains("temp directory")
            || text.contains("$HOME"),
        "refusal must name the fix: {text}"
    );
    assert!(!marker.exists(), "test probe must not exist (sanity)");
    if !parent_before {
        assert!(
            !store_before.exists(),
            "must not create a store under the temp cwd: {}",
            store_before.display()
        );
        let sessions = store_before
            .parent()
            .unwrap()
            .join("sessions")
            .join("live.jsonl");
        assert!(
            !sessions.exists(),
            "must not create a session log under the temp cwd: {}",
            sessions.display()
        );
    }
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
    let typed = match Store::open_read_only(&db) {
        Ok(_) => panic!("schema-behind must refuse"),
        Err(err) => err,
    };
    assert!(
        Store::is_unsupported_schema(&typed),
        "open_read_only must expose a typed unsupported-schema error: {typed}"
    );
    let slot = StoreSlot::new(&db);
    let err = match slot.get() {
        Ok(_) => panic!("schema-behind must not open"),
        Err(err) => err,
    };
    assert!(
        err.contains("rebuild_required"),
        "open_mcp_store must classify by typed error: {err}"
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
    let slot = Arc::new(StoreSlot::resolving(None, scratch("case-cwd"), None));
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
    let slot = Arc::new(StoreSlot::resolving(None, unix.clone(), None));
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
    let slot = Arc::new(StoreSlot::resolving(None, scratch("cand-cwd"), None));
    slot.set_mcp_roots(vec![a.clone(), b.clone()]);
    let response = call(
        &slot,
        "devmap_status",
        json!({"repo_path": a.display().to_string()}),
    )
    .await;
    assert_eq!(response["result"]["isError"], json!(false), "{response}");
    let structured = structured(&response);
    assert_eq!(structured["ambiguous"], json!(false), "{structured}");
    assert_eq!(
        structured["repository"]["resolved_from"],
        json!("repo_path"),
        "{structured}"
    );
    let candidates = structured["candidate_roots"]
        .as_array()
        .expect("candidate_roots");
    assert!(candidates.len() >= 2, "{structured}");
}

#[tokio::test]
async fn mixed_calls_cannot_first_wins_across_cwd_and_mcp_root() {
    let via_roots = scratch("stress-roots");
    let via_cwd = scratch("stress-cwd");
    plant_store(&via_roots);
    plant_store(&via_cwd);
    let slot = Arc::new(StoreSlot::resolving(None, via_cwd.clone(), None));
    slot.set_mcp_roots(vec![via_roots.clone()]);
    let roots = via_roots.canonicalize().unwrap();
    let cwd = via_cwd.canonicalize().unwrap();

    for i in 0..64 {
        match i % 6 {
            0 => {
                let text = tool_error_text(&call(&slot, "devmap_status", json!({})).await);
                assert!(
                    text.contains(&via_roots.display().to_string())
                        && text.contains(&via_cwd.display().to_string()),
                    "iteration {i}: empty args must stay ambiguous: {text}"
                );
            }
            1 => {
                let response = call(
                    &slot,
                    "devmap_status",
                    json!({"repo_path": via_cwd.display().to_string()}),
                )
                .await;
                assert_eq!(response["result"]["isError"], json!(false), "{response}");
                assert_eq!(
                    Path::new(
                        structured(&response)["repository"]["root"]
                            .as_str()
                            .unwrap_or_default()
                    ),
                    cwd.as_path()
                );
            }
            2 => {
                let response = call(
                    &slot,
                    "devmap_status",
                    json!({"root": via_roots.display().to_string()}),
                )
                .await;
                assert_eq!(response["result"]["isError"], json!(false), "{response}");
                assert_eq!(
                    Path::new(
                        structured(&response)["repository"]["root"]
                            .as_str()
                            .unwrap_or_default()
                    ),
                    roots.as_path()
                );
            }
            3 => {
                let text = tool_error_text(
                    &call(
                        &slot,
                        "devmap_status",
                        json!({"repo_path": "relative/repo"}),
                    )
                    .await,
                );
                assert!(
                    text.to_lowercase().contains("relative") || text.contains("repo_path"),
                    "iteration {i}: {text}"
                );
            }
            4 => {
                let text = tool_error_text(
                    &call(
                        &slot,
                        "devmap_status",
                        json!({
                            "repo_path": via_cwd.display().to_string(),
                            "root": via_roots.display().to_string()
                        }),
                    )
                    .await,
                );
                assert!(
                    text.contains("differ") || text.contains("repo_path"),
                    "iteration {i}: disagreeing aliases must not pick one: {text}"
                );
            }
            _ => {
                let text =
                    tool_error_text(&call(&slot, "devmap_status", json!({"repo_path": ""})).await);
                assert!(
                    text.contains("empty") || text.contains("repo_path"),
                    "iteration {i}: {text}"
                );
            }
        }
    }
}

#[tokio::test]
async fn explicit_root_pin_resolves_despite_ambiguous_mcp_roots() {
    // Plan B: `--root A` with MCP roots `[A, B]` must resolve A as `resolved_from: "root"`,
    // not refuse as ambiguous (the current cwd-folding behaviour).
    let pinned = scratch("pin-a");
    let other = scratch("pin-b");
    plant_store(&pinned);
    plant_store(&other);

    let slot = Arc::new(StoreSlot::resolving(
        None,
        scratch("pin-cwd"),
        Some(pinned.clone()),
    ));
    slot.set_mcp_roots(vec![pinned.clone(), other.clone()]);

    let response = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(
        response["result"]["isError"],
        json!(false),
        "explicit --root must pin through multi-root ambiguity: {response}"
    );
    let structured = structured(&response);
    let reported = structured["repository"]["root"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(
        Path::new(reported),
        pinned.canonicalize().unwrap().as_path(),
        "answered from the pin, not first-wins: {structured}"
    );
    assert_eq!(
        structured["repository"]["resolved_from"],
        json!("root"),
        "{structured}"
    );
    let candidates = structured["candidate_roots"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        candidates.len() >= 2,
        "candidate_roots must still name the other MCP roots: {structured}"
    );
}

#[tokio::test]
async fn repo_path_against_a_foreign_root_pin_is_a_tool_error_naming_both() {
    let pinned = scratch("pin-match-a");
    let foreign = scratch("pin-match-b");
    plant_store(&pinned);
    plant_store(&foreign);

    let slot = Arc::new(StoreSlot::resolving(
        None,
        scratch("pin-match-cwd"),
        Some(pinned.clone()),
    ));
    slot.set_mcp_roots(vec![pinned.clone(), foreign.clone()]);

    let response = call(
        &slot,
        "devmap_status",
        json!({"repo_path": foreign.display().to_string()}),
    )
    .await;
    let text = tool_error_text(&response);
    assert!(
        text.contains(&pinned.display().to_string())
            || text.contains(&pinned.canonicalize().unwrap().display().to_string()),
        "error must name the --root pin: {text}"
    );
    assert!(
        text.contains(&foreign.display().to_string())
            || text.contains(&foreign.canonicalize().unwrap().display().to_string()),
        "error must name the refused repo_path: {text}"
    );
}

#[tokio::test]
async fn late_devmap_roots_reply_is_ignored_after_a_newer_seq() {
    let newer = scratch("seq-newer");
    let older = scratch("seq-older");
    plant_store(&newer);
    plant_store(&older);
    let slot = Arc::new(StoreSlot::resolving(None, scratch("seq-cwd"), None));

    let apply_newer = json!({
        "jsonrpc": "2.0",
        "id": "devmap-roots-2",
        "result": {
            "roots": [{"uri": format!("file://{}", newer.display())}]
        }
    });
    assert!(
        handle_line(&slot, &apply_newer.to_string()).await.is_none(),
        "roots/list response is not a request"
    );

    let apply_older = json!({
        "jsonrpc": "2.0",
        "id": "devmap-roots-1",
        "result": {
            "roots": [{"uri": format!("file://{}", older.display())}]
        }
    });
    assert!(handle_line(&slot, &apply_older.to_string()).await.is_none());

    let candidates = slot.candidate_roots();
    let newer_canon = newer.canonicalize().unwrap();
    let older_canon = older.canonicalize().unwrap();
    assert!(
        candidates.iter().any(|root| root == &newer_canon),
        "seq-2 roots must remain: {candidates:?}"
    );
    assert!(
        !candidates.iter().any(|root| root == &older_canon),
        "late seq-1 must not overwrite seq-2: {candidates:?}"
    );
}

#[tokio::test]
async fn roots_list_error_keeps_the_previous_good_list() {
    let first = scratch("err-keep");
    plant_store(&first);
    let slot = Arc::new(StoreSlot::resolving(None, scratch("err-cwd"), None));
    slot.set_mcp_roots(vec![first.clone()]);
    assert!(slot.get().is_ok(), "precondition: good list opens");

    let err_reply = json!({
        "jsonrpc": "2.0",
        "id": "devmap-roots-3",
        "error": {"code": -32603, "message": "roots unavailable"}
    });
    assert!(handle_line(&slot, &err_reply.to_string()).await.is_none());

    assert!(
        slot.get().is_ok(),
        "an error reply must keep the last good roots list, not collapse it to empty"
    );
    let candidates = slot.candidate_roots();
    assert!(
        candidates
            .iter()
            .any(|root| root == &first.canonicalize().unwrap()),
        "previous good root must survive: {candidates:?}"
    );
}

#[test]
fn parse_roots_list_caps_at_max_and_reports_overflow() {
    let roots: Vec<Value> = (0..200)
        .map(|i| json!({"uri": format!("file:///tmp/devmap-overflow-{i}")}))
        .collect();
    let (paths, _skipped, overflow) =
        devmap_serve::mcp::parse_roots_list_result(&json!({"roots": roots}));
    assert_eq!(
        paths.len(),
        devmap_serve::root_resolve::MAX_ROOTS,
        "parse must cap at MAX_ROOTS"
    );
    assert_eq!(overflow, 200 - devmap_serve::root_resolve::MAX_ROOTS);
}
