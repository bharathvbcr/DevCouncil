//! Fault injection around retention, reclamation and store integrity.
//!
//! The retention work (SC1/SC7) added two new destructive operations on the
//! hot write path: generation pruning and extraction-cache eviction. Both
//! delete rows across several tables, and both now run on every build and
//! every daemon resync. Destructive code on the hot path needs its failure
//! modes pinned, not just its happy path.
//!
//! The governing rule for every case here: a check that could not run must
//! never report what a check that ran and passed reports. Silent emptiness is
//! the failure mode to hunt — an empty answer from a broken store is
//! indistinguishable from an empty answer from a healthy one unless the store
//! refuses.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_resolve::Resolver;
use devmap_store::Store;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-fault-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Commit `rounds` generations, churning one file so each is genuinely new.
///
/// Each extraction is also admitted to `extraction_cache` through the real
/// `admit_cached_extraction` path. `save_generation` alone does not populate
/// the cache — only the extraction path does — so a fixture that skips this
/// leaves the cache empty and any assertion about eviction passes vacuously.
fn build_generations(store: &Store, rounds: usize) {
    for round in 0..rounds {
        let sources: Vec<(String, String)> = (0..12)
            .map(|i| {
                (
                    format!("src/f{i}.py"),
                    format!("def fn{i}():\n    return {i}\n"),
                )
            })
            .chain(std::iter::once((
                "src/churn.py".to_string(),
                format!("def churn():\n    return {round}\n"),
            )))
            .collect();

        let extractions: Vec<_> = sources
            .iter()
            .map(|(path, source)| extract_file(path, source))
            .collect();
        for ((_, source), ext) in sources.iter().zip(&extractions) {
            let key = devmap_extract::cache::CacheKey::for_source(&ext.language, source);
            store.admit_cached_extraction(&key, ext).unwrap();
        }

        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = analyze(&extractions, &resolution);
        store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap();
    }
}

/// A generation is either wholly present or wholly gone — never half-pruned.
///
/// The prune deletes from seven tables in sequence inside one transaction. If
/// that transaction were ever split, a generation could lose its edges while
/// keeping its files, and every query against it would return quietly wrong
/// answers rather than failing. This asserts the invariant directly across
/// every generation-scoped table.
#[test]
fn pruning_never_leaves_a_partially_deleted_generation() {
    let dir = tmp_dir("atomic");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    build_generations(&store, 6);
    store.prune_generations_except_latest(2).unwrap();
    store.prune_extraction_cache().unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let live: Vec<i64> = conn
        .prepare("SELECT id FROM generations ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(!live.is_empty(), "prune removed every generation");

    // No orphan may reference a generation that no longer exists...
    for table in [
        "generation_files",
        "generation_nodes",
        "generation_edges",
        "generation_dead_symbols",
        "nodes_fts_map",
    ] {
        let orphans: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {table} \
                     WHERE generation_id NOT IN (SELECT id FROM generations)"
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            orphans, 0,
            "{table} holds {orphans} rows for pruned generations"
        );
    }

    // ...and every surviving generation must still have its file rows. A
    // generation stripped of content but left in `generations` is the silent
    // half-prune this test exists to catch.
    for gen in &live {
        let files: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM generation_files WHERE generation_id = ?1",
                [gen],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            files > 0,
            "surviving generation {gen} has no files; it was partially pruned"
        );
    }
}

/// A truncated database must fail loudly, not answer as if it were empty.
///
/// This is the failure that matters most for a code-intelligence store: a
/// corrupt index that returns zero dead symbols looks exactly like a clean
/// repository, so "approved" comes to mean "unexamined".
#[test]
fn a_truncated_database_never_answers_as_though_it_were_healthy() {
    let dir = tmp_dir("truncated");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    build_generations(&store, 3);
    let healthy_symbols = store.count_search_symbols("fn").unwrap();
    assert!(healthy_symbols > 0, "fixture must index some symbols");
    drop(store);

    // Cut the file in half, keeping a valid header so it still opens.
    let len = fs::metadata(&db_path).unwrap().len();
    let file = fs::OpenOptions::new().write(true).open(&db_path).unwrap();
    file.set_len(len / 2).unwrap();
    drop(file);

    // Either the open fails or the query fails. What must NOT happen is a
    // successful open followed by a confident, empty answer.
    match Store::open(&db_path) {
        Err(_) => {}
        Ok(store) => match store.count_search_symbols("fn") {
            Err(_) => {}
            Ok(found) => panic!(
                "truncated database answered successfully with {found} symbols \
                 (healthy store had {healthy_symbols}); corruption must fail closed"
            ),
        },
    }
}

/// Cache eviction must not be defeated by SQL `NOT IN` NULL semantics.
///
/// `prune_extraction_cache` deletes rows whose `(content_hash, language)` is
/// `NOT IN` the retained generations. In SQL, `NOT IN` against a set containing
/// a NULL yields NULL — not true — for every row, so a single NULL would
/// silently turn the whole eviction into a no-op and the cache would resume
/// growing without bound with no error anywhere. The schema declares both
/// columns `NOT NULL`; this pins the guarantee the eviction depends on.
#[test]
fn cache_eviction_cannot_be_silently_disabled_by_a_null_key() {
    let dir = tmp_dir("nullkey");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    build_generations(&store, 4);
    store.prune_generations_except_latest(1).unwrap();

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let nullable: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM generation_files \
             WHERE content_hash IS NULL OR language IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        nullable, 0,
        "a NULL key would turn cache eviction into a silent no-op"
    );
    drop(conn);

    let removed = store.prune_extraction_cache().unwrap();
    assert!(
        removed > 0,
        "four generations of churn then a prune must leave unreachable cache rows to evict"
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let unreachable: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM extraction_cache c WHERE NOT EXISTS \
             (SELECT 1 FROM generation_files g \
              WHERE g.content_hash = c.content_hash AND g.language = c.language)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        unreachable, 0,
        "eviction left {unreachable} unreachable rows"
    );
}

/// Pruning with nothing to prune must be a no-op, not a wipe.
///
/// `prune_extraction_cache` deletes everything `NOT IN` the retained set, so an
/// empty retained set means "delete the entire cache". That is correct, but it
/// makes ordering load-bearing: run before the generation is committed and it
/// would throw away a cache that is about to be needed.
#[test]
fn pruning_a_fresh_store_is_harmless() {
    let dir = tmp_dir("fresh");
    let store = Store::open(dir.join("index.sqlite")).unwrap();

    assert_eq!(store.prune_generations_except_latest(2).unwrap(), 0);
    assert_eq!(store.prune_extraction_cache().unwrap(), 0);

    build_generations(&store, 1);
    let pruned = store.prune_generations_except_latest(2).unwrap();
    assert_eq!(pruned, 0, "a single generation is within retention");

    // The one committed generation must survive both prunes intact.
    assert!(
        store.latest_generation_id().unwrap().is_some(),
        "pruning a single-generation store destroyed it"
    );
    assert!(
        store.count_search_symbols("fn").unwrap() > 0,
        "pruning a single-generation store emptied its symbols"
    );
}

/// Concurrent readers must not observe a half-pruned store, and the prune must
/// not fail because a reader held the database.
///
/// The daemon prunes on every resync while IPC queries run against the same
/// file, so this is the real concurrency shape, not a synthetic one.
#[test]
fn a_reader_during_prune_sees_a_consistent_store() {
    let dir = tmp_dir("concurrent");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    build_generations(&store, 8);

    let reader_path = db_path.clone();
    let reader = std::thread::spawn(move || {
        let reader = Store::open(&reader_path).expect("reader opens");
        let mut observations = Vec::new();
        for _ in 0..40 {
            // A reader must always see symbols: pruning removes old
            // generations, never the latest one.
            observations.push(reader.count_search_symbols("fn").expect("reader queries"));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        observations
    });

    store.prune_generations_except_latest(1).unwrap();
    store.prune_extraction_cache().unwrap();
    store.vacuum_if_needed().unwrap();

    let observations = reader.join().expect("reader thread survived the prune");
    assert!(
        observations.iter().all(|count| *count > 0),
        "a concurrent reader observed an empty store during pruning: {observations:?}"
    );
}
