use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tracing::warn;

use devmap_extract::{
    git_metadata, ignore_rule_files, is_gitignored_reporting, is_ignored_path, is_indexable_source,
};

const MAX_IGNORE_CACHE_ENTRIES: usize = 8_192;

#[derive(Debug, Clone, PartialEq, Eq)]
struct IgnoreRuleStamp {
    path: PathBuf,
    len: Option<u64>,
    modified: Option<SystemTime>,
}

#[derive(Debug)]
struct CachedIgnoreVerdict {
    rule_stamps: Vec<IgnoreRuleStamp>,
    ignored: bool,
}

#[derive(Debug, Default)]
struct IgnoreVerdictCache {
    entries: BTreeMap<(PathBuf, bool), CachedIgnoreVerdict>,
    rebuilds: usize,
    /// Rule-file diagnostics already logged, so a `.gitignore` line the kernel
    /// cannot compile is reported once rather than once per event for the life
    /// of the daemon. Bounded by the number of distinct rule files, and cleared
    /// with the verdicts whenever a rule file changes — so an edit that fixes
    /// the line stops the message, and one that breaks a new line prints it.
    reported_rule_problems: BTreeSet<String>,
}

impl IgnoreVerdictCache {
    fn stamp_rule(rule: PathBuf) -> anyhow::Result<IgnoreRuleStamp> {
        match std::fs::metadata(&rule) {
            Ok(metadata) if metadata.is_file() => Ok(IgnoreRuleStamp {
                path: rule,
                len: Some(metadata.len()),
                modified: metadata.modified().ok(),
            }),
            Ok(_) => Ok(IgnoreRuleStamp {
                path: rule,
                len: None,
                modified: None,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(IgnoreRuleStamp {
                path: rule,
                len: None,
                modified: None,
            }),
            Err(error) => Err(error.into()),
        }
    }

    fn rule_stamps(root: &Path, path: &Path, is_dir: bool) -> anyhow::Result<Vec<IgnoreRuleStamp>> {
        ignore_rule_files(root, path, is_dir)?
            .into_iter()
            .map(Self::stamp_rule)
            .collect()
    }

    fn is_ignored(&mut self, root: &Path, path: &Path, is_dir: bool) -> anyhow::Result<bool> {
        let key = (path.to_path_buf(), is_dir);
        let rule_stamps = Self::rule_stamps(root, path, is_dir)?;
        if let Some(cached) = self.entries.get(&key) {
            if cached.rule_stamps == rule_stamps {
                return Ok(cached.ignored);
            }
        }

        let (ignored, problems) = is_gitignored_reporting(root, path, is_dir)?;
        for problem in problems {
            if self.reported_rule_problems.insert(problem.clone()) {
                warn!("{problem}");
            }
        }
        self.rebuilds = self.rebuilds.saturating_add(1);
        if self.entries.len() >= MAX_IGNORE_CACHE_ENTRIES && !self.entries.contains_key(&key) {
            self.entries.clear();
        }
        self.entries.insert(
            key,
            CachedIgnoreVerdict {
                rule_stamps,
                ignored,
            },
        );
        Ok(ignored)
    }

    fn observe_rule_event(&mut self, path: &Path) -> bool {
        let is_rule = path.file_name().is_some_and(|name| name == ".gitignore")
            || path.ends_with("info/exclude");
        if is_rule {
            self.entries.clear();
            // The diagnostics describe the file that just changed, so they are
            // stale for the same reason the verdicts are.
            self.reported_rule_problems.clear();
        }
        is_rule
    }

    #[cfg(test)]
    fn rebuild_count(&self) -> usize {
        self.rebuilds
    }
}

/// Upper bound on how long a batch may sit unflushed while events keep
/// arriving. The debounce flushes only after `DEBOUNCE` of silence, so a
/// continuously churning tree (a build system, an editor autosave loop) never
/// went quiet and the buffered paths were never delivered — the daemon fell
/// behind forever. The cap forces delivery once the oldest pending path has
/// waited this long, even mid-storm.
const MAX_DEBOUNCE_HOLD: Duration = Duration::from_secs(10);

/// The most paths the debounce buffer will itemise before collapsing to a
/// whole-tree rescan.
///
/// K-B2. `pending` was an uncapped `BTreeSet<String>` held for up to
/// `MAX_DEBOUNCE_HOLD`, so a `git checkout` across a large tree grew it without
/// limit for ten seconds of churn — bounded only by how fast the tree changed.
///
/// The cap collapses rather than drops. Past this many paths the itemised list
/// has stopped being cheaper than re-walking the tree, and the root entry the
/// drain already expands covers every one of them plus anything the buffer had
/// not yet been told about. Dropping entries here would lose edits with nothing
/// recording the loss; collapsing loses only the itemisation.
const MAX_DEBOUNCE_PATHS: usize = 4_096;

/// How many watcher events may sit unread before the queue is treated as
/// overflowed.
///
/// K-B2. The notify-to-thread channel was `std::sync::mpsc::channel()` —
/// unbounded — so during a large `git checkout` it grew for as long as the
/// producer outran the consumer, with no ceiling and nothing recording that it
/// had. Bounding it is only half the fix: the send must also never *block*,
/// because the notify thread is the one the OS delivers to, and stalling it is
/// how events get dropped in the first place. So the send is a `try_send`, and
/// a refusal is recorded and answered with a whole-tree rescan — the same
/// answer the kernel's own "I dropped events" notice already gets (K-A3).
const WATCH_QUEUE_CAPACITY: usize = 4_096;

struct DebounceBuffer {
    debounce: Duration,
    pending: BTreeSet<String>,
    last_event: Option<Instant>,
    oldest_pending: Option<Instant>,
    /// What `pending` collapses to once it passes [`MAX_DEBOUNCE_PATHS`].
    ///
    /// Required rather than optional: a buffer with nowhere to collapse to
    /// could only enforce the cap by discarding paths, and a silently discarded
    /// edit is the failure this cap exists to prevent.
    collapse_to: Vec<String>,
    /// Set once `pending` has collapsed, cleared when the batch flushes.
    ///
    /// Sticky on purpose: a whole-tree rescan already covers anything that
    /// arrives before the flush, so continuing to itemise paths after the
    /// collapse is pure cost — and it is exactly the churn that caused the
    /// collapse, so it is cost paid at the worst moment.
    collapsed: bool,
}

impl DebounceBuffer {
    fn new(debounce: Duration, collapse_to: Vec<String>) -> Self {
        Self {
            debounce,
            pending: BTreeSet::new(),
            last_event: None,
            oldest_pending: None,
            collapse_to,
            collapsed: false,
        }
    }

    fn push<I, S>(&mut self, paths: I, now: Instant)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        // A fully-filtered batch carries no work, so it must not move either
        // clock: continuous gitignored churn would otherwise reset the
        // silence timer forever and starve real edits behind the noise.
        let admitted: Vec<String> = paths.into_iter().map(Into::into).collect();
        if admitted.is_empty() {
            return;
        }
        if self.oldest_pending.is_none() {
            self.oldest_pending = Some(now);
        }
        if self.collapsed {
            // The rescan already covers whatever this batch names. Record that
            // the tree is still moving, and nothing else.
            self.last_event = Some(now);
            return;
        }
        self.pending.extend(admitted);
        if self.pending.len() > MAX_DEBOUNCE_PATHS {
            self.pending = self.collapse_to.iter().cloned().collect();
            self.collapsed = true;
        }
        self.last_event = Some(now);
    }

    /// Everything held, regardless of how long it has been held.
    ///
    /// The timers exist to coalesce churn into fewer batches; on the way out
    /// there is no later batch to coalesce into, so waiting for one is waiting
    /// for something that will not happen. Used only by the stop paths.
    fn flush(&mut self) -> Option<Vec<String>> {
        if self.pending.is_empty() {
            return None;
        }
        self.last_event = None;
        self.oldest_pending = None;
        self.collapsed = false;
        Some(std::mem::take(&mut self.pending).into_iter().collect())
    }

    fn take_ready(&mut self, now: Instant) -> Option<Vec<String>> {
        let last_event = self.last_event?;
        if self.pending.is_empty() {
            return None;
        }
        let quiet_long_enough = now.saturating_duration_since(last_event) >= self.debounce;
        let held_too_long = self
            .oldest_pending
            .is_some_and(|oldest| now.saturating_duration_since(oldest) >= MAX_DEBOUNCE_HOLD);
        if !quiet_long_enough && !held_too_long {
            return None;
        }
        self.last_event = None;
        self.oldest_pending = None;
        self.collapsed = false;
        Some(std::mem::take(&mut self.pending).into_iter().collect())
    }
}

pub struct WatcherHandle {
    stop: Option<Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WatcherHandle {
    fn stop_and_join(&mut self) -> anyhow::Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("file watcher thread panicked"))?;
        }
        Ok(())
    }

    pub fn shutdown(mut self) -> anyhow::Result<()> {
        self.stop_and_join()
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        if let Err(error) = self.stop_and_join() {
            warn!("failed to stop file watcher cleanly: {error}");
        }
    }
}

/// The sentinel path enqueued when git's HEAD or refs move.
///
/// A real path would be re-extracted; this is not a file to extract, it is a
/// statement that the *whole* generation may describe a tree that no longer
/// exists. The daemon recognises it, drops it from the extraction set, and
/// forces a full rebuild.
pub const GIT_HEAD_SENTINEL: &str = "\u{0}devmap:git-head-changed";

/// Whether this path is git telling us the checkout moved. (B5)
///
/// `.git/` is a dotted directory and `is_ignored_path` prunes it, which is
/// correct for indexing and wrong for *noticing*: a commit, branch switch,
/// rebase or stash changes what the index should contain while touching no
/// watched file, so the daemon never woke at all. Without this the clean-room
/// design reproduces exactly the staleness the rewrite exists to fix.
///
/// Deliberately narrow. `.git/` churns constantly — object writes, index
/// updates, lock files — and admitting it wholesale would wake the daemon on
/// every `git status`. Only the files that define *which commit is checked
/// out* qualify:
///
/// - `HEAD` — the symbolic ref, rewritten by checkout and rebase
/// - `refs/**` — loose refs, rewritten by commit and reset
/// - `packed-refs` — the packed form of the same, rewritten by gc and clone
///
/// `.git/index` is excluded on purpose: it changes on `git add` with no change
/// to the working tree, and the working tree is what the graph describes.
fn is_git_ref_event(root: &Path, path: &Path) -> anyhow::Result<bool> {
    let dirs = match git_metadata(root)? {
        Some(metadata) => vec![metadata.git_dir, metadata.common_dir],
        None => vec![root.join(".git")],
    };
    Ok(dirs.iter().any(|dir| {
        let Ok(relative) = path.strip_prefix(dir) else {
            return false;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        !relative.ends_with(".lock")
            && (relative == "HEAD" || relative == "packed-refs" || relative.starts_with("refs/"))
    }))
}

/// The queue entry one watched path becomes, or `None` if there is nothing to
/// index there.
///
/// `None` is a positive claim — "this path is not work" — so it must never be
/// the answer given when the question could not be answered. That is why this
/// returns a plain `Option` and not a `Result`: the caller had an `Err` arm
/// that logged and returned `None`, which spelled "the ignore check failed"
/// exactly like "the ignore check said ignore", and a `warn!` nobody reads was
/// all that separated a frozen index from a quiet one.
fn admitted_watch_path(
    root: &Path,
    path: &Path,
    ignore_cache: &mut IgnoreVerdictCache,
) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    // A name whose bytes are not UTF-8 cannot be a queue entry: the pending
    // queue is keyed by `String` and so is every stored extraction. Lossy
    // conversion does not produce this path, it produces a *different* one —
    // `U+FFFD` where the bytes were — naming a file that does not exist. The
    // drain then took its `!exists()` branch and recorded a **deletion** of a
    // path nothing had ever indexed, so an edit to a real file arrived at the
    // store as the removal of an imaginary one and the real file went
    // unindexed with nothing saying so.
    //
    // Refused and named instead, which is what `collect_sources_with_report`
    // already answers for the same name (`DiscoverySkipReason::NonUtf8Path`)
    // and what the connect-time sweep already logs. `None` is honest here in a
    // way it would not be for an undecidable ignore verdict: this is a settled
    // fact about the name, not a check that failed to run, and no retry can
    // change it.
    let (Some(absolute), Some(relative)) = (path.to_str(), relative.to_str()) else {
        warn!(
            "watcher skipped unrepresentable non-UTF-8 path {path:?}; it cannot be a \
             queue entry and is absent from the graph — this is a refusal, not a \
             clean tree"
        );
        return None;
    };
    let relative = relative.replace('\\', "/");
    if is_ignored_path(&relative) {
        return None;
    }
    let metadata = std::fs::metadata(path);
    let is_dir = metadata.as_ref().is_ok_and(|metadata| metadata.is_dir());
    // Fail *open*, for the reason `head_differs_from_last_generation` already
    // states: a redundant re-extract costs time, a dropped one costs
    // correctness. The undecidable cases are real — a symlink loop, a parent
    // that lost `+x`, a stale handle on a network mount — and the store's queue
    // is where an unresolvable path becomes visible (it is retried, then
    // quarantined), not this filter.
    let ignored = match ignore_cache.is_ignored(root, path, is_dir) {
        Ok(ignored) => ignored,
        Err(error) => {
            warn!(
                "cannot evaluate ignore rules for {path:?}; queueing it rather than \
                 mistaking a check that could not run for one that said `ignore`: {error}"
            );
            false
        }
    };
    if ignored {
        return None;
    }
    match metadata {
        Ok(metadata) if metadata.is_dir() || is_indexable_source(&relative) => {
            Some(absolute.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Removed files and directories no longer have metadata. Admit
            // them so deletion reconciliation can remove previously indexed
            // descendants; the daemon re-checks indexability and scope.
            Some(absolute.to_string())
        }
        _ => None,
    }
}

/// The queue entry that asks for a whole-tree rescan.
///
/// Not a new control token: the repository root already *is* the store's
/// spelling for this — "the root itself: a whole-tree rescan the drain expands"
/// (`classify_pending_entry`) — and the drain expands a queued directory into a
/// full discovery pass with deletion reconciliation. Reusing it means a lost
/// coverage notice also makes `pending_count` non-zero, so `status` answers
/// `is_fresh: false` until the rescan has actually run.
pub(crate) fn whole_tree_rescan(root: &Path) -> Vec<String> {
    vec![root.to_string_lossy().into_owned()]
}

/// The paths one watcher event asks the daemon to re-examine.
///
/// Split out of the receive loop so each class of event can be asserted
/// directly: the loop itself is driven by the OS, and the notices that matter
/// most here are exactly the ones a test cannot make the kernel emit on demand.
fn watch_event_paths(
    root: &Path,
    event: notify::Event,
    ignore_cache: &mut IgnoreVerdictCache,
) -> Vec<String> {
    // The OS telling us it dropped events is the one notice that must never be
    // filtered, and it arrives shaped like nothing else: `EventKind::Other`
    // carrying `Flag::Rescan` and *no paths at all*
    // (`notify-6.1.1/src/fsevent.rs:114-123` for FSEvents `MUST_SCAN_SUBDIRS`,
    // `src/inotify.rs:208-209` for `Q_OVERFLOW`). It therefore fell into the
    // catch-all below and was discarded — after which `pending_count` stayed 0,
    // `status` answered `is_fresh: true`, and every later query was served from
    // a generation missing an arbitrary, unknowable subset of the edits. Since
    // `reconcile_connect_time` is the only full sweep and runs once at startup,
    // that gap persisted until the daemon restarted.
    if event.need_rescan() {
        warn!(
            "the OS reported dropped filesystem events for {root:?} \
             ({:?}); requesting a whole-tree rescan",
            event.info().unwrap_or("no detail")
        );
        return whole_tree_rescan(root);
    }
    if matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        event
            .paths
            .into_iter()
            .filter_map(|path| {
                if ignore_cache.observe_rule_event(&path) {
                    // Rules can expose or hide files without touching source.
                    return whole_tree_rescan(root).into_iter().next();
                }
                // Checked before the ignore rules, because `.git/` is pruned by
                // them and this is the one thing inside it the daemon has to
                // see.
                match is_git_ref_event(root, &path) {
                    Ok(true) => return Some(GIT_HEAD_SENTINEL.to_string()),
                    Ok(false) => {}
                    Err(error) => {
                        warn!(
                            "cannot resolve Git metadata for {root:?}: {error}; requesting rescan"
                        );
                        return whole_tree_rescan(root).into_iter().next();
                    }
                }
                admitted_watch_path(root, &path, ignore_cache)
            })
            .collect()
    } else {
        Vec::new()
    }
}

/// The paths a watcher *error* asks the daemon to re-examine.
///
/// Also lost coverage, and treated the same way. `ErrorKind::MaxFilesWatch`
/// ("OS file watch limit reached") means whole subtrees are no longer watched
/// at all; the others mean this watcher no longer knows what it is seeing.
/// Logging and continuing left the daemon reporting fresh about a tree it had
/// stopped watching.
fn watch_error_paths(root: &Path, error: &notify::Error) -> Vec<String> {
    warn!("watch error ({error:?}); requesting a whole-tree rescan of {root:?}");
    whole_tree_rescan(root)
}

/// Hand one raw notify event to the consumer thread, or record that it could
/// not be handed over.
///
/// Never blocks. This runs on the thread notify delivers to, and blocking it is
/// precisely what makes the OS drop events — so a full queue is a *refusal*,
/// and `overflowed` is what keeps that refusal from passing for a quiet tree.
/// The flag is a plain `bool` because there is nothing to order against it: the
/// consumer's answer is the same whether one event was refused or a million.
///
/// Split out of the closure so the refusal can be produced deterministically:
/// staging it through the real OS means racing a real `git checkout` against a
/// real consumer, which the suite cannot schedule.
fn offer_watch_event(
    tx: &std::sync::mpsc::SyncSender<notify::Result<notify::Event>>,
    overflowed: &AtomicBool,
    event: notify::Result<notify::Event>,
) {
    if tx.try_send(event).is_err() {
        overflowed.store(true, Ordering::Relaxed);
    }
}

/// The watcher's consumer loop: debounce, answer refusals, deliver batches.
///
/// `debounce` is a parameter rather than the constant so a test can drive the
/// loop on events alone instead of on elapsed time.
fn run_watch_loop<F: Fn(Vec<String>)>(
    root: &Path,
    rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    stop_rx: std::sync::mpsc::Receiver<()>,
    overflowed: &AtomicBool,
    debounce: Duration,
    callback: F,
) {
    let mut buffer = DebounceBuffer::new(debounce, whole_tree_rescan(root));
    let mut ignore_cache = IgnoreVerdictCache::default();

    loop {
        if stop_rx.try_recv().is_ok() {
            // Hand over what is buffered before the thread goes away.
            //
            // The debounce holds an observed edit in memory for `DEBOUNCE`, and
            // the store's pending queue is the only place that edit is ever
            // written down. Breaking straight out took the buffer with it — and
            // the daemon's own exit line says the opposite: "pending work stays
            // queued in the store". Measured with the release binary, a file
            // edited and `SIGTERM` 500 ms later left `gen=1 pending=0
            // is_fresh=true` and the new symbol absent from the index. The next
            // daemon's connect-time sweep re-finds it by content hash, so this
            // is a window and not a permanent loss; it is a window in which
            // every reader is told the index is current, and nothing has to
            // start a daemon to be told that.
            if let Some(paths) = buffer.flush() {
                callback(paths);
            }
            break;
        }
        // Deliver matured batches on the event path too: a tree under
        // continuous churn never lets `recv_timeout` expire, so a flush
        // evaluated only on silence would never run again.
        // A refused event is answered before anything else: the rescan it
        // asks for supersedes whatever individual paths are queued behind
        // it, and delaying it would let the batch flush as though the queue
        // had never overflowed.
        if overflowed.swap(false, Ordering::Relaxed) {
            warn!(
                "watcher event queue overflowed ({WATCH_QUEUE_CAPACITY} events); \
                 requesting a whole-tree rescan of {root:?} rather than reporting \
                 the tree quiet"
            );
            buffer.push(whole_tree_rescan(root), Instant::now());
        }
        let now = Instant::now();
        if let Some(paths) = buffer.take_ready(now) {
            callback(paths);
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(event)) => {
                let admitted = watch_event_paths(root, event, &mut ignore_cache);
                buffer.push(admitted, Instant::now());
            }
            Ok(Err(error)) => {
                let admitted = watch_error_paths(root, &error);
                buffer.push(admitted, Instant::now());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Some(paths) = buffer.take_ready(Instant::now()) {
                    callback(paths);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // The producer is gone, so nothing more will arrive — but what
                // already arrived is still owed to the queue, for the reason the
                // stop path above gives.
                if let Some(paths) = buffer.flush() {
                    callback(paths);
                }
                break;
            }
        }
    }
}

/// Start a debounced recursive file watcher. The watcher thread owns the
/// `RecommendedWatcher` so it is not dropped immediately (previous bug).
pub fn start_file_watcher<P: AsRef<Path>, F: Fn(Vec<String>) + Send + 'static>(
    root_path: P,
    callback: F,
) -> anyhow::Result<WatcherHandle> {
    let root = root_path.as_ref().canonicalize()?;
    let (tx, rx) = sync_channel(WATCH_QUEUE_CAPACITY);
    let (stop_tx, stop_rx) = channel();
    // Set when the queue refuses an event. Read and cleared by the loop, which
    // answers it with a rescan. An event that cannot be queued is lost
    // coverage, and lost coverage must not be indistinguishable from a quiet
    // tree — the same rule K-A3 applies to the kernel's own drop notice.
    let overflowed = Arc::new(AtomicBool::new(false));
    let overflow_writer = Arc::clone(&overflowed);
    let mut watcher = RecommendedWatcher::new(
        move |event| offer_watch_event(&tx, &overflow_writer, event),
        Config::default(),
    )?;
    watcher.watch(&root, RecursiveMode::Recursive)?;
    if let Some(metadata) = git_metadata(&root)? {
        // Linked worktrees keep HEAD and shared refs/excludes outside the source
        // root. Watch only metadata surfaces, never shared object storage.
        let mut watched = BTreeSet::new();
        for (path, recursive) in [
            (metadata.git_dir, false),
            (metadata.common_dir.clone(), false),
            (metadata.common_dir.join("refs"), true),
            (metadata.common_dir.join("info"), true),
        ] {
            if !path.starts_with(&root) && path.exists() && watched.insert(path.clone()) {
                watcher.watch(
                    &path,
                    if recursive {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    },
                )?;
            }
        }
    }

    let thread = std::thread::spawn(move || {
        let _watcher = watcher; // keep alive for the thread lifetime
        run_watch_loop(
            &root,
            rx,
            stop_rx,
            &overflowed,
            Duration::from_secs(2),
            callback,
        );
    });

    Ok(WatcherHandle {
        stop: Some(stop_tx),
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn scratch_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "devmap-watcher-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn linked_worktree_ignore_rules_and_external_head_are_observed() {
        let scratch = scratch_root("linked-metadata");
        let root = scratch.join("worktree");
        let common = scratch.join("shared.git");
        let git_dir = common.join("worktrees/session");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(common.join("info")).unwrap();
        std::fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        std::fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        std::fs::write(common.join("info/exclude"), "ignored.py\n").unwrap();
        std::fs::write(root.join("ignored.py"), "pass\n").unwrap();
        let mut cache = IgnoreVerdictCache::default();
        assert!(cache
            .is_ignored(&root, &root.join("ignored.py"), false)
            .unwrap());
        assert!(is_git_ref_event(&root, &git_dir.join("HEAD")).unwrap());
        assert!(is_git_ref_event(&root, &common.join("refs/heads/main")).unwrap());
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[test]
    fn changing_ignore_rules_requests_a_rescan_without_a_source_edit() {
        let root = scratch_root("ignore-rescan");
        let event = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.join(".gitignore"));
        assert_eq!(
            watch_event_paths(&root, event, &mut IgnoreVerdictCache::default()),
            whole_tree_rescan(&root)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// K-A3: the OS saying "I dropped events, rescan" is the one notice the
    /// watcher must never filter.
    ///
    /// Both backends deliver it as `EventKind::Other` carrying `Flag::Rescan`
    /// (verified in the vendored crate: `notify-6.1.1/src/fsevent.rs:114-123`
    /// for FSEvents `MUST_SCAN_SUBDIRS`, `src/inotify.rs:208-209` for
    /// `Q_OVERFLOW`), and it carries no paths. Dropping it leaves the queue
    /// empty, so `pending_count` stays 0 and `status` answers `is_fresh: true`
    /// about a generation missing an arbitrary, unknowable subset of the edits.
    ///
    /// The repository root is the store's own spelling for "a whole-tree
    /// rescan the drain expands" (`classify_pending_entry`), so that is what a
    /// lost-coverage notice must enqueue.
    #[test]
    fn a_dropped_event_notice_requests_a_whole_tree_rescan() {
        let root = scratch_root("rescan");
        let mut cache = IgnoreVerdictCache::default();

        let dropped = notify::Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert!(
            dropped.need_rescan(),
            "the fixture must be the notice the OS actually sends"
        );

        assert_eq!(
            watch_event_paths(&root, dropped, &mut cache),
            vec![root.to_string_lossy().into_owned()],
            "a dropped-event notice must enqueue the repository root; \
             discarding it freezes the index while `status` keeps saying fresh"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Positive control for the rescan handling: an ordinary event is still
    /// classified exactly as before, and an uninteresting one still yields
    /// nothing. A watcher that answers "rescan the world" to every event is a
    /// different bug with the same green test.
    #[test]
    fn ordinary_events_are_unaffected_by_the_rescan_handling() {
        let root = scratch_root("rescan-control");
        std::fs::write(root.join("main.py"), "def main(): pass\n").unwrap();
        let mut cache = IgnoreVerdictCache::default();

        let modified = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.join("main.py"));
        assert_eq!(
            watch_event_paths(&root, modified, &mut cache),
            vec![root.join("main.py").to_string_lossy().into_owned()],
            "an ordinary edit must still be admitted as itself"
        );

        // `EventKind::Other` *without* the rescan flag carries no claim about
        // lost coverage, and `Access` never did.
        for uninteresting in [
            notify::Event::new(EventKind::Other).add_path(root.join("main.py")),
            notify::Event::new(EventKind::Access(notify::event::AccessKind::Read))
                .add_path(root.join("main.py")),
        ] {
            assert!(
                watch_event_paths(&root, uninteresting, &mut cache).is_empty(),
                "only a rescan notice may ask for a whole-tree rescan"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// K-A3, second half: a watch *error* is also lost coverage.
    ///
    /// `notify::ErrorKind::MaxFilesWatch` ("OS file watch limit reached") means
    /// whole subtrees are no longer watched. Logging it and continuing leaves
    /// the daemon reporting fresh about a tree it has stopped seeing.
    #[test]
    fn a_watch_error_requests_a_whole_tree_rescan() {
        let root = scratch_root("watch-error");
        let error = notify::Error::new(notify::ErrorKind::MaxFilesWatch);

        assert_eq!(
            watch_error_paths(&root, &error),
            vec![root.to_string_lossy().into_owned()],
            "a watcher that lost its watches must ask for a rescan, not just log"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// K-A1 at the watcher: one malformed glob in `.gitignore` must not make
    /// the watcher drop every path in the tree.
    ///
    /// `[z-a]` is a plausible typo that `git` tolerates and `ignore` rejects.
    /// `GitignoreBuilder::add` reports it as a *partial* error with every other
    /// line still compiled, but `add_ignore_rules` treated any return as fatal,
    /// so the verdict for every path became `Err` — which this loop read as
    /// "not a path to index", the same answer a genuinely ignored path gets.
    #[test]
    fn a_malformed_ignore_glob_does_not_make_the_watcher_drop_the_tree() {
        let root = scratch_root("malformed-glob");
        std::fs::write(root.join(".gitignore"), "ignored.py\n[z-a]\n").unwrap();
        std::fs::write(root.join("main.py"), "def main(): pass\n").unwrap();
        std::fs::write(root.join("ignored.py"), "def ignored(): pass\n").unwrap();
        let mut cache = IgnoreVerdictCache::default();

        let edited = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.join("main.py"));
        assert_eq!(
            watch_event_paths(&root, edited, &mut cache),
            vec![root.join("main.py").to_string_lossy().into_owned()],
            "one bad glob must not freeze the incremental index"
        );

        // Positive control: the rules that *are* well formed still apply.
        let ignored = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.join("ignored.py"));
        assert!(
            watch_event_paths(&root, ignored, &mut cache).is_empty(),
            "a genuinely ignored file must still be dropped"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// K-A1(b): an ignore verdict that could not be *computed* must not be
    /// spelled the same way as a verdict of "ignored".
    ///
    /// A `.gitignore` that is a symlink loop is the readable form of the cases
    /// the store already names as undecidable — "a symlink loop, a parent that
    /// lost `+x`, a stale handle on a network mount". Stamping it fails, the
    /// verdict fails, and the path was dropped with a `warn!` nobody reads.
    /// A redundant re-extract costs time; a dropped one costs correctness.
    #[cfg(unix)]
    #[test]
    fn an_uncomputable_ignore_verdict_admits_the_path_instead_of_dropping_it() {
        let root = scratch_root("undecidable");
        std::fs::write(root.join(".gitignore"), "ignored.py\n").unwrap();
        std::fs::write(root.join("ignored.py"), "def ignored(): pass\n").unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/file.py"), "def f(): pass\n").unwrap();
        // A self-referential symlink: every `stat` of it returns ELOOP, so the
        // rule stamp — and with it the verdict — cannot be computed.
        std::os::unix::fs::symlink(".gitignore", root.join("sub/.gitignore")).unwrap();
        assert!(
            std::fs::metadata(root.join("sub/.gitignore")).is_err(),
            "the fixture must actually be unstattable"
        );
        let mut cache = IgnoreVerdictCache::default();

        let edited = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.join("sub/file.py"));
        assert_eq!(
            watch_event_paths(&root, edited, &mut cache),
            vec![root.join("sub/file.py").to_string_lossy().into_owned()],
            "an ignore check that could not run must admit the path, not drop it"
        );

        // Positive control: failing open must not disable the rules that can be
        // evaluated.
        let ignored = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
            .add_path(root.join("ignored.py"));
        assert!(
            watch_event_paths(&root, ignored, &mut cache).is_empty(),
            "a computable verdict of `ignored` must still drop the path"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A name whose bytes are not UTF-8 must be refused, not fabricated into a
    /// different path.
    ///
    /// The queue is string-keyed, and so is every stored extraction. The
    /// watcher answered that by lossily converting, which does not produce the
    /// path — it produces a *different* one, with `U+FFFD` where the bytes
    /// were, naming a file that does not exist. The drain then found no such
    /// file, took its `!exists()` branch, and recorded a **deletion** of a path
    /// nothing had ever indexed. So an edit to a real file arrived at the store
    /// as the removal of an imaginary one, and nothing anywhere recorded that
    /// the real file had gone unindexed.
    ///
    /// `collect_sources_with_report` already refuses these
    /// (`DiscoverySkipReason::NonUtf8Path`) and the connect-time sweep already
    /// logs the refusal. This is the same fact reaching the queue by the other
    /// producer, and it gets the same answer.
    /// The fixture is a path that does not exist, which is the shape this
    /// reaches the watcher in on macOS — APFS rejects a non-UTF-8 name with
    /// `EILSEQ`, so only removals and events for names created elsewhere can
    /// carry one. On Linux the same name is an ordinary file and the create and
    /// modify events carry it too; both take this branch of
    /// `admitted_watch_path`, and both used to fabricate.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_refused_rather_than_fabricated() {
        use std::os::unix::ffi::OsStrExt;

        let root = scratch_root("non-utf8");
        // `caf<0xE9>.py`: latin-1 `é`, which is not valid UTF-8.
        let path = root.join(std::ffi::OsStr::from_bytes(b"caf\xe9.py"));
        assert!(
            path.to_str().is_none(),
            "the fixture must actually be unrepresentable"
        );
        let mut cache = IgnoreVerdictCache::default();

        assert_eq!(
            admitted_watch_path(&root, &path, &mut cache),
            None,
            "a path that cannot be a queue entry must be refused; the lossy \
             spelling names a file that does not exist, and the drain reconciles \
             that as a deletion"
        );

        // Positive control: refusing the unrepresentable must not start
        // refusing the ordinary.
        let ordinary = root.join("cafe.py");
        std::fs::write(&ordinary, "def cafe():\n    return 1\n").unwrap();
        assert_eq!(
            admitted_watch_path(&root, &ordinary, &mut cache),
            Some(ordinary.to_string_lossy().into_owned()),
            "a representable source is still queued"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A refused event must reach the daemon as a whole-tree rescan, ahead of
    /// everything the queue still holds.
    ///
    /// The event queue is a `sync_channel` and the notify callback `try_send`s
    /// into it, because blocking the thread the OS delivers on is precisely how
    /// events get dropped. That makes a full queue a *refusal*, and a refused
    /// event is lost coverage — which must never be indistinguishable from a
    /// quiet tree (K-A3).
    ///
    /// Staged without the OS: `offer_watch_event` is the producer the notify
    /// callback calls and `run_watch_loop` is the consumer thread, so the
    /// overflow can be produced by filling the queue directly instead of hoping
    /// a real `git checkout` outruns a real consumer. The loop is driven with a
    /// zero debounce so the flush is a function of the events, not of elapsed
    /// time — this asserts ordering and content, never a duration.
    #[test]
    fn a_refused_event_becomes_a_rescan_ahead_of_the_queued_batch() {
        let root = scratch_root("overflow");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
        let edited = || {
            Ok(
                notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(root.join("src/a.py")),
            )
        };

        let (tx, rx) = sync_channel(WATCH_QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));

        for filled in 0..WATCH_QUEUE_CAPACITY {
            offer_watch_event(&tx, &overflowed, edited());
            assert!(
                !overflowed.load(Ordering::Relaxed),
                "the queue still had room at {filled} of {WATCH_QUEUE_CAPACITY}; \
                 an overflow flag that trips early would ask for a whole-tree \
                 rescan on every busy moment"
            );
        }
        offer_watch_event(&tx, &overflowed, edited());
        assert!(
            overflowed.load(Ordering::Relaxed),
            "the {}st event has nowhere to go and must be recorded as refused, \
             not dropped in silence",
            WATCH_QUEUE_CAPACITY + 1
        );

        let (stop_tx, stop_rx) = channel();
        let (batches_tx, batches_rx) = channel();
        let loop_root = root.clone();
        let loop_flag = Arc::clone(&overflowed);
        let thread = std::thread::spawn(move || {
            run_watch_loop(
                &loop_root,
                rx,
                stop_rx,
                &loop_flag,
                Duration::ZERO,
                move |paths| {
                    let _ = batches_tx.send(paths);
                },
            );
        });

        let first = batches_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the loop must deliver the refusal's answer");
        assert_eq!(
            first,
            whole_tree_rescan(&root),
            "the first thing delivered after a refusal must be the whole-tree \
             rescan: the individual paths still queued behind it cannot cover \
             the events that were turned away, and flushing them first lets the \
             batch land as though the queue had never overflowed"
        );
        assert!(
            !overflowed.load(Ordering::Relaxed),
            "and the flag must be consumed, or every later tick re-requests a \
             rescan that already ran"
        );

        let _ = stop_tx.send(());
        drop(tx);
        thread.join().expect("the watch loop must stop cleanly");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A batch still inside the debounce window is delivered on the way out.
    ///
    /// The debounce holds an observed edit in memory for `DEBOUNCE` (2 s), and
    /// the store's pending queue is the only place that edit is ever written
    /// down. The loop's stop path broke straight out of the loop, so everything
    /// the buffer was holding went with the thread — and the daemon says the
    /// opposite as it goes:
    ///
    /// ```text
    /// INFO SIGTERM received; releasing the IPC endpoint and exiting
    ///      (pending work stays queued in the store)
    /// ```
    ///
    /// Measured against the pre-fix release binary: a file edited, `SIGTERM`
    /// 500 ms later, and then, with no daemon left running,
    ///
    /// ```text
    /// gen=1 nodes=60 fresh=True pending=0 degraded=None
    /// ADDED_DURING_DEBOUNCE present in the index: False
    /// ```
    ///
    /// The next daemon's connect-time sweep does eventually re-find it by
    /// content hash, so this is a window rather than a permanent loss — but it
    /// is a window in which every reader is told the index is current, and
    /// nothing has to start a daemon to be told that. An editor that saves and a
    /// tool that restarts the daemon are the ordinary way to land in it.
    ///
    /// `Duration::from_secs(30)` as the debounce, so the flush under test can
    /// only be the shutdown one: nothing else could have matured in the
    /// milliseconds this test runs for.
    #[test]
    fn a_debounced_batch_is_delivered_when_the_watcher_is_told_to_stop() {
        let root = scratch_root("shutdown-flush");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();

        let (tx, rx) = sync_channel(WATCH_QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        offer_watch_event(
            &tx,
            &overflowed,
            Ok(
                notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(root.join("src/a.py")),
            ),
        );

        let (stop_tx, stop_rx) = channel();
        let (batches_tx, batches_rx) = channel();
        let loop_root = root.clone();
        let loop_flag = Arc::clone(&overflowed);
        let thread = std::thread::spawn(move || {
            run_watch_loop(
                &loop_root,
                rx,
                stop_rx,
                &loop_flag,
                // Far longer than this test lives, so a delivery here is the
                // shutdown flush and cannot be the debounce maturing.
                Duration::from_secs(30),
                move |paths| {
                    let _ = batches_tx.send(paths);
                },
            );
        });

        // Let the loop take the event off the queue and into the buffer. The
        // receive poll is 250 ms; this is comfortably past it, and the assertion
        // below distinguishes "not yet buffered" from "dropped" by failing with
        // a timeout either way — which is the honest outcome for both.
        std::thread::sleep(Duration::from_millis(900));
        assert!(
            batches_rx.try_recv().is_err(),
            "fixture precondition: with a 30 s debounce nothing may have been \
             delivered yet, or this test is measuring the ordinary flush"
        );

        let _ = stop_tx.send(());
        let delivered = batches_rx.recv_timeout(Duration::from_secs(10)).expect(
            "a watcher told to stop must hand over what it is holding. The daemon \
             logs `pending work stays queued in the store` as it exits, and the \
             store's queue is the only place a debounced edit is ever written \
             down — dropped here, the edit is absent from the index while \
             `status` reports `is_fresh: true` to every reader until some later \
             daemon's connect-time sweep happens to re-find it",
        );
        assert_eq!(
            delivered,
            vec![root.join("src/a.py").to_string_lossy().into_owned()],
            "and it must hand over the paths themselves, not a rescan"
        );

        drop(tx);
        thread.join().expect("the watch loop must stop cleanly");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The OFF direction: stopping an empty watcher delivers nothing.
    ///
    /// Without this, a shutdown flush that unconditionally called the callback
    /// would pass the test above while enqueueing an empty batch on every clean
    /// exit — and `run_loop`'s callback treats a non-empty batch as a reason to
    /// touch the activity clock, so an empty one is not merely wasteful.
    #[test]
    fn stopping_a_watcher_that_is_holding_nothing_delivers_nothing() {
        let root = scratch_root("shutdown-flush-empty");
        let (tx, rx) = sync_channel(WATCH_QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let (stop_tx, stop_rx) = channel();
        let (batches_tx, batches_rx) = channel();
        let loop_root = root.clone();
        let loop_flag = Arc::clone(&overflowed);
        let thread = std::thread::spawn(move || {
            run_watch_loop(
                &loop_root,
                rx,
                stop_rx,
                &loop_flag,
                Duration::from_secs(30),
                move |paths| {
                    let _ = batches_tx.send(paths);
                },
            );
        });

        std::thread::sleep(Duration::from_millis(400));
        let _ = stop_tx.send(());
        drop(tx);
        thread.join().expect("the watch loop must stop cleanly");
        assert!(
            batches_rx.try_recv().is_err(),
            "a watcher holding nothing must deliver nothing on the way out"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The OFF direction: a queue with room never sets the flag, and the loop
    /// delivers the itemised paths rather than a rescan.
    #[test]
    fn an_unrefused_queue_delivers_itemised_paths() {
        let root = scratch_root("no-overflow");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();

        let (tx, rx) = sync_channel(WATCH_QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        offer_watch_event(
            &tx,
            &overflowed,
            Ok(
                notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Any))
                    .add_path(root.join("src/a.py")),
            ),
        );
        assert!(!overflowed.load(Ordering::Relaxed));

        let (stop_tx, stop_rx) = channel();
        let (batches_tx, batches_rx) = channel();
        let loop_root = root.clone();
        let loop_flag = Arc::clone(&overflowed);
        let thread = std::thread::spawn(move || {
            run_watch_loop(
                &loop_root,
                rx,
                stop_rx,
                &loop_flag,
                Duration::ZERO,
                move |paths| {
                    let _ = batches_tx.send(paths);
                },
            );
        });

        let first = batches_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the loop must deliver the edit");
        assert_eq!(
            first,
            vec![root.join("src/a.py").to_string_lossy().into_owned()],
            "one edit is one edit, not a reason to re-index the tree"
        );

        let _ = stop_tx.send(());
        drop(tx);
        thread.join().expect("the watch loop must stop cleanly");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A batch whose every path was filtered out must not move the debounce
    /// window.
    ///
    /// The watcher filters each raw event through ignore rules before pushing
    /// it, and a busy tree produces far more filtered-out events than real
    /// ones — gitignored build output, editor churn, dependency trees. `push`
    /// stamped `last_event` even for an empty batch, so continuous noise reset
    /// the silence timer forever and a real edit behind the noise was never
    /// flushed: the watcher looked alive while indexing nothing until the
    /// noise happened to pause longer than the debounce.
    #[test]
    fn empty_batches_must_not_extend_the_debounce_window() {
        let start = Instant::now();
        let mut buffer = DebounceBuffer::new(
            Duration::from_secs(2),
            whole_tree_rescan(std::path::Path::new("/tmp/devmap-debounce-fixture")),
        );
        buffer.push(["real.py"], start);

        // Ten seconds of fully-filtered event batches, 200 ms apart. Each one
        // used to re-stamp `last_event`, pushing eligibility ten seconds out.
        for step in 1..=50usize {
            let empty: Vec<String> = Vec::new();
            buffer.push(empty, start + Duration::from_millis(200 * step as u64));
        }

        assert_eq!(
            buffer.take_ready(start + Duration::from_secs(2)).unwrap(),
            ["real.py".to_string()],
            "empty batches must not extend the debounce window; \
             filtered noise must not starve real edits"
        );
    }

    /// End to end: sustained gitignored churn must not delay delivery of a
    /// real source edit past the debounce window, and shutdown must still be
    /// observed while the churn is running.
    ///
    /// This is the regression form of the starvation above: the noise keeps
    /// flowing for the whole test, so the old flush path (evaluate readiness
    /// only when the event channel goes quiet) never got a chance to run even
    /// though the real edit had been quiet for longer than the debounce.
    #[test]
    fn sustained_filtered_noise_does_not_starve_a_real_edit_or_shutdown() {
        let root = std::env::temp_dir().join(format!(
            "devmap-noise-starve-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".gitignore"), "noise.txt\n").unwrap();
        std::fs::write(root.join("main.py"), "def first(): pass\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let handle = start_file_watcher(&root, move |paths| tx.send(paths).unwrap()).unwrap();

        let noise_root = root.clone();
        let stop_noise = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_flag = std::sync::Arc::clone(&stop_noise);
        let noise_thread = std::thread::spawn(move || {
            let mut round = 0u32;
            // ~9 s of continuous ignored-file churn, faster than the 250 ms
            // receive window: the event channel never goes quiet on its own.
            while !stop_flag.load(std::sync::atomic::Ordering::Relaxed) {
                round += 1;
                let _ = std::fs::write(noise_root.join("noise.txt"), format!("noise {round}\n"));
                std::thread::sleep(Duration::from_millis(100));
            }
        });

        // The real edit lands shortly after the noise starts, then stays
        // quiet far longer than the debounce.
        std::thread::sleep(Duration::from_millis(300));
        let edit_at = Instant::now();
        std::fs::write(root.join("main.py"), "def second(): pass\n").unwrap();

        let deadline = edit_at + Duration::from_secs(6);
        let mut delivered = false;
        while Instant::now() < deadline {
            if let Ok(paths) = rx.recv_timeout(Duration::from_millis(250)) {
                if paths.iter().any(|path| path.ends_with("main.py")) {
                    delivered = true;
                    break;
                }
            }
        }
        assert!(
            delivered,
            "a real edit must flush within the debounce window even while \
             filtered noise keeps arriving"
        );

        // Shutdown must be observed promptly even though the noise is still
        // running: the stop flag is checked every loop iteration.
        handle.shutdown().expect("clean shutdown under churn");

        stop_noise.store(true, std::sync::atomic::Ordering::Relaxed);
        noise_thread.join().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn debounce_buffer_waits_deduplicates_and_sorts() {
        let start = Instant::now();
        let mut buffer = DebounceBuffer::new(
            Duration::from_secs(2),
            whole_tree_rescan(std::path::Path::new("/tmp/devmap-debounce-fixture")),
        );
        buffer.push(["z.py", "a.py", "z.py"], start);

        assert!(buffer
            .take_ready(start + Duration::from_millis(1_999))
            .is_none());
        assert_eq!(
            buffer.take_ready(start + Duration::from_secs(2)).unwrap(),
            ["a.py", "z.py"]
        );
        assert!(buffer.take_ready(start + Duration::from_secs(10)).is_none());
    }

    /// A continuously churning tree never goes quiet for `debounce`, so a
    /// flush that only fires on silence starved forever: build systems and
    /// autosave loops kept resetting `last_event` and the buffered paths were
    /// never delivered. The hold cap forces delivery once the oldest pending
    /// path has waited long enough — and resets cleanly afterwards.
    #[test]
    fn a_batch_held_past_the_cap_flushes_even_without_quiet() {
        let start = Instant::now();
        let mut buffer = DebounceBuffer::new(
            Duration::from_secs(2),
            whole_tree_rescan(std::path::Path::new("/tmp/devmap-debounce-fixture")),
        );

        // Events arriving continuously: every one inside the quiet window.
        buffer.push(["a.py"], start);
        for step in 1..=6 {
            buffer.push(
                [format!("a{step}.py")],
                start + Duration::from_millis(500 * step),
            );
            assert!(
                buffer
                    .take_ready(start + Duration::from_millis(500 * step))
                    .is_none(),
                "batch must stay debounced while events keep arriving"
            );
        }

        // Past MAX_DEBOUNCE_HOLD from the first push, the batch flushes even
        // though events never stopped.
        let flushed = buffer.take_ready(start + MAX_DEBOUNCE_HOLD).unwrap();
        assert!(flushed.contains(&"a.py".to_string()));
        assert_eq!(flushed.len(), 7);
        assert!(
            flushed.iter().any(|path| path == "a6.py"),
            "paths pushed after the cap elapsed but before the flush must not be dropped"
        );

        // After a cap flush the state is reset: nothing more until new work.
        assert!(buffer.take_ready(start + MAX_DEBOUNCE_HOLD).is_none());
    }

    #[test]
    fn watcher_handle_has_explicit_shutdown() {
        let root = std::env::temp_dir().join(format!(
            "devmap-watcher-handle-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let handle = start_file_watcher(&root, |_| {}).unwrap();
        handle.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn test_s12_ignore_cache_reuses_rules_and_invalidates_on_rule_change() {
        let root = std::env::temp_dir().join(format!(
            "devmap-ignore-cache-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.py\n").unwrap();
        let ignored = root.join("ignored.py");
        std::fs::write(&ignored, "def ignored(): pass\n").unwrap();

        let mut cache = IgnoreVerdictCache::default();
        assert!(cache.is_ignored(&root, &ignored, false).unwrap());
        assert!(cache.is_ignored(&root, &ignored, false).unwrap());
        assert_eq!(cache.rebuild_count(), 1, "unchanged rules must be reused");

        let ignore_path = root.join(".gitignore");
        std::fs::write(&ignore_path, "another.py\n").unwrap();
        assert!(cache.observe_rule_event(&ignore_path));
        assert!(!cache.is_ignored(&root, &ignored, false).unwrap());
        assert_eq!(
            cache.rebuild_count(),
            2,
            "rule events must invalidate the cached verdict"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    /// Ignore-rule stamps must reflect the rule files on disk.
    ///
    /// `rule_stamps` was replaceable with `Ok(vec![])` and `stamp_rule`'s
    /// file/NotFound guards were both mutable, undetected. The stamps are the
    /// entire cache-invalidation signal: constant-empty stamps compare equal
    /// forever, so an edited `.gitignore` never invalidates a cached verdict
    /// and the watcher keeps applying rules the user has already changed.
    #[test]
    fn ignore_rule_stamps_track_the_rules_on_disk() {
        let root = std::env::temp_dir().join(format!(
            "devmap-stamps-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let rules = root.join(".gitignore");
        std::fs::write(&rules, "build/\n").unwrap();
        let probe = root.join("src.py");
        std::fs::write(&probe, "x\n").unwrap();

        let before = IgnoreVerdictCache::rule_stamps(&root, &probe, false).unwrap();
        assert!(
            !before.is_empty(),
            "an existing .gitignore must produce a stamp; empty stamps can never \
             invalidate a cached verdict"
        );
        assert!(
            before.iter().any(|stamp| stamp.len.is_some()),
            "a real rule file must stamp its length, not read as absent: {before:?}"
        );

        // Editing the rules must change the stamp, or the cache is stuck.
        std::thread::sleep(Duration::from_millis(10));
        std::fs::write(&rules, "build/\ndist/\nmore/\n").unwrap();
        let after = IgnoreVerdictCache::rule_stamps(&root, &probe, false).unwrap();
        assert_ne!(
            before, after,
            "editing a rule file must change its stamp so the cached verdict is rebuilt"
        );

        // A missing rule file stamps as absent rather than erroring, so the
        // walk can still proceed.
        std::fs::remove_file(&rules).unwrap();
        let removed = IgnoreVerdictCache::rule_stamps(&root, &probe, false).unwrap();
        assert!(
            removed.iter().all(|stamp| stamp.len.is_none()),
            "a deleted rule file must stamp as absent: {removed:?}"
        );
        assert_ne!(
            after, removed,
            "deleting a rule file must invalidate the cache"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn watcher_never_reports_its_own_database_files() {
        let root = std::env::temp_dir().join(format!(
            "devmap-watcher-filter-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".devcouncil/codeintel")).unwrap();
        std::fs::write(root.join(".gitignore"), "ignored.py\n").unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = start_file_watcher(&root, move |paths| tx.send(paths).unwrap()).unwrap();

        // Write once per round, then stay QUIET long enough for the debounce to
        // settle.
        //
        // Two failure modes have to be avoided at once, and they pull in
        // opposite directions. `start_file_watcher` returning does not
        // guarantee the platform watch is registered, so a single write up
        // front can land in that gap and be missed outright — the observed
        // failure was an empty result set, not a slow one, and no amount of
        // waiting recovers an event that was never delivered. But the watcher
        // only emits after `DEBOUNCE_SETTLE` of silence, so a loop that keeps
        // rewriting to compensate resets `last_event` on every pass and the
        // buffer never flushes: continuous retries starve the very callback
        // they are trying to trigger. Hence write, then wait out more than the
        // debounce before writing again. All three files are rewritten together
        // so the ignore assertions below stay under test on every round.
        const DEBOUNCE_SETTLE: Duration = Duration::from_secs(2);
        let quiet_window = DEBOUNCE_SETTLE * 3;
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut observed = Vec::new();
        let mut round = 0;
        let saw_main = |observed: &Vec<String>| {
            observed
                .iter()
                .any(|path: &String| path.ends_with("main.py"))
        };

        while Instant::now() < deadline && !saw_main(&observed) {
            round += 1;
            std::fs::write(
                root.join("main.py"),
                format!("def changed{round}(): pass\n"),
            )
            .unwrap();
            std::fs::write(
                root.join("ignored.py"),
                format!("def ignored{round}(): pass\n"),
            )
            .unwrap();
            std::fs::write(
                root.join(".devcouncil/codeintel/index.sqlite-wal"),
                format!("noise{round}").as_bytes(),
            )
            .unwrap();

            let quiet_until = Instant::now() + quiet_window;
            while Instant::now() < quiet_until && !saw_main(&observed) {
                if let Ok(paths) = rx.recv_timeout(Duration::from_millis(250)) {
                    observed.extend(paths);
                }
            }
        }
        handle.shutdown().unwrap();
        assert!(
            observed.iter().any(|path| path.ends_with("main.py")),
            "source change was not observed: {observed:?}"
        );
        assert!(
            observed.iter().all(|path| {
                !devmap_extract::paths::STATE_DIR_NAMES
                    .iter()
                    .any(|dir| path.contains(dir))
            }),
            "internal database change escaped watcher filter: {observed:?}"
        );
        assert!(
            observed.iter().all(|path| !path.ends_with("ignored.py")),
            "gitignored source escaped watcher filter: {observed:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// K-B2: the debounce buffer must be bounded, and bounded by collapsing
    /// rather than dropping.
    ///
    /// `pending` was an uncapped `BTreeSet<String>` held for up to
    /// `MAX_DEBOUNCE_HOLD` (10 s). A `git checkout` across a large tree grows it
    /// with every changed path for that whole window, bounded only by how fast
    /// the tree changes — the watcher's memory is then a function of the user's
    /// git history, which is not a bound at all.
    ///
    /// What makes the cap safe is that it collapses to the root entry the drain
    /// already expands into a whole-tree rescan. That covers every path the
    /// buffer was holding *and* anything it had not been told about yet, so the
    /// only thing lost is the itemisation. Discarding paths instead would drop
    /// real edits with nothing recording the loss.
    #[test]
    fn a_flood_of_paths_collapses_to_a_rescan_instead_of_growing() {
        let root = std::path::Path::new("/tmp/devmap-debounce-flood");
        let sentinel = whole_tree_rescan(root);
        let start = Instant::now();
        let mut buffer = DebounceBuffer::new(Duration::from_secs(2), sentinel.clone());

        for index in 0..(MAX_DEBOUNCE_PATHS + 500) {
            buffer.push([format!("src/file_{index}.py")], start);
        }

        let ready = buffer
            .take_ready(start + Duration::from_secs(3))
            .expect("a matured batch must flush");
        assert!(
            ready.len() <= MAX_DEBOUNCE_PATHS,
            "the buffer held {} paths; the cap is {MAX_DEBOUNCE_PATHS}, and an \
             unbounded set here is bounded only by how fast the tree changes",
            ready.len()
        );
        assert_eq!(
            ready, sentinel,
            "past the cap the buffer must carry the whole-tree rescan entry, which \
             covers every path it was holding plus whatever it had not yet seen. \
             Anything else here is a dropped edit."
        );
    }

    /// The OFF direction: an ordinary batch is still itemised.
    ///
    /// A buffer that answered "rescan the world" to every batch would satisfy
    /// the test above while making every single-file edit re-walk the tree.
    #[test]
    fn an_ordinary_batch_is_not_collapsed() {
        let root = std::path::Path::new("/tmp/devmap-debounce-ordinary");
        let start = Instant::now();
        let mut buffer = DebounceBuffer::new(Duration::from_secs(2), whole_tree_rescan(root));
        buffer.push(["src/a.py", "src/b.py"], start);

        let ready = buffer
            .take_ready(start + Duration::from_secs(3))
            .expect("a matured batch must flush");
        assert_eq!(
            ready,
            vec!["src/a.py".to_string(), "src/b.py".to_string()],
            "two edits are two edits, not a reason to re-index everything"
        );
    }
}

#[cfg(test)]
mod git_head_tests {
    use super::*;

    fn is_git_ref_event(root: &Path, path: &Path) -> bool {
        super::is_git_ref_event(root, path).expect("fixture metadata is readable")
    }

    fn root() -> std::path::PathBuf {
        std::path::PathBuf::from("/repo")
    }

    /// A moved checkout is visible to the watcher. (B5)
    ///
    /// `.git/` is pruned by `is_ignored_path`, which is right for indexing and
    /// wrong for noticing: a commit, branch switch, rebase or stash changes
    /// what the index should contain while touching no watched file, so before
    /// this the daemon never woke at all.
    #[test]
    fn git_ref_movement_is_observed() {
        for path in [
            ".git/HEAD",
            ".git/packed-refs",
            ".git/refs/heads/main",
            ".git/refs/remotes/origin/main",
            ".git/refs/tags/v1.0.0",
        ] {
            assert!(
                is_git_ref_event(&root(), &root().join(path)),
                "{path} defines which commit is checked out and must be observed"
            );
        }
    }

    /// The observation is narrow on purpose.
    ///
    /// `.git/` churns constantly — object writes, index updates, lock files.
    /// Admitting it wholesale would wake the daemon on every `git status`, and
    /// a watcher that fires continuously is one an operator turns off.
    #[test]
    fn ordinary_git_churn_is_not_observed() {
        for (path, why) in [
            (
                ".git/index",
                "changes on `git add` with no working-tree change",
            ),
            (".git/objects/ab/cdef", "object writes are not ref movement"),
            (".git/COMMIT_EDITMSG", "an editor buffer, not a ref"),
            (".git/logs/HEAD", "the reflog trails the ref it records"),
            (".git/config", "configuration is not a checkout"),
        ] {
            assert!(
                !is_git_ref_event(&root(), &root().join(path)),
                "{path} must not wake the daemon: {why}"
            );
        }
    }

    /// Git's write-in-progress files must not double-wake.
    ///
    /// Every ref update writes `X.lock` then renames it onto `X`. Admitting
    /// both means two resyncs per commit, the first against a half-written ref.
    #[test]
    fn lock_files_are_not_observed() {
        for path in [".git/HEAD.lock", ".git/refs/heads/main.lock"] {
            assert!(
                !is_git_ref_event(&root(), &root().join(path)),
                "{path} is git's write-in-progress file; the real file follows"
            );
        }
    }

    /// A path outside the watched root is never a ref event, whatever it looks
    /// like. A nested checkout's `.git` is not this repository's.
    #[test]
    fn a_foreign_git_directory_is_not_observed() {
        assert!(!is_git_ref_event(
            &root(),
            std::path::Path::new("/elsewhere/.git/HEAD")
        ));
    }

    /// Ordinary source files are unaffected by the carve-out.
    #[test]
    fn source_paths_are_not_mistaken_for_ref_events() {
        for path in ["src/main.rs", "docs/.gitignore", "vendor/git/HEAD.rs"] {
            assert!(!is_git_ref_event(&root(), &root().join(path)), "{path}");
        }
    }
}
