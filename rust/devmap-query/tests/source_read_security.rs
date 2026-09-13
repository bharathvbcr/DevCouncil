#![cfg(all(unix, feature = "parse"))]

use devmap_extract::subprocess::{run_bounded, Bounds};
use devmap_query::api_routes::{route_map, shape_check_over, ScanBudget};
use serde_json::{json, Value};
use std::{fs, path::PathBuf, process::Command, time::Duration};

fn fixture(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("devmap-source-{name}-{}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}

fn graph(paths: &[&str]) -> Value {
    json!({"nodes": paths.iter().map(|path| json!({"id":path,"path":path,"kind":"file"})).collect::<Vec<_>>(), "edges":[]})
}

#[test]
fn special_sources_cannot_block_graph_clients_or_handlers() {
    let mut failures = Vec::new();
    for mode in ["graph", "clients", "handlers"] {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--ignored",
                "--exact",
                "special_source_child",
                "--nocapture",
            ])
            .env("DEVMAP_SPECIAL_SOURCE_MODE", mode);
        let result = run_bounded(
            &mut child,
            Bounds {
                deadline: Duration::from_secs(3),
                stdout_cap: 16384,
                stderr_cap: 16384,
            },
        );
        match result {
            Ok(output) if output.status.success() => {}
            other => failures.push(format!("{mode}: {other:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "bounded child of special_sources_cannot_block_graph_clients_or_handlers"]
fn special_source_child() {
    let mode = std::env::var("DEVMAP_SPECIAL_SOURCE_MODE").unwrap();
    let root = fixture(&mode);
    let fifo = root.join("z.py");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    if mode == "graph" {
        let ext = devmap_extract::extract_file("z.py", "def handler(): pass\n");
        let value =
            devmap_query::build_graph_core_value(&[ext], &Default::default(), &[], root.to_str());
        assert!(value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["line"] == 0));
    } else {
        fs::write(root.join("a.ts"), "fetch('/example')\n").unwrap();
        let mut value = graph(&["a.ts", "z.py"]);
        value["nodes"][1] =
            json!({"id":"z.py::handler","path":"z.py","kind":"function","line":1,"end_line":2});
        value["edges"] =
            json!([{"kind":"routes_to","source":"GET /example","target":"z.py::handler"}]);
        let budget = ScanBudget {
            max_files: if mode == "handlers" { 1 } else { 10 },
            ..Default::default()
        };
        let result = route_map(&root, &value, &budget);
        assert_eq!(result["scan"]["complete"], false);
        assert_eq!(result["routes"][0]["handler_keys_available"], false);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_reads_count_toward_the_attempt_budget() {
    let root = fixture("attempts");
    let result = route_map(
        &root,
        &graph(&["a.ts", "b.ts", "c.ts"]),
        &ScanBudget {
            max_files: 1,
            ..Default::default()
        },
    );
    assert_eq!(result["scan"]["files_unreadable"], 1);
    assert_eq!(result["scan"]["files_skipped_budget"], 2);
    assert_eq!(result["scan"]["complete"], false);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_paths_cannot_leave_the_repository() {
    let base = fixture("containment");
    let root = base.join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(base.join("outside.ts"), "fetch('/outside-sentinel')\n").unwrap();
    std::os::unix::fs::symlink(base.join("outside.ts"), root.join("alias.ts")).unwrap();
    for path in [
        "../outside.ts",
        "alias.ts",
        base.join("outside.ts").to_str().unwrap(),
    ] {
        let result = route_map(&root, &graph(&[path]), &ScanBudget::default());
        assert_eq!(result["scan"]["sites_found"], 0, "{path}");
        assert_eq!(result["scan"]["files_unreadable"], 1, "{path}");
        assert_eq!(result["scan"]["complete"], false);
    }
    fs::write(root.join("normal.ts"), "fetch('/inside')\n").unwrap();
    std::os::unix::fs::symlink("normal.ts", root.join("inside.ts")).unwrap();
    let result = route_map(&root, &graph(&["inside.ts"]), &ScanBudget::default());
    assert_eq!(result["scan"]["sites_found"], 1);
    assert_eq!(result["scan"]["complete"], true);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn unavailable_handler_source_is_not_a_shape_mismatch() {
    let root = fixture("shape");
    fs::write(root.join("client.ts"), "const res = await fetch('/example');\nconst data = await res.json();\nconsole.log(data.name);\n").unwrap();
    let value = json!({"nodes":[{"id":"client","path":"client.ts","kind":"file"},{"id":"handler","path":"missing.ts","kind":"function","line":1}],"edges":[{"kind":"routes_to","source":"GET /example","target":"handler"}]});
    let result = route_map(&root, &value, &ScanBudget::default());
    let shape = shape_check_over(&result, None);
    assert_eq!(shape["checks"][0]["mismatch"], false);
    assert_eq!(shape["checks"][0]["verdict"], "handler_source_unavailable");
    assert!(result["routes"][0]["handler_keys"].is_null());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn graph_spans_require_the_indexed_source_revision_and_size() {
    let root = fixture("graph-revision");
    let source = "\ndef handler(): pass\n";
    let ext = devmap_extract::extract_file("source.py", source);
    for text in ["# changed\n".to_string(), "#".repeat(2 * 1024 * 1024)] {
        fs::write(root.join("source.py"), text).unwrap();
        let value = devmap_query::build_graph_core_value(
            std::slice::from_ref(&ext),
            &Default::default(),
            &[],
            root.to_str(),
        );
        assert!(value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["line"] == 0));
    }
    fs::write(root.join("source.py"), source).unwrap();
    let value =
        devmap_query::build_graph_core_value(&[ext], &Default::default(), &[], root.to_str());
    assert!(value["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["line"] == 2));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stored_search_paths_cannot_disclose_outside_source() {
    use devmap_query::{Request, StoreQueryEngine};
    use devmap_store::{GenerationWriteOpts, Store};
    let base = fixture("stored-search");
    let root = base.join("repo");
    fs::create_dir_all(&root).unwrap();
    let source = "def sentinel():\n    return 'outside bytes'\n";
    fs::write(base.join("outside.py"), source).unwrap();
    fs::write(root.join("ordinary.py"), source).unwrap();
    std::os::unix::fs::symlink(base.join("outside.py"), root.join("alias.py")).unwrap();
    for path in ["../outside.py", "alias.py", "ordinary.py"] {
        let exts = vec![devmap_extract::extract_file(path, source)];
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&exts);
        let resolution = resolver.resolve_all(&exts).unwrap();
        let analysis = devmap_analyze::analyze(&exts, &resolution);
        let store = Store::open_in_memory().unwrap();
        store
            .save_generation_with_opts(
                &exts,
                &resolution,
                &analysis,
                GenerationWriteOpts {
                    repo_root: Some(root.to_string_lossy().into_owned()),
                    ..Default::default()
                },
            )
            .unwrap();
        let response = StoreQueryEngine::new(&store)
            .search(Request {
                query: "sentinel".into(),
                token_budget: 2000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .unwrap();
        assert_eq!(response.items.len(), 1, "{path}");
        let hit = &response.items[0];
        if path == "ordinary.py" {
            assert!(hit.source_span.contains("outside bytes"));
            assert!(hit.source_unavailable_reason.is_none());
        } else {
            assert!(hit.source_span.is_empty(), "{path} disclosed source");
            assert!(hit.source_unavailable_reason.is_some());
        }
    }
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn source_cache_and_handler_work_have_reported_aggregate_limits() {
    let root = fixture("aggregate");
    let text = "#".repeat(1024 * 1024);
    let paths: Vec<_> = (0..65)
        .map(|index| format!("source{index:02}.py"))
        .collect();
    for path in &paths {
        fs::write(root.join(path), &text).unwrap();
    }
    let value = graph(&paths.iter().map(String::as_str).collect::<Vec<_>>());
    let mapped = route_map(&root, &value, &ScanBudget::default());
    assert_eq!(mapped["scan"]["files_read"], 64);
    assert_eq!(mapped["scan"]["files_skipped_budget"], 1);
    assert_eq!(
        mapped["scan"]["source_bytes_read"],
        mapped["scan"]["source_bytes_limit"]
    );
    assert_eq!(mapped["scan"]["complete"], false);
    let value = json!({
        "nodes":(1..=65).map(|line|json!({"id":format!("h{line}"),"path":"source00.py","kind":"function","line":line})).collect::<Vec<_>>(),
        "edges":(1..=65).map(|line|json!({"kind":"routes_to","source":format!("GET /route{line}"),"target":format!("h{line}")})).collect::<Vec<_>>()
    });
    let mapped = route_map(&root, &value, &ScanBudget::default());
    assert_eq!(mapped["scan"]["files_read"], 1);
    assert_eq!(mapped["scan"]["handler_sources_unavailable"], 1);
    assert_eq!(mapped["scan"]["complete"], false);
    assert_eq!(
        mapped["routes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|route| route["handler_keys_available"] == false)
            .count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}
