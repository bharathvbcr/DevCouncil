//! Per-call repository identity for the shared MCP process.
//!
//! Cursor holds one `devmap mcp` for every workspace tab. The store this process
//! opened at handshake is not "the" repository — each `tools/call` names one,
//! or is refused when more than one MCP root could answer. Opened stores are
//! cached by canonical path so `gitpulse` and `GitPulse` on APFS are one
//! entry, and they are opened read-only so a shared reader never migrates a
//! file the user did not ask to upgrade.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use devmap_store::Store;
use serde_json::{json, Value};

use crate::root_resolve::validate_repo_path;

/// How many distinct stores one MCP process will hold open.
pub const STORE_CACHE_CAP: usize = 8;

/// Page cache and file mapping of the connections this server keeps open.
///
/// 16 MiB against the 64 MiB every other read-only connection uses, and no
/// mmap. Measured (`examples/mcp_reader_memory_bench.rs`, 384 MB store, n=10
/// fresh processes per arm, interleaved): physical footprint 218 MiB median
/// at 64 MiB, 154 MiB at 16 MiB, spread within an arm 10 MiB or less. The
/// cold-call latency cost of the smaller cache was not separable from noise
/// at the load of that run (107-130); mmap added about 2 MiB of gain, inside
/// the spread, and a truncated mapped file is SIGBUS, so it is not used.
pub const MCP_READER_MEMORY: devmap_store::ReaderMemory = devmap_store::ReaderMemory {
    cache_kib: 16_384,
    mmap_bytes: 0,
};

/// How long a cached store may go unused before it is dropped.
///
/// A dropped store releases its SQLite page cache (up to 64 MiB per
/// connection) and its cached edge index, which together are most of what an
/// idle `devmap mcp` holds: 27 idle servers held about 2.1 GB between them
/// (2026-11-09 audit, F6). Ten minutes outlasts a pause inside one working
/// session, so a store in use is not reopened mid-conversation, while a server
/// left behind by a closed tab stops holding the memory. The next call after an
/// eviction reopens the store and rebuilds the edge index on demand (one cold
/// call), the same cost as the first call of a fresh server.
///
/// Only stores this cache opened itself expire: a [`StoreOrigin::CallerOwned`]
/// store may exist nowhere else (an in-memory one), so dropping it would lose
/// it rather than release it.
pub const STORE_IDLE_TTL: Duration = Duration::from_secs(10 * 60);

/// How often the idle sweeper looks. A store is therefore dropped between
/// [`STORE_IDLE_TTL`] and [`STORE_IDLE_TTL`] plus this long after its last use.
pub const STORE_IDLE_SWEEP: Duration = Duration::from_secs(30);

/// Where the cache reads the time. Injected so idle eviction is testable
/// without sleeping.
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

fn system_clock() -> Clock {
    Arc::new(Instant::now)
}

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
    last_used: Instant,
}

/// Bounded LRU of open stores, keyed by canonical store path.
pub struct StoreLru {
    entries: Vec<LruEntry>,
    clock: Clock,
}

impl Default for StoreLru {
    fn default() -> Self {
        Self::with_clock(system_clock())
    }
}

impl StoreLru {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_clock(clock: Clock) -> Self {
        Self {
            entries: Vec::new(),
            clock,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Take out the store this cache opened under `key`. A caller-owned store
    /// is never taken: it may exist nowhere else, so it is reported on and
    /// kept, not dropped and lost.
    ///
    /// The evicted handles are returned, not dropped here: closing a SQLite
    /// connection and freeing an edge index must not happen while the caller
    /// holds the cache lock and other calls wait on it. The caller drops them
    /// after releasing it.
    #[must_use = "drop the evicted stores after releasing the cache lock"]
    pub fn evict_opened(&mut self, key: &Path) -> Vec<Arc<Store>> {
        self.take_where(|entry| entry.key == key && matches!(entry.origin, StoreOrigin::Opened(_)))
    }

    /// Take out every store this cache opened that has gone [`STORE_IDLE_TTL`]
    /// without a lookup. Returned for the caller to drop outside the lock, as
    /// [`Self::evict_opened`] explains.
    #[must_use = "drop the evicted stores after releasing the cache lock"]
    pub fn evict_idle(&mut self) -> Vec<Arc<Store>> {
        let now = (self.clock)();
        self.take_where(|entry| match entry.origin {
            StoreOrigin::CallerOwned => false,
            StoreOrigin::Opened(_) => {
                now.saturating_duration_since(entry.last_used) >= STORE_IDLE_TTL
            }
        })
    }

    /// Whether the entry under `key` was handed in by the caller.
    pub fn holds_caller_owned(&self, key: &Path) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.key == key && matches!(entry.origin, StoreOrigin::CallerOwned))
    }

    fn take_where(&mut self, mut selected: impl FnMut(&LruEntry) -> bool) -> Vec<Arc<Store>> {
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(self.entries.len());
        for entry in self.entries.drain(..) {
            if selected(&entry) {
                taken.push(entry.store);
            } else {
                kept.push(entry);
            }
        }
        self.entries = kept;
        taken
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
        let mut entry = self.entries.remove(pos);
        entry.last_used = (self.clock)();
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
        let last_used = (self.clock)();
        self.entries.push(LruEntry {
            key,
            store,
            origin,
            last_used,
        });
    }
}

/// Keeps a background thread sweeping idle stores out of one cache.
///
/// Sweeping only on lookup would never run in the case it exists for: a server
/// nobody is calling. The thread holds the cache weakly and ends as soon as the
/// owner of this value is dropped.
pub struct IdleSweeper {
    _stop: Sender<()>,
}

impl IdleSweeper {
    /// `None` when the thread could not be started; the cache is then swept
    /// only when a store is opened, which still bounds a busy server.
    pub fn start(cache: Weak<Mutex<StoreLru>>, every: Duration) -> Option<Self> {
        let (stop, stopped) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("devmap-store-idle-sweep".into())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(every) {
                    let Some(cache) = cache.upgrade() else {
                        return;
                    };
                    let idle = crate::lock_recover(&cache).evict_idle();
                    drop(idle);
                }
            })
            .ok()?;
        Some(Self { _stop: stop })
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
    open_mcp_store_classified(path).map_err(|failure| failure.message)
}

/// Why an open failed, with the one distinction a caller acts on kept as a
/// field rather than recovered from the text.
#[derive(Debug)]
pub struct OpenFailure {
    pub message: String,
    /// SQLite reported `SQLITE_CORRUPT` (or a re-worded full-text corruption)
    /// while opening: the file is damaged, and a caller may remember that.
    pub corrupt: bool,
}

impl OpenFailure {
    fn other(message: String) -> Self {
        Self {
            message,
            corrupt: false,
        }
    }
}

/// [`open_mcp_store`], saying whether the failure was damage.
pub fn open_mcp_store_classified(path: &Path) -> Result<Store, OpenFailure> {
    if path.file_name().and_then(|n| n.to_str()) == Some("base.sqlite") {
        // A lane's private base: opened through its lease and recorded identity,
        // the same checks `StackedStore::open` makes.
        return devmap_query::stacked::open_lane_base_store(path).map_err(|err| {
            OpenFailure::other(format!(
                "immutable base snapshot at {} could not be opened: {err}",
                path.display()
            ))
        });
    }
    match Store::open_read_only_with(path, MCP_READER_MEMORY) {
        Ok(store) => Ok(store),
        Err(err) if Store::is_unsupported_schema(&err) => Err(OpenFailure::other(format!(
            "rebuild_required: the DevMap store at {} is not this binary's schema and \
will not be migrated in-place. Run `devmap build` in that repository. {err}",
            path.display()
        ))),
        Err(err) if Store::sqlite_error_is_corrupt(&err) => Err(OpenFailure {
            message: corrupt_store_message(path, &err, CorruptContext::Reopened),
            corrupt: true,
        }),
        Err(err) => Err(OpenFailure::other(format!(
            "devmap index at {} could not be opened: {err}",
            path.display()
        ))),
    }
}

/// Why a tool call is reporting `SQLITE_CORRUPT`, which decides what it can
/// honestly say about the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorruptContext {
    /// A connection opened afresh reported it too: the file, not a stale handle.
    Reopened,
    /// This file was found damaged on an earlier call and has not changed
    /// (same identity, size and modification time, `-wal` included) since, so
    /// it was not opened and queried again.
    KnownUnchanged,
    /// A store the caller handed in already open reported it. It is not
    /// evicted or reopened: it may exist nowhere else.
    CallerOwned,
}

/// What a tool call says when SQLite reports `SQLITE_CORRUPT`.
///
/// The raw SQLite text ("database disk image is malformed") names no store and
/// offers no way forward, and an agent that receives only that falls back to
/// grep. This names the file, what is known about the cause, and the two
/// commands that act on it.
pub fn corrupt_store_message(
    store: &Path,
    error: &dyn std::fmt::Display,
    context: CorruptContext,
) -> String {
    let cause = match context {
        CorruptContext::Reopened => {
            "including on a connection opened afresh, so this is the file and not a stale handle"
        }
        CorruptContext::KnownUnchanged => {
            "on an earlier call, and the file has not changed since, so it was not read again"
        }
        CorruptContext::CallerOwned => {
            "on a store this server was handed already open, which it does not reopen"
        }
    };
    format!(
        "the devmap store at {} is damaged: SQLite reported SQLITE_CORRUPT ({error}), {cause}. \
No answer is given, because a read that failed is not a statement that nothing was found. Run \
`devmap doctor` in that repository to inspect the store, and `devmap build` to rebuild it.",
        store.display()
    )
}

/// How many damaged files one server remembers.
pub const DAMAGE_MEMORY_CAP: usize = 32;

/// A store file's identity and state, as far as the filesystem shows it:
/// identity, length and modification time of the file and of its `-wal`.
///
/// A rebuild, a repair, a checkpoint or a restore changes at least one of
/// them; an unchanged stamp is the evidence for "still the file that was
/// damaged".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DamageStamp {
    identity: StoreFileIdentity,
    len: u64,
    modified: Option<std::time::SystemTime>,
    wal: Option<(u64, Option<std::time::SystemTime>)>,
}

impl DamageStamp {
    pub fn of(store: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(store)
            .ok()
            .filter(std::fs::Metadata::is_file)?;
        let mut wal_name = store.as_os_str().to_owned();
        wal_name.push("-wal");
        let wal = std::fs::metadata(PathBuf::from(wal_name))
            .ok()
            .map(|wal| (wal.len(), wal.modified().ok()));
        Some(Self {
            identity: StoreFileIdentity::of(&metadata),
            len: metadata.len(),
            modified: metadata.modified().ok(),
            wal,
        })
    }
}

/// Files found damaged, so a damaged store costs one stat per call and not an
/// open and a failing query (twice) on every call, across every server.
#[derive(Default)]
pub struct DamageMemory {
    entries: Vec<(PathBuf, DamageStamp)>,
}

impl DamageMemory {
    pub fn remember(&mut self, key: PathBuf, stamp: DamageStamp) {
        self.entries.retain(|(known, _)| *known != key);
        if self.entries.len() >= DAMAGE_MEMORY_CAP {
            self.entries.remove(0);
        }
        self.entries.push((key, stamp));
    }

    /// Whether `key` is remembered as damaged and `current` is still the file
    /// that was. A stamp that differs, or a file that cannot be stat-ed,
    /// forgets the entry: the file has changed, so it is read again.
    pub fn still_damaged(&mut self, key: &Path, current: Option<DamageStamp>) -> bool {
        let Some(pos) = self.entries.iter().position(|(known, _)| known == key) else {
            return false;
        };
        if current == Some(self.entries[pos].1) {
            return true;
        }
        self.entries.remove(pos);
        false
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Validate `repo_path` and require that `paths::store_path` (or a valid
/// `stack_lease.json` base snapshot) is a readable file.
///
/// Never falls back to another repository: a missing store or corrupt lease at
/// the named root is a tool error naming that path.
pub fn store_for_repo_path(raw: &str) -> Result<RepositoryRef, String> {
    let root = validate_repo_path(raw)?;
    let Some(store) = crate::root_resolve::effective_store_for_root(&root)? else {
        let expected = devmap_extract::paths::store_path(&root);
        let remedy = if devmap_extract::main_checkout_of(&root).is_some() {
            " Run `devmap seed` (or `devmap build --manifest`) in that worktree first."
        } else {
            ""
        };
        return Err(format!(
            "repo_path {}: no readable DevMap store at {}. This call will not fall back to \
another repository.{remedy}",
            root.display(),
            expected.display()
        ));
    };
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
    use super::{
        CacheLookup, Clock, DamageMemory, DamageStamp, IdleSweeper, StoreFileIdentity, StoreLru,
        StoreOrigin, DAMAGE_MEMORY_CAP, STORE_IDLE_TTL,
    };

    include!("../../testsupport/timing.rs");
    use devmap_store::Store;
    use std::cell::Cell;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

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

    /// A clock the test moves by hand: `advance` is the only thing that passes
    /// time, so no test here sleeps.
    fn manual_clock() -> (Clock, Arc<AtomicU64>) {
        let base = Instant::now();
        let offset_ms = Arc::new(AtomicU64::new(0));
        let reader = Arc::clone(&offset_ms);
        let clock: Clock =
            Arc::new(move || base + Duration::from_millis(reader.load(Ordering::SeqCst)));
        (clock, offset_ms)
    }

    fn advance(offset_ms: &AtomicU64, by: Duration) {
        offset_ms.fetch_add(by.as_millis() as u64, Ordering::SeqCst);
    }

    const A_MOMENT: Duration = Duration::from_millis(1);

    fn opened_entry(cache: &mut StoreLru, tag: &str) -> (PathBuf, Arc<Store>) {
        let file = scratch_file(tag);
        let identity = StoreFileIdentity::of(&std::fs::metadata(&file).unwrap());
        let store = Arc::new(Store::open_in_memory().unwrap());
        cache.insert(
            file.clone(),
            Arc::clone(&store),
            StoreOrigin::Opened(identity),
        );
        (file, store)
    }

    #[test]
    fn a_store_unused_for_the_idle_ttl_is_dropped_and_its_memory_released() {
        let (clock, offset) = manual_clock();
        let mut cache = StoreLru::with_clock(clock);
        let (file, store) = opened_entry(&mut cache, "idle");
        assert_eq!(Arc::strong_count(&store), 2, "the cache holds one handle");

        advance(&offset, STORE_IDLE_TTL - A_MOMENT);
        assert!(
            cache.evict_idle().is_empty(),
            "just inside the ttl is still in use"
        );
        assert_eq!(cache.len(), 1);

        advance(&offset, A_MOMENT);
        let idle = cache.evict_idle();
        assert_eq!(idle.len(), 1, "at the ttl it is taken out");
        assert!(cache.is_empty());
        assert_eq!(
            Arc::strong_count(&store),
            2,
            "the cache hands the store back instead of dropping it under its lock"
        );
        drop(idle);
        assert_eq!(
            Arc::strong_count(&store),
            1,
            "the cache released its handle, so the connection and edge index can be freed"
        );
        assert!(matches!(
            cache.get_verified(&file, |path: &Path| std::fs::metadata(path)),
            CacheLookup::Miss
        ));
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_hit_restarts_the_idle_clock() {
        let (clock, offset) = manual_clock();
        let mut cache = StoreLru::with_clock(clock);
        let (file, _store) = opened_entry(&mut cache, "touched");

        advance(&offset, STORE_IDLE_TTL - A_MOMENT);
        assert!(matches!(
            cache.get_verified(&file, |path: &Path| std::fs::metadata(path)),
            CacheLookup::Hit(_)
        ));
        advance(&offset, STORE_IDLE_TTL - A_MOMENT);
        assert!(
            cache.evict_idle().is_empty(),
            "the ttl runs from the last use, not from the open"
        );
        advance(&offset, A_MOMENT);
        assert_eq!(cache.evict_idle().len(), 1);
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_caller_owned_store_never_expires() {
        let (clock, offset) = manual_clock();
        let mut cache = StoreLru::with_clock(clock);
        cache.insert(
            PathBuf::from("in-memory"),
            Arc::new(Store::open_in_memory().unwrap()),
            StoreOrigin::CallerOwned,
        );
        advance(&offset, STORE_IDLE_TTL * 100);
        assert!(
            cache.evict_idle().is_empty(),
            "dropping it would lose it, not free it"
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_caller_owned_store_is_never_evicted_by_name() {
        let mut cache = StoreLru::new();
        cache.insert(
            PathBuf::from("in-memory"),
            Arc::new(Store::open_in_memory().unwrap()),
            StoreOrigin::CallerOwned,
        );
        assert!(
            cache.evict_opened(Path::new("in-memory")).is_empty(),
            "a corrupt in-memory store is reported, not dropped and lost"
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn evict_opened_takes_only_the_named_entry() {
        let mut cache = StoreLru::new();
        let (first, _a) = opened_entry(&mut cache, "evict-a");
        let (second, _b) = opened_entry(&mut cache, "evict-b");
        assert_eq!(cache.evict_opened(&first).len(), 1);
        assert!(cache.evict_opened(&first).is_empty(), "already gone");
        assert_eq!(cache.len(), 1);
        assert!(matches!(
            cache.get_verified(&second, |path: &Path| std::fs::metadata(path)),
            CacheLookup::Hit(_)
        ));
        std::fs::remove_dir_all(first.parent().unwrap()).unwrap();
        std::fs::remove_dir_all(second.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_sweeper_drops_an_idle_store_with_no_call_arriving() {
        let (clock, offset) = manual_clock();
        let cache = Arc::new(Mutex::new(StoreLru::with_clock(clock)));
        let (file, _store) = opened_entry(&mut cache.lock().unwrap(), "sweeper");
        advance(&offset, STORE_IDLE_TTL);
        let _sweeper = IdleSweeper::start(Arc::downgrade(&cache), Duration::from_millis(5))
            .expect("the sweeper thread starts");
        // Polls for the thread's effect, one millisecond apart, under a
        // deadline scaled the way the repo's other load-tolerant tests are.
        let deadline = Instant::now() + coverage_relaxed(Duration::from_secs(30));
        while !cache.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the sweeper never ran");
            std::thread::sleep(Duration::from_millis(1));
        }
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_mcp_open_uses_the_smaller_cache_and_no_mapping() {
        let file = scratch_file("memory");
        let store_path = file.with_file_name("devmap.sqlite");
        drop(Store::open(&store_path).expect("create store"));
        let opened = super::open_mcp_store(&store_path).expect("the mcp open");
        assert_eq!(
            opened.memory_settings().unwrap(),
            super::MCP_READER_MEMORY,
            "the MCP server's connections use their own setting"
        );
        assert_eq!(super::MCP_READER_MEMORY.cache_kib, 16_384);
        assert_eq!(super::MCP_READER_MEMORY.mmap_bytes, 0);
        let plain = Store::open_read_only(&store_path).expect("a plain read-only open");
        assert_eq!(plain.memory_settings().unwrap().cache_kib, 65_536);
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    fn damaged_file(tag: &str) -> PathBuf {
        let file = scratch_file(tag);
        std::fs::write(&file, b"damaged").unwrap();
        file
    }

    #[test]
    fn a_damaged_file_is_remembered_until_it_changes() {
        let file = damaged_file("damage-memory");
        let mut memory = DamageMemory::default();
        assert!(
            !memory.still_damaged(&file, DamageStamp::of(&file)),
            "nothing known yet"
        );

        memory.remember(file.clone(), DamageStamp::of(&file).unwrap());
        assert!(memory.still_damaged(&file, DamageStamp::of(&file)));
        assert!(
            memory.still_damaged(&file, DamageStamp::of(&file)),
            "stays known"
        );

        // New length and a later modification time.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&file, b"repaired").unwrap();
        assert!(
            !memory.still_damaged(&file, DamageStamp::of(&file)),
            "a changed file is read again"
        );
        assert!(memory.is_empty(), "and forgotten");

        memory.remember(file.clone(), DamageStamp::of(&file).unwrap());
        std::fs::remove_file(&file).unwrap();
        assert!(
            !memory.still_damaged(&file, DamageStamp::of(&file)),
            "a file that cannot be seen is not vouched for"
        );
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_change_to_the_wal_alone_clears_a_remembered_damage() {
        let file = damaged_file("damage-wal");
        let mut memory = DamageMemory::default();
        memory.remember(file.clone(), DamageStamp::of(&file).unwrap());
        let mut wal = file.as_os_str().to_owned();
        wal.push("-wal");
        std::fs::write(PathBuf::from(wal), b"frames").unwrap();
        assert!(!memory.still_damaged(&file, DamageStamp::of(&file)));
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }

    #[test]
    fn damage_memory_is_bounded() {
        let mut memory = DamageMemory::default();
        let file = damaged_file("damage-cap");
        let stamp = DamageStamp::of(&file).unwrap();
        for index in 0..DAMAGE_MEMORY_CAP * 3 {
            memory.remember(PathBuf::from(format!("store-{index}")), stamp);
        }
        assert_eq!(memory.len(), DAMAGE_MEMORY_CAP);
        assert!(
            !memory.still_damaged(Path::new("store-0"), Some(stamp)),
            "the oldest is the one forgotten"
        );
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
