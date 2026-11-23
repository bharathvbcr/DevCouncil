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
    let reply = ask(
        2,
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
    );
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
        eprintln!(
            "{label}: cli keys {:?}\n{label}: mcp keys {:?}",
            keys(&from_cli),
            keys(&from_mcp)
        );
        let missing: Vec<_> = keys(&from_mcp)
            .difference(&keys(&from_cli))
            .cloned()
            .collect();
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
    assert_eq!(
        v["repository"]["store"],
        json!(copy.display().to_string()),
        "{v}"
    );
    assert_eq!(v["repository"]["resolved_from"], json!("db"));

    let plain = cli(
        &a,
        &["--root", b.to_str().unwrap(), "--json", "search", "helper"],
    );
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

fn named_corpus(dir_name: &str) -> PathBuf {
    let parent = corpus("hostile-parent");
    let root = parent.join(dir_name);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("lib.py"), "def helper(rows):\n    return rows\n").unwrap();
    let built = Command::new(devmap())
        .args(["build", "."])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    root.canonicalize().unwrap()
}

/// Attack: every read command that prints a store-backed answer names its
/// repository, not only the ones that go through `ReadStore::emit_answer`.
/// `routes`, `api-impact`, `suspects` and `cypher` open the whole store and
/// printed through `emit_json`, so they had none while the MCP tools of the
/// same names carried it.
#[test]
fn whole_store_read_commands_name_their_repository_like_their_mcp_tools() {
    let root = corpus("whole-store");
    for (label, cli_args, tool, arguments) in [
        (
            "routes",
            vec!["--json", "routes"],
            "devmap_routes",
            json!({}),
        ),
        (
            "api-impact",
            vec!["--json", "api-impact", "/x"],
            "devmap_api_impact",
            json!({"route": "/x"}),
        ),
        (
            "suspects",
            vec!["--json", "suspects", "helper", "--since", "HEAD"],
            "devmap_suspects",
            json!({"symptom": "helper", "since": "HEAD"}),
        ),
        (
            "cypher",
            vec![
                "--json",
                "cypher",
                "MATCH (a)-[r:calls]->(b) RETURN a, b LIMIT 1",
            ],
            "devmap_cypher",
            json!({"query": "MATCH (a)-[r:calls]->(b) RETURN a, b LIMIT 1"}),
        ),
    ] {
        let from_cli = cli(&root, &cli_args);
        let from_mcp = mcp(&root, tool, arguments);
        assert_eq!(
            from_cli["repository"]["root"], from_mcp["repository"]["root"],
            "{label}: CLI {from_cli}"
        );
        assert_eq!(
            from_cli["repository"]["store"], from_mcp["repository"]["store"],
            "{label}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Attack: a root with a space, a double quote, non-ASCII characters. The CLI
/// and MCP must report the same strings (JSON-escaped, not mangled).
#[test]
fn hostile_root_names_are_reported_exactly_as_the_mcp_envelope_reports_them() {
    for dir in [
        "a b c",
        "q\"uote",
        "r\u{e9}p\u{f6}-\u{65e5}\u{672c}-\u{1f600}",
    ] {
        let root = named_corpus(dir);
        let from_cli = cli(&root, &["--json", "search", "helper"]);
        let from_mcp = mcp(&root, "devmap_search", json!({"query": "helper"}));
        assert_eq!(
            from_cli["repository"]["root"],
            json!(root.display().to_string()),
            "{dir}"
        );
        assert_eq!(
            from_cli["repository"]["root"], from_mcp["repository"]["root"],
            "{dir}"
        );
        assert_eq!(
            from_cli["repository"]["store"], from_mcp["repository"]["store"],
            "{dir}"
        );
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }
}

/// Attack: a relative `--db` from a cwd that is not the repository is reported
/// as one absolute file, and `--db` wins over a `--root` naming another repo.
#[test]
fn a_relative_db_from_another_cwd_is_reported_absolute_and_beats_root() {
    let a = corpus("reldb-a");
    let b = corpus("reldb-b");
    let db = devmap_extract::paths::store_path(&b)
        .canonicalize()
        .unwrap();
    let rel = db
        .strip_prefix(a.parent().unwrap())
        .unwrap()
        .display()
        .to_string();
    let v = cli(
        a.parent().unwrap(),
        &[
            "--root",
            a.to_str().unwrap(),
            "--db",
            &rel,
            "--json",
            "search",
            "helper",
        ],
    );
    assert_eq!(v["repository"]["resolved_from"], json!("db"), "{v}");
    assert_eq!(
        v["repository"]["store"],
        json!(db.display().to_string()),
        "{v}"
    );
    assert_eq!(
        v["repository"]["root"],
        json!(b.display().to_string()),
        "{v}"
    );
    let _ = std::fs::remove_dir_all(&a);
    let _ = std::fs::remove_dir_all(&b);
}
