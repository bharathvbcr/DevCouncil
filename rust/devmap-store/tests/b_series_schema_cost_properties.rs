//! PLAN section 3 properties B6, B7 and B9, asserted against the kernel store.
//!
//! * B6 -- generation prune is a bounded delete: every per-generation delete
//!   the prune issues is answered by an index search on `generation_id`, never
//!   a scan of the table, so its cost follows the pruned generation and not the
//!   rows the retained one holds.
//! * B7 -- cache admission is O(1): admitting one extraction costs the same
//!   whether the cache holds a few rows or tens of thousands.
//! * B9 -- no redundant column in any primary key.

use devmap_extract::cache::CacheKey;
use devmap_extract::extract_file;
use devmap_store::Store;
use rusqlite::Connection;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn scratch_db(label: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("devmap-{label}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir.join("devmap.sqlite")
}

fn plan(conn: &Connection, sql: &str) -> Vec<String> {
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
    let rows: Vec<String> = statement
        .query_map([1], |row| row.get::<_, String>(3))
        .expect("plan rows")
        .collect::<Result<_, _>>()
        .expect("every plan row reads");
    rows
}

/// Tables `Store::prune_generations_except_latest` deletes from by
/// `generation_id`, in the order it deletes them.
const PRUNED_PER_GENERATION: [&str; 7] = [
    "nodes_fts_map",
    "generation_nodes",
    "generation_file_rows",
    "generation_coverage_gaps",
    "generation_file_digests",
    "generation_dead_symbols",
    "generation_literals",
];

#[test]
fn b6_every_prune_delete_is_an_index_search_not_a_table_scan() {
    let path = scratch_db("b6-prune-plan");
    drop(Store::open(&path).expect("fresh store"));
    let conn = Connection::open(&path).expect("inspect connection");

    for table in PRUNED_PER_GENERATION {
        let steps = plan(
            &conn,
            &format!("DELETE FROM {table} WHERE generation_id = ?1"),
        );
        assert!(
            steps.iter().any(|step| step.contains("SEARCH"))
                && !steps.iter().any(|step| step.contains("SCAN")),
            "pruning {table} must search by generation_id, got {steps:?}"
        );
    }

    // The FTS rows are found through the membership map, so the map must be
    // searched by generation; a scan of it makes prune cost the whole index.
    let fts = plan(
        &conn,
        "DELETE FROM nodes_fts WHERE rowid IN \
         (SELECT rowid_ref FROM nodes_fts_map WHERE generation_id = ?1)",
    );
    assert!(
        fts.iter()
            .any(|step| step.contains("nodes_fts_map") && step.contains("generation_id=?")),
        "the FTS prune must look members up by generation_id, got {fts:?}"
    );
    assert!(
        !fts.iter().any(|step| step.starts_with("SCAN nodes_fts_map")),
        "the FTS prune must not scan the membership map, got {fts:?}"
    );
}

#[test]
fn b6_the_plan_check_is_not_vacuous() {
    let path = scratch_db("b6-plan-detector");
    drop(Store::open(&path).expect("fresh store"));
    let conn = Connection::open(&path).expect("inspect connection");
    let unindexed = plan(
        &conn,
        "DELETE FROM generation_dead_symbols WHERE symbol_name = ?1",
    );
    assert!(
        unindexed.iter().any(|step| step.contains("SCAN")),
        "a delete on an unindexed column must read as a scan, got {unindexed:?}"
    );
}

fn admission_batch(store: &Store, tag: &str, count: usize) -> Duration {
    let prepared: Vec<_> = (0..count)
        .map(|index| {
            let source = format!("def f_{tag}_{index}():\n    return {index}\n");
            let extraction = extract_file(&format!("pkg/{tag}_{index}.py"), &source);
            (
                CacheKey::for_source(&extraction.language, &source),
                extraction,
            )
        })
        .collect();
    let started = Instant::now();
    for (key, extraction) in &prepared {
        store
            .admit_cached_extraction(key, extraction)
            .expect("admit");
    }
    started.elapsed()
}

fn seed_cache(path: &std::path::Path, rows: i64) {
    let seeder = Connection::open(path).expect("seed connection");
    let payload = "x".repeat(2048);
    seeder.execute_batch("BEGIN").expect("begin");
    {
        let mut insert = seeder
            .prepare(
                "INSERT INTO extraction_cache (content_hash, language, grammar_version, \
                 analyzer_version, payload_json, accessed_at) VALUES (?1, 'seed', 'g', 'a', ?2, 0)",
            )
            .expect("prepare seed insert");
        for row in 0..rows {
            insert
                .execute(rusqlite::params![row, payload])
                .expect("seed row");
        }
    }
    seeder.execute_batch("COMMIT").expect("commit");
}

#[test]
fn b7_admission_cost_does_not_grow_with_the_cache() {
    let path = scratch_db("b7-admission");
    let store = Store::open(&path).expect("store");
    let batch = 200;
    let empty_best = (0..3)
        .map(|round| admission_batch(&store, &format!("empty{round}"), batch))
        .min()
        .expect("three batches");
    seed_cache(&path, 50_000);
    let full_best = (0..5)
        .map(|round| admission_batch(&store, &format!("full{round}"), batch))
        .min()
        .expect("five batches");
    // A scan of 50,000 rows of 2 KiB costs orders of magnitude more per
    // admission than a keyed upsert; 8x leaves room for B-tree depth and load.
    assert!(
        full_best < empty_best * 8,
        "admission into a 50k-row cache took {full_best:?} per {batch}, \
         against {empty_best:?} into a near-empty one"
    );
}

#[test]
fn b9_no_primary_key_carries_a_column_its_ordinal_already_determines() {
    let path = scratch_db("b9-primary-keys");
    drop(Store::open(&path).expect("fresh store"));
    let conn = Connection::open(&path).expect("inspect connection");
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND sql IS NOT NULL ORDER BY name",
        )
        .expect("list tables")
        .query_map([], |row| row.get(0))
        .expect("names")
        .collect::<Result<_, _>>()
        .expect("every name reads");

    let mut checked = 0;
    for table in &tables {
        let mut statement = conn
            .prepare(&format!(
                "SELECT name FROM pragma_table_info('{table}') WHERE pk > 0 ORDER BY pk"
            ))
            .expect("table_info");
        let key: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("pk rows")
            .collect::<Result<_, _>>()
            .expect("pk reads");
        if key.iter().any(|column| column == "ordinal") {
            // (generation_id, ordinal) already identifies a row; anything more
            // is the redundant column B9 retired (`payload_hash` in the plan).
            assert_eq!(
                key,
                ["generation_id", "ordinal"],
                "{table} primary key carries a redundant column"
            );
            checked += 1;
        }
        assert!(
            !key.iter().any(|column| column == "payload_hash"),
            "{table} keys on payload_hash, which a row's position already determines"
        );
    }
    assert!(
        checked >= 2,
        "B9 must have inspected the ordinal-keyed tables, saw {checked}"
    );
}
