//! Probe 8 against `lane/deps-outbound` @ 55a1c302: `devmap_dependencies` (MCP),
//! the socket protocol's `deps`, and the engine itself must return the same
//! payload for the same query. Failing tests are findings; nothing is fixed here.

use std::sync::Arc;

use devmap_query::{Request, StoreQueryEngine};
use devmap_serve::handle_stream;
use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn corpus() -> Arc<Store> {
    let files = [
        (
            "lib.py",
            "RETRY = 2\n\n\ndef helper():\n    return 1\n\n\ndef worker():\n    helper()\n    return RETRY\n",
        ),
        ("callers.py", "from lib import worker\n\n\ndef caller():\n    return worker()\n"),
        ("a.py", "def dup():\n    return 1\n"),
        ("b.py", "def dup():\n    return 2\n"),
    ];
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
    Arc::new(store)
}

/// What each surface says: the answer payload, or the refusal text.
#[derive(Debug, PartialEq)]
enum Said {
    Answer(Value),
    Refused(String),
}

fn strip_volatile(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        // Freshness is stamped per call.
        o.remove("source_freshness");
        o.remove("tokens_used");
        // The MCP surface stamps which repository answered; that is envelope,
        // not payload.
        o.remove("repository");
    }
    v
}

fn engine_says(store: &Store, query: &str, budget: u32) -> Said {
    match StoreQueryEngine::new(store).dependencies_at_rung(
        Request {
            query: query.to_string(),
            token_budget: budget,
            min_confidence: 0.0,
            max_depth: 1,
        },
        None,
    ) {
        Ok(r) => Said::Answer(strip_volatile(serde_json::to_value(r).unwrap())),
        Err(e) => Said::Refused(e.to_string()),
    }
}

async fn mcp_says(store: &Arc<Store>, query: &str, budget: u32) -> Said {
    let slot = Arc::new(StoreSlot::ready("in-memory", store.clone()));
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "devmap_dependencies", "arguments": {"target": query, "budget": budget}}
    });
    let response = handle_line(&slot, &frame.to_string()).await.expect("frame");
    let result = &response["result"];
    if result["isError"] == json!(true) {
        return Said::Refused(result["content"][0]["text"].as_str().unwrap_or_default().to_string());
    }
    Said::Answer(strip_volatile(result["structuredContent"].clone()))
}

async fn ipc_says(store: &Arc<Store>, query: &str, budget: u32) -> Said {
    let frame = json!({"version": 1, "cmd": "deps", "target": query, "budget": budget}).to_string();
    let (client, server) = tokio::io::duplex(1 << 20);
    let task = tokio::spawn(handle_stream(server, store.clone()));
    let (mut reader, mut writer) = tokio::io::split(client);
    let _ = writer.write_all(frame.as_bytes()).await;
    let _ = writer.shutdown().await;
    let mut reply = Vec::new();
    reader.read_to_end(&mut reply).await.unwrap();
    let _ = task.await;
    let value: Value = serde_json::from_slice(&reply[..reply.len() - 1]).unwrap();
    if value["ok"] == json!(true) {
        Said::Answer(strip_volatile(value["result"].clone()))
    } else {
        Said::Refused(value["error"]["message"].as_str().unwrap_or_default().to_string())
    }
}

fn same(a: &Said, b: &Said) -> bool {
    match (a, b) {
        (Said::Refused(a), Said::Refused(b)) => a.contains(b.as_str()) || b.contains(a.as_str()),
        (a, b) => a == b,
    }
}

/// Probe 8. Same query, same payload across the engine, the socket protocol
/// and MCP, for file, symbol, qualified, ambiguous, unknown and path-spelled
/// queries, at two budgets.
#[tokio::test]
async fn p8_engine_protocol_and_mcp_return_identical_payloads() {
    let store = corpus();
    let mut report = String::new();
    for query in [
        "lib.py",
        "worker",
        "RETRY",
        "lib.py::worker",
        "dup",
        "a.py::dup",
        "nope",
        "./lib.py",
        "callers.py",
        "helper",
    ] {
        for budget in [8_000u32, 1] {
            let engine = engine_says(&store, query, budget);
            let ipc = ipc_says(&store, query, budget).await;
            let mcp = mcp_says(&store, query, budget).await;
            if !same(&engine, &ipc) {
                report.push_str(&format!(
                    "  {query:?} b={budget}: engine != ipc\n    engine={engine:?}\n    ipc={ipc:?}\n"
                ));
            }
            if !same(&engine, &mcp) {
                report.push_str(&format!(
                    "  {query:?} b={budget}: engine != mcp\n    engine={engine:?}\n    mcp={mcp:?}\n"
                ));
            }
        }
    }
    assert!(report.is_empty(), "surfaces disagree:\n{report}");
}

/// Probe 5/8. A complete-and-empty answer reaches an MCP client with
/// `walk_incomplete` set; the field is documented as "None on a complete
/// answer". Asserts the contract; its failure is the finding.
#[tokio::test]
async fn p5_mcp_complete_empty_answer_carries_no_walk_incomplete() {
    let store = corpus();
    let Said::Answer(answer) = mcp_says(&store, "RETRY", 8_000).await else {
        panic!("RETRY must answer");
    };
    assert_eq!(answer["items"], json!([]));
    assert!(
        answer.get("walk_incomplete").is_none(),
        "complete empty answer flagged walk_incomplete: {}",
        answer["walk_incomplete"]
    );
}
