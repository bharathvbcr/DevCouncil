//! Named finding-ID regression suite (append-only 117-property coverage).

use devmap_analyze::traversal::{traverse_graph, TraversalOptions};
use devmap_analyze::*;
use devmap_extract::cache::{
    cache_admits, grammar_version_for, CacheKey, ANALYZER_VERSION, EXTRACTION_SCHEMA_VERSION,
};
use devmap_extract::*;
use devmap_query::*;
use devmap_resolve::*;
use devmap_store::*;
use std::collections::BTreeMap;
use std::time::Instant;

#[test]
fn test_g8_parametric_depth_limits_traversal() {
    // closes G8
    let resolution = ResolutionResult {
        edges: [("a", "b"), ("b", "c"), ("c", "d")]
            .into_iter()
            .map(|(source, target)| ResolvedEdge {
                source_file: format!("{source}.py"),
                target_file: format!("{target}.py"),
                source_symbol: source.to_string(),
                target_symbol: target.to_string(),
                edge_kind: EdgeKind::Calls,
                confidence: Confidence::DETERMINISTIC,
                resolution: None,
                details: None,
            })
            .collect(),
        receiver_types: BTreeMap::new(),
        reexport_chains: BTreeMap::new(),
        unresolved: Vec::new(),
    };
    let start = vec!["a".to_string()];
    let shallow = traverse_graph(
        &start,
        &resolution.edges,
        &TraversalOptions {
            max_depth: 1,
            max_nodes: 100,
            reverse: false,
        },
    );
    let deep = traverse_graph(
        &start,
        &resolution.edges,
        &TraversalOptions {
            max_depth: 10,
            max_nodes: 100,
            reverse: false,
        },
    );
    assert_eq!(
        shallow.visited_nodes,
        ["a".to_string(), "b".to_string()].into()
    );
    assert!(!shallow.visited_nodes.contains("d"));
    assert!(deep.visited_nodes.contains("d"));
    assert!(deep.visited_nodes.len() > shallow.visited_nodes.len());
}

#[test]
fn test_r4_g26_deterministic_double_build() {
    // closes R4, G4, G26
    let files = [
        extract_file("z.py", "def z(): pass\n"),
        extract_file("a.py", "def a(): pass\n"),
    ];
    let mut r1 = Resolver::new();
    r1.index_extractions(&files);
    let res1 = r1.resolve_all(&files);
    let ana1 = analyze(&files, &res1);

    let mut r2 = Resolver::new();
    r2.index_extractions(&files);
    let res2 = r2.resolve_all(&files);
    let ana2 = analyze(&files, &res2);

    let j1 = serde_json::to_string(&ana1.communities).unwrap();
    let j2 = serde_json::to_string(&ana2.communities).unwrap();
    assert_eq!(j1, j2);
}

#[test]
fn test_t1_manifest_within_budget() {
    // closes T1
    let mut files = Vec::new();
    for i in 0..40 {
        files.push(extract_file(&format!("pkg/m{i}.py"), "def f(): pass\n"));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files);
    let analysis = analyze(&files, &resolution);
    let (_, json) = generate_manifest(
        &files,
        &analysis,
        FreshnessInfo {
            head_sha: "test-head".into(),
            generation_id: 1,
            pending_count: 0,
            stamped: Default::default(),
        },
    );
    assert!(json.len() < 8000, "manifest JSON should stay compact");
}

#[test]
fn test_v12_dead_honest_counts() {
    // closes V12
    let mut src = String::new();
    for i in 0..30 {
        src.push_str(&format!("def dead_{i}(): pass\n"));
    }
    let ext = extract_file("dead.py", &src);
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    let dead: Vec<_> = analysis
        .dead_symbols
        .into_iter()
        .filter(|d| !d.is_exempt)
        .collect();
    let payload = budget_take(dead, 50, |_| 20);
    assert_eq!(payload.shown + payload.hidden, payload.total);
    assert_eq!(payload.hidden, payload.total - payload.shown);
    if payload.truncated {
        assert!(payload.shown < payload.total);
    }
}

#[test]
fn test_x14_tri_state_unavailable_deps() {
    // closes X14 — a missing target is unknown, never verified-zero.
    let resolution = ResolutionResult {
        edges: vec![],
        receiver_types: BTreeMap::new(),
        reexport_chains: BTreeMap::new(),
        unresolved: Vec::new(),
    };
    let engine = QueryEngine::new(&[], &resolution);
    let resp = engine.dependencies(Request {
        query: "missing.py".into(),
        token_budget: 2000,
        min_confidence: 0.0,
        max_depth: 1,
    });
    assert_eq!(resp.shown, 0);
    assert_eq!(resp.total, 0);
    assert!(!resp.truncated);
    assert!(matches!(
        resp.resolution,
        ResolutionAvailability::Unavailable { .. }
    ));
}

#[test]
fn test_s3_fts_fuzz_corpus() {
    // closes S3 — expanded adversarial FTS corpus
    let ext = extract_file("fts.py", "def alpha_beta(): pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    for query in [
        "alpha\"beta",
        "alpha-beta",
        "name:alpha",
        "alpha OR beta",
        "(alpha)",
        "alpha*",
        "\"alpha\"",
        "alpha\\beta",
        "column:alpha",
        "--alpha",
        "alpha;drop",
    ] {
        let result = store.search_fts(query, 10);
        assert!(
            result.is_ok(),
            "FTS fuzz query {:?} failed: {:?}",
            query,
            result
        );
    }
}

#[test]
fn differential_membership_preserves_full_generation_while_b3_write_amplification_is_open() {
    // Membership remains correct. This does not prove B3's <100-row gate;
    // the current carry-forward schema still writes O(repository size) rows.
    let n = 500usize;
    let mut files = Vec::new();
    for i in 0..n {
        files.push(extract_file(
            &format!("src/f{i}.py"),
            &format!("def fn_{i}(): pass\n"),
        ));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files);
    let analysis = analyze(&files, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&files, &resolution, &analysis)
        .unwrap();

    let mut edited = files.clone();
    edited[0] = extract_file("src/f0.py", "def fn_0(): return 1\n");
    let mut resolver2 = Resolver::new();
    resolver2.index_extractions(&edited);
    let resolution2 = resolver2.resolve_all(&edited);
    let analysis2 = analyze(&edited, &resolution2);
    let gen2 = store
        .save_generation_with_opts(
            &edited,
            &resolution2,
            &analysis2,
            GenerationWriteOpts {
                affected_paths: vec!["src/f0.py".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
                discovery_refusals: None,
            },
        )
        .unwrap();
    assert_eq!(gen2, 2);
    let paths = store.list_generation_paths(2).unwrap();
    assert_eq!(paths.len(), n);
}

#[test]
fn test_explore_p95_soft_ratchet_2k() {
    // soft gate: explore/search p95 at 2k files
    let n = 2000usize;
    let mut files = Vec::new();
    for i in 0..n {
        files.push(extract_file(
            &format!("pkg/m{i}.py"),
            &format!("def sym_{i}(): pass\n"),
        ));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files);
    let engine = QueryEngine::new(&files, &resolution);

    let mut latencies = Vec::new();
    for sample in [0, n / 4, n / 2, n - 1] {
        let start = Instant::now();
        let _ = engine.search(Request {
            query: format!("sym_{sample}"),
            token_budget: 2000,
            min_confidence: 0.0,
            max_depth: 1,
        });
        latencies.push(start.elapsed().as_millis());
    }
    let p95 = *latencies.iter().max().unwrap_or(&0);
    assert!(p95 < 500, "explore p95 too slow at 2k: {p95}ms");
}

#[test]
fn test_s14_cache_key_uses_real_grammar_version() {
    // closes S14
    let ext = extract_file("mod.py", "def x(): pass\n");
    let key = CacheKey::for_extraction(&ext);
    assert!(key.grammar_version.starts_with("tree-sitter-python@"));
    assert!(key.grammar_version.contains(":python:abi"));
    assert_eq!(
        key.analyzer_version,
        format!("{ANALYZER_VERSION}:extract-v{EXTRACTION_SCHEMA_VERSION}")
    );
}

#[test]
fn test_x7_cache_admits_only_clean_partial() {
    // closes X7
    assert!(!cache_admits(&ParseOutcome::Failed { reason: "x".into() }));
    assert!(cache_admits(&ParseOutcome::Clean));
    assert!(cache_admits(&ParseOutcome::Partial {
        error_ranges: vec![]
    }));
}

#[test]
fn extraction_cache_round_trip() {
    let store = Store::open_in_memory().unwrap();
    let ext = extract_file("shard.py", "def shard(): pass\n");
    let key = CacheKey::for_extraction(&ext);
    store.admit_cached_extraction(&key, &ext).unwrap();
    let cached = store.try_get_cached_extraction(&key).unwrap();
    assert!(cached.is_some());
    assert_eq!(cached.unwrap().symbols.len(), ext.symbols.len());
}

#[test]
fn lexical_search_operates_while_rrf_remains_open() {
    let ext = extract_file("search.py", "def find_me(): pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let engine = QueryEngine::new(std::slice::from_ref(&ext), &resolution);
    let resp = engine.search(Request {
        query: "find_me".into(),
        token_budget: 2000,
        min_confidence: 0.0,
        max_depth: 1,
    });
    assert!(resp.shown >= 1);
}

#[test]
fn test_language_extractor_routing_all_specs() {
    // closes X8 — every LanguageSpec extension routes
    use devmap_extract::languages::{find_spec_by_extension, LANGUAGE_SPECS};
    for spec in LANGUAGE_SPECS {
        for ext in spec.extensions {
            let found = find_spec_by_extension(ext);
            assert!(found.is_some(), "extension {ext} missing from registry");
            assert_eq!(found.unwrap().extractor_id, spec.extractor_id);
        }
    }
}

#[test]
fn test_grammar_versions_differ_by_language() {
    let py = grammar_version_for("python");
    let rs = grammar_version_for("rust");
    assert_ne!(py, rs);
}
