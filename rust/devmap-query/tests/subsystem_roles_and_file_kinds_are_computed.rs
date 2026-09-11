//! `subsystems[].role_files` and `files[].kind` must be computed, not constants.
//!
//! The third and fourth instances of the shape `neighbors` and `handoff_paths`
//! already carried in this writer:
//!
//! - `role_files` was `{}` for every subsystem, for the field's whole life in
//!   this kernel, while `indexing/map_artifacts.py` writes step 5 of the
//!   *generated agent guide* as "Use `role_files` in `subsystems` for subsystem
//!   role buckets (entry, runtime, policy, adapters, etc.)". Four readers took
//!   that instruction and got nothing back, one of them functionally:
//!   `verification/test_resolver.py` resolves a subsystem's tests through
//!   `role_files["tests"]`, so a change with no direct test importer had no
//!   fallback at all.
//! - `files[].kind` was the literal `"code"` for every entry. Measured on this
//!   repository's `.devcouncil/repo_map.json` before the fix: 1,417 of 1,417
//!   files `kind=code`, including 108 markdown files and 172 JSON files, and
//!   `README.md` — which the map itself already labelled `language: markdown`.
//!   The kind was not a classification, it was a constant wearing one's name.
//!
//! The role vocabulary and rule order are the retired Python writer's
//! (`_GENERIC_ROLE_RULES`, removed in d232dea), deliberately: consumers were
//! written against those names, and inventing a second vocabulary here would
//! make the field's history unreadable. Ordered most-specific first, first
//! match wins, so a test file is a test before it is an `api` file.

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
        // Fields this fixture does not exercise. Spread rather than
        // enumerated so a new analysis field does not break every test
        // literal in the workspace; the one production construction in
        // `analyze()` still names every field exhaustively.
        ..Default::default()
    };
    let (_, json) = generate_manifest_with_edges(
        &extractions,
        &analysis,
        FreshnessInfo::new("head".into(), 1, 0),
        &resolution.edges,
        // No tree on disk behind these synthesised extractions.
        None,
    );
    serde_json::from_str(&json).expect("the manifest is JSON")
}

/// A shape with one file of each kind the classifier distinguishes.
fn mixed_manifest() -> serde_json::Value {
    manifest_for(vec![
        extract_file("README.md", "# sample\n\nprose, not code.\n"),
        extract_file("docs/design.md", "# design\n"),
        extract_file("svc/api/routes.py", "def get(request):\n    return 1\n"),
        extract_file("svc/models/user.py", "class User:\n    pass\n"),
        extract_file(
            "svc/tests/test_routes.py",
            "def test_get():\n    assert True\n",
        ),
        extract_file("svc/config/settings.yaml", "debug: true\n"),
        extract_file("svc/main.py", "def main():\n    return 0\n"),
    ])
}

fn kind_of(map: &serde_json::Value, path: &str) -> String {
    map["files"]
        .as_array()
        .expect("files is an array")
        .iter()
        .find(|entry| entry["path"] == path)
        .unwrap_or_else(|| panic!("no files entry for {path}"))["kind"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn roles_of(map: &serde_json::Value, area: &str) -> serde_json::Value {
    map["subsystems"]
        .as_array()
        .expect("subsystems is an array")
        .iter()
        .find(|entry| entry["area"] == area)
        .unwrap_or_else(|| panic!("no subsystem for {area}: {}", map["subsystems"]))["role_files"]
        .clone()
}

/// The assertion the strict-`xfail` Python test has been carrying:
/// `README.md` is a document, and the map already knew it was markdown.
#[test]
fn a_markdown_file_is_classified_as_a_document() {
    let map = mixed_manifest();
    assert_eq!(
        kind_of(&map, "README.md"),
        "doc",
        "README.md is prose; the map labels it language=markdown and must not call it code"
    );
    assert_eq!(
        kind_of(&map, "docs/design.md"),
        "doc",
        "a file under docs/ is a document twice over"
    );
}

#[test]
fn source_config_and_test_files_get_their_own_kinds() {
    let map = mixed_manifest();
    assert_eq!(kind_of(&map, "svc/main.py"), "module");
    assert_eq!(kind_of(&map, "svc/config/settings.yaml"), "config");
    assert_eq!(
        kind_of(&map, "svc/tests/test_routes.py"),
        "test",
        "a test is a test before it is a module"
    );
}

/// `kind` must actually partition the corpus. A classifier that answers one
/// value for everything is the defect this test exists to prevent, and
/// asserting individual paths above would not catch a regression back to it.
#[test]
fn the_kind_field_is_not_one_constant_wearing_a_classification_s_name() {
    let map = mixed_manifest();
    let kinds: std::collections::BTreeSet<String> = map["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        kinds.len() >= 4,
        "this fixture holds documents, config, tests and modules; \
         the classifier produced only {kinds:?}"
    );
}

/// Every subsystem the map emits has files under it by construction, so every
/// one of them has an answer — even if that answer is only "other".
#[test]
fn every_subsystem_gets_a_non_empty_role_bucket() {
    let map = mixed_manifest();
    let subsystems = map["subsystems"].as_array().expect("subsystems");
    assert!(
        !subsystems.is_empty(),
        "no subsystems, so this proves nothing"
    );
    for entry in subsystems {
        let roles = entry["role_files"]
            .as_object()
            .expect("role_files is an object");
        assert!(
            !roles.is_empty(),
            "{} has files under it but no role bucket; `{{}}` is the ambiguity \
             this field is being fixed to remove",
            entry["area"]
        );
    }
}

#[test]
fn a_role_bucket_names_the_files_that_play_that_role() {
    let map = mixed_manifest();
    let roles = roles_of(&map, "svc/tests");
    let tests = roles["tests"]
        .as_array()
        .unwrap_or_else(|| panic!("svc/tests has no `tests` bucket: {roles}"));
    assert!(
        tests
            .iter()
            .any(|value| value == "svc/tests/test_routes.py"),
        "the tests bucket must name the test file: {roles}"
    );
}

/// First match wins, and the order is most-specific first. `test_routes.py`
/// sits under a path the `api` rule also matches (`/routes`), and it must be a
/// test — otherwise the buckets stop partitioning the subsystem and
/// `test_resolver.py` reads a route handler as a test to run.
#[test]
fn a_test_under_an_api_path_is_a_test_not_an_api_file() {
    let map = manifest_for(vec![
        extract_file("pkg/routes/handler.py", "def handle():\n    return 1\n"),
        extract_file(
            "pkg/routes/test_handler.py",
            "def test_handle():\n    assert True\n",
        ),
    ]);
    let roles = roles_of(&map, "pkg/routes");
    let api: Vec<&str> = roles["api"]
        .as_array()
        .map(|values| values.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        !api.contains(&"pkg/routes/test_handler.py"),
        "the test was claimed by the api bucket, so the rules no longer partition: {roles}"
    );
    let tests: Vec<&str> = roles["tests"]
        .as_array()
        .map(|values| values.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert!(
        tests.contains(&"pkg/routes/test_handler.py"),
        "and it must land in tests instead: {roles}"
    );
}

/// The buckets are a capped sample for orientation, never an inventory, so the
/// real per-role total has to travel with them. Without it `role_files["tests"]`
/// naming four files reads as "this subsystem has four tests".
#[test]
fn the_real_per_role_total_travels_with_the_capped_sample() {
    let paths: Vec<String> = (0..9).map(|i| format!("pkg/tests/test_{i}.py")).collect();
    let many: Vec<_> = paths
        .iter()
        .map(|path| extract_file(path, "def test_x():\n    assert True\n"))
        .collect();
    let map = manifest_for(many);
    let entry = map["subsystems"]
        .as_array()
        .expect("subsystems")
        .iter()
        .find(|entry| entry["area"] == "pkg/tests")
        .unwrap_or_else(|| panic!("no pkg/tests subsystem: {}", map["subsystems"]));
    let shown = entry["role_files"]["tests"]
        .as_array()
        .expect("tests bucket")
        .len();
    assert_eq!(shown, 4, "the per-role cap is four examples");
    assert_eq!(
        entry["role_file_counts"]["tests"],
        serde_json::json!(9),
        "and the count must be the truth, not the size of the sample: {}",
        entry["role_file_counts"]
    );
}

/// The Class A half, for both fields.
#[test]
fn the_manifest_states_that_roles_and_kinds_were_computed() {
    let map = mixed_manifest();
    assert_eq!(
        map["meta"]["devmap_rust"]["role_files_computed"],
        serde_json::json!(true),
        "the manifest must say role_files was derived, not left at `{{}}`: {}",
        map["meta"]
    );
    assert_eq!(
        map["meta"]["devmap_rust"]["file_kinds_computed"],
        serde_json::json!(true),
        "and that `kind` is a classification rather than a constant: {}",
        map["meta"]
    );
    let meta = &map["liveness_meta"]["subsystems"];
    assert!(
        meta["role_files_total"].as_u64().unwrap_or(0) > 0,
        "some file was bucketed, so the total cannot be zero: {meta}"
    );
    assert_eq!(
        meta["role_files_truncated"],
        serde_json::json!(false),
        "nothing was cut in this fixture: {meta}"
    );
}

/// Determinism (R4): the same generation must render the same bytes.
#[test]
fn the_role_buckets_are_deterministic() {
    let first = mixed_manifest();
    let second = mixed_manifest();
    assert_eq!(
        first["subsystems"], second["subsystems"],
        "two renderings of one generation disagree"
    );
    assert_eq!(
        first["files"], second["files"],
        "and the file kinds disagree"
    );
}
