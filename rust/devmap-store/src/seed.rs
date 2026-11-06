//! Seeding a new worktree's store from another worktree's.
//!
//! A cold `devmap build` of a linked worktree re-parses every file the main
//! checkout's store already describes. Copying that store and then running the
//! ordinary incremental build gives the same graph for the files that did not
//! change. This module is the file and SQL half of that: it never decides
//! *whether* a copy is allowed (the CLI decides which checkout may seed which),
//! it only produces a staged store file that is safe to place.
//!
//! The rules the store imposes shape every step:
//!
//! * A store with more than one hard link is refused at open, so the copy is a
//!   separate inode: a reflink clone (`clonefile` on macOS, `FICLONE` on Linux)
//!   or a plain copy, never a hard link.
//! * A store whose recorded owner is a live sibling is refused, so the copy is
//!   rebound to its own worktree in `pending_state` and `generations` before it
//!   is placed.
//! * A raw copy of a database file is a consistent snapshot only when no WAL
//!   holds committed pages the file lacks. A non-empty `-wal`, or a source that
//!   changes while it is copied, takes SQLite's own `VACUUM INTO` instead. The
//!   `-shm` is never copied.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};

/// How the staged file was produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedMethod {
    /// Copy-on-write clone of the database file.
    Clone,
    /// Plain byte copy of the database file (the filesystem has no reflink).
    Copy,
    /// SQLite `VACUUM INTO`, taken because the source had a live WAL.
    Backup,
}

impl SeedMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::Clone => "clone",
            Self::Copy => "copy",
            Self::Backup => "backup",
        }
    }
}

/// `path` with `suffix` appended to its file name (`devmap.sqlite` + `-wal`).
pub fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// What identifies the source's bytes: if it differs after a raw copy, a writer
/// touched it meanwhile and the copy cannot be trusted.
#[derive(PartialEq, Eq)]
struct Fingerprint {
    db_len: u64,
    db_mtime: Option<SystemTime>,
    wal_len: u64,
    wal_mtime: Option<SystemTime>,
}

impl Fingerprint {
    fn of(db: &Path) -> Result<Self> {
        let meta = std::fs::symlink_metadata(db)
            .with_context(|| format!("cannot inspect {}", db.display()))?;
        if !meta.is_file() {
            bail!("{} is not a regular file", db.display());
        }
        let wal = sidecar(db, "-wal");
        let (wal_len, wal_mtime) = match std::fs::symlink_metadata(&wal) {
            Ok(wal_meta) if wal_meta.is_file() => (wal_meta.len(), wal_meta.modified().ok()),
            Ok(_) => bail!("{} is not a regular file", wal.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (0, None),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", wal.display()))
            }
        };
        Ok(Self {
            db_len: meta.len(),
            db_mtime: meta.modified().ok(),
            wal_len,
            wal_mtime,
        })
    }
}

/// Remove `path` and its `-wal` / `-shm`. For files this process staged.
pub fn discard_staged(path: &Path) {
    for file in [
        path.to_path_buf(),
        sidecar(path, "-wal"),
        sidecar(path, "-shm"),
    ] {
        let _ = std::fs::remove_file(file);
    }
}

/// Write a consistent copy of the store at `source` to `staged`, which must not
/// exist. A symlinked source is refused, not followed.
pub fn stage_seed(source: &Path, staged: &Path) -> Result<SeedMethod> {
    let before = Fingerprint::of(source)?;
    if before.wal_len == 0 {
        let method = clone_or_copy(source, staged)?;
        if Fingerprint::of(source)? == before {
            return Ok(method);
        }
        discard_staged(staged);
    }
    backup(source, staged)?;
    Ok(SeedMethod::Backup)
}

fn backup(source: &Path, staged: &Path) -> Result<()> {
    let conn = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("cannot open {} for backup", source.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let target = staged
        .to_str()
        .context("staged store path is not valid UTF-8")?;
    conn.execute("VACUUM INTO ?1", [target])
        .with_context(|| format!("VACUUM INTO {target} failed"))?;
    Ok(())
}

/// Seed one derived artifact (`repo_map.json`, `code_graph.json`, ...) beside a
/// seeded store: clone `source` to `target` when `target` is absent.
///
/// The manifest writer patches a changed artifact in place of the previous
/// file, keeping every unchanged block (see `devmap_query::code_graph`). A
/// worktree seeded with the store alone has no previous file, so its first
/// manifest build lays the graph out afresh and shares nothing with the main
/// checkout. A clone gives that build the main checkout's file to patch. The
/// clone is never trusted as current: the artifacts stamp is not copied, so
/// the next manifest build regenerates it for this worktree.
///
/// `None` when nothing was placed: `target` already exists, or `source` is not
/// a regular file. Staged under a sibling name and renamed into place, so a
/// reader never sees a partial artifact.
pub fn seed_artifact(source: &Path, target: &Path) -> Result<Option<SeedMethod>> {
    match std::fs::symlink_metadata(target) {
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", target.display()))
        }
    }
    match std::fs::symlink_metadata(source) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", source.display()))
        }
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    let staged = sidecar(target, &format!(".seed-{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    let placed = clone_or_copy(source, &staged).and_then(|method| {
        std::fs::rename(&staged, target)
            .with_context(|| format!("cannot place {}", target.display()))?;
        Ok(method)
    });
    if placed.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    placed.map(Some)
}

fn clone_or_copy(source: &Path, staged: &Path) -> Result<SeedMethod> {
    if try_clone(source, staged) {
        return Ok(SeedMethod::Clone);
    }
    // `create_new`: never write through something already at `staged`.
    let mut input = std::fs::File::open(source)?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(SeedMethod::Copy)
}

#[cfg(target_os = "macos")]
fn try_clone(source: &Path, staged: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let (Ok(src), Ok(dst)) = (
        std::ffi::CString::new(source.as_os_str().as_bytes()),
        std::ffi::CString::new(staged.as_os_str().as_bytes()),
    ) else {
        return false;
    };
    // SAFETY: both are valid NUL-terminated paths that outlive the call;
    // `clonefile` fails with EEXIST rather than replacing `staged`.
    unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) == 0 }
}

#[cfg(target_os = "linux")]
fn try_clone(source: &Path, staged: &Path) -> bool {
    use std::os::unix::io::AsRawFd;
    // _IOW(0x94, 9, int)
    const FICLONE: u32 = 0x4004_9409;
    let Ok(input) = std::fs::File::open(source) else {
        return false;
    };
    let Ok(output) = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staged)
    else {
        return false;
    };
    // SAFETY: both descriptors are open for the duration of the call.
    let cloned = unsafe { libc::ioctl(output.as_raw_fd(), FICLONE as _, input.as_raw_fd()) == 0 };
    if !cloned {
        drop(output);
        let _ = std::fs::remove_file(staged);
    }
    cloned
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn try_clone(_source: &Path, _staged: &Path) -> bool {
    false
}

/// Rebind a staged store to `root` in one IMMEDIATE transaction: the
/// `pending_state` singleton and every `generations` row. A store that has no
/// `pending_state` row is refused: it cannot be rebound, so it is not seedable.
pub fn rebind_staged(staged: &Path, root: &Path) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", root.display()))?;
    let root = root
        .to_str()
        .context("worktree root is not valid UTF-8")?
        .to_owned();
    let mut conn = Connection::open(staged)?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let bound = tx.execute(
        "UPDATE pending_state SET repo_root = ?1 WHERE singleton = 1",
        [&root],
    )?;
    if bound != 1 {
        bail!("the seed store has no pending_state row to rebind");
    }
    tx.execute("UPDATE generations SET repo_root = ?1", [&root])?;
    tx.commit()?;
    conn.close().map_err(|(_, error)| error)?;
    Ok(())
}

/// Fold the staged store's WAL into the file and drop its sidecars, so what is
/// placed is one self-contained file.
pub fn settle_staged(staged: &Path) -> Result<()> {
    let conn = Connection::open(staged)?;
    let (busy, _log, _checkpointed): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
    if busy != 0 {
        bail!("the staged store could not be checkpointed (busy)");
    }
    conn.close().map_err(|(_, error)| error)?;
    for suffix in ["-wal", "-shm"] {
        let side = sidecar(staged, suffix);
        if let Ok(meta) = std::fs::symlink_metadata(&side) {
            if meta.len() != 0 {
                bail!("{} still holds data after checkpoint", side.display());
            }
            let _ = std::fs::remove_file(&side);
        }
    }
    Ok(())
}
