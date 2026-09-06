//! `subsystems[].handoff_paths` must be a computed answer, not a literal.
//!
//! `build_repo_map_value` emitted `"handoff_paths": []` for every subsystem,
//! for the field's whole life in this kernel — the same defect
//! `subsystems[].neighbors` carried until it was derived from the stored
//! edges. A reader cannot tell "this subsystem hands off to nothing" from
//! "this producer does not compute the field", and the guides tell agents to
//! navigate by it: `indexing/map_artifacts.py` writes step 6 of the generated
//! agent guide as "Use `neighbors` and `handoff_paths` in `subsystems` to
//! follow cross-subsystem flow", so every agent that followed the instruction
//! read an empty list as "there is no cross-subsystem flow here".
//!
//! Measured on this repository's `.devcouncil/repo_map.json` before the fix:
//! every subsystem carried an empty `handoff_paths`.
//!
//! The derivation shares one sweep with `neighbors` so the two can never
//! disagree: a handoff is the ordered file pair `source -> target` behind a
//! `calls`/`references`/`imports` edge at `extracted` confidence that leaves
//! this subsystem's area for another. `neighbors` is that relation made
//! symmetric and projected onto areas; `handoff_paths` is the same relation
//! kept directed and kept at file granularity. An ambiguous edge is a
//! resolution the analyser declined to make, and is excluded from both.

#![cfg(feature = "parse")]

use devmap_analyze::model::{AnalysisStatus, AnalysisSummary, CommunityReport};
use devmap_extract::extract_file;
use devmap_query::{generate_manifest_with_edges, FreshnessInfo};
use devmap_resolve::Resolver;

fn manifest_for(extractions: Vec<devmap_extract::model::Extraction>) -> serde_json::Value {
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let communities = extractions
        .iter()
        .enumerate()
        .map(|(index, ext)| CommunityReport {
            community_id: index as u32,
            name: format!("community-{index}"),
            members: vec![ext.file_path.clone()],
            cohesion_score: 1.0,
        })
        .collect();
    let analysis = AnalysisSummary {
        discovery_refused_files: None,
        total_files: extractions.len(),
        total_symbols: extractions.len(),
        total_edges: resolution.edges.len(),
        dead_symbols: Vec::new(),
        communities,
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
        resolution_rate: Default::default(),
    };
    let (_, json) = generate_manifest_with_edges(
        &extractions,
        &analysis,
        FreshnessInfo::new("head".into(), 1, 0),
        &resolution.edges,
    );
    serde_json::from_str(&json).expect("the manifest is JSON")
}

/// Three areas. `beta` calls into `alpha`; `gamma` is coupled to nothing.
fn manifest() -> serde_json::Value {
    manifest_for(vec![
        extract_file("alpha/core.py", "def helper(rows):\n    return sum(rows)\n"),
        extract_file(
            "beta/app.py",
            "from alpha.core import helper\n\n\ndef run(rows):\n    return helper(rows)\n",
        ),
        extract_file("gamma/lonely.py", "def alone():\n    return 1\n"),
    ])
}

fn handoffs_of(map: &serde_json::Value, area: &str) -> Vec<String> {
    map["subsystems"]
        .as_array()
        .expect("subsystems is an array")
        .iter()
        .find(|entry| entry["area"] == area)
        .unwrap_or_else(|| panic!("no subsystem for {area}: {}", map["subsystems"]))
        ["handoff_paths"]
        .as_array()
        .expect("handoff_paths is an array")
        .iter()
        .map(|value| value.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The directed half. `beta` imports `alpha`, so the handoff is recorded on
/// `beta` — the subsystem the flow *leaves* — and names both files.
#[test]
fn a_cross_area_call_records_the_ordered_file_pair_on_the_calling_area() {
    let map = manifest();
    assert_eq!(
        handoffs_of(&map, "beta"),
        vec!["beta/app.py -> alpha/core.py".to_string()],
        "beta calls into alpha, so beta hands off to alpha through this file pair"
    );
}

/// The direction is the answer, not an accident of iteration order. `neighbors`
/// is symmetric because a coupling binds both areas; a *handoff* is not, and
/// recording it on both would tell a reader that `alpha` calls into `beta`.
#[test]
fn the_called_area_does_not_claim_the_reverse_handoff() {
    let map = manifest();
    assert!(
        handoffs_of(&map, "alpha").is_empty(),
        "alpha is called into and calls out to nothing; the reverse pair would be a false claim"
    );
}

#[test]
fn an_area_with_no_cross_area_edge_gets_a_computed_empty_list() {
    let map = manifest();
    assert!(
        handoffs_of(&map, "gamma").is_empty(),
        "gamma references nothing outside itself; its empty list is the answer"
    );
}

/// The Class A half. A consumer must be able to tell an empty list that was
/// *computed* from a producer that stubs the field. Without the marker every
/// Python reader of `handoff_paths` — the prompt builder's "Cross-subsystem
/// flow" line, the wiki page, the MCP subsystem detail, the graph HTML — reads
/// a stub as a measured negative.
#[test]
fn the_manifest_states_that_handoff_paths_were_computed() {
    let map = manifest();
    // The flag lives beside `neighbors_computed`, under the key
    // `code_graph.json` already uses for a producer's account of its own run;
    // the counts live beside the artifact's other shown/total pairs.
    assert_eq!(
        map["meta"]["devmap_rust"]["handoff_paths_computed"],
        serde_json::json!(true),
        "the manifest must say the field was derived, not left at a literal: {}",
        map["meta"]
    );
    let meta = &map["liveness_meta"]["subsystems"];
    assert_eq!(
        meta["handoff_paths_total"],
        serde_json::json!(1),
        "one directed handoff was found (beta/app.py -> alpha/core.py): {meta}"
    );
    assert_eq!(
        meta["handoff_paths_shown"],
        serde_json::json!(1),
        "and it was emitted rather than cut: {meta}"
    );
    assert_eq!(
        meta["handoff_paths_truncated"],
        serde_json::json!(false),
        "nothing was cut here: {meta}"
    );
}

/// Determinism (R4): the same generation must render the same bytes.
#[test]
fn the_handoff_lists_are_deterministic() {
    let first = manifest();
    let second = manifest();
    assert_eq!(
        first["subsystems"], second["subsystems"],
        "two renderings of one generation disagree"
    );
}

/// Every endpoint must be a path the consumer can resolve to a real file.
///
/// Both Python readers that validate the field split on `->` and assert each
/// half is an indexed path (`tests/unit/test_repo_mapper.py`,
/// `tests/unit/test_repo_map_artifact.py`). The resolver also emits edges whose
/// endpoint is the *synthetic* Go package node (`package:<dir>/<pkg>`), which is
/// not a file and which no path will ever match — the same trap that put a
/// `package:` string in a neighbour list.
#[test]
fn no_handoff_endpoint_is_a_synthetic_node_the_consumer_can_never_match() {
    let map = manifest_for(vec![
        extract_file(
            "gopkg/lib/lib.go",
            "package lib\n\nfunc Helper() int { return 1 }\n",
        ),
        extract_file(
            "gopkg/app/app.go",
            "package app\n\nimport \"gopkg/lib\"\n\nfunc Run() int { return lib.Helper() }\n",
        ),
    ]);

    let subsystems = map["subsystems"].as_array().expect("subsystems");
    assert!(
        !subsystems.is_empty(),
        "the Go fixture produced no subsystems, so this proves nothing"
    );
    // The coupling itself must survive: dropping the synthetic endpoint
    // entirely would satisfy the loop below while losing a real import.
    assert_eq!(
        handoffs_of(&map, "gopkg/app"),
        vec!["gopkg/app/app.go -> gopkg/lib/lib.go".to_string()],
        "the Go import must reach the package member's own file, not vanish"
    );
    for entry in subsystems {
        for handoff in entry["handoff_paths"].as_array().expect("handoff_paths") {
            let text = handoff.as_str().unwrap_or_default();
            assert!(
                !text.contains("package:"),
                "{} names the synthetic node in {text:?}; no file path resolves to it",
                entry["area"]
            );
            let halves: Vec<&str> = text.split(" -> ").collect();
            assert_eq!(
                halves.len(),
                2,
                "{text:?} is not the `source -> target` shape both Python readers split on"
            );
        }
    }
}

/// A handoff never leaves its own area, and never contradicts `neighbors`.
///
/// The two fields come from one sweep; this is the assertion that keeps them
/// from drifting apart if that ever stops being true. Every handoff recorded on
/// an area must start inside it and land in an area that area calls a
/// neighbour.
#[test]
fn every_handoff_starts_in_its_own_area_and_lands_on_a_neighbour() {
    let map = manifest();
    for entry in map["subsystems"].as_array().expect("subsystems") {
        let area = entry["area"].as_str().expect("area");
        let neighbours: Vec<&str> = entry["neighbors"]
            .as_array()
            .expect("neighbors")
            .iter()
            .map(|value| value.as_str().unwrap_or_default())
            .collect();
        for handoff in entry["handoff_paths"].as_array().expect("handoff_paths") {
            let text = handoff.as_str().unwrap_or_default();
            let (source, target) = text.split_once(" -> ").expect("the `a -> b` shape");
            assert!(
                source.starts_with(&format!("{area}/")),
                "{area} claims a handoff starting outside itself: {text:?}"
            );
            assert!(
                !target.starts_with(&format!("{area}/")),
                "{area} claims a handoff that never leaves it: {text:?}"
            );
            let landed = target.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
            assert!(
                neighbours.contains(&landed),
                "{area} hands off to {landed:?}, which is absent from its neighbours {neighbours:?}"
            );
        }
    }
}
