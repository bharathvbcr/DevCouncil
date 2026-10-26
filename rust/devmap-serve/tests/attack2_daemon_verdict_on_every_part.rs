//! Closing attack on ft-b79f84e2 (`StoreQueryEngine::exit`): the daemon (IPC
//! socket) and the MCP tools wrap the engine, so every part of every answer they
//! return must carry the store's verdict, not only the top-level envelope.
//!
//! The store is stale (a tracked file changed after the generation was built and
//! the verdict was verified). Every JSON object shaped like a `Response` (it has
//! `items`, `shown` and `hidden`) must carry `source_freshness.fresh == false`,
//! and no `source_freshness` anywhere may say "not checked".

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::{GenerationWriteOpts, Store};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const FILES: [(&str, &str); 4] = [
    ("core.py", "def helper(rows):\n    return sum(rows)\n"),
    (
        "caller.py",
        "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
    ),
    (
        "outer.py",
        "from caller import run\n\n\ndef outer(rows):\n    return run(rows)\n",
    ),
    (
        "test_top.py",
        "from outer import outer\n\n\ndef test_top():\n    assert outer([1]) == 1\n",
    ),
];

fn stale_store() -> (PathBuf, Arc<Store>) {
    let root = std::env::temp_dir().join(format!("devmap-attack2-daemon-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    for (name, body) in FILES {
        fs::write(root.join(name), body).unwrap();
    }
    let extractions: Vec<_> = FILES
        .iter()
        .map(|(path, source)| devmap_extract::extract_file(path, source))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    let fresh = store.status("t").unwrap();
    assert_eq!(fresh.source_freshness, Some(true), "{fresh:?}");
    fs::write(root.join("core.py"), "def helper(rows):\n    return 0\n").unwrap();
    fs::write(root.join("brand_new.py"), "def z():\n    return 1\n").unwrap();
    let stale = store.status("t").unwrap();
    assert_eq!(stale.source_freshness, Some(false), "{stale:?}");
    (root, Arc::new(store))
}

/// Every `Response`-shaped object under `value`, with its path, and whether it
/// carries a verdict that is a verified "stale".
fn audit(value: &Value, path: &str, bad: &mut Vec<String>, seen: &mut usize) {
    match value {
        Value::Object(map) => {
            let shaped = map.contains_key("items")
                && map.contains_key("shown")
                && map.contains_key("hidden");
            if shaped {
                *seen += 1;
                match map.get("source_freshness") {
                    Some(f) if f["fresh"] == json!(false) => {}
                    other => bad.push(format!("{path}: {other:?}")),
                }
            }
            if let Some(f) = map.get("source_freshness") {
                if f["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("not checked")
                {
                    bad.push(format!("{path}.source_freshness says not checked: {f}"));
                }
            }
            for (k, v) in map {
                audit(v, &format!("{path}.{k}"), bad, seen);
            }
        }
        Value::Array(list) => {
            for (i, v) in list.iter().enumerate() {
                audit(v, &format!("{path}[{i}]"), bad, seen);
            }
        }
        _ => {}
    }
}

async fn socket_request(store: Arc<Store>, mut request: Value) -> Value {
    request
        .as_object_mut()
        .unwrap()
        .insert("version".into(), json!(1));
    let (mut client, server) = tokio::io::duplex(4 * 1024 * 1024);
    let task = tokio::spawn(devmap_serve::handle_stream(server, store));
    let mut payload = serde_json::to_vec(&request).unwrap();
    payload.push(b'\n');
    client.write_all(&payload).await.unwrap();
    let mut line = String::new();
    tokio::io::BufReader::new(&mut client)
        .read_line(&mut line)
        .await
        .unwrap();
    task.await.unwrap().expect("socket handler");
    serde_json::from_str(&line).expect("socket envelope is JSON")
}

async fn mcp(slot: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    handle_line(slot, &frame.to_string())
        .await
        .expect("a frame")
}

#[tokio::test]
async fn every_part_of_every_daemon_and_mcp_answer_carries_the_stale_verdict() {
    let (_root, store) = stale_store();
    let slot = Arc::new(StoreSlot::ready("in-memory", Arc::clone(&store)));
    // (mcp tool, mcp args, ipc command)
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "devmap_search",
            json!({"query": "helper"}),
            json!({"cmd": "search", "query": "helper"}),
        ),
        (
            "devmap_search",
            json!({"query": "nothing_zzz"}),
            json!({"cmd": "search", "query": "nothing_zzz"}),
        ),
        (
            "devmap_explore",
            json!({"query": "helper"}),
            json!({"cmd": "explore", "query": "helper"}),
        ),
        (
            "devmap_explore",
            json!({"query": "nope_unknown"}),
            json!({"cmd": "explore", "query": "nope_unknown"}),
        ),
        (
            "devmap_impact",
            json!({"target": "helper"}),
            json!({"cmd": "impact", "target": "helper"}),
        ),
        (
            "devmap_impact",
            json!({"target": "helper", "layers": true}),
            json!({"cmd": "impact", "target": "helper", "layers": true}),
        ),
        (
            "devmap_impact",
            json!({"target": "nope_unknown"}),
            json!({"cmd": "impact", "target": "nope_unknown"}),
        ),
        (
            "devmap_trace",
            json!({"from": "helper"}),
            json!({"cmd": "trace", "from": "helper"}),
        ),
        (
            "devmap_trace",
            json!({"from": "run", "to": "helper"}),
            json!({"cmd": "trace", "from": "run", "to": "helper"}),
        ),
        (
            "devmap_dependencies",
            json!({"target": "run"}),
            json!({"cmd": "deps", "target": "run"}),
        ),
        (
            "devmap_neighbors",
            json!({"targets": ["helper", "run", "nope_unknown"]}),
            json!({"cmd": "neighbors", "targets": ["helper", "run", "nope_unknown"]}),
        ),
        ("devmap_dead_symbols", json!({}), json!({"cmd": "dead"})),
        ("devmap_clones", json!({}), json!({"cmd": "clones"})),
        (
            "devmap_affected_tests",
            json!({"targets": ["helper"]}),
            json!({"cmd": "affected", "targets": ["helper"]}),
        ),
        (
            "devmap_affected_tests",
            json!({"targets": ["nope_unknown"]}),
            json!({"cmd": "affected", "targets": ["nope_unknown"]}),
        ),
        (
            "devmap_ask",
            json!({"query": "helper sums rows"}),
            json!({"cmd": "ask", "query": "helper sums rows"}),
        ),
        (
            "devmap_ask_evidence",
            json!({"query": "helper sums rows"}),
            json!({"cmd": "ask_evidence", "query": "helper sums rows"}),
        ),
        (
            "devmap_preview",
            json!({"file": "core.py", "content": "def gone():\n    pass\n"}),
            json!({"cmd": "preview", "file": "core.py", "content": "def gone():\n    pass\n"}),
        ),
        (
            "devmap_skeleton",
            json!({"file": "core.py"}),
            json!({"cmd": "skeleton", "file": "core.py"}),
        ),
    ];
    let mut failures = Vec::new();
    let mut total_parts = 0usize;
    for (tool, args, ipc) in cases {
        let label = format!("{tool} {args}");
        let m = mcp(&slot, tool, args).await;
        if m["result"]["isError"] == json!(true) {
            failures.push(format!("MCP {label} could not run: {m}"));
        } else {
            let body = m["result"]["structuredContent"].clone();
            let mut bad = Vec::new();
            let mut seen = 0;
            audit(&body, "$", &mut bad, &mut seen);
            total_parts += seen;
            // A skeleton is one report with its own top-level verdict.
            let own_verdict = body["source_freshness"]["fresh"] == json!(false);
            if seen == 0 && !own_verdict {
                failures.push(format!(
                    "MCP {label}: no Response-shaped part found (probe blind): {body}"
                ));
            }
            failures.extend(bad.into_iter().map(|b| format!("MCP {label}: {b}")));
        }
        let env = socket_request(Arc::clone(&store), ipc).await;
        if env["ok"] != json!(true) {
            failures.push(format!("IPC {label} could not run: {env}"));
            continue;
        }
        let mut bad = Vec::new();
        let mut seen = 0;
        audit(&env, "$", &mut bad, &mut seen);
        total_parts += seen;
        failures.extend(bad.into_iter().map(|b| format!("IPC {label}: {b}")));
    }
    assert!(total_parts > 40, "audit saw only {total_parts} parts");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
