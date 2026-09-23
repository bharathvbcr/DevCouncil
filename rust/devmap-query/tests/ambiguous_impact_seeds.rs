//! Bare-name impact seeds must be unique.
//!
//! When two files define the same symbol, `impact`, `affected`, and `blast_walk`
//! seeding that bare name used to union both blast radii into one answer. That
//! is a wrong answer: the caller asked about one name, not about every
//! namesake. Search and explore may still return the set; a walk that starts
//! from every match must not. File targets and `file::symbol` stay free to
//! union — those are deliberate scopes.

use devmap_extract::extract_file;
use devmap_query::{Request, ResolutionAvailability, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-ambiguous-impact-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// Two same-named definitions with disjoint callers.
///
/// A union walk from the bare name `authenticate` reaches both `use_a` and
/// `use_b`. Fail-closed behaviour must refuse before that walk starts.
fn namesake_fixture(root: &Path) -> Store {
    let files = [
        ("auth_a.py", "def authenticate():\n    return \"a\"\n"),
        ("auth_b.py", "def authenticate():\n    return \"b\"\n"),
        (
            "caller_a.py",
            "from auth_a import authenticate\n\n\ndef use_a():\n    return authenticate()\n",
        ),
        (
            "caller_b.py",
            "from auth_b import authenticate\n\n\ndef use_b():\n    return authenticate()\n",
        ),
    ];
    let mut extractions = Vec::new();
    for (path, body) in files {
        std::fs::write(root.join(path), body).unwrap();
        extractions.push(extract_file(path, body));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
    store
}

fn impact_req(target: &str) -> Request<String> {
    Request {
        query: target.to_string(),
        token_budget: 8_000,
        min_confidence: 0.0,
        max_depth: 3,
    }
}

/// The defect: a bare name with two definitions currently walks both.
///
/// After the fix this must refuse with both candidate ids and must not report
/// either caller's blast radius as the answer.
#[test]
fn bare_name_with_two_definitions_refuses_impact_without_walking() {
    let root = scratch("refuse");
    let store = namesake_fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    // Characterize the pre-fix hole if the walk still runs: both callers
    // appear. The assertion that matters is the refuse path below.
    let layered = engine.impact_layered(impact_req("authenticate"));
    match layered {
        Ok(answer) => {
            let reached: Vec<&str> = answer
                .blast_radius
                .layers
                .items
                .iter()
                .flat_map(|layer| layer.nodes.iter().map(String::as_str))
                .collect();
            panic!(
                "bare-name impact must refuse when two definitions match; \
                 walked instead (total_impacted={}, reached={reached:?})",
                answer.blast_radius.total_impacted
            );
        }
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.to_lowercase().contains("ambiguous"),
                "refusal must name the ambiguity: {message}"
            );
            assert!(
                message.contains("auth_a.py::authenticate")
                    && message.contains("auth_b.py::authenticate"),
                "refusal must list both candidate ids: {message}"
            );
            assert!(
                !message.contains("use_a") && !message.contains("use_b"),
                "refusal must not walk into callers: {message}"
            );
        }
    }
}

#[test]
fn bare_name_with_two_definitions_refuses_affected_without_walking() {
    let root = scratch("affected");
    let store = namesake_fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let outcome = engine.affected_tests(&["authenticate".into()], 8_000, 0.0, 3);
    match outcome {
        Ok(report) => panic!(
            "affected must refuse an ambiguous bare name; got tests={:?} \
             total_impacted={}",
            report.tests.items, report.blast_radius.total_impacted
        ),
        Err(err) => {
            let message = err.to_string();
            assert!(
                message.to_lowercase().contains("ambiguous"),
                "refusal must name the ambiguity: {message}"
            );
            assert!(
                message.contains("auth_a.py::authenticate")
                    && message.contains("auth_b.py::authenticate"),
                "refusal must list both candidate ids: {message}"
            );
        }
    }
}

#[test]
fn qualified_file_symbol_still_walks_one_namesake() {
    let root = scratch("qualified");
    let store = namesake_fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .impact_layered(impact_req("auth_a.py::authenticate"))
        .expect("a qualified target is unambiguous");
    assert!(
        matches!(
            answer.blast_radius.layers.resolution,
            ResolutionAvailability::Available
        ),
        "{:?}",
        answer.blast_radius.layers.resolution
    );
    let reached: Vec<&str> = answer
        .blast_radius
        .layers
        .items
        .iter()
        .flat_map(|layer| layer.nodes.iter().map(String::as_str))
        .collect();
    assert!(
        reached.iter().any(|n| n.contains("use_a")),
        "auth_a.py::authenticate must still reach use_a: {reached:?}"
    );
    assert!(
        !reached.iter().any(|n| n.contains("use_b")),
        "must not union the other namesake: {reached:?}"
    );
}

#[test]
fn file_target_still_unions_members() {
    let root = scratch("file");
    let store = namesake_fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let answer = engine
        .impact_layered(impact_req("auth_a.py"))
        .expect("a file target may expand to its members");
    assert!(
        matches!(
            answer.blast_radius.layers.resolution,
            ResolutionAvailability::Available
        ),
        "{:?}",
        answer.blast_radius.layers.resolution
    );
    assert!(
        answer.blast_radius.total_impacted > 0
            || !answer.edges.items.is_empty()
            || !answer.blast_radius.layers.items.is_empty(),
        "file seed must still walk: {:?}",
        answer.blast_radius
    );
}

#[test]
fn search_and_explore_still_return_the_set() {
    let root = scratch("search");
    let store = namesake_fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let search = engine.search(impact_req("authenticate")).unwrap();
    assert!(
        search.total >= 2,
        "search must still report both namesakes: total={}",
        search.total
    );

    let explore = engine.explore("authenticate", 20, 8_000, 0.0, 3).unwrap();
    assert!(
        explore.definitions.total >= 2,
        "explore must still report both definitions: total={}",
        explore.definitions.total
    );
    assert!(
        matches!(
            explore.definitions.resolution,
            ResolutionAvailability::Available
        ),
        "explore must not refuse the set: {:?}",
        explore.definitions.resolution
    );
}
