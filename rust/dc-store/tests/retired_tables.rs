//! Every table `schema.rs` creates has a non-test reader or writer, and a
//! store from before the retirement still opens.
//!
//! The retired tables are not in the DDL. An older file still has them, so
//! open has to drop them and leave everything else where it was. Dropping them
//! needs no version of its own: the only stamp a v9 store receives on open is
//! schema 10's, from retiring the legacy tasks table into workbench items.

use dc_store::{SCHEMA_VERSION, Store, schema};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};

mod support;
use support::LEGACY_TASKS;

/// Planning state (`assumptions`, `project_state`, `critique_findings`) left
/// with the Python `planning/` package in Phase 7. The shell, file-change and
/// semantic-diff tables never grew a producer. `correction_manifests` was only
/// ever named by a comment.
const RETIRED: &[&str] = &[
    "assumptions",
    "critique_findings",
    "project_state",
    "shell_sessions",
    "shell_command_events",
    "file_change_events",
    "semantic_diffs",
    "correction_manifests",
];

/// A table with no non-test reader or writer stays out of the failure only
/// with a reason written here. An empty reason is itself a failure.
const RESERVED: &[(&str, &str)] = &[];

const PLANTED: &str = r#"
CREATE TABLE assumptions (
    id VARCHAR NOT NULL PRIMARY KEY,
    statement VARCHAR NOT NULL
);
INSERT INTO assumptions (id, statement) VALUES ('a', 'kept until open');
CREATE TABLE critique_findings (
    id VARCHAR NOT NULL PRIMARY KEY,
    claim VARCHAR NOT NULL
);
INSERT INTO critique_findings (id, claim) VALUES ('c', 'kept until open');
CREATE TABLE project_state (
    id VARCHAR NOT NULL PRIMARY KEY,
    current_phase VARCHAR NOT NULL
);
INSERT INTO project_state (id, current_phase) VALUES ('singleton', 'plan');
CREATE TABLE shell_sessions (
    id VARCHAR NOT NULL PRIMARY KEY,
    task_id VARCHAR NOT NULL
);
INSERT INTO shell_sessions (id, task_id) VALUES ('s', 'T');
CREATE TABLE shell_command_events (
    id INTEGER PRIMARY KEY,
    command VARCHAR NOT NULL
);
INSERT INTO shell_command_events (command) VALUES ('true');
CREATE TABLE file_change_events (
    id INTEGER PRIMARY KEY,
    path VARCHAR NOT NULL
);
INSERT INTO file_change_events (path) VALUES ('a.go');
CREATE TABLE semantic_diffs (
    id INTEGER PRIMARY KEY,
    summary VARCHAR NOT NULL
);
INSERT INTO semantic_diffs (summary) VALUES ('none');
CREATE TABLE correction_manifests (
    id VARCHAR NOT NULL PRIMARY KEY,
    task_id VARCHAR NOT NULL
);
INSERT INTO correction_manifests (id, task_id) VALUES ('m', 'T');
"#;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-store-retired-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

fn table_present(conn: &Connection, name: &str) -> bool {
    let found: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .unwrap();
    found != 0
}

fn created_tables(ddl: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in ddl.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("CREATE TABLE IF NOT EXISTS ") else {
            continue;
        };
        let name = rest
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .expect("CREATE TABLE names a table");
        names.push(name.to_string());
    }
    names
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn contains_word(hay: &str, word: &str) -> bool {
    let bytes = hay.as_bytes();
    let needle = word.as_bytes();
    if needle.is_empty() || needle.len() > bytes.len() {
        return false;
    }
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let before = i == 0 || !is_ident(bytes[i - 1]);
            let after = i + needle.len();
            let after_ok = after == bytes.len() || !is_ident(bytes[after]);
            if before && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn source_mentions(root: &Path, table: &str) -> bool {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name == "target" || name == "testdata" || name.starts_with('.') {
                    continue;
                }
                pending.push(path);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if ext != "rs" && ext != "go" {
                continue;
            }
            if name.ends_with("_test.go") || name == "schema.rs" {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if contains_word(&text, table) {
                return true;
            }
        }
    }
    false
}

#[test]
fn every_created_table_is_read_or_written_or_reserved() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .expect("manifest dir");
    let roots = [
        manifest.join("src"),
        manifest.join("../../backend/go_orchestrator"),
    ];
    for root in &roots {
        assert!(root.is_dir(), "source root {} is missing", root.display());
    }
    let tables = created_tables(schema::SCHEMA);
    assert!(
        tables.len() > 5,
        "the DDL parser saw {:?}, which is not the schema",
        tables
    );
    let mut reserved = std::collections::BTreeMap::new();
    for (name, reason) in RESERVED {
        assert!(!reason.is_empty(), "{name} is reserved with no reason");
        assert!(
            reserved.insert(*name, *reason).is_none(),
            "{name} is reserved twice"
        );
    }
    let mut unread = Vec::new();
    for table in &tables {
        if reserved.contains_key(table.as_str()) {
            continue;
        }
        let mentioned = roots.iter().any(|root| source_mentions(root, table));
        if !mentioned {
            unread.push(table.clone());
        }
    }
    assert!(
        unread.is_empty(),
        "tables with no non-test reader or writer, and no reserved reason: {unread:?}"
    );
    for name in reserved.keys() {
        assert!(
            tables.iter().any(|table| table == name),
            "{name} is reserved but schema.rs does not create it"
        );
    }
    for name in RETIRED {
        assert!(
            !tables.iter().any(|table| table == name),
            "{name} is retired and schema.rs still creates it"
        );
    }
}

#[test]
fn a_new_store_does_not_create_retired_tables() {
    let store = Store::open_in_memory().expect("open");
    for name in RETIRED {
        assert!(
            !table_present(store.connection(), name),
            "{name} was created"
        );
    }
    let version: i64 = store
        .connection()
        .query_row(
            "SELECT version FROM schema_version WHERE id = 'singleton'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, i64::from(SCHEMA_VERSION));
}

#[test]
fn an_existing_v9_store_opens_without_retired_tables() {
    let dir = temp_dir("reopen");
    let path = dir.join("state.sqlite");
    {
        // A store as schema 9 wrote it: the current tables, the legacy tasks
        // table holding one task, and the retired tables an older build made.
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(schema::SCHEMA).unwrap();
        conn.execute_batch(LEGACY_TASKS).unwrap();
        conn.execute_batch(
            "INSERT INTO tasks (id, title, description) VALUES ('T', 'title', 'body');
             INSERT OR REPLACE INTO schema_version (id, version) VALUES ('singleton', 9);",
        )
        .unwrap();
        conn.execute_batch(PLANTED).unwrap();
        for name in RETIRED {
            assert!(table_present(&conn, name), "did not plant {name}");
        }
    }
    let store = Store::open(&path).expect("an existing store should open");
    assert!(
        !table_present(store.connection(), "tasks"),
        "the legacy tasks table survived open"
    );
    let title: String = store
        .connection()
        .query_row("SELECT title FROM work_items WHERE id = 'T'", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(title, "title");
    let version: i64 = store
        .connection()
        .query_row(
            "SELECT version FROM schema_version WHERE id = 'singleton'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, i64::from(SCHEMA_VERSION));
    for name in RETIRED {
        assert!(
            !table_present(store.connection(), name),
            "{name} is still in the store after open"
        );
    }
}
