//! Probe 6 of the refusal-freshness attack: the CLI and the MCP dispatch must
//! disclose the same `source_freshness` for the same question, refusal or not.
//!
//! The CLI is the real `devmap` binary reading the store a real `devmap build`
//! wrote. The MCP side is `devmap_serve::mcp::handle_line` over a read-only
//! store on the same file. Each is a fresh process/engine, so neither has a
//! status-primed cache and both should say exactly the same thing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"))
}

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-refusal-parity-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(
        root.join("chain.py"),
        "def a():\n    return b()\n\n\ndef b():\n    return c()\n\n\ndef c():\n    return 1\n",
    )
    .unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "a@example.invalid"],
        vec!["config", "user.name", "attack"],
        vec!["add", "-A"],
        vec!["commit", "-q", "-m", "one"],
    ] {
        let ok = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(&args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }
    let built = run(&root, &["build", "."]);
    assert!(
        built.status.success(),
        "build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    root
}

/// Every `source_freshness` object anywhere in `value`, serialised and sorted.
fn freshness_objects(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if key == "source_freshness" {
                    out.push(inner.to_string());
                } else {
                    freshness_objects(inner, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| freshness_objects(item, out)),
        _ => {}
    }
}

fn collected(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    freshness_objects(value, &mut out);
    out.sort();
    out
}

fn mcp_payload(slot: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let frame = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    })
    .to_string();
    let reply = runtime
        .block_on(handle_line(slot, &frame))
        .expect("a request is answered");
    let result = &reply["result"];
    assert!(result.is_object(), "{tool}: {reply}");
    if let Some(structured) = result.get("structuredContent") {
        return structured.clone();
    }
    let text = result["content"][0]["text"].as_str().expect("text content");
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{tool}: not JSON ({e}): {text}"))
}

fn cli_json(root: &Path, args: &[&str]) -> Value {
    let out = run(root, args);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    serde_json::from_str(stdout.lines().next().unwrap_or("null")).unwrap_or_else(|e| {
        panic!(
            "devmap {args:?}: stdout not JSON ({e}): {stdout}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

#[test]
fn the_cli_and_the_mcp_dispatch_disclose_the_same_freshness_for_a_refusal() {
    let root = fixture("p6");
    let db = devmap_extract::paths::store_path(&root);
    let store = Store::open_read_only(&db).unwrap();
    let slot = Arc::new(StoreSlot::ready(db.clone(), Arc::new(store)));

    // (label, CLI argv, MCP tool, MCP arguments)
    let cases: Vec<(&str, Vec<&str>, &str, Value)> = vec![
        (
            "impact unknown",
            vec!["impact", "zz_unknown", "--json"],
            "devmap_impact",
            json!({"target": "zz_unknown"}),
        ),
        (
            "impact unknown layers",
            vec!["impact", "zz_unknown", "--layers", "--json"],
            "devmap_impact",
            json!({"target": "zz_unknown", "layers": true}),
        ),
        (
            "impact known",
            vec!["impact", "c", "--json"],
            "devmap_impact",
            json!({"target": "c"}),
        ),
        (
            "trace unknown",
            vec!["trace", "zz_unknown", "--json"],
            "devmap_trace",
            json!({"from": "zz_unknown"}),
        ),
        (
            "deps unknown",
            vec!["deps", "zz_unknown", "--json"],
            "devmap_dependencies",
            json!({"target": "zz_unknown"}),
        ),
        (
            "explore unknown",
            vec!["explore", "zz_unknown", "--json"],
            "devmap_explore",
            json!({"query": "zz_unknown"}),
        ),
        (
            "affected unknown",
            vec!["affected", "zz_unknown", "--json"],
            "devmap_affected_tests",
            json!({"targets": ["zz_unknown"]}),
        ),
    ];
    let mut disagreements = Vec::new();
    for (label, argv, tool, arguments) in cases {
        let cli = collected(&cli_json(&root, &argv));
        let mcp = collected(&mcp_payload(&slot, tool, arguments));
        eprintln!("{label}: cli={cli:?} mcp={mcp:?}");
        if cli.is_empty() || mcp.is_empty() || cli != mcp {
            disagreements.push(format!("{label}:\n  cli={cli:?}\n  mcp={mcp:?}"));
        }
    }
    assert!(
        disagreements.is_empty(),
        "CLI and MCP disagree about source_freshness:\n{}",
        disagreements.join("\n")
    );
}
