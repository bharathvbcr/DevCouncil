//! `cypher`, `routes`, `shape-check` and `api-impact` read two arrays of the
//! exported graph — `nodes` and `edges` — and nothing else in it. Measured on
//! this repository's store (release binary, n=7):
//!
//! ```text
//! cypher       643 ms     search  8 ms
//! ```
//!
//! Every one of those milliseconds beyond the two arrays was the rest of the
//! artifact: a `git log` for the churn panel (122 ms), the intel panels, the
//! dead-code list, the subsystem summary. The core value stops at the arrays,
//! and this test is the proof that it stops *after* the same rows: node for
//! node and edge for edge, the core is what the artifact carries.

use devmap_analyze::model::{AnalysisStatus, AnalysisSummary};
use devmap_extract::extract_file;
use devmap_query::{build_code_graph_value, build_graph_core_value, FreshnessInfo};
use devmap_resolve::Resolver;

/// A corpus with declarations on several lines, one route, one import cycle
/// and one duplicate identity, so every branch of the node loop runs.
fn corpus() -> Vec<devmap_extract::model::Extraction> {
    vec![
        extract_file(
            "core/hub.py",
            "def shared():\n    return 1\n\n\ndef also():\n    return 2\n",
        ),
        extract_file(
            "app/one.py",
            "from core.hub import shared\n\n\ndef one():\n    return shared()\n",
        ),
        extract_file(
            "app/routes.py",
            "from flask import Flask\napp = Flask(__name__)\n\n\n@app.route(\"/x\", methods=[\"GET\"])\ndef x():\n    return \"x\"\n",
        ),
        extract_file(
            "cyc/a.py",
            "from cyc.b import bee\n\n\ndef ay():\n    return bee()\n",
        ),
        extract_file(
            "cyc/b.py",
            "from cyc.a import ay\n\n\ndef bee():\n    return 1\n\n\ndef bee():\n    return 2\n",
        ),
    ]
}

#[test]
fn the_core_graph_carries_the_artifacts_nodes_and_edges_and_nothing_else() {
    let extractions = corpus();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = AnalysisSummary {
        total_files: extractions.len(),
        total_symbols: extractions.iter().map(|ext| ext.symbols.len()).sum(),
        total_edges: resolution.edges.len(),
        status: AnalysisStatus::Ok,
        ..Default::default()
    };

    let artifact = build_code_graph_value(
        &extractions,
        &analysis,
        &resolution.edges,
        &FreshnessInfo::new("head".into(), 1, 0),
        None,
    )
    .expect("the artifact builds");
    let core = build_graph_core_value(&extractions, &analysis, &resolution.edges, None);

    assert_eq!(
        core["nodes"], artifact["nodes"],
        "the core's nodes must be the artifact's nodes, row for row"
    );
    assert_eq!(
        core["edges"], artifact["edges"],
        "the core's edges must be the artifact's edges, row for row"
    );
    assert!(
        !artifact["nodes"].as_array().unwrap().is_empty()
            && !artifact["edges"].as_array().unwrap().is_empty(),
        "an empty fixture proves nothing"
    );
    assert_eq!(core["schema_version"], artifact["schema_version"]);

    let mut keys: Vec<&str> = core
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["edges", "nodes", "schema_version"],
        "the core stops at the two arrays; a reader wanting a panel takes the artifact"
    );
}
