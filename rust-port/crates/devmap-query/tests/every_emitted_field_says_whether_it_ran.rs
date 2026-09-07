//! No field ships in every response at the same constant value without saying so.
//!
//! `repo_map.json` carried eight fields that were the same value on every
//! repository the kernel has ever mapped:
//!
//! | field | value | consumers |
//! |---|---|---|
//! | `subsystems[].summary` | `""` (a literal, manifest.rs:383) | `wiki.py:214,311,360`, `map_artifacts.py:69`, `mcp/handlers/map.py:147` |
//! | `frameworks` | `[]` | `wiki.py:155,285`, `mcp/handlers/map.py:327` |
//! | `package_managers` | `[]` | `wiki.py`, `mcp/handlers/map.py` |
//! | `test_commands` | `[]` | `wiki.py`, `mcp/handlers/map.py` |
//! | `candidate_files` | `[]` | `map_artifacts.py:379` (fills it on `--goal`) |
//! | `lsp` | `{}` | none found in `src/` |
//! | `dependency_risks` | `[]` | `prompt_builder.py:830`, `map_artifacts.py:383` (fills it on `--scan-deps`) |
//! | `processes` | `[]` | none found in `src/` for the manifest's copy |
//!
//! None carried a `*_computed` marker, so a consumer could not tell "this
//! repository has none" from "this producer never looked" — the same defect
//! `neighbors_computed`, `handoff_paths_computed`, `role_files_computed` and
//! `file_kinds_computed` were added to fix for four other fields. The map's own
//! comments name the rule: a degraded flag that is always on carries no
//! information, and an empty list nothing computed is exactly that.
//!
//! Two are now computed from evidence the kernel already holds. The remaining
//! six are marked `false`, each because the kernel cannot see what it would
//! need: `package_managers` is a lockfile question and lockfiles are not
//! indexed (`.lock` has no language spec, so `is_indexable_source` excludes
//! them), `test_commands` needs manifest *contents*, `candidate_files` is
//! goal-dependent, `dependency_risks` needs an SCA run, and `lsp`/`processes`
//! have no producer in this kernel at all. Marked, not guessed. No third state.

#![cfg(feature = "parse")]

use devmap_analyze::model::{AnalysisStatus, AnalysisSummary, CommunityReport};
use devmap_extract::extract_file;
use devmap_query::{generate_manifest_with_edges, FreshnessInfo};
use devmap_resolve::Resolver;

/// A corpus with a Flask route, so `frameworks` has something true to say, and
/// two areas so the subsystem summaries have composition to describe.
fn manifest() -> serde_json::Value {
    let extractions = vec![
        extract_file(
            "api/routes.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n\
             @app.route(\"/health\")\ndef health():\n    return \"ok\"\n\n\n\
             @app.post(\"/users\")\ndef create_user():\n    return \"\"\n",
        ),
        extract_file("api/helpers.py", "def shape(row):\n    return row\n"),
        extract_file("core/model.py", "class Row:\n    pass\n"),
        extract_file("core/test_model.py", "def test_row():\n    assert True\n"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let communities = ["api/routes.py", "core/model.py"]
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
        total_symbols: 5,
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

/// The general rule, asserted over the whole set rather than field by field:
/// every one of the eight either carries a value or carries a marker saying it
/// was not computed. A field that does neither is back to being unreadable.
#[test]
fn no_always_empty_field_ships_without_provenance() {
    let map = manifest();
    let meta = &map["meta"]["devmap_rust"];
    for (field, marker) in [
        ("frameworks", "frameworks_computed"),
        ("package_managers", "package_managers_computed"),
        ("test_commands", "test_commands_computed"),
        ("candidate_files", "candidate_files_computed"),
        ("lsp", "lsp_computed"),
        ("dependency_risks", "dependency_risks_computed"),
        ("processes", "processes_computed"),
    ] {
        assert!(
            map.get(field).is_some(),
            "{field} vanished from the manifest; its Python readers index it directly"
        );
        assert!(
            meta.get(marker).is_some_and(serde_json::Value::is_boolean),
            "{field} ships in every response with no {marker}, so a consumer cannot tell \
             'none' from 'never looked': {meta}"
        );
    }
    assert!(
        meta.get("subsystem_summaries_computed")
            .is_some_and(serde_json::Value::is_boolean),
        "subsystems[].summary has no provenance marker: {meta}"
    );
}

/// `frameworks` is computable and is now computed. The evidence is the route
/// extractor's own `framework` field — a framework is named because its routes
/// were found, not because a filename looked like one.
#[test]
fn frameworks_are_derived_from_the_routes_that_prove_them() {
    let map = manifest();
    assert_eq!(
        map["meta"]["devmap_rust"]["frameworks_computed"],
        serde_json::json!(true),
        "frameworks is derivable from extracted routes and must claim to be computed"
    );
    let frameworks: Vec<&str> = map["frameworks"]
        .as_array()
        .expect("frameworks is an array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    // `fastapi/flask`, not `flask`. The two frameworks share the
    // `@app.<verb>("/path")` decorator syntax exactly, so the matcher that
    // found these routes cannot tell them apart and says so in the label
    // rather than picking one. That disjunction is the honest answer and is
    // pinned here deliberately: a future change that "tidies" it into a single
    // name would be inventing a distinction the evidence does not support.
    assert!(
        frameworks.contains(&"fastapi/flask"),
        "api/routes.py declares two decorator routes and the manifest names no \
         framework: {frameworks:?}"
    );
}

/// An empty answer from a producer that *did* look is still an answer, and must
/// keep saying so. This is the half that makes the marker mean anything: if
/// `frameworks_computed` were only ever true when the list was non-empty it
/// would be a restatement of the list.
#[test]
fn a_repository_with_no_routes_still_reports_frameworks_as_computed() {
    let extractions = vec![extract_file("core/model.py", "class Row:\n    pass\n")];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = AnalysisSummary {
        total_files: 1,
        total_symbols: 1,
        total_edges: resolution.edges.len(),
        communities: vec![CommunityReport {
            community_id: 0,
            name: "community-0".into(),
            members: vec!["core/model.py".into()],
            cohesion_score: 1.0,
        }],
        status: AnalysisStatus::Ok,
        ..Default::default()
    };
    let (_, json) = generate_manifest_with_edges(
        &extractions,
        &analysis,
        FreshnessInfo::new("head".into(), 1, 0),
        &resolution.edges,
    );
    let map: serde_json::Value = serde_json::from_str(&json).expect("the manifest is JSON");
    assert_eq!(
        map["frameworks"],
        serde_json::json!([]),
        "no routes were found, so the honest answer is an empty list"
    );
    assert_eq!(
        map["meta"]["devmap_rust"]["frameworks_computed"],
        serde_json::json!(true),
        "an empty list that was computed must still say it was computed, or the marker \
         is just the list again"
    );
}

/// `subsystems[].summary` was the literal `""`, and six readers render it as
/// prose — `wiki.py:311` emits `- [area](…) — ` and `map_artifacts.py:69`
/// emits `1. \`area/\` — `, both with a dangling dash. The kernel cannot write
/// prose, but it can state composition, which is what those readers actually
/// need and what it already computes for `role_files` and `languages`.
#[test]
fn subsystem_summaries_describe_composition_instead_of_being_blank() {
    let map = manifest();
    assert_eq!(
        map["meta"]["devmap_rust"]["subsystem_summaries_computed"],
        serde_json::json!(true),
    );
    let subsystems = map["subsystems"]
        .as_array()
        .expect("subsystems is an array");
    assert!(!subsystems.is_empty(), "fixture produced no subsystems");
    for subsystem in subsystems {
        let summary = subsystem["summary"]
            .as_str()
            .unwrap_or_else(|| panic!("summary is not a string: {subsystem}"));
        assert!(
            !summary.is_empty(),
            "summary is still the empty literal, so every consumer that renders it \
             emits a dangling dash: {subsystem}"
        );
        // It has to be about *this* subsystem, not a constant sentence — a
        // constant would be the same defect with more characters.
        assert!(
            summary.contains("file"),
            "the summary says nothing measurable about the subsystem: {summary:?}"
        );
    }
    let summaries: Vec<&str> = subsystems
        .iter()
        .map(|s| s["summary"].as_str().unwrap_or_default())
        .collect();
    assert!(
        summaries
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            > 1
            || summaries.len() == 1,
        "every subsystem got the same sentence, which is a literal with extra steps: {summaries:?}"
    );
}

/// The six that are marked `false` must be marked `false` — not quietly
/// flipped to `true` by someone filling the marker without filling the field.
#[test]
fn the_fields_this_kernel_cannot_see_admit_it() {
    let map = manifest();
    let meta = &map["meta"]["devmap_rust"];
    for marker in [
        "package_managers_computed",
        "test_commands_computed",
        "candidate_files_computed",
        "lsp_computed",
        "dependency_risks_computed",
        "processes_computed",
    ] {
        assert_eq!(
            meta[marker],
            serde_json::json!(false),
            "{marker} claims the kernel computed a field it emits as a constant: {meta}"
        );
    }
}

/// Determinism (R4): the same generation renders the same bytes.
#[test]
fn the_new_fields_are_deterministic() {
    assert_eq!(
        manifest()["subsystems"],
        manifest()["subsystems"],
        "two renderings of one generation disagree about subsystems"
    );
    assert_eq!(manifest()["frameworks"], manifest()["frameworks"]);
}
