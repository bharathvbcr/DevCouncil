//! `devmap <query> --json` names the repository it answered from, as the MCP
//! envelope does.
//!
//! MCP answers carried `repository` ({root, store, resolved_from}); the CLI
//! answers did not, so a caller holding only the CLI line could not tell which
//! checkout it came from. Two properties: the top-level key sets of the CLI and
//! MCP answers agree for search, impact and explore, and `--root B` run from
//! a cwd in A reports B.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn devmap() -> String {
    env!("CARGO_BIN_EXE_devmap").to_string()
}

fn corpus(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-cli-repo-envelope-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("lib.py"),
        "def helper(rows):\n    return rows\n\n\ndef caller(rows):\n    return helper(rows)\n",
    )
    .unwrap();
    let built = Command::new(devmap())
        .args(["build", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    root.canonicalize().unwrap()
}

fn cli(cwd: &Path, args: &[&str]) -> Value {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.lines().next().unwrap_or("")).unwrap_or_else(|e| {
        panic!(
            "devmap {args:?}: not JSON ({e}): {stdout} / {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// One `tools/call` against a real `devmap mcp` child pinned to `root`.
fn mcp(root: &Path, tool: &str, arguments: Value) -> Value {
    let mut child = Command::new(devmap())
        .arg("--root")
        .arg(root)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut ask = |id: u64, method: &str, params: Value| -> Value {
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        stdin.flush().unwrap();
        serde_json::from_str(&lines.next().expect("a frame").unwrap()).unwrap()
    };
    ask(
        1,
        "initialize",
        json!({"protocolVersion": "2025-11-25", "capabilities": {},
               "clientInfo": {"name": "parity", "version": "0"}}),
    );
    let reply = ask(2, "tools/call", json!({"name": tool, "arguments": arguments}));
    drop(stdin);
    let _ = child.wait();
    assert_eq!(reply["result"]["isError"], json!(false), "{tool}: {reply}");
    reply["result"]["structuredContent"].clone()
}

fn keys(value: &Value) -> BTreeSet<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn cli_and_mcp_answers_share_the_repository_key() {
    let root = corpus("parity");
    let rs = root.to_str().unwrap();
    for (label, cli_args, tool, arguments) in [
        (
            "search",
            vec!["--json", "search", "helper"],
            "devmap_search",
            json!({"query": "helper"}),
        ),
        (
            "impact",
            vec!["--json", "impact", "helper"],
            "devmap_impact",
            json!({"target": "helper"}),
        ),
        (
            "explore",
            vec!["--json", "explore", "helper"],
            "devmap_explore",
            json!({"query": "helper"}),
        ),
    ] {
        let from_cli = cli(&root, &cli_args);
        let from_mcp = mcp(&root, tool, arguments);
        assert!(
            from_cli.get("repository").is_some(),
            "{label}: CLI answer has no repository; keys {:?}",
            keys(&from_cli)
        );
        eprintln!("{label}: cli keys {:?}\n{label}: mcp keys {:?}", keys(&from_cli), keys(&from_mcp));
        let missing: Vec<_> = keys(&from_mcp).difference(&keys(&from_cli)).cloned().collect();
        let repository_shape = |v: &Value| keys(&v["repository"]);
        assert_eq!(
            repository_shape(&from_cli),
            repository_shape(&from_mcp),
            "{label}: repository sub-keys differ"
        );
        // Every MCP-only key must be one the transport owns, not a CLI gap.
        let mcp_only_ok: BTreeSet<&str> = BTreeSet::new();
        let unexplained: Vec<_> = missing
            .iter()
            .filter(|k| !mcp_only_ok.contains(k.as_str()))
            .collect();
        assert!(
            unexplained.is_empty(),
            "{label}: keys MCP carries and the CLI does not: {unexplained:?} (root {rs})"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_named_root_is_the_reported_repository_not_the_cwd() {
    let a = corpus("cwd-a");
    let b = corpus("named-b");
    for args in [
        vec!["--root", b.to_str().unwrap(), "--json", "search", "helper"],
        vec!["--root", b.to_str().unwrap(), "--json", "dead"],
        vec!["--root", b.to_str().unwrap(), "--json", "deps", "helper"],
    ] {
        let v = cli(&a, &args);
        assert_eq!(
            v["repository"]["root"],
            json!(b.display().to_string()),
            "{args:?}: {v}"
        );
        assert_eq!(v["repository"]["resolved_from"], json!("root"));
        assert!(v["repository"]["store"]
            .as_str()
            .unwrap()
            .starts_with(b.to_str().unwrap()));
    }
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}

/// `repository.store` is the file the command opened: `--db` beats `--root`,
/// and the root is then inferred from the db, as for a pinned MCP server.
#[test]
fn the_reported_store_is_the_one_the_command_opened() {
    let a = corpus("store-a");
    let b = corpus("store-b");
    let db = devmap_extract::paths::store_path(&b);
    let copy = a.join("copy.sqlite");
    std::fs::copy(&db, &copy).unwrap();
    let copy = copy.canonicalize().unwrap();
    let v = cli(
        &a,
        &[
            "--root",
            a.to_str().unwrap(),
            "--db",
            copy.to_str().unwrap(),
            "--json",
            "search",
            "helper",
        ],
    );
    assert_eq!(v["repository"]["store"], json!(copy.display().to_string()), "{v}");
    assert_eq!(v["repository"]["resolved_from"], json!("db"));

    let plain = cli(&a, &["--root", b.to_str().unwrap(), "--json", "search", "helper"]);
    assert_eq!(
        plain["repository"]["store"],
        json!(devmap_extract::paths::store_path(&b)
            .canonicalize()
            .unwrap()
            .display()
            .to_string())
    );
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}
