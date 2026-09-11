//! The stdio transport under concurrent load.
//!
//! Requests stopped being processed one-at-a-time when cancellation was added:
//! each now runs in its own task, with writes serialized behind one lock. That
//! bought two things — a slow `impact` no longer blocks every other tool call on
//! the connection, and a `notifications/cancelled` can be *read* while the call
//! it cancels is still running — and it introduced two ways to be wrong that
//! serial processing could not have:
//!
//! 1. **Interleaved writes.** Two responses whose bytes cross produce one
//!    unparseable line, and the client loses both. Nothing in a low-volume
//!    manual test would show this; it needs many concurrent writers.
//! 2. **Lost or duplicated ids.** A client matches responses to pending requests
//!    by id. One missing id hangs that call forever; one duplicate resolves the
//!    wrong pending future.
//!
//! Both are asserted here on every response, not sampled.

use std::collections::HashSet;
use std::sync::Arc;

use devmap_serve::mcp::StoreSlot;
use devmap_store::Store;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// Requests fired down one connection before any response is read.
///
/// Large enough that the writer lock is genuinely contended and the tasks
/// finish out of order; small enough that the suite stays fast.
const IN_FLIGHT: usize = 200;

fn corpus() -> Arc<StoreSlot> {
    let files = [
        ("core.py", "def helper(rows):\n    return sum(rows)\n"),
        (
            "caller.py",
            "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
        ),
    ];
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| devmap_extract::extract_file(path, source))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");
    Arc::new(StoreSlot::ready("in-memory", Arc::new(store)))
}

/// Every request gets exactly one well-formed response, and no two responses
/// share a line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_never_interleave_or_lose_an_id() {
    let (client, server) = tokio::io::duplex(1024 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(devmap_serve::mcp::serve_streams(
        corpus(),
        tokio::io::BufReader::new(server_read),
        server_write,
    ));

    let (client_read, mut client_write) = tokio::io::split(client);

    // A deliberate mix: cheap methods that return immediately beside real
    // queries that take the store lock, so fast responses are racing slow ones
    // rather than all finishing in submission order.
    let mut sent = String::new();
    for id in 0..IN_FLIGHT {
        let frame = match id % 4 {
            0 => format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#),
            1 => format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list"}}"#),
            2 => format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"devmap_impact","arguments":{{"target":"helper"}}}}}}"#
            ),
            _ => format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"devmap_search","arguments":{{"query":"helper"}}}}}}"#
            ),
        };
        sent.push_str(&frame);
        sent.push('\n');
    }
    // Interleaved notifications, which must consume no response slot. If one
    // ever produced a frame, the id accounting below would find an extra.
    sent.push_str("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n");

    let pump = tokio::spawn(async move {
        client_write.write_all(sent.as_bytes()).await.unwrap();
        client_write.shutdown().await.unwrap();
    });

    let mut seen: HashSet<u64> = HashSet::new();
    let mut lines = tokio::io::BufReader::new(client_read).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        // The parse IS the interleaving assertion: two responses whose bytes
        // crossed do not produce valid JSON.
        let frame: Value = serde_json::from_str(&line).unwrap_or_else(|err| {
            panic!("a response line was not valid JSON ({err}); writes interleaved: {line}")
        });
        assert_eq!(frame["jsonrpc"], "2.0");
        let id = frame["id"]
            .as_u64()
            .unwrap_or_else(|| panic!("response carried no numeric id: {frame}"));
        assert!(
            seen.insert(id),
            "id {id} was answered twice; a client would resolve the wrong pending call"
        );
        assert!(
            frame.get("result").is_some(),
            "id {id} came back as an error under load: {frame}"
        );
    }

    pump.await.unwrap();
    task.await
        .unwrap()
        .expect("transport loop must exit cleanly");

    assert_eq!(
        seen.len(),
        IN_FLIGHT,
        "{} of {IN_FLIGHT} requests went unanswered; each one hangs its caller forever",
        IN_FLIGHT - seen.len()
    );
    for id in 0..IN_FLIGHT as u64 {
        assert!(seen.contains(&id), "id {id} was never answered");
    }
}

/// A malformed frame in the middle of a stream costs its own response and
/// nothing else.
///
/// The session used to die on the first non-UTF-8 byte, taking every later
/// request with it. This drives the failure modes *between* good requests and
/// asserts the good ones on both sides still answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_bad_frame_does_not_cost_the_session() {
    let (client, server) = tokio::io::duplex(256 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let task = tokio::spawn(devmap_serve::mcp::serve_streams(
        corpus(),
        tokio::io::BufReader::new(server_read),
        server_write,
    ));

    let (client_read, mut client_write) = tokio::io::split(client);
    let pump = tokio::spawn(async move {
        client_write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        // Invalid UTF-8.
        client_write.write_all(&[0xff, 0xfe, b'\n']).await.unwrap();
        // Not JSON.
        client_write.write_all(b"{not json at all\n").await.unwrap();
        // Valid JSON, invalid request.
        client_write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2}\n")
            .await
            .unwrap();
        client_write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        client_write.shutdown().await.unwrap();
    });

    let mut frames = Vec::new();
    let mut lines = tokio::io::BufReader::new(client_read).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        frames.push(serde_json::from_str::<Value>(&line).expect("every frame is JSON"));
    }
    pump.await.unwrap();
    task.await
        .unwrap()
        .expect("the session must survive bad frames");

    assert_eq!(
        frames.len(),
        5,
        "expected one frame per input line, got {}: {frames:?}",
        frames.len()
    );

    // The request after all three bad frames is the one that matters: it proves
    // the session was still alive. Looked up by id rather than taken as the last
    // frame — responses are produced concurrently and JSON-RPC guarantees no
    // ordering between them, so "last written" is not "last submitted".
    let after = frames
        .iter()
        .find(|f| f["id"] == 3)
        .expect("the request following the bad frames was lost with the session");
    assert!(
        after.get("result").is_some(),
        "the request after the bad frames failed: {after}"
    );

    // And the malformed request keeps its id, so its caller can resolve it.
    let invalid = frames
        .iter()
        .find(|f| f["id"] == 2)
        .expect("the invalid request must be answered against its own id");
    assert_eq!(invalid["error"]["code"], -32600);
}
