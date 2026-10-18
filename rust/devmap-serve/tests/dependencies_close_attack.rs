//! Closing probe 1 against `lane/deps-outbound` @ 41a0d9be: `note` is `deps`
//! only. Failing tests are findings; nothing is fixed here.

use std::sync::Arc;

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};

fn corpus() -> Arc<Store> {
    let files = [
        ("lib.py", "RETRY = 2\n\n\ndef helper():\n    return 1\n\n\ndef worker():\n    helper()\n    return RETRY\n"),
        ("callers.py", "from lib import worker\n\n\ndef caller():\n    return worker()\n"),
    ];
    let extractions: Vec<_> =
        files.iter().map(|(p, s)| devmap_extract::extract_file(p, s)).collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store.save_generation(&extractions, &resolution, &analysis).unwrap();
    Arc::new(store)
}

async fn call(slot: &Arc<StoreSlot>, id: u64, method: &str, params: Value) -> Value {
    let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    handle_line(slot, &frame.to_string()).await.expect("frame")
}

/// Probe 1. Every MCP tool that answers must omit `note` (not null it) unless
/// it is `devmap_dependencies`; and `devmap_dependencies` over a callers-only
/// symbol must answer (its schema admits the note) with a string note.
#[tokio::test]
async fn p1_note_appears_only_on_dependencies_and_never_as_null() {
    let store = corpus();
    let slot = Arc::new(StoreSlot::ready("in-memory", store));
    let listed = call(&slot, 1, "tools/list", json!({})).await;
    let tools = listed["result"]["tools"].as_array().expect("tools").clone();
    let mut report = String::new();
    let mut answered = 0;
    for tool in tools {
        let name = tool["name"].as_str().unwrap().to_string();
        let mut args = serde_json::Map::new();
        let props = &tool["inputSchema"]["properties"];
        for req in tool["inputSchema"]["required"].as_array().cloned().unwrap_or_default() {
            let key = req.as_str().unwrap();
            let value = match props[key]["type"].as_str() {
                Some("array") => json!(["worker"]),
                Some("integer") => json!(1),
                _ => json!("worker"),
            };
            args.insert(key.to_string(), value);
        }
        if name == "devmap_dependencies" {
            args.insert("target".into(), json!("RETRY"));
        }
        let reply = call(&slot, 2, "tools/call", json!({"name": name, "arguments": args})).await;
        let result = &reply["result"];
        if name == "devmap_dependencies" {
            if result["isError"] == json!(true) {
                report.push_str(&format!("{name}: refused its own note: {result}\n"));
            } else if !result["structuredContent"]["note"].is_string() {
                report.push_str(&format!("{name}: no string note: {}\n", result["structuredContent"]));
            }
            continue;
        }
        if result["isError"] == json!(true) {
            continue;
        }
        answered += 1;
        let mut stack = vec![&result["structuredContent"]];
        while let Some(v) = stack.pop() {
            match v {
                Value::Object(o) => {
                    if o.contains_key("note") {
                        report.push_str(&format!("{name}: carries note {:?}\n", o["note"]));
                    }
                    stack.extend(o.values());
                }
                Value::Array(a) => stack.extend(a.iter()),
                _ => {}
            }
        }
    }
    assert!(answered >= 3, "only {answered} other tools answered");
    assert!(report.is_empty(), "{report}");
}
