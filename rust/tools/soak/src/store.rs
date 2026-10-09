//! The kernel's store, as the soak measures it: where it is, what graph it
//! holds, and how many bytes it takes on disk.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

/// The edges of the newest generation, one row per edge, in a stable order.
/// `devmap-store`'s gate-script SQL test prepares this literal against the
/// current schema, so a column rename fails `cargo test` rather than a soak.
const EDGES: &str = "SELECT source_symbol||'>'||target_symbol||':'||edge_kind FROM generation_edges
     WHERE generation_id=(SELECT max(id) FROM generations) ORDER BY 1";

/// Ask the kernel where it put its store.
///
/// Resolved from the kernel after the first build, never hard-coded. The state
/// directory is a property of the repository — `$DEVMAP_HOME`, else `.devmap/`
/// if present, else `.devcouncil/` if present, else `.devmap/` — and its owner
/// is `devmap_extract::paths`; `devmap --json status` reports what it decided.
/// The first harness read the *Python* engine's file for its whole life, so
/// its digest was the empty string and its growth limit compared 0 against 0:
/// a check that could not run reporting what a check that ran and passed
/// reports. A path this cannot read, or reads and does not find, is an error.
pub fn resolve(devmap: &Path, root: &Path) -> Result<PathBuf, String> {
    let output = Command::new(devmap)
        .args(["--json", "status"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {}: {e}", devmap.display()))?;
    if !output.status.success() {
        return Err(format!(
            "`devmap --json status` exited with {}",
            output.status
        ));
    }
    let status: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("`devmap --json status` is not JSON: {e}"))?;
    let path = status
        .get("db_path")
        .and_then(serde_json::Value::as_str)
        .filter(|p| !p.is_empty())
        .ok_or("`devmap --json status` reports no db_path")?;
    Ok(root.join(path))
}

/// Digest of the newest generation's edges. Fails closed: a store that cannot
/// be read, and one with no edges to compare, are errors — hashing nothing
/// still yields a digest, and two empty digests always agree.
pub fn digest(store: &Path) -> Result<String, String> {
    // Read-write without create, as `sqlite3` opened it: a missing store is an
    // error, not a new empty one.
    let conn = Connection::open_with_flags(
        store,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("cannot open {}: {e}", store.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let mut statement = conn.prepare(EDGES).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<String>, _>>()
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Err("no graph edges were measured".into());
    }
    let mut hasher = Sha256::new();
    for row in &rows {
        hasher.update(row.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// The main database plus its write-ahead log: both are the store's bytes on
/// disk, and a WAL that never checkpoints is exactly the unbounded growth the
/// soak looks for. Counting only the main file would hide it.
pub fn bytes(store: &Path) -> u64 {
    let mut wal = store.as_os_str().to_owned();
    wal.push("-wal");
    file_bytes(store) + file_bytes(Path::new(&wal))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("soak-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn store_with(dir: &Path, edges: &[(&str, &str, &str, i64)]) -> PathBuf {
        let path = dir.join("devmap.sqlite");
        let conn = Connection::open(&path).expect("create");
        conn.execute_batch(
            "CREATE TABLE generations(id INTEGER PRIMARY KEY);
             CREATE TABLE generation_edges(generation_id INTEGER, source_symbol TEXT, target_symbol TEXT, edge_kind TEXT);",
        )
        .expect("schema");
        for (s, t, k, generation) in edges {
            conn.execute(
                "INSERT OR IGNORE INTO generations(id) VALUES (?1)",
                [generation],
            )
            .expect("generation");
            conn.execute(
                "INSERT INTO generation_edges VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![generation, s, t, k],
            )
            .expect("edge");
        }
        path
    }

    #[test]
    fn the_digest_reads_only_the_newest_generation_in_a_stable_order() {
        let dir = scratch("digest");
        let a = store_with(
            &dir,
            &[
                ("b", "c", "calls", 2),
                ("a", "b", "calls", 2),
                ("x", "y", "calls", 1),
            ],
        );
        let first = digest(&a).expect("digest");
        std::fs::remove_file(&a).expect("reset");
        let b = store_with(
            &dir,
            &[
                ("a", "b", "calls", 2),
                ("b", "c", "calls", 2),
                ("z", "z", "calls", 1),
            ],
        );
        assert_eq!(
            first,
            digest(&b).expect("digest"),
            "an older generation or row order changed the digest"
        );
        std::fs::remove_file(&b).expect("reset");
        let c = store_with(&dir, &[("a", "b", "calls", 2)]);
        assert_ne!(first, digest(&c).expect("digest"));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn a_store_with_no_edges_or_no_file_is_not_a_digest() {
        let dir = scratch("empty");
        let empty = store_with(&dir, &[]);
        assert_eq!(digest(&empty).unwrap_err(), "no graph edges were measured");
        assert!(digest(&dir.join("absent.sqlite")).is_err());
        assert!(
            !dir.join("absent.sqlite").exists(),
            "the digest created the store it was asked to read"
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn store_bytes_include_the_write_ahead_log() {
        let dir = scratch("bytes");
        let store = dir.join("devmap.sqlite");
        std::fs::write(&store, [0u8; 100]).expect("main");
        assert_eq!(bytes(&store), 100);
        std::fs::write(dir.join("devmap.sqlite-wal"), [0u8; 23]).expect("wal");
        assert_eq!(bytes(&store), 123);
        assert_eq!(bytes(&dir.join("missing")), 0);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
