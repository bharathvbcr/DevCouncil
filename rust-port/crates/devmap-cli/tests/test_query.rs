use devmap_analyze::model::*;
use devmap_extract::*;
use devmap_query::*;
use devmap_resolve::*;

fn freshness() -> FreshnessInfo {
    FreshnessInfo {
        head_sha: "test-head".into(),
        generation_id: 1,
        pending_count: 0,
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
        total_files: 3,
        total_symbols: 1,
        total_edges: 0,
        dead_symbols: vec![],
        communities: vec![],
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
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
        total_files: 0,
        total_symbols: 0,
        total_edges: 0,
        dead_symbols: vec![],
        communities: vec![],
        status: AnalysisStatus::Ok,
        unresolved_calls: 0,
    };
    let (manifest, _) = generate_manifest(
        &[],
        &analysis,
        FreshnessInfo {
            head_sha: "0123456789abcdef".into(),
            generation_id: 42,
            pending_count: 7,
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
