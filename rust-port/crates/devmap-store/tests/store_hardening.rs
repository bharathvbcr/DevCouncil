use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use devmap_analyze::{analyze, DeadSymbolReport};
use devmap_extract::cache::CacheKey;
use devmap_extract::extract_all;
use devmap_extract::{extract_file, extract_tree, FileRef, SymbolKind};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store, WalCheckpointMode, CURRENT_SCHEMA_VERSION};

/// A 1 us `SystemTime` tick is not a unique key: same-microsecond callers used
/// to collide on one path, and the `remove_dir_all` below then deleted a live
/// sibling test's fixture. pid plus a monotonic counter make the name unique.
fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pending_queue_survives_reopen() {
    let dir = tmp_dir("pending");
    let db = dir.join("index.sqlite");
    {
        let store = Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&["a.py".into(), "b.py".into()])
            .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let pending = store.get_pending_paths().unwrap();
    assert_eq!(pending.len(), 2);
    store.clear_pending_paths(&["a.py".into()]).unwrap();
    assert_eq!(store.get_pending_paths().unwrap(), vec!["b.py".to_string()]);
    let _ = fs::remove_dir_all(&dir);
}

/// A watcher event arriving mid-drain is newer work and must survive the
/// acknowledgement of the batch it interrupted.
///
/// The guard used to be `attempts > 0`, which forced the drain to charge an
/// attempt to every path it was about to succeed at just to make the delete
/// fire — and that accounting is what let one store-level failure quarantine a
/// whole batch (K1(d)). The guard is now the `queued_at` the row was claimed
/// at, which re-enqueueing moves. Same invariant, without the side effect.
#[test]
fn requeued_path_survives_post_attempt_acknowledgement() {
    let store = Store::open_in_memory().unwrap();
    store.enqueue_pending_paths(&["a.py".to_string()]).unwrap();
    let claimed = store.claim_pending_batch(16).unwrap();
    assert_eq!(claimed.len(), 1);

    // A watcher event arriving during the rebuild represents newer work and
    // moves the queue timestamp. Acknowledging the old claim must preserve it.
    std::thread::sleep(std::time::Duration::from_millis(5));
    store.enqueue_pending_paths(&["a.py".to_string()]).unwrap();
    assert_eq!(store.clear_claimed_pending_paths(&claimed).unwrap(), 0);
    assert_eq!(store.get_pending_paths().unwrap(), vec!["a.py".to_string()]);
    assert_eq!(
        store.pending_attempts("a.py").unwrap(),
        Some(0),
        "claiming and acknowledging must not charge a failed attempt"
    );
}

#[test]
fn test_s7_quarantined_poison_path_stops_hot_loop_until_a_new_event() {
    let store = Store::open_in_memory().unwrap();
    let path = "poison.py".to_string();
    store
        .enqueue_pending_paths(std::slice::from_ref(&path))
        .unwrap();
    for _ in 0..5 {
        store
            .bump_pending_attempts(std::slice::from_ref(&path))
            .unwrap();
    }

    assert!(
        store.get_pending_paths().unwrap().is_empty(),
        "a quarantined path must not be selected for another immediate retry"
    );
    let status = store.status(":memory:").unwrap();
    assert_eq!(status.pending_count, 1);
    assert_eq!(status.quarantined_count, 1);

    // A new filesystem event is new evidence and resets the retry state.
    store
        .enqueue_pending_paths(std::slice::from_ref(&path))
        .unwrap();
    assert_eq!(store.get_pending_paths().unwrap(), [path]);
}

#[test]
fn deletion_reconciliation_removes_live_nodes() {
    let a = extract_file("a.py", "def keep():\n    return 1\n");
    let b = extract_file("b.py", "def gone():\n    return 2\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[a.clone(), b.clone()]);
    let resolution = resolver.resolve_all(&[a.clone(), b.clone()]);
    let analysis = analyze(&[a.clone(), b.clone()], &resolution);

    let store = Store::open_in_memory().unwrap();
    let g1 = store
        .save_generation(&[a.clone(), b.clone()], &resolution, &analysis)
        .unwrap();
    assert_eq!(g1, 1);
    let paths = store.list_generation_paths(1).unwrap();
    assert!(paths.contains(&"a.py".into()));
    assert!(paths.contains(&"b.py".into()));

    // Rebuild with b.py deleted and only a.py present.
    let mut resolver2 = Resolver::new();
    resolver2.index_extractions(std::slice::from_ref(&a));
    let resolution2 = resolver2.resolve_all(std::slice::from_ref(&a));
    let analysis2 = analyze(std::slice::from_ref(&a), &resolution2);
    let g2 = store
        .save_generation_with_opts(
            std::slice::from_ref(&a),
            &resolution2,
            &analysis2,
            GenerationWriteOpts {
                affected_paths: vec![],
                deleted_paths: vec!["b.py".into()],
                build_started: None,
                repo_root: None,
            },
        )
        .unwrap();
    assert_eq!(g2, 2);
    let paths2 = store.list_generation_paths(2).unwrap();
    assert!(paths2.contains(&"a.py".into()));
    assert!(
        !paths2.contains(&"b.py".into()),
        "deleted file must not remain as live nodes: {:?}",
        paths2
    );
}

#[test]
fn fts_rowids_do_not_collide_across_generations() {
    let a = extract_file("a.py", "def alpha():\n    return 1\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&a));
    let resolution = resolver.resolve_all(std::slice::from_ref(&a));
    let analysis = analyze(std::slice::from_ref(&a), &resolution);
    let store = Store::open_in_memory().unwrap();
    let g1 = store
        .save_generation(std::slice::from_ref(&a), &resolution, &analysis)
        .unwrap();
    let a2 = extract_file("a.py", "def alpha():\n    return 2\n");
    let mut resolver2 = Resolver::new();
    resolver2.index_extractions(std::slice::from_ref(&a2));
    let resolution2 = resolver2.resolve_all(std::slice::from_ref(&a2));
    let analysis2 = analyze(std::slice::from_ref(&a2), &resolution2);
    let g2 = store
        .save_generation(&[a2], &resolution2, &analysis2)
        .unwrap();
    assert_ne!(g1, g2);
    let hits = store.search_fts("alpha", 10).unwrap();
    assert!(!hits.is_empty());
}

#[test]
fn test_s18_busy_truncate_checkpoint_falls_back_to_passive() {
    let dir = tmp_dir("wal-checkpoint");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();

    let first = extract_file("a.py", "def alpha():\n    return 1\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&first));
    let resolution = resolver.resolve_all(std::slice::from_ref(&first));
    let analysis = analyze(std::slice::from_ref(&first), &resolution);
    store
        .save_generation(std::slice::from_ref(&first), &resolution, &analysis)
        .unwrap();

    let reader = rusqlite::Connection::open(&db).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let _: i64 = reader
        .query_row("SELECT COUNT(*) FROM generations", [], |row| row.get(0))
        .unwrap();

    let second = extract_file("a.py", "def alpha():\n    return 2\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&second));
    let resolution = resolver.resolve_all(std::slice::from_ref(&second));
    let analysis = analyze(std::slice::from_ref(&second), &resolution);
    store
        .save_generation(std::slice::from_ref(&second), &resolution, &analysis)
        .unwrap();

    let started = Instant::now();
    let checkpoint = store.checkpoint_wal().unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "checkpoint fallback must not hold the store lock for SQLite's busy timeout: {:?}",
        started.elapsed()
    );
    assert_eq!(checkpoint.mode, WalCheckpointMode::Passive);
    assert!(
        checkpoint.log_frames >= checkpoint.checkpointed_frames,
        "checkpoint counts must be internally consistent: {checkpoint:?}"
    );

    reader.execute_batch("ROLLBACK").unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn fts_hostile_queries_are_syntax_safe() {
    let ext = extract_file("src/fts.py", "def alpha_beta(): pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    for query in [
        "alpha\"",
        "alpha-beta",
        "name:alpha",
        "alpha OR beta",
        "(",
        "\\",
    ] {
        let result = store.search_fts(query, 10);
        assert!(
            result.is_ok(),
            "FTS query {:?} returned {:?}",
            query,
            result
        );
    }
}

#[test]
fn prune_zero_retains_latest_generation_and_search_index() {
    let store = Store::open_in_memory().unwrap();
    for version in 0..3 {
        let source = format!("def alpha():\n    return {version}\n");
        let ext = extract_file("src/alpha.py", &source);
        let mut resolver = Resolver::new();
        resolver.index_extractions(std::slice::from_ref(&ext));
        let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
        let analysis = analyze(std::slice::from_ref(&ext), &resolution);
        assert_eq!(
            store
                .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
                .unwrap(),
            version + 1
        );
    }

    assert_eq!(store.prune_generations_except_latest(0).unwrap(), 2);
    assert_eq!(store.latest_generation_id().unwrap(), Some(3));
    assert_eq!(
        store.list_generation_paths(3).unwrap(),
        vec!["src/alpha.py".to_string()]
    );
    assert!(
        !store.search_fts("alpha", 10).unwrap().is_empty(),
        "pruning must not remove the latest generation's FTS rows"
    );
}

#[test]
fn stress_extract_and_store_thousands_of_files() {
    let n = 2_000usize;
    let mut sources: Vec<(String, String)> = Vec::with_capacity(n);
    for i in 0..n {
        let path = format!("pkg/f{i}.py");
        let src = format!("def fn_{i}():\n    return {i}\n");
        sources.push((path, src));
    }
    let refs: Vec<FileRef> = sources
        .iter()
        .map(|(p, s)| FileRef {
            path: p.as_str(),
            source: s.as_str(),
        })
        .collect();

    let t0 = Instant::now();
    let extractions = extract_all(&refs);
    let extract_ms = t0.elapsed().as_millis();
    assert_eq!(extractions.len(), n);

    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);

    let t1 = Instant::now();
    let store = Store::open_in_memory().unwrap();
    let gen = store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    let store_ms = t1.elapsed().as_millis();
    assert_eq!(gen, 1);
    let status = store.status(":memory:").unwrap();
    assert!(status.node_count >= n);

    eprintln!(
        "stress n={n} extract_ms={extract_ms} store_ms={store_ms} nodes={}",
        status.node_count
    );
    // Soft ratchet: 2k tiny files should finish well under a minute on CI hardware.
    assert!(
        extract_ms + store_ms < 60_000,
        "stress too slow: extract_ms={extract_ms} store_ms={store_ms}"
    );
}

#[test]
fn phase5_persisted_search_p95_is_under_50ms_at_10k_files() {
    let n = 10_000usize;
    let mut sources = Vec::with_capacity(n);
    for index in 0..n {
        sources.push((
            format!("pkg/f{index}.py"),
            format!("def searchable_{index}():\n    return {index}\n"),
        ));
    }
    let refs: Vec<FileRef> = sources
        .iter()
        .map(|(path, source)| FileRef { path, source })
        .collect();
    let extractions = extract_all(&refs);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let mut latencies = Vec::new();
    for sample in (0..n).step_by(500) {
        let start = Instant::now();
        let hits = store
            .search_symbols(&format!("searchable_{sample}"), 10)
            .unwrap();
        assert!(hits
            .iter()
            .any(|hit| hit.name == format!("searchable_{sample}")));
        latencies.push(start.elapsed());
    }
    latencies.sort();
    let p95_index = ((latencies.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(latencies.len() - 1);
    let p95 = latencies[p95_index];
    eprintln!("persisted search 10k p95={p95:?}");
    assert!(
        p95 < std::time::Duration::from_millis(50),
        "persisted search p95 exceeded 50ms at 10k files: {p95:?}"
    );
}

#[test]
fn extract_tree_fixture_corpus() {
    // Prefer in-repo testdata when present; otherwise synthesize.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    if root.is_dir() {
        let exts = extract_tree(&root).unwrap();
        assert!(!exts.is_empty());
        assert!(exts.iter().any(|e| e.language == "python"));
    }
}

#[test]
fn test_x7_failed_parse_never_admitted_to_cache() {
    // closes X7
    use devmap_extract::cache::{CacheKey, ANALYZER_VERSION};
    use devmap_extract::model::ParseOutcome;

    let mut ext = extract_file("broken.py", "def ((( invalid");
    ext.parse_outcome = ParseOutcome::Failed {
        reason: "syntax fatal".to_string(),
    };
    let key = CacheKey {
        content_hash: ext.content_hash,
        language: ext.language.clone(),
        grammar_version: devmap_extract::cache::grammar_version_for(&ext.language),
        analyzer_version: ANALYZER_VERSION.to_string(),
    };
    let store = Store::open_in_memory().unwrap();
    store.admit_cached_extraction(&key, &ext).unwrap();
    assert!(store.try_get_cached_extraction(&key).unwrap().is_none());
    assert_eq!(store.extraction_retry_count(ext.content_hash).unwrap(), 1);
}

#[test]
fn test_s2_migration_v3_to_v4_preserves_cache_rows() {
    // closes S2
    let dir = tmp_dir("migration");
    let db_path = dir.join("legacy.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE extraction_cache (
                content_hash INTEGER PRIMARY KEY,
                payload_json TEXT NOT NULL,
                accessed_at REAL NOT NULL
            );",
        )
        .unwrap();
        conn.execute("PRAGMA user_version = 3", []).unwrap();
        conn.execute(
            "INSERT INTO extraction_cache (content_hash, payload_json, accessed_at) VALUES (42, '{\"file_path\":\"legacy.py\",\"language\":\"python\",\"content_hash\":42,\"parse_outcome\":\"Clean\",\"symbols\":[],\"imports\":[],\"calls\":[],\"exports\":[],\"references\":[],\"routes\":[],\"wiring\":[]}', 1.0)",
            [],
        )
        .unwrap();
    }
    let store = Store::open(&db_path).unwrap();
    let key = CacheKey {
        content_hash: 42,
        language: "unknown".to_string(),
        grammar_version: "legacy".to_string(),
        analyzer_version: "legacy".to_string(),
    };
    let cached = store.try_get_cached_extraction(&key).unwrap();
    assert!(cached.is_some(), "legacy cache row must survive migration");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_s2_migration_v4_to_v5_preserves_generations_and_adds_analysis() {
    let dir = tmp_dir("migration-v5");
    let db_path = dir.join("legacy-v4.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE generations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at REAL NOT NULL,
                head_sha TEXT
            );
            INSERT INTO generations (created_at, head_sha) VALUES (1.0, 'legacy-head');
            PRAGMA user_version = 4;",
        )
        .unwrap();
    }
    {
        let store = Store::open(&db_path).unwrap();
        assert_eq!(store.latest_generation_id().unwrap(), Some(1));
        let analysis = store.latest_analysis().unwrap().unwrap();
        assert_eq!(analysis.total_files, 0);
        assert!(analysis.dead_symbols.is_empty());
    }
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_s2_migration_v5_to_v6_preserves_graph_and_adds_history() {
    let dir = tmp_dir("migration-v6");
    let db_path = dir.join("legacy-v5.sqlite");
    let ext = extract_file("legacy.py", "def legacy():\n    return 1\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    {
        let store = Store::open(&db_path).unwrap();
        store
            .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
            .unwrap();
    }
    {
        // Reconstruct the exact v5 shape from a current database: v6 added
        // only build_history, so removing it and stamping 5 is lossless.
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "DROP TABLE build_history;
             PRAGMA user_version = 5;",
        )
        .unwrap();
    }

    let store = Store::open(&db_path).unwrap();
    assert_eq!(store.latest_generation_id().unwrap(), Some(1));
    assert_eq!(store.latest_extractions().unwrap().len(), 1);
    assert!(store.build_history(10).unwrap().is_empty());
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();
    let history = store.build_history(10).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].generation_id, 2);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_s2_initial_schema_failure_rolls_back_every_prior_statement() {
    let dir = tmp_dir("initial-schema-rollback");
    let db_path = dir.join("conflict.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE VIEW generations AS SELECT 1 AS id;")
            .unwrap();
    }

    assert!(
        Store::open(&db_path).is_err(),
        "a conflicting schema object must fail initialization"
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0, "failed initialization must not bump the schema");
    let paths_created: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'paths'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        paths_created, 0,
        "statements before the injected conflict must roll back atomically"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn future_schema_version_is_rejected_fail_closed() {
    let dir = tmp_dir("future-schema");
    let db_path = dir.join("future.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 99", []).unwrap();
    }

    assert!(
        Store::open(&db_path).is_err(),
        "a newer schema must not be opened with an older binary"
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let created_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table', 'index', 'trigger') AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        created_tables, 0,
        "rejecting a future schema must not mutate it before failing"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_b4_reader_unblocked_during_generation_write() {
    let dir = tmp_dir("wal-reader-writer");
    let db = dir.join("index.sqlite");
    let store = Store::open(&db).unwrap();
    let ext = extract_file("a.py", "def alpha(): pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);

    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    // Separate handles are required: sharing Store's Mutex would serialize
    // the test before SQLite's WAL behavior was exercised.
    let writer = rusqlite::Connection::open(&db).unwrap();
    let reader = rusqlite::Connection::open(&db).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer
        .execute("UPDATE generations SET head_sha = 'uncommitted'", [])
        .unwrap();

    let started = Instant::now();
    let visible: String = reader
        .query_row(
            "SELECT head_sha FROM generations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_millis(100),
        "WAL reader blocked behind a separate writer: {:?}",
        started.elapsed()
    );
    assert_ne!(
        visible, "uncommitted",
        "reader must see the committed snapshot"
    );
    writer.execute_batch("ROLLBACK").unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn persisted_generation_answers_queries_without_source_tree() {
    let target = extract_file("src/target.py", "def target():\n    return 1\n");
    let caller = extract_file(
        "src/caller.py",
        "from target import target\n\ndef caller():\n    return target()\n",
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(&[target.clone(), caller.clone()]);
    let resolution = resolver.resolve_all(&[target.clone(), caller.clone()]);
    let analysis = analyze(&[target.clone(), caller.clone()], &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&[target, caller], &resolution, &analysis)
        .unwrap();

    let symbols = store.search_symbols("target", 10).unwrap();
    assert!(symbols.iter().any(|row| row.path == "src/target.py"));
    let dependencies = store.latest_edges_for_file("src/caller.py", 0.0).unwrap();
    assert!(dependencies.iter().any(|edge| {
        edge.source_file == "src/caller.py" && edge.target_file == "src/target.py"
    }));
    assert!(store.latest_path_is_indexed("src/caller.py").unwrap());
}

#[test]
fn analysis_rows_are_generation_scoped_and_persisted() {
    let ext = extract_file("src/dead.py", "def abandoned():\n    return 1\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    assert!(analysis
        .dead_symbols
        .iter()
        .any(|row| row.symbol_name == "abandoned"));

    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();
    let persisted = store.latest_dead_symbols().unwrap();
    assert!(persisted
        .iter()
        .any(|row| row.symbol_name == "abandoned" && row.file_path == "src/dead.py"));
    let persisted_summary = store.latest_analysis().unwrap().unwrap();
    assert!(persisted_summary
        .dead_symbols
        .iter()
        .any(|row| row.symbol_name == "abandoned" && row.file_path == "src/dead.py"));
}

#[test]
fn generation_metadata_preserves_the_supplied_head_identity() {
    let ext = extract_file("src/head.py", "def head(): pass\n");
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_metadata(
            std::slice::from_ref(&ext),
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
            "0123456789abcdef",
        )
        .unwrap();
    assert_eq!(
        store.latest_generation_head().unwrap().as_deref(),
        Some("0123456789abcdef")
    );
}

#[test]
fn durable_generation_does_not_duplicate_raw_source_text() {
    let secret_marker = "RAW_SOURCE_MUST_NOT_BE_PERSISTED";
    let ext = extract_file(
        "src/no_raw_source.py",
        &format!("def marker():\n    return {secret_marker:?}\n"),
    );
    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    let durable = store.latest_extractions().unwrap();
    assert_eq!(durable.len(), 1);
    assert!(durable[0].source_code.is_none());
    assert_eq!(durable[0].content_hash, ext.content_hash);
    assert!(
        durable[0].references.iter().all(|reference| {
            !matches!(
                reference.kind,
                devmap_extract::model::ReferenceKind::Call
                    | devmap_extract::model::ReferenceKind::Constructor
                    | devmap_extract::model::ReferenceKind::JsxTag
            )
        }),
        "Call-kind references are already in `calls`; persisting them doubles the blob: {:?}",
        durable[0].references
    );
}

#[test]
fn go_package_star_survives_reload_without_source_code() {
    let files = [
        extract_file("pkg/a.go", "package pkg\nfunc A() {}\n"),
        extract_file("pkg/b.go", "package pkg\nfunc B() {}\n"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&files);
    let resolution = resolver.resolve_all(&files);
    let analysis = analyze(&files, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&files, &resolution, &analysis)
        .unwrap();

    let durable = store.latest_extractions().unwrap();
    assert!(durable.iter().all(|ext| ext.source_code.is_none()));
    assert!(
        durable
            .iter()
            .all(|ext| ext.go_package.as_deref() == Some("pkg")),
        "package clause must survive durable reload: {:?}",
        durable
            .iter()
            .map(|ext| ext.go_package.clone())
            .collect::<Vec<_>>()
    );

    let mut reloaded = Resolver::new();
    reloaded.index_extractions(&durable);
    let again = reloaded.resolve_all(&durable);
    assert!(
        again.edges.iter().any(|edge| {
            edge.edge_kind == devmap_extract::model::EdgeKind::MemberOf
                && edge.target_file == "package:pkg/pkg"
        }),
        "G20 star must not depend on persisted source_code: {:?}",
        again.edges
    );
}

#[test]
fn durable_extraction_json_is_smaller_than_the_live_blob() {
    let ext = extract_file(
        "Modal.tsx",
        "export function Modal() {\n  const handleClose = () => { persist(); persist(); };\n  return <button onClick={handleClose}>x</button>;\n}\nfunction persist() {}\n",
    );
    let durable = ext.for_durable_store();
    let live = serde_json::to_string(&ext).unwrap();
    let stored = serde_json::to_string(&durable).unwrap();
    assert!(durable.source_code.is_none());
    assert!(
        stored.len() < live.len(),
        "durable JSON {} must be smaller than live {}",
        stored.len(),
        live.len()
    );
}

#[test]
fn build_history_separates_confident_ambiguous_and_unmeasured_values() {
    let clean = extract_file("src/live.py", "def live():\n    return 1\n");
    // A language declared in the registry but with no linked grammar, whose
    // content also yields nothing to tier-2 pattern recovery — so the engine
    // reports it unavailable rather than parsing *or* pattern-matching it.
    //
    // This was `.java` until Java was linked, at which point the fixture
    // silently stopped testing anything; it then held `Class Legacy` until the
    // regex fallback landed and started recovering that declaration, which is
    // the same failure a second time. Both are why the precondition below is
    // explicit: the fixture has to keep meaning "nothing was extracted", and
    // that now takes content no tier can read.
    let unavailable = extract_file("src/legacy.vb", "' just a comment\n\n");
    assert!(
        format!("{:?}", unavailable.engine).contains("Unavailable"),
        "fixture precondition: `.vb` must have no linked grammar and no \
         pattern-recoverable declarations, got {:?}",
        unavailable.engine
    );
    assert!(
        unavailable
            .symbols
            .iter()
            .all(|s| s.kind == SymbolKind::File),
        "fixture precondition: the unavailable file must declare nothing"
    );
    let extractions = vec![clean, unavailable];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let mut analysis = analyze(&extractions, &resolution);
    analysis.dead_symbols = vec![
        DeadSymbolReport {
            symbol_name: "confident".into(),
            file_path: "src/live.py".into(),
            confidence: 0.9,
            is_exempt: false,
            exemption_reason: None,
        },
        DeadSymbolReport {
            symbol_name: "ambiguous".into(),
            file_path: "src/live.py".into(),
            confidence: 0.4,
            is_exempt: false,
            exemption_reason: Some("only_ambiguous_callers".into()),
        },
        DeadSymbolReport {
            symbol_name: "exempt".into(),
            file_path: "src/live.py".into(),
            confidence: 0.3,
            is_exempt: true,
            exemption_reason: Some("exported".into()),
        },
    ];

    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_metadata(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
            "history-head",
        )
        .unwrap();
    let history = store.build_history(10).unwrap();
    assert_eq!(history.len(), 1);
    let row = &history[0];
    assert_eq!(row.head_sha, "history-head");
    assert_eq!(row.files, 2);
    assert_eq!(row.parse_failed, 1);
    assert_eq!(row.languages_covered, 2);
    assert_eq!(row.dead_confident, 1);
    assert_eq!(row.dead_ambiguous, 1);
    assert_eq!(
        row.build_ms, None,
        "unmeasured time must remain unavailable"
    );
}

/// `count_search_symbols` must let the full-text match drive its join.
///
/// Written as a plain `JOIN`, SQLite 3.45 leads with `nodes_fts_map` filtered on
/// `generation_id` and re-scans full-text storage once per mapped row. That
/// measured 12.7s against a 50k-file index while the match alone took 1.7ms,
/// and it never surfaced because the only existing latency gate gated
/// `search_symbols`, which is anchored to the FTS table by its `ORDER BY
/// bm25(...)` and so was fast for an unrelated reason.
///
/// The bound below is deliberately loose: the correct plan answers in
/// single-digit milliseconds, the pathological one takes seconds, so anything
/// in between still fails long before it reaches a user.
#[test]
fn count_search_symbols_is_driven_by_the_fts_match_not_the_generation_map() {
    let n = 10_000usize;
    let sources: Vec<(String, String)> = (0..n)
        .map(|index| {
            (
                format!("pkg/f{index}.py"),
                format!("def searchable_{index}():\n    return {index}\n"),
            )
        })
        .collect();
    let refs: Vec<FileRef> = sources
        .iter()
        .map(|(path, source)| FileRef { path, source })
        .collect();
    let extractions = extract_all(&refs);
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    // A miss is the sharpest probe: it does the same scan with no result rows,
    // so any cost measured here is pure plan overhead.
    for query in ["searchable_4242", "zzzz_no_such_symbol_zzzz"] {
        let started = Instant::now();
        let total = store.count_search_symbols(query).unwrap();
        let elapsed = started.elapsed();
        eprintln!("count_search_symbols({query}) = {total} in {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "count_search_symbols({query}) took {elapsed:?}; the FTS match is no \
             longer driving the join"
        );
    }
}

/// `vacuum_if_needed` must actually return space to the filesystem.
///
/// Mutation testing found this function fully replaceable with `Ok(())`
/// without any test noticing, and both of its threshold comparisons mutable
/// in every direction. That matters because pruning only moves pages to the
/// freelist: without a working vacuum a pruned database never shrinks, so the
/// CLI would prune correctly and the user would still watch the file grow.
#[test]
fn vacuum_returns_freed_pages_to_the_filesystem() {
    let dir = tmp_dir("vacuum");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();

    // Enough distinct content to make the freelist material after pruning.
    let mut extractions = Vec::new();
    for i in 0..60 {
        extractions.push(extract_file(
            &format!("src/f{i}.py"),
            &format!("def fn{i}():\n    return {i}\n"),
        ));
    }
    for round in 0..6 {
        let mut churn = extractions.clone();
        churn.push(extract_file(
            "src/churn.py",
            &format!("def churn():\n    return {round}\n"),
        ));
        let mut resolver = Resolver::new();
        resolver.index_extractions(&churn);
        let resolution = resolver.resolve_all(&churn);
        let analysis = analyze(&churn, &resolution);
        store
            .save_generation(&churn, &resolution, &analysis)
            .unwrap();
    }

    store.prune_generations_except_latest(1).unwrap();
    store.prune_extraction_cache().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let freelist: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    let pages_before: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    assert!(
        freelist > 0,
        "pruning should have left free pages to reclaim, found none"
    );

    store.vacuum_if_needed().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let pages_after: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    let freelist_after: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    drop(conn);

    assert!(
        pages_after < pages_before,
        "vacuum did not shrink the database: {pages_before} -> {pages_after} pages"
    );
    assert!(
        freelist_after < freelist,
        "vacuum did not consume the freelist: {freelist} -> {freelist_after}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// A freshly created store must be in incremental auto-vacuum mode, and its
/// reclaim must not be a whole-file rewrite.
///
/// The existing reclaim test asserts only that space comes back, which a full
/// `VACUUM` also satisfies — so it cannot tell the cheap path from the
/// expensive one. That distinction is the entire point of the change: a full
/// `VACUUM` costs O(database) and ran on nearly every build (measured: 937 ms,
/// 28% of an incremental build on this repository), while
/// `PRAGMA incremental_vacuum` costs O(pages reclaimed).
///
/// Mode is asserted at creation because that is the only moment it can be set
/// on a database that has no tables yet. If `configure_connection` stopped
/// issuing the pragma, or started issuing it after `migrate`, every new store
/// would silently fall back to whole-file rewrites — no failure, just the old
/// cost back.
#[test]
fn new_stores_use_incremental_auto_vacuum_and_reclaim_without_a_full_rewrite() {
    let dir = tmp_dir("vacuum-incremental");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let auto_vacuum: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    assert_eq!(
        auto_vacuum, 2,
        "a new store should be auto_vacuum=INCREMENTAL (2), found {auto_vacuum} \
         (0=NONE forces a whole-file VACUUM to reclaim anything)"
    );

    // Same churn shape as the reclaim test: enough generations that pruning
    // leaves a freelist above the threshold.
    let mut extractions = Vec::new();
    for i in 0..60 {
        extractions.push(extract_file(
            &format!("src/f{i}.py"),
            &format!("def fn{i}():\n    return {i}\n"),
        ));
    }
    for round in 0..6 {
        let mut churn = extractions.clone();
        churn.push(extract_file(
            "src/churn.py",
            &format!("def churn():\n    return {round}\n"),
        ));
        let mut resolver = Resolver::new();
        resolver.index_extractions(&churn);
        let resolution = resolver.resolve_all(&churn);
        let analysis = analyze(&churn, &resolution);
        store
            .save_generation(&churn, &resolution, &analysis)
            .unwrap();
    }
    store.prune_generations_except_latest(1).unwrap();
    store.prune_extraction_cache().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let freelist_before: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    let pages_before: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    assert!(
        freelist_before > 0,
        "pruning should have left free pages to reclaim, found none"
    );

    store.vacuum_if_needed().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let pages_after: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    let freelist_after: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    let auto_vacuum_after: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    drop(conn);

    // The reclaim contract is identical to the full-vacuum path's: space must
    // actually return to the filesystem. Only the cost differs.
    assert!(
        pages_after < pages_before,
        "incremental vacuum did not shrink the database: \
         {pages_before} -> {pages_after} pages"
    );
    assert!(
        freelist_after < freelist_before,
        "incremental vacuum did not consume the freelist: \
         {freelist_before} -> {freelist_after}"
    );
    assert_eq!(
        auto_vacuum_after, 2,
        "reclaim must not drop the store out of incremental mode"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// A pre-existing mode-NONE store is converted by its next reclaim, not left
/// paying for whole-file rewrites forever.
///
/// Stores created before this change are `auto_vacuum=NONE`, and that mode
/// cannot be switched on a populated database except by a full rewrite. The
/// conversion therefore rides on the `VACUUM` such a store was already going to
/// run, so it costs nothing extra — but only if it actually happens. Without
/// this test the fallback branch would keep working correctly and every
/// long-lived store would keep the old cost, which is invisible from behaviour
/// alone.
#[test]
fn a_legacy_none_mode_store_is_converted_to_incremental_by_its_next_reclaim() {
    let dir = tmp_dir("vacuum-convert");
    let db_path = dir.join("index.sqlite");

    // Build a store and force it back to the legacy mode, which is what every
    // database created before this change looks like on disk.
    {
        let store = Store::open(&db_path).unwrap();
        let mut extractions = Vec::new();
        for i in 0..60 {
            extractions.push(extract_file(
                &format!("src/f{i}.py"),
                &format!("def fn{i}():\n    return {i}\n"),
            ));
        }
        for round in 0..6 {
            let mut churn = extractions.clone();
            churn.push(extract_file(
                "src/churn.py",
                &format!("def churn():\n    return {round}\n"),
            ));
            let mut resolver = Resolver::new();
            resolver.index_extractions(&churn);
            let resolution = resolver.resolve_all(&churn);
            let analysis = analyze(&churn, &resolution);
            store
                .save_generation(&churn, &resolution, &analysis)
                .unwrap();
        }
    }
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.pragma_update(None, "auto_vacuum", "NONE").unwrap();
        conn.execute("VACUUM", []).unwrap();
        let mode: i64 = conn
            .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, 0, "test setup failed to produce a legacy store");
    }

    let store = Store::open(&db_path).unwrap();
    store.prune_generations_except_latest(1).unwrap();
    store.prune_extraction_cache().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let freelist_before: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    assert!(
        freelist_before > 0,
        "pruning should have left free pages to reclaim, found none"
    );

    store.vacuum_if_needed().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let mode_after: i64 = conn
        .query_row("PRAGMA auto_vacuum", [], |r| r.get(0))
        .unwrap();
    let freelist_after: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    drop(conn);

    assert_eq!(
        mode_after, 2,
        "a legacy store's reclaim should convert it to incremental mode, \
         leaving it at {mode_after}"
    );
    assert!(
        freelist_after < freelist_before,
        "the converting vacuum must still reclaim: \
         {freelist_before} -> {freelist_after}"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// `vacuum_if_needed` must also decline when there is little to reclaim.
///
/// The companion test above proves it vacuums when the freelist is large, but
/// that alone leaves the whole threshold mutable: mutation testing could
/// replace the `&&` with `||`, or the ratio's `/` with `*` or `%`, and no test
/// noticed, because every surviving mutant still vacuumed in the one scenario
/// under test. A full `VACUUM` takes an exclusive lock and rewrites the file,
/// so "vacuums on every build regardless of need" is a real performance defect
/// that looks identical to correct behaviour unless the declining case is
/// pinned too.
#[test]
fn vacuum_declines_when_the_freelist_is_below_threshold() {
    let dir = tmp_dir("vacuum-threshold");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();

    let mut extractions = Vec::new();
    for i in 0..60 {
        extractions.push(extract_file(
            &format!("src/f{i}.py"),
            &format!("def fn{i}():\n    return {i}\n"),
        ));
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    // A single freshly written generation leaves almost nothing on the
    // freelist, so the 5% policy must decline.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let freelist: i64 = conn
        .query_row("PRAGMA freelist_count", [], |r| r.get(0))
        .unwrap();
    let pages_before: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    assert!(
        (freelist as f64) / (pages_before as f64) <= 0.05,
        "fixture must sit below the vacuum threshold, got {freelist}/{pages_before}"
    );

    store.vacuum_if_needed().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let pages_after: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    assert_eq!(
        pages_after, pages_before,
        "vacuum ran below the 5% freelist threshold: {pages_before} -> {pages_after} pages; \
         an unconditional VACUUM takes an exclusive lock and rewrites the file on every build"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// The vacuum policy itself, asserted directly.
///
/// Every case below corresponds to a mutant that survived when the policy was
/// only reachable through `vacuum_if_needed`'s side effect: `&&` → `||`, `/` →
/// `*`, `/` → `%`, and the `>` boundaries. A full VACUUM takes an exclusive
/// lock and rewrites the whole file, so vacuuming when it is not warranted is
/// a real defect that produces the same page counts as behaving correctly.
#[test]
fn vacuum_policy_triggers_only_above_the_freelist_ratio() {
    // Empty database: no pages, nothing to reclaim. Kills `&&` -> `||`, which
    // would vacuum on ratio alone with a zero page count.
    assert!(
        !Store::should_vacuum(0, 0),
        "empty database must not vacuum"
    );
    assert!(
        !Store::should_vacuum(10, 0),
        "a zero page count must never vacuum, whatever the freelist says"
    );

    // Below threshold: 5 free of 1000 pages is 0.5%.
    assert!(
        !Store::should_vacuum(5, 1000),
        "0.5% free must not justify an exclusive-lock rewrite"
    );

    // Above threshold: 100 free of 1000 pages is 10%.
    assert!(
        Store::should_vacuum(100, 1000),
        "10% free must reclaim space"
    );

    // Exactly at the threshold is not above it. Kills `>` -> `>=`.
    let at_threshold = (1000.0 * Store::VACUUM_FREELIST_RATIO) as i64;
    assert!(
        !Store::should_vacuum(at_threshold, 1000),
        "the ratio bound is exclusive: exactly {}% must not vacuum",
        Store::VACUUM_FREELIST_RATIO * 100.0
    );
    assert!(
        Store::should_vacuum(at_threshold + 1, 1000),
        "one page past the bound must vacuum"
    );

    // `/` -> `*` and `/` -> `%` both explode on a small freelist against a
    // large page count, so this single case pins the operator.
    assert!(
        !Store::should_vacuum(1, 100_000),
        "one free page in 100k must not vacuum; the ratio must be a division"
    );
}

/// SC8: a generation's payload serves cache misses — but only on full identity.
///
/// `extraction_cache` and `generation_files` held byte-identical payloads for
/// the same content, so every extraction was stored twice: 153 MiB of a 374 MiB
/// database on one corpus. The naive dedup is unsafe, though — the cache is
/// keyed `(content_hash, language, grammar_version, analyzer_version)` exactly
/// so a payload produced by older extraction semantics can never be reused, and
/// `generation_files` recorded only the first two. Schema v8 carries the
/// identity so the fallback can enforce the same guarantee.
#[test]
fn a_generation_payload_serves_cache_misses_only_on_matching_identity() {
    let dir = tmp_dir("sc8-fallback");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();

    let source = "def helper():\n    return 1\n";
    let ext = extract_file("src/helper.py", source);
    let key = CacheKey::for_source(&ext.language, source);

    let mut resolver = Resolver::new();
    resolver.index_extractions(std::slice::from_ref(&ext));
    let resolution = resolver.resolve_all(std::slice::from_ref(&ext));
    let analysis = analyze(std::slice::from_ref(&ext), &resolution);
    store
        .save_generation(std::slice::from_ref(&ext), &resolution, &analysis)
        .unwrap();

    // Nothing was ever admitted to the cache, so a hit here can only have come
    // from the generation's copy.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let cached: i64 = conn
        .query_row("SELECT COUNT(*) FROM extraction_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cached, 0, "fixture must not pre-populate the cache");

    let found = store.try_get_cached_extraction(&key).unwrap();
    assert!(
        found.is_some(),
        "a retained generation's payload must serve a cache miss on the same identity"
    );
    assert_eq!(found.unwrap().file_path, "src/helper.py");

    // A different analyzer version is a different identity and must MISS —
    // this is the whole reason the columns exist. Serving it would resurrect
    // payloads produced by older extraction semantics, which is precisely how
    // fixed false positives come back.
    let stale = CacheKey {
        analyzer_version: format!("{}-stale", key.analyzer_version),
        ..key.clone()
    };
    assert!(
        store.try_get_cached_extraction(&stale).unwrap().is_none(),
        "a payload from different extraction semantics must never be served"
    );

    let stale_grammar = CacheKey {
        grammar_version: format!("{}-stale", key.grammar_version),
        ..key.clone()
    };
    assert!(
        store
            .try_get_cached_extraction(&stale_grammar)
            .unwrap()
            .is_none(),
        "a payload from a different grammar must never be served"
    );

    // Rows written before v8 carry NULL identity. Absence of a recorded
    // identity is not proof of a matching one, so they must never be eligible.
    conn.execute(
        "UPDATE generation_files SET grammar_version = NULL, analyzer_version = NULL",
        [],
    )
    .unwrap();
    drop(conn);
    assert!(
        store.try_get_cached_extraction(&key).unwrap().is_none(),
        "a pre-v8 row with unknown identity must never satisfy a cache lookup"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// D17: the unresolved-call ledger persists, is readable, and is pruned.
///
/// The resolver has always computed these — they are the honest denominator for
/// "is this symbol really uncalled?" — but they lived only in memory, so nothing
/// could ask why a symbol had no callers. Persisting them adds a per-generation
/// table, which means it also has to be pruned: an unpruned side table is
/// exactly the SC1 failure, where the store grows by O(repository) per build
/// forever while every visible count looks bounded.
#[test]
fn unresolved_calls_are_persisted_and_pruned_with_their_generation() {
    let extractions = vec![extract_file(
        "app.py",
        "def run():\n    return never_defined_anywhere()\n",
    )];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    assert!(
        !resolution.unresolved.is_empty(),
        "fixture precondition: the call must actually be unresolved"
    );
    let analysis = analyze(&extractions, &resolution);

    let store = Store::open_in_memory().unwrap();
    for generation in 0..4 {
        store
            .save_generation_with_metadata(
                &extractions,
                &resolution,
                &analysis,
                GenerationWriteOpts::default(),
                &format!("head-{generation}"),
            )
            .unwrap();
    }

    let rows = store.latest_unresolved(100).unwrap();
    assert!(
        rows.iter()
            .any(|(_, callee, _)| callee == "never_defined_anywhere"),
        "the unresolved callee must be readable back: {rows:?}"
    );

    // Retention is applied by the build path, not by `save_generation`, so the
    // prune is invoked here the way `devmap build` invokes it. Four commits
    // must not leave four generations' worth of ledger rows behind.
    store
        .prune_generations_except_latest(devmap_store::schema::GENERATION_RETENTION)
        .unwrap();
    let store_rows = store.count_unresolved_rows().unwrap();
    let per_generation = resolution.unresolved.len();
    let retained = devmap_store::schema::GENERATION_RETENTION;
    assert!(
        store_rows <= per_generation * retained,
        "ledger rows must be pruned with their generation: {store_rows} rows for \
         {per_generation} unresolved calls across {retained} retained generations"
    );
}

/// Body signatures must survive the differential write path.
///
/// An incremental build re-extracts only the changed files and carries every
/// other file's symbol rows forward from the previous generation. If the carry
/// dropped the three signature columns, the very next build after an edit would
/// report the whole repository as unsigned — and an unsigned symbol reads as
/// "not examined", so `devmap clones` would answer "nothing found" over a tree
/// it had simply stopped looking at. The failure is silent, which is why it is
/// pinned here rather than left to the end-to-end suite.
#[test]
fn body_signatures_survive_an_incremental_carry_forward() {
    let body = "\n    total = 0\n    for row in rows:\n        if row.active:\n            total += row.amount * rate\n        else:\n            total -= row.penalty\n    return total\n";
    let shared = format!("def compute(rows, rate):{body}");
    let a = extract_file("a.py", &shared);
    let b = extract_file("b.py", &shared);
    let c = extract_file("c.py", "def solo(x):\n    return x + 1\n");

    let signed_in_extraction = |e: &devmap_extract::model::Extraction| {
        e.symbols
            .iter()
            .filter(|s| s.body_signature.is_some())
            .count()
    };
    assert_eq!(
        signed_in_extraction(&a),
        1,
        "fixture must produce a signed symbol or this test proves nothing"
    );

    let all = [a.clone(), b.clone(), c.clone()];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&all);
    let resolution = resolver.resolve_all(&all);
    let analysis = analyze(&all, &resolution);

    let store = Store::open_in_memory().unwrap();
    store.save_generation(&all, &resolution, &analysis).unwrap();

    let (cold, _) = store.latest_clone_candidates().unwrap();
    let cold_signed = cold.len();
    assert!(
        cold_signed >= 2,
        "cold build signed {cold_signed} symbols; expected the two shared bodies"
    );

    // Edit only c.py. a.py and b.py — which hold the duplicate — are carried.
    let c2 = extract_file("c.py", "def solo(x):\n    return x + 2\n");
    let after = [a, b, c2];
    let mut resolver2 = Resolver::new();
    resolver2.index_extractions(&after);
    let resolution2 = resolver2.resolve_all(&after);
    let analysis2 = analyze(&after, &resolution2);
    store
        .save_generation_with_opts(
            &after,
            &resolution2,
            &analysis2,
            GenerationWriteOpts {
                affected_paths: vec!["c.py".into()],
                deleted_paths: vec![],
                build_started: None,
                repo_root: None,
            },
        )
        .unwrap();

    let (warm, _) = store.latest_clone_candidates().unwrap();
    assert_eq!(
        warm.len(),
        cold_signed,
        "an incremental build lost signatures: {} signed after carry-forward, {} before",
        warm.len(),
        cold_signed
    );

    // And the duplicate is still findable, which is the fact the columns exist for.
    let summary = devmap_analyze::group_clones(&warm, 0);
    assert!(
        summary.groups.iter().any(|g| g.members.len() == 2
            && g.members.iter().any(|m| m.file_path == "a.py")
            && g.members.iter().any(|m| m.file_path == "b.py")),
        "the a.py/b.py duplicate did not survive the incremental build: {:?}",
        summary.groups
    );
}
