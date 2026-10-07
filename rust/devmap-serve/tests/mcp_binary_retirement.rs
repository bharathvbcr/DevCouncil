//! A running MCP server refuses to answer once its binary is replaced.
//!
//! The stdio server lives as long as its host session, and a reinstall never
//! reached it: seven of ten live `devmap mcp` processes on one machine were
//! found serving an old inode of the installed binary, some for three days. The
//! `serve` daemon retires on this condition; the MCP server now asks the same
//! predicate on every `tools/call` and refuses with the reason.
//!
//! Its own test file for the reason `daemon_binary_retirement.rs` gives: the
//! test moves the *running test binary's* modification time, which every other
//! server in the same process would read too. One test per file is one process.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use devmap_serve::mcp::serve_streams;
use devmap_serve::StoreSlot;
use devmap_store::Store;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Move a file's modification time forward, the way a reinstall does.
///
/// Opened read-only: a running executable cannot be opened for write
/// (`ETXTBSY`), and `futimens` with explicit times needs ownership of the file
/// rather than write permission on it.
fn advance_modification_time(path: &Path) {
    let file = std::fs::File::open(path).expect("the running executable must be readable");
    let modified = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .expect("the running executable must have a modification time");
    let later = modified + Duration::from_secs(5);
    let times = std::fs::FileTimes::new()
        .set_accessed(later)
        .set_modified(later);
    file.set_times(times)
        .expect("the test process owns its own executable");
}

fn indexed_slot() -> Arc<StoreSlot> {
    let files = [("src/app.py", "def checkout_cart():\n    return 1\n")];
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| devmap_extract::extract_file(path, source))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");
    Arc::new(StoreSlot::ready("in-memory", Arc::new(store)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_binary_makes_a_running_server_refuse_tool_calls() {
    let slot = indexed_slot();
    let (client, server) = tokio::io::duplex(256 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let serving = tokio::spawn(serve_streams(
        Arc::clone(&slot),
        tokio::io::BufReader::new(server_read),
        server_write,
    ));
    let (client_read, mut writer) = tokio::io::split(client);
    let mut lines = tokio::io::BufReader::new(client_read).lines();
    let search = |id: i64| {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": "devmap_search", "arguments": {"query": "checkout_cart"}}
        })
    };

    let before = ask(&mut writer, &mut lines, search(1)).await;
    assert_eq!(
        before["result"]["isError"],
        json!(false),
        "precondition: an unreplaced server answers: {before}"
    );

    advance_modification_time(&std::env::current_exe().unwrap());

    let after = ask(&mut writer, &mut lines, search(2)).await;
    assert_eq!(
        after["result"]["isError"],
        json!(true),
        "a server whose binary was replaced must refuse the call, not answer from the old \
         kernel: {after}"
    );
    let reason = after["result"]["content"][0]["text"]
        .as_str()
        .expect("the refusal carries its reason");
    assert!(
        reason.starts_with("binary_replaced:")
            && reason.contains(&format!("pid {}", std::process::id()))
            && reason.contains("Restart the MCP host"),
        "the refusal must name the condition, this process and the fix: {reason}"
    );

    // Only tool calls are refused. The connection itself stays healthy, so the
    // host can still list tools and ping while it gets restarted.
    let ping = ask(
        &mut writer,
        &mut lines,
        json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}),
    )
    .await;
    assert!(
        ping.get("error").is_none(),
        "ping must still answer: {ping}"
    );

    drop(writer);
    drop(lines);
    let _ = tokio::time::timeout(Duration::from_secs(5), serving).await;
}

/// Send one frame and read the one response it is owed.
async fn ask<W, R>(writer: &mut W, lines: &mut tokio::io::Lines<R>, frame: Value) -> Value
where
    W: tokio::io::AsyncWrite + Unpin,
    R: tokio::io::AsyncBufRead + Unpin,
{
    writer
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    writer.flush().await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .expect("the server must answer")
        .unwrap()
        .expect("a response frame");
    serde_json::from_str(&reply).expect("a JSON response")
}
