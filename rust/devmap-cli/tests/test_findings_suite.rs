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
                evidence: None,
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
    let res1 = r1.resolve_all(&files).unwrap();
    let ana1 = analyze(&files, &res1);

    let mut r2 = Resolver::new();
    r2.index_extractions(&files);
    let res2 = r2.resolve_all(&files).unwrap();
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
    let resolution = resolver.resolve_all(&files).unwrap();
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
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext)).unwrap();
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
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext)).unwrap();
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

/// Rows one generation's write left behind, per relation, for a corpus of `n`
/// files where `src/f{i}.py` calls into `src/f{i+1}.py` and the second
/// generation edits the body of `src/f0.py` alone.
///
/// Physical rows, not rows a generation *reads*: since v28 nothing but the two
/// per-generation relations is keyed by `generation_id`, and a count taken
/// through a compatibility view is (row x generations), which says nothing
/// about what the write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct B3WriteSet {
    /// `payload_nodes` and `nodes_fts` rows the edit added: the edited file's
    /// new payload holds its symbols, and the index one entry for each.
    nodes: i64,
    fts: i64,
    /// `payload_literals` rows added.
    literals: i64,
    /// Membership ranges the edit opened (`valid_from = 2`) and closed
    /// (`valid_to = 2`).
    file_rows_opened: i64,
    file_rows_closed: i64,
    /// Digest ranges, likewise.
    digests_opened: i64,
    digests_closed: i64,
    /// The two relations still written per generation, in full.
    dead: i64,
    gaps: i64,
    /// `file_payloads` the edit added: content-addressed, so the unchanged
    /// files reuse generation 1's payload and only the edited file adds one.
    payloads_added: i64,
    /// Range rows the edit opened (`valid_from = 2`) or closed (`valid_to = 2`).
    edges_opened: i64,
    edges_closed: i64,
    unresolved_opened: i64,
    unresolved_closed: i64,
    /// Generation 1's edge count, so a fixture with no edges cannot pass the
    /// "the range tables are differential" half vacuously.
    gen1_edges: i64,
    members: usize,
}

impl B3WriteSet {
    /// Every row the edit wrote, over every relation: B3's "<100 rows" gate.
    fn written(&self) -> i64 {
        self.payloads_added
            + self.nodes
            + self.fts
            + self.literals
            + self.file_rows_opened
            + self.file_rows_closed
            + self.digests_opened
            + self.digests_closed
            + self.edges_opened
            + self.edges_closed
            + self.unresolved_opened
            + self.unresolved_closed
            + self.dead
            + self.gaps
    }
}

fn b3_measure_one_file_edit(n: usize) -> B3WriteSet {
    let source = |i: usize, next: usize| {
        format!("from src.f{next} import fn_{next}\n\ndef fn_{i}():\n    return fn_{next}()\n")
    };
    let files: Vec<_> = (0..n)
        .map(|i| extract_file(&format!("src/f{i}.py"), &source(i, (i + 1) % n)))
        .collect();
    let dir = std::env::temp_dir().join(format!("devmap-b3-gate-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("devmap.sqlite");

    let resolve = |files: &[Extraction]| {
        let mut resolver = Resolver::new();
        resolver.index_extractions(files);
        let resolution = resolver.resolve_all(files).unwrap();
        let analysis = analyze(files, &resolution);
        (resolution, analysis)
    };
    // Every count is taken through a connection opened after the `Store` is
    // dropped: a second connection on a file a live `Store` holds would release
    // that store's POSIX locks when it closed.
    let count = |sql: &str| -> i64 {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    };

    {
        let (resolution, analysis) = resolve(&files);
        let store = Store::open(&db).unwrap();
        assert_eq!(
            store
                .save_generation(&files, &resolution, &analysis)
                .unwrap(),
            1
        );
    }
    let before = |sql: &str| count(sql);
    let payloads_before = before("SELECT COUNT(*) FROM file_payloads");
    let nodes_before = before("SELECT COUNT(*) FROM payload_nodes");
    let fts_before = before("SELECT COUNT(*) FROM nodes_fts");
    let literals_before = before("SELECT COUNT(*) FROM payload_literals");
    let gen1_edges = count("SELECT COUNT(*) FROM edge_rows WHERE valid_from = 1");

    let mut edited = files.clone();
    // Retarget the call, so the edit has edges to close and to open.
    edited[0] = extract_file("src/f0.py", &source(0, 2));
    let members = {
        let (resolution, analysis) = resolve(&edited);
        let store = Store::open(&db).unwrap();
        let generation = store
            .save_generation_with_opts(
                &edited,
                &resolution,
                &analysis,
                GenerationWriteOpts {
                    affected_paths: vec!["src/f0.py".into()],
                    deleted_paths: vec![],
                    build_started: None,
                    repo_root: None,
                    discovery_refusals: None,
                    over_limit: None,
                    verify_every_row: false,
                },
            )
            .unwrap();
        assert_eq!(generation, 2);
        store.list_generation_paths(2).unwrap().len()
    };

    let at_two = |table: &str| {
        count(&format!(
            "SELECT COUNT(*) FROM {table} WHERE generation_id = 2"
        ))
    };
    let set = B3WriteSet {
        nodes: count("SELECT COUNT(*) FROM payload_nodes") - nodes_before,
        fts: count("SELECT COUNT(*) FROM nodes_fts") - fts_before,
        literals: count("SELECT COUNT(*) FROM payload_literals") - literals_before,
        file_rows_opened: count("SELECT COUNT(*) FROM file_rows WHERE valid_from = 2"),
        file_rows_closed: count("SELECT COUNT(*) FROM file_rows WHERE valid_to = 2"),
        digests_opened: count("SELECT COUNT(*) FROM digest_rows WHERE valid_from = 2"),
        digests_closed: count("SELECT COUNT(*) FROM digest_rows WHERE valid_to = 2"),
        dead: at_two("generation_dead_symbols"),
        gaps: at_two("generation_coverage_gaps"),
        payloads_added: count("SELECT COUNT(*) FROM file_payloads") - payloads_before,
        edges_opened: count("SELECT COUNT(*) FROM edge_rows WHERE valid_from = 2"),
        edges_closed: count("SELECT COUNT(*) FROM edge_rows WHERE valid_to = 2"),
        unresolved_opened: count("SELECT COUNT(*) FROM unresolved_rows WHERE valid_from = 2"),
        unresolved_closed: count("SELECT COUNT(*) FROM unresolved_rows WHERE valid_to = 2"),
        gen1_edges,
        members,
    };
    let _ = std::fs::remove_dir_all(&dir);
    set
}

/// B3's gate -- "a one-file edit writes <100 rows for that generation" -- is
/// met, and this test is what pins it.
///
/// It measures the same one-file edit at two corpus sizes and asserts the rows
/// the write left behind, physical and per relation, so that what it writes
/// cannot come to grow with the repository unnoticed:
///
/// - the range relations (`edge_rows`, `unresolved_rows` since v18, and since
///   v28 `file_rows` and `digest_rows`) open and close only what the edit
///   changed;
/// - the payload-keyed relations (`file_payloads` since v17, and since v28
///   `payload_nodes`, `payload_literals` and the `nodes_fts` entries) gain one
///   payload's rows;
/// - `generation_dead_symbols` and `generation_coverage_gaps` are still
///   written per generation, in full. They are whole-graph results rather than
///   derivations of one file; the fixture has one dead symbol and no gaps, so
///   they cost a row here, and the number is pinned so that changing it is a
///   decision.
///
/// Until v28 this asserted `6n + 1` rows copied for every file and symbol of
/// the repository, and the gate was re-scoped in `docs/devmap/DIVERGENCES.md`
/// (B3) with those numbers rather than claimed.
#[test]
fn b3_one_file_edit_write_set_is_differential_for_every_relation_but_the_two_per_generation_ones() {
    let small = b3_measure_one_file_edit(200);
    let large = b3_measure_one_file_edit(500);
    eprintln!("B3 write set, n=200: {small:?}");
    eprintln!("B3 write set, n=500: {large:?}");

    for (n, set) in [(200i64, small), (500i64, large)] {
        // Membership stays complete: an incremental generation is the whole tree.
        assert_eq!(set.members as i64, n);
        assert!(
            set.gen1_edges >= n,
            "fixture must carry its cross-file calls: {set:?}"
        );
        // One payload for the edited file, and the two edges the retargeted
        // call closed and opened (the call and its import).
        assert_eq!(set.payloads_added, 1, "{set:?}");
        assert_eq!((set.edges_closed, set.edges_opened), (2, 2), "{set:?}");
        // The edited file's symbols (the File node and the function), once in
        // the payload table and once in the index; no literals in the fixture.
        assert_eq!((set.nodes, set.fts, set.literals), (2, 2, 0), "{set:?}");
        // One membership range closed and one opened, and the one digest the
        // retargeted call changed -- the file's own.
        assert_eq!(
            (set.file_rows_opened, set.file_rows_closed),
            (1, 1),
            "{set:?}"
        );
        assert_eq!((set.digests_opened, set.digests_closed), (1, 1), "{set:?}");
        assert_eq!((set.dead, set.gaps), (1, 0), "{set:?}");
        // The gate, over every relation at once, and independent of `n`.
        assert!(set.written() < 100, "{set:?}");
    }
    assert_eq!(
        small.written(),
        large.written(),
        "what a one-file edit writes must not depend on the size of the repository"
    );
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
    let resolution = resolver.resolve_all(&files).unwrap();
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
        error_ranges: vec![],
        nameless_ranges: vec![],
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
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext)).unwrap();
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
