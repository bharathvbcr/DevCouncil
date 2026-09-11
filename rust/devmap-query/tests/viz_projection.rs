use devmap_query::viz::{build_payload, VizOptions};
use serde_json::{json, Value};

fn graph() -> Value {
    json!({"nodes": [
        {"id":"file-a", "kind":"file", "path":"src/A.swift"},
        {"id":"file-b", "kind":"file", "path":"src/B.swift"},
        {"id":"sym-a", "kind":"function", "path":"src/A.swift"},
        {"id":"sym-a2", "kind":"function", "path":"src/A.swift"},
        {"id":"sym-b", "kind":"function", "path":"src/B.swift"}
    ], "edges": [
        {"source":"sym-a", "target":"sym-b", "kind":"calls", "confidence":0.9, "resolution":"exact"},
        {"source":"sym-a2", "target":"sym-b", "kind":"calls", "confidence":0.6, "resolution":"inferred"},
        {"source":"sym-b", "target":"sym-a", "kind":"references"},
        {"source":"sym-a", "target":"sym-a2", "kind":"calls", "confidence":1.0},
        {"source":"file-a", "target":"sym-a", "kind":"contains"}
    ]})
}

#[test]
fn file_view_projects_symbol_evidence_using_paths_not_id_syntax() {
    let payload = build_payload(&graph(), &VizOptions::default());
    let links = payload["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    let calls = links.iter().find(|l| l["kind"] == "calls").unwrap();
    assert_eq!(calls["source"], "file-a");
    assert_eq!(calls["target"], "file-b");
    assert_eq!(calls["evidence_count"], 2);
    assert_eq!(calls["confidence"], 0.6);
    assert_eq!(calls["resolution"], "mixed");
    let references = links.iter().find(|l| l["kind"] == "references").unwrap();
    assert!(
        references["confidence"].is_null(),
        "unknown confidence must stay unknown"
    );
    assert_eq!(payload["meta"]["projection"]["internal_edges_omitted"], 1);
}

#[test]
fn symbol_view_keeps_references_and_opaque_symbol_ids() {
    let payload = build_payload(
        &graph(),
        &VizOptions {
            symbols: true,
            ..Default::default()
        },
    );
    assert_eq!(payload["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(payload["links"].as_array().unwrap().len(), 4);
    assert!(payload["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["kind"] == "references"));
}

#[test]
fn missing_and_ambiguous_endpoints_never_become_phantom_dependencies() {
    let mut graph = graph();
    graph["nodes"].as_array_mut().unwrap().extend([
        json!({"id":"file-duplicate", "kind":"file", "path":"src/B.swift"}),
        json!({"id":"duplicate", "kind":"function", "path":"src/A.swift"}),
        json!({"id":"duplicate", "kind":"function", "path":"src/B.swift"}),
        json!({"name":"no id"}),
        Value::Null,
    ]);
    graph["edges"].as_array_mut().unwrap().extend([
        json!({"source":"missing::symbol", "target":"sym-a", "kind":"calls"}),
        json!({"source":"duplicate", "target":"sym-a", "kind":"calls"}),
        Value::Null,
    ]);
    let payload = build_payload(&graph, &VizOptions::default());
    assert!(payload["links"].as_array().unwrap().is_empty());
    assert!(
        payload["meta"]["projection"]["invalid_edges"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(payload["meta"]["projection"]["invalid_nodes"], 4);
}

#[test]
fn projection_is_stable_under_order_changes_and_merges_duplicate_evidence() {
    let mut graph = graph();
    let repeated = graph["edges"][0].clone();
    graph["edges"].as_array_mut().unwrap().push(repeated);
    let before = build_payload(&graph, &VizOptions::default());
    graph["nodes"].as_array_mut().unwrap().reverse();
    graph["edges"].as_array_mut().unwrap().reverse();
    assert_eq!(before, build_payload(&graph, &VizOptions::default()));
    let calls = before["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["kind"] == "calls")
        .unwrap();
    assert_eq!(calls["evidence_count"], 2);
}

#[test]
fn caps_count_projected_relationships_and_never_keep_cut_endpoints() {
    let graph = graph();
    for cap in [0, 1, 2, usize::MAX] {
        let payload = build_payload(
            &graph,
            &VizOptions {
                max_nodes: cap,
                ..Default::default()
            },
        );
        assert_eq!(payload["counts"]["links_total"], 2);
        assert_eq!(payload["counts"]["nodes_total"], 2);
        for edge in payload["links"].as_array().unwrap() {
            for end in ["source", "target"] {
                assert!(payload["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|n| n["id"] == edge[end]));
            }
        }
    }
}

#[test]
fn dense_projection_bounds_rendered_links_and_preserves_total_coverage() {
    let nodes: Vec<Value> = (0..350)
        .flat_map(|i| {
            [
                json!({"id":format!("{i}.rs"), "kind":"file", "path":format!("{i}.rs")}),
                json!({"id":format!("symbol-{i}"), "kind":"function", "path":format!("{i}.rs")}),
            ]
        })
        .collect();
    let edges: Vec<Value> = (0..350).flat_map(|i| (0..350).filter(move |j| *j != i).map(move |j|
        json!({"source":format!("symbol-{i}"), "target":format!("symbol-{j}"), "kind":"calls", "confidence":1.0})
    )).collect();
    let started = std::time::Instant::now();
    let payload = build_payload(
        &json!({"nodes":nodes,"edges":edges}),
        &VizOptions::default(),
    );
    assert_eq!(payload["counts"]["links_shown"], 50_000);
    assert_eq!(payload["counts"]["links_total"], 122_150);
    assert_eq!(payload["counts"]["nodes_shown"], 350);
    assert!(
        started.elapsed().as_secs() < 30,
        "dense projection exceeded its stress-test budget"
    );
    eprintln!("122150-edge projection: {:?}", started.elapsed());
}
