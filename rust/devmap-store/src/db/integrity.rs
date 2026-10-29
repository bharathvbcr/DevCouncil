//! A store file whose pages are damaged: how it is recognised, confirmed and
//! set aside.
//!
//! Damage inside the full-text index's shadow tables made every remedy the
//! store named fail with the same "database disk image is malformed": `repair
//! --fts` and `build --full` both read the damaged pages on the way to
//! replacing them, and a plain `build` compared hashes, found nothing changed
//! and exited 0 over a store whose search errored. Every row in a store is
//! derived from the working tree, so the one remedy that cannot fail is to
//! move the file aside and build again. These are the pieces of that, kept
//! apart from the build command so the daemon or any other writer can use the
//! same three answers.

use super::{Store, WriterLock};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

impl Store {
    /// Whether `error`, or anything it wraps, is SQLite's `SQLITE_CORRUPT`.
    ///
    /// By error *code*, never by message: `fts_failure` and `refusal` re-wrap
    /// an error into text, and a substring match on that text would call a
    /// refusal that merely quotes the words "database disk image is malformed"
    /// a damaged file. `SQLITE_NOTADB` is deliberately not included: a file
    /// that is not a database at all may be a file the caller pointed `--db`
    /// at by mistake, and moving that aside is not this function's call.
    pub fn is_database_corrupt(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            cause
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(Self::sqlite_error_is_corrupt)
        })
    }

    /// [`Self::is_database_corrupt`] for an error not yet wrapped.
    pub fn sqlite_error_is_corrupt(error: &rusqlite::Error) -> bool {
        error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseCorrupt)
    }

    /// Check the file at `db_path` page by page, on a connection of its own.
    ///
    /// `PRAGMA quick_check`: every b-tree page is read and its structure
    /// checked, which is what a damaged page fails, without the index-content
    /// cross-checks `integrity_check` adds. It is the confirmation that
    /// separates "this file is damaged" from "this process saw one bad read",
    /// and it reads the whole file, so it runs only after something has
    /// already failed.
    ///
    /// `Ok(None)` when the file is whole; `Ok(Some(why))` when SQLite says it
    /// is not, or cannot read it as corrupt. Any other failure (a permission
    /// error, a missing file) is an `Err`: a check that could not run must not
    /// read as one that found damage, or as one that passed.
    pub fn quick_check(db_path: &Path) -> anyhow::Result<Option<String>> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::harden_connection(&conn)?;
        let verdict = (|| -> rusqlite::Result<Vec<String>> {
            let mut stmt = conn.prepare("PRAGMA quick_check(10)")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect()
        })();
        match verdict {
            Ok(rows) if rows.len() == 1 && rows[0] == "ok" => Ok(None),
            Ok(rows) => Ok(Some(rows.join("; "))),
            Err(error)
                if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseCorrupt) =>
            {
                Ok(Some(error.to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Move the damaged store at `db_path`, with its `-wal` and `-shm`, aside
    /// and return where the main file went.
    ///
    /// The target is `<name>.corrupt-<unix seconds>`, with a numeric suffix if
    /// that is taken: nothing is ever overwritten, and nothing is deleted. The
    /// writer-lock file stays where it is, since the caller holds it and it
    /// names the path, not the contents. The caller opens a fresh store at the
    /// same path afterwards.
    ///
    /// Takes the writer lock as a witness: set aside under no lock, the move
    /// would race another writer's open handle. The `Store` itself must already
    /// be dropped, so no connection of this process holds the old inode.
    pub fn quarantine_damaged(db_path: &Path, _writer: &WriterLock) -> anyhow::Result<PathBuf> {
        let db_path = devmap_extract::safe_fs::resolve_file_alias(db_path)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        let name = db_path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("{} has no file name", db_path.display()))?
            .to_string_lossy()
            .into_owned();
        let free = |attempt: u32| {
            let suffix = if attempt == 0 {
                String::new()
            } else {
                format!("-{attempt}")
            };
            db_path.with_file_name(format!("{name}.corrupt-{stamp}{suffix}"))
        };
        let sidecar = |path: &Path, ext: &str| {
            let mut full = path.as_os_str().to_os_string();
            full.push(ext);
            PathBuf::from(full)
        };
        // The first suffix at which the main file and both sidecars are free.
        let target = (0..1000)
            .map(free)
            .find(|target| {
                !target.exists()
                    && !sidecar(target, "-wal").exists()
                    && !sidecar(target, "-shm").exists()
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no free quarantine name next to {} (1000 attempts)",
                    db_path.display()
                )
            })?;
        std::fs::rename(&db_path, &target).map_err(|error| {
            anyhow::anyhow!(
                "could not move the damaged store {} aside to {}: {error}",
                db_path.display(),
                target.display()
            )
        })?;
        for ext in ["-wal", "-shm"] {
            let from = sidecar(&db_path, ext);
            if from.exists() {
                std::fs::rename(&from, sidecar(&target, ext)).map_err(|error| {
                    anyhow::anyhow!(
                        "moved {} to {} but could not move its {ext} file: {error}",
                        db_path.display(),
                        target.display()
                    )
                })?;
            }
        }
        Ok(target)
    }
}
