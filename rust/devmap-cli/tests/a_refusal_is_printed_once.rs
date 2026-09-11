//! A store refusal reaches the operator once, not twice.
//!
//! Measured through the release binary against this repository's own store
//! after the 2026-09-07 merge:
//!
//! ```text
//! {"error":"required index idx_file_payloads_cache_identity is missing; … — run `devmap build`
//!   to rebuild it: required index idx_file_payloads_cache_identity is missing; … — run
//!   `devmap build` to rebuild it"}
//! ```
//!
//! The store carries its refusals in a rusqlite variant that both displays
//! its boxed error and returns it as `source()`, and the CLI renders the
//! whole chain (`{:#}`), so every store refusal read as two. A chain that
//! repeats a message contributes it once.

use std::path::PathBuf;
use std::process::Command;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-once-{tag}-{}-{}",
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

#[test]
fn a_future_schema_refusal_is_printed_once_in_both_renderings() {
    let dir = scratch("future");
    let db = dir.join("devmap.sqlite");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let out = Command::new(devmap())
        .args(["--json", "--db"])
        .arg(&db)
        .args(["build", "--progress", "never"])
        .arg(&dir)
        .output()
        .expect("devmap runs");
    assert!(!out.status.success(), "a future schema is refused");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let sentence = "schema version 99 is not supported by this binary";
    for (name, text) in [
        ("stdout JSON", stdout.as_ref()),
        ("stderr", stderr.as_ref()),
    ] {
        let times = text.matches(sentence).count();
        assert_eq!(
            times, 1,
            "{name} must carry the refusal exactly once, found {times}: {text}"
        );
    }
}
