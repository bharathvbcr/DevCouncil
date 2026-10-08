//! Page-size, vacuum-cap and declared-index contracts that need only the
//! store's public API.
//!
//! Moved out of `db.rs`'s inline `connection_tests` module. The tests that
//! stayed there read the private connection, schema gate or vacuum budget, so
//! they live in `src/db/tests/connection_tests.rs` as a child of `db` instead.

use devmap_store::{declared_index_names, declared_index_statements, Store};
use rusqlite::Connection;

fn scratch(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("devmap-{label}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// A page size larger than the whole budget still reclaims something.
///
/// `bytes / page_size` is zero once the page exceeds the budget, and a
/// request of zero pages would step the pragma zero times and then report a
/// bounded reclaim — a check that could not run reporting as one that ran.
/// Zero and negative are included because the value is read from
/// `PRAGMA page_size` at runtime, not from a constant.
#[test]
fn the_reclaim_cap_never_requests_nothing() {
    for page in [0, -1, i64::MAX, 1 << 30] {
        assert!(
            Store::incremental_vacuum_max_pages(page) >= 1,
            "page size {page} must still request at least one page"
        );
    }
}

/// A plain `VACUUM` does not convert an existing store's page size.
///
/// This pins the assumption the comment on the `page_size` pragma makes,
/// because getting it wrong is silent: `VACUUM` adopts a pending
/// `auto_vacuum`, so "a full vacuum converts it" reads as true for both
/// settings and is only true for one. SQLite will not change `page_size` on
/// a WAL database, and reports no error when it declines.
///
/// Both halves are asserted — that the plain vacuum leaves 4 KiB, and that
/// leaving WAL for the rewrite is what actually converts — so the remedy in
/// that comment is executable rather than remembered.
#[test]
fn a_plain_vacuum_does_not_convert_an_existing_page_size() {
    let dir = scratch("pagesize-vacuum");
    let path = dir.join("devmap.sqlite");
    {
        let conn = Connection::open(&path).expect("seed connection");
        conn.pragma_update(None, "page_size", 4096).expect("4 KiB");
        conn.execute_batch("CREATE TABLE seed (x INTEGER); DROP TABLE seed;")
            .expect("fix the page size into the file header");
    }
    // Opening puts it in WAL, which is the state a real store is in.
    drop(Store::open(&path).expect("store"));

    let conn = Connection::open(&path).expect("connection");
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal_mode");
    assert_eq!(
        mode.to_lowercase(),
        "wal",
        "the store under test must be WAL"
    );

    conn.execute_batch("PRAGMA page_size=16384; VACUUM;")
        .expect("a plain vacuum must succeed, not error");
    let after_plain: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .expect("page_size");
    assert_eq!(
        after_plain, 4096,
        "a plain VACUUM on a WAL database leaves the page size alone — it does \
             not report failure, which is why the claim that it converts survives"
    );

    conn.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA page_size=16384; VACUUM; PRAGMA journal_mode=WAL;",
    )
    .expect("the documented conversion must succeed");
    let after_documented: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .expect("page_size");
    assert_eq!(
        after_documented, 16384,
        "leaving WAL for the rewrite is what actually converts the page size"
    );
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The index gate's expectation is parsed out of DDL, so the parse itself
/// needs pinning against what SQLite actually built.
///
/// The statements replayed at open name exactly the indexes the gate
/// demands, and every one is `IF NOT EXISTS` — a replay on a complete
/// store must be a no-op, or healing would break what it meant to mend.
#[test]
fn every_declared_index_statement_is_idempotent_and_names_a_gated_index() {
    let statements = declared_index_statements();
    let mut gated = declared_index_names();
    gated.sort();
    let mut named: Vec<String> = statements
        .iter()
        .map(|statement| {
            assert!(
                statement.contains("IF NOT EXISTS"),
                "a replayed statement must be idempotent: {statement}"
            );
            assert!(statement.ends_with(';'), "{statement}");
            statement
                .split("IF NOT EXISTS ")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    named.sort();
    assert_eq!(
        named, gated,
        "the statements replayed at open and the names the gate demands must be one set"
    );
}
