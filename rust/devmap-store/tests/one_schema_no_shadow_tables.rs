//! PLAN B-series property D2: one schema, no rollback-compat shadow tables.
//!
//! A migration that keeps the table it replaced beside the new one (`*_old`,
//! `*_v22`, `*_backup`) doubles the write surface and leaves a second copy of
//! every row that nothing reads. The store opens to exactly one live table per
//! relation.

use devmap_store::Store;
use rusqlite::Connection;

fn shadow_names(path: &std::path::Path) -> Vec<String> {
    let conn = Connection::open(path).expect("inspect connection");
    let mut statement = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .expect("list tables");
    let names: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("read names")
        .collect::<Result<_, _>>()
        .expect("every name reads");
    names
        .into_iter()
        .filter(|name| is_shadow(name))
        .collect()
}

fn is_shadow(name: &str) -> bool {
    let versioned = name
        .rsplit_once("_v")
        .is_some_and(|(_, tail)| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()));
    versioned
        || ["_old", "_bak", "_backup", "_shadow", "_legacy", "_tmp"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn scratch_db(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("devmap-{label}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir.join("devmap.sqlite")
}

#[test]
fn d2_a_fresh_store_and_its_reopen_carry_no_shadow_table() {
    let path = scratch_db("d2-shadow");
    drop(Store::open(&path).expect("fresh store"));
    assert_eq!(shadow_names(&path), Vec::<String>::new(), "fresh store");
    drop(Store::open(&path).expect("reopened store"));
    assert_eq!(shadow_names(&path), Vec::<String>::new(), "reopened store");
}

#[test]
fn d2_the_shadow_detector_is_not_vacuous() {
    for name in ["extraction_cache_v4", "unresolved_rows_v22", "nodes_old", "x_backup"] {
        assert!(is_shadow(name), "{name} must read as a shadow table");
    }
    for name in ["generation_nodes", "reader_compat", "paths", "pending_state"] {
        assert!(!is_shadow(name), "{name} is a live table");
    }
}
