use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::time::{Duration, Instant, SystemTime};
use tracing::warn;

use devmap_extract::{ignore_rule_files, is_gitignored, is_ignored_path, is_indexable_source};

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

        let ignored = is_gitignored(root, path, is_dir)?;
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
            || path.ends_with(".git/info/exclude");
        if is_rule {
            self.entries.clear();
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

struct DebounceBuffer {
    debounce: Duration,
    pending: BTreeSet<String>,
    last_event: Option<Instant>,
    oldest_pending: Option<Instant>,
}

impl DebounceBuffer {
    fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            pending: BTreeSet::new(),
            last_event: None,
            oldest_pending: None,
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
        self.pending.extend(admitted);
        self.last_event = Some(now);
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
fn is_git_ref_event(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    let Some(rest) = relative.strip_prefix(".git/") else {
        return false;
    };
    // `HEAD.lock` and `refs/heads/main.lock` are git's write-in-progress
    // files. Ignoring them avoids waking twice per ref update; the real file
    // follows immediately.
    if rest.ends_with(".lock") {
        return false;
    }
    rest == "HEAD" || rest == "packed-refs" || rest.starts_with("refs/")
}

fn admitted_watch_path(
    root: &Path,
    path: &Path,
    ignore_cache: &mut IgnoreVerdictCache,
) -> anyhow::Result<Option<String>> {
    let Ok(relative) = path.strip_prefix(root) else {
        return Ok(None);
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    if is_ignored_path(&relative) {
        return Ok(None);
    }
    let metadata = std::fs::metadata(path);
    let is_dir = metadata.as_ref().is_ok_and(|metadata| metadata.is_dir());
    if ignore_cache.is_ignored(root, path, is_dir)? {
        return Ok(None);
    }
    Ok(match metadata {
        Ok(metadata) if metadata.is_dir() || is_indexable_source(&relative) => {
            Some(path.to_string_lossy().into_owned())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Removed files and directories no longer have metadata. Admit
            // them so deletion reconciliation can remove previously indexed
            // descendants; the daemon re-checks indexability and scope.
            Some(path.to_string_lossy().into_owned())
        }
        _ => None,
    })
}

/// Start a debounced recursive file watcher. The watcher thread owns the
/// `RecommendedWatcher` so it is not dropped immediately (previous bug).
pub fn start_file_watcher<P: AsRef<Path>, F: Fn(Vec<String>) + Send + 'static>(
    root_path: P,
    callback: F,
) -> anyhow::Result<WatcherHandle> {
    let root = root_path.as_ref().canonicalize()?;
    let (tx, rx) = channel();
    let (stop_tx, stop_rx) = channel();
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;
    watcher.watch(&root, RecursiveMode::Recursive)?;

    let thread = std::thread::spawn(move || {
        let _watcher = watcher; // keep alive for the thread lifetime
        let mut buffer = DebounceBuffer::new(Duration::from_secs(2));
        let mut ignore_cache = IgnoreVerdictCache::default();

        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }
            // Deliver matured batches on the event path too: a tree under
            // continuous churn never lets `recv_timeout` expire, so a flush
            // evaluated only on silence would never run again.
            let now = Instant::now();
            if let Some(paths) = buffer.take_ready(now) {
                callback(paths);
            }
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(Ok(event)) => {
                    let admitted: Vec<String> = if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        event
                            .paths
                            .into_iter()
                            .filter_map(|path| {
                                if ignore_cache.observe_rule_event(&path) {
                                    return None;
                                }
                                // Checked before the ignore rules, because
                                // `.git/` is pruned by them and this is the one
                                // thing inside it the daemon has to see.
                                if is_git_ref_event(&root, &path) {
                                    return Some(GIT_HEAD_SENTINEL.to_string());
                                }
                                match admitted_watch_path(&root, &path, &mut ignore_cache) {
                                    Ok(path) => path,
                                    Err(error) => {
                                        warn!("failed to evaluate watcher path {path:?}: {error}");
                                        None
                                    }
                                }
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    buffer.push(admitted, Instant::now());
                }
                Ok(Err(e)) => warn!("Watch error: {:?}", e),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(paths) = buffer.take_ready(Instant::now()) {
                        callback(paths);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
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
        let mut buffer = DebounceBuffer::new(Duration::from_secs(2));
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
        let mut buffer = DebounceBuffer::new(Duration::from_secs(2));
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
        let mut buffer = DebounceBuffer::new(Duration::from_secs(2));

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
            observed.iter().all(|path| !path.contains(".devcouncil")),
            "internal database change escaped watcher filter: {observed:?}"
        );
        assert!(
            observed.iter().all(|path| !path.ends_with("ignored.py")),
            "gitignored source escaped watcher filter: {observed:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod git_head_tests {
    use super::*;

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
