//! Probes 5 and 8 against `lane/deps-outbound` @ 55a1c302, at the surface a
//! person or a script types: `devmap deps`. Failing tests are findings.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use devmap_query::{Request, StoreQueryEngine};
use devmap_store::Store;

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

fn corpus(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-deps-attack-{name}-{}-{}",
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
        "RETRY = 2\n\n\ndef helper():\n    return 1\n\n\ndef worker():\n    helper()\n    return RETRY\n",
    )
    .unwrap();
    std::fs::write(
        root.join("callers.py"),
        "from lib import worker\n\n\ndef caller():\n    return worker()\n",
    )
    .unwrap();
    std::fs::write(root.join("a.py"), "def dup():\n    return 1\n").unwrap();
    std::fs::write(root.join("b.py"), "def dup():\n    return 2\n").unwrap();
    let built = run(&root, &["build", "."]);
    assert!(
        built.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    root
}

fn strip_volatile(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(o) = v.as_object_mut() {
        o.remove("source_freshness");
        o.remove("tokens_used");
    }
    v
}

/// Probe 8. The CLI's `--json deps` payload is the engine's payload for the
/// same store, for file, symbol, qualified and empty-answer queries.
#[test]
fn p8_the_cli_json_payload_is_the_engine_payload() {
    let root = corpus("payload");
    let store = Store::open_read_only(devmap_extract::paths::store_path(&root)).unwrap();
    let mut report = String::new();
    for query in [
        "lib.py",
        "worker",
        "RETRY",
        "lib.py::worker",
        "callers.py",
        "nope",
        "./lib.py",
    ] {
        let out = run(&root, &["--json", "deps", "--", query]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let line = stdout.lines().next().unwrap_or("");
        let cli: serde_json::Value = serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
        let engine = StoreQueryEngine::new(&store).dependencies_at_rung(
            Request {
                query: query.to_string(),
                token_budget: 2000,
                min_confidence: 0.0,
                max_depth: 1,
            },
            None,
        );
        match engine {
            Ok(r) => {
                let want = strip_volatile(serde_json::to_value(r).unwrap());
                if strip_volatile(cli.clone()) != want {
                    report.push_str(&format!("  {query}: cli={cli}\n    engine={want}\n"));
                }
            }
            Err(e) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                if out.status.success()
                    || !(stderr.contains(&e.to_string()) || stdout.contains(&e.to_string()))
                {
                    report.push_str(&format!(
                        "  {query}: engine refused ({e}); cli status={:?} stderr={stderr}\n",
                        out.status
                    ));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
    assert!(report.is_empty(), "cli != engine:\n{report}");
}

/// Probe 5. A complete answer is not a warning. `deps RETRY` is complete and
/// empty; the human rendering prints the inbound note as `warning:`, the same
/// prefix it uses for a capped walk, so a reader (or a script grepping stderr/
/// stdout for `warning`) treats a finished answer as a partial one.
#[test]
fn p5_a_complete_empty_answer_prints_no_warning() {
    let root = corpus("warning");
    let out = run(&root, &["deps", "--", "RETRY"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
    assert!(out.status.success(), "{text}");
    assert!(
        !text.contains("warning:"),
        "complete answer rendered as a warning:\n{text}"
    );
}

/// Probe 5/8. The same fact, over the CLI, in JSON: a complete answer has no
/// `walk_incomplete` key.
#[test]
fn p5_a_complete_empty_answer_has_no_walk_incomplete_in_json() {
    let root = corpus("json");
    let out = run(&root, &["--json", "deps", "--", "RETRY"]);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = std::fs::remove_dir_all(&root);
    let v: serde_json::Value =
        serde_json::from_str(stdout.lines().next().unwrap_or("null")).unwrap();
    assert_eq!(v["items"], serde_json::json!([]), "{v}");
    assert!(
        v.get("walk_incomplete").is_none(),
        "{}",
        v["walk_incomplete"]
    );
}

/// Probe 8. The Go client (`backend/go_orchestrator/dc/devmap`) shells
/// `devmap deps --budget N -- <file>`. Its flag set must still parse.
#[test]
fn p7_the_go_clients_invocation_still_parses_and_answers_a_file() {
    let root = corpus("goargs");
    let out = run(&root, &["deps", "--budget", "2000", "--", "lib.py"]);
    let ok = out.status.success();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = std::fs::remove_dir_all(&root);
    assert!(ok, "{text}");
    let json = run(
        &corpus("goargs2"),
        &["--json", "deps", "--budget", "2000", "--", "lib.py"],
    );
    let v: serde_json::Value = serde_json::from_str(
        String::from_utf8_lossy(&json.stdout)
            .lines()
            .next()
            .unwrap_or("null"),
    )
    .unwrap();
    // Every row the Go `Edge` type decodes must carry source_file/target_file.
    for item in v["items"].as_array().unwrap() {
        assert!(
            item["source_file"].is_string() && item["target_file"].is_string(),
            "{item}"
        );
    }
    // The Go client documents the answer as "every edge touching a file, in both
    // directions" (devmap.go:784-791) and decodes only items/hidden. After the
    // lane every row has source_file == the queried file.
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["source_file"] == "lib.py"),
        "outbound only: {v}"
    );
}
