//! `subsystems[].neighbors` must be a computed answer, not a literal.
//!
//! `build_repo_map_value` emitted `"neighbors": []` for every subsystem, for
//! the field's whole life in this kernel. Two consumers read it and neither can
//! tell an empty list from an absent feature:
//!
//! - `src/devcouncil/indexing/subsystem_map.py::are_neighbors` answers `False`
//!   for every distinct pair of areas, so `execution/policy_engine.py`'s "File
//!   is in a neighboring subsystem of a planned file" rung can never fire, and
//!   `verification/checks/subsystem_boundary.py` raises `architecture_drift`
//!   for every cross-area change.
//! - `backend/go_orchestrator/repomap` gave up on the field and derives
//!   adjacency itself, saying so in its package doc: "the Rust producer emits
//!   `\"neighbors\": []` unconditionally — the field is a literal in its
//!   manifest writer".
//!
//! Measured on this repository's `.devcouncil/repo_map.json` before the fix:
//! 16 subsystems, 0 with a non-empty `neighbors`. On the scholarlm map: 12
//! subsystems, 0 non-empty.
//!
//! The derivation matches the Go reader's, so the two cannot disagree: two
//! areas are neighbours when a `calls`/`references`/`imports` edge at
//! `extracted` confidence runs between a symbol in one and a symbol in the
//! other. An ambiguous edge is a resolution the analyser declined to make, and
//! a scope decision may not rest on one.

#![cfg(feature = "parse")]

use devmap_analyze::model::{AnalysisStatus, AnalysisSummary, CommunityReport};
use devmap_extract::extract_file;
use devmap_query::{generate_manifest_with_edges, FreshnessInfo};
use devmap_resolve::Resolver;

/// Three areas. `beta` calls into `alpha`; `gamma` is coupled to nothing.
fn manifest() -> serde_json::Value {
    let extractions = vec![
        extract_file("alpha/core.py", "def helper(rows):\n    return sum(rows)\n"),
        extract_file(
            "beta/app.py",
            "from alpha.core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
        ),
        extract_file("gamma/lonely.py", "def alone():\n    return 1\n"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let communities = ["alpha/core.py", "beta/app.py", "gamma/lonely.py"]
        .iter()
        .enumerate()
        .map(|(index, path)| CommunityReport {
            community_id: index as u32,
            name: format!("community-{index}"),
            members: vec![(*path).to_string()],
            cohesion_score: 1.0,
        })
        .collect();
    let analysis = AnalysisSummary {
        discovery_refused_files: None,
        total_files: extractions.len(),
        total_symbols: 3,
        total_edges: resolution.edges.len(),
        dead_symbols: Vec::new(),
        communities,
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
    };
    let (_, json) = generate_manifest_with_edges(
        &extractions,
        &analysis,
        FreshnessInfo::new("head".into(), 1, 0),
        &resolution.edges,
    );
    serde_json::from_str(&json).expect("the manifest is JSON")
}

fn neighbours_of(map: &serde_json::Value, area: &str) -> Vec<String> {
    map["subsystems"]
        .as_array()
        .expect("subsystems is an array")
        .iter()
        .find(|entry| entry["area"] == area)
        .unwrap_or_else(|| panic!("no subsystem for {area}: {}", map["subsystems"]))["neighbors"]
        .as_array()
        .expect("neighbors is an array")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn a_cross_area_call_makes_each_area_the_other_s_neighbour() {
    let map = manifest();
    assert_eq!(
        neighbours_of(&map, "alpha"),
        vec!["beta".to_string()],
        "beta calls into alpha, so the coupling must be recorded on alpha"
    );
    assert_eq!(
        neighbours_of(&map, "beta"),
        vec!["alpha".to_string()],
        "and symmetrically on beta: a reference in one direction is a coupling"
    );
}

#[test]
fn an_area_with_no_cross_area_edge_gets_a_computed_empty_list() {
    let map = manifest();
    assert!(
        neighbours_of(&map, "gamma").is_empty(),
        "gamma references nothing outside itself; its empty list is the answer"
    );
}

/// The Class A half. A consumer must be able to tell an empty list that was
/// *computed* from a producer that stubs the field — that is precisely the
/// distinction the Go reader could not make, and why it derives adjacency
/// itself rather than reading this artifact.
#[test]
fn the_manifest_states_that_neighbours_were_computed() {
    let map = manifest();
    let meta = &map["liveness_meta"]["subsystems"];
    assert_eq!(
        meta["neighbors_computed"],
        serde_json::json!(true),
        "the manifest must say the field was derived, not left at a literal: {meta}"
    );
    assert_eq!(
        meta["neighbors_total"],
        serde_json::json!(2),
        "two directed couplings were found (alpha->beta and beta->alpha): {meta}"
    );
    assert_eq!(
        meta["neighbors_truncated"],
        serde_json::json!(false),
        "nothing was cut here: {meta}"
    );
}

/// Determinism (R4): the same generation must render the same bytes.
#[test]
fn the_neighbour_lists_are_deterministic() {
    let first = manifest();
    let second = manifest();
    assert_eq!(
        first["subsystems"], second["subsystems"],
        "two renderings of one generation disagree"
    );
}
