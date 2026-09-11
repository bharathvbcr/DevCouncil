//! Two surfaces that answered confidently from a check that had not run.
//!
//! Both were found by audit and both are the repository's Class A rule: a check
//! that could not run must never report what a check that ran and passed
//! reports. The failures are worth restating because neither looks like a bug in
//! its output — each returns a well-formed, plausible, *wrong* answer.
//!
//! * `preview` compared a buffer against a file it could not read and reported
//!   the result as though there had been no file at all, so a genuine symbol
//!   **removal disappeared** and the report came back clean.
//! * `QueryEngine::impact`/`trace` computed "the walk stopped at the depth cap;
//!   this is a lower bound, not the full blast radius" and then dropped it,
//!   publishing `walk_incomplete: None`. For `impact` that is the reading that
//!   gets a live symbol deleted: an incomplete blast radius and a small one look
//!   identical.
//! * `StoreQueryEngine::dead_symbols` published a delete-this list with no
//!   denominator at all — neither `AnalysisSummary::status` nor
//!   `unresolved_calls`, the field whose own documentation says it exists so a
//!   reader can tell "nothing calls this" from "we could not work out what
//!   this calls". A generation with 4,242 unattributed calls answered in the
//!   exact shape of one with none.

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, Extraction, ExtractionEngine, ParseOutcome, SymbolKind};
use devmap_query::{QueryEngine, Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

#[test]
fn dependencies_disclose_unbound_calls_but_not_known_builtins() {
    for (source, has_gap) in [
        ("def entry(callback):\n    callback()\n", true),
        ("def entry():\n    print(1)\n", false),
    ] {
        let files = [("app.py", source)];
        let store = store_of(&files, &[]);
        let extractions = vec![extract_file("app.py", source)];
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let req = || Request {
            query: "app.py".into(),
            token_budget: 2000,
            min_confidence: 0.0,
            max_depth: 3,
        };
        assert_eq!(
            StoreQueryEngine::new(&store)
                .dependencies(req())
                .unwrap()
                .walk_incomplete
                .is_some(),
            has_gap,
            "stored: {source}"
        );
        assert_eq!(
            QueryEngine::new(&extractions, &resolution)
                .dependencies(req())
                .walk_incomplete
                .is_some(),
            has_gap,
            "memory: {source}"
        );
    }
}

#[test]
fn semantic_search_and_explore_keep_parse_loss_on_hits_and_misses() {
    let store = store_of(
        &[
            ("read.py", "def present():\n    return 1\n"),
            ("lost.py", "def missing():\n    return 2\n"),
        ],
        &["lost.py"],
    );
    let engine = StoreQueryEngine::new(&store);
    for query in ["present", "missing"] {
        let semantic = engine.search_semantic(query, 20_000).unwrap();
        assert!(
            semantic.walk_incomplete.is_some(),
            "semantic {query}: {semantic:?}"
        );
        let explore = engine.explore(query, 5, 20_000, 0.0, 64).unwrap();
        assert!(
            explore.definitions.walk_incomplete.is_some(),
            "explore {query}: {explore:?}"
        );
    }
}

#[test]
fn scoped_traces_keep_attribution_loss_on_found_and_missing_paths() {
    let store = store_of(&[("app.py", "def destination():\n    return 1\ndef source(callback):\n    callback()\n    return destination()\n")], &[]);
    let engine = StoreQueryEngine::new(&store);
    for target in ["destination", "absent"] {
        let answer = engine
            .trace_between(Request {
                query: ("source".into(), target.into()),
                token_budget: 2000,
                min_confidence: 0.0,
                max_depth: 64,
            })
            .unwrap();
        assert!(answer.walk_incomplete.is_some(), "{target}: {answer:?}");
    }
}

#[test]
fn explore_preserves_qualified_definition_identity() {
    let store = store_of(&[("app.py", "class A:\n    def ping(self):\n        return 1\nclass B:\n    def ping(self):\n        return 2\ndef first():\n    return A().ping()\ndef second():\n    return B().ping()\n")], &[]);
    let engine = StoreQueryEngine::new(&store);
    let report = engine.explore("ping", 10, 20_000, 0.0, 64).unwrap();
    assert_eq!(report.definitions.shown, 2, "{report:?}");
    let ids: std::collections::BTreeSet<_> = report
        .definitions
        .items
        .iter()
        .map(|d| d.id.as_str())
        .collect();
    assert_eq!(
        ids.len(),
        2,
        "different methods must have different traversal identities: {report:?}"
    );
    for definition in &report.definitions.items {
        assert_eq!(definition.id, definition.qualified_name);
        assert!(
            definition
                .callers
                .items
                .iter()
                .all(|edge| edge.target_symbol == definition.id),
            "{definition:?}"
        );
        assert!(
            !definition.callers.items.is_empty(),
            "fixture needs real callers: {definition:?}"
        );
    }
}

#[test]
fn explore_radius_discloses_definitions_omitted_by_limit_or_budget() {
    let store = store_of(&[("app.py", "def helper_a():\n    return 1\ndef helper_b():\n    return 2\ndef entry():\n    helper_a()\n    helper_b()\n")], &[]);
    let engine = StoreQueryEngine::new(&store);
    for (limit, budget) in [(1, 20_000), (10, 0)] {
        let report = engine.explore("helper", limit, budget, 0.0, 64).unwrap();
        assert!(report.definitions.hidden > 0, "{report:?}");
        assert!(
            report
                .blast_radius
                .layers
                .walk_incomplete
                .as_deref()
                .is_some_and(|r| r.contains("definition")),
            "omitted seeds cannot imply a complete radius: {report:?}"
        );
    }
}

#[test]
fn absent_indexes_do_not_publish_available_empty_radii() {
    let store = Store::open_in_memory().unwrap();
    let engine = StoreQueryEngine::new(&store);
    let explore = engine.explore("helper", 5, 2000, 0.0, 3).unwrap();
    let affected = engine
        .affected_tests(&["helper".into()], 2000, 0.0, 3)
        .unwrap();
    for radius in [explore.blast_radius, affected.blast_radius] {
        assert!(
            matches!(
                radius.layers.resolution,
                devmap_query::ResolutionAvailability::Unavailable { .. }
            ),
            "{radius:?}"
        );
    }
}

#[test]
fn memory_queries_refuse_nan_like_the_store_queries() {
    let extractions = vec![extract_file(
        "app.py",
        "def helper():\n    return 1\ndef entry():\n    return helper()\n",
    )];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let engine = QueryEngine::new(&extractions, &resolution);
    let req = |query: &str| Request {
        query: query.into(),
        token_budget: 2000,
        min_confidence: f32::NAN,
        max_depth: 3,
    };
    for answer in [
        engine.dependencies(req("app.py")),
        engine.impact(req("helper")),
        engine.trace(req("entry")),
    ] {
        assert!(
            matches!(answer.resolution, devmap_query::ResolutionAvailability::Unavailable { ref reason } if reason.contains("NaN")),
            "{answer:?}"
        );
    }
}

#[test]
fn memory_traversal_enforces_the_same_depth_ceiling_as_storage() {
    let mut source = "def node_80():\n    return 1\n".to_string();
    for i in 0..80 {
        source.push_str(&format!("def node_{i}():\n    return node_{}()\n", i + 1));
    }
    let extractions = vec![extract_file("app.py", &source)];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let memory = QueryEngine::new(&extractions, &resolution);
    let store = store_of(&[("app.py", &source)], &[]);
    let req = || Request {
        query: "node_0".into(),
        token_budget: 20_000,
        min_confidence: 0.0,
        max_depth: usize::MAX,
    };
    let stored = StoreQueryEngine::new(&store).trace(req()).unwrap();
    assert!(
        stored
            .walk_incomplete
            .as_deref()
            .is_some_and(|s| s.contains("depth 64")),
        "{stored:?}"
    );
    assert_eq!(memory.trace(req()).walk_incomplete, stored.walk_incomplete);
}

#[test]
fn confidence_filtering_cannot_walk_through_an_excluded_bridge() {
    let extractions = vec![extract_file("app.py", "def target():\n    return 1\ndef bridge():\n    return target()\ndef entry():\n    return bridge()\n")];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let mut resolution = resolver.resolve_all(&extractions);
    let bridge = resolution
        .edges
        .iter_mut()
        .find(|edge| {
            edge.source_symbol == "app.py::entry" && edge.target_symbol == "app.py::bridge"
        })
        .expect("fixture bridge");
    bridge.confidence = Confidence(0.4);
    let memory = QueryEngine::new(&extractions, &resolution);
    let req = |target: &str, floor| Request {
        query: target.into(),
        token_budget: 20_000,
        min_confidence: floor,
        max_depth: 64,
    };
    assert!(memory
        .trace(req("entry", 0.0))
        .items
        .iter()
        .any(|e| e.target_symbol == "app.py::target"));
    let forward = memory.trace(req("entry", 0.8));
    assert!(
        !forward
            .items
            .iter()
            .any(|e| e.target_symbol == "app.py::target"),
        "a disconnected edge leaked across a filtered bridge: {forward:?}"
    );

    // Reverse the floor placement: a low edge into the target must hide the
    // high edge further upstream, in the same way as forward traversal.
    for edge in &mut resolution.edges {
        if edge.source_symbol == "app.py::entry" && edge.target_symbol == "app.py::bridge" {
            edge.confidence = Confidence(1.0);
        }
        if edge.source_symbol == "app.py::bridge" && edge.target_symbol == "app.py::target" {
            edge.confidence = Confidence(0.4);
        }
    }
    let memory = QueryEngine::new(&extractions, &resolution);
    let reverse = memory.impact(req("target", 0.8));
    assert!(
        !reverse
            .items
            .iter()
            .any(|e| e.source_symbol == "app.py::entry"),
        "{reverse:?}"
    );
}

#[test]
fn layered_impact_spends_one_budget_across_both_parts() {
    let mut source = "def leaf():\n    return 1\n".to_string();
    for i in 0..50 {
        let target = if i == 0 {
            "leaf".to_string()
        } else {
            format!("node_{}", i - 1)
        };
        source.push_str(&format!("def node_{i}():\n    return {target}()\n"));
    }
    let store = store_of(&[("app.py", &source)], &[]);
    let report = StoreQueryEngine::new(&store)
        .impact_layered(Request {
            query: "leaf".into(),
            token_budget: 1000,
            min_confidence: 0.0,
            max_depth: 64,
        })
        .unwrap();
    let edge_tokens = report.edges.tokens_used;
    let layer_tokens = report.blast_radius.layers.tokens_used;
    assert!(
        edge_tokens > 0 && layer_tokens > 0,
        "both sides must spend tokens: {report:?}"
    );
    assert!(
        edge_tokens <= 500 && layer_tokens <= 500,
        "the caller supplied one shared budget: {report:?}"
    );
    assert!(edge_tokens + layer_tokens <= 1000);
}

#[test]
fn explained_calls_do_not_make_the_index_incomplete() {
    for (path, source) in [
        ("builtin.py", "def entry():\n    print(1)\n"),
        ("host.ts", "export function entry() { return setTimeout(() => 1, 10); }"),
        ("external.ts", "import { readFileSync } from 'node:fs'; export function entry() { return readFileSync('fixture'); }"),
    ] {
        let store = store_of(&[(path, source)], &[]);
        let analysis = store.latest_analysis().unwrap().unwrap();
        assert!(analysis.unresolved_calls > 0, "fixture must exercise unresolved sites: {path}");
        assert_eq!(analysis.resolution_rate.unresolved_sites, analysis.resolution_rate.explained_sites, "fixture must contain only explained sites: {analysis:?}");
        let engine = StoreQueryEngine::new(&store);
        let dead = engine.dead_symbols(10_000).unwrap();
        assert_eq!(dead.walk_incomplete, None, "known external targets must not claim missing internal edges: {path}");
        let impact = engine.impact(Request {query: "entry".into(), token_budget: 10_000, min_confidence: 0.0, max_depth: 64}).unwrap();
        assert_eq!(impact.walk_incomplete, None, "{path}: {impact:?}");
    }
}

#[test]
fn mixed_attribution_counts_exclude_explained_sites_and_name_their_scope() {
    let store = store_of(&[("app.py", "def entry(callback, receiver):\n    print(1)\n    callback()\n    receiver.method()\n    missing()\n")], &[]);
    let analysis = store.latest_analysis().unwrap().unwrap();
    let rate = &analysis.resolution_rate;
    assert!(rate.explained_sites > 0 && rate.unresolved_sites > rate.explained_sites);
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();
    let reason = dead.walk_incomplete.unwrap();
    let remaining = rate.unresolved_sites - rate.explained_sites;
    assert!(
        reason.contains(&format!("{remaining} of {}", rate.unresolved_sites)),
        "{reason}"
    );
    assert!(
        reason.contains("repository-wide") && reason.contains("not specific to this target"),
        "{reason}"
    );
    assert!(
        !reason.contains("missing that many edges"),
        "unresolved sites are not an exact edge count: {reason}"
    );
}

#[test]
fn affected_tests_and_explore_keep_repository_coverage_warnings() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "test_app.py",
                "from lib import helper\ndef test_helper():\n    return helper()\n",
            ),
            ("unknown.py", "def entry(callback):\n    callback()\n"),
        ],
        &[],
    );
    let engine = StoreQueryEngine::new(&store);
    let affected = engine
        .affected_tests(&["helper".into()], 10_000, 0.0, 64)
        .unwrap();
    assert!(affected
        .tests
        .items
        .iter()
        .any(|test| test.path == "test_app.py"));
    assert!(
        affected.tests.walk_incomplete.is_some(),
        "a completed walk must not erase unresolved callbacks: {affected:?}"
    );
    assert_eq!(
        affected.tests.walk_incomplete,
        affected.blast_radius.layers.walk_incomplete
    );
    let explored = engine.explore("helper", 5, 10_000, 0.0, 64).unwrap();
    assert!(
        explored.blast_radius.layers.walk_incomplete.is_some(),
        "{explored:?}"
    );
}

#[test]
fn layered_impact_keeps_the_same_coverage_warning_on_both_halves() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\ndef entry(callback):\n    helper()\n    callback()\n",
            ),
        ],
        &[],
    );
    let report = StoreQueryEngine::new(&store)
        .impact_layered(Request {
            query: "helper".into(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 64,
        })
        .unwrap();
    assert!(report.edges.walk_incomplete.is_some());
    assert_eq!(
        report.edges.walk_incomplete,
        report.blast_radius.layers.walk_incomplete
    );
}

#[test]
fn in_memory_traversals_keep_the_same_attribution_warning_as_storage() {
    let files = [
        ("lib.py", "def helper():\n    return 1\n"),
        (
            "app.py",
            "from lib import helper\ndef entry(callback):\n    helper()\n    callback()\n",
        ),
    ];
    let extractions: Vec<_> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let memory = QueryEngine::new(&extractions, &resolution);
    let store = store_of(&files, &[]);
    let stored = StoreQueryEngine::new(&store);
    let req = |target: &str| Request {
        query: target.into(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 64,
    };
    let expected = stored.impact(req("helper")).unwrap().walk_incomplete;
    assert!(expected.is_some());
    assert_eq!(memory.impact(req("helper")).walk_incomplete, expected);
    assert_eq!(
        memory.trace(req("entry")).walk_incomplete,
        stored.trace(req("entry")).unwrap().walk_incomplete
    );
    assert_eq!(
        memory.impact(req("missing_target")).walk_incomplete,
        stored
            .impact(req("missing_target"))
            .unwrap()
            .walk_incomplete
    );
    assert_eq!(
        memory.trace(req("missing_target")).walk_incomplete,
        stored.trace(req("missing_target")).unwrap().walk_incomplete
    );
}

#[test]
fn output_budget_depth_and_attribution_gaps_survive_together() {
    let store = store_of(
        &[
            ("d.py", "def d():\n    return 1\n"),
            ("c.py", "from d import d\ndef c():\n    return d()\n"),
            ("b.py", "from c import c\ndef b():\n    return c()\n"),
            (
                "a.py",
                "from b import b\ndef a(callback):\n    b()\n    callback()\n",
            ),
        ],
        &[],
    );
    let report = StoreQueryEngine::new(&store)
        .impact_layered(Request {
            query: "d".into(),
            token_budget: 0,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    assert!(report.edges.truncated && report.edges.total > report.edges.shown);
    let reason = report.edges.walk_incomplete.as_ref().unwrap();
    assert!(
        reason.contains("depth") && reason.contains("unresolved attribution"),
        "{reason}"
    );
    assert_eq!(
        report.edges.walk_incomplete,
        report.blast_radius.layers.walk_incomplete
    );
}

/// A chain `a -> b -> c -> d`, four hops, so a depth-2 walk must stop short.
fn chain() -> Vec<devmap_extract::model::Extraction> {
    let files = [
        ("d.py", "def d():\n    return 1\n"),
        ("c.py", "from d import d\n\n\ndef c():\n    return d()\n"),
        ("b.py", "from c import c\n\n\ndef b():\n    return c()\n"),
        ("a.py", "from b import b\n\n\ndef a():\n    return b()\n"),
    ];
    files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect()
}

#[test]
fn a_depth_capped_impact_walk_reports_that_it_stopped() {
    let extractions = chain();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let engine = QueryEngine::new(&extractions, &resolution);

    let capped = engine.impact(Request {
        query: "d".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 1,
    });

    assert!(
        capped.walk_incomplete.is_some(),
        "a walk stopped by the depth cap must say so; publishing `None` presents a \
         lower bound as the full blast radius, which is the reading that gets a live \
         symbol deleted. Got: {capped:?}"
    );
    let reason = capped.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        reason.contains("depth"),
        "the reason must name what stopped the walk, got {reason:?}"
    );
}

/// The signal must be *absent* when the walk actually completed.
///
/// Without this, a fix that always sets `walk_incomplete` would pass the test
/// above while making the marker meaningless — every answer would carry it, and
/// a caller that cannot tell complete from incomplete is back where it started.
#[test]
fn a_complete_walk_does_not_claim_to_be_incomplete() {
    let extractions = chain();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let engine = QueryEngine::new(&extractions, &resolution);

    let complete = engine.impact(Request {
        query: "d".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 64,
    });

    assert!(
        complete.walk_incomplete.is_none(),
        "a walk that reached the end of the graph must not be marked incomplete, \
         got {:?}",
        complete.walk_incomplete
    );
}

#[test]
fn a_depth_capped_trace_walk_reports_that_it_stopped() {
    let extractions = chain();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let engine = QueryEngine::new(&extractions, &resolution);

    let capped = engine.trace(Request {
        query: "a".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 1,
    });

    assert!(
        capped.walk_incomplete.is_some(),
        "trace must carry the same signal impact does; got {capped:?}"
    );
}

/// A persisted generation over `files`, as `devmap build` would write one.
fn store_of(files: &[(&str, &str)], refuse: &[&str]) -> Store {
    let mut extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    for ext in &mut extractions {
        if !refuse.contains(&ext.file_path.as_str()) {
            continue;
        }
        // Shaped as `refused_extraction` builds one: the File node only.
        ext.parse_outcome = ParseOutcome::Failed {
            reason: "forced parse failure".to_string(),
        };
        ext.engine = ExtractionEngine::Unavailable {
            requested_language: ext.language.clone(),
        };
        ext.symbols.retain(|sym| sym.kind == SymbolKind::File);
        ext.imports.clear();
        ext.calls.clear();
        ext.wiring.clear();
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
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

/// A call the resolution ladder cannot attribute to anything, beside a symbol
/// nothing calls. The dead-symbol finding and the blind spot that could
/// contradict it live in the same generation, which is the whole point.
#[test]
fn dead_symbols_computed_over_unattributed_calls_say_so() {
    let store = store_of(
        &[
            ("lib.py", "def abandoned():\n    return 1\n"),
            (
                "app.py",
                "def run(thing):\n    return thing.mystery_method()\n",
            ),
        ],
        &[],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();

    assert!(
        dead.items.iter().any(|row| row.symbol_name == "abandoned"),
        "the finding itself must survive — hiding it would be its own lie: {:?}",
        dead.items
    );
    assert!(
        dead.walk_incomplete.is_some(),
        "this list is read as 'delete these', and it was computed from a call \
         graph with holes in it. A generation with unattributed calls must not \
         answer in the shape of one with none. Got {dead:?}"
    );
    let reason = dead.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        reason.contains("unattributed") || reason.contains("unresolved"),
        "the reason must name the unattributed calls, got {reason:?}"
    );
}

/// The status half. Corpus-level extraction loss already reaches
/// `AnalysisStatus::Partial` and `graph_degraded`; the dead-symbol query is the
/// surface that most needs it and was the one not carrying it.
#[test]
fn dead_symbols_from_a_degraded_analysis_say_so() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &["app.py"],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();

    let reason = dead.walk_incomplete.as_deref().unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "the analysis this list came from is `partial`; the list must not \
         read as complete. Got {dead:?}"
    );
    assert!(
        reason.contains("partial"),
        "the reason must carry the analysis status, got {reason:?}"
    );
}

/// The OFF direction. A generation where every call resolved and the analysis
/// converged must answer clean — otherwise the marker is on every answer and a
/// caller is back to having no way to tell the two apart.
#[test]
fn dead_symbols_over_a_complete_analysis_do_not_claim_to_be_incomplete() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &[],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();

    assert!(
        dead.items.iter().any(|row| row.symbol_name == "main"),
        "nothing calls `main`, so the query still has an answer: {:?}",
        dead.items
    );
    assert!(
        dead.walk_incomplete.is_none(),
        "every call resolved and the analysis converged: {:?}",
        dead.walk_incomplete
    );
}

/// Q-8: the qualification has to carry a *denominator*, not just an adjective.
///
/// "The analysis is partial" tells a reader to be careful and nothing about how
/// careful. The audit's complaint was the missing number: a list read as
/// "delete these" needs to say how much of the corpus was actually searched for
/// callers, because one unread file is a different risk from two hundred.
///
/// The numbers reach here through `ExtractionCoverage::degraded_reason`, which
/// `analyze()` folds into `AnalysisStatus::Partial` and `dead_symbols` surfaces
/// on `walk_incomplete`. This pins the whole chain: a change anywhere along it
/// that drops the counts leaves the adjective behind and fails here.
#[test]
fn a_degraded_dead_symbol_list_names_how_much_of_the_corpus_was_read() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &["app.py"],
    );
    let dead = StoreQueryEngine::new(&store).dead_symbols(10_000).unwrap();
    let reason = dead
        .walk_incomplete
        .as_deref()
        .expect("a partial analysis must qualify its dead list");

    assert!(
        reason.contains("did not cover the whole corpus"),
        "the reason must name the coverage gap, got {reason:?}"
    );
    assert!(
        reason.contains("1 file(s) failed to parse"),
        "the reason must carry the count of unread files, got {reason:?}"
    );
    assert!(
        reason.contains("lower bound"),
        "the reason must say the list is a lower bound, got {reason:?}"
    );
    // And the counters still describe the page honestly alongside it.
    assert_eq!(dead.shown as usize, dead.items.len());
    assert_eq!(dead.total, dead.shown + dead.hidden);
}

/// Force a stored parse outcome, as the extractor would have recorded it.
///
/// `Fallback` is what the pattern scanner produces for a language with no
/// linked grammar (`devmap-extract/src/fallback.rs`): named declarations are
/// matched, and **no calls and no imports are extracted at all**. Reproducing
/// that here rather than relying on which grammars this build happens to link
/// keeps the test about the outcome flag, which is the thing every consumer
/// reads.
fn degrade(ext: &mut Extraction, outcome: ParseOutcome) {
    if matches!(outcome, ParseOutcome::Fallback { .. }) {
        ext.engine = ExtractionEngine::Unavailable {
            requested_language: ext.language.clone(),
        };
        ext.imports.clear();
        ext.calls.clear();
    }
    ext.parse_outcome = outcome;
}

fn store_of_extractions(extractions: Vec<Extraction>) -> Store {
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
            GenerationWriteOpts::default(),
        )
        .unwrap();
    store
}

/// `dependencies` called a pattern-recovered file's edge list complete.
///
/// Both engines special-cased `ParseOutcome::Failed` — a file that contributed
/// nothing — and let `Fallback` fall through to `resolution: Available` with no
/// caveat. But a fallback extraction contains no imports and no calls by
/// construction, so its dependency set is empty *because nothing looked*, and
/// that answer was byte-identical to the one given for a fully parsed file that
/// genuinely imports nothing. `preview` has always reported this file state;
/// `dependencies` is where a reader goes to ask what a file needs.
///
/// `Partial` is the same shape one tier up: tree-sitter parsed the file and
/// flagged error ranges, and a call inside an error region is invisible to
/// extraction — so the edge list is a lower bound there too.
#[test]
fn dependencies_over_a_degraded_parse_say_the_edge_list_is_a_lower_bound() {
    for outcome in [
        ParseOutcome::Fallback {
            reason: "no linked grammar for vb; declarations recovered by pattern".to_string(),
        },
        ParseOutcome::Partial {
            error_ranges: Vec::new(),
        },
    ] {
        let mut extractions = vec![
            extract_file("lib.py", "def helper():\n    return 1\n"),
            extract_file(
                "app.py",
                "from lib import helper\n\n\ndef run():\n    return helper()\n",
            ),
        ];
        degrade(&mut extractions[1], outcome.clone());
        let store = store_of_extractions(extractions.clone());

        let stored = StoreQueryEngine::new(&store)
            .dependencies(Request {
                query: "app.py".to_string(),
                token_budget: 10_000,
                min_confidence: 0.0,
                max_depth: 3,
            })
            .unwrap();
        assert!(
            stored.walk_incomplete.is_some(),
            "a {outcome:?} file's dependency list is a lower bound and must say so; got {stored:?}"
        );

        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let in_memory = QueryEngine::new(&extractions, &resolution).dependencies(Request {
            query: "app.py".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        });
        assert!(
            in_memory.walk_incomplete.is_some(),
            "the in-memory engine must carry the same caveat as the store-backed one; \
             got {in_memory:?}"
        );
    }
}

/// The caveat must not appear on a clean parse.
///
/// A marker that rides on every answer leaves a reader exactly where they
/// started, which is the failure mode `dead_symbol_coverage_gap` documents.
#[test]
fn dependencies_over_a_clean_parse_carry_no_coverage_caveat() {
    let extractions = vec![
        extract_file("lib.py", "def helper():\n    return 1\n"),
        extract_file(
            "app.py",
            "from lib import helper\n\n\ndef run():\n    return helper()\n",
        ),
    ];
    let store = store_of_extractions(extractions.clone());

    let stored = StoreQueryEngine::new(&store)
        .dependencies(Request {
            query: "app.py".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .unwrap();
    assert_eq!(
        stored.walk_incomplete, None,
        "a cleanly parsed file's dependency list is complete: {stored:?}"
    );
    assert!(
        !stored.items.is_empty(),
        "the fixture must actually produce edges, or the assertion above is vacuous"
    );
}

/// `impact` answered "nothing calls this" over a corpus it had not fully read.
///
/// `dead_symbols` has carried the analysis coverage on `walk_incomplete` since
/// Q-8, and it is the *safer* of the two surfaces: it is explicitly a
/// candidate list, and its exemption rules already drop symbols in unread
/// files. `impact` is the one a reader consults immediately before deleting or
/// changing a symbol — the MCP tool description says exactly that — and it
/// published `items: [], resolution: Available, walk_incomplete: None` on a
/// generation whose call extraction never covered the file that might hold the
/// caller. An empty blast radius and an unsearched one were the same answer.
///
/// `trace` is the same walk in the other direction and gets it from the same
/// place: there is one `traverse_over`.
#[test]
fn a_traversal_over_a_partly_read_corpus_says_so() {
    // `app.py` contributed no calls at all, so nothing in it can appear as a
    // caller of `helper` — which is precisely the file a reader would need to
    // have been searched before believing an empty answer.
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &["app.py"],
    );
    let engine = StoreQueryEngine::new(&store);
    let request = |query: &str| Request {
        query: query.to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 3,
    };

    let blast = engine.impact(request("helper")).unwrap();
    let reason = blast
        .walk_incomplete
        .as_deref()
        .unwrap_or_else(|| panic!("impact over a partial analysis must qualify itself: {blast:?}"));
    assert!(
        reason.contains("did not cover the whole corpus"),
        "the qualification must carry the coverage numbers, not just an adjective: {reason:?}"
    );

    // `helper` has no forward edges, and `trace` says so outright with
    // `Unavailable` — an honest refusal, not a silent empty answer, so it is
    // not the case worth pinning. `lib.py` does have forward edges (it contains
    // `helper`), and that is where the same walk must carry the same signal.
    let forward = engine.trace(request("lib.py")).unwrap();
    assert!(
        !forward.items.is_empty(),
        "the fixture must give trace something to walk: {forward:?}"
    );
    assert!(
        forward.walk_incomplete.is_some(),
        "trace shares one traversal with impact and must carry the same signal: {forward:?}"
    );
}

/// The corpus caveat must not displace the walk's own stop reason.
///
/// Two independent qualifications — "the graph I walked has holes" and "I
/// stopped before the graph ran out" — and a reader deciding whether to delete
/// a symbol needs both. Assigning rather than composing would have silently
/// dropped whichever ran second.
#[test]
fn a_capped_walk_over_a_partly_read_corpus_reports_both_reasons() {
    let store = store_of(
        &[
            ("d.py", "def d():\n    return 1\n"),
            ("c.py", "from d import d\n\n\ndef c():\n    return d()\n"),
            ("b.py", "from c import c\n\n\ndef b():\n    return c()\n"),
            ("a.py", "from b import b\n\n\ndef a():\n    return b()\n"),
            ("unread.py", "def spare():\n    return 2\n"),
        ],
        &["unread.py"],
    );
    let capped = StoreQueryEngine::new(&store)
        .impact(Request {
            query: "d".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 1,
        })
        .unwrap();
    let reason = capped
        .walk_incomplete
        .as_deref()
        .unwrap_or_else(|| panic!("a capped walk must report its cap: {capped:?}"));
    assert!(
        reason.contains("depth"),
        "the walk's own stop reason must survive: {reason:?}"
    );
    assert!(
        reason.contains("did not cover the whole corpus"),
        "the corpus coverage gap must survive alongside it: {reason:?}"
    );
}

/// A complete analysis leaves a complete walk unqualified.
#[test]
fn a_complete_traversal_over_a_complete_corpus_claims_nothing() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "from lib import helper\n\n\ndef main():\n    return helper()\n",
            ),
        ],
        &[],
    );
    let blast = StoreQueryEngine::new(&store)
        .impact(Request {
            query: "helper".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 5,
        })
        .unwrap();
    assert!(
        !blast.items.is_empty(),
        "the fixture must produce a real blast radius, or the assertion below is vacuous"
    );
    assert_eq!(
        blast.walk_incomplete, None,
        "a complete walk over a fully read corpus must claim nothing: {blast:?}"
    );
}

/// `search` reported "no such symbol" over a corpus with a hole in it.
///
/// The last member of the class the tests above close, and the one that reads
/// as the most definitive: `total=0 shown=0 hidden=0 truncated=false
/// resolution=Available walk_incomplete=None` is this kernel's way of saying
/// *"there are zero matches in this corpus"* as a completed check. When a file
/// was refused — a parse that blew its budget, a grammar that would not load, a
/// NUL byte at the boundary — every symbol in it is absent from the index, and
/// a name that lives only there answers exactly the same way as a name that
/// exists nowhere.
///
/// The distinction matters most for the reading a caller acts on: "this symbol
/// does not exist, so I can name my new one that" and "this symbol may exist in
/// a file nothing read" are different facts, and they were the same answer.
#[test]
fn search_over_a_corpus_with_a_refused_file_says_so() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            ("unread.py", "def only_declared_here():\n    return 2\n"),
        ],
        &["unread.py"],
    );
    let found = StoreQueryEngine::new(&store)
        .search(Request {
            query: "only_declared_here".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .unwrap();

    assert_eq!(
        found.total, 0,
        "the fixture must actually hide the symbol, or this test proves nothing: {found:?}"
    );
    let reason = found.walk_incomplete.as_deref().unwrap_or_else(|| {
        panic!("a search over a partly read corpus must not report zero matches as a completed check: {found:?}")
    });
    assert!(
        reason.contains("did not cover the whole corpus"),
        "the qualification must carry the coverage numbers, not just an adjective: {reason:?}"
    );
    assert!(
        reason.contains("1 file(s) failed to parse"),
        "the reason must name how many files went unread: {reason:?}"
    );
    assert!(
        reason.contains("not indexed") || reason.contains("not a complete read"),
        "the reason must say what the gap means for a *search*, not only for the \
         dead-code surface the counts were first written for: {reason:?}"
    );
}

/// A non-empty page over the same corpus carries it too.
///
/// The caveat is about the corpus, not about the answer being empty: a caller
/// reading `total=1` has just as much reason to know a second definition may
/// sit in a file nothing read.
#[test]
fn a_non_empty_search_over_a_refused_file_carries_the_same_caveat() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            ("unread.py", "def helper_two():\n    return 2\n"),
        ],
        &["unread.py"],
    );
    let found = StoreQueryEngine::new(&store)
        .search(Request {
            query: "helper".to_string(),
            token_budget: 10_000,
            min_confidence: 0.0,
            max_depth: 3,
        })
        .unwrap();

    assert!(
        !found.items.is_empty(),
        "the fixture must return hits, or the assertion below is vacuous: {found:?}"
    );
    assert!(
        found.walk_incomplete.is_some(),
        "a page drawn from a partly read corpus is a lower bound whether or not it \
         is empty: {found:?}"
    );
}

/// The OFF direction, and the load-bearing one.
///
/// A marker that rides on every search tells a reader nothing. `unresolved_calls`
/// is deliberately *not* folded in here for exactly that reason: unattributed
/// call edges are non-zero on essentially every real corpus and say nothing
/// about whether a symbol was indexed.
#[test]
fn search_over_a_complete_corpus_claims_nothing() {
    let store = store_of(
        &[
            ("lib.py", "def helper():\n    return 1\n"),
            (
                "app.py",
                "def run(thing):\n    return thing.mystery_method()\n",
            ),
        ],
        &[],
    );
    let engine = StoreQueryEngine::new(&store);
    let request = |query: &str| Request {
        query: query.to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 3,
    };

    // `mystery_method` is unattributable, so this generation really does hold
    // unresolved calls — the condition that must *not* qualify a search.
    let dead = engine.dead_symbols(10_000).unwrap();
    assert!(
        dead.walk_incomplete
            .as_deref()
            .unwrap_or_default()
            .contains("unresolved attribution"),
        "the fixture must actually hold unattributed calls, or the assertions \
         below are vacuous: {dead:?}"
    );

    let hit = engine.search(request("helper")).unwrap();
    assert_eq!(
        hit.walk_incomplete, None,
        "every file in this corpus was read; a search over it must claim nothing: {hit:?}"
    );
    assert!(
        !hit.items.is_empty(),
        "the fixture must return hits, or the assertion above is vacuous: {hit:?}"
    );

    let miss = engine.search(request("no_such_symbol_anywhere")).unwrap();
    assert_eq!(miss.total, 0, "the fixture must actually miss: {miss:?}");
    assert_eq!(
        miss.walk_incomplete, None,
        "an empty answer over a fully read corpus is a completed check and may \
         say so: {miss:?}"
    );
}

/// The in-memory engine shares the defect and must share the fix.
///
/// `QueryEngine::search` scans the extraction slice it was handed, so a refused
/// file in that slice hides its symbols exactly as a refused row does in the
/// store. One surface fixed and the other not would let the same corpus answer
/// differently depending on which engine the caller reached.
#[test]
fn the_in_memory_search_carries_the_same_caveat() {
    let mut extractions = vec![
        extract_file("lib.py", "def helper():\n    return 1\n"),
        extract_file("unread.py", "def only_declared_here():\n    return 2\n"),
    ];
    degrade(
        &mut extractions[1],
        ParseOutcome::Failed {
            reason: "forced parse failure".to_string(),
        },
    );
    extractions[1]
        .symbols
        .retain(|sym| sym.kind == SymbolKind::File);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);

    let found = QueryEngine::new(&extractions, &resolution).search(Request {
        query: "only_declared_here".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 3,
    });
    assert_eq!(
        found.total, 0,
        "the fixture must actually hide the symbol: {found:?}"
    );
    assert!(
        found.walk_incomplete.is_some(),
        "the in-memory engine must carry the same caveat as the store-backed one: {found:?}"
    );

    let clean = vec![
        extract_file("lib.py", "def helper():\n    return 1\n"),
        extract_file("other.py", "def spare():\n    return 2\n"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&clean);
    let resolution = resolver.resolve_all(&clean);
    let complete = QueryEngine::new(&clean, &resolution).search(Request {
        query: "helper".to_string(),
        token_budget: 10_000,
        min_confidence: 0.0,
        max_depth: 3,
    });
    assert_eq!(
        complete.walk_incomplete, None,
        "a fully read slice must leave the marker off: {complete:?}"
    );
}
