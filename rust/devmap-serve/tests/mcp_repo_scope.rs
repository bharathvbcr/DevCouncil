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

#[tokio::test]
#[cfg(unix)]
async fn raw_state_directory_aliases_are_refused_before_canonicalization() {
    let base = scratch("linked-state");
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    let db = plant_store(&root);
    let normal = Arc::new(StoreSlot::new(db.clone()));
    assert_eq!(
        call(&normal, "devmap_status", json!({})).await["result"]["isError"],
        false
    );
    drop(normal);
    let state = db.parent().unwrap().parent().unwrap();
    let outside = base.join("outside");
    std::fs::rename(state, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, state).unwrap();
    let log = outside.join("codeintel/sessions/live.jsonl");
    let before = std::fs::read(&log).ok();
    let slots = [
        StoreSlot::new(db.clone()),
        StoreSlot::resolving(Some(db), base.clone(), None),
        StoreSlot::resolving(None, base.clone(), Some(root.clone())),
        StoreSlot::resolving(None, base.clone(), None),
    ];
    for slot in slots {
        let response = call(&Arc::new(slot), "devmap_status", json!({"repo_path":root})).await;
        assert!(!tool_error_text(&response).is_empty());
        assert_eq!(std::fs::read(&log).ok(), before);
    }
    std::fs::remove_dir_all(base).unwrap();
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
        names(&text, &via_roots) && names(&text, &via_cwd),
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
        names(&text, &a) && names(&text, &b),
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

#[cfg(not(windows))]
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
                    names(&text, &via_roots) && names(&text, &via_cwd),
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
        names(&text, &pinned)
            || text.contains(&pinned.canonicalize().unwrap().display().to_string()),
        "error must name the --root pin: {text}"
    );
    assert!(
        names(&text, &foreign)
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
            "roots": [{"uri": file_uri(&newer)}]
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
            "roots": [{"uri": file_uri(&older)}]
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
        // In the host's own spelling: a Windows server refuses a drive-less
        // `file:///tmp/...` as a unix path, which left nothing to cap.
        .map(|i| json!({"uri": file_uri(&std::env::temp_dir().join(format!("devmap-overflow-{i}")))}))
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

/// The same rule from the other side: a Windows server does not resolve a
/// drive-less unix path against its current drive.
#[cfg(windows)]
#[tokio::test]
async fn unix_file_uri_is_rejected_not_reinterpreted_on_windows() {
    let native = scratch("uri-native");
    plant_store(&native);
    let slot = Arc::new(StoreSlot::resolving(None, native.clone(), None));
    let response = json!({
        "jsonrpc": "2.0",
        "id": ROOTS_LIST_REQUEST_ID,
        "result": {
            "roots": [{"uri": "file:///Users/nobody/project"}]
        }
    });
    assert!(handle_line(&slot, &response.to_string()).await.is_none());
    assert!(
        !slot
            .candidate_roots()
            .iter()
            .any(|root| root.to_string_lossy().contains("nobody")),
        "a unix URI must not become a root on Windows"
    );
}

fn wire_registered_worktree(main: &Path, name: &str, wt: &Path) {
    let wt_gitdir = main.join(".git").join("worktrees").join(name);
    std::fs::create_dir_all(&wt_gitdir).unwrap();
    std::fs::write(main.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(wt).unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", wt_gitdir.display()),
    )
    .unwrap();
    std::fs::write(wt_gitdir.join("commondir"), "../..\n").unwrap();
    std::fs::write(
        wt_gitdir.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
}

#[tokio::test]
async fn explicit_root_pin_accepts_registered_worktrees_and_rejects_gitlink_forgery() {
    let base = scratch("pin-wt-family");
    let main = base.join("main");
    let wt = base.join("wt-lane");
    let forged = base.join("forged-lane");

    wire_registered_worktree(&main, "wt-lane", &wt);
    plant_store(&main);
    plant_store(&wt);

    std::fs::create_dir_all(&forged).unwrap();
    std::fs::write(
        forged.join(".git"),
        format!(
            "gitdir: {}\n",
            main.join(".git/worktrees/wt-lane").display()
        ),
    )
    .unwrap();
    plant_store(&forged);

    let slot = Arc::new(StoreSlot::resolving(None, main.clone(), Some(main.clone())));

    let ok = call(
        &slot,
        "devmap_status",
        json!({"repo_path": wt.display().to_string()}),
    )
    .await;
    assert_eq!(
        ok["result"]["isError"],
        json!(false),
        "a bidirectionally registered worktree of --root must be served: {ok}"
    );
    assert_eq!(
        Path::new(structured(&ok)["repository"]["root"].as_str().unwrap()),
        wt.canonicalize().unwrap().as_path()
    );

    let bad = call(
        &slot,
        "devmap_status",
        json!({"repo_path": forged.display().to_string()}),
    )
    .await;
    let err = tool_error_text(&bad);
    assert!(
        names(&err, &forged) && names(&err, &main),
        "forged gitlink must be refused against --root pin: {err}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn unindexed_worktree_in_mcp_roots_or_cwd_never_silently_leaks_anchor_store() {
    let base = scratch("f3a-leak-guard");
    let anchor = base.join("anchor");
    let sibling_wt = base.join("wt-unindexed");
    let nested_wt = anchor.join(".claude").join("worktrees").join("lane");
    let anchor_subdir = anchor.join("packages").join("ui");

    wire_registered_worktree(&anchor, "wt-unindexed", &sibling_wt);
    wire_registered_worktree(&anchor, "lane", &nested_wt);
    std::fs::create_dir_all(&anchor_subdir).unwrap();
    plant_store(&anchor);

    // 1. mcp_roots = [anchor, unindexed_wt] with the cwd at the anchor: the one
    //    indexed root is served (an extra root is not an ambiguity), and the
    //    answer names the anchor.
    let multi_slot = Arc::new(StoreSlot::resolving(None, anchor.clone(), None));
    multi_slot.set_mcp_roots(vec![anchor.clone(), sibling_wt.clone()]);
    let multi_ok = call(&multi_slot, "devmap_status", json!({})).await;
    assert_eq!(multi_ok["result"]["isError"], json!(false), "{multi_ok}");
    assert_eq!(
        Path::new(
            structured(&multi_ok)["repository"]["root"]
                .as_str()
                .unwrap()
        ),
        anchor.canonicalize().unwrap().as_path()
    );

    // 2. Critique 2b: mcp_roots = [anchor], client_cwd = sibling_wt (no store) must refuse.
    let cwd_wt_slot = Arc::new(StoreSlot::resolving(None, sibling_wt.clone(), None));
    cwd_wt_slot.set_mcp_roots(vec![anchor.clone()]);
    let cwd_err = tool_error_text(&call(&cwd_wt_slot, "devmap_status", json!({})).await);
    assert!(
        names(&cwd_err, &anchor) && names(&cwd_err, &sibling_wt),
        "must refuse leaking anchor store to unindexed sibling worktree cwd: {cwd_err}"
    );

    // 3. Critique 2b nested: mcp_roots = [anchor], client_cwd = <anchor>/.claude/worktrees/lane must refuse.
    let nested_slot = Arc::new(StoreSlot::resolving(None, nested_wt.clone(), None));
    nested_slot.set_mcp_roots(vec![anchor.clone()]);
    let nested_err = tool_error_text(&call(&nested_slot, "devmap_status", json!({})).await);
    assert!(
        names(&nested_err, &anchor) && names(&nested_err, &nested_wt),
        "must refuse leaking anchor store to nested unindexed worktree cwd: {nested_err}"
    );

    // 4. Normal subdirectory inside anchor (same worktree) must still succeed!
    let subdir_slot = Arc::new(StoreSlot::resolving(None, anchor_subdir.clone(), None));
    subdir_slot.set_mcp_roots(vec![anchor.clone(), anchor_subdir.clone()]);
    let subdir_ok = call(&subdir_slot, "devmap_status", json!({})).await;
    assert_eq!(
        subdir_ok["result"]["isError"],
        json!(false),
        "normal monorepo subdirectory of anchor must resolve cleanly: {subdir_ok}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn stacked_worktree_lease_answers_mcp_with_delta_overlay_and_fails_closed_on_corrupt_lease() {
    let base = scratch("stacked-mcp-lease");
    let anchor = base.join("anchor");
    let stacked_wt = base.join("wt-stacked");
    // A real repository: the donor must be a built, committed, clean checkout.
    std::fs::create_dir_all(&anchor).unwrap();
    std::fs::write(anchor.join("lib.rs"), "pub fn base_only() -> i32 { 1 }\n").unwrap();
    let git = |dir: &Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@example.invalid"])
            .args(["-c", "commit.gpgSign=false", "-c", "core.hooksPath="])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&anchor, &["init", "-q", "-b", "main"]);
    git(&anchor, &["add", "."]);
    git(&anchor, &["commit", "-qm", "base"]);
    git(
        &anchor,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            stacked_wt.to_str().unwrap(),
        ],
    );
    let head = git(&anchor, &["rev-parse", "HEAD"]);

    // Build a committed generation in anchor's store so publish_base_snapshot can snapshot it.
    let anchor_db = devmap_extract::paths::store_path(&anchor);
    std::fs::create_dir_all(anchor_db.parent().unwrap()).unwrap();
    {
        let store = devmap_store::Store::open(&anchor_db).unwrap();
        let anchor_canon = anchor.canonicalize().unwrap();
        store.bind_repo_root(&anchor_canon).unwrap();
        let exts = vec![devmap_extract::extract_file(
            "lib.rs",
            "pub fn base_only() -> i32 { 1 }\n",
        )];
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&exts);
        let res = resolver.resolve_all(&exts).unwrap();
        let analysis = devmap_analyze::analyze(&exts, &res);
        store
            .save_generation_with_metadata(
                &exts,
                &res,
                &analysis,
                devmap_store::GenerationWriteOpts::default(),
                &head,
            )
            .unwrap();
    }

    devmap_query::stacked::provision_stacked_worktree(&anchor, &stacked_wt).unwrap();
    assert!(
        !devmap_extract::paths::store_path(&stacked_wt).exists(),
        "stacked worktree must not create local devmap.sqlite"
    );

    // Edit stacked_wt/lib.rs and sync delta via real Tree-sitter!
    std::fs::write(
        stacked_wt.join("lib.rs"),
        "pub fn stacked_delta_fn() -> i32 { 42 }\n",
    )
    .unwrap();
    {
        let mut s = devmap_query::stacked::StackedStore::open(&stacked_wt).unwrap();
        s.sync_file_from_disk("lib.rs").unwrap();
    }

    let slot = Arc::new(StoreSlot::resolving(
        None,
        stacked_wt.clone(),
        Some(anchor.clone()),
    ));
    let status = call(
        &slot,
        "devmap_status",
        json!({"repo_path": stacked_wt.display().to_string()}),
    )
    .await;
    assert_eq!(status["result"]["isError"], json!(false), "{status}");
    assert_eq!(
        Path::new(
            structured(&status)["repository"]["root"]
                .as_str()
                .unwrap_or_default()
        ),
        stacked_wt.canonicalize().unwrap().as_path()
    );
    assert_eq!(structured(&status)["stacked"]["dirty_files"], json!(1));

    let search_new = call(
        &slot,
        "devmap_search",
        json!({"repo_path": stacked_wt.display().to_string(), "query": "stacked_delta_fn"}),
    )
    .await;
    assert_eq!(
        search_new["result"]["isError"],
        json!(false),
        "{search_new}"
    );
    assert_eq!(structured(&search_new)["shown"], json!(1), "{search_new}");

    let search_tombstoned = call(
        &slot,
        "devmap_search",
        json!({"repo_path": stacked_wt.display().to_string(), "query": "base_only"}),
    )
    .await;
    assert_eq!(
        structured(&search_tombstoned)["shown"],
        json!(0),
        "base_only was removed in delta edit and must be masked: {search_tombstoned}"
    );

    // Every non-status answer on a dirty lane says it is read from the base plus a
    // delta, on the lane's own repository, not silently as if it were complete.
    let skeleton = call(
        &slot,
        "devmap_skeleton",
        json!({"repo_path": stacked_wt.display().to_string(), "file": "lib.rs"}),
    )
    .await;
    assert_eq!(skeleton["result"]["isError"], json!(false), "{skeleton}");
    assert_eq!(
        Path::new(
            structured(&skeleton)["repository"]["root"]
                .as_str()
                .unwrap_or_default()
        ),
        stacked_wt.canonicalize().unwrap().as_path()
    );
    let incomplete = structured(&skeleton)["walk_incomplete"]
        .as_str()
        .unwrap_or_default();
    assert!(
        incomplete.contains("stacked worktree delta"),
        "a skeleton answered from a stacked base must carry the delta note: {skeleton}"
    );

    // The merged search keeps the base's own total and carries the note too.
    assert!(
        structured(&search_new)["total"].as_u64().unwrap_or(0) >= 1,
        "{search_new}"
    );

    // Corrupting stack_lease.json must fail closed instead of falling back!
    let lease_path = devmap_extract::paths::stack_lease_path(&stacked_wt);
    let lease_bytes = std::fs::read(&lease_path).unwrap();
    std::fs::write(&lease_path, b"{not valid json").unwrap();
    let corrupt_err = tool_error_text(
        &call(
            &slot,
            "devmap_status",
            json!({"repo_path": stacked_wt.display().to_string()}),
        )
        .await,
    );
    assert!(
        corrupt_err.contains("corrupt") || corrupt_err.contains("stack_lease.json"),
        "corrupt stack_lease.json must fail closed: {corrupt_err}"
    );

    // A store beside a valid lease (an interrupted materialize): a read refuses
    // with the recovery command and deletes nothing.
    std::fs::write(&lease_path, &lease_bytes).unwrap();
    std::fs::copy(&anchor_db, devmap_extract::paths::store_path(&stacked_wt)).unwrap();
    let stale_err = tool_error_text(
        &call(
            &slot,
            "devmap_status",
            json!({"repo_path": stacked_wt.display().to_string()}),
        )
        .await,
    );
    assert!(stale_err.contains("devmap seed --sync-delta"), "{stale_err}");
    assert!(lease_path.is_file(), "an MCP read must not delete lane state");

    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn extra_non_git_mcp_root_does_not_block_the_indexed_root() {
    let base = scratch("extra-root");
    let indexed = base.join("indexed");
    let plain = base.join("plain-dir");
    std::fs::create_dir_all(&plain).unwrap();
    plant_store(&indexed);
    let slot = Arc::new(StoreSlot::resolving(None, indexed.clone(), None));
    slot.set_mcp_roots(vec![indexed.clone(), plain.clone()]);
    let ok = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(ok["result"]["isError"], json!(false), "{ok}");
    assert_eq!(
        Path::new(structured(&ok)["repository"]["root"].as_str().unwrap()),
        indexed.canonicalize().unwrap().as_path()
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn corrupt_lease_in_another_root_does_not_block_resolution() {
    let base = scratch("corrupt-lease-other-root");
    let indexed = base.join("indexed");
    let broken = base.join("broken-lane");
    plant_store(&indexed);
    let lease = devmap_extract::paths::stack_lease_path(&broken);
    std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
    std::fs::write(&lease, "{corrupt").unwrap();
    let slot = Arc::new(StoreSlot::resolving(None, indexed.clone(), None));
    slot.set_mcp_roots(vec![indexed.clone(), broken.clone()]);
    let ok = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(ok["result"]["isError"], json!(false), "{ok}");
    assert_eq!(
        Path::new(structured(&ok)["repository"]["root"].as_str().unwrap()),
        indexed.canonicalize().unwrap().as_path()
    );
    // Naming the corrupt lease explicitly still fails closed.
    let explicit = call(
        &slot,
        "devmap_status",
        json!({"repo_path": broken.display().to_string()}),
    )
    .await;
    assert!(tool_error_text(&explicit).contains("corrupt"), "{explicit}");
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test]
async fn nested_unregistered_repo_cwd_is_served_from_the_root() {
    let base = scratch("nested-repo-cwd");
    let indexed = base.join("indexed");
    let nested = indexed.join("vendor").join("dep");
    std::fs::create_dir_all(&nested).unwrap();
    plant_store(&indexed);
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&nested)
        .output()
        .unwrap();
    assert!(init.status.success());
    let slot = Arc::new(StoreSlot::resolving(None, nested.clone(), None));
    slot.set_mcp_roots(vec![indexed.clone()]);
    let ok = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(ok["result"]["isError"], json!(false), "{ok}");
    assert_eq!(
        Path::new(structured(&ok)["repository"]["root"].as_str().unwrap()),
        indexed.canonicalize().unwrap().as_path()
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[tokio::test]
async fn the_same_repo_cwd_refusal_holds_via_symlink_dotdot_and_case() {
    let base = scratch("cwd-refusal-spellings");
    let anchor = base.join("anchor");
    let sibling = base.join("wt-unindexed");
    wire_registered_worktree(&anchor, "wt-unindexed", &sibling);
    plant_store(&anchor);

    let link = base.join("link-to-sibling");
    std::os::unix::fs::symlink(&sibling, &link).unwrap();
    let dotdot = base.join("anchor").join("..").join("wt-unindexed");
    let mut spellings = vec![("symlink", link), ("dotdot", dotdot)];
    // On a case-insensitive filesystem (APFS default) another case names the
    // same directory; on a case-sensitive one it does not exist and is skipped.
    let upper = base.join("WT-UNINDEXED");
    if upper.is_dir() {
        spellings.push(("case", upper));
    }
    for (label, cwd) in spellings {
        let slot = Arc::new(StoreSlot::resolving(None, cwd.clone(), None));
        slot.set_mcp_roots(vec![anchor.clone()]);
        let response = call(&slot, "devmap_status", json!({})).await;
        assert_eq!(
            response["result"]["isError"],
            json!(true),
            "{label}: a cwd in another registered worktree of the same repo must be refused: {response}"
        );
        let text = tool_error_text(&response);
        assert!(names(&text, &anchor), "{label}: {text}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

/// The slot caches a `Store` per path and re-checks only the file's identity,
/// so an in-place `devmap build` (same inode) never evicts it. The cached
/// connection must therefore follow the writer: an `?immutable=1` open of a
/// quiescent WAL store answers from the generation it first read.
#[test]
fn a_cached_store_follows_a_build_that_writes_in_place() {
    use std::os::unix::fs::MetadataExt;
    let root = scratch("in-place-rebuild");
    let db = plant_store(&root);
    let inode = std::fs::metadata(&db).unwrap().ino();
    let slot = StoreSlot::new(&db);
    let first = slot.get().expect("open through the MCP slot");
    assert!(first.get_pending_paths().unwrap().is_empty());

    let writer = Store::open(&db).expect("writer attaches in place");
    writer
        .enqueue_pending_paths(&["src/written_after_the_slot_opened.rs".into()])
        .unwrap();
    drop(writer);
    assert_eq!(std::fs::metadata(&db).unwrap().ino(), inode);

    let again = slot.get().expect("cache hit on an unchanged inode");
    assert!(
        Arc::ptr_eq(&first, &again),
        "precondition: the same inode is a cache hit, not a reopen"
    );
    assert_eq!(
        again.get_pending_paths().unwrap(),
        vec!["src/written_after_the_slot_opened.rs".to_string()],
        "the cached store must see what an in-place build committed"
    );
}

/// Every regular file under `dir`: relative path, length, mtime and bytes.
fn state_snapshot(
    dir: &Path,
) -> std::collections::BTreeMap<PathBuf, (u64, std::time::SystemTime, Vec<u8>)> {
    let mut out = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                pending.push(path);
            } else {
                let rel = path.strip_prefix(dir).unwrap().to_path_buf();
                out.insert(
                    rel,
                    (
                        meta.len(),
                        meta.modified().unwrap(),
                        std::fs::read(&path).unwrap(),
                    ),
                );
            }
        }
    }
    out
}

#[tokio::test]
async fn mcp_stacked_graph_tools_merge_delta_with_zero_writes_and_refuse_legacy_pointer() {
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

    let base = scratch("mcp-stacked-zero-write");
    let anchor = base.join("anchor");
    let stacked_wt = base.join("stacked-wt");
    let legacy_wt = base.join("legacy-wt");
    std::fs::create_dir_all(&anchor).unwrap();
    git(&anchor, &["init", "-q", "-b", "main"]);
    std::fs::write(
        anchor.join("lib.rs"),
        "pub fn shared_target() -> i32 { 1 }\npub fn base_caller() -> i32 { shared_target() }\n",
    )
    .unwrap();
    git(&anchor, &["add", "lib.rs"]);
    git(&anchor, &["commit", "-qm", "init"]);
    for wt in [&stacked_wt, &legacy_wt] {
        git(
            &anchor,
            &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()],
        );
    }
    let head = git(&anchor, &["rev-parse", "HEAD"]);

    let anchor_db = devmap_extract::paths::store_path(&anchor);
    std::fs::create_dir_all(anchor_db.parent().unwrap()).unwrap();
    {
        let store = devmap_store::Store::open(&anchor_db).unwrap();
        store
            .bind_repo_root(&anchor.canonicalize().unwrap())
            .unwrap();
        let src = std::fs::read_to_string(anchor.join("lib.rs")).unwrap();
        let exts = vec![devmap_extract::extract_file("lib.rs", &src)];
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&exts);
        let res = resolver.resolve_all(&exts).unwrap();
        let analysis = devmap_analyze::analyze(&exts, &res);
        store
            .save_generation_with_metadata(
                &exts,
                &res,
                &analysis,
                devmap_store::GenerationWriteOpts::default(),
                &head,
            )
            .unwrap();
        let _ = store.write_latest_csr_sidecar().unwrap();
    }

    devmap_query::stacked::provision_stacked_worktree(&anchor, &stacked_wt).unwrap();
    let lease_path = devmap_extract::paths::stack_lease_path(&stacked_wt);
    let state = lease_path.parent().unwrap().to_path_buf();

    // The delta drops `base_caller` and adds `delta_caller() -> shared_target()`.
    std::fs::write(
        stacked_wt.join("lib.rs"),
        "pub fn shared_target() -> i32 { 1 }\npub fn delta_caller() -> i32 { shared_target() }\n",
    )
    .unwrap();
    {
        let mut s = devmap_query::stacked::StackedStore::open(&stacked_wt).unwrap();
        s.sync_file_from_disk("lib.rs").unwrap();
    }
    // A second edit that is on disk only: a read must not ingest it.
    std::fs::write(
        stacked_wt.join("unsynced.rs"),
        "pub fn unsynced_fn() -> i32 { shared_target() }\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let before = state_snapshot(&state);
    assert!(
        before.contains_key(Path::new("delta.sqlite"))
            && before.contains_key(Path::new("stack_lease.json"))
            && before.keys().any(|p| p.starts_with("stack")),
        "the fixture must hold a delta, a lease and a stack/ dir: {:?}",
        before.keys().collect::<Vec<_>>()
    );

    let slot = Arc::new(StoreSlot::resolving(
        None,
        stacked_wt.clone(),
        Some(anchor.clone()),
    ));
    let repo = stacked_wt.display().to_string();

    let impact = call(
        &slot,
        "devmap_impact",
        json!({"repo_path": repo, "target": "shared_target"}),
    )
    .await;
    assert_eq!(impact["result"]["isError"], json!(false), "{impact}");
    let callers: Vec<&str> = structured(&impact)["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["source_symbol"].as_str())
        .collect();
    assert!(
        callers.iter().any(|c| c.contains("delta_caller")),
        "impact of a base callee must list the delta caller: {impact}"
    );
    assert!(
        !callers.iter().any(|c| c.contains("base_caller")),
        "impact must mask the caller the delta removed: {impact}"
    );

    let search = call(
        &slot,
        "devmap_search",
        json!({"repo_path": repo, "query": "delta_caller"}),
    )
    .await;
    assert_eq!(search["result"]["isError"], json!(false), "{search}");
    assert_eq!(structured(&search)["shown"], json!(1), "{search}");
    let unsynced = call(
        &slot,
        "devmap_search",
        json!({"repo_path": repo, "query": "unsynced_fn"}),
    )
    .await;
    assert_eq!(structured(&unsynced)["shown"], json!(0), "{unsynced}");

    let skeleton = call(
        &slot,
        "devmap_skeleton",
        json!({"repo_path": repo, "file": "lib.rs"}),
    )
    .await;
    assert_eq!(skeleton["result"]["isError"], json!(false), "{skeleton}");
    let skeleton_text = structured(&skeleton).to_string();
    assert!(
        skeleton_text.contains("delta_caller") && !skeleton_text.contains("base_caller"),
        "skeleton must show the edit, not the base: {skeleton}"
    );

    let explore = call(
        &slot,
        "devmap_explore",
        json!({"repo_path": repo, "query": "delta_caller"}),
    )
    .await;
    assert_eq!(explore["result"]["isError"], json!(false), "{explore}");
    assert_eq!(
        structured(&explore)["definitions"]["shown"],
        json!(1),
        "{explore}"
    );

    assert_eq!(
        state_snapshot(&state),
        before,
        "an MCP read changed a byte or an mtime under the lane's state dir (store, lease, delta, stack/)"
    );

    // A retired pointer on a lane with no store and no lease fails closed, by name.
    let legacy_ref = legacy_wt.join(".devmap/codeintel/base_store_ref.json");
    std::fs::create_dir_all(legacy_ref.parent().unwrap()).unwrap();
    std::fs::write(
        &legacy_ref,
        json!({"anchor_repo_root": anchor, "base_sqlite_path": anchor_db}).to_string(),
    )
    .unwrap();
    let legacy_resp = call(
        &slot,
        "devmap_search",
        json!({"repo_path": legacy_wt.display().to_string(), "query": "shared_target"}),
    )
    .await;
    assert_eq!(
        legacy_resp["result"]["isError"],
        json!(true),
        "{legacy_resp}"
    );
    let legacy_err = tool_error_text(&legacy_resp);
    assert!(
        legacy_err.contains("seed --stacked --from"),
        "the refusal must name the upgrade command: {legacy_err}"
    );
    assert!(!devmap_extract::paths::stack_lease_path(&legacy_wt).exists());
    assert!(!devmap_extract::paths::stack_delta_path(&legacy_wt).exists());

    let _ = std::fs::remove_dir_all(&base);
}
