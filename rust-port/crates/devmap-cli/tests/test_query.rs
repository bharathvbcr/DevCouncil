use devmap_analyze::model::*;
use devmap_extract::*;
use devmap_query::*;
use devmap_resolve::*;

fn freshness() -> FreshnessInfo {
    FreshnessInfo {
        head_sha: "test-head".into(),
        generation_id: 1,
        pending_count: 0,
        stamped: Default::default(),
    }
}

#[test]
fn test_token_budgeting() {
    let ext = extract_file("src/handler.py", "def process_request():\n    pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));

    let extractions = vec![ext];
    let engine = QueryEngine::new(&extractions, &resolution);

    // Request with generous budget
    let req = Request {
        query: "process_request".to_string(),
        token_budget: Budget::SEARCH,
        min_confidence: 0.5,
        max_depth: 1,
    };
    let res = engine.search(req);
    assert_eq!(res.shown, 1);
    assert_eq!(res.total, 1);
    assert!(!res.truncated);
    assert!(res.items[0].source_span.contains("process_request"));

    // Request with tight budget forcing truncation
    let req_tight = Request {
        query: "process_request".to_string(),
        token_budget: 5,
        min_confidence: 0.5,
        max_depth: 1,
    };
    let res_tight = engine.search(req_tight);
    assert!(res_tight.truncated || res_tight.shown <= 1);
    assert_eq!(res_tight.total, 1);
}

#[test]
fn test_manifest_budget_t1() {
    let extractions = vec![
        extract_file("src/main.rs", "fn main() {}\n"),
        extract_file("Cargo.toml", "[package]\nname=\"foo\"\n"),
        extract_file("README.md", "# DevCouncil\n"),
    ];
    let analysis = AnalysisSummary {
        // No discovery step ran over this hand-built corpus, so there is no
        // refusal count to report. `None` says that; `0` would claim a walk.
        discovery_refused_files: None,
        total_files: 3,
        total_symbols: 1,
        total_edges: 0,
        dead_symbols: vec![],
        communities: vec![],
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
        // Fields this fixture does not exercise. Spread rather than
        // enumerated so a new analysis field does not break every test
        // literal in the workspace; the one production construction in
        // `analyze()` still names every field exhaustively.
        ..Default::default()
    };

    let (_manifest, json_str) = generate_manifest(&extractions, &analysis, freshness());
    let estimated_tokens = (json_str.len() / 4) as u32;
    assert!(
        estimated_tokens <= Budget::MANIFEST,
        "Manifest tokens {} exceeds limit {}",
        estimated_tokens,
        Budget::MANIFEST
    );
}

#[test]
fn manifest_reports_supplied_head_and_pending_count() {
    let analysis = AnalysisSummary {
        // No discovery step ran over this hand-built corpus, so there is no
        // refusal count to report. `None` says that; `0` would claim a walk.
        discovery_refused_files: None,
        total_files: 0,
        total_symbols: 0,
        total_edges: 0,
        dead_symbols: vec![],
        communities: vec![],
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
        // Fields this fixture does not exercise. Spread rather than
        // enumerated so a new analysis field does not break every test
        // literal in the workspace; the one production construction in
        // `analyze()` still names every field exhaustively.
        ..Default::default()
    };
    let (manifest, _) = generate_manifest(
        &[],
        &analysis,
        FreshnessInfo {
            head_sha: "0123456789abcdef".into(),
            generation_id: 42,
            pending_count: 7,
            stamped: Default::default(),
        },
    );
    assert_eq!(manifest.freshness.head_sha, "0123456789abcdef");
    assert_eq!(manifest.freshness.generation_id, 42);
    assert_eq!(manifest.freshness.pending_count, 7);
}

#[test]
fn test_manifest_budget_resists_hostile_paths() {
    let long_path = format!("src/{}.ts", "x".repeat(9_000));
    let analysis = AnalysisSummary {
        // No discovery step ran over this hand-built corpus, so there is no
        // refusal count to report. `None` says that; `0` would claim a walk.
        discovery_refused_files: None,
        total_files: 1,
        total_symbols: 1,
        total_edges: 0,
        dead_symbols: vec![],
        communities: vec![CommunityReport {
            community_id: 1,
            name: "hostile-community".to_string(),
            members: vec![long_path],
            cohesion_score: 1.0,
        }],
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
        // Fields this fixture does not exercise. Spread rather than
        // enumerated so a new analysis field does not break every test
        // literal in the workspace; the one production construction in
        // `analyze()` still names every field exhaustively.
        ..Default::default()
    };

    let (manifest, json) = generate_manifest(&[], &analysis, freshness());
    let estimated_tokens = json.len().div_ceil(4) as u32;
    assert!(
        estimated_tokens <= Budget::MANIFEST,
        "manifest tokens {} exceeds limit {}",
        estimated_tokens,
        Budget::MANIFEST
    );
    assert!(
        manifest.subsystems.is_empty(),
        "oversized entry must be omitted"
    );
}

#[test]
fn test_search_never_exceeds_hard_token_budget() {
    let ext = extract_file(
        "src/sample.py",
        "def expensive_func():\n    return 'a deliberately long source span'\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let engine = QueryEngine::new(std::slice::from_ref(&ext), &resolution);

    let response = engine.search(Request {
        query: "expensive_func".to_string(),
        token_budget: 1,
        min_confidence: 0.0,
        max_depth: 1,
    });

    assert_eq!(response.total, 1);
    assert_eq!(response.shown, 0);
    assert!(response.truncated);
    assert!(
        response.tokens_used <= 1,
        "response used {} tokens with a budget of 1",
        response.tokens_used
    );
}

#[test]
fn test_manifest_order_is_deterministic_for_tied_communities() {
    let extractions = vec![
        extract_file("z.py", "if __name__ == '__main__':\n    pass\n"),
        extract_file("a.py", "if __name__ == '__main__':\n    pass\n"),
    ];
    let analysis = AnalysisSummary {
        // No discovery step ran over this hand-built corpus, so there is no
        // refusal count to report. `None` says that; `0` would claim a walk.
        discovery_refused_files: None,
        total_files: 2,
        total_symbols: 2,
        total_edges: 0,
        dead_symbols: vec![],
        communities: vec![
            CommunityReport {
                community_id: 1,
                name: "community-z".to_string(),
                members: vec!["z.py".to_string()],
                cohesion_score: 0.0,
            },
            CommunityReport {
                community_id: 2,
                name: "community-a".to_string(),
                members: vec!["a.py".to_string()],
                cohesion_score: 0.0,
            },
        ],
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
        clone_coverage: Default::default(),
        // Fields this fixture does not exercise. Spread rather than
        // enumerated so a new analysis field does not break every test
        // literal in the workspace; the one production construction in
        // `analyze()` still names every field exhaustively.
        ..Default::default()
    };

    let (_manifest, json) = generate_manifest(&extractions, &analysis, freshness());
    assert!(
        json.find("community-a").unwrap() < json.find("community-z").unwrap(),
        "tied communities must have a stable path-based order: {json}"
    );
    assert!(
        json.find("a.py").unwrap() < json.find("z.py").unwrap(),
        "entry roots must be sorted before truncation: {json}"
    );
}

#[test]
fn test_empty_search_is_fail_closed() {
    let ext = extract_file("src/empty_query.py", "def visible(): pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let engine = QueryEngine::new(std::slice::from_ref(&ext), &resolution);

    let response = engine.search(Request {
        query: "   ".to_string(),
        token_budget: Budget::SEARCH,
        min_confidence: 0.0,
        max_depth: 1,
    });

    assert!(response.items.is_empty());
    assert_eq!(response.shown, 0);
    assert_eq!(response.total, 0);
    assert!(!response.truncated);
    assert_eq!(response.tokens_used, 0);
}

#[test]
fn test_g8_impact_depth_expands_the_full_inbound_chain() {
    let files = vec![
        extract_file("a.py", "def a(): pass\n"),
        extract_file("b.py", "def b(): pass\n"),
        extract_file("c.py", "def c(): pass\n"),
    ];
    let edge = |source: &str, target: &str| ResolvedEdge {
        source_file: format!("{source}.py"),
        target_file: format!("{target}.py"),
        source_symbol: source.to_string(),
        target_symbol: target.to_string(),
        edge_kind: EdgeKind::Calls,
        confidence: Confidence::DETERMINISTIC,
        resolution: None,
        details: None,
        evidence: None,
    };
    let resolution = ResolutionResult {
        edges: vec![edge("a", "b"), edge("b", "c")],
        receiver_types: Default::default(),
        reexport_chains: Default::default(),
        unresolved: Vec::new(),
    };
    let engine = QueryEngine::new(&files, &resolution);
    let direct_walk = devmap_analyze::traversal::traverse_graph(
        &["c".to_string()],
        &resolution.edges,
        &devmap_analyze::traversal::TraversalOptions {
            max_depth: 2,
            max_nodes: 5000,
            reverse: true,
        },
    );
    assert_eq!(direct_walk.traversed_edges.len(), 2);

    let depth_one = engine.impact(Request {
        query: "c".to_string(),
        token_budget: Budget::DEPS,
        min_confidence: 0.0,
        max_depth: 1,
    });
    let depth_two = engine.impact(Request {
        query: "c".to_string(),
        token_budget: Budget::DEPS,
        min_confidence: 0.0,
        max_depth: 2,
    });

    assert_eq!(depth_one.items.len(), 1);
    assert!(depth_one
        .items
        .iter()
        .any(|edge| edge.source_symbol == "b" && edge.target_symbol == "c"));
    assert_eq!(
        depth_two.items.len(),
        2,
        "{}",
        serde_json::to_string(&depth_two.items).expect("impact payload serializes")
    );
    assert!(depth_two
        .items
        .iter()
        .any(|edge| edge.source_symbol == "a" && edge.target_symbol == "b"));
}

/// End-to-end for `devmap clones`: extraction stamps the signatures, the store
/// persists them, and the query engine groups them back into findings.
#[test]
fn clones_round_trip_through_the_store() {
    use devmap_store::Store;

    let body = "\n    total = 0\n    for row in rows:\n        if row.active:\n            total += row.amount * rate\n        else:\n            total -= row.penalty\n    return total\n";
    let shared = format!("def compute(rows, rate):{body}");
    let extractions = vec![
        extract_file("pkg/a.py", &shared),
        extract_file("other/b.py", &shared),
        extract_file("pkg/c.py", "def solo(x):\n    return x + 1\n"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);

    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let report = StoreQueryEngine::new(&store).clones(2000, None, 0).unwrap();
    assert_eq!(report.signed_symbols, 2, "expected the two shared bodies");
    assert!(
        report.unsigned_symbols > 0,
        "the trivial function and the file nodes should be unsigned"
    );
    assert_eq!(report.groups.items.len(), 1, "{:?}", report.groups.items);

    let group = &report.groups.items[0];
    assert_eq!(group.kind, devmap_analyze::CloneKind::Exact);
    assert_eq!(group.members.len(), 2);
    assert_eq!(group.members[0].file_path, "other/b.py");
    assert_eq!(group.members[1].file_path, "pkg/a.py");
    assert!(group.min_nodes >= 32);
    assert!(!report.groups.truncated);

    // The build recorded the same denominator the query reports, so a caller
    // reading the generation sees no different a picture than one running the
    // query against it.
    assert_eq!(
        analysis.clone_coverage.signed_symbols,
        report.signed_symbols
    );
    assert_eq!(
        analysis.clone_coverage.unsigned_symbols,
        report.unsigned_symbols
    );

    // A budget too small for the single group withholds it and says so, rather
    // than reporting an empty tree.
    let squeezed = StoreQueryEngine::new(&store).clones(10, None, 0).unwrap();
    assert!(squeezed.groups.items.is_empty());
    assert!(squeezed.groups.truncated, "truncation was not reported");
    assert_eq!(
        squeezed.groups.total, 1,
        "the withheld group was not counted"
    );
    assert_eq!(
        squeezed.signed_symbols, 2,
        "coverage must survive truncation; it is what makes an empty list readable"
    );
}

/// With nothing built, the report says so instead of reporting a clean tree.
#[test]
fn clones_on_an_empty_store_are_unavailable_not_clean() {
    use devmap_store::Store;
    let store = Store::open_in_memory().unwrap();
    let report = StoreQueryEngine::new(&store).clones(2000, None, 0).unwrap();
    assert!(matches!(
        report.groups.resolution,
        ResolutionAvailability::Unavailable { .. }
    ));
    assert_eq!(report.signed_symbols, 0);
}

/// Filters must run before the budget, not after it.
///
/// Filtering an already-budgeted page cannot reach what the budget cut, and
/// re-budgeting the survivors leaves `hidden` at zero — so a filtered subset
/// comes back labelled complete. Observed on this repository before the fix:
/// `--kind exact --min-nodes 100` under a 900-token budget reported "2 groups,
/// not truncated" where the true answer was 29.
///
/// The fixture makes every *structural* group heavier than every *exact* one,
/// so a small budget's unfiltered page is entirely structural. Asking for
/// `exact` must then still reach the exact groups.
#[test]
fn clone_filters_apply_before_the_token_budget() {
    use devmap_store::Store;

    let long_body = |name: &str, var: &str| {
        format!(
            "def {name}(rows, rate):\n    {var} = 0\n    for row in rows:\n        if row.active:\n            {var} += row.amount * rate\n        elif row.pending:\n            {var} += row.amount\n        else:\n            {var} -= row.penalty\n    for extra in rows:\n        {var} += extra.bonus * rate\n    return {var}\n"
        )
    };
    let short_body = |name: &str| {
        format!(
            "def {name}(rows, rate):\n    total = 0\n    for row in rows:\n        total += row.amount * rate\n    return total\n"
        )
    };

    let mut sources: Vec<(String, String)> = Vec::new();
    // Heavy pairs that differ only by a variable name: structural, not exact.
    for i in 0..6 {
        sources.push((
            format!("s{i}a.py"),
            long_body(&format!("wide_{i}"), "total"),
        ));
        sources.push((format!("s{i}b.py"), long_body(&format!("wide_{i}"), "sum_")));
    }
    // Lighter pairs that are byte-identical: exact.
    for i in 0..6 {
        let body = short_body(&format!("thin_{i}"));
        sources.push((format!("e{i}a.py"), body.clone()));
        sources.push((format!("e{i}b.py"), body));
    }

    let extractions: Vec<_> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let engine = StoreQueryEngine::new(&store);
    let all = engine.clones(1_000_000, None, 0).unwrap();
    let exact_total = all
        .groups
        .items
        .iter()
        .filter(|g| g.kind == devmap_analyze::CloneKind::Exact)
        .count() as u32;
    assert!(exact_total > 0, "fixture must produce exact groups");

    // The heaviest group sorts first. Renaming makes the long bodies a single
    // structural group rather than one per name, which is the point of Type-2 —
    // so the counts are read off the data instead of assumed.
    let heaviest = all.groups.items.first().expect("groups exist");
    assert_eq!(
        heaviest.kind,
        devmap_analyze::CloneKind::Structural,
        "fixture must put a structural group at the top of the weight order"
    );
    let budget = devmap_query::clone_group_tokens(heaviest);

    // A budget that holds exactly the heaviest group: the unfiltered page has
    // no exact group in it at all.
    let page = engine.clones(budget, None, 0).unwrap();
    assert!(page.groups.truncated, "budget must actually bite");
    assert!(
        page.groups
            .items
            .iter()
            .all(|g| g.kind == devmap_analyze::CloneKind::Structural),
        "fixture is wrong: the unfiltered page already contains an exact group"
    );

    // Filtering before the budget reaches them; filtering after cannot.
    let narrow = engine
        .clones(budget, Some(devmap_analyze::CloneKind::Exact), 0)
        .unwrap();
    assert!(
        !narrow.groups.items.is_empty(),
        "every exact group was unreachable: the filter ran after the budget"
    );
    assert!(
        narrow
            .groups
            .items
            .iter()
            .all(|g| g.kind == devmap_analyze::CloneKind::Exact),
        "the kind filter let a structural group through"
    );
    assert_eq!(
        narrow.groups.total, exact_total,
        "total must count every group matching the filter, not just the page"
    );
    assert_eq!(narrow.groups.shown + narrow.groups.hidden, exact_total);
}

/// A corpus figure that silently skips what it could not open understates the
/// alternative, which flatters the map. The count must survive.
#[test]
fn savings_counts_files_it_could_not_read_rather_than_calling_them_empty() {
    use devmap_store::{GenerationWriteOpts, Store};

    let dir = std::env::temp_dir().join(format!("devmap-savings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let present = "def kept():\n    return 1\n";
    std::fs::write(dir.join("present.py"), present).unwrap();
    // Indexed but never written to disk: the state a deleted or moved file
    // leaves behind between builds.
    let extractions = vec![
        extract_file("present.py", present),
        extract_file("vanished.py", "def gone():\n    return 2\n"),
    ];
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
                affected_paths: Vec::new(),
                deleted_paths: Vec::new(),
                build_started: None,
                repo_root: Some(dir.to_string_lossy().into_owned()),
                discovery_refusals: None,
            },
        )
        .unwrap();

    let report = StoreQueryEngine::new(&store).savings(None, 2000).unwrap();
    assert_eq!(report.indexed_files, 2);
    assert_eq!(
        report.corpus_files_unreadable, 1,
        "the missing file was folded into the corpus as zero bytes"
    );
    assert_eq!(
        report.corpus_bytes,
        present.len() as u64,
        "corpus_bytes must be the sum of what was actually read"
    );
    assert!(
        report.basis.contains("not a tokenizer count"),
        "the estimate must say it is one: {}",
        report.basis
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The report has no stake in the answer. If the files are cheaper than the
/// query, that is what it says.
#[test]
fn savings_reports_the_query_side_without_assuming_a_saving() {
    use devmap_store::{GenerationWriteOpts, Store};

    let dir = std::env::temp_dir().join(format!("devmap-savings-q-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tiny = "def findme():\n    return 1\n";
    std::fs::write(dir.join("tiny.py"), tiny).unwrap();

    let extractions = vec![extract_file("tiny.py", tiny)];
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
                affected_paths: Vec::new(),
                deleted_paths: Vec::new(),
                build_started: None,
                repo_root: Some(dir.to_string_lossy().into_owned()),
                discovery_refusals: None,
            },
        )
        .unwrap();

    let report = StoreQueryEngine::new(&store)
        .savings(Some("findme"), 2000)
        .unwrap();
    let query = report.query.expect("a named query is accounted for");
    assert_eq!(query.query, "findme");
    assert!(query.hits > 0, "fixture symbol was not found");
    assert_eq!(query.files_named, 1);
    assert_eq!(
        query.files_bytes,
        tiny.len() as u64,
        "the named file's size is measured from disk"
    );
    assert_eq!(query.files_unreadable, 0);
    // No assertion that a saving exists: on a two-line file it does not, and a
    // test that required one would be pinning the metric to flatter itself.
    let _ = std::fs::remove_dir_all(&dir);
}
