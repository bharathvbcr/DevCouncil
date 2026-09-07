//! `subsystems[].neighbors` must see inheritance, not only calls and imports.
//!
//! `area_adjacency` admitted `Calls | References | Imports`, matching
//! `backend/go_orchestrator/repomap`'s `couplingKinds` exactly — the two are
//! deliberately one derivation, and its own doc comment says so. Both were
//! narrower than the graph: `EdgeKind::Extends`, `Implements` and
//! `HandlesRoute` are emitted (as `inherits`, `implements` and `routes_to`,
//! `code_graph.rs:154-157`) and neither consumer counted them.
//!
//! Why it matters, reproduced against the release binary on a two-directory
//! Ruby corpus — `core/base.rb` declaring `BaseRecord`, `web/user.rb`
//! declaring `class UserRecord < BaseRecord`, no `require`:
//!
//! ```text
//! Counter({('contains','extracted'): 6, ('inherits','extracted'): 1})
//!   inherits extracted | web/user.rb::UserRecord -> core/base.rb::BaseRecord
//! ```
//!
//! One class derives directly from the other and the generation holds exactly
//! one edge saying so. Ruby autoloads rather than requires in the common case,
//! so nothing emits the `imports` edge that would have carried the same fact —
//! and imports are the *weakest* covered relation here, absent entirely in 24
//! of the 35 languages the extractor handles, among them Java, C#, Swift,
//! Kotlin and Scala, where inheritance is the primary cross-area binding.
//!
//! The consequence is a scope decision: `neighbors` is read by the write gate
//! to decide whether a file is near the plan, so under the narrow set an edit
//! spanning a subclass and its base class is `architecture_drift`.

#![cfg(feature = "parse")]

use devmap_analyze::model::{AnalysisStatus, AnalysisSummary, CommunityReport};
use devmap_extract::extract_file;
use devmap_query::{generate_manifest_with_edges, FreshnessInfo};
use devmap_resolve::Resolver;

/// Two areas whose only connection is inheritance. Ruby, and without a
/// `require_relative`, because that is the shape that has no import edge to
/// fall back on — the same corpus the module docs quote.
fn manifest() -> serde_json::Value {
    let extractions = vec![
        extract_file(
            "core/base.rb",
            "class BaseRecord\n  def save\n    true\n  end\nend\n",
        ),
        extract_file(
            "web/user.rb",
            "class UserRecord < BaseRecord\n  def name\n    \"u\"\n  end\nend\n",
        ),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let communities = ["core/base.rb", "web/user.rb"]
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
        total_files: extractions.len(),
        total_symbols: 4,
        total_edges: resolution.edges.len(),
        communities,
        status: AnalysisStatus::Ok,
        ..Default::default()
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
fn inheritance_alone_couples_two_areas() {
    let map = manifest();
    // Guard the fixture before the assertion it supports: if the extractor
    // stopped emitting the inheritance edge, the neighbour assertion below
    // would fail for a reason that has nothing to do with what this test is
    // about, and a green run after a "fix" would mean nothing.
    let mut resolver = Resolver::new();
    let extractions = vec![
        extract_file(
            "core/base.rb",
            "class BaseRecord\n  def save\n    true\n  end\nend\n",
        ),
        extract_file(
            "web/user.rb",
            "class UserRecord < BaseRecord\n  def name\n    \"u\"\n  end\nend\n",
        ),
    ];
    resolver.index_extractions(&extractions);
    let edges = resolver.resolve_all(&extractions).edges;
    let cross: Vec<_> = edges
        .iter()
        .filter(|edge| edge.source_file != edge.target_file)
        .map(|edge| (edge.edge_kind, edge.confidence.0))
        .collect();
    assert!(
        !cross.is_empty(),
        "the fixture has no cross-file edge at all, so it cannot show what \
         neighbours misses: {edges:#?}"
    );

    assert_eq!(
        neighbours_of(&map, "core"),
        vec!["web".to_string()],
        "web/user.rb::UserRecord derives from core/base.rb::BaseRecord — the only \
         edge between the two areas — and the coupling must be recorded. \
         Cross-file edges in this generation: {cross:?}"
    );
    assert_eq!(
        neighbours_of(&map, "web"),
        vec!["core".to_string()],
        "and symmetrically: a base class is as coupled to its subclass as the \
         subclass is to it"
    );
}

#[test]
fn the_widened_coupling_is_still_counted_in_the_meta() {
    let map = manifest();
    let meta = &map["liveness_meta"]["subsystems"];
    assert_eq!(
        meta["neighbors_total"],
        serde_json::json!(2),
        "two directed couplings (core->web and web->core) must be counted, not \
         just listed: {meta}"
    );
}
