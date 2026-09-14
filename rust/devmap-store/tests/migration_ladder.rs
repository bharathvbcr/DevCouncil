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

        -- The ledger's three interned columns, seeded as v22 stores them. The
        -- reason and the classification go into the pool the same statement
        -- reads them back out of, so the reduction down to v21 has real text to
        -- materialise rather than a dangling id.
        INSERT INTO unresolved_texts (text) VALUES ('no candidate'), ('unresolved');
        INSERT INTO unresolved_rows
            (source_file_id, source_symbol, callee_name, reason_id, classification_id, receiver,
             valid_from, valid_to)
        VALUES (3, 'b.py::main', 'mystery',
                (SELECT id FROM unresolved_texts WHERE text = 'no candidate'),
                (SELECT id FROM unresolved_texts WHERE text = 'unresolved'),
                'obj', 1, NULL);

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
        // MIGRATION_V21_TO_V22: the ledger's interned columns. Materialise the
        // three ids back into the text they stand for, drop the pool, and put
        // the v18 view back over the flat table.
        //
        // `paths` is *not* reduced with them: it long predates v22 and the rows
        // the ledger referenced were already there for the nodes and edges. The
        // pool is v22's own and goes.
        22 => {
            "CREATE TABLE unresolved_rows_v21 (
                 unresolved_id  INTEGER PRIMARY KEY,
                 source_file    TEXT NOT NULL,
                 source_symbol  TEXT NOT NULL,
                 callee_name    TEXT NOT NULL,
                 reason         TEXT NOT NULL,
                 classification TEXT NOT NULL DEFAULT 'unresolved',
                 receiver       TEXT,
                 valid_from     INTEGER NOT NULL,
                 valid_to       INTEGER,
                 CHECK (valid_to IS NULL OR valid_to > valid_from)
             );
             INSERT INTO unresolved_rows_v21
                 (unresolved_id, source_file, source_symbol, callee_name,
                  reason, classification, receiver, valid_from, valid_to)
             SELECT u.unresolved_id, p.path, u.source_symbol, u.callee_name,
                    r.text, c.text, u.receiver, u.valid_from, u.valid_to
               FROM unresolved_rows u
               JOIN paths p            ON p.id = u.source_file_id
               JOIN unresolved_texts r ON r.id = u.reason_id
               JOIN unresolved_texts c ON c.id = u.classification_id;
             DROP VIEW IF EXISTS generation_unresolved;
             DROP TABLE unresolved_rows;
             DROP TABLE unresolved_texts;
             ALTER TABLE unresolved_rows_v21 RENAME TO unresolved_rows;
             CREATE INDEX idx_unresolved_rows_closed
                 ON unresolved_rows(valid_to) WHERE valid_to IS NOT NULL;
             CREATE VIEW generation_unresolved AS
             SELECT g.id             AS generation_id,
                    u.unresolved_id  AS ordinal,
                    u.source_file    AS source_file,
                    u.source_symbol  AS source_symbol,
                    u.callee_name    AS callee_name,
                    u.reason         AS reason,
                    u.classification AS classification,
                    u.receiver       AS receiver
               FROM unresolved_rows u
               JOIN generations g
                 ON g.id >= u.valid_from
                AND (u.valid_to IS NULL OR g.id < u.valid_to);"
        }
        // MIGRATION_V20_TO_V21: the two unread ledger indexes. The reduction
        // puts them back, so a "v20 store" reduced from current really does
        // carry what a v20 binary would have left behind — which is the whole
        // point of reducing rather than hand-building a legacy store, and the
        // only way `a_migrated_store_carries_the_same_schema_as_a_fresh_one`
        // can prove the rung actually drops them.
        21 => {
            "CREATE INDEX idx_unresolved_rows_callee
                 ON unresolved_rows(callee_name);
             CREATE INDEX idx_unresolved_rows_class
                 ON unresolved_rows(classification);"
        }
        20 => "DROP TABLE pending_state; ALTER TABLE pending_paths DROP COLUMN revision;",
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

/// A store an older build of the *same* version left without an index the
/// fresh schema declares heals at open, whatever rung it starts on.
///
/// Measured on this repository's own store on 2026-09-07: created at v17 by a
/// build whose v17 shape had no `idx_file_payloads_cache_identity`, it walked
/// 17→18→19 under the merged kernel and was then refused by the index gate —
/// and the refusal's remedy was `devmap build`, the command that had just
/// refused. The ladder only runs the rungs above the stamp; an index added to
/// the fresh schema *within* a version number is never on any of them. So the
/// open path recreates every declared index (all `IF NOT EXISTS`) before the
/// gate asks for them, and the gate is left to catch what could not be created.
#[test]
fn a_store_missing_a_declared_index_two_rungs_down_heals_at_open() {
    let dir = tmp_dir("heal-index-ladder");
    let db_path = dir.join("older-build.sqlite");
    seed_current_store(&db_path);
    {
        let conn = Connection::open(&db_path).unwrap();
        reduce_one_rung(&conn, CURRENT_SCHEMA_VERSION);
        reduce_one_rung(&conn, CURRENT_SCHEMA_VERSION - 1);
        conn.execute_batch("DROP INDEX idx_file_payloads_cache_identity;")
            .unwrap();
    }
    let store = Store::open(&db_path).expect("a missing declared index is recreated, not refused");
    drop(store);
    let conn = Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, CURRENT_SCHEMA_VERSION,
        "the ladder still ran to the top"
    );
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
             AND name = 'idx_file_payloads_cache_identity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        present, 1,
        "the index the older build never made exists after open"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The same for a store already stamped at the current version — the shape a
/// future same-version build leaves when it adds an index to the fresh schema.
#[test]
fn a_current_store_missing_a_declared_index_heals_at_open() {
    let dir = tmp_dir("heal-index-current");
    let db_path = dir.join("same-version.sqlite");
    seed_current_store(&db_path);
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("DROP INDEX idx_file_payloads_cache_identity;")
            .unwrap();
    }
    let store = Store::open(&db_path).expect("a current store missing a declared index heals");
    drop(store);
    let conn = Connection::open(&db_path).unwrap();
    let present: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
             AND name = 'idx_file_payloads_cache_identity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(present, 1);
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
/// Failure is induced with damage the open path cannot mend: a required
/// *column* is gone. (A missing declared index used to be the fixture here;
/// since the open path recreates those, it no longer fails the gate — see
/// `a_store_missing_a_declared_index_two_rungs_down_heals_at_open`.) The rung
/// the fixture starts on is `CURRENT_SCHEMA_VERSION - 1` and moves with the
/// end of the chain — validation runs once, at the last step, because
/// `validate_schema` asserts the *current* schema and no earlier rung's shape
/// satisfies it.
#[test]
fn a_step_that_fails_its_gate_leaves_the_version_where_it_was() {
    let dir = tmp_dir("migration-halfway");
    let db_path = dir.join("halfway.sqlite");
    seed_current_store(&db_path);
    {
        let conn = Connection::open(&db_path).unwrap();
        reduce_one_rung(&conn, CURRENT_SCHEMA_VERSION);
        conn.execute_batch(&format!(
            "ALTER TABLE generations DROP COLUMN repo_root;
             PRAGMA user_version = {};",
            CURRENT_SCHEMA_VERSION - 1
        ))
        .unwrap();
    }

    let error = Store::open(&db_path)
        .err()
        .expect("a store missing a required column must not open");
    let text = error.to_string();
    assert!(
        text.contains("generations.repo_root"),
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
    // the refusal was about the column and not about some other damage.
    conn.execute_batch(devmap_store::MIGRATION_V6_TO_V7)
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

#[test]
fn concurrent_v19_openers_keep_pending_work_and_one_durable_identity() {
    let dir = tmp_dir("v20-concurrent");
    let db = store_at_version(&dir, 19);
    assert!(
        Store::open_read_only(&db).is_err(),
        "a reader must not migrate v19"
    );
    let barrier = std::sync::Barrier::new(16);
    std::thread::scope(|scope| {
        for _ in 0..16 {
            let barrier = &barrier;
            let db = &db;
            scope.spawn(move || {
                barrier.wait();
                let store = Store::open(db).unwrap();
                assert_eq!(store.get_pending_paths().unwrap(), vec!["later.py"]);
            });
        }
    });
    let store = Store::open(&db).unwrap();
    let watermark = store.pending_watermark().unwrap();
    let claim = store.claim_pending_batch(1).unwrap();
    drop(store);
    let reopened = Store::open(&db).unwrap();
    assert_eq!(watermark, reopened.pending_watermark().unwrap());
    reopened
        .enqueue_pending_paths(&["later.py".into()])
        .unwrap();
    assert_eq!(reopened.clear_claimed_pending_paths(&claim).unwrap(), 0);
    drop(reopened);
    fs::remove_dir_all(dir).unwrap();
}

/// One ledger row as the fixture below seeds it: file id, symbol, callee,
/// reason, classification, receiver, and the validity range.
type SeededRow = (
    i64,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    i64,
    Option<i64>,
);

/// One ledger row as the view projects it, for comparison across a migration.
type LedgerRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

/// Every row `generation_unresolved` returns, in a fixed order.
///
/// Ordered by `(generation_id, ordinal)` and compared as a sequence rather than
/// as a set, deliberately. The fixture below contains two rows that are each
/// other's transposition, so a migration that swapped a pair of columns would
/// leave the *set* of rows untouched and only a per-row comparison can see it.
fn ledger_view(conn: &Connection) -> Vec<LedgerRow> {
    let mut stmt = conn
        .prepare(
            "SELECT generation_id, ordinal, source_file, source_symbol,
                    callee_name, reason, classification, receiver
               FROM generation_unresolved
              ORDER BY generation_id, ordinal",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

/// **R16.** A migration must preserve what the view *returns*, not only how
/// many rows it returns.
///
/// v22's backfill joins `unresolved_texts` twice — once for `reason`, once for
/// `classification` — and both columns are `TEXT NOT NULL`:
///
/// ```sql
/// JOIN unresolved_texts r ON r.text = u.reason
/// JOIN unresolved_texts c ON c.text = u.classification
/// ```
///
/// Transpose those two and every row survives and every count matches, so
/// `migrating_from_any_rung_keeps_every_row` cannot see it. `source_symbol` and
/// `callee_name` are the same hazard one column over.
///
/// # What this adds, measured rather than assumed
///
/// Transposing `r.id`/`c.id` in the backfill and running this file was tried.
/// Three results, and the middle one corrects the obvious motivation for this
/// test:
///
/// * `migrating_from_any_rung_keeps_every_row` — **passed**. Counting is blind,
///   as expected.
/// * `a_v17_store_with_two_generations_carries_both_onto_ranges` — **failed**.
///   It compares `unresolved_by_generation` across the whole chain, so a plain
///   transposition was *already* caught. This test is not closing an open hole;
///   it is narrowing an incidental catch into a direct one.
/// * this test — **failed**, at the view comparison.
///
/// So the value here is the fixture, not the comparison. The v17 test inherits
/// `seed_current_store`'s single ledger row: one file, no closed rows, ASCII
/// text, and a `reason` and `classification` that no other row uses. This one
/// is built to be hostile on the axes that row cannot reach — rows 1 and 2
/// carry each other's `reason`/`classification` *and* each other's
/// `source_symbol`/`callee_name`, so both texts are in the pool either way and
/// a transposed join still finds a match, which isolates "transposed" from
/// "failed to look up"; row 3 carries bytes hostile to string handling; row 4
/// shares row 1's reason so two rows must resolve to one pool entry; and row 5
/// closes at generation 2, so the view's range scoping is exercised rather
/// than assumed. Rows are compared as an ordered sequence keyed by ordinal,
/// which is what lets the transposition twins be a fixture rather than a
/// blind spot.
#[test]
fn migrating_the_ledger_preserves_every_column_and_not_only_the_count() {
    let dir = tmp_dir("v22-content-parity");
    let db_path = dir.join("index.sqlite");
    drop(Store::open(&db_path).expect("a fresh store must open"));

    let hostile =
        "quote\" backslash\\ newline\n tab\t unicode \u{00e9}\u{4e2d} emoji \u{1f600} \u{1}\u{7f}";

    // (file_id, source_symbol, callee_name, reason, classification, receiver, valid_from, valid_to)
    let seeded: Vec<SeededRow> = vec![
        (
            1,
            "sym-one",
            "callee-one",
            "reason-A",
            "class-B",
            Some("recv"),
            1,
            None,
        ),
        // The transposition twin of row 1.
        (
            2,
            "callee-one",
            "sym-one",
            "class-B",
            "reason-A",
            None,
            1,
            None,
        ),
        (
            1,
            hostile,
            "callee-hostile",
            hostile,
            "class-B",
            Some(hostile),
            1,
            None,
        ),
        // Shares row 1's reason, so the pool must hold one entry for two rows.
        (
            2,
            "sym-four",
            "callee-four",
            "reason-A",
            "class-four",
            None,
            1,
            None,
        ),
        // Closed at generation 2: visible from generation 1 only.
        (
            1,
            "sym-five",
            "callee-five",
            "reason-five",
            "class-five",
            Some(""),
            1,
            Some(2),
        ),
    ];

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            INSERT INTO paths (id, path) VALUES (1, 'alpha.py'), (2, 'beta.py');
            INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
            VALUES (1, 1.0, 'deadbeef', '/tmp/probe',
                    '{"total_files":2,"total_symbols":0,"total_edges":0,
                      "dead_symbols":[],"communities":[],"status":"Ok"}'),
                   (2, 2.0, 'cafebabe', '/tmp/probe',
                    '{"total_files":2,"total_symbols":0,"total_edges":0,
                      "dead_symbols":[],"communities":[],"status":"Ok"}');
            "#,
        )
        .unwrap();

        let intern = |text: &str| -> i64 {
            conn.execute(
                "INSERT OR IGNORE INTO unresolved_texts (text) VALUES (?1)",
                [text],
            )
            .unwrap();
            conn.query_row(
                "SELECT id FROM unresolved_texts WHERE text = ?1",
                [text],
                |row| row.get(0),
            )
            .unwrap()
        };

        for (ordinal, (file, symbol, callee, reason, class, receiver, from, to)) in
            seeded.iter().enumerate()
        {
            conn.execute(
                "INSERT INTO unresolved_rows
                     (unresolved_id, source_file_id, source_symbol, callee_name,
                      reason_id, classification_id, receiver, valid_from, valid_to)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    ordinal as i64 + 1,
                    file,
                    symbol,
                    callee,
                    intern(reason),
                    intern(class),
                    receiver,
                    from,
                    to
                ],
            )
            .unwrap();
        }
    }

    let (before, pool_before) = {
        let conn = Connection::open(&db_path).unwrap();
        let pool: i64 = conn
            .query_row("SELECT COUNT(*) FROM unresolved_texts", [], |row| {
                row.get(0)
            })
            .unwrap();
        (ledger_view(&conn), pool)
    };

    // The fixture has to actually reach the view, or every assertion below is
    // satisfied by two empty vectors. Rows 1-4 are live and so appear under
    // both generations; row 5 closes at 2 and appears under generation 1 only.
    assert_eq!(
        before.len(),
        9,
        "the fixture must reach the view before anything is migrated: {before:#?}"
    );
    let distinct_texts: BTreeSet<&str> = seeded
        .iter()
        .flat_map(|(_, _, _, reason, class, _, _, _)| [*reason, *class])
        .collect();
    assert_eq!(
        pool_before,
        distinct_texts.len() as i64,
        "the fixture's own pool must already be deduped"
    );

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        reduce_one_rung(&conn, CURRENT_SCHEMA_VERSION);
        let stamped: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stamped, CURRENT_SCHEMA_VERSION - 1);
        // The reduction must really have produced the flat pre-v22 shape,
        // otherwise the reopen below migrates nothing and this test compares a
        // store with itself.
        let flat: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('unresolved_rows') WHERE name = 'reason'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(flat, 1, "the reduction must leave a flat `reason` column");
    }

    // Opening walks the v21 rung, which is the statement under test.
    drop(Store::open(&db_path).expect("a reduced store must migrate and open"));

    let conn = Connection::open(&db_path).unwrap();
    let interned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('unresolved_rows') WHERE name = 'reason_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(interned, 1, "the reopen must have run the v22 rung");

    assert_eq!(
        ledger_view(&conn),
        before,
        "the v22 backfill changed what the ledger says"
    );

    // Defensive, and not load-bearing today -- said plainly because the
    // comment that first stood here claimed otherwise.
    //
    // `unresolved_texts.text` is `NOT NULL UNIQUE`, so the pool cannot hold a
    // duplicate however the backfill is written, and this assertion is
    // satisfied for free. Dropping the `UNIQUE` together with the backfill's
    // `DISTINCT`/`OR IGNORE` was tried: this test does fail, but at the
    // fixture's own pool guard above rather than here, because the fixture
    // interns through the same constraint.
    //
    // It stays because it is the only statement in the suite that says the
    // pool is one row per distinct text, and the day the `UNIQUE` is relaxed
    // for some other reason it becomes the thing that notices.
    let pool_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM unresolved_texts", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        pool_after, pool_before,
        "the migrated pool must hold one row per distinct text"
    );

    let _ = fs::remove_dir_all(&dir);
}
