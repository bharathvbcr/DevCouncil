//! `devmap manifest` must publish both consumer artifacts from one invocation.
//!
//! The Python `dev map` writes `.devcouncil/repo_map.json` *and*
//! `.devcouncil/graph/code_graph.json`; eleven modules under `src/devcouncil/`
//! read the second. Before this suite the Rust port wrote only the first, so a
//! cutover would have left every graph consumer reading a stale Python file or
//! nothing at all.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// Unique per call, not merely per instant.
fn temp_root(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-code-graph-{tag}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    root
}

/// Two files, one calling across a real import, so the graph has calls,
/// contains edges and a dead symbol rather than only structure.
fn write_fixture(root: &Path) {
    fs::write(
        root.join("src/target.py"),
        "def durable_symbol():\n    return 1\n\n\ndef never_called():\n    return 2\n",
    )
    .expect("write target fixture");
    fs::write(
        root.join("src/caller.py"),
        "from target import durable_symbol\n\n\ndef caller():\n    return durable_symbol()\n",
    )
    .expect("write caller fixture");
}

fn build(root: &Path, db: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--db"])
        .arg(db)
        .arg("build")
        .arg(root)
        .output()
        .expect("run build");
    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_manifest(root: &Path, db: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .args(["--json", "--db"])
        .arg(db)
        .arg("manifest")
        .arg(root)
        .args(extra)
        .output()
        .expect("run manifest")
}

/// One `devmap manifest` publishes both artifacts, and the graph is the
/// consumer schema — not a Rust-shaped payload at a Python path.
#[test]
fn one_manifest_invocation_publishes_both_consumer_artifacts() {
    let root = temp_root("both");
    let db = root.join("index.sqlite");
    write_fixture(&root);
    build(&root, &db);

    let output = run_manifest(&root, &db, &[]);
    assert!(
        output.status.success(),
        "manifest failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let map_path = devmap_extract::paths::repo_map_path(&root);
    let graph_path = devmap_extract::paths::code_graph_path(&root);
    assert!(map_path.is_file(), "repo_map.json must be written");
    assert!(
        graph_path.is_file(),
        "code_graph.json must be written by the same invocation — consumers get \
         both from one `dev map` run"
    );

    let payload: Value = serde_json::from_slice(&output.stdout).expect("manifest returns JSON");
    // Compared by suffix: macOS resolves the temp root through /private, so the
    // reported absolute path is canonical while the fixture path is not.
    assert!(
        payload["graph_output"]
            .as_str()
            .is_some_and(|p| p.ends_with(".devmap/graph/code_graph.json")),
        "the JSON result must name the graph it wrote: {payload}"
    );

    let graph: Value =
        serde_json::from_str(&fs::read_to_string(&graph_path).unwrap()).expect("graph parses");

    // The pydantic `CodeGraph` model's fields, all of them.
    let mut keys: Vec<&str> = graph
        .as_object()
        .expect("graph is an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        devmap_query::CODE_GRAPH_TOP_LEVEL_KEYS,
        "the artifact must carry the schema Python consumers validate, which \
         `CODE_GRAPH_TOP_LEVEL_KEYS` declares once for the writer, this test and \
         `schema.py` alike — a fourth hand-written copy is how the last key \
         landed on one side only"
    );
    assert_eq!(graph["schema_version"], 2);

    let nodes = graph["nodes"].as_array().expect("nodes array");
    assert!(
        nodes.len() >= 5,
        "two files plus three functions: {}",
        nodes.len()
    );
    let node_ids: Vec<&str> = nodes.iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert!(
        node_ids.contains(&"src/target.py::durable_symbol"),
        "{node_ids:?}"
    );

    // A symbol node carries the line its byte span resolves to, read from the
    // real file. `durable_symbol` opens line 1 of `src/target.py`.
    let target = nodes
        .iter()
        .find(|n| n["id"] == "src/target.py::durable_symbol")
        .unwrap();
    assert_eq!(target["line"], 1, "{target}");
    assert_eq!(target["kind"], "function");
    assert_eq!(target["language"], "python");
    assert_eq!(target["path"], "src/target.py");
    let never = nodes
        .iter()
        .find(|n| n["id"] == "src/target.py::never_called")
        .unwrap();
    assert_eq!(
        never["line"], 5,
        "later symbols get their own line: {never}"
    );

    // Every edge endpoint must name a node, or the graph cannot be traversed.
    let edges = graph["edges"].as_array().expect("edges array");
    assert!(!edges.is_empty(), "the fixture has calls and containment");
    for edge in edges {
        for side in ["source", "target"] {
            let id = edge[side].as_str().unwrap();
            assert!(
                node_ids.contains(&id),
                "edge {side} {id:?} names no node: {edge}"
            );
        }
        assert!(
            ["extracted", "inferred", "ambiguous"].contains(&edge["confidence"].as_str().unwrap()),
            "confidence must be a Python Confidence value: {edge}"
        );
    }
    assert!(
        edges
            .iter()
            .any(|e| e["kind"] == "calls" && e["source"] == "src/caller.py::caller"),
        "the cross-file call must be in the graph: {edges:?}"
    );

    // Liveness fields are honest about what was and was not computed.
    //
    // Retired from "never computed" (W1.1/W2.4). The component pass answers
    // reachability now, and this fixture's analysis is `ok`, so the empty list
    // is a real result rather than a placeholder. The property being pinned is
    // the same one: the list and the flag must agree about whether anything was
    // measured, in both directions.
    assert_eq!(graph["unreachable_files"], serde_json::json!([]));
    assert_eq!(
        graph["meta"]["liveness_unreachable_unreliable"], false,
        "the analysis is `ok`, so reachability was answered and the empty list \
         is a computed zero"
    );
    assert!(
        graph["meta"]["devmap_rust"]["unavailable"]
            .get("unreachable_files")
            .is_none(),
        "a computed answer must not also be declared unavailable"
    );
    assert!(
        graph["dead_clusters"].is_array(),
        "the component pass publishes its findings beside the per-symbol list"
    );
    assert_eq!(graph["meta"]["map_engine"], "devmap-rust");
    assert_eq!(graph["meta"]["devmap_rust"]["analysis_status"], "ok");

    // A dead-code entry joins the nodes it describes.
    for entry in graph["dead_code"].as_array().unwrap() {
        let id = entry["id"].as_str().unwrap();
        assert!(node_ids.contains(&id), "dead entry names no node: {entry}");
        assert!(
            ["extracted", "inferred", "ambiguous"].contains(&entry["confidence"].as_str().unwrap()),
            "{entry}"
        );
    }

    let _ = fs::remove_dir_all(&root);
}

/// A Python-written graph is never clobbered without `--force`, and the guard
/// leaves the file untouched when it refuses.
#[test]
fn a_python_written_graph_is_protected_until_forced() {
    let root = temp_root("guard");
    let db = root.join("index.sqlite");
    write_fixture(&root);
    build(&root, &db);
    assert!(run_manifest(&root, &db, &[]).status.success());

    let graph_path = devmap_extract::paths::code_graph_path(&root);
    // The live Python artifact's shape: a real meta dict with no engine marker.
    let python =
        r#"{"schema_version": 2, "nodes": [], "edges": [], "meta": {"parse_cache_version": 8}}"#;
    fs::write(&graph_path, python).unwrap();

    let refused = run_manifest(&root, &db, &[]);
    assert!(
        !refused.status.success(),
        "a foreign graph must not be silently replaced"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("--force") && stderr.contains("code graph"),
        "the refusal must say what it refused and how to override: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(&graph_path).unwrap(),
        python,
        "a refused write must leave the existing artifact byte-identical"
    );

    let forced = run_manifest(&root, &db, &["--force"]);
    assert!(
        forced.status.success(),
        "--force must replace it: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
    let replaced: Value = serde_json::from_str(&fs::read_to_string(&graph_path).unwrap()).unwrap();
    assert_eq!(replaced["meta"]["map_engine"], "devmap-rust");

    let _ = fs::remove_dir_all(&root);
}

/// Two cold builds of the same tree, into separate stores under separate
/// roots, produce a byte-identical `code_graph.json`.
///
/// The determinism gate compares digests across cold builds, and every
/// collection in this artifact is derived from a map or set somewhere upstream.
#[test]
fn two_cold_builds_produce_a_byte_identical_code_graph() {
    let mut rendered = Vec::new();
    for tag in ["cold-a", "cold-b"] {
        let root = temp_root(tag);
        let db = root.join("index.sqlite");
        write_fixture(&root);
        // A third file so community assignment and sorting have something to do.
        fs::write(
            root.join("src/extra.py"),
            "from target import never_called\n\n\ndef extra():\n    return never_called()\n",
        )
        .unwrap();
        build(&root, &db);
        assert!(run_manifest(&root, &db, &[]).status.success());
        rendered.push((
            root.clone(),
            fs::read_to_string(devmap_extract::paths::code_graph_path(&root)).unwrap(),
        ));
    }

    assert_eq!(
        rendered[0].1, rendered[1].1,
        "two cold builds of identical sources must render an identical graph"
    );
    // And the content is real, not two identical empty documents.
    let graph: Value = serde_json::from_str(&rendered[0].1).unwrap();
    // Three files plus four functions.
    assert!(graph["nodes"].as_array().unwrap().len() >= 7);
    assert!(!graph["edges"].as_array().unwrap().is_empty());

    for (root, _) in rendered {
        let _ = fs::remove_dir_all(root);
    }
}

/// `manifest` against an unbuilt store errors instead of publishing an empty
/// graph. `nodes: []` from a store that was never built is indistinguishable
/// from an empty repository, and every liveness answer drawn from the second
/// is wrong.
#[test]
fn an_unbuilt_store_publishes_no_graph_at_all() {
    let root = temp_root("unbuilt");
    let db = root.join("index.sqlite");
    write_fixture(&root);

    let output = run_manifest(&root, &db, &[]);
    assert!(
        !output.status.success(),
        "an unbuilt store must not produce artifacts"
    );
    assert!(
        !devmap_extract::paths::code_graph_path(&root).exists(),
        "no graph may be left behind"
    );

    let _ = fs::remove_dir_all(&root);
}

/// The interned encoding is opt-in, additive, and says the same thing (G6).
///
/// The assertion that carries the weight is the last one: the interned file,
/// decoded, must equal the verbose file the *same* invocation wrote. Anything
/// weaker — that it exists, that it is smaller, that it parses — would pass on
/// an encoder that silently dropped a field, which is the failure this whole
/// design is arranged against.
#[test]
fn the_interned_graph_is_opt_in_and_decodes_to_the_verbose_one() {
    let root = temp_root("compact");
    let db = root.join("index.sqlite");
    write_fixture(&root);
    build(&root, &db);

    let compact_path = devmap_extract::paths::compact_code_graph_path(&root);
    let graph_path = devmap_extract::paths::code_graph_path(&root);

    // Off by default: the flag is the only thing that writes it, and the
    // absence must be reported as absent rather than as an empty path.
    let plain = run_manifest(&root, &db, &[]);
    assert!(plain.status.success());
    assert!(
        !compact_path.exists(),
        "the interned artifact must not appear without being asked for"
    );
    let plain_json: Value =
        serde_json::from_slice(&plain.stdout).expect("manifest --json must emit JSON");
    assert_eq!(
        plain_json.get("compact_graph_output"),
        Some(&Value::Null),
        "no interned artifact must read as null, never as an empty path"
    );

    let asked = run_manifest(
        &root,
        &db,
        &[
            "--compact-graph-output",
            ".devmap/graph/code_graph.compact.json",
            "--force",
        ],
    );
    assert!(
        asked.status.success(),
        "manifest failed: stdout={} stderr={}",
        String::from_utf8_lossy(&asked.stdout),
        String::from_utf8_lossy(&asked.stderr)
    );
    assert!(
        graph_path.is_file(),
        "the verbose artifact stays canonical and is written either way"
    );
    assert!(
        compact_path.is_file(),
        "the interned artifact must be written"
    );

    let verbose: Value =
        serde_json::from_str(&fs::read_to_string(&graph_path).expect("read verbose"))
            .expect("verbose must be JSON");
    let compact: Value =
        serde_json::from_str(&fs::read_to_string(&compact_path).expect("read interned"))
            .expect("interned must be JSON");

    assert_eq!(
        compact["encoding"],
        Value::String("devmap-compact-v1".to_string())
    );
    assert_eq!(
        devmap_query::decode_compact(&compact).expect("decode"),
        verbose,
        "the two files this invocation wrote must be the same model"
    );

    // The clobber guard covers both paths: identity is `meta.map_engine`, and
    // the question it asks does not depend on the encoding.
    fs::write(&compact_path, r#"{"nodes": [], "edges": []}"#).expect("plant a foreign file");
    let refused = run_manifest(
        &root,
        &db,
        &[
            "--compact-graph-output",
            ".devmap/graph/code_graph.compact.json",
        ],
    );
    assert!(
        !refused.status.success(),
        "a foreign file at the interned path must be refused without --force"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refuse to overwrite"),
        "the refusal must say why: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}
