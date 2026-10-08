//! `backend/contracts/lease.schema.md` against the DDL this crate applies.
//!
//! The contract says it describes the tables "as implemented, not as
//! intended", and that where a consumer disagrees, `schema.rs` wins. Nothing
//! held the document to that: GitPulse vendors it, and the only checks on it
//! were checksums of the document against itself. This executes the document's
//! own SQL blocks and compares the result with a store `dc_store::Store::open`
//! actually created — columns, types, nullability, defaults, keys and every
//! index on each documented table — so the document cannot describe a table
//! the code stopped building.

use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn contract_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../backend/contracts/lease.schema.md")
}

/// The SQL in every ```sql fence, concatenated.
fn sql_blocks(markdown: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in markdown.lines() {
        let fence = line.trim_start();
        if !inside && fence.starts_with("```sql") {
            inside = true;
        } else if inside && fence.starts_with("```") {
            inside = false;
            out.push('\n');
        } else if inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

type Columns = Vec<(String, String, bool, Option<String>, i64)>;

#[derive(Debug, PartialEq)]
struct TableShape {
    columns: Columns,
    /// index name -> normalised CREATE statement
    indexes: BTreeMap<String, String>,
}

fn normalise(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(" IF NOT EXISTS", "")
}

fn shape(conn: &Connection, table: &str) -> TableShape {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_xinfo(\"{table}\")"))
        .unwrap();
    let columns = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT name, sql FROM sqlite_master
             WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL",
        )
        .unwrap();
    let indexes = stmt
        .query_map([table], |row| {
            Ok((row.get::<_, String>(0)?, normalise(&row.get::<_, String>(1)?)))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    TableShape { columns, indexes }
}

/// Every difference between what the document builds and what the crate builds,
/// for each table the document creates.
fn drift(document_sql: &str, store: &Connection) -> (Vec<String>, Vec<String>) {
    let documented = Connection::open_in_memory().unwrap();
    documented
        .execute_batch(document_sql)
        .expect("the contract's SQL must itself execute");
    let tables: Vec<String> = documented
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let mut problems = Vec::new();
    for table in &tables {
        let want = shape(&documented, table);
        let have = shape(store, table);
        if have.columns.is_empty() {
            problems.push(format!("{table}: documented, but the store has no such table"));
            continue;
        }
        if want.columns != have.columns {
            problems.push(format!(
                "{table}: columns differ\n    document: {:?}\n    store:    {:?}",
                want.columns, have.columns
            ));
        }
        if want.indexes != have.indexes {
            problems.push(format!(
                "{table}: indexes differ\n    document: {:?}\n    store:    {:?}",
                want.indexes, have.indexes
            ));
        }
    }
    (tables, problems)
}

fn real_store() -> (tempfile_dir::Dir, Connection) {
    let dir = tempfile_dir::Dir::new();
    let path = dir.0.join("state.sqlite");
    drop(dc_store::Store::open(&path).expect("dc-store creates its schema"));
    let conn = Connection::open(&path).unwrap();
    (dir, conn)
}

#[test]
fn the_lease_contract_describes_the_tables_dc_store_builds() {
    let markdown = std::fs::read_to_string(contract_path())
        .unwrap_or_else(|err| panic!("reading {}: {err}", contract_path().display()));
    let (_dir, store) = real_store();
    let (tables, problems) = drift(&sql_blocks(&markdown), &store);
    // The document creates `tasks` and `task_leases`. Finding fewer means the
    // fence parse stopped working, and an empty comparison must not pass.
    assert!(
        tables.len() >= 2,
        "found only {tables:?} in the contract's SQL; the parse is broken"
    );
    assert!(
        problems.is_empty(),
        "lease.schema.md no longer describes what dc-store builds — update the \
         document (schema.rs wins):\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn a_drifted_document_is_reported() {
    let markdown = std::fs::read_to_string(contract_path()).unwrap();
    let sql = sql_blocks(&markdown);
    let (_dir, store) = real_store();
    let dropped_column = sql.replacen("    released_at VARCHAR,\n", "", 1);
    assert_ne!(dropped_column, sql, "fixture: the column to drop must exist");
    let (_, problems) = drift(&dropped_column, &store);
    assert!(
        problems.iter().any(|p| p.starts_with("task_leases: columns differ")),
        "a dropped column must be reported: {problems:?}"
    );
    let dropped_index = sql.replacen(
        "CREATE INDEX IF NOT EXISTS ix_task_leases_status ON task_leases (status);",
        "",
        1,
    );
    assert_ne!(dropped_index, sql, "fixture: the index to drop must exist");
    let (_, problems) = drift(&dropped_index, &store);
    assert!(
        problems.iter().any(|p| p.starts_with("task_leases: indexes differ")),
        "a missing index must be reported: {problems:?}"
    );
}

/// A scratch directory without a dev-dependency.
mod tempfile_dir {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    pub struct Dir(pub PathBuf);

    impl Dir {
        pub fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "dc-store-lease-contract-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
