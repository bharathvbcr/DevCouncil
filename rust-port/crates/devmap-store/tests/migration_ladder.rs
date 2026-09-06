//! Every rung of the migration ladder, walked with a real store on it.
//!
//! The v17 doc comment already named this blindness class in its own words —
//! "No test migrated a real v16 store, so 1,842 of them passed over it" — and
//! naming it was not enough, because the fix that followed covered exactly one
//! rung. Two more defects landed underneath it:
//!
//! * `Store::open` on **any** store stamped 5 through 14 failed outright. The
//!   `version == 14` block stamped `user_version = 15` and then called
//!   `validate_schema`, which asserts the *current* schema and demands
//!   `generation_edges.candidate_total` — a column `MIGRATION_V15_TO_V16` had
//!   not added yet. Every existing installation below v15 was unopenable.
//! * `MIGRATION_V16_TO_V17` created two of the three payload indexes the fresh
//!   schema declares, so a migrated store full-scanned `file_payloads` on every
//!   extraction-cache miss — the exact cost v13 was introduced to remove, and
//!   invisible because `validate_schema` checks columns and never indexes.
//!
//! Neither is findable by a test that creates a store: a fresh store applies
//! `CREATE_SCHEMA_V3` and walks no rung at all. So this file does the one thing
//! the rest of the suite does not — it *reduces* a real store to each earlier
//! shape and opens it — and it asserts the property that makes the whole class
//! unrepeatable rather than the two instances of it:
//!
//! > **A migrated store and a fresh store are the same schema.** Same tables,
//! > same views, same columns, same indexes, same index expressions.
//!
//! That contract is derived from `CREATE_SCHEMA_V3` by reading it out of
//! SQLite, not hand-listed here, so a column, table, view or index added to the
//! fresh schema and forgotten in the ladder fails without anyone remembering to
//! extend this file.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::{Store, CURRENT_SCHEMA_VERSION};
use rusqlite::Connection;

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

/// The oldest rung this file reduces a store to.
///
/// Below 5 there is no `generations` table to hold rows, and the `version == 3`
/// and `version == 4` blocks re-run `CREATE_SCHEMA_V3` — which builds the
/// *current* shape — so those two rungs are covered by
/// `test_s2_migration_v3_to_v4` and its neighbour in `store_hardening.rs` and
/// are structurally incapable of the divergence this file hunts.
const OLDEST_REDUCIBLE: i32 = 5;

/// A store at the current schema, carrying rows in every relation the ladder
/// touches.
///
/// Rows, not an empty shell. A migration that drops a table and recreates it
/// empty passes a shape check and loses a repository's index; the row counts
/// asserted after each walk are what tells the two apart.
fn seed_current_store(db_path: &Path) {
    let store = Store::open(db_path).expect("a fresh store must open");
    drop(store);
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        r#"
        INSERT INTO paths (id, path) VALUES (1, 'a.py'), (2, 'twin.py'), (3, 'b.py');

        INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
        VALUES (1, 1.0, 'deadbeef', '/tmp/probe', '{"total_files":3,"total_symbols":2,
                "total_edges":1,"dead_symbols":[],"communities":[],"status":"Ok"}');

        INSERT INTO file_payloads
            (payload_id, file_id, content_hash, language, grammar_version, analyzer_version,
             parse_outcome_json, engine_json, extraction_json)
        VALUES (1, 1, 99, 'python', 'g1', 'a1', '"Clean"', '"TreeSitter"',
                '{"file_path":"a.py"}'),
               (2, 2, 99, 'python', 'g1', 'a1', '"Clean"', '"TreeSitter"',
                '{"file_path":"twin.py"}'),
               (3, 3, 77, 'python', NULL, NULL, '"Clean"', '"TreeSitter"',
                '{"file_path":"b.py"}');

        INSERT INTO generation_file_rows (generation_id, file_id, payload_id)
        VALUES (1, 1, 1), (1, 2, 2), (1, 3, 3);

        INSERT INTO generation_nodes
            (generation_id, ordinal, file_id, name, qualified_name, kind,
             span_start, span_end, is_exported, body_exact, body_structural, body_nodes)
        VALUES (1, 0, 1, 'helper', 'a.py::helper', 'Function', 0, 10, 0, 111, 222, 4),
               (1, 1, 3, 'main', 'b.py::main', 'Function', 0, 10, 1, NULL, NULL, NULL);

        INSERT INTO generation_edges
            (generation_id, ordinal, source_file_id, target_file_id, source_symbol,
             target_symbol, edge_kind, confidence, resolution, candidate_total)
        VALUES (1, 0, 3, 1, 'b.py::main', 'a.py::helper', 'Calls', 0.9, 'Exact', NULL);

        INSERT INTO generation_unresolved
            (generation_id, ordinal, source_file, source_symbol, callee_name,
             reason, classification, receiver)
        VALUES (1, 0, 'b.py', 'b.py::main', 'mystery', 'no candidate',
                'unresolved', 'obj');

        INSERT INTO generation_dead_symbols
            (generation_id, ordinal, file_path, symbol_name, confidence,
             is_exempt, exemption_reason)
        VALUES (1, 0, 'a.py', 'a.py::helper', 0.9, 0, NULL);

        INSERT INTO generation_coverage_gaps (generation_id, gap, path, reason)
        VALUES (1, 'ParseFailed', 'c.rs', 'grammar refused');

        INSERT INTO build_history
            (generation_id, built_at, head_sha, files, symbols, edges, dead_confident,
             dead_ambiguous, parse_failed, languages_covered, build_ms, db_bytes)
        VALUES (1, 1.0, 'deadbeef', 3, 2, 1, 1, 0, 1, 1, 5, 4096);

        INSERT INTO extraction_cache
            (content_hash, language, grammar_version, analyzer_version, payload_json, accessed_at)
        VALUES (99, 'python', 'g1', 'a1', '{"file_path":"a.py"}', 1.0);

        INSERT INTO extraction_retry (content_hash, language, attempts, last_reason, updated_at)
        VALUES (55, 'python', 1, 'timeout', 1.0);

        INSERT INTO pending_paths (path, queued_at, attempts) VALUES ('later.py', 1.0, 0);
        "#,
    )
    .expect("seeding a current store must succeed");
}

/// Undo exactly what the step into `version` created, and nothing else.
///
/// Each arm mirrors one `MIGRATION_V*` constant. Reversing *more* than a
/// migration created would make the parity assertion below fail for this
/// helper's reasons rather than the ladder's, so every statement here is
/// traceable to a line of `schema.rs`.
fn reduce_one_rung(conn: &Connection, from_version: i32) {
    let sql: &str = match from_version {
        // MIGRATION_V16_TO_V17: the payload split. Materialise the view back
        // into the base table it replaced and restore v13's index, which the
        // migration drops.
        17 => {
            "CREATE TABLE gf_flat AS SELECT * FROM generation_files;
             DROP VIEW generation_files;
             DROP TABLE generation_file_rows;
             DROP TABLE file_payloads;
             ALTER TABLE gf_flat RENAME TO generation_files;
             CREATE INDEX idx_generation_files_cache_identity
                 ON generation_files(content_hash, language, grammar_version, analyzer_version);"
        }
        // MIGRATION_V15_TO_V16
        16 => "ALTER TABLE generation_edges DROP COLUMN candidate_total;",
        // MIGRATION_V14_TO_V15
        15 => "ALTER TABLE generation_edges DROP COLUMN resolution;",
        // COVERAGE_GAPS_TABLE
        14 => "DROP TABLE generation_coverage_gaps;",
        // MIGRATION_V12_TO_V13
        13 => "DROP INDEX idx_generation_files_cache_identity;",
        // MIGRATION_V11_TO_V12
        12 => {
            "ALTER TABLE generation_nodes DROP COLUMN body_exact;
             ALTER TABLE generation_nodes DROP COLUMN body_structural;
             ALTER TABLE generation_nodes DROP COLUMN body_nodes;"
        }
        // MIGRATION_V10_TO_V11
        11 => "ALTER TABLE generation_unresolved DROP COLUMN receiver;",
        // MIGRATION_V9_TO_V10 — the index has to go first: SQLite refuses to
        // drop a column an index names, and this migration creates both.
        10 => {
            "DROP INDEX idx_generation_unresolved_class;
             ALTER TABLE generation_unresolved DROP COLUMN classification;"
        }
        // MIGRATION_V8_TO_V9 (UNRESOLVED_TABLE)
        9 => "DROP TABLE generation_unresolved;",
        // MIGRATION_V7_TO_V8
        8 => {
            "ALTER TABLE generation_files DROP COLUMN grammar_version;
             ALTER TABLE generation_files DROP COLUMN analyzer_version;"
        }
        // MIGRATION_V6_TO_V7
        7 => "ALTER TABLE generations DROP COLUMN repo_root;",
        // MIGRATION_V5_TO_V6 (BUILD_HISTORY_TABLE)
        6 => "DROP TABLE build_history;",
        other => panic!("no reduction defined for schema version {other}"),
    };
    conn.execute_batch(sql)
        .unwrap_or_else(|error| panic!("reducing a v{from_version} store failed: {error}"));
    conn.execute_batch(&format!("PRAGMA user_version = {};", from_version - 1))
        .unwrap();
}

/// A real store reduced to `target`, on disk, with its rows.
fn store_at_version(dir: &Path, target: i32) -> PathBuf {
    let db_path = dir.join(format!("legacy-v{target}.sqlite"));
    seed_current_store(&db_path);
    let conn = Connection::open(&db_path).unwrap();
    // Foreign keys are off by default in SQLite, and the reductions rename and
    // drop referenced tables; say so rather than depending on the default.
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    let mut version = CURRENT_SCHEMA_VERSION;
    while version > target {
        reduce_one_rung(&conn, version);
        version -= 1;
    }
    let stamped: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        stamped, target,
        "the reduction must leave the rung it names"
    );
    drop(conn);
    db_path
}

/// Every relation and every index SQLite reports, with the SQL that defines it.
///
/// `sqlite_master.sql` is the canonical text, so this compares index
/// *expressions* — the `COALESCE(grammar_version, '')` that makes
/// `idx_file_payloads_identity` NULL-safe is part of the contract, not
/// decoration. Auto-indexes and the FTS shadow tables are excluded: SQLite
/// names them itself and their shape is not this schema's to declare.
fn schema_objects(conn: &Connection) -> BTreeMap<String, String> {
    let mut stmt = conn
        .prepare(
            "SELECT type, name, COALESCE(sql, '') FROM sqlite_master
              WHERE name NOT LIKE 'sqlite_%'
              ORDER BY type, name",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    // `nodes_fts` is an FTS5 virtual table; SQLite creates and owns
    // `nodes_fts_data`, `_idx`, `_content`, `_docsize` and `_config` beneath it,
    // and their presence follows from the virtual table alone.
    .filter(|(_, name, _)| !name.starts_with("nodes_fts_") || name == "nodes_fts_map")
    .map(|(kind, name, sql)| (format!("{kind}:{name}"), normalise_sql(&sql)))
    .collect()
}

/// Collapse whitespace and comments so two spellings of one definition compare
/// equal.
///
/// The fresh schema carries doc comments inside its DDL and the ladder does
/// not; comparing raw text would report a difference that no query can observe.
fn normalise_sql(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    for line in sql.lines() {
        let code = match line.find("--") {
            Some(at) => &line[..at],
            None => line,
        };
        for word in code.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out
}

/// The columns of every relation, so a view's projection is compared too.
fn schema_columns(conn: &Connection) -> BTreeMap<String, Vec<String>> {
    let mut relations = conn
        .prepare(
            "SELECT name FROM sqlite_master
              WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
              ORDER BY name",
        )
        .unwrap();
    let names: Vec<String> = relations
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .filter(|name| !name.starts_with("nodes_fts_") || name == "nodes_fts_map")
        .collect();
    let mut out = BTreeMap::new();
    for name in names {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info(\"{name}\")"))
            .unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        out.insert(name, columns);
    }
    out
}

/// Row counts of the relations a migration could silently empty.
fn row_census(conn: &Connection) -> BTreeMap<&'static str, i64> {
    [
        "paths",
        "generations",
        "generation_nodes",
        "generation_files",
        "generation_edges",
        "generation_unresolved",
        "generation_dead_symbols",
        "generation_coverage_gaps",
        "build_history",
        "extraction_cache",
        "extraction_retry",
        "pending_paths",
        "file_payloads",
        "generation_file_rows",
    ]
    .into_iter()
    .map(|table| {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .unwrap_or_else(|error| panic!("counting {table} failed: {error}"));
        (table, count)
    })
    .collect()
}

/// **R1.** A store at any rung from 5 up must open.
///
/// This is the assertion that was missing. `test_s2_migration_v3_to_v4` builds
/// a v3 store with no `generation_edges` at all, the v4→v5 case likewise, v5→v6
/// reconstructs from a *current* store, and the v16 case starts above the break
/// — so nothing in 1,842 tests ever handed the ladder a store that had to walk
/// rung 14, which is where it stopped.
#[test]
fn every_rung_from_five_up_migrates_to_the_current_schema() {
    let dir = tmp_dir("ladder-open");
    let mut failures = Vec::new();
    for target in OLDEST_REDUCIBLE..CURRENT_SCHEMA_VERSION {
        let db_path = store_at_version(&dir, target);
        match Store::open(&db_path) {
            Ok(store) => {
                drop(store);
                let conn = Connection::open(&db_path).unwrap();
                let reached: i32 = conn
                    .query_row("PRAGMA user_version", [], |row| row.get(0))
                    .unwrap();
                if reached != CURRENT_SCHEMA_VERSION {
                    failures.push(format!(
                        "v{target} opened but stopped at schema {reached}, not \
                         {CURRENT_SCHEMA_VERSION}"
                    ));
                }
            }
            Err(error) => failures.push(format!("v{target} failed to open: {error}")),
        }
    }
    assert!(
        failures.is_empty(),
        "the migration ladder is broken for existing installations:\n  {}",
        failures.join("\n  ")
    );
    let _ = fs::remove_dir_all(&dir);
}

/// **R2, R15.** A migrated store and a fresh store are the same schema.
///
/// The contract that makes the class unrepeatable. `validate_schema` compares
/// columns against a hand-written `REQUIRED_SCHEMA`, which by construction
/// cannot see a *table* nobody added to it and does not look at indexes at all
/// — so `MIGRATION_V16_TO_V17` could omit `idx_file_payloads_cache_identity`
/// and every migrated store would silently full-scan the payload table on each
/// extraction-cache miss, with nothing to report it.
///
/// Derived, not listed: the expectation is read out of a store this build
/// created, so a relation or index added to `CREATE_SCHEMA_V3` and forgotten in
/// the ladder fails here without anyone extending this file.
#[test]
fn a_migrated_store_carries_the_same_schema_as_a_fresh_one() {
    let dir = tmp_dir("ladder-parity");

    let fresh_path = dir.join("fresh.sqlite");
    let fresh_store = Store::open(&fresh_path).unwrap();
    drop(fresh_store);
    let fresh = Connection::open(&fresh_path).unwrap();
    let expected_objects = schema_objects(&fresh);
    let expected_columns = schema_columns(&fresh);
    drop(fresh);

    let mut failures = Vec::new();
    for target in OLDEST_REDUCIBLE..CURRENT_SCHEMA_VERSION {
        let db_path = store_at_version(&dir, target);
        let Ok(store) = Store::open(&db_path) else {
            // `every_rung_from_five_up_migrates_to_the_current_schema` owns the
            // open failure; reporting it twice buries the parity result.
            continue;
        };
        drop(store);
        let conn = Connection::open(&db_path).unwrap();
        let objects = schema_objects(&conn);

        let missing: Vec<&String> = expected_objects
            .keys()
            .filter(|key| !objects.contains_key(*key))
            .collect();
        for key in missing {
            failures.push(format!(
                "v{target}: migrated store has no {key}, but a fresh store does"
            ));
        }
        let extra: Vec<&String> = objects
            .keys()
            .filter(|key| !expected_objects.contains_key(*key))
            .collect();
        for key in extra {
            failures.push(format!(
                "v{target}: migrated store carries {key}, which a fresh store does not"
            ));
        }
        for (key, expected_sql) in &expected_objects {
            if let Some(actual_sql) = objects.get(key) {
                if actual_sql != expected_sql {
                    failures.push(format!(
                        "v{target}: {key} is defined differently\n     fresh: {expected_sql}\n  migrated: {actual_sql}"
                    ));
                }
            }
        }

        let columns = schema_columns(&conn);
        for (relation, expected) in &expected_columns {
            match columns.get(relation) {
                Some(actual) if actual == expected => {}
                Some(actual) => failures.push(format!(
                    "v{target}: {relation} columns are {actual:?}, fresh has {expected:?}"
                )),
                None => failures.push(format!("v{target}: {relation} is absent after migration")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "a migrated store diverges from a fresh one:\n  {}",
        failures.join("\n  ")
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A migration that recreates a table empty passes a shape check and loses the
/// repository's index. Every rung must carry its rows across.
#[test]
fn migrating_from_any_rung_keeps_every_row() {
    let dir = tmp_dir("ladder-rows");

    // The census a fresh, seeded store carries, taken before any reduction.
    let reference_path = dir.join("reference.sqlite");
    seed_current_store(&reference_path);
    let reference = Connection::open(&reference_path).unwrap();
    let expected = row_census(&reference);
    drop(reference);

    let mut failures = Vec::new();
    for target in OLDEST_REDUCIBLE..CURRENT_SCHEMA_VERSION {
        let db_path = store_at_version(&dir, target);
        let Ok(store) = Store::open(&db_path) else {
            continue;
        };
        drop(store);
        let conn = Connection::open(&db_path).unwrap();
        for (table, count) in row_census(&conn) {
            // A rung below the one that created a table legitimately has no
            // rows in it: the reduction dropped the table, and the migration
            // recreates it empty. Only tables that existed at `target` carry an
            // expectation.
            let created_at = match table {
                "build_history" => 6,
                "generation_unresolved" => 9,
                "generation_coverage_gaps" => 14,
                _ => OLDEST_REDUCIBLE,
            };
            if target < created_at {
                continue;
            }
            let want = expected[table];
            if count != want {
                failures.push(format!(
                    "v{target}: {table} has {count} rows after migrating, seeded with {want}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "the ladder lost rows:\n  {}",
        failures.join("\n  ")
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The ladder must be walkable twice: a store migrated to current and reopened
/// takes no step at all, and nothing in the chain is order-dependent on having
/// just run.
#[test]
fn a_migrated_store_reopens_without_migrating_again() {
    let dir = tmp_dir("ladder-idempotent");
    for target in OLDEST_REDUCIBLE..CURRENT_SCHEMA_VERSION {
        let db_path = store_at_version(&dir, target);
        let first = Store::open(&db_path).unwrap_or_else(|error| {
            panic!("v{target} must open: {error}");
        });
        drop(first);
        let before = {
            let conn = Connection::open(&db_path).unwrap();
            schema_objects(&conn)
        };
        let second = Store::open(&db_path)
            .unwrap_or_else(|error| panic!("v{target} must reopen after migrating: {error}"));
        drop(second);
        let after = {
            let conn = Connection::open(&db_path).unwrap();
            schema_objects(&conn)
        };
        assert_eq!(
            before.keys().collect::<BTreeSet<_>>(),
            after.keys().collect::<BTreeSet<_>>(),
            "reopening a store migrated from v{target} changed its schema"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
