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

/// The per-generation deletes `prune_generations_except_latest` really issues,
/// read from its source, so the plan check above cannot drift from the code it
/// claims to describe.
#[test]
fn b6_the_checked_deletes_are_the_ones_prune_issues() {
    let source = include_str!("../src/db/maintenance.rs");
    let start = source
        .find("pub fn prune_generations_except_latest")
        .expect("prune is defined in maintenance.rs");
    let body = &source[start..];
    let body = &body[..body[1..].find("\n    pub fn ").map_or(body.len(), |end| end + 1)];

    let suffix = " WHERE generation_id = ?1\"";
    let mut issued: Vec<&str> = body
        .match_indices("DELETE FROM ")
        .filter_map(|(at, marker)| {
            let rest = &body[at + marker.len()..];
            let table = rest.split(' ').next()?;
            rest[table.len()..].starts_with(suffix).then_some(table)
        })
        .collect();
    issued.sort_unstable();
    let mut checked = PRUNED_PER_GENERATION.to_vec();
    checked.sort_unstable();
    assert_eq!(
        issued, checked,
        "prune deletes by generation_id from a different set of tables than the plan check covers"
    );
    assert!(
        body.contains(
            "DELETE FROM nodes_fts WHERE rowid IN (SELECT rowid_ref FROM nodes_fts_map WHERE generation_id = ?1)"
        ),
        "the FTS prune statement the plan check mirrors is no longer the one prune issues"
    );
}

/// A store whose four generations each rename the entry function, so every
/// generation closes an edge the one before it opened.
fn four_generations(label: &str) -> (Store, std::path::PathBuf, Vec<u32>) {
    use devmap_resolve::Resolver;
    let path = scratch_db(label);
    let store = Store::open(&path).expect("store");
    let generations = (0..4)
        .map(|revision| {
            let body = format!(
                "def leaf():\n    return {revision}\n\n\ndef caller():\n    return leaf()\n\n\ndef rev_{revision}():\n    return caller()\n"
            );
            let extractions = vec![extract_file("mod.py", &body)];
            let mut resolver = Resolver::new();
            resolver.index_extractions(&extractions);
            let resolution = resolver.resolve_all(&extractions).expect("resolve");
            let analysis = devmap_analyze::analyze(&extractions, &resolution);
            store
                .save_generation(&extractions, &resolution, &analysis)
                .expect("generation")
        })
        .collect();
    (store, path, generations)
}

fn rows_of(conn: &Connection, table: &str, generation: u32) -> i64 {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE generation_id = ?1"),
        [generation],
        |row| row.get(0),
    )
    .unwrap_or_else(|error| panic!("{table}: {error}"))
}

fn count_of(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

/// What the bounded delete does, not only how it is planned: pruning removes
/// every per-generation row of exactly the generations outside the retention
/// window, reports how many generations it removed, keeps the retained ones
/// byte-for-count intact, and reclaims the validity-range rows that ended
/// before the oldest retained generation and no others.
#[test]
fn b6_prune_removes_exactly_the_generations_outside_the_window() {
    for (keep, removed) in [(1usize, 3usize), (2, 2)] {
        let (store, path, generations) = four_generations(&format!("b6-prune-{keep}"));
        let inspect = Connection::open(&path).expect("inspect connection");
        let before: Vec<Vec<i64>> = generations
            .iter()
            .map(|generation| {
                PRUNED_PER_GENERATION
                    .iter()
                    .map(|table| rows_of(&inspect, table, *generation))
                    .collect()
            })
            .collect();
        for (generation, counts) in generations.iter().zip(&before) {
            assert!(
                counts[1] > 0 && counts[0] > 0,
                "fixture precondition: generation {generation} has nodes and FTS members, got {counts:?}"
            );
        }
        let oldest_kept = generations[removed];
        assert!(
            count_of(
                &inspect,
                &format!("SELECT COUNT(*) FROM edge_rows WHERE valid_to = {oldest_kept}")
            ) > 0,
            "fixture precondition: an edge ended exactly at the oldest retained generation"
        );
        assert!(
            count_of(
                &inspect,
                &format!("SELECT COUNT(*) FROM edge_rows WHERE valid_to < {oldest_kept}")
            ) > 0,
            "fixture precondition: an edge ended before the retained window"
        );

        let still_readable = count_of(
            &inspect,
            &format!("SELECT COUNT(*) FROM edge_rows WHERE valid_to > {oldest_kept}"),
        );
        let pruned = store
            .prune_generations_except_latest(keep)
            .expect("prune");
        assert_eq!(pruned, removed, "keep {keep} of 4 generations removes {removed}");

        let remaining: Vec<u32> = inspect
            .prepare("SELECT id FROM generations ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(remaining, generations[removed..]);
        for (index, generation) in generations.iter().enumerate() {
            let now: Vec<i64> = PRUNED_PER_GENERATION
                .iter()
                .map(|table| rows_of(&inspect, table, *generation))
                .collect();
            if index < removed {
                assert!(
                    now.iter().all(|rows| *rows == 0),
                    "pruned generation {generation} left rows behind: {now:?}"
                );
            } else {
                assert_eq!(
                    now, before[index],
                    "retained generation {generation} lost rows to the prune"
                );
            }
        }
        // `valid_to <= oldest retained` is unreachable by any retained
        // generation; a range closed later is still read by one of them.
        assert_eq!(
            count_of(
                &inspect,
                &format!("SELECT COUNT(*) FROM edge_rows WHERE valid_to <= {oldest_kept}")
            ),
            0,
            "edge ranges that ended before the retained window must be reclaimed"
        );
        assert_eq!(
            count_of(
                &inspect,
                &format!("SELECT COUNT(*) FROM edge_rows WHERE valid_to > {oldest_kept}")
            ),
            still_readable,
            "ranges still readable by a retained generation must survive"
        );
    }
    // Nothing to prune is a zero, not an error, and deletes nothing.
    let (store, path, generations) = four_generations("b6-prune-noop");
    assert_eq!(store.prune_generations_except_latest(4).unwrap(), 0);
    assert_eq!(store.prune_generations_except_latest(9).unwrap(), 0);
    let inspect = Connection::open(&path).unwrap();
    assert_eq!(
        count_of(&inspect, "SELECT COUNT(*) FROM generations"),
        generations.len() as i64
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
    // The cost is only the cost of admission if admission happened: a no-op
    // admit is O(1) too. Every extraction the batches offered is a row, and
    // none of them was diverted to the retry ledger.
    let inspect = Connection::open(&path).expect("inspect connection");
    let count = |sql: &str| -> i64 {
        inspect
            .query_row(sql, [], |row| row.get(0))
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
    };
    assert_eq!(
        count("SELECT COUNT(*) FROM extraction_cache WHERE language != 'seed'"),
        (3 + 5) * batch as i64,
        "every admitted extraction must be a cache row"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM extraction_retry"),
        0,
        "a clean parse must be admitted, not recorded as a retry"
    );
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
