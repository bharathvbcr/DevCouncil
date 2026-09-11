//! `code_graph.json`'s `meta` must carry the graph-intelligence panels again.
//!
//! `enrich_graph_intel` wrote `god_nodes`, `hotspots`, `circular_imports`,
//! `processes`, `communities` and `node_communities` into the Python graph's
//! `meta`. `d232dea` ("retire RepoMapper.map_repo and the Python graph
//! builder") deleted both of its call sites — `build_code_graph` and
//! `refresh_map_for_paths` — and nothing has called it since. Verified two
//! ways: `rg -uu` over the tree finds the name only at its own definition
//! (`intel.py:657`) and in the lazy re-export table of `graph/__init__.py`,
//! and `git log -S enrich_graph_intel` names `d232dea` as the commit that
//! removed the callers.
//!
//! The consequence is a post-cutover regression with a visible surface:
//! `viz.py:520-522` builds the Intel tab's three panels straight off those
//! meta keys, so the God Nodes and Cycles panels of every graph HTML written
//! since that commit have rendered `(none)` — not because the repository has
//! none, but because nothing computed them.
//!
//! Ported here rather than revived in Python: the kernel already holds the
//! edges both computations need, and a second producer of an artifact the
//! kernel owns is what `d232dea` existed to remove.
//!
//! `hotspots` is ported too, and its churn half is `inventory::churn` — one
//! bounded `git log`, run only where the artifact is written. These fixtures
//! pass no repository root, so there is no history to read and the marker
//! correctly says so; `devmap-cli/tests/hotspots_are_churn_times_coupling.rs`
//! covers the computed case against a fixture with real commits.

#![cfg(feature = "parse")]

use devmap_analyze::model::{AnalysisStatus, AnalysisSummary};
use devmap_extract::extract_file;
use devmap_query::{build_code_graph_value, FreshnessInfo};
use devmap_resolve::Resolver;

/// A corpus with one obvious hub and one genuine import cycle.
///
/// `core/hub.py` is called from four other modules, so it must out-rank them.
/// `cyc/a.py` and `cyc/b.py` import each other, which is a two-file strongly
/// connected component in the file import graph.
fn graph() -> serde_json::Value {
    let extractions = vec![
        extract_file(
            "core/hub.py",
            "def shared():\n    return 1\n\n\ndef also():\n    return 2\n",
        ),
        extract_file(
            "app/one.py",
            "from core.hub import shared\n\n\ndef one():\n    return shared()\n",
        ),
        extract_file(
            "app/two.py",
            "from core.hub import shared\n\n\ndef two():\n    return shared()\n",
        ),
        extract_file(
            "app/three.py",
            "from core.hub import shared\n\n\ndef three():\n    return shared()\n",
        ),
        extract_file(
            "app/four.py",
            "from core.hub import also\n\n\ndef four():\n    return also()\n",
        ),
        extract_file(
            "cyc/a.py",
            "from cyc.b import bee\n\n\ndef ay():\n    return bee()\n",
        ),
        extract_file(
            "cyc/b.py",
            "from cyc.a import ay\n\n\ndef bee():\n    return 1\n",
        ),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = AnalysisSummary {
        total_files: extractions.len(),
        total_symbols: 8,
        total_edges: resolution.edges.len(),
        status: AnalysisStatus::Ok,
        ..Default::default()
    };
    build_code_graph_value(
        &extractions,
        &analysis,
        &resolution.edges,
        &FreshnessInfo::new("head".into(), 1, 0),
        None,
    )
    .expect("the graph builds")
}

#[test]
fn god_nodes_rank_the_hubs_the_graph_actually_has() {
    let value = graph();
    let gods = value["meta"]["god_nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("meta.god_nodes is not an array: {}", value["meta"]));
    assert!(
        !gods.is_empty(),
        "the corpus has a symbol reached from four modules and the panel is empty"
    );
    // The shape viz.py:967 indexes: it reads `gn.name||gn.path||gn.id`, then
    // `gn.degree`, `gn.fan_in`, `gn.fan_out`. A missing key renders the string
    // "undefined" into the HTML rather than failing, so the keys are asserted
    // rather than assumed.
    for entry in gods {
        for key in ["id", "path", "name", "degree", "fan_in", "fan_out"] {
            assert!(
                entry.get(key).is_some(),
                "god node entry is missing {key}, which viz.py renders directly: {entry}"
            );
        }
    }
    let top = &gods[0];
    assert!(
        top["id"].as_str().unwrap_or_default().contains("hub.py"),
        "the most-connected node is not the hub four modules import: {top}"
    );
    // Ranking has to be by measured degree, not by insertion order.
    let degrees: Vec<i64> = gods
        .iter()
        .map(|entry| entry["degree"].as_i64().unwrap_or_default())
        .collect();
    assert!(
        degrees.windows(2).all(|pair| pair[0] >= pair[1]),
        "god nodes are not ordered by degree: {degrees:?}"
    );
}

#[test]
fn a_genuine_import_cycle_is_reported() {
    let value = graph();
    let cycles = value["meta"]["circular_imports"]
        .as_array()
        .unwrap_or_else(|| panic!("meta.circular_imports is not an array: {}", value["meta"]));
    assert_eq!(
        cycles.len(),
        1,
        "cyc/a.py and cyc/b.py import each other and nothing else does: {cycles:?}"
    );
    let cycle = &cycles[0];
    assert_eq!(cycle["length"], serde_json::json!(2));
    let nodes: Vec<&str> = cycle["nodes"]
        .as_array()
        .expect("nodes is an array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        nodes,
        vec!["cyc/a.py", "cyc/b.py"],
        "the cycle's members must be file paths, sorted, as viz.py:971 joins them"
    );
}

#[test]
fn the_intel_says_what_it_computed_and_what_it_did_not() {
    let value = graph();
    let provenance = &value["meta"]["devmap_rust"];
    assert_eq!(
        provenance["god_nodes_computed"],
        serde_json::json!(true),
        "an empty god-node list has to be distinguishable from an absent pass: {provenance}"
    );
    assert_eq!(
        provenance["circular_imports_computed"],
        serde_json::json!(true)
    );
    // No repository root reaches this fixture, so no history was read. Saying
    // so is the whole point: the panel has been empty since the cutover and
    // nothing in the artifact explained which kind of empty it was.
    assert_eq!(
        provenance["hotspots_computed"],
        serde_json::json!(false),
        "no repository root here, so the churn read cannot have happened"
    );
    assert!(
        provenance["hotspots_unavailable_reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "an uncomputed hotspot list must name why: {provenance}"
    );
    // Every capped list carries its own bounds, so a reader never mistakes the
    // sample for the population.
    for key in [
        "god_nodes_shown",
        "god_nodes_total",
        "circular_imports_shown",
        "circular_imports_total",
        "hotspots_shown",
        "hotspots_total",
    ] {
        assert!(
            provenance[key].is_number(),
            "{key} is missing, so the cap is invisible: {provenance}"
        );
    }
    assert_eq!(provenance["god_nodes_truncated"], serde_json::json!(false));
    assert_eq!(
        provenance["circular_imports_truncated"],
        serde_json::json!(false)
    );
}

/// The cap has to report itself when it actually bites, not only when it does
/// not. A `truncated` that is false on every input is the same defect as a
/// missing one.
#[test]
fn a_capped_god_node_list_says_it_was_capped() {
    // More distinct hubs than GOD_NODE_CAP, each reached from one caller, so
    // the ranking has more candidates than it can seat.
    let mut extractions = Vec::new();
    let mut hub_body = String::new();
    for index in 0..40 {
        hub_body.push_str(&format!("def hub{index}():\n    return {index}\n\n\n"));
    }
    extractions.push(extract_file("core/hub.py", &hub_body));
    for index in 0..40 {
        extractions.push(extract_file(
            &format!("app/mod{index}.py"),
            &format!(
                "from core.hub import hub{index}\n\n\ndef use{index}():\n    return hub{index}()\n"
            ),
        ));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = AnalysisSummary {
        total_files: extractions.len(),
        total_symbols: 80,
        total_edges: resolution.edges.len(),
        status: AnalysisStatus::Ok,
        ..Default::default()
    };
    let value = build_code_graph_value(
        &extractions,
        &analysis,
        &resolution.edges,
        &FreshnessInfo::new("head".into(), 1, 0),
        None,
    )
    .expect("the graph builds");
    let provenance = &value["meta"]["devmap_rust"];
    let shown = provenance["god_nodes_shown"].as_u64().unwrap_or_default();
    let total = provenance["god_nodes_total"].as_u64().unwrap_or_default();
    assert!(
        total > shown,
        "this corpus has more ranked nodes than the cap seats: shown={shown} total={total}"
    );
    assert_eq!(
        provenance["god_nodes_truncated"],
        serde_json::json!(true),
        "the cap bit and the artifact does not say so: {provenance}"
    );
    assert_eq!(
        value["meta"]["god_nodes"].as_array().map(Vec::len),
        Some(shown as usize),
        "shown must count the list that was actually emitted"
    );
}

/// Determinism (R4). Ranked output whose ties are broken by hash order renders
/// different bytes for one generation.
#[test]
fn the_intel_is_deterministic() {
    let first = graph();
    let second = graph();
    assert_eq!(first["meta"]["god_nodes"], second["meta"]["god_nodes"]);
    assert_eq!(
        first["meta"]["circular_imports"],
        second["meta"]["circular_imports"]
    );
}
