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
/// `test_s2_migration_v3_to_v4_preserves_cache_rows` and its neighbour in
/// `store_hardening.rs` and
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

        -- `generation_edges` and `generation_unresolved` are views over
        -- validity ranges since v18 and are not insertable; the rows live in
        -- `edge_rows` and `unresolved_rows`. `valid_to` NULL means "still
        -- valid", which for a store with one generation is every row.
        INSERT INTO edge_rows
            (source_file_id, target_file_id, source_symbol, target_symbol,
             edge_kind, confidence, resolution, candidate_total, valid_from, valid_to)
        VALUES (3, 1, 'b.py::main', 'a.py::helper', 'Calls', 0.9, 'Exact', NULL, 1, NULL);

        INSERT INTO unresolved_rows
            (source_file, source_symbol, callee_name, reason, classification, receiver,
             valid_from, valid_to)
        VALUES ('b.py', 'b.py::main', 'mystery', 'no candidate', 'unresolved', 'obj', 1, NULL);

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
        // MIGRATION_V18_TO_V19: the per-file row digests. Purely additive, so
        // the reduction is the table and nothing else — there is no backfill to
        // undo, which is the property that lets a v18 store migrate by gaining
        // an empty table and comparing every row on its next build.
        19 => "DROP TABLE generation_file_digests;",
        // MIGRATION_V17_TO_V18: the validity ranges. Materialise both views
        // back into the base tables they replaced, restore the two edge indexes
        // v5 created on `generation_edges`, and drop the range tables with the
        // indexes SQLite dropped along with them.
        //
        // `generation_edges.ordinal` comes back as the view's `ordinal`, which
        // is the range row's own id. That is not the resolver's emission order
        // any more and nothing reads it as one — `edge_read_order` took
        // `resolution` as its last key in the same change — so the reduced store
        // is a v17 store in every respect the ladder asserts.
        18 => {
            "CREATE TABLE ge_flat AS SELECT * FROM generation_edges;
             CREATE TABLE gu_flat AS SELECT * FROM generation_unresolved;
             DROP VIEW generation_edges;
             DROP VIEW generation_unresolved;
             DROP TABLE edge_rows;
             DROP TABLE unresolved_rows;
             ALTER TABLE ge_flat RENAME TO generation_edges;
             ALTER TABLE gu_flat RENAME TO generation_unresolved;
             CREATE INDEX idx_generation_edges_source
                 ON generation_edges(generation_id, source_file_id);
             CREATE INDEX idx_generation_edges_target
                 ON generation_edges(generation_id, target_file_id);
             CREATE INDEX idx_generation_unresolved_callee
                 ON generation_unresolved(generation_id, callee_name);
             CREATE INDEX idx_generation_unresolved_class
                 ON generation_unresolved(generation_id, classification);"
        }
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
/// This is the assertion that was missing.
/// `test_s2_migration_v3_to_v4_preserves_cache_rows` builds
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

// --- the ladder under duress ------------------------------------------------
//
// The four tests above walk the chain the way it is meant to be walked. These
// three attack it: a step that cannot finish, several processes walking it at
// once, and a database carrying objects the schema never declared. Each is a
// real shape on a user's disk — an interrupted build, a daemon racing an editor
// hook, an operator's ad-hoc index — and none of them was covered.

/// A step that fails its own gate must not advance `user_version`.
///
/// The last arm of the chain stamps its version and *then* validates, both
/// inside one transaction, so a failed validation has to take the stamp down
/// with it. That rests on `PRAGMA user_version` being transactional in SQLite —
/// true, and load-bearing enough to be worth a test rather than a comment: if
/// it ever were not, a store that failed validation would reopen claiming to be
/// at the top, skip the chain entirely, and every later read would run against
/// a shape nothing had checked.
///
/// Failure is induced the way it actually happens: a step's idempotency probe
/// sees the work already done, skips its DDL batch, and an object a previous
/// partial attempt never created stays missing. The rung the fixture starts on
/// is `CURRENT_SCHEMA_VERSION - 1` and moves with the end of the chain —
/// validation runs once, at the last step, because `validate_schema` asserts
/// the *current* schema and no earlier rung's shape satisfies it.
#[test]
fn a_step_that_fails_its_gate_leaves_the_version_where_it_was() {
    let dir = tmp_dir("migration-halfway");
    let db_path = dir.join("halfway.sqlite");
    seed_current_store(&db_path);
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(&format!(
            "DROP INDEX idx_file_payloads_cache_identity;
             PRAGMA user_version = {};",
            CURRENT_SCHEMA_VERSION - 1
        ))
        .unwrap();
    }

    let error = Store::open(&db_path)
        .err()
        .expect("a store missing a declared index must not open");
    let text = error.to_string();
    assert!(
        text.contains("idx_file_payloads_cache_identity"),
        "the refusal must name what is missing, or an operator cannot act on \
         it: {text}"
    );

    let conn = Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version,
        CURRENT_SCHEMA_VERSION - 1,
        "the step failed, so the store is still at the rung it started on; a \
         stamp that survived its own failed validation would make the next \
         open skip the chain and trust an unchecked shape"
    );
    // And the failure left nothing else behind: the store is exactly what it
    // was, so a rebuild has a clean base.
    let is_view: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
              WHERE name = 'generation_files' AND type = 'view'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(is_view, 1, "the rollback must not have unmade the view");

    // The same store opens once the missing object is restored, which proves
    // the refusal was about the index and not about some other damage.
    conn.execute_batch(
        "CREATE INDEX idx_file_payloads_cache_identity
             ON file_payloads(content_hash, language, grammar_version, analyzer_version);",
    )
    .unwrap();
    drop(conn);
    let store = Store::open(&db_path).expect("with the index back, the step completes");
    drop(store);
    let conn = Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let _ = fs::remove_dir_all(&dir);
}

/// Several openers walking the chain at once all reach the top, and the rows
/// survive.
///
/// Only the `version == 0` branch re-reads the version under the write lock;
/// every rung above it samples once and runs its step, so two openers at the
/// same rung both execute it — the second against a database where the work is
/// already done. That is exactly what each step's idempotency probe exists for,
/// and it had never been exercised *concurrently*: the re-entrancy test in
/// `store_hardening.rs` re-runs steps one after another in a single thread.
///
/// A daemon, an editor hook and a manual `devmap build` opening the same store
/// on the first run after an upgrade is the ordinary case, not an exotic one.
#[test]
fn concurrent_openers_of_a_legacy_store_all_reach_the_current_schema() {
    let dir = tmp_dir("migration-race");
    let db_path = store_at_version(&dir, OLDEST_REDUCIBLE);
    let expected = {
        // What one uncontended migration produces, as the yardstick.
        let reference_dir = tmp_dir("migration-race-reference");
        let reference = store_at_version(&reference_dir, OLDEST_REDUCIBLE);
        let store = Store::open(&reference).expect("reference migration");
        drop(store);
        let conn = Connection::open(&reference).unwrap();
        let census = row_census(&conn);
        drop(conn);
        let _ = fs::remove_dir_all(&reference_dir);
        census
    };

    let outcomes: Vec<std::result::Result<(), String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = db_path.clone();
                scope.spawn(move || {
                    Store::open(&path)
                        .map(drop)
                        .map_err(|error| error.to_string())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("no opener may panic"))
            .collect()
    });
    let failures: Vec<&String> = outcomes.iter().filter_map(|r| r.as_ref().err()).collect();
    assert!(
        failures.is_empty(),
        "every opener must reach the current schema; a step that is not \
         re-entrant under contention fails only some of them, which is the \
         hardest kind of failure to reproduce: {failures:?}"
    );

    let conn = Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    assert_eq!(
        row_census(&conn),
        expected,
        "a contended migration must move the same rows as an uncontended one; \
         a step run twice that appends rather than probes shows up here as a \
         doubled count"
    );
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

/// Objects the schema never declared are left alone, not treated as damage.
///
/// The index gate added in this pass asserts that every *declared* index is
/// present. The inverse — that nothing undeclared may exist — would be a very
/// different and much worse contract: an operator's ad-hoc index, a leftover
/// table from a rolled-back experiment, or a shadow relation from a tool
/// nothing here owns would all become an unopenable store.
///
/// The OFF direction for the gate, in other words. Without it the cheapest way
/// to write the gate — set equality — passes every other test in this file.
#[test]
fn objects_the_schema_does_not_declare_do_not_fail_the_gate() {
    let dir = tmp_dir("migration-extras");
    let db_path = store_at_version(&dir, OLDEST_REDUCIBLE);
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE operator_scratch (id INTEGER PRIMARY KEY, note TEXT);
             CREATE INDEX idx_operator_scratch_note ON operator_scratch(note);
             CREATE INDEX idx_paths_path_extra ON paths(path);",
        )
        .unwrap();
    }

    let store = Store::open(&db_path).unwrap_or_else(|error| {
        panic!("undeclared objects must not make a store unopenable: {error}")
    });
    drop(store);

    let conn = Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let survived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'operator_scratch'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        survived, 1,
        "and the migration must not delete what it does not own"
    );
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

/// A v17 store with **two** generations keeps both of them across v18.
///
/// The rest of this file walks the ladder over a store with one generation,
/// which is exactly the shape that cannot see the v17→v18 backfill's only real
/// decision: what `valid_to` is for a row whose generation some later
/// generation also has. Every row of a one-generation store is still valid, so
/// the whole `MIN(g.id) WHERE g.id > e.generation_id` expression reads NULL and
/// any wrong answer passes.
///
/// The rows are carried as they stand — each generation's set becoming
/// `[g, next_g)`, so an edge present in both generations becomes two rows and
/// the store is no smaller the moment it migrates. That is asserted here rather
/// than merely intended: collapsing them would mean deciding in SQL which of
/// two generations' rows are "the same edge", which is what the *write* path
/// computes from a freshly resolved multiset and what a migration has no
/// business inventing.
#[test]
fn a_v17_store_with_two_generations_carries_both_onto_ranges() {
    let dir = tmp_dir("v18-backfill-two-generations");
    let db_path = dir.join("v17-two.sqlite");
    seed_current_store(&db_path);

    let conn = Connection::open(&db_path).unwrap();
    // Down to 17 however far the top of the ladder has moved: this test is
    // about the v17→v18 backfill specifically, and every rung above it has to
    // come off first.
    let mut version = CURRENT_SCHEMA_VERSION;
    while version > 17 {
        reduce_one_rung(&conn, version);
        version -= 1;
    }
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 17, "the fixture must be a v17 store");

    // A second generation that shares one edge with the first and adds one, so
    // the carried row and the closed row are both exercised. The unresolved
    // ledger keeps its single row across both, which is the case the measured
    // corpus is made almost entirely of.
    conn.execute_batch(
        r#"
        INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
        VALUES (2, 2.0, 'cafebabe', '/tmp/probe', '{"total_files":3,"total_symbols":2,
                "total_edges":2,"dead_symbols":[],"communities":[],"status":"Ok"}');

        INSERT INTO generation_edges
            (generation_id, ordinal, source_file_id, target_file_id, source_symbol,
             target_symbol, edge_kind, confidence, resolution, candidate_total)
        VALUES (2, 0, 3, 1, 'b.py::main', 'a.py::helper', 'Calls', 0.9, 'Exact', NULL),
               (2, 1, 1, 3, 'a.py::helper', 'b.py::main', 'References', 0.8, 'UniqueGlobal', NULL);

        INSERT INTO generation_unresolved
            (generation_id, ordinal, source_file, source_symbol, callee_name,
             reason, classification, receiver)
        VALUES (2, 0, 'b.py', 'b.py::main', 'mystery', 'no candidate',
                'unresolved', 'obj');
        "#,
    )
    .expect("seeding a second v17 generation must succeed");

    let before_edges = edges_by_generation(&conn);
    let before_unresolved = unresolved_by_generation(&conn);
    assert_eq!(
        before_edges.len(),
        2,
        "the fixture must have two generations"
    );
    drop(conn);

    let store = Store::open(&db_path).expect("a two-generation v17 store must migrate");
    drop(store);

    let conn = Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    assert_eq!(
        edges_by_generation(&conn),
        before_edges,
        "a generation must read back exactly what it held before the migration"
    );
    assert_eq!(
        unresolved_by_generation(&conn),
        before_unresolved,
        "and so must its unresolved ledger"
    );

    // The ranges themselves: the edge only generation 1 had is closed at 2, the
    // two generation 2 has are open, and the shared edge is two rows.
    let mut ranges: Vec<(i64, Option<i64>, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT valid_from, valid_to, source_symbol || '>' || target_symbol
                   FROM edge_rows",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    ranges.sort();
    assert_eq!(
        ranges,
        vec![
            (1, Some(2), "b.py::main>a.py::helper".to_string()),
            (2, None, "a.py::helper>b.py::main".to_string()),
            (2, None, "b.py::main>a.py::helper".to_string()),
        ],
        "the backfill must give each generation's rows the half-open range \
         [g, next_g), and must not collapse the edge both generations hold"
    );
    let ledger: Vec<(i64, Option<i64>)> = {
        let mut stmt = conn
            .prepare("SELECT valid_from, valid_to FROM unresolved_rows ORDER BY valid_from")
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows
    };
    assert_eq!(ledger, vec![(1, Some(2)), (2, None)]);
    drop(conn);
    let _ = fs::remove_dir_all(&dir);
}

/// Every generation's edges, keyed by generation, in a form that compares
/// across the migration: `ordinal` is deliberately absent, because v18 replaces
/// the resolver's emission ordinal with the range row's own id and nothing
/// reads it as a position.
fn edges_by_generation(conn: &Connection) -> BTreeMap<i64, Vec<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT generation_id, source_file_id, target_file_id, source_symbol,
                    target_symbol, edge_kind, printf('%.17g', confidence),
                    COALESCE(resolution, '<none>'),
                    COALESCE(CAST(candidate_total AS TEXT), '<none>')
               FROM generation_edges",
        )
        .unwrap();
    let mut out: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                format!(
                    "{}|{}|{}|{}|{}|{}|{}|{}",
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ),
            ))
        })
        .unwrap();
    for row in rows {
        let (generation, text) = row.unwrap();
        out.entry(generation).or_default().push(text);
    }
    for rows in out.values_mut() {
        rows.sort();
    }
    out
}

fn unresolved_by_generation(conn: &Connection) -> BTreeMap<i64, Vec<String>> {
    let mut stmt = conn
        .prepare(
            "SELECT generation_id, source_file, source_symbol, callee_name, reason,
                    classification, COALESCE(receiver, '<none>')
               FROM generation_unresolved",
        )
        .unwrap();
    let mut out: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                format!(
                    "{}|{}|{}|{}|{}|{}",
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ),
            ))
        })
        .unwrap();
    for row in rows {
        let (generation, text) = row.unwrap();
        out.entry(generation).or_default().push(text);
    }
    for rows in out.values_mut() {
        rows.sort();
    }
    out
}
