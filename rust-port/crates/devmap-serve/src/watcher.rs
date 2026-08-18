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

struct DebounceBuffer {
    debounce: Duration,
    pending: BTreeSet<String>,
    last_event: Option<Instant>,
}

impl DebounceBuffer {
    fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            pending: BTreeSet::new(),
            last_event: None,
        }
    }

    fn push<I, S>(&mut self, paths: I, now: Instant)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.pending.extend(paths.into_iter().map(Into::into));
        self.last_event = Some(now);
    }

    fn take_ready(&mut self, now: Instant) -> Option<Vec<String>> {
        let last_event = self.last_event?;
        if self.pending.is_empty() || now.saturating_duration_since(last_event) < self.debounce {
            return None;
        }
        self.last_event = None;
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
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(Ok(event)) => {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        buffer.push(
                            event.paths.into_iter().filter_map(|path| {
                                if ignore_cache.observe_rule_event(&path) {
                                    return None;
                                }
                                match admitted_watch_path(&root, &path, &mut ignore_cache) {
                                    Ok(path) => path,
                                    Err(error) => {
                                        warn!("failed to evaluate watcher path {path:?}: {error}");
                                        None
                                    }
                                }
                            }),
                            Instant::now(),
                        );
                    }
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
