use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use devmap_analyze::{analyze_with_discovery, DiscoveryCoverage};
use devmap_extract::{
    collect_go_modules, collect_sources_with_report, content_hash, extract_file,
    is_indexable_source, DiscoverySkipReason, MAX_SOURCE_BYTES,
};
use devmap_resolve::Resolver;
use devmap_store::{current_git_head, GenerationWriteOpts, Store, GENERATION_RETENTION};

use crate::watcher::start_file_watcher;

const STABLE_READ_ATTEMPTS: usize = 3;

/// How many per-path failures one drain's error message itemises.
///
/// The count of failures is bounded by `batch_limit` (8192) and each rendered
/// entry carries a path plus a whole error, so an itemised list of all of them
/// is an error message megabytes long — logged in full by `run_loop`, and again
/// on every backoff retry. Five is a sample; `failed.len()` beside it is the
/// truth, and both are reported. Same shape, and the same reason, as
/// `Store::DEGRADED_SAMPLE`.
const DRAIN_FAILURE_SAMPLE: usize = 5;

struct AbortTaskOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Abort the IPC task and *wait* for it to be gone.
///
/// `abort()` only schedules the task's future to be dropped, and the socket
/// file is removed by that drop (`UnixIpcServer::drop`). Returning from
/// `run_loop` without awaiting leaves a window in which the process can exit
/// with the endpoint still on disk — which is the same stale-socket state a
/// `kill -9` leaves, arrived at through the orderly path.
async fn release_ipc_endpoint<T>(task: &mut AbortTaskOnDrop<T>) {
    task.0.abort();
    let _ = (&mut task.0).await;
}

/// The signals that mean "stop serving".
///
/// Held as long-lived streams rather than created per loop iteration: a
/// listener registered only while a `select!` branch is being polled can miss
/// the signal that arrives between iterations, and `recv` on these is
/// cancel-safe, so losing the race to another branch costs nothing.
struct ShutdownSignals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_shutdown: tokio::signal::windows::CtrlShutdown,
}

impl ShutdownSignals {
    #[cfg(unix)]
    fn install() -> anyhow::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    #[cfg(windows)]
    fn install() -> anyhow::Result<Self> {
        Ok(Self {
            ctrl_c: tokio::signal::windows::ctrl_c()?,
            ctrl_shutdown: tokio::signal::windows::ctrl_shutdown()?,
        })
    }

    /// Resolves with the name of whichever signal arrived.
    #[cfg(unix)]
    async fn recv(&mut self) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
        }
    }

    #[cfg(windows)]
    async fn recv(&mut self) -> &'static str {
        tokio::select! {
            _ = self.ctrl_c.recv() => "CTRL_C",
            _ = self.ctrl_shutdown.recv() => "CTRL_SHUTDOWN",
        }
    }
}

/// Whether a stat-read-stat round proves the read was not torn.
///
/// K-B3. This was `before.modified().ok() == after.modified().ok()` inline. On
/// a filesystem where `modified()` errors, both sides collapse to `None`,
/// `None == None` is true, and the guard silently degrades to a length
/// comparison — which is exactly what a same-size in-place edit survives. The
/// read would then be admitted as clean and stored as though it were the file
/// on disk, with nothing anywhere recording that the timestamp half of the
/// check never ran.
///
/// The timestamps are the only signal separating "nothing moved" from "it
/// changed to something the same size", so losing them has to make the answer
/// *I cannot tell*, not *yes*. Both sides are required; one alone is not a
/// comparison.
fn read_is_stable(
    before_len: u64,
    after_len: u64,
    before_modified: Option<std::time::SystemTime>,
    after_modified: Option<std::time::SystemTime>,
    source_len: usize,
) -> bool {
    let (Some(before_modified), Some(after_modified)) = (before_modified, after_modified) else {
        return false;
    };
    before_len == after_len
        && before_modified == after_modified
        && u64::try_from(source_len).is_ok_and(|length| length == after_len)
}

fn read_stable_source(path: &std::path::Path, relative: &str) -> anyhow::Result<String> {
    read_stable_source_with(path, relative, || std::fs::read_to_string(path))
}

/// Whether a filesystem error means the path is **not there**, as opposed to
/// the syscall not having been possible.
///
/// The distinction is the whole point. "Not there" is a fact the queue can act
/// on — it reconciles as a deletion. "I could not look" — a symlink loop
/// (`ELOOP`), a parent that lost `+x` (`EACCES`), a stale handle on a network
/// mount, an I/O error on the device — is not, and answering it with a deletion
/// drops the file's rows out of the generation on the strength of a check that
/// never ran.
///
/// `NotADirectory` is absence too: a parent component became a file, so nothing
/// can live at this path and no retry will change that.
fn path_is_absent(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// Whether a failure means the path stopped existing while it was being read.
///
/// Searched down the whole cause chain because the read is wrapped in context
/// before it reaches the caller, and a `NotFound` that only survives as text in
/// a message is not a fact anything can branch on.
///
/// This is the one read failure that is not a failure at all: the file is gone,
/// which is a state the queue already knows how to express. Every other kind —
/// `PermissionDenied`, `IsADirectory`, an I/O error on the device — stays a
/// failure, because retrying those can succeed and pretending the file was
/// deleted would drop its rows out of the graph.
fn vanished_under_the_reader(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(path_is_absent)
}

fn read_stable_source_with<F>(
    path: &std::path::Path,
    relative: &str,
    mut read: F,
) -> anyhow::Result<String>
where
    F: FnMut() -> std::io::Result<String>,
{
    for _ in 0..STABLE_READ_ATTEMPTS {
        let before = std::fs::metadata(path)?;
        if !before.is_file() {
            anyhow::bail!("changed source {relative:?} is not a regular file");
        }
        if before.len() > MAX_SOURCE_BYTES {
            anyhow::bail!(
                "changed source {relative:?} is {} bytes; limit is {MAX_SOURCE_BYTES}",
                before.len()
            );
        }
        // `context`, not `anyhow!("{error}")`: formatting the cause into a
        // string throws the `io::ErrorKind` away, and the caller has to be able
        // to tell "this file was removed under me" from every other read
        // failure — one is a deletion to reconcile, the others are retries to
        // charge. See `vanished_under_the_reader`. The rendered message is the
        // same either way.
        let source = read().map_err(|error| {
            anyhow::Error::new(error).context(format!("cannot read changed source {relative:?}"))
        })?;
        let after = std::fs::metadata(path)?;
        if read_is_stable(
            before.len(),
            after.len(),
            before.modified().ok(),
            after.modified().ok(),
            source.len(),
        ) {
            return Ok(source);
        }
    }
    // Names both ways a round can fail, because they call for different
    // responses: a file genuinely churning will settle, while a filesystem that
    // cannot report modification times never will, and an operator reading only
    // "did not stabilize" would keep waiting for the second case to clear.
    anyhow::bail!(
        "changed source {relative:?} did not stabilize across {STABLE_READ_ATTEMPTS} \
         stat-read-stat attempts: it is being written during the read, or its \
         modification time cannot be read and a same-length edit therefore cannot be \
         distinguished from a stable one"
    )
}

#[derive(Clone)]
pub struct Daemon {
    store: Arc<Store>,
    root: std::path::PathBuf,
    batch_limit: usize,
    idle_poll: Duration,
    ipc_path: std::path::PathBuf,
    /// How long the daemon may sit with no IPC request, no pending work and
    /// no watcher event before retiring itself. `None` (the default) reads
    /// `DEVMAP_MAX_IDLE_SECS`, falling back to [`DEFAULT_MAX_IDLE_SECS`];
    /// a zero value disables retirement entirely.
    max_idle: Option<Option<Duration>>,
    /// Tripped by [`Daemon::request_shutdown`], and by SIGTERM/SIGINT, to end
    /// [`Daemon::run_loop`] through its orderly-release path.
    shutdown: Arc<tokio::sync::Notify>,
    /// The store file this daemon serves, when it has one on disk.
    ///
    /// `None` means "not stated" — an in-memory store, or a caller that did not
    /// say — and the store half of [`Daemon::vanished_reason`] is then skipped
    /// rather than guessed at. A guess here retires a working daemon.
    store_path: Option<std::path::PathBuf>,
}

/// What one drain established about git HEAD, from one reading of it.
///
/// Two fields rather than two calls: see [`Daemon::head_for_drain`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct DrainHead {
    /// The identity to stamp the generation with.
    sha: String,
    /// Whether it differs from the stored generation's, and the rows in that
    /// generation therefore describe a checkout that is no longer current.
    moved: bool,
}

/// One batch of watcher/discovery work, before it is persisted.
#[derive(Debug, Default)]
struct PendingDelta {
    affected: std::collections::BTreeSet<String>,
    deleted: std::collections::BTreeSet<String>,
    fresh: Vec<devmap_extract::Extraction>,
}

/// Default bounded lifetime for an idle daemon, in seconds.
///
/// Daemons are spawned detached (`start_new_session=True` on the Python side),
/// so every one whose spawning client exited used to outlive its consumer
/// indefinitely — six were found holding stores open across unrelated
/// repositories on one development machine, some running superseded binaries.
/// Identity of the executable backing this process: `(size, mtime)`.
///
/// `None` when it cannot be determined — a deleted or unreadable `/proc` entry,
/// a platform without `current_exe`. `None` compares equal to `None`, so an
/// undeterminable identity never *causes* a retirement; the daemon then behaves
/// exactly as it did before this check existed. Failing the other way would let
/// an unreadable executable path restart the daemon on every tick.
fn executable_identity() -> Option<(u64, std::time::SystemTime)> {
    let path = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

/// Whether a changed executable identity should retire the daemon.
///
/// Split from the tick so the policy can be asserted directly. The effect —
/// "process exits at some point in the next two seconds" — is not something a
/// test can observe without racing, and the branch guards a silent failure
/// (stale answers from a rebuilt kernel), which is exactly the kind that
/// survives an untested predicate.
fn should_retire_for_new_binary(
    started_as: Option<(u64, std::time::SystemTime)>,
    current: Option<(u64, std::time::SystemTime)>,
) -> bool {
    started_as != current
}

/// A generous idle bound retires them; the next client call respawns a fresh
/// one against the current kernel. Set `DEVMAP_MAX_IDLE_SECS=0` for a
/// never-exit daemon.
pub const DEFAULT_MAX_IDLE_SECS: u64 = 1800;

/// Pending rows one drain generation claims.
///
/// K1(h): this was 64, and the cap multiplied whole rebuilds rather than
/// bounding work. Every drain resolves and analyses the *entire* repository —
/// resolution is global by design — and commits one generation, so the cost of
/// a batch is essentially independent of how many paths it carries. Capping at
/// 64 therefore did not make a tick cheaper; it made a 50,000-path event (a
/// branch switch, a `git checkout` of a large tree) take about 780 consecutive
/// full rebuilds, one every two seconds, each writing a generation and pruning
/// the last.
///
/// 8,192 is a bound, not a budget: it exists so a pathological queue cannot
/// make one transaction unboundedly large, and it is far above any realistic
/// single event. Beyond it the queue still drains over consecutive ticks, as
/// before — just 128 times fewer of them.
pub const DEFAULT_DRAIN_BATCH_LIMIT: usize = 8192;

impl Daemon {
    pub fn new(store: Store, root: std::path::PathBuf) -> Self {
        let ipc_path = default_ipc_path_for(&root);
        Self {
            store: Arc::new(store),
            root,
            batch_limit: DEFAULT_DRAIN_BATCH_LIMIT,
            idle_poll: Duration::from_secs(2),
            ipc_path,
            max_idle: None,
            shutdown: Arc::new(tokio::sync::Notify::new()),
            store_path: None,
        }
    }

    /// Tell the daemon which file its store lives in, so it can notice the file
    /// being deleted out from under it.
    ///
    /// Passed in rather than read back off the `Store`, which does not expose
    /// the path it was opened from.
    pub fn with_store_path(mut self, store_path: std::path::PathBuf) -> Self {
        self.store_path = Some(store_path);
        self
    }

    /// End [`Self::run_loop`] the way a signal does: the loop returns, the IPC
    /// task is dropped, and the endpoint's socket and lock are removed before
    /// the call to `run_loop` resolves.
    ///
    /// Shares the path a SIGTERM takes rather than duplicating it, so a test
    /// that exercises this exercises the signal handling too — sending the
    /// process a real signal from inside a test would take down the whole test
    /// binary, and a second cleanup routine that only tests use would be a
    /// second thing to keep correct.
    pub fn request_shutdown(&self) {
        // `notify_one` stores a permit if nobody is waiting yet, so a shutdown
        // requested before the loop reaches its select is not lost.
        self.shutdown.notify_one();
    }

    pub fn with_batch_limit(mut self, batch_limit: usize) -> Self {
        self.batch_limit = batch_limit.max(1);
        self
    }

    pub fn with_ipc_path(mut self, ipc_path: std::path::PathBuf) -> Self {
        self.ipc_path = ipc_path;
        self
    }

    pub fn with_idle_poll(mut self, idle_poll: Duration) -> Self {
        self.idle_poll = idle_poll.max(Duration::from_millis(10));
        self
    }

    /// Override the bounded-idle lifetime. `Some(limit)` with a zero limit
    /// disables retirement; `None` defers to `DEVMAP_MAX_IDLE_SECS` and
    /// [`DEFAULT_MAX_IDLE_SECS`].
    pub fn with_max_idle(mut self, max_idle: Option<Duration>) -> Self {
        self.max_idle = Some(max_idle);
        self
    }

    fn resolved_max_idle(&self) -> Option<Duration> {
        match self.max_idle {
            Some(explicit) => explicit,
            None => match std::env::var("DEVMAP_MAX_IDLE_SECS") {
                Ok(raw) => match raw.trim().parse::<u64>() {
                    Ok(0) => None,
                    Ok(secs) => Some(Duration::from_secs(secs)),
                    Err(_) => {
                        warn!(
                            "DEVMAP_MAX_IDLE_SECS={raw:?} is not a non-negative integer; \
                             using the default {DEFAULT_MAX_IDLE_SECS}s"
                        );
                        Some(Duration::from_secs(DEFAULT_MAX_IDLE_SECS))
                    }
                },
                Err(_) => Some(Duration::from_secs(DEFAULT_MAX_IDLE_SECS)),
            },
        }
    }

    /// Why this daemon has nothing left to serve, if that is the case.
    ///
    /// A daemon outlives its repository routinely: a test harness builds a map
    /// under a temporary directory, the directory is deleted, and the daemon
    /// keeps running against a store that no longer exists. Measured on one
    /// development machine: **288** live daemons, one per deleted pytest
    /// temporary directory, each waiting out a 30-minute idle bound.
    ///
    /// The idle bound cannot catch this. It bounds *quiet*, and these daemons
    /// are not quiet — the watcher keeps firing on the deletion itself, the
    /// drain keeps failing and rescheduling — they are pointless, which is a
    /// different fact.
    ///
    /// Both halves are checked because either can go alone: `rm -rf` of the
    /// tree takes the store with it, but `rm` of the database leaves a live
    /// tree behind a daemon answering from a deleted inode.
    ///
    /// `None` is returned whenever the question cannot be answered — an
    /// in-memory store has no path — rather than a guess, because a false
    /// positive here retires a daemon that was working.
    fn vanished_reason(&self) -> Option<String> {
        match self.root.canonicalize() {
            Err(error) => {
                return Some(format!(
                    "repository root {:?} can no longer be resolved ({error})",
                    self.root
                ))
            }
            Ok(canonical) if !canonical.is_dir() => {
                return Some(format!(
                    "repository root {:?} is no longer a directory",
                    self.root
                ))
            }
            Ok(_) => {}
        }
        // Not stated (an in-memory store, or a caller that did not say): there
        // is nothing on disk this daemon claims, so there is nothing to check.
        let path = self.store_path.as_ref()?;
        if !path.exists() {
            return Some(format!("store {path:?} no longer exists"));
        }
        None
    }

    /// Reconcile the durable generation against disk before serving queries.
    /// Watchers are lossy across downtime and can miss racy edits, so startup
    /// must hash the current source set and enqueue changed, new, and deleted
    /// paths before it can truthfully report a fresh index.
    pub fn reconcile_connect_time(&self) -> anyhow::Result<usize> {
        let (sources, discovery) = collect_sources_with_report(&self.root)?;

        let previous: std::collections::BTreeMap<_, _> = self
            .store
            .latest_extractions()?
            .into_iter()
            .map(|extraction| (extraction.file_path, extraction.content_hash))
            .collect();
        let current: std::collections::BTreeMap<_, _> = sources
            .iter()
            .map(|(path, source)| (path.clone(), content_hash(source)))
            .collect();
        let mut pending = std::collections::BTreeSet::new();
        for (path, hash) in &current {
            if previous.get(path) != Some(hash) {
                pending.insert(path.clone());
            }
        }
        for path in previous.keys() {
            if !current.contains_key(path) {
                pending.insert(path.clone());
            }
        }
        for (path, reason) in discovery.skipped_paths {
            match reason {
                DiscoverySkipReason::NonSource => {}
                DiscoverySkipReason::Oversized { .. } | DiscoverySkipReason::Unreadable { .. } => {
                    // K1(c): reported as a refusal, not queued as work.
                    //
                    // Queuing these was self-defeating: the drain applies the
                    // *same* size and readability limits, so an oversized file
                    // enqueued here was guaranteed to fail every attempt until
                    // it quarantined, and then to sit in the queue forever
                    // holding `is_fresh` at false. The 30 MB vendored
                    // `parser.c` measured on this repository is exactly that —
                    // it is over `MAX_SOURCE_BYTES` and no retry will shrink
                    // it. A refusal is a fact about coverage, and the build
                    // path already prints it; it is not pending work.
                    //
                    // A file that later becomes readable or shrinks is picked
                    // up by the watcher event that changes it, or by the next
                    // connect-time sweep.
                    warn!(
                        "connect-time sweep refused {path:?} ({reason:?}); it is absent from \
                         the graph and is NOT queued — no retry can change the outcome"
                    );
                }
                DiscoverySkipReason::NonUtf8Path => {
                    // A name that cannot be represented as UTF-8 can never be
                    // stored in the pending queue, which is keyed by string.
                    // Skipping it degrades exactly like the CLI build — which
                    // reports the refusal and continues — instead of killing
                    // IPC availability for a repository that is otherwise
                    // perfectly indexable. The refusal is logged loudly so it
                    // cannot pass for complete coverage.
                    warn!(
                        "connect-time sweep skipped unrepresentable non-UTF-8 source path {path:?}"
                    );
                }
            }
        }
        let pending: Vec<_> = pending.into_iter().collect();
        // K1(a): one canonical spelling. `collect_sources_with_report` already
        // yields repo-relative paths, so this is a no-op for well-formed input
        // — and that is the point: the *same* call the watcher callback makes,
        // so the two producers can no longer write two spellings of one file.
        let report = self
            .store
            .enqueue_pending_paths_under_root(&self.root, &pending)?;
        for (path, reason) in &report.refused {
            warn!("connect-time sweep refused pending path {path:?}: {reason}");
        }
        Ok(report.enqueued.len())
    }

    fn collect_pending_path(
        &self,
        root: &std::path::Path,
        previous: &[devmap_extract::Extraction],
        pending: &str,
    ) -> anyhow::Result<PendingDelta> {
        self.collect_pending_path_with(root, previous, pending, &read_stable_source)
    }

    /// [`Self::collect_pending_path`] with the source read injected.
    ///
    /// The seam exists so a test can make the file vanish *between* the
    /// existence check and the read — the window this function has to survive
    /// and the one no test can schedule from the outside. Same reason
    /// [`read_stable_source_with`] has one.
    fn collect_pending_path_with(
        &self,
        root: &std::path::Path,
        previous: &[devmap_extract::Extraction],
        pending: &str,
        read_source: &dyn Fn(&std::path::Path, &str) -> anyhow::Result<String>,
    ) -> anyhow::Result<PendingDelta> {
        let raw = std::path::Path::new(pending);
        if raw
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            anyhow::bail!("pending path contains parent traversal: {pending:?}");
        }
        let candidate = if raw.is_absolute() {
            if let Ok(relative) = raw.strip_prefix(&self.root) {
                root.join(relative)
            } else {
                raw.to_path_buf()
            }
        } else {
            root.join(raw)
        };
        if !candidate.starts_with(root) {
            anyhow::bail!(
                "pending path escapes daemon root: {:?} is outside {:?}",
                candidate,
                root
            );
        }

        let mut delta = PendingDelta::default();
        if candidate.is_dir() {
            let canonical = candidate.canonicalize()?;
            if !canonical.starts_with(root) {
                anyhow::bail!("watched directory resolves outside daemon root: {canonical:?}");
            }
            let directory_prefix = stored_path(root, &canonical)?;
            let prefix = if directory_prefix.is_empty() {
                String::new()
            } else {
                format!("{directory_prefix}/")
            };
            let (sources, discovery) = collect_sources_with_report(&canonical)?;
            // Discovery refusals inside a changed directory degrade to
            // skipping that one path. Bailing the whole batch made a single
            // poison file permanently block its directory — the watcher then
            // retried with backoff forever. Naively dropping the bail would
            // have been worse: the refusal must not look like a deletion, and
            // the file still exists, so its stored row is kept untouched (it
            // stays at the last good extraction until it becomes readable
            // again).
            //
            // Which skips count is `DiscoverySkipReason::is_refusal`, through
            // `DiscoveryReport::refusals`, and nowhere else. Spelled here as
            // `!matches!(reason, NonSource)` it was a second copy of the rule
            // the CLI's build path reads from the owner — and a wildcard copy
            // at that, so a skip reason added later would silently default to
            // "not a refusal" on this path while the CLI stopped compiling
            // until someone chose a side.
            for (path, reason) in discovery.refusals() {
                warn!("refused source {path:?} in changed directory {canonical:?}: {reason:?}");
            }
            let refused: std::collections::BTreeSet<String> = discovery
                .refusals()
                .map(|(path, _)| {
                    // Discovery reports paths relative to the changed
                    // directory; stored rows are relative to the daemon
                    // root. Map through the same prefix the sources below
                    // use, or the deletion guard misses them.
                    if prefix.is_empty() {
                        path.clone()
                    } else {
                        format!("{prefix}{path}")
                    }
                })
                .collect();
            let mut live_under_directory = std::collections::BTreeSet::new();
            for (relative_to_directory, source) in sources {
                let path = if prefix.is_empty() {
                    relative_to_directory
                } else {
                    format!("{prefix}{relative_to_directory}")
                };
                live_under_directory.insert(path.clone());
                delta.affected.insert(path.clone());
                delta.fresh.push(extract_file(&path, &source));
            }
            for extraction in previous {
                if (prefix.is_empty() || extraction.file_path.starts_with(&prefix))
                    && !live_under_directory.contains(&extraction.file_path)
                    && !refused.contains(&extraction.file_path)
                {
                    delta.affected.insert(extraction.file_path.clone());
                    delta.deleted.insert(extraction.file_path.clone());
                }
            }
            return Ok(delta);
        }

        let relative = stored_path(root, &candidate)?;
        delta.affected.insert(relative.clone());
        // `Path::exists()` collapses *every* stat failure into `false`, not
        // just "no such file": a symlink loop (`ELOOP`), a parent that lost
        // `+x` (`EACCES`), a stale handle on a network mount. Each of those is
        // a question that could not be answered, and this branch answers with a
        // **deletion** — which drops the file's rows out of the generation. A
        // wrongly-kept row is merely stale; a wrongly-dropped one is a symbol
        // the dead-code pass is then free to call unreferenced. So only the
        // errors that actually mean "not there" may reconcile as a deletion,
        // and the rest are failures the queue retries and then quarantines,
        // which is where an unresolvable path is supposed to become visible.
        match std::fs::metadata(&candidate) {
            Ok(_) => {}
            Err(error) if path_is_absent(&error) => {
                delta.deleted.insert(relative);
                return Ok(delta);
            }
            Err(error) => {
                return Err(anyhow::Error::new(error).context(format!(
                    "cannot determine whether changed source {relative:?} still exists"
                )));
            }
        }
        // Everything from here down races an ordinary create-then-delete: the
        // existence check above is a stat, and the canonicalize, the stat inside
        // the stable read and the read itself are three more syscalls after it.
        // A file that goes away in any of those windows used to come back as a
        // *failure*, which charges the path a retry attempt — five of them and
        // the store quarantines it permanently. Editors writing scratch files,
        // a build emitting and removing intermediates, `git checkout` churn:
        // all of them are this, and none of them is an error. A path that is
        // gone is a deletion, which is the answer the absence branch above
        // already gives; losing a race to reach it must not change the answer.
        let vanished = |error: anyhow::Error| -> anyhow::Result<PendingDelta> {
            if !vanished_under_the_reader(&error) {
                return Err(error);
            }
            let mut delta = PendingDelta::default();
            delta.affected.insert(relative.clone());
            delta.deleted.insert(relative.clone());
            info!(
                "changed source {relative:?} was removed while it was being read; \
                 reconciling it as a deletion rather than charging a retry: {error}"
            );
            Ok(delta)
        };
        let canonical = match candidate.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) => return vanished(error.into()),
        };
        if !canonical.starts_with(root) {
            anyhow::bail!("watched file resolves outside daemon root: {canonical:?}");
        }
        // Same rule as the existence check above: `is_file()` is another stat,
        // and swallowing its failure into `false` turns "I could not look" into
        // "not a regular file, drop its rows".
        let metadata = match std::fs::metadata(&canonical) {
            Ok(metadata) => metadata,
            Err(error) => return vanished(anyhow::Error::new(error)),
        };
        if !metadata.is_file() || !is_indexable_source(&relative) {
            delta.deleted.insert(relative);
            return Ok(delta);
        }
        let source = match read_source(&canonical, &relative) {
            Ok(source) => source,
            Err(error) => return vanished(error),
        };
        delta.fresh.push(extract_file(&relative, &source));
        Ok(delta)
    }

    /// The git HEAD one drain builds against, read exactly once. (B5)
    ///
    /// Both facts the drain needs about HEAD come out of a *single* reading,
    /// because they are two uses of one observation and the old code took two:
    ///
    /// - `moved` decided whether the rows in the last generation may be carried
    ///   forward, from `current_git_head` called near the top of the drain;
    /// - `sha` stamped the generation about to be written, from a *second*
    ///   `current_git_head` called after extraction, resolution and analysis —
    ///   seconds later on any real repository.
    ///
    /// A checkout landing between the two made the drain decide "HEAD has not
    /// moved, carry the rows forward" and then stamp the result with the HEAD
    /// it had moved to. That stamp is what made the damage permanent rather
    /// than transient: the very next drain compares the current HEAD against
    /// the one just stored, finds them equal, and carries forward again. The
    /// rebuild the checkout called for is never run, by a check reading its own
    /// mistake back as proof that nothing happened.
    ///
    /// `moved` is also no longer gated on a git-HEAD sentinel being in the same
    /// claimed batch. The sentinel is what *wakes* the daemon when only `.git`
    /// changed; it is not evidence, and requiring it meant an ordinary file
    /// event drained at a moved HEAD took the carry-forward path — then stamped
    /// the new HEAD and suppressed the sentinel's own rebuild one batch later.
    /// Comparing unconditionally costs nothing: the drain has to read HEAD for
    /// the stamp regardless, and this is that same read.
    ///
    /// Fails *safe*, not quiet. A stored `head_sha` that cannot be read answers
    /// `moved`, forcing a full rebuild: a redundant one costs time while a
    /// skipped one costs correctness. An unreadable *current* HEAD is folded
    /// into the stamp instead, for the reason stated at the comparison below.
    fn head_for_drain(
        &self,
        root: &std::path::Path,
        read_head: &dyn Fn(&std::path::Path) -> anyhow::Result<String>,
    ) -> DrainHead {
        // `"unavailable"` is what this kernel stamps when HEAD cannot be read —
        // `devmap build` uses the same spelling, and
        // `latest_generation_head_sha` documents that it comes back verbatim
        // rather than as "never built".
        let sha = match read_head(root) {
            Ok(sha) => sha,
            Err(error) => {
                warn!("cannot read git HEAD for {root:?}: {error}");
                "unavailable".to_string()
            }
        };
        // Compared against the *stamp*, not against the raw reading. The two
        // differ in exactly one case and it matters: outside a git repository
        // every reading fails, so treating an unreadable HEAD as "moved" made
        // every drain in such a tree a full re-extraction, for ever. Comparing
        // stamps makes the rule self-consistent — this drain stores `sha`, so
        // the next one finds them equal and stops — while keeping every real
        // move loud: a stored `abc123` against an unreadable HEAD still differs
        // from `"unavailable"` and still rebuilds.
        //
        // The residual: a checkout that moves while HEAD stays unreadable is
        // not noticed *by this check*. It is not lost — the connect-time sweep
        // content-hashes the whole tree and the watcher sees the files a
        // checkout rewrites — only the shortcut is. Weighed against a daemon
        // that re-extracts the entire repository on every batch, which is what
        // the alternative actually did.
        let moved = match self.store.latest_generation_head_sha() {
            // No generation yet: nothing to invalidate.
            Ok(None) => false,
            Ok(Some(stored)) => sha != stored,
            Err(error) => {
                warn!("cannot read the stored head_sha, assuming HEAD moved: {error}");
                true
            }
        };
        DrainHead { sha, moved }
    }

    /// Process up to `batch_limit` claimed paths. Only claimed files are read
    /// from disk; unchanged extraction payloads come from the durable generation.
    /// Attempts are recorded before work starts, and paths are acknowledged only
    /// after extraction, resolution, analysis, and persistence all succeed.
    pub fn drain_pending_batch(&self) -> anyhow::Result<usize> {
        self.drain_pending_batch_with_head(&current_git_head)
    }

    /// [`Self::drain_pending_batch`] with the HEAD reading injected.
    ///
    /// The seam exists for the same reason [`read_stable_source_with`]'s does:
    /// the property under test is *when* the kernel looks at something that
    /// another process is free to change, and a test cannot schedule a `git
    /// checkout` into the middle of a resolve. With the reading injected it can
    /// hand back a different answer on a second call and assert there is no
    /// second call.
    fn drain_pending_batch_with_head(
        &self,
        read_head: &dyn Fn(&std::path::Path) -> anyhow::Result<String>,
    ) -> anyhow::Result<usize> {
        let claims = self.store.claim_pending_batch(self.batch_limit)?;
        if claims.is_empty() {
            return Ok(0);
        }
        let batch: Vec<String> = claims.iter().map(|claim| claim.path.clone()).collect();
        // Timed from the moment real work starts, so the history row reflects
        // incremental resync cost rather than idle polling.
        let resync_started = std::time::Instant::now();
        // K1(d): attempts are charged *after* the per-path loop, and only to
        // the paths that actually failed. Bumping the whole batch up front made
        // every path share one fate: any failure in a later, batch-wide step —
        // a persist, a prune — returned before the acknowledgement below, so
        // all 64 claimed paths carried an attempt they had not earned. Five
        // such failures quarantined the entire batch, which is precisely the
        // count of permanently stuck rows measured on this repository's store.

        let root = self.root.canonicalize().map_err(|error| {
            anyhow::anyhow!("cannot canonicalize daemon root {:?}: {error}", self.root)
        })?;
        let previous = self.store.latest_extractions()?;
        let mut affected = std::collections::BTreeSet::new();
        let mut deleted = std::collections::BTreeSet::new();
        // Keyed by path, because two queue entries can cover one file: a
        // directory and a file inside it arrive together in a single watcher
        // batch, and a whole-tree rescan sits beside whatever per-path events
        // survived the drop that caused it. The store refuses a generation
        // whose input names a path twice, so an overlapping batch used to fail
        // on every retry — backing off to 64 s while `pending_count` stayed
        // non-zero — until the paths quarantined. Both entries describe the
        // same file on disk, so the later read simply wins.
        let mut fresh: std::collections::BTreeMap<String, devmap_extract::Extraction> =
            std::collections::BTreeMap::new();

        let mut succeeded: Vec<devmap_store::PendingClaim> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut failures = Vec::new();
        // Indexed once rather than scanned per path. `claims` is bounded by
        // `batch_limit` (8192) and `claim_of` runs once per path in it, so the
        // linear `find` this replaces made the lookup step itself O(batch) and
        // the drain O(batch²) — a stride is not a bound when one step is not.
        // The constant was raised from 64 to 8,192 precisely so that large
        // batches become normal, which is what turned the scan from a cost into
        // a bound nobody was holding.
        //
        // The map is *exactly* equivalent to the scan, not merely faster:
        // `pending_paths.path` is the table's conflict target, so a claim set
        // holds each path once — same claim for every path, same panic on a
        // path that came from no claim. Measured, rustc -O: batch 1024
        // 898.8 us -> 45.5 us; 4096 16.18 ms -> 286.3 us; 8192 (the real bound)
        // 62.35 ms -> 348.6 us.
        let claim_of: std::collections::HashMap<&str, &devmap_store::PendingClaim> = claims
            .iter()
            .map(|claim| (claim.path.as_str(), claim))
            .collect();
        let claim_of = |path: &str| {
            claim_of
                .get(path)
                .map(|claim| (*claim).clone())
                .expect("every batch path came from a claim")
        };
        // B5: git moved HEAD or a ref. Not a file to extract — a statement that
        // the generation may describe a tree that no longer exists. Consumed
        // here so `collect_pending_path` never sees a path that is not one, and
        // acknowledged either way so it cannot be redelivered forever.
        let head_event = batch
            .iter()
            .any(|pending| pending == crate::watcher::GIT_HEAD_SENTINEL);
        // One reading, used both to decide and to stamp. See `head_for_drain`.
        let head = self.head_for_drain(&root, read_head);
        let head_moved = head.moved;
        if head_event {
            succeeded.push(claim_of(crate::watcher::GIT_HEAD_SENTINEL));
        }
        for pending in &batch {
            if pending == crate::watcher::GIT_HEAD_SENTINEL {
                continue;
            }
            match self.collect_pending_path(&root, &previous, pending) {
                Ok(delta) => {
                    affected.extend(delta.affected);
                    deleted.extend(delta.deleted);
                    for extraction in delta.fresh {
                        fresh.insert(extraction.file_path.clone(), extraction);
                    }
                    succeeded.push(claim_of(pending));
                }
                Err(error) => {
                    warn!("pending path {pending:?} failed in isolation: {error}");
                    failed.push(pending.clone());
                    // A sample, kept at a fixed size. The batch is bounded at
                    // `batch_limit` (8192) and every entry here carries a whole
                    // rendered error, so accumulating one per path built an
                    // error message whose length was a function of how badly
                    // the batch went — megabytes into a single `warn!` line,
                    // re-emitted on every retry. `failed.len()` is the honest
                    // total and is reported beside the sample.
                    if failures.len() < DRAIN_FAILURE_SAMPLE {
                        failures.push(format!("{pending}: {error}"));
                    }
                }
            }
        }

        // Charge the attempt now, to the paths that earned it, before any
        // batch-wide step can fail and take the whole batch down with it.
        if !failed.is_empty() {
            self.store.bump_pending_attempts(&failed)?;
        }

        if succeeded.is_empty() {
            // Both numbers, never just the sample. `StoreStatus::degraded_reason`
            // reports quarantined paths the same way and for the same reason: a
            // list that stops at five without saying so reads exactly like a
            // batch in which only five things went wrong.
            let elided = failed.len().saturating_sub(failures.len());
            let shown = failures.join("; ");
            if elided > 0 {
                anyhow::bail!(
                    "all {} claimed path(s) failed; first {} of them: {shown}, \
                     and {elided} more not shown",
                    failed.len(),
                    failures.len()
                );
            }
            anyhow::bail!("all {} claimed path(s) failed: {shown}", failed.len());
        }
        // One entry per file from here down, whatever the queue asked for.
        let fresh: Vec<devmap_extract::Extraction> = fresh.into_values().collect();

        // Everything below reuses `previous` — the payloads stored by whichever
        // kernel last wrote a generation. After an extractor or grammar upgrade
        // those describe the same bytes with an older schema, and neither the
        // resolution built from them nor the rows carried forward from them are
        // this build's answer. The store refuses to carry them; the daemon's
        // way out is to re-extract the tree once and write a full generation,
        // rather than replaying a batch that can never succeed.
        let payload_is_current = self.store.latest_generation_payload_is_current()?;
        if !payload_is_current {
            warn!(
                "stored payloads predate this kernel; re-extracting {:?} in full before resyncing",
                self.root
            );
        }
        if head_moved {
            warn!(
                "git HEAD moved since the last generation; re-extracting {:?} in full",
                self.root
            );
        }
        // A moved HEAD invalidates the carry-forward for the same reason a
        // stale payload does: `previous` describes a different checkout, and
        // every file it holds that this batch did not touch may now differ.
        // Carrying them forward would leave the graph describing a mixture of
        // two commits, which is worse than describing the old one.
        // K-A2, daemon half. What discovery refused cannot be seen in
        // `extractions` — a file turned away has no `Extraction` at all — so it
        // must travel beside them, or the analysis below reports full coverage
        // of a corpus it never fully saw. Getting this wrong here does not just
        // produce one bad answer: the drain *overwrites* the stored
        // `analysis_json`, so it also erases the correct `Partial` that
        // `devmap build` recorded, and one watcher event is enough to do it.
        let (mut extractions, full_rebuild, discovery) = if payload_is_current && !head_moved {
            let mut carried: Vec<_> = previous
                .into_iter()
                .filter(|extraction| !affected.contains(&extraction.file_path))
                .collect();
            carried.extend(fresh.iter().cloned());
            // This branch never re-walks discovery, so it cannot measure
            // refusals — but it can decline to *deny* them. The previous
            // generation measured this same tree, and a file it could not read
            // is still unread unless something changed it.
            //
            // Known residual: once the refused file is fixed — shrunk below the
            // ceiling, made readable — this count stays high until a full
            // re-extraction or a `devmap build` re-measures. That under-claims
            // coverage rather than over-claiming it, which is the side of the
            // trade the rest of the kernel is built on: an unread file wrongly
            // reported as read is what deletes working code.
            let discovery = match self
                .store
                .latest_analysis()?
                .and_then(|summary| summary.discovery_refused_files)
            {
                Some(refused) => DiscoveryCoverage::refused(refused),
                // Only reachable for a generation written before the count was
                // recorded at all. `none()` leaves it honestly unmeasured
                // instead of asserting zero — and the payload check above sends
                // a store that old down the full-rebuild branch regardless.
                None => DiscoveryCoverage::none(),
            };
            (carried, false, discovery)
        } else {
            // The branch that actually walks the tree is the one that can
            // measure it. This report was discarded as `_report`, which is what
            // made a full re-extraction the *most* confident thing the daemon
            // did and the least entitled to be.
            let (whole_tree, report) =
                devmap_store::extract_tree_cached_with_report(&self.store, &self.root)?;
            (
                whole_tree,
                true,
                DiscoveryCoverage::refused(report.refused_count()),
            )
        };
        extractions.sort_by(|left, right| left.file_path.cmp(&right.file_path));
        let mut resolver = Resolver::new();
        match collect_go_modules(&self.root) {
            Ok(modules) => resolver.index_go_modules(&modules),
            Err(error) => warn!("go.mod collection failed for {:?}: {error}", self.root),
        }
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = analyze_with_discovery(&extractions, &resolution, discovery);
        // The reading the carry-forward decision was made against, not a fresh
        // one: a checkout that landed while the resolve above was running must
        // leave the generation stamped at the HEAD it actually describes, so
        // the next drain still sees a difference and rebuilds. See
        // `head_for_drain`.
        let head_sha = head.sha;
        // K13: hold the cross-process writer lock across persist + prune. A
        // `devmap build` running beside the daemon otherwise races it on
        // SQLite's busy timeout alone, and the loser surfaces `database is
        // locked` after paying for a full resolve. Taken *here* rather than at
        // the top of the drain because everything above is reads and
        // extraction, which two writers may safely do at once.
        let _writer = self
            .store
            .lock_writer(devmap_store::Store::WRITER_LOCK_WAIT)?;
        self.store.save_generation_with_metadata(
            if full_rebuild { &extractions } else { &fresh },
            &resolution,
            &analysis,
            GenerationWriteOpts {
                // A full rebuild declares nothing affected: an empty affected
                // set *is* the full-rewrite signal, and every row it needs is
                // in the extractions above.
                affected_paths: if full_rebuild {
                    Vec::new()
                } else {
                    affected.into_iter().collect()
                },
                deleted_paths: if full_rebuild {
                    Vec::new()
                } else {
                    deleted.into_iter().collect()
                },
                repo_root: Some(root.to_string_lossy().into_owned()),
                build_started: Some(resync_started),
            },
            &head_sha,
        )?;

        // Bound retention on the write path that runs most often. An always-on
        // watcher commits a generation per edit batch, and each one carries a
        // full copy of the repository (SC1). Vacuuming is left to the
        // maintenance loop so a watcher tick never stalls on an exclusive lock.
        self.store
            .prune_generations_except_latest(GENERATION_RETENTION)?;
        // The watcher is where the cache leak actually bites: one row per edit
        // per file, forever (SC7). Runs after the generation prune so it sees
        // the retained set.
        self.store.prune_extraction_cache()?;

        // Acknowledge only after a complete durable generation exists. A crash
        // or any failed stage above leaves the claimed work queued for replay —
        // now without an attempt charged against it, so a store-level fault can
        // no longer quarantine paths that never failed.
        //
        // The claim's `queued_at` is the guard: a watcher event that arrived
        // while this batch was resolving re-enqueued the path with a newer
        // timestamp, and that row survives the acknowledgement.
        self.store.clear_claimed_pending_paths(&succeeded)?;
        Ok(succeeded.len())
    }

    /// Long-lived loop: file watcher enqueues durable pending paths; idle poll drains them.
    /// Does not return until cancelled / fatal error / idle retirement.
    ///
    /// The IPC listener binds *before* the connect-time reconcile. Reconcile
    /// hashes the whole source tree, which on any real repository takes
    /// seconds; binding first means `status` answers immediately from the last
    /// committed generation (honestly reporting the pending work), instead of
    /// clients seeing a dead endpoint for the entire sweep — the Python client
    /// gave up and killed the daemon it had just spawned after three seconds,
    /// then fell back to re-doing the work through the CLI, on every call.
    pub async fn run_loop(&self) -> anyhow::Result<()> {
        info!(
            "DevMap daemon started for {:?} (batch_limit={})",
            self.root, self.batch_limit
        );
        if let Some(limit) = self.resolved_max_idle() {
            info!(
                "daemon retires after {limit:?} with no requests, pending work or \
                 watcher events (DEVMAP_MAX_IDLE_SECS=0 disables)"
            );
        } else {
            info!("bounded-idle retirement disabled (DEVMAP_MAX_IDLE_SECS=0)");
        }

        let activity = Arc::new(crate::protocol::Activity::default());
        let max_idle = self.resolved_max_idle();

        // Installed before the endpoint is bound, so a signal arriving at any
        // point after it exists is queued rather than missed — and so this
        // failure, which is fatal, happens while there is still no socket on
        // disk to strand. A daemon that cannot install them is a daemon whose
        // socket a `kill` would leave behind.
        let mut signals = ShutdownSignals::install()?;

        let store = Arc::clone(&self.store);
        let root = self.root.clone();
        // The watcher canonicalizes its own root, so the absolute paths it
        // emits are canonical-rooted; normalise against the same form or every
        // event from a symlinked tree looks like an escape.
        let enqueue_root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let watcher_activity = Arc::clone(&activity);
        let _watcher = start_file_watcher(root, move |paths| {
            if paths.is_empty() {
                return;
            }
            watcher_activity.touch();
            // K1(a): the watcher emits *absolute* paths while the connect-time
            // sweep emits repo-relative ones, and the queue used to store both
            // verbatim. That is how this repository's store came to hold 64
            // rows naming a directory the checkout had moved out of, none of
            // which any drain could process and nothing could delete. Both
            // producers now write the same canonical, root-checked form.
            match store.enqueue_pending_paths_under_root(&enqueue_root, &paths) {
                Ok(report) => {
                    for (path, reason) in &report.refused {
                        warn!("watcher emitted a path outside the tree: {path:?} ({reason})");
                    }
                    info!("enqueued {} changed path(s)", report.enqueued.len());
                }
                Err(err) => warn!("failed to enqueue pending paths: {err}"),
            }
        })?;

        #[cfg(unix)]
        let mut ipc_task = AbortTaskOnDrop({
            let server = crate::protocol::UnixIpcServer::bind(&self.ipc_path)?;
            let store = Arc::clone(&self.store);
            tokio::spawn(server.run(store, Arc::clone(&activity)))
        });

        #[cfg(windows)]
        let mut ipc_task = AbortTaskOnDrop({
            let store = Arc::clone(&self.store);
            let name = self.ipc_path.to_string_lossy().into_owned();
            tokio::spawn(async move {
                crate::protocol::run_named_pipe(store, &name, Arc::clone(&activity)).await
            })
        });

        // Reconcile after the endpoint is live: a failure here is logged and
        // survived rather than fatal. Queries keep answering from the last
        // committed generation, and the drain loop still replays whatever was
        // enqueued before the failure; a daemon that refused to serve until
        // its startup sweep succeeded turned one unreadable path into a full
        // map outage.
        match self.reconcile_connect_time() {
            Ok(reconciled) => {
                if reconciled > 0 {
                    info!("connect-time sweep enqueued {reconciled} changed path(s)");
                }
            }
            Err(err) => warn!("connect-time sweep failed; serving stale generation: {err}"),
        }

        let maintenance_store = Arc::clone(&self.store);
        let _maintenance_task = AbortTaskOnDrop(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                match maintenance_store.checkpoint_wal() {
                    Ok(result) if result.busy != 0 => warn!(
                        "WAL checkpoint remained busy after {:?} fallback: {}/{} frames checkpointed",
                        result.mode, result.checkpointed_frames, result.log_frames
                    ),
                    Ok(_) => {}
                    Err(err) => warn!("WAL checkpoint failed: {err}"),
                }
                // K13: reclaim is a write. Take the same cross-process writer
                // lock a build takes, so a maintenance vacuum and a concurrent
                // `devmap build` queue instead of racing on SQLite's busy
                // timeout. The wait is short and a miss is skipped rather than
                // retried: this loop runs every five minutes, so losing one
                // pass costs nothing, while blocking a build for a minute to
                // reclaim pages would be the wrong trade.
                match maintenance_store.lock_writer(Duration::from_secs(5)) {
                    Ok(_writer) => {
                        if let Err(err) = maintenance_store.vacuum_if_needed() {
                            warn!("vacuum_if_needed failed: {err}");
                        }
                    }
                    Err(err) => {
                        info!("skipping maintenance vacuum; another writer holds the store: {err}")
                    }
                }
            }
        }));

        let mut ticker = tokio::time::interval(self.idle_poll);
        let mut consecutive_failures = 0u32;
        let mut next_attempt = tokio::time::Instant::now();
        // An untouched activity record means no consumer ever spoke to this
        // daemon — idle time runs from loop start, not from zero. That is the
        // orphan case: spawned, used once, client died, nothing left to ask.
        let started_at = std::time::Instant::now();
        // Captured once, at startup, so the tick below compares against what
        // this process was actually launched from. See the retirement check.
        let started_as = executable_identity();
        // Every way out of this loop is a `break` carrying a [`LoopExit`], and
        // the single release below is the only caller of
        // `release_ipc_endpoint`. Two of the six exits — the binary-replacement
        // retirement and the idle retirement, the latter being the *routine*
        // one — used to `return` straight out and skip it, which is how the
        // orderly path arrived at the same stale-socket state a `kill -9`
        // leaves. Adding a `return` here would reintroduce that; there is
        // nothing to return to but the `break`.
        let exit = loop {
            tokio::select! {
                result = &mut ipc_task.0 => {
                    break LoopExit::IpcTaskEnded(
                        result
                            .map_err(|error| anyhow::anyhow!("IPC task join failed: {error}"))
                            .and_then(|served| served),
                    );
                }
                // Both shutdown routes converge here. Without them the socket
                // file was removed only by `UnixIpcServer`'s `Drop`, which a
                // signal never reaches: `kill` ended the process where it stood
                // and left `/tmp/devmap-*/ipc.sock` on disk, so the next
                // daemon's start hung on the liveness probe deciding whether
                // the corpse was alive.
                signal = signals.recv() => {
                    info!(
                        "{signal} received; releasing the IPC endpoint and exiting \
                         (pending work stays queued in the store)"
                    );
                    break LoopExit::ReleaseEndpoint(Ok(()));
                }
                _ = self.shutdown.notified() => {
                    info!(
                        "shutdown requested; releasing the IPC endpoint and exiting \
                         (pending work stays queued in the store)"
                    );
                    break LoopExit::ReleaseEndpoint(Ok(()));
                }
                _ = ticker.tick() => {
                    // Exit when there is nothing left to serve. Checked before
                    // every other rule on this tick, because a daemon whose
                    // tree or store is gone has no correct answer to give and
                    // no idle bound that would ever end it — the watcher keeps
                    // firing on the deletion and the drain keeps failing, so it
                    // never looks idle.
                    if let Some(reason) = self.vanished_reason() {
                        info!(
                            "{reason}; releasing the IPC endpoint and exiting \
                             (nothing left to serve)"
                        );
                        break LoopExit::ReleaseEndpoint(Ok(()));
                    }
                    // Retire when the binary that started this process has been
                    // replaced on disk.
                    //
                    // The IPC handshake checks `PROTOCOL_VERSION`, which is a
                    // wire-format number, not a build identity — it does not
                    // move when the kernel is rebuilt. So a daemon started
                    // before a `cargo build` keeps answering from the old code
                    // for up to its full idle bound (30 minutes by default),
                    // and every client gets pre-fix answers from a tree that
                    // has been fixed. Observed exactly that while fixing the
                    // oversized-search-hit bug: the rebuilt kernel returned the
                    // match, the daemon in front of it kept returning nothing,
                    // and the difference was invisible from either side.
                    //
                    // Identity is (size, mtime) rather than a version string,
                    // for the reason `find_engine_binary` already documents:
                    // every build of this workspace reports `devmap 0.1.0`, so
                    // a version cannot distinguish two of them.
                    //
                    // Unlike the idle bound below, this does **not** wait for
                    // the pending queue to drain.
                    //
                    // The idle bound waits because retiring mid-queue would
                    // stall the resync until some future client respawned us.
                    // That reasoning inverts here: draining under a superseded
                    // binary means the old kernel writes the generation, which
                    // is the outcome this check exists to prevent. The queue is
                    // persisted in the store, not held in memory, so the daemon
                    // the next client spawns picks up exactly the same paths and
                    // drains them with the new kernel — nothing is lost by
                    // leaving now.
                    //
                    // Waiting would also make the check unreliable precisely
                    // where it matters most: on a repository busy enough that
                    // the queue never empties, a stale daemon would serve old
                    // answers indefinitely.
                    if should_retire_for_new_binary(started_as, executable_identity()) {
                        info!(
                            "devmap binary changed on disk since this daemon started; \
                             retiring so the next client gets the current kernel \
                             (pending work stays queued in the store)"
                        );
                        break LoopExit::ReleaseEndpoint(Ok(()));
                    }
                    if let Some(limit) = max_idle {
                        let idle_for = activity
                            .idle_for()
                            .unwrap_or_else(|| started_at.elapsed());
                        if idle_for >= limit {
                            // Pending work keeps the daemon alive through its
                            // idle bound: retiring mid-queue would stall the
                            // resync until some future client respawned us.
                            //
                            // A store that cannot answer is not evidence that
                            // the queue is empty, so it ends the loop through
                            // the release rather than through `?`.
                            match self.store.get_pending_paths() {
                                Ok(pending) if pending.is_empty() => {
                                    info!(
                                        "no IPC request, pending work or watcher event \
                                         for {idle_for:?}; retiring daemon"
                                    );
                                    break LoopExit::ReleaseEndpoint(Ok(()));
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    break LoopExit::ReleaseEndpoint(Err(error.into()));
                                }
                            }
                        }
                    }
                    if tokio::time::Instant::now() < next_attempt {
                        continue;
                    }
                    let worker = self.clone();
                    let drain_result = tokio::task::spawn_blocking(move || {
                        worker.drain_pending_batch()
                    })
                    .await
                    .map_err(|error| anyhow::anyhow!("pending drain task failed: {error}"))
                    .and_then(|result| result);
                    match drain_result {
                        Ok(0) => consecutive_failures = 0,
                        Ok(n) => {
                            consecutive_failures = 0;
                            info!("drained {n} pending path(s)");
                        }
                        Err(err) => {
                            // A failed drain is the loudest symptom of a
                            // deleted tree or store, and backing off means
                            // waiting up to 64 seconds before the next tick
                            // would notice. Ask now rather than schedule a
                            // retry against something that is gone.
                            if let Some(reason) = self.vanished_reason() {
                                info!(
                                    "pending drain failed and {reason}; releasing the \
                                     IPC endpoint and exiting (nothing left to serve): {err}"
                                );
                                break LoopExit::ReleaseEndpoint(Ok(()));
                            }
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            let exponent = consecutive_failures.saturating_sub(1).min(6);
                            let delay = Duration::from_secs(1u64 << exponent);
                            next_attempt = tokio::time::Instant::now() + delay;
                            warn!(
                                "pending drain failed (attempt {consecutive_failures}; retry in {delay:?}): {err}"
                            );
                        }
                    }
                }
            }
        };

        match exit {
            // The IPC future has already completed, so `UnixIpcServer::drop`
            // has already run and the endpoint is gone. Awaiting the handle
            // again would poll a `JoinHandle` whose output was taken.
            LoopExit::IpcTaskEnded(result) => result,
            LoopExit::ReleaseEndpoint(result) => {
                release_ipc_endpoint(&mut ipc_task).await;
                result
            }
        }
    }
}

/// How [`Daemon::run_loop`] stopped, and therefore whether the IPC endpoint
/// still has to be released before the call resolves.
///
/// Two states rather than a bare `Result`, because "the endpoint is already
/// gone" and "the endpoint is still bound" are the only two shapes an exit can
/// have and the wrong one is invisible: `AbortTaskOnDrop` *schedules* the drop
/// that removes the socket, so a `return` that skips the await usually looks
/// fine in a test — the runtime gets around to it — and strands the socket in
/// production, where the process exits instead.
enum LoopExit {
    /// The IPC task ended on its own; its future is complete and its listener
    /// dropped with it.
    IpcTaskEnded(anyhow::Result<()>),
    /// Every other exit. The listener is still live and must be awaited away.
    ReleaseEndpoint(anyhow::Result<()>),
}

/// Identity of a repository's IPC endpoint.
///
/// # The formula, exactly
///
/// A second implementation has to reproduce this — the Python client derives
/// the same path without spawning anything, and when the two disagree each side
/// starts its own daemon against one store — so every step is stated rather
/// than implied:
///
/// 1. **Input**: the repository root, *canonicalized* (`realpath`): symlinks
///    resolved, `.`/`..` removed, absolute. Not the string the user typed.
/// 2. **Encoding**: that canonical path's bytes as UTF-8, with no trailing
///    separator and no terminator.
/// 3. **Hash**: FNV-1a, 64-bit. Offset basis `0xcbf29ce484222325`, prime
///    `0x100000001b3`; per byte, `hash = (hash XOR byte) * prime`, multiplication
///    wrapping at 64 bits. This is `devmap_extract::content_hash`, reused rather
///    than re-spelled so the constants have one definition.
/// 4. **Rendering**: lowercase hexadecimal, zero-padded to exactly 16 digits.
/// 5. **Directory**: `<system temp dir>/devmap-<hex>`, created mode `0700`.
/// 6. **Socket**: the file `ipc.sock` inside it. On Windows, the pipe
///    `\\.\pipe\devmap-<hex>` instead, with no directory.
///
/// `devmap serve --print-socket-path <root>` prints exactly this and creates
/// nothing, so the two implementations can be checked against each other.
///
/// FNV-1a rather than a cryptographic digest because this is a namespacing
/// hash, not a security boundary — the socket's protection is the `0700`
/// directory and the `0600` socket, not the unguessability of the name.
///
/// # Why canonical
///
/// It hashed the root *as written*, so `devmap serve .`, `devmap serve
/// /tmp/repo` and a symlinked path were three endpoints for one repository,
/// each with its own daemon on the same store.
///
/// Canonicalization that fails — the root does not exist, or is unreadable —
/// falls back to the path as given rather than to a wrong root: a
/// non-existent repository has no daemon to collide with, and refusing here
/// would turn a mistyped path into a startup crash instead of a clear "no such
/// directory" from the store.
pub fn ipc_identity_for(root: &std::path::Path) -> u64 {
    let canonical = root.canonicalize();
    let canonical = canonical.as_deref().unwrap_or(root);
    devmap_extract::content_hash(&canonical.to_string_lossy())
}

/// `<temp_dir>/devmap-{ipc_identity_for(root):016x}/ipc.sock`.
///
/// The per-repository directory is created 0700 at bind time (see
/// `UnixIpcServer::bind`), so the socket is not merely owner-only itself but
/// sits behind an owner-only directory.
#[cfg(unix)]
pub fn default_ipc_path_for(root: &std::path::Path) -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!("devmap-{:016x}", ipc_identity_for(root)))
        .join("ipc.sock")
}

/// `\\.\pipe\devmap-{ipc_identity_for(root):016x}`.
#[cfg(windows)]
pub fn default_ipc_path_for(root: &std::path::Path) -> std::path::PathBuf {
    format!(r"\\.\pipe\devmap-{:016x}", ipc_identity_for(root)).into()
}

/// The repo-relative key `candidate` is stored under.
///
/// Refuses a name whose bytes are not UTF-8 rather than converting lossily.
/// The lossy spelling is not this file — it is a *different* path, with
/// `U+FFFD` where the bytes were, naming something that does not exist. Stored,
/// it becomes a row no lookup can ever match and no deletion pass can ever
/// clear; used as an extraction key, it claims the daemon indexed a file it
/// did not. Both are the same defect: a conversion that could not be performed
/// answering exactly like one that was.
///
/// The watcher refuses these before they reach the queue, so the reachable
/// route here is a symlinked component resolving through an unrepresentable
/// name during `canonicalize`. A refusal costs the path a retry attempt and,
/// after `MAX_PENDING_ATTEMPTS`, a place in `degraded_reason` — which is the
/// honest end state for a path this kernel cannot index, and the one
/// `collect_sources_with_report` already gives it on the build path.
fn stored_path(root: &std::path::Path, candidate: &std::path::Path) -> anyhow::Result<String> {
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| anyhow::anyhow!("path {:?} is outside daemon root {:?}", candidate, root))?;
    let relative = relative.to_str().ok_or_else(|| {
        anyhow::anyhow!(
            "path {:?} is not UTF-8 relative to daemon root {:?}; it cannot be stored \
             and is absent from the graph",
            candidate,
            root
        )
    })?;
    Ok(relative.replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // The fixtures below build their own corpora, so the plain two-argument
    // form is the correct one for them: no discovery step ran over what they
    // assembled. The drain itself must never use it.
    use devmap_analyze::analyze;

    /// The HEAD identity a drain reads in a scratch tree that is not a git
    /// repository: `current_git_head` fails there, and the drain stamps
    /// `"unavailable"` — the same spelling `devmap build` uses outside git.
    ///
    /// Fixtures below record it explicitly rather than using `save_generation`,
    /// whose convenience default is `"unknown"`. `"unknown"` is not a claim
    /// that HEAD is unchanged, it is the absence of a claim, and the drain
    /// answers an absent claim by rebuilding rather than carrying rows forward
    /// from a checkout it cannot vouch for. Every real writer — `devmap build`,
    /// the extraction cache, this drain — records a real identity, so
    /// `"unknown"` reaches no production store; a fixture that leaves it there
    /// silently moves the test onto the full-rebuild branch and stops
    /// exercising the differential one it was written for.
    const SCRATCH_HEAD: &str = "unavailable";

    /// `Store::save_generation` with the scratch tree's HEAD recorded. See
    /// [`SCRATCH_HEAD`].
    fn save_scratch_generation(
        store: &Store,
        extractions: &[devmap_extract::Extraction],
        resolution: &devmap_resolve::ResolutionResult,
        analysis: &devmap_analyze::model::AnalysisSummary,
    ) {
        store
            .save_generation_with_metadata(
                extractions,
                resolution,
                analysis,
                GenerationWriteOpts::default(),
                SCRATCH_HEAD,
            )
            .unwrap();
    }

    /// The stat-read-stat loop refuses a file that changed under it, and the
    /// size limit is exclusive.
    ///
    /// All three comparisons here were mutable without a failure. The
    /// conjunction is what makes the read *stable*: relaxing it to `||` accepts
    /// a source whose length matches but whose mtime moved — a torn read, which
    /// then gets indexed and stored as if it were the file on disk.
    #[test]
    fn a_source_changing_under_the_reader_is_refused() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("devmap-stable-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("src.py");
        fs::write(&path, "def stable(): pass\n").unwrap();

        // A file that does not move is read successfully.
        assert!(
            read_stable_source(&path, "src.py").is_ok(),
            "an unchanging file must read cleanly"
        );

        // A reader that returns content of a different length than the file on
        // disk is a torn read and must be refused, not returned.
        let torn = read_stable_source_with(&path, "src.py", || {
            Ok("def stable(): pass\nextra\n".to_string())
        });
        assert!(
            torn.is_err(),
            "content disagreeing with the file's size must be refused as a torn read"
        );

        // Oversized sources are refused outright rather than read.
        let big = dir.join("big.py");
        fs::write(&big, "x".repeat((MAX_SOURCE_BYTES + 1) as usize)).unwrap();
        assert!(
            read_stable_source(&big, "big.py").is_err(),
            "a source past the size limit must be refused"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// Every path the daemon accepts must resolve inside its root.
    ///
    /// `collect_pending_path` carries three containment guards — a `..`
    /// traversal check and two `starts_with(root)` checks, one for directories
    /// and one for files. Mutation testing deleted the `!` on both
    /// `starts_with` guards and flipped the traversal check without a single
    /// failure. Inverted, they accept exactly the paths meant to be refused and
    /// refuse the legitimate ones: a watcher event naming `../../etc` would be
    /// indexed, and the daemon reads and stores whatever it is pointed at.
    #[test]
    fn pending_paths_outside_the_daemon_root_are_refused() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("devmap-escape-{}-{stamp}", std::process::id()));
        let outside =
            std::env::temp_dir().join(format!("devmap-outside-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        // macOS `temp_dir()` is a symlink (`/var` -> `/private/var`), and the
        // guards compare against a *canonicalized* path. Canonicalize the
        // fixture roots too, or the positive control fails for a reason that
        // has nothing to do with containment.
        let root = root.canonicalize().unwrap();
        let outside = outside.canonicalize().unwrap();
        fs::write(root.join("inside.py"), "def ok(): pass\n").unwrap();
        fs::write(outside.join("secret.py"), "def leaked(): pass\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        let daemon = Daemon::new(store, root.clone());
        let previous: Vec<devmap_extract::Extraction> = Vec::new();

        // A parent-traversal component is refused outright.
        assert!(
            daemon
                .collect_pending_path(&root, &previous, "../outside/secret.py")
                .is_err(),
            "a `..` component must be refused before any filesystem access"
        );

        // An absolute path outside the root is refused.
        assert!(
            daemon
                .collect_pending_path(
                    &root,
                    &previous,
                    &outside.join("secret.py").to_string_lossy()
                )
                .is_err(),
            "an absolute path outside the daemon root must be refused"
        );

        // A directory outside the root is refused.
        assert!(
            daemon
                .collect_pending_path(&root, &previous, &outside.to_string_lossy())
                .is_err(),
            "a directory outside the daemon root must be refused"
        );

        // Positive control: a legitimate path inside the root is accepted, so
        // the assertions above cannot pass by refusing everything.
        assert!(
            daemon
                .collect_pending_path(&root, &previous, "inside.py")
                .is_ok(),
            "a path inside the root must still be accepted"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    use devmap_extract::extract_tree;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn default_ipc_paths_are_repo_scoped_and_portably_short() {
        let first = default_ipc_path_for(std::path::Path::new("/tmp/repo-a"));
        let second = default_ipc_path_for(std::path::Path::new("/tmp/repo-b"));
        assert_ne!(first, second);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert!(first.as_os_str().as_bytes().len() <= 100);
            assert!(second.as_os_str().as_bytes().len() <= 100);
        }
    }

    #[test]
    fn drain_pending_batch_is_bounded() {
        let store = Store::open_in_memory().unwrap();
        let paths: Vec<String> = (0..10).map(|i| format!("f{i}.py")).collect();
        store.enqueue_pending_paths(&paths).unwrap();
        let daemon = Daemon::new(store, ".".into()).with_batch_limit(3);
        let n = daemon.drain_pending_batch().unwrap();
        assert_eq!(n, 3);
        assert_eq!(daemon.store.get_pending_paths().unwrap().len(), 7);
    }

    /// SC1/SC7 on the path that runs most often.
    ///
    /// The watcher commits a generation per edit batch, so it — not the CLI —
    /// is where unbounded retention actually hurts: an always-on daemon would
    /// add a full carry-forward copy of the repository, plus one permanent
    /// extraction-cache row per edit, every time a file was saved. The CLI
    /// prune was covered first and this path was left unproven, so this test
    /// exists to stop "wired in the CLI" from being mistaken for "bounded".
    ///
    /// Fails against a daemon without the prune calls: five drains leave five
    /// generations rather than `GENERATION_RETENTION`.
    #[test]
    fn daemon_resync_bounds_generations_and_cache() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("devmap-daemon-retention-{}", stamp));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let churn = dir.join("churn.py");
        let stable = dir.join("stable.py");
        fs::write(&stable, "def stable():\n    return 1\n").unwrap();

        let db_path = dir.join("index.sqlite");
        let store = Store::open(&db_path).unwrap();
        let daemon = Daemon::new(store, dir.clone());

        let mut sizes = Vec::new();
        for round in 0..5 {
            fs::write(&churn, format!("def churn():\n    return {round}\n")).unwrap();
            daemon
                .store
                .enqueue_pending_paths(&[
                    churn.to_string_lossy().into_owned(),
                    stable.to_string_lossy().into_owned(),
                ])
                .unwrap();
            daemon.drain_pending_batch().unwrap();
            sizes.push(fs::metadata(&db_path).unwrap().len());
        }

        // Asserted on file size rather than a row count: growth on disk is the
        // property that actually matters, and it needs no test-only accessor on
        // the production Store API. Once retention is bounded, later resyncs
        // reuse pages instead of adding them.
        let settled = sizes[2];
        let final_size = sizes[4];
        let limit = settled + settled / 20 + 65_536;
        assert!(
            final_size <= limit,
            "daemon store grew from {settled} to {final_size} bytes between resync 3 and 5 \
             (sizes: {sizes:?}); retention is unbounded on the watcher path"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// An upgraded kernel must not leave the watcher replaying a batch that
    /// can never commit.
    ///
    /// Everything the incremental resync builds on comes from the last
    /// generation: the extractions it resolves, and the rows the store carries
    /// forward. After an extractor or grammar bump those describe the same
    /// bytes in an older schema, so the store refuses to reuse them — and the
    /// daemon, whose only failure handling is to keep the pending paths for a
    /// later attempt, would retry the same impossible batch for as long as it
    /// ran. It re-extracts the tree instead, once.
    ///
    /// Fails against a daemon without that branch: `drain_pending_batch`
    /// returns the store's refusal rather than a committed generation.
    #[test]
    fn an_upgraded_kernel_makes_the_daemon_rebuild_rather_than_replay() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("devmap-daemon-upgrade-{stamp}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for index in 0..6 {
            fs::write(
                dir.join(format!("mod_{index}.py")),
                format!("def leaf_{index}():\n    return {index}\n\n\ndef caller_{index}():\n    return leaf_{index}()\n"),
            )
            .unwrap();
        }

        let db_path = dir.join("index.sqlite");
        let store = Store::open(&db_path).unwrap();
        let daemon = Daemon::new(store, dir.clone());

        // A first generation, written by "the previous kernel".
        daemon
            .store
            .enqueue_pending_paths(
                &(0..6)
                    .map(|index| {
                        dir.join(format!("mod_{index}.py"))
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        daemon.drain_pending_batch().unwrap();
        assert!(
            daemon.store.latest_generation_payload_is_current().unwrap(),
            "fixture precondition: the first generation is written by this kernel"
        );

        // The upgrade: stored rows now carry an identity this kernel does not
        // produce, exactly as two schema bumps left DevCouncil's own store.
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute(
                "UPDATE generation_files SET analyzer_version = '0.0.9:extract-v1'
                 WHERE generation_id = (SELECT max(id) FROM generations)",
                [],
            )
            .unwrap();
        }
        assert!(
            !daemon.store.latest_generation_payload_is_current().unwrap(),
            "fixture precondition: the store now looks like it predates this kernel"
        );

        fs::write(
            dir.join("mod_3.py"),
            "def leaf_3():\n    return 33\n\n\ndef caller_3():\n    return leaf_3() + 3\n",
        )
        .unwrap();
        daemon
            .store
            .enqueue_pending_paths(&[dir.join("mod_3.py").to_string_lossy().into_owned()])
            .unwrap();

        let drained = daemon
            .drain_pending_batch()
            .expect("an upgraded store must resync, not fail forever");
        assert_eq!(drained, 1);
        assert!(
            daemon.store.latest_generation_payload_is_current().unwrap(),
            "the resync must leave a generation this kernel could have written"
        );
        assert_eq!(
            daemon.store.get_pending_paths().unwrap().len(),
            0,
            "a committed resync acknowledges its batch instead of replaying it"
        );
        // The rebuild is whole-tree, not just the one pending file.
        assert_eq!(
            daemon.store.latest_extractions().unwrap().len(),
            6,
            "a full re-extraction must keep every file in the generation"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// A refused file inside a changed directory must neither block the
    /// directory's resync nor delete its own previously-stored extraction.
    ///
    /// The directory branch bailed the whole batch when discovery refused any
    /// file (oversized, unreadable): one poison file meant the directory could
    /// never be resynced, and the watcher retried it with backoff forever.
    /// Worse, had the bail been dropped naively, the refusal would have made
    /// the poison path look *deleted* — the file still exists on disk, so
    /// removing its stored row would be a falsehood about the tree.
    #[test]
    fn a_refused_file_in_a_changed_directory_neither_blocks_nor_deletes_siblings() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-dir-poison-{stamp}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("good.py"), "def good_v1():\n    return 1\n").unwrap();
        fs::write(root.join("poison.py"), "def poison_v1():\n    return 2\n").unwrap();

        let initial = extract_tree(&root).unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&initial);
        let resolution = resolver.resolve_all(&initial);
        let analysis = analyze(&initial, &resolution);
        let store = Store::open_in_memory().unwrap();
        save_scratch_generation(&store, &initial, &resolution, &analysis);

        // The edit: one legitimate change, and the sibling grows past the
        // source-size limit so discovery refuses it.
        fs::write(root.join("good.py"), "def good_v2():\n    return 11\n").unwrap();
        fs::write(
            root.join("poison.py"),
            vec![b'x'; (MAX_SOURCE_BYTES + 1) as usize],
        )
        .unwrap();

        store
            .enqueue_pending_paths(&[root.to_string_lossy().into_owned()])
            .unwrap();
        let daemon = Daemon::new(store, root.clone());

        daemon
            .drain_pending_batch()
            .expect("a refused sibling must not block resyncing the directory");

        let persisted = daemon.store.latest_extractions().unwrap();
        assert!(
            persisted.iter().any(|extraction| {
                extraction.file_path == "good.py"
                    && extraction
                        .symbols
                        .iter()
                        .any(|symbol| symbol.name == "good_v2")
            }),
            "the healthy sibling must be re-extracted"
        );
        assert!(
            persisted.iter().any(|extraction| {
                extraction.file_path == "poison.py"
                    && extraction
                        .symbols
                        .iter()
                        .any(|symbol| symbol.name == "poison_v1")
            }),
            "a refused-but-existing file must keep its stored row instead of \
             being recorded as deleted"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// One unrepresentable filename must not stop the daemon from starting at
    /// all. The CLI build reports the refusal and continues; connect-time
    /// reconciliation used to bail instead, so a single non-UTF-8 source name
    /// killed IPC availability for the whole repository.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_source_path_is_skipped_not_fatal_at_reconcile() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-nonutf8-{stamp}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ok.py"), "def ok(): pass\n").unwrap();

        let initial = extract_tree(&root).unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&initial);
        let resolution = resolver.resolve_all(&initial);
        let analysis = analyze(&initial, &resolution);
        let store = Store::open_in_memory().unwrap();
        save_scratch_generation(&store, &initial, &resolution, &analysis);

        // A source whose name is not valid UTF-8 appears after the last build.
        // Some volume configurations (APFS with name normalization among them)
        // refuse such names outright rather than storing them; there the
        // fixture cannot exist and the daemon-tolerance property has no local
        // subject, so say so instead of panicking on fixture setup.
        if let Err(error) = fs::write(
            root.join(OsStr::from_bytes(b"bad\xff.py")),
            "def odd(): pass\n",
        ) {
            if error.raw_os_error().is_some() {
                eprintln!(
                    "skipping non-UTF-8 reconcile test: this platform/volume refuses \
                     non-UTF-8 filenames ({error})"
                );
                let _ = fs::remove_dir_all(&root);
                return;
            }
            panic!("unexpected fixture failure: {error}");
        }

        let daemon = Daemon::new(store, root.clone());
        let reconciled = daemon
            .reconcile_connect_time()
            .expect("an unrepresentable filename must degrade to a skip, not kill the sweep");
        assert_eq!(
            reconciled, 0,
            "only representable paths may enter the pending queue"
        );
        assert!(daemon.store.get_pending_paths().unwrap().is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn watcher_enqueue_survives_via_store() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("devmap-daemon-{}", stamp));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("index.sqlite");
        let store = Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&[format!("{}/a.py", dir.display())])
            .unwrap();
        drop(store);
        let reopened = Store::open(&db).unwrap();
        assert_eq!(reopened.get_pending_paths().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_rebuild_keeps_pending_work_for_replay() {
        // closes B1 — kill-9 / crash mid-build leaves pending work for replay
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-daemon-missing-{}", stamp));
        let _ = fs::remove_dir_all(&root);
        let store = Store::open_in_memory().unwrap();
        store
            .enqueue_pending_paths(&["missing.py".to_string()])
            .unwrap();
        let daemon = Daemon::new(store, root);

        assert!(daemon.drain_pending_batch().is_err());
        assert_eq!(daemon.store.get_pending_paths().unwrap(), ["missing.py"]);
    }

    #[test]
    fn poison_path_does_not_block_a_valid_sibling_in_the_same_batch() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("devmap-batch-isolation-{stamp}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("a_poison.py"),
            vec![b'x'; (MAX_SOURCE_BYTES + 1) as usize],
        )
        .unwrap();
        fs::write(dir.join("b_good.py"), "def healthy():\n    return 1\n").unwrap();

        let store = Store::open(dir.join("index.sqlite")).unwrap();
        store
            .enqueue_pending_paths(&["a_poison.py".into(), "b_good.py".into()])
            .unwrap();
        let daemon = Daemon::new(store, dir.clone()).with_batch_limit(2);
        assert_eq!(daemon.drain_pending_batch().unwrap(), 1);
        assert_eq!(
            daemon.store.get_pending_paths().unwrap(),
            ["a_poison.py".to_string()]
        );
        assert!(daemon
            .store
            .list_generation_paths(1)
            .unwrap()
            .contains(&"b_good.py".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_b5_s17_connect_time_sweep_enqueues_changed_new_and_deleted_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-daemon-catchup-{stamp}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("changed.py"), "def changed():\n    return 1\n").unwrap();
        fs::write(root.join("deleted.py"), "def deleted():\n    return 1\n").unwrap();

        let initial = extract_tree(&root).unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&initial);
        let resolution = resolver.resolve_all(&initial);
        let analysis = analyze(&initial, &resolution);
        let store = Store::open_in_memory().unwrap();
        save_scratch_generation(&store, &initial, &resolution, &analysis);

        fs::write(root.join("changed.py"), "def changed():\n    return 2\n").unwrap();
        fs::remove_file(root.join("deleted.py")).unwrap();
        fs::write(root.join("new.py"), "def new():\n    return 1\n").unwrap();

        let daemon = Daemon::new(store, root.clone());
        assert_eq!(daemon.reconcile_connect_time().unwrap(), 3);
        assert_eq!(
            daemon.store.get_pending_paths().unwrap(),
            ["changed.py", "deleted.py", "new.py"]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_rebuild_acknowledges_after_persisting_generation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-daemon-success-{}", stamp));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("main.py"), "def main(): pass\n").unwrap();

        let store = Store::open_in_memory().unwrap();
        store
            .enqueue_pending_paths(&[root.join("main.py").display().to_string()])
            .unwrap();
        let daemon = Daemon::new(store, root.clone());

        assert_eq!(daemon.drain_pending_batch().unwrap(), 1);
        assert!(daemon.store.get_pending_paths().unwrap().is_empty());
        let status = daemon.store.status(":memory:").unwrap();
        assert_eq!(status.latest_generation, Some(1));
        assert!(status.node_count >= 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn differential_batch_does_not_read_unclaimed_files() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-daemon-delta-{stamp}"));
        fs::create_dir_all(&root).unwrap();
        let a_path = root.join("a.py");
        let b_path = root.join("b.py");
        fs::write(&a_path, "def old_a():\n    return 1\n").unwrap();
        fs::write(&b_path, "def stable_b():\n    return 2\n").unwrap();

        let initial = extract_tree(&root).unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&initial);
        let resolution = resolver.resolve_all(&initial);
        let analysis = analyze(&initial, &resolution);
        let store = Store::open_in_memory().unwrap();
        save_scratch_generation(&store, &initial, &resolution, &analysis);

        fs::write(&a_path, "def new_a():\n    return 3\n").unwrap();
        fs::remove_file(&b_path).unwrap();
        store
            .enqueue_pending_paths(&[a_path.display().to_string()])
            .unwrap();
        let daemon = Daemon::new(store, root.clone());
        assert_eq!(daemon.drain_pending_batch().unwrap(), 1);

        let persisted = daemon.store.latest_extractions().unwrap();
        assert!(persisted.iter().any(|ext| {
            ext.file_path == "a.py" && ext.symbols.iter().any(|symbol| symbol.name == "new_a")
        }));
        assert!(persisted.iter().any(|ext| {
            ext.file_path == "b.py" && ext.symbols.iter().any(|symbol| symbol.name == "stable_b")
        }));
        let _ = fs::remove_dir_all(&root);
    }

    /// A checkout that lands *during* a drain must not be able to stamp itself
    /// onto a generation built from the previous one.
    ///
    /// The drain used to read git HEAD twice: once to decide whether the stored
    /// rows may be carried forward, and again — after extraction, resolution
    /// and analysis, seconds later on any real repository — to stamp the
    /// generation. A `git checkout` in between made it decide "unmoved, carry
    /// forward" and then record the HEAD it had moved to. The next drain then
    /// compared the current HEAD against that stamp, found them equal, and
    /// carried forward again: the rebuild the checkout called for is never run,
    /// because the check reads its own mistake back as proof nothing happened.
    ///
    /// The reading is injected because a test cannot schedule a checkout into
    /// the middle of a resolve. A reader that answers differently the second
    /// time makes the two-read shape visible directly: there must be no second
    /// call, and the stamp must be the answer the decision was made on.
    #[test]
    fn one_drain_reads_git_head_exactly_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = std::env::temp_dir().join(format!(
            "devmap-head-once-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();

        let decided_at = "a".repeat(40);
        let moved_to = "b".repeat(40);

        let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone());
        daemon
            .store
            .enqueue_pending_paths_under_root(&root, &["src/a.py".to_string()])
            .unwrap();
        let first = decided_at.clone();
        assert_eq!(
            daemon
                .drain_pending_batch_with_head(&move |_: &std::path::Path| Ok(first.clone()))
                .unwrap(),
            1
        );

        // A file the drain is never told about, so a carry-forward and a full
        // re-extraction are distinguishable from the stored generation alone.
        fs::write(root.join("src/b.py"), "def b():\n    return 2\n").unwrap();
        fs::write(root.join("src/a.py"), "def a():\n    return 99\n").unwrap();
        daemon
            .store
            .enqueue_pending_paths_under_root(&root, &["src/a.py".to_string()])
            .unwrap();

        let reads = AtomicUsize::new(0);
        let decided = decided_at.clone();
        let moved = moved_to.clone();
        let drained = daemon
            .drain_pending_batch_with_head(&move |_: &std::path::Path| {
                // First answer matches the stored generation, so the drain
                // decides "carry forward". Every later answer is the checkout
                // that landed while it was working.
                if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(decided.clone())
                } else {
                    Ok(moved.clone())
                }
            })
            .unwrap();
        assert_eq!(drained, 1);

        assert_eq!(
            daemon
                .store
                .latest_generation_head_sha()
                .unwrap()
                .as_deref(),
            Some(decided_at.as_str()),
            "the generation must be stamped at the HEAD its carry-forward decision \
             was made against; stamping a later reading is what makes the next \
             drain believe the checkout never happened"
        );
        let mut persisted: Vec<String> = daemon
            .store
            .latest_extractions()
            .unwrap()
            .into_iter()
            .map(|extraction| extraction.file_path)
            .collect();
        persisted.sort();
        assert_eq!(
            persisted,
            vec!["src/a.py".to_string()],
            "and it must be the generation that decision produced"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A stored key that cannot be spelled is a refusal, not a renamed file.
    ///
    /// `to_string_lossy` on a name whose bytes are not UTF-8 does not produce
    /// the path; it produces a different one, with `U+FFFD` where the bytes
    /// were. Stored as an extraction key that is a row nothing can ever match;
    /// used as a queue entry it names a file that does not exist, which the
    /// drain reconciles as a *deletion*. Either way a conversion that could not
    /// be performed answered exactly like one that was.
    #[cfg(unix)]
    #[test]
    fn a_stored_path_that_is_not_utf8_is_refused_not_lossily_renamed() {
        use std::os::unix::ffi::OsStrExt;

        let root = std::path::Path::new("/repo");
        let unrepresentable = root.join(std::ffi::OsStr::from_bytes(b"src/caf\xe9.py"));
        assert!(
            unrepresentable.to_str().is_none(),
            "the fixture must actually be unrepresentable"
        );
        let error = stored_path(root, &unrepresentable)
            .expect_err("an unspellable key must be refused, not invented");
        assert!(
            error.to_string().contains("not UTF-8"),
            "and the refusal must say why: {error}"
        );

        // Positive control: ordinary names still resolve, and still resolve to
        // forward slashes.
        assert_eq!(
            stored_path(root, &root.join("src/cafe.py")).unwrap(),
            "src/cafe.py"
        );
    }

    /// A wholly-failed batch reports a bounded sample and the honest total.
    ///
    /// The message itemised every failure. `batch_limit` is 8192 and each entry
    /// is a path plus a rendered error, so a batch that went entirely wrong —
    /// a vendored tree that lost its read bit, a mount that went away — built a
    /// megabyte-scale string, handed it to `run_loop`, and had it logged whole
    /// on every backoff retry for as long as the condition lasted. Bounding it
    /// without carrying the total would be the other failure: five named paths
    /// and nothing saying there were more reads exactly like a batch in which
    /// only five things went wrong.
    #[cfg(unix)]
    #[test]
    fn a_wholly_failed_batch_reports_a_bounded_sample_and_the_real_total() {
        use std::os::unix::fs::PermissionsExt;

        let (root, daemon) = daemon_with_one_indexed_source("failure-sample");
        let daemon = daemon.with_batch_limit(64);
        let poisoned = DRAIN_FAILURE_SAMPLE * 3;
        let mut paths = Vec::new();
        for index in 0..poisoned {
            let relative = format!("src/poison_{index}.py");
            let path = root.join(&relative);
            fs::write(&path, "def poisoned():\n    return 1\n").unwrap();
            // Readable-by-nobody, so the read inside the drain fails while the
            // file plainly still exists — a failure, not a deletion.
            fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
            paths.push(relative);
        }
        // The one indexed source is not enqueued, so every claimed path fails.
        daemon
            .store
            .enqueue_pending_paths_under_root(&root, &paths)
            .unwrap();

        let error = daemon
            .drain_pending_batch()
            .expect_err("every claimed path failed, so the batch must fail");
        let rendered = error.to_string();

        assert!(
            rendered.contains(&format!("all {poisoned} claimed path(s) failed")),
            "the honest total must be reported, got: {rendered}"
        );
        let named = paths
            .iter()
            .filter(|relative| rendered.contains(relative.as_str()))
            .count();
        assert_eq!(
            named, DRAIN_FAILURE_SAMPLE,
            "exactly the sample may be itemised; {named} of {poisoned} were named in: \
             {rendered}"
        );
        assert!(
            rendered.contains(&format!(
                "{} more not shown",
                poisoned - DRAIN_FAILURE_SAMPLE
            )),
            "and the elided count must be stated, or the sample reads as the whole \
             set: {rendered}"
        );

        for relative in &paths {
            let _ = fs::set_permissions(root.join(relative), fs::Permissions::from_mode(0o600));
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// A scratch tree holding one previously-indexed source, and a daemon
    /// rooted at it. Returns the canonical root and the daemon.
    fn daemon_with_one_indexed_source(tag: &str) -> (std::path::PathBuf, Daemon) {
        let root = std::env::temp_dir().join(format!(
            "devmap-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
        let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone());
        (root, daemon)
    }

    /// A file removed between the existence check and the read is a deletion,
    /// not a failed attempt.
    ///
    /// `collect_pending_path` stats the path, canonicalizes it, stats it again
    /// inside the stable read, and only then reads it. A create-then-delete —
    /// an editor's scratch file, a build's intermediate, `git checkout` churn —
    /// routinely lands in one of those windows. Every one of them used to
    /// propagate as an error, and an error charges the path a retry attempt:
    /// five of those and the store quarantines it *permanently*, so a file that
    /// was only ever deleted stops being indexed for the life of the store and
    /// shows up in `degraded_reason` as though something were wrong with it.
    ///
    /// The `!exists()` branch already answers "deleted" for exactly this file.
    /// Losing a race to reach that branch must not change the answer.
    #[test]
    fn a_source_removed_while_it_is_read_is_reconciled_as_a_deletion() {
        let (root, daemon) = daemon_with_one_indexed_source("vanish-read");
        let previous = vec![extract_file("src/a.py", "def a():\n    return 1\n")];

        let delta = daemon
            .collect_pending_path_with(&root, &previous, "src/a.py", &|_, _| {
                Err(anyhow::Error::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "No such file or directory (os error 2)",
                ))
                .context("cannot read changed source \"src/a.py\""))
            })
            .expect(
                "a file that stopped existing under the reader is a deletion the queue \
                 already knows how to express, not an attempt to charge toward quarantine",
            );

        assert!(
            delta.deleted.contains("src/a.py"),
            "the vanished path must be reconciled as deleted, got {:?}",
            delta.deleted
        );
        assert!(
            delta.fresh.is_empty(),
            "and nothing may be extracted from a file that is not there"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// A path whose existence cannot be determined must not be reported gone.
    ///
    /// `Path::exists()` and `Path::is_dir()` answer `false` for *every* stat
    /// failure, not only "no such file": a symlink loop (`ELOOP`), a parent
    /// that lost `+x` (`EACCES`), a stale handle on a network mount. Each is a
    /// question that could not be answered, and the drain answered all of them
    /// with a **deletion** — which removes the file's rows from the graph. That
    /// is the one direction that destroys information: a wrongly-kept row is
    /// stale, a wrongly-dropped one is a symbol the dead-code pass is now free
    /// to call unreferenced.
    ///
    /// The undecidable answer is a failure, which the queue already knows what
    /// to do with: retry, then quarantine and name the path in
    /// `degraded_reason`. `admitted_watch_path` states the same rule for the
    /// same cases on the way in.
    #[cfg(unix)]
    #[test]
    fn a_path_whose_existence_is_undecidable_is_not_reported_deleted() {
        let (root, daemon) = daemon_with_one_indexed_source("eloop");
        // Self-referential: every stat of it returns ELOOP.
        std::os::unix::fs::symlink("looped.py", root.join("src/looped.py")).unwrap();
        assert!(
            fs::metadata(root.join("src/looped.py")).is_err(),
            "the fixture must actually be unstattable"
        );
        let previous = vec![extract_file(
            "src/looped.py",
            "def looped():\n    return 1\n",
        )];

        let error = daemon
            .collect_pending_path(&root, &previous, "src/looped.py")
            .expect_err(
                "a stat that could not run must not answer the same as one that ran and \
                 found the file gone — that answer deletes the file's rows",
            );
        assert!(
            error.to_string().contains("still exists"),
            "and the refusal must say the question is the one that failed: {error}"
        );

        // Positive control: a path that is genuinely absent is still a deletion.
        let previous = vec![extract_file("src/gone.py", "def gone():\n    return 1\n")];
        let delta = daemon
            .collect_pending_path(&root, &previous, "src/gone.py")
            .expect("an absent path is an answerable question");
        assert!(
            delta.deleted.contains("src/gone.py"),
            "a file that is not there is deleted, got {:?}",
            delta.deleted
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The OFF direction, and the reason this is a `NotFound` check and not a
    /// blanket `Ok`: a file that exists but cannot be read is a *failure*.
    /// Retrying it can succeed once the permission is fixed, and answering
    /// "deleted" would drop its rows out of the graph on the strength of a
    /// check that never ran.
    #[test]
    fn an_unreadable_source_is_still_a_failure_not_a_deletion() {
        let (root, daemon) = daemon_with_one_indexed_source("vanish-denied");
        let previous = vec![extract_file("src/a.py", "def a():\n    return 1\n")];

        let error = daemon
            .collect_pending_path_with(&root, &previous, "src/a.py", &|_, _| {
                Err(anyhow::Error::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Permission denied (os error 13)",
                ))
                .context("cannot read changed source \"src/a.py\""))
            })
            .expect_err("an unreadable file that still exists must not read as deleted");
        assert!(
            error.to_string().contains("cannot read changed source"),
            "and the refusal must still name the read, got: {error}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    /// The wrapper must keep the `io::ErrorKind` reachable.
    ///
    /// `read_stable_source_with` formatted its cause into a message, which left
    /// "the file was removed" indistinguishable from every other read failure
    /// once it reached the caller — a fact that only survives as text is not
    /// one anything can branch on, and the branch above is what keeps an
    /// ordinary deletion out of quarantine.
    #[test]
    fn a_vanished_read_stays_recognisable_as_vanished() {
        let (root, daemon) = daemon_with_one_indexed_source("vanish-chain");
        drop(daemon);
        let path = root.join("src/a.py");

        let error = read_stable_source_with(&path, "src/a.py", || {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "No such file or directory (os error 2)",
            ))
        })
        .expect_err("the injected read fails");
        assert!(
            error.to_string().contains("cannot read changed source"),
            "the message must not change: {error}"
        );
        assert!(
            vanished_under_the_reader(&error),
            "and the kind must survive the wrapping, or the caller cannot tell a \
             deletion from a device error: {error:?}"
        );

        let denied = read_stable_source_with(&path, "src/a.py", || {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Permission denied (os error 13)",
            ))
        })
        .expect_err("the injected read fails");
        assert!(
            !vanished_under_the_reader(&denied),
            "and a predicate that answered yes to everything would be worth nothing"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stat_read_stat_never_accepts_a_concurrently_changing_file() {
        let root = std::env::temp_dir().join(format!(
            "devmap-stable-read-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("changing.py");
        fs::write(&path, "x = 1\n").unwrap();
        let mut iteration = 0;
        let error = read_stable_source_with(&path, "changing.py", || {
            let observed = fs::read_to_string(&path)?;
            iteration += 1;
            fs::write(&path, "#".repeat(10 + iteration))?;
            Ok(observed)
        })
        .unwrap_err();
        assert!(error.to_string().contains("did not stabilize"));
        fs::remove_dir_all(root).unwrap();
    }

    /// Short unix socket base for fixtures: macOS `temp_dir()` alone exceeds
    /// the 100-byte portable sockaddr_un limit once a file name is appended,
    /// which would make every assertion here trip the path-length guard
    /// instead of the behavior under test.
    #[cfg(unix)]
    fn short_unix_fixture_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from("/tmp")
            .join(format!("devmap-fx-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A rebuilt binary retires the daemon; an unchanged one does not.
    ///
    /// The hazard is specific: `PROTOCOL_VERSION` does not move when the kernel
    /// is rebuilt, so without this check a daemon started before a `cargo
    /// build` keeps serving the old code for its whole idle bound — 30 minutes
    /// by default — and every client reads pre-fix answers from a fixed tree.
    #[test]
    fn a_changed_executable_identity_retires_the_daemon() {
        let epoch = std::time::UNIX_EPOCH;
        let started = Some((1_000u64, epoch));

        assert!(
            !should_retire_for_new_binary(started, started),
            "an unchanged binary must not restart the daemon on every tick"
        );
        assert!(
            should_retire_for_new_binary(started, Some((2_000, epoch))),
            "a binary whose size changed is a rebuild"
        );
        assert!(
            should_retire_for_new_binary(
                started,
                Some((1_000, epoch + std::time::Duration::from_secs(1))),
            ),
            "a rebuild that happens to produce the same size still moves mtime"
        );
    }

    /// An undeterminable identity must not cause a retirement.
    ///
    /// `None` is "could not tell", not "changed". Treating it as a change would
    /// make the daemon exit on every tick on any platform or sandbox where
    /// `current_exe` fails — turning a diagnostic gap into a crash loop that
    /// respawns a process per client call.
    #[test]
    fn an_undeterminable_executable_identity_never_retires() {
        assert!(!should_retire_for_new_binary(None, None));
    }

    /// The real probe answers for this test binary, and answers consistently.
    ///
    /// Without this the two tests above would pass over a function that always
    /// returned `None` in practice, and the check would be dead.
    #[test]
    fn executable_identity_resolves_and_is_stable() {
        let first = executable_identity();
        assert!(
            first.is_some(),
            "current_exe/metadata should resolve for the test binary"
        );
        assert_eq!(
            first,
            executable_identity(),
            "identity must be stable for an unchanged binary"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn status_answers_while_the_connect_time_sweep_is_still_running() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // Regression for the bind-after-reconcile ordering. The sweep hashed
        // the whole tree before the IPC listener bound, so a client's three-
        // second readiness deadline expired on any real repository: the
        // daemon was killed mid-startup, then every call repeated the spawn,
        // wait and kill before falling back to the CLI. Bound first, so
        // `status` answers from the last committed generation while the
        // sweep is still grinding.
        let root = short_unix_fixture_dir("bindfirst");
        // Enough *work* that hashing the tree takes comfortably longer than
        // the probe budget below; the precondition asserts this so the test
        // cannot pass vacuously on a fast machine.
        //
        // The margin is in bytes per file rather than in file count. At two
        // lines per file this fixture drifted down to 115 ms against its own
        // 120 ms floor and failed on the precondition — correctly, since the
        // ordering assertion would have been meaningless — as the kernel got
        // faster. Reaching the same margin by multiplying the file count would
        // mean tens of thousands of inodes and a setup slower than the test;
        // hashing cost scales with content, so bigger files buy the same
        // headroom for 6,000 `fs::write` calls instead of 25,000.
        const FILE_COUNT: usize = 6_000;
        const BODIES_PER_FILE: usize = 24;
        for index in 0..FILE_COUNT {
            let mut body = String::with_capacity(BODIES_PER_FILE * 48);
            for leaf in 0..BODIES_PER_FILE {
                body.push_str(&format!(
                    "def leaf_{index}_{leaf}():\n    return {index} + {leaf}\n"
                ));
            }
            fs::write(root.join(format!("mod_{index}.py")), body).unwrap();
        }

        let sweep_started = std::time::Instant::now();
        let (sources, _report) = devmap_extract::collect_sources_with_report(&root).unwrap();
        let _hashes: Vec<u64> = sources
            .iter()
            .map(|(_, source)| devmap_extract::content_hash(source))
            .collect();
        let sweep_elapsed = sweep_started.elapsed();
        assert!(
            sweep_elapsed >= Duration::from_millis(120),
            "fixture precondition failed: a {FILE_COUNT}-file sweep took only \
             {sweep_elapsed:?}; the ordering assertion below would be vacuous"
        );

        let store = Store::open_in_memory().unwrap();
        let socket = root.join("b.sock");
        let probe_budget = sweep_elapsed / 4;
        let daemon = Daemon::new(store, root.clone())
            .with_ipc_path(socket.clone())
            .with_idle_poll(Duration::from_millis(10))
            .with_max_idle(Some(Duration::from_secs(30)));
        let task = tokio::spawn(async move { daemon.run_loop().await });

        let bind_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !socket.exists() && tokio::time::Instant::now() < bind_deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(socket.exists(), "daemon IPC socket did not start");

        // The whole round trip must land inside a fraction of one sweep —
        // under the old ordering it could not start until a full sweep
        // finished.
        let answer = tokio::time::timeout(probe_budget, async {
            loop {
                match tokio::net::UnixStream::connect(&socket).await {
                    Err(_) => tokio::time::sleep(Duration::from_millis(2)).await,
                    Ok(mut stream) => {
                        stream
                            .write_all(b"{\"version\":1,\"cmd\":\"status\"}\n")
                            .await
                            .unwrap();
                        let mut response = String::new();
                        stream.read_to_string(&mut response).await.unwrap();
                        break serde_json::from_str::<serde_json::Value>(response.trim()).unwrap();
                    }
                }
            }
        })
        .await;
        let payload = answer.expect("status must answer within a fraction of one sweep");
        assert_eq!(payload["ok"], true);
        // The store here has never committed a generation, and the wire form
        // for that is JSON null — the same shape the Python client maps to 0
        // (`try_connect` treats it as unusable, which is correct: nothing has
        // been indexed). Pinning null keeps the envelope contract honest; a
        // fabricated 0 would claim a generation exists.
        assert!(payload["result"]["generation_id"].is_null());

        task.abort();
        let _ = task.await;
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_idle_daemon_with_no_pending_work_retires_itself() {
        // Orphaned daemons used to live forever: spawned detached, they kept
        // their store open and their watcher running with no consumer at all.
        // A bounded idle lifetime retires them; pending work keeps them alive.
        let root = short_unix_fixture_dir("idle");
        fs::write(root.join("main.py"), "def main(): pass\n").unwrap();

        let socket = root.join("idle.sock");
        let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone())
            .with_ipc_path(socket.clone())
            .with_idle_poll(Duration::from_millis(20))
            .with_max_idle(Some(Duration::from_millis(100)));
        let task = tokio::spawn(async move { daemon.run_loop().await });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !socket.exists() && tokio::time::Instant::now() < deadline {
            if task.is_finished() {
                let joined = task.await;
                let inner = joined.expect("join");
                panic!(
                    "run_loop exited before binding IPC: {inner:?}",
                    inner = inner.map_err(|error| error.to_string())
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket.exists(), "daemon IPC socket did not start");

        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("daemon did not retire within its idle bound")
            .expect("retirement task panicked");
        result.expect("idle retirement must be a clean exit");
        for _ in 0..100 {
            if !socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!socket.exists(), "retired daemon leaked its IPC endpoint");
        fs::remove_dir_all(root).unwrap();
    }

    /// K-A3, closing the loop: what the watcher emits when the OS says it
    /// dropped events must actually re-index the tree, and must make the
    /// freshness surfaces say so while it is outstanding.
    ///
    /// The unit test beside `watch_event_paths` proves the notice produces this
    /// queue entry. This proves the entry is worth producing: it goes through
    /// the same `enqueue_pending_paths_under_root` the daemon's watcher
    /// callback calls, the same `Store::status` the IPC `status` answer derives
    /// `is_fresh` from, and the same `drain_pending_batch` the tick runs.
    ///
    /// The file added here is one no incremental event ever named — exactly
    /// what a dropped-event window leaves behind.
    #[test]
    fn a_rescan_request_re_indexes_the_tree_and_is_not_reported_fresh() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-daemon-rescan-{stamp}"));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("main.py"), "def main():\n    return 1\n").unwrap();

        let initial = extract_tree(&root).unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&initial);
        let resolution = resolver.resolve_all(&initial);
        let analysis = analyze(&initial, &resolution);
        let store = Store::open_in_memory().unwrap();
        save_scratch_generation(&store, &initial, &resolution, &analysis);

        // The window in which the OS dropped events: this file appears, and no
        // per-path event for it is ever delivered.
        fs::write(
            root.join("missed.py"),
            "def missed_in_the_gap():\n    return 1\n",
        )
        .unwrap();

        let daemon = Daemon::new(store, root.clone());
        assert!(
            !daemon
                .store
                .latest_extractions()
                .unwrap()
                .iter()
                .any(|extraction| extraction.file_path == "missed.py"),
            "precondition: the missed file must not already be indexed"
        );
        assert_eq!(
            daemon.store.status("<memory>").unwrap().pending_count,
            0,
            "precondition: nothing queued, so `is_fresh` currently answers true"
        );

        let canonical_root = root.canonicalize().unwrap();
        let report = daemon
            .store
            .enqueue_pending_paths_under_root(
                &canonical_root,
                &crate::watcher::whole_tree_rescan(&canonical_root),
            )
            .unwrap();
        assert!(
            report.refused.is_empty(),
            "the rescan request must be an acceptable queue entry: {:?}",
            report.refused
        );
        assert!(
            daemon.store.status("<memory>").unwrap().pending_count > 0,
            "a repository whose watcher lost coverage must not read as fresh \
             until the rescan has run"
        );

        assert_eq!(
            daemon.drain_pending_batch().unwrap(),
            1,
            "the rescan entry must be claimed and acknowledged"
        );
        assert!(
            daemon
                .store
                .latest_extractions()
                .unwrap()
                .iter()
                .any(|extraction| extraction.file_path == "missed.py"),
            "the rescan must pick up the file no event ever named"
        );
        assert_eq!(
            daemon.store.status("<memory>").unwrap().pending_count,
            0,
            "and the tree is genuinely fresh again once it has run"
        );

        fs::remove_dir_all(root).unwrap();
    }

    /// A directory and a file inside it, claimed in the same batch, describe the
    /// same file twice — and the store refuses a generation whose input names a
    /// path twice ("duplicate extraction path in generation input").
    ///
    /// Found end to end while proving K-A3: a rescan queues the repository root
    /// beside the per-path events that did survive the drop, every drain then
    /// failed on the collision, and the retries backed off to 64 s while
    /// `pending_count` stayed non-zero — a queue that could never drain. The
    /// same collision was already reachable without a rescan (a new
    /// subdirectory and a file inside it arrive in one watcher batch); the
    /// rescan just makes it the normal case.
    ///
    /// The extractions are deduplicated where they are collected, so no caller
    /// has to know that two queue entries can cover one file.
    #[test]
    fn a_directory_and_a_file_inside_it_in_one_batch_do_not_collide() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("devmap-daemon-overlap-{stamp}"));
        fs::create_dir_all(root.join("pkg")).unwrap();
        fs::write(root.join("main.py"), "def main():\n    return 1\n").unwrap();
        fs::write(root.join("pkg/mod.py"), "def helper():\n    return 1\n").unwrap();

        let initial = extract_tree(&root).unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&initial);
        let resolution = resolver.resolve_all(&initial);
        let analysis = analyze(&initial, &resolution);
        let store = Store::open_in_memory().unwrap();
        save_scratch_generation(&store, &initial, &resolution, &analysis);

        fs::write(root.join("pkg/mod.py"), "def helper():\n    return 2\n").unwrap();

        let canonical_root = root.canonicalize().unwrap();
        let daemon = Daemon::new(store, root.clone());
        // The whole tree, the subdirectory, and one file inside it: every
        // combination a rescan-plus-events batch can hold.
        daemon
            .store
            .enqueue_pending_paths_under_root(
                &canonical_root,
                &[
                    canonical_root.to_string_lossy().into_owned(),
                    canonical_root.join("pkg").to_string_lossy().into_owned(),
                    canonical_root
                        .join("pkg/mod.py")
                        .to_string_lossy()
                        .into_owned(),
                ],
            )
            .unwrap();

        let drained = daemon
            .drain_pending_batch()
            .expect("overlapping queue entries must not make the batch unpersistable");
        assert_eq!(drained, 3, "every claimed entry must be acknowledged");
        assert!(
            daemon.store.get_pending_paths().unwrap().is_empty(),
            "a batch that cannot drain retries forever and quarantines"
        );

        // The generation is correct, not merely writable: the re-read file wins
        // and nothing was lost to the deduplication.
        let stored = daemon.store.latest_extractions().unwrap();
        let mut paths: Vec<&str> = stored
            .iter()
            .map(|extraction| extraction.file_path.as_str())
            .collect();
        paths.sort_unstable();
        assert_eq!(paths, ["main.py", "pkg/mod.py"]);

        fs::remove_dir_all(root).unwrap();
    }

    /// K-A5: idle retirement is the *routine* exit, and it skipped the release
    /// the code documents as mandatory.
    ///
    /// The distinction the existing retirement test cannot see is timing.
    /// `AbortTaskOnDrop::drop` only *schedules* the IPC future to be dropped;
    /// the socket file is removed by that drop. A test that waits a second for
    /// the file to disappear passes either way — the runtime gets around to it.
    /// A real daemon does not wait: it returns from `run_loop` and the process
    /// exits, leaving the endpoint on disk in exactly the state a `kill -9`
    /// leaves. `probe_endpoint_liveness` then answers `Some(true)` for the
    /// corpse and the next client's daemon refuses to start.
    ///
    /// So this awaits `run_loop` in the test's own task and asserts with no
    /// intervening `.await`: on a current-thread runtime nothing else can run
    /// in that gap, which is precisely the window the process exit falls into.
    #[cfg(unix)]
    #[tokio::test]
    async fn idle_retirement_releases_the_endpoint_before_run_loop_returns() {
        let root = short_unix_fixture_dir("idle-release");
        fs::write(root.join("main.py"), "def main(): pass\n").unwrap();

        let socket = root.join("idle-release.sock");
        let lock = crate::protocol::ipc_lock_path(&socket);
        let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone())
            .with_ipc_path(socket.clone())
            .with_idle_poll(Duration::from_millis(20))
            .with_max_idle(Some(Duration::from_millis(100)));

        let outcome = tokio::time::timeout(Duration::from_secs(20), daemon.run_loop())
            .await
            .expect("the daemon must retire within its idle bound");
        // NO `.await` between the line above and the assertions below.
        assert!(outcome.is_ok(), "idle retirement reported: {outcome:?}");
        assert!(
            !socket.exists(),
            "idle retirement returned with its socket still at {} — \
             the endpoint must be released before `run_loop` resolves",
            socket.display()
        );
        assert!(
            !lock.exists(),
            "idle retirement returned with its lock still at {}",
            lock.display()
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pending_work_defers_idle_retirement() {
        let root = short_unix_fixture_dir("busyhold");
        fs::write(root.join("main.py"), "def main(): pass\n").unwrap();

        let socket = root.join("busy.sock");
        // max_idle (50ms) is far below the drain cost of the batch below; the
        // daemon must keep working until the queue empties rather than retire
        // mid-resync.
        let store = Store::open_in_memory().unwrap();
        let paths: Vec<String> = (0..3)
            .map(|i| format!("{}/m{i}.py", root.display()))
            .collect();
        for path in &paths {
            fs::write(path, format!("def f{}(): pass\n", path.len())).unwrap();
        }
        store.enqueue_pending_paths(&paths).unwrap();
        let daemon = Daemon::new(store, root.clone())
            .with_ipc_path(socket.clone())
            .with_idle_poll(Duration::from_millis(20))
            .with_max_idle(Some(Duration::from_millis(50)))
            .with_batch_limit(1);
        let handle = tokio::spawn(async move { daemon.run_loop().await });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        // Retiring cleanly after the queue drains is exactly what must happen:
        // max_idle is far below one drain, so only deferred-by-work retirement
        // lets this succeed.
        let retired = matches!(
            tokio::time::timeout_at(deadline, handle).await,
            Ok(Ok(Ok(())))
        );
        assert!(
            retired,
            "daemon neither drained its queue nor retired cleanly in time"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A `kill` must not leave the endpoint behind.
    ///
    /// The socket file was removed only by `UnixIpcServer`'s `Drop`, which a
    /// signal never reaches: the process dies where it stands, the socket file
    /// survives, and the next daemon's start depends on the liveness probe
    /// deciding the stale file is dead. The signal handlers route a SIGTERM or
    /// SIGINT into the same orderly shutdown this asserts — the loop returns,
    /// the IPC task is dropped, and the endpoint's socket *and* lock are gone
    /// when it does.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_shutdown_request_releases_the_socket_and_its_lock() {
        let root = std::env::temp_dir().join(format!(
            "devmap-daemon-shutdown-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let socket = std::path::PathBuf::from(format!(
            "/tmp/devmap-shutdown-{:016x}.sock",
            devmap_extract::content_hash(&root.to_string_lossy())
        ));
        let lock = crate::protocol::ipc_lock_path(&socket);
        let _ = fs::remove_file(&socket);
        let _ = fs::remove_file(&lock);

        let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone())
            .with_ipc_path(socket.clone())
            .with_idle_poll(Duration::from_millis(20));
        let running = daemon.clone();
        let task = tokio::spawn(async move { running.run_loop().await });

        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket.exists(), "daemon IPC socket did not start");
        assert!(lock.exists(), "daemon IPC lock was never taken");

        daemon.request_shutdown();
        let outcome = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the run loop must return on a shutdown request")
            .expect("the run loop task must not panic");
        assert!(outcome.is_ok(), "shutdown reported an error: {outcome:?}");

        assert!(
            !socket.exists(),
            "an orderly shutdown left its socket at {}",
            socket.display()
        );
        assert!(
            !lock.exists(),
            "an orderly shutdown left its lock file at {}",
            lock.display()
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// A daemon whose repository was deleted must exit, not wait out its idle
    /// bound.
    ///
    /// Measured on this machine: 288 daemons alive at once, one per deleted
    /// pytest temporary directory, each holding a store that no longer existed
    /// and each waiting out a 30-minute idle bound. The idle bound is the wrong
    /// instrument for this — it is a bound on *quiet*, and a daemon whose tree
    /// is gone is not quiet, it is pointless.
    ///
    /// Idle retirement is disabled here, so nothing but the vanished root can
    /// end this loop.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_daemon_whose_repository_was_deleted_exits() {
        let root = std::env::temp_dir().join(format!(
            "devmap-daemon-vanished-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let socket = std::path::PathBuf::from(format!(
            "/tmp/devmap-vanished-{:016x}.sock",
            devmap_extract::content_hash(&root.to_string_lossy())
        ));
        let lock = crate::protocol::ipc_lock_path(&socket);
        let _ = fs::remove_file(&socket);
        let _ = fs::remove_file(&lock);

        let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone())
            .with_ipc_path(socket.clone())
            .with_idle_poll(Duration::from_millis(20))
            // `None` disables idle retirement, so nothing but the vanished
            // repository can end this loop.
            .with_max_idle(None);
        let task = tokio::spawn(async move { daemon.run_loop().await });

        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket.exists(), "daemon IPC socket did not start");

        // Still serving while the tree is there: this must be the deletion that
        // ends it, not merely elapsed time.
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !task.is_finished(),
            "the daemon exited before anything vanished"
        );

        fs::remove_dir_all(&root).unwrap();

        let outcome = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("a daemon whose repository was deleted must exit")
            .expect("the run loop task must not panic");
        assert!(outcome.is_ok(), "exit reported an error: {outcome:?}");
        assert!(!socket.exists(), "a vanished daemon left its socket behind");
        assert!(!lock.exists(), "a vanished daemon left its lock behind");
    }

    /// The store file is the other half: the root can survive a `rm` of the
    /// database alone, and a daemon serving a store that is gone answers from a
    /// connection to a deleted inode.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_daemon_whose_store_was_deleted_exits() {
        let root = std::env::temp_dir().join(format!(
            "devmap-daemon-nostore-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let db = root.join("devmap.sqlite");
        let socket = std::path::PathBuf::from(format!(
            "/tmp/devmap-nostore-{:016x}.sock",
            devmap_extract::content_hash(&root.to_string_lossy())
        ));
        let lock = crate::protocol::ipc_lock_path(&socket);
        let _ = fs::remove_file(&socket);
        let _ = fs::remove_file(&lock);

        let daemon = Daemon::new(Store::open(&db).unwrap(), root.clone())
            .with_store_path(db.clone())
            .with_ipc_path(socket.clone())
            .with_idle_poll(Duration::from_millis(20))
            // `None` disables idle retirement, so nothing but the vanished
            // repository can end this loop.
            .with_max_idle(None);
        let task = tokio::spawn(async move { daemon.run_loop().await });

        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket.exists(), "daemon IPC socket did not start");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(
            !task.is_finished(),
            "the daemon exited before anything vanished"
        );

        fs::remove_file(&db).unwrap();

        let outcome = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("a daemon whose store was deleted must exit")
            .expect("the run loop task must not panic");
        assert!(outcome.is_ok(), "exit reported an error: {outcome:?}");
        assert!(
            !socket.exists(),
            "a storeless daemon left its socket behind"
        );
        assert!(!lock.exists(), "a storeless daemon left its lock behind");
        let _ = fs::remove_dir_all(&root);
    }

    /// The IPC path must be a function of the *canonical* root.
    ///
    /// It hashed the root string as given, so `devmap serve .`, `devmap serve
    /// /tmp/repo` and `devmap serve` through a symlinked path each derived a
    /// different socket for one repository — and each spawned its own daemon
    /// against the same store. The Python client mirrors this formula without
    /// spawning anything, so it has to be stated in terms of something both
    /// sides can compute: FNV-1a 64 over the UTF-8 bytes of the canonical root.
    #[cfg(unix)]
    #[test]
    fn the_ipc_path_is_a_function_of_the_canonical_root() {
        let base = std::env::temp_dir().join(format!(
            "devmap-ipc-canon-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let real = base.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = base.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let direct = default_ipc_path_for(&real);
        assert_eq!(
            direct,
            default_ipc_path_for(&link),
            "a symlinked root derived a different endpoint than the directory it \
             points at, so one repository would be served by two daemons"
        );
        assert_eq!(
            direct,
            default_ipc_path_for(&real.join(".")),
            "a trailing `.` derived a different endpoint"
        );

        // The formula itself, so the Python client can mirror it: FNV-1a 64
        // over the UTF-8 bytes of the canonical root, in a 0700 directory under
        // the system temp dir.
        let canonical = real.canonicalize().unwrap();
        let expected = std::env::temp_dir()
            .join(format!(
                "devmap-{:016x}",
                devmap_extract::content_hash(&canonical.to_string_lossy())
            ))
            .join("ipc.sock");
        assert_eq!(direct, expected);

        fs::remove_dir_all(&base).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_daemon_cleans_up_ipc_child_and_socket() {
        let root = std::env::temp_dir().join(format!(
            "devmap-daemon-cancel-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let socket = std::path::PathBuf::from(format!(
            "/tmp/devmap-cancel-{:016x}.sock",
            devmap_extract::content_hash(&root.to_string_lossy())
        ));
        let _ = fs::remove_file(&socket);
        let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone())
            .with_ipc_path(socket.clone());
        let task = tokio::spawn(async move { daemon.run_loop().await });

        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket.exists(), "daemon IPC socket did not start");
        task.abort();
        let _ = task.await;
        for _ in 0..100 {
            if !socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!socket.exists(), "cancelled daemon leaked its IPC child");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn watcher_edit_reaches_durable_generation_and_ipc_query() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let root = std::env::temp_dir().join(format!(
            "devmap-daemon-e2e-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
        let source = root.join("main.py");
        fs::write(&source, "def old_symbol():\n    return 1\n").unwrap();
        let initial = extract_tree(&root).unwrap();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&initial);
        let resolution = resolver.resolve_all(&initial);
        let analysis = analyze(&initial, &resolution);
        let database = root.join(".devcouncil/codeintel/index.sqlite");
        let store = Store::open(&database).unwrap();
        save_scratch_generation(&store, &initial, &resolution, &analysis);
        let socket = root.join("devmap.sock");
        let daemon = Daemon::new(store, root.clone())
            .with_idle_poll(Duration::from_millis(50))
            .with_ipc_path(socket.clone());
        let task = tokio::spawn(async move { daemon.run_loop().await });
        for _ in 0..200 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(socket.exists(), "daemon IPC socket did not start");

        // Rewrite each round, then stay quiet longer than the watcher's
        // debounce before rewriting again.
        //
        // A single write up front can land before the platform watch is
        // registered and be missed outright, which no amount of waiting
        // recovers. But the watcher only flushes after `DEBOUNCE_SETTLE` of
        // silence, so retrying faster than that resets the timer and starves
        // the very callback it is trying to trigger — the failure mode that
        // made `watcher_never_reports_its_own_database_files` flaky under load.
        const DEBOUNCE_SETTLE: Duration = Duration::from_secs(2);
        let quiet_window = DEBOUNCE_SETTLE * 3;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut found = false;
        let mut round = 0u32;
        let mut next_write = tokio::time::Instant::now();
        while tokio::time::Instant::now() < deadline {
            if tokio::time::Instant::now() >= next_write {
                round += 1;
                fs::write(
                    &source,
                    format!("# probe {round}\ndef new_symbol():\n    return 2\n"),
                )
                .unwrap();
                next_write = tokio::time::Instant::now() + quiet_window;
            }
            let mut stream = tokio::net::UnixStream::connect(&socket).await.unwrap();
            stream
                .write_all(
                    b"{\"version\":1,\"cmd\":\"search\",\"query\":\"new_symbol\",\"budget\":2000}\n",
                )
                .await
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            let payload: serde_json::Value = serde_json::from_str(response.trim()).unwrap();
            if payload["ok"] == true && payload["result"]["total"].as_u64().unwrap_or(0) > 0 {
                found = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            found,
            "watcher edit never became query-visible before deadline"
        );

        task.abort();
        let _ = task.await;
        for _ in 0..100 {
            if !socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!socket.exists(), "daemon cancellation leaked socket");
        let reopened = Store::open(&database).unwrap();
        let persisted = reopened.latest_extractions().unwrap();
        assert!(persisted.iter().any(|extraction| {
            extraction.file_path == "main.py"
                && extraction
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == "new_symbol")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    /// K-B3: a stability check that could not run must not answer "stable".
    ///
    /// The guard read `before.modified().ok() == after.modified().ok()`. On a
    /// filesystem where `modified()` errors, both sides become `None`, `None ==
    /// None` is true, and the guard silently degrades to a length comparison —
    /// so an in-place edit that keeps the file's length is admitted as a clean
    /// read and stored as though it were the file on disk. The timestamps are
    /// the only thing that distinguishes those two cases, so losing them must
    /// make the answer "I cannot tell", not "yes".
    #[test]
    fn a_read_with_no_usable_timestamp_is_not_called_stable() {
        assert!(
            !read_is_stable(10, 10, None, None, 10),
            "with no modification time on either side there is nothing left but the \
             length, and equal lengths are exactly what a same-size in-place edit \
             produces. Answering `true` here is a check that could not run reporting \
             what a check that ran and passed reports."
        );
        // One side alone is no better: a comparison needs both.
        let now = std::time::SystemTime::now();
        assert!(
            !read_is_stable(10, 10, Some(now), None, 10),
            "half a timestamp comparison is not a timestamp comparison"
        );
        assert!(!read_is_stable(10, 10, None, Some(now), 10));
    }

    /// The OFF direction: a genuinely stable read must still be admitted, or
    /// the daemon simply stops indexing.
    #[test]
    fn a_read_with_matching_metadata_is_still_stable() {
        let now = std::time::SystemTime::now();
        assert!(
            read_is_stable(10, 10, Some(now), Some(now), 10),
            "same length, same mtime, and the content length agrees: this is the \
             case the guard exists to admit"
        );
        // And the real signals still reject.
        assert!(
            !read_is_stable(10, 11, Some(now), Some(now), 10),
            "a length that moved under the reader is a torn read"
        );
        let later = now + std::time::Duration::from_secs(1);
        assert!(
            !read_is_stable(10, 10, Some(now), Some(later), 10),
            "an mtime that moved under the reader is a torn read"
        );
        assert!(
            !read_is_stable(10, 10, Some(now), Some(now), 9),
            "content shorter than the file it came from is a torn read"
        );
    }
}
