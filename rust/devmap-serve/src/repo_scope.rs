//! Per-call repository identity for the shared MCP process.
//!
//! Cursor holds one `devmap mcp` for every workspace tab. The store this process
//! opened at handshake is not "the" repository — each `tools/call` names one,
//! or is refused when more than one MCP root could answer. Opened stores are
//! cached by canonical path so `gitpulse` and `GitPulse` on APFS are one
//! entry, and they are opened read-only so a shared reader never migrates a
//! file the user did not ask to upgrade.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devmap_store::Store;
use serde_json::{json, Value};

use crate::root_resolve::validate_repo_path;

/// How many distinct stores one MCP process will hold open.
pub const STORE_CACHE_CAP: usize = 8;

/// Which repository a successful tool call answered from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRef {
    pub root: PathBuf,
    pub store: PathBuf,
    pub resolved_from: &'static str,
}

impl RepositoryRef {
    pub fn to_json(&self) -> Value {
        json!({
            "root": self.root.display().to_string(),
            "store": self.store.display().to_string(),
            "resolved_from": self.resolved_from,
        })
    }
}

/// Which file a cached connection was opened on.
///
/// A `Store` holds one long-lived SQLite connection, and on Unix that
/// connection keeps an unlinked file alive: after `rm -rf .devmap && devmap
/// build` — the documented quarantine remedy — a cache keyed by path alone
/// went on answering from the deleted file's last generation, on every slot
/// mode. The path is the same; the file is not.
///
/// Unix compares `(dev, ino)`, which a recreated file cannot share with one
/// this process still holds open (the open handle keeps the inode allocated).
/// Elsewhere std has no stable file id, so size and mtime stand in: a write
/// to the store also changes them, which costs a reopen and never a stale
/// answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreFileIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(not(unix))]
    len: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

impl StoreFileIdentity {
    pub fn of(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                dev: metadata.dev(),
                ino: metadata.ino(),
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                len: metadata.len(),
                modified: metadata.modified().ok(),
            }
        }
    }
}

/// Where a cached store came from, which decides whether a hit is re-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreOrigin {
    /// Opened by this cache from a file whose identity was taken just before
    /// the open. Every hit re-stats the path against it.
    Opened(StoreFileIdentity),
    /// Handed in already open by a caller that owns it (`StoreSlot::ready`):
    /// an in-memory store, or one whose lifetime the caller manages. There
    /// is no file for this cache to check, so it is served as given.
    CallerOwned,
}

/// What a cache lookup found.
pub enum CacheLookup {
    /// The cached store, still the file at its path.
    Hit(Arc<Store>),
    /// An entry existed but its file is gone or was replaced; it has been
    /// evicted, and the caller must look at the path afresh.
    Evicted,
    /// No entry under this key.
    Miss,
}

struct LruEntry {
    key: PathBuf,
    store: Arc<Store>,
    origin: StoreOrigin,
}

/// Bounded LRU of open stores, keyed by canonical store path.
#[derive(Default)]
pub struct StoreLru {
    entries: Vec<LruEntry>,
}

impl StoreLru {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_store_path(&self) -> Option<PathBuf> {
        self.entries.last().map(|entry| entry.key.clone())
    }

    /// The cached store under `key`, provided the file at `key` is still the
    /// one it was opened on.
    ///
    /// `stat` is asked at most once — `FnOnce` makes that a property of the
    /// signature — and only when an entry exists, so a steady-state hit costs
    /// one stat and no reopen. A stat that fails for any reason, or names
    /// something other than a regular file, evicts: the cache never vouches
    /// for a file it could not see.
    pub fn get_verified(
        &mut self,
        key: &Path,
        stat: impl FnOnce(&Path) -> std::io::Result<std::fs::Metadata>,
    ) -> CacheLookup {
        let Some(pos) = self.entries.iter().position(|entry| entry.key == key) else {
            return CacheLookup::Miss;
        };
        if let StoreOrigin::Opened(recorded) = self.entries[pos].origin {
            let current = stat(key)
                .ok()
                .filter(std::fs::Metadata::is_file)
                .map(|metadata| StoreFileIdentity::of(&metadata));
            if current != Some(recorded) {
                self.entries.remove(pos);
                return CacheLookup::Evicted;
            }
        }
        let entry = self.entries.remove(pos);
        let store = Arc::clone(&entry.store);
        self.entries.push(entry);
        CacheLookup::Hit(store)
    }

    pub fn insert(&mut self, key: PathBuf, store: Arc<Store>, origin: StoreOrigin) {
        if let Some(pos) = self.entries.iter().position(|entry| entry.key == key) {
            self.entries.remove(pos);
        }
        if self.entries.len() >= STORE_CACHE_CAP {
            self.entries.remove(0);
        }
        self.entries.push(LruEntry { key, store, origin });
    }
}

pub fn canonicalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Open a store the MCP server is allowed to hold: read-only, never migrate.
///
/// A schema-behind file is reported as `rebuild_required` rather than upgraded
/// in place. The MCP process is a shared reader; migrating under it would change
/// a file the user did not ask this binary to write, from a host whose cwd may
/// not even be that repository.
pub fn open_mcp_store(path: &Path) -> Result<Store, String> {
    match Store::open_read_only(path) {
        Ok(store) => Ok(store),
        Err(err) if Store::is_unsupported_schema(&err) => Err(format!(
            "rebuild_required: the DevMap store at {} is not this binary's schema and \
will not be migrated in-place. Run `devmap build` in that repository. {err}",
            path.display()
        )),
        Err(err) => Err(format!(
            "devmap index at {} could not be opened: {err}",
            path.display()
        )),
    }
}

/// Validate `repo_path` and require that `paths::store_path` is a readable file.
///
/// Never falls back to another repository: a missing store at the named root
/// is a tool error naming that path.
pub fn store_for_repo_path(raw: &str) -> Result<RepositoryRef, String> {
    let root = validate_repo_path(raw)?;
    let store = devmap_extract::paths::store_path(&root);
    if !store.is_file() {
        return Err(format!(
            "repo_path {}: no readable DevMap store at {}. This call will not fall back to \
another repository.",
            root.display(),
            store.display()
        ));
    }
    Ok(RepositoryRef {
        root,
        store: devmap_extract::safe_fs::resolve_file_alias(&store)
            .map_err(|error| format!("unsafe store path {}: {error}", store.display()))?,
        resolved_from: "repo_path",
    })
}

/// Infer a repository root from a store file (`<root>/.devmap/codeintel/…`).
pub fn infer_root_from_store(store_path: &Path) -> PathBuf {
    store_path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(canonicalize_path)
        .unwrap_or_else(|| canonicalize_path(store_path))
}

#[cfg(test)]
mod tests {
    use super::{CacheLookup, StoreFileIdentity, StoreLru, StoreOrigin};
    use devmap_store::Store;
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn scratch_file(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devmap-store-lru-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("store.sqlite");
        std::fs::write(&file, b"first").unwrap();
        file
    }

    #[test]
    fn a_hit_on_an_unchanged_file_costs_one_stat_and_returns_the_same_store() {
        let file = scratch_file("steady");
        let identity = StoreFileIdentity::of(&std::fs::metadata(&file).unwrap());
        let store = Arc::new(Store::open_in_memory().unwrap());
        let mut cache = StoreLru::new();
        cache.insert(
            file.clone(),
            Arc::clone(&store),
            StoreOrigin::Opened(identity),
        );
        let stats = Cell::new(0);
        for _ in 0..3 {
            let lookup = cache.get_verified(&file, |path: &Path| {
                stats.set(stats.get() + 1);
                std::fs::metadata(path)
            });
            let CacheLookup::Hit(hit) = lookup else {
                panic!("an unchanged file must be a hit");
            };
            assert!(Arc::ptr_eq(&hit, &store));
        }
        assert_eq!(stats.get(), 3, "one stat per lookup");
        // A miss never stats: there is nothing to compare against.
        assert!(matches!(
            cache.get_verified(Path::new("/nonexistent/other"), |_: &Path| {
                stats.set(stats.get() + 1);
                Err(std::io::Error::other("unreachable"))
            }),
            CacheLookup::Miss
        ));
        assert_eq!(stats.get(), 3);
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_replaced_or_vanished_file_evicts_its_entry() {
        let file = scratch_file("replaced");
        let identity = StoreFileIdentity::of(&std::fs::metadata(&file).unwrap());
        let mut cache = StoreLru::new();
        let store = Arc::new(Store::open_in_memory().unwrap());
        cache.insert(
            file.clone(),
            Arc::clone(&store),
            StoreOrigin::Opened(identity),
        );
        // Keep the original inode alive, as an open SQLite connection does,
        // so the replacement cannot reuse its number.
        let _held = std::fs::File::open(&file).unwrap();
        let sibling = file.with_extension("new");
        std::fs::write(&sibling, b"second, a different length").unwrap();
        std::fs::rename(&sibling, &file).unwrap();
        assert!(matches!(
            cache.get_verified(&file, |path: &Path| std::fs::metadata(path)),
            CacheLookup::Evicted
        ));
        assert!(matches!(
            cache.get_verified(&file, |path: &Path| std::fs::metadata(path)),
            CacheLookup::Miss
        ));

        cache.insert(file.clone(), store, StoreOrigin::Opened(identity));
        std::fs::remove_file(&file).unwrap();
        assert!(matches!(
            cache.get_verified(&file, |path: &Path| std::fs::metadata(path)),
            CacheLookup::Evicted
        ));
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_caller_owned_store_is_served_without_a_stat() {
        let mut cache = StoreLru::new();
        let store = Arc::new(Store::open_in_memory().unwrap());
        cache.insert(PathBuf::from("in-memory"), store, StoreOrigin::CallerOwned);
        assert!(matches!(
            cache.get_verified(
                Path::new("in-memory"),
                |_: &Path| -> std::io::Result<std::fs::Metadata> {
                    panic!("a caller-owned store has no file to stat")
                }
            ),
            CacheLookup::Hit(_)
        ));
    }
}
