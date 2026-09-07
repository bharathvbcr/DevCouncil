//! Every refusal this store raises itself — a future schema, a read-only
//! file, a NUL in a search query, a Python-era database handed to `--db` —
//! was carried in `rusqlite::Error::InvalidParameterName`, whose `Display`
//! is `Invalid parameter name: {message}`. So the kernel told an operator
//!
//! ```text
//! Error: Invalid parameter name: devmap store x.sqlite: schema version 99 …
//! ```
//!
//! about a store, not a parameter. The reason is the message; nothing may be
//! prepended to it.

use std::path::PathBuf;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-refusal-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn stamped(dir: &std::path::Path, user_version: i32) -> PathBuf {
    let path = dir.join("devmap.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", user_version)
        .unwrap();
    path
}

fn refusal(path: &std::path::Path) -> String {
    match devmap_store::Store::open(path) {
        Ok(_) => panic!("{} must be refused", path.display()),
        Err(error) => format!("{error:#}"),
    }
}

#[test]
fn a_future_schema_is_refused_by_the_reason_alone() {
    let dir = scratch("future");
    let message = refusal(&stamped(&dir, 99));
    assert!(
        message.contains("schema version 99 is not supported by this binary"),
        "the refusal must state the reason: {message}"
    );
    assert!(
        !message.contains("Invalid parameter name"),
        "a store refusal is not a parameter error: {message}"
    );
}

#[test]
fn the_python_engines_database_is_named_with_the_number_the_store_owns() {
    let dir = scratch("python");
    let message = refusal(&stamped(&dir, devmap_store::PYTHON_INDEX_SCHEMA_VERSION));
    assert!(
        message.contains("Python engine's database")
            && message.contains(&format!(
                "schema {}",
                devmap_store::PYTHON_INDEX_SCHEMA_VERSION
            )),
        "the refusal must name the Python database by its number: {message}"
    );
    assert!(
        !message.contains("Invalid parameter name"),
        "a store refusal is not a parameter error: {message}"
    );
}
