//! `explore` and `affected` — the two query surfaces that used to be answered
//! by a second engine in Python.
//!
//! The Python `CodeIntelQueryEngine` loaded a whole `CodeGraph` into process
//! memory and walked it there, and its answers overclaimed in four specific
//! ways. Every test here pins the repaired behaviour, and each names the claim
//! it would have let through:
//!
//! * a definition list cut *before* it was ranked (R7),
//! * a source snippet that could not be read reported as an empty one (Class A),
//! * an affected-test list derived from a list the token budget had already
//!   trimmed, with no counter saying so,
//! * a target that matched nothing reported as a target with no impact.

use devmap_extract::extract_file;
use devmap_query::{ResolutionAvailability, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::{Path, PathBuf};

/// A scratch directory named for the test using it.
///
/// No `tempfile` dev-dependency exists in this workspace and this does not add
/// one; the convention here is a process-scoped path under the system temp dir,
/// with the test name in it because these run in parallel in one process.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-explore-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`); the recorded
    // repo root is always canonical, so the fixture's must be too.
    dir.canonicalize().unwrap()
}

const CORE: &str =
    "def render(rows):\n    return sum(rows)\n\n\ndef render_helper(rows):\n    return rows\n";
const SERVICE: &str = "from core import render\n\n\ndef serve(rows):\n    return render(rows)\n";
const TEST_DIRECT: &str =
    "from core import render\n\n\ndef test_render(rows):\n    return render(rows)\n";
const TEST_INDIRECT: &str =
    "from service import serve\n\n\ndef test_serve(rows):\n    return serve(rows)\n";

/// Four files: a helper, a direct caller, a test that calls the helper, and a
/// test that reaches it only through the caller.
///
/// The two-hop test is what makes the derivation tests meaningful — a
/// depth-limited or budget-trimmed walk loses it and keeps the one-hop test, so
/// "the answer got shorter" is observable rather than theoretical.
fn fixture(root: &Path) -> Store {
    let files = [
        ("core.py", CORE),
        ("service.py", SERVICE),
        ("tests/test_direct.py", TEST_DIRECT),
        ("tests/test_indirect.py", TEST_INDIRECT),
    ];
    std::fs::create_dir_all(root.join("tests")).unwrap();
    let mut extractions = Vec::new();
    for (path, body) in files {
        std::fs::write(root.join(path), body).unwrap();
        extractions.push(extract_file(path, body));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
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

#[test]
fn zero_depth_affected_tests_only_reports_seed_tests() {
    let root = scratch("zero-depth");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);
    let answer = engine
        .affected_tests(&["render".into()], 8_000, 0.0, 0)
        .unwrap();
    assert!(answer.tests.items.is_empty(), "{:?}", answer.tests.items);
    assert!(answer.blast_radius.layers.items.is_empty());
    assert!(answer
        .tests
        .walk_incomplete
        .as_deref()
        .unwrap_or_default()
        .contains("depth 0"));
    let seed = engine
        .affected_tests(&["test_render".into()], 8_000, 0.0, 0)
        .unwrap();
    assert_eq!(seed.tests.items.len(), 1);
    assert_eq!(seed.tests.items[0].depth, 0);
}

/// The best match survives a cut of one, whatever its path sorts like.
///
/// Python built its match list as `exact_matches + partial_matches` and then
/// sliced `[:limit]`, so within each half the order was whatever the store
/// yielded. `render_helper` contains `render`; `render` *is* `render`. A
/// one-definition answer that returns the helper is not a smaller answer, it is
/// the wrong one.
#[test]
fn one_definition_is_the_highest_ranked_one_not_the_first_row() {
    let root = scratch("rank");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let report = engine.explore("render", 1, 8_000, 0.0, 3).unwrap();

    assert_eq!(
        report.definitions.shown, 1,
        "asked for one definition, got {}",
        report.definitions.shown
    );
    assert_eq!(
        report.definitions.items[0].symbol_name, "render",
        "the exact match must outrank the substring match"
    );
    assert!(
        report.definitions.items[0].score >= 1.0,
        "an exact name match scores 1.0, got {}",
        report.definitions.items[0].score
    );
}

/// A capped list reports the count it was capped from.
///
/// R7: `shown + hidden == total`, `truncated == (hidden > 0)`, and `total` is
/// what the index holds — not the size of the page the budget could show.
/// `DevMapClient._budgeted` enforces exactly these three on the wire, so a
/// response that breaks them is not merely misleading, it is unreadable.
#[test]
fn a_capped_definition_list_carries_the_total_it_was_capped_from() {
    let root = scratch("counts");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let full = engine.explore("render", 20, 8_000, 0.0, 3).unwrap();
    assert!(
        full.definitions.total >= 2,
        "fixture must offer more than one match, got {}",
        full.definitions.total
    );

    let capped = engine.explore("render", 1, 8_000, 0.0, 3).unwrap();
    assert_eq!(
        capped.definitions.total, full.definitions.total,
        "the total must describe the index, not the page"
    );
    assert_eq!(
        capped.definitions.shown + capped.definitions.hidden,
        capped.definitions.total,
        "shown + hidden must equal total"
    );
    assert!(
        capped.definitions.truncated,
        "a list with hidden items must say it is truncated"
    );
    assert!(
        capped.definitions.tokens_used <= capped.budget.definitions,
        "the packer spent {} against a {} definition allowance",
        capped.definitions.tokens_used,
        capped.budget.definitions
    );
}

/// Every part of one `explore` answer fits inside the one budget it was given.
///
/// The report is four sections funded from a single number, so "the definition
/// list fit" is not the same claim as "the answer fit". The split is published
/// in `budget` precisely so this can be checked rather than trusted.
#[test]
fn the_whole_report_stays_inside_the_budget_it_was_given() {
    let root = scratch("budget");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let budget = 4_000u32;
    let report = engine.explore("render", 20, budget, 0.0, 3).unwrap();

    let edge_spend: u32 = report
        .definitions
        .items
        .iter()
        .map(|definition| definition.callers.tokens_used + definition.callees.tokens_used)
        .sum();
    let total_spend =
        report.definitions.tokens_used + edge_spend + report.blast_radius.layers.tokens_used;
    assert!(
        total_spend <= budget,
        "the composed answer spent {total_spend} against a {budget} budget \
         (definitions {}, edges {edge_spend}, blast {})",
        report.definitions.tokens_used,
        report.blast_radius.layers.tokens_used
    );
    for definition in &report.definitions.items {
        assert!(
            definition.callers.tokens_used <= report.budget.edges_per_direction,
            "one caller list spent {} against a {} per-direction allowance",
            definition.callers.tokens_used,
            report.budget.edges_per_direction
        );
    }
}

/// A snippet that could not be read is not an empty snippet.
///
/// Class A. Python's `_snippet` returned `""` for a file it could not open and
/// `""` for a symbol whose span was empty, so "we did not look" and "there is
/// nothing there" arrived identically. Here the reason travels with the
/// definition and the span collapses to `(0, 0)` rather than claiming lines
/// nobody read.
#[test]
fn an_unreadable_source_file_reports_why_instead_of_an_empty_snippet() {
    let root = scratch("classa");
    let store = fixture(&root);
    // Indexed, then gone: exactly what a query against a generation built
    // before a delete sees.
    std::fs::remove_file(root.join("core.py")).unwrap();
    let engine = StoreQueryEngine::new(&store);

    let report = engine.explore("render", 5, 8_000, 0.0, 3).unwrap();
    let definition = report
        .definitions
        .items
        .iter()
        .find(|definition| definition.file_path == "core.py")
        .expect("the deleted file is still indexed and must still be returned");

    assert!(
        definition.source.is_empty(),
        "nothing could be read, so there is no source to show"
    );
    let reason = definition
        .source_unavailable_reason
        .as_deref()
        .expect("an unread file must say it was unread, not present an empty body");
    assert!(
        reason.contains("core.py"),
        "the reason must name the file it is about, got {reason:?}"
    );
    assert_eq!(
        definition.span,
        (0, 0),
        "a line range nobody could compute must not be asserted"
    );
}

/// The affected-test list comes from the walk, not from the trimmed layers.
///
/// The blast radius is sampled per band and then packed into a token budget,
/// and both trims are reported *on the radius*. Deriving the test list from the
/// trimmed layers would drop tests whose band the budget cut, and the test
/// list's own counters would then describe a complete answer over an already
/// incomplete set — a capped sample presented as coverage.
///
/// The budget here is deliberately tiny, so the radius carries almost nothing;
/// the two-hop test must still be found.
#[test]
fn affected_tests_survive_a_budget_that_trims_the_blast_radius() {
    let root = scratch("derivation");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let generous = engine
        .affected_tests(&["render".to_string()], 8_000, 0.0, 3)
        .unwrap();
    let starved = engine
        .affected_tests(&["render".to_string()], 60, 0.0, 3)
        .unwrap();

    let generous_paths: Vec<&str> = generous
        .tests
        .items
        .iter()
        .map(|test| test.path.as_str())
        .collect();
    assert!(
        generous_paths.contains(&"tests/test_indirect.py"),
        "the two-hop test must be reachable at all: {generous_paths:?}"
    );
    assert!(
        starved.blast_radius.layers.truncated || starved.blast_radius.layers.shown == 0,
        "the tiny budget must actually trim the radius, or this test proves nothing"
    );
    assert_eq!(
        starved.tests.total, generous.tests.total,
        "the test count must come from the walk, so a budget that trims the \
         radius cannot shrink it"
    );
}

/// Nearest first, so a trimmed list keeps the tests most likely to break.
#[test]
fn affected_tests_are_ranked_by_distance_before_they_are_truncated() {
    let root = scratch("ranking");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let report = engine
        .affected_tests(&["render".to_string()], 8_000, 0.0, 3)
        .unwrap();
    let depths: Vec<usize> = report.tests.items.iter().map(|test| test.depth).collect();
    let mut sorted = depths.clone();
    sorted.sort_unstable();
    assert_eq!(depths, sorted, "tests must be ordered nearest-first");
    let direct = report
        .tests
        .items
        .iter()
        .find(|test| test.path == "tests/test_direct.py")
        .expect("the direct caller's test must be present");
    let indirect = report
        .tests
        .items
        .iter()
        .find(|test| test.path == "tests/test_indirect.py")
        .expect("the two-hop test must be present");
    assert!(
        direct.depth < indirect.depth,
        "a direct caller's test must rank above a two-hop one: {} vs {}",
        direct.depth,
        indirect.depth
    );
}

/// A target that matched nothing is named, not silently dropped.
///
/// Python resolved targets to node ids and moved on; a typo produced an empty
/// seed set and an empty answer that read as "nothing is affected". That is the
/// flattering reading of "we did not look", and it is the reading a caller acts
/// on when deciding which tests to run.
#[test]
fn a_target_that_matches_nothing_is_reported_rather_than_answered_as_zero() {
    let root = scratch("unmatched");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let report = engine
        .affected_tests(&["no_such_symbol_anywhere".to_string()], 2_000, 0.0, 3)
        .unwrap();

    assert_eq!(
        report.blast_radius.unmatched_targets,
        vec!["no_such_symbol_anywhere".to_string()],
        "an unmatched target must be named"
    );
    assert!(
        matches!(
            report.blast_radius.layers.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "a radius with no seed must be unavailable, not an available zero"
    );

    // A mix must not let the matched half vouch for the unmatched half.
    let mixed = engine
        .affected_tests(
            &["render".to_string(), "no_such_symbol_anywhere".to_string()],
            8_000,
            0.0,
            3,
        )
        .unwrap();
    assert_eq!(
        mixed.blast_radius.unmatched_targets,
        vec!["no_such_symbol_anywhere".to_string()],
        "the unmatched target must still be named when another target matched"
    );
    assert!(
        !mixed.blast_radius.seeds.is_empty(),
        "the matched target must still seed the walk"
    );
}

/// A radius stopped by its depth bound says so.
///
/// `truncated`/`hidden` describe the budget and cannot express a walk that
/// withheld an unknown quantity; `walk_incomplete` is the field that can, and
/// without it a depth-1 radius reads as the whole blast radius.
#[test]
fn a_depth_capped_radius_reports_that_it_is_a_lower_bound() {
    let root = scratch("capped");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let shallow = engine
        .affected_tests(&["render".to_string()], 8_000, 0.0, 1)
        .unwrap();
    assert!(
        shallow.blast_radius.layers.walk_incomplete.is_some(),
        "a walk stopped at depth 1 with more graph beyond it must say so"
    );
    assert!(
        shallow.tests.walk_incomplete.is_some(),
        "a test list derived from a capped walk is a lower bound and must say so"
    );

    let deep = engine
        .affected_tests(&["render".to_string()], 8_000, 0.0, 8)
        .unwrap();
    assert!(
        deep.blast_radius.layers.walk_incomplete.is_none(),
        "a walk that ran out of graph must not claim to have been cut short: {:?}",
        deep.blast_radius.layers.walk_incomplete
    );
}

/// An empty store answers "unavailable", never "no matches".
#[test]
fn an_unbuilt_store_is_unavailable_rather_than_empty() {
    let store = Store::open_in_memory().unwrap();
    let engine = StoreQueryEngine::new(&store);

    let explored = engine.explore("render", 20, 8_000, 0.0, 3).unwrap();
    assert!(
        matches!(
            explored.definitions.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "an unbuilt store must not report zero matches as a measured answer"
    );

    let affected = engine
        .affected_tests(&["render".to_string()], 2_000, 0.0, 3)
        .unwrap();
    assert!(
        matches!(
            affected.tests.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "an unbuilt store must not report zero affected tests as a measured answer"
    );
}

/// More targets than the fan-out bound is refused, not trimmed.
///
/// The same rule `neighbors` applies: answering the first sixteen of twenty
/// hands back a short list that reads exactly like a complete one.
#[test]
fn too_many_affected_targets_are_refused() {
    let root = scratch("fanout");
    let store = fixture(&root);
    let engine = StoreQueryEngine::new(&store);

    let targets: Vec<String> = (0..devmap_query::MAX_NEIGHBOR_TARGETS + 1)
        .map(|index| format!("target_{index}"))
        .collect();
    let error = engine
        .affected_tests(&targets, 2_000, 0.0, 3)
        .expect_err("an over-long target list must be refused");
    assert!(
        error.to_string().contains("at most"),
        "the refusal must say what the bound is, got {error}"
    );
}

/// Directory and filename shapes that are tests, and near-misses that are not.
///
/// Python asked `"/test" in "/" + path`, which counted `src/testing_utils.py`,
/// `lib/latest/mod.rs` and `app/protester/main.go` as tests. Recorded as a
/// deliberate divergence: the affected-test list gets shorter, and everything
/// that leaves it was never a test.
#[test]
fn test_path_detection_matches_directories_and_affixes_not_substrings() {
    for path in [
        "tests/test_thing.py",
        "src/__tests__/widget.tsx",
        "pkg/foo_test.go",
        "web/widget.spec.ts",
        "web/widget.test.tsx",
        "spec/models/user_spec.rb",
        "src/FooTest.java",
        "e2e/checkout.ts",
    ] {
        assert!(devmap_query::is_test_path(path), "{path} is a test path");
    }
    for path in [
        "src/testing_utils.py",
        "lib/latest/mod.rs",
        "app/protester/main.go",
        "src/contest.py",
        "src/attestation.rs",
    ] {
        assert!(
            !devmap_query::is_test_path(path),
            "{path} merely contains the substring and is not a test path"
        );
    }
}
