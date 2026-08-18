use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use devmap_analyze::analyze;
use devmap_extract::{
    collect_go_modules, collect_sources_with_report, content_hash, extract_file,
    is_indexable_source, DiscoverySkipReason, MAX_SOURCE_BYTES,
};
use devmap_resolve::Resolver;
use devmap_store::{current_git_head, GenerationWriteOpts, Store, GENERATION_RETENTION};

use crate::watcher::start_file_watcher;

const STABLE_READ_ATTEMPTS: usize = 3;

struct AbortTaskOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortTaskOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn read_stable_source(path: &std::path::Path, relative: &str) -> anyhow::Result<String> {
    read_stable_source_with(path, relative, || std::fs::read_to_string(path))
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
        let source = read()
            .map_err(|error| anyhow::anyhow!("cannot read changed source {relative:?}: {error}"))?;
        let after = std::fs::metadata(path)?;
        if before.len() == after.len()
            && before.modified().ok() == after.modified().ok()
            && u64::try_from(source.len()).is_ok_and(|length| length == after.len())
        {
            return Ok(source);
        }
    }
    anyhow::bail!(
        "changed source {relative:?} did not stabilize across {STABLE_READ_ATTEMPTS} stat-read-stat attempts"
    )
}

#[derive(Clone)]
pub struct Daemon {
    store: Arc<Store>,
    root: std::path::PathBuf,
    batch_limit: usize,
    idle_poll: Duration,
    ipc_path: std::path::PathBuf,
}

#[derive(Default)]
struct PendingDelta {
    affected: std::collections::BTreeSet<String>,
    deleted: std::collections::BTreeSet<String>,
    fresh: Vec<devmap_extract::Extraction>,
}

impl Daemon {
    pub fn new(store: Store, root: std::path::PathBuf) -> Self {
        let ipc_path = default_ipc_path_for(&root);
        Self {
            store: Arc::new(store),
            root,
            batch_limit: 64,
            idle_poll: Duration::from_secs(2),
            ipc_path,
        }
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
                    // Preserve failure as durable work. The supervised drain
                    // loop will retry with backoff and quarantine the path;
                    // refusing to bind IPC here would make status unavailable.
                    pending.insert(path);
                }
                DiscoverySkipReason::NonUtf8Path => {
                    anyhow::bail!(
                        "connect-time sweep cannot represent non-UTF-8 source path {path:?}"
                    );
                }
            }
        }
        let pending: Vec<_> = pending.into_iter().collect();
        self.store.enqueue_pending_paths(&pending)?;
        Ok(pending.len())
    }

    fn collect_pending_path(
        &self,
        root: &std::path::Path,
        previous: &[devmap_extract::Extraction],
        pending: &str,
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
            if let Some((path, reason)) = discovery
                .skipped_paths
                .iter()
                .find(|(_, reason)| !matches!(reason, DiscoverySkipReason::NonSource))
            {
                anyhow::bail!(
                    "changed directory contains an unindexable source {path:?}: {reason:?}"
                );
            }
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
                {
                    delta.affected.insert(extraction.file_path.clone());
                    delta.deleted.insert(extraction.file_path.clone());
                }
            }
            return Ok(delta);
        }

        let relative = stored_path(root, &candidate)?;
        delta.affected.insert(relative.clone());
        if candidate.exists() {
            let canonical = candidate.canonicalize()?;
            if !canonical.starts_with(root) {
                anyhow::bail!("watched file resolves outside daemon root: {canonical:?}");
            }
            if !canonical.is_file() || !is_indexable_source(&relative) {
                delta.deleted.insert(relative);
                return Ok(delta);
            }
            let source = read_stable_source(&canonical, &relative)?;
            delta.fresh.push(extract_file(&relative, &source));
        } else {
            delta.deleted.insert(relative);
        }
        Ok(delta)
    }

    /// Process up to `batch_limit` claimed paths. Only claimed files are read
    /// from disk; unchanged extraction payloads come from the durable generation.
    /// Attempts are recorded before work starts, and paths are acknowledged only
    /// after extraction, resolution, analysis, and persistence all succeed.
    pub fn drain_pending_batch(&self) -> anyhow::Result<usize> {
        let batch = self.store.get_pending_paths_limited(self.batch_limit)?;
        if batch.is_empty() {
            return Ok(0);
        }
        // Timed from the moment real work starts, so the history row reflects
        // incremental resync cost rather than idle polling.
        let resync_started = std::time::Instant::now();
        self.store.bump_pending_attempts(&batch)?;

        let root = self.root.canonicalize().map_err(|error| {
            anyhow::anyhow!("cannot canonicalize daemon root {:?}: {error}", self.root)
        })?;
        let previous = self.store.latest_extractions()?;
        let mut affected = std::collections::BTreeSet::new();
        let mut deleted = std::collections::BTreeSet::new();
        let mut fresh = Vec::new();

        let mut succeeded = Vec::new();
        let mut failures = Vec::new();
        for pending in &batch {
            match self.collect_pending_path(&root, &previous, pending) {
                Ok(delta) => {
                    affected.extend(delta.affected);
                    deleted.extend(delta.deleted);
                    fresh.extend(delta.fresh);
                    succeeded.push(pending.clone());
                }
                Err(error) => {
                    warn!("pending path {pending:?} failed in isolation: {error}");
                    failures.push(format!("{pending}: {error}"));
                }
            }
        }

        if succeeded.is_empty() {
            anyhow::bail!("every pending path failed: {}", failures.join("; "));
        }

        let mut extractions: Vec<_> = previous
            .into_iter()
            .filter(|extraction| !affected.contains(&extraction.file_path))
            .collect();
        extractions.extend(fresh.iter().cloned());
        extractions.sort_by(|left, right| left.file_path.cmp(&right.file_path));
        let mut resolver = Resolver::new();
        match collect_go_modules(&self.root) {
            Ok(modules) => resolver.index_go_modules(&modules),
            Err(error) => warn!("go.mod collection failed for {:?}: {error}", self.root),
        }
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = analyze(&extractions, &resolution);
        let head_sha = current_git_head(&self.root).unwrap_or_else(|_| "unavailable".to_string());
        self.store.save_generation_with_metadata(
            &fresh,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                affected_paths: affected.into_iter().collect(),
                deleted_paths: deleted.into_iter().collect(),
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
        // or any failed stage above leaves the bumped work queued for replay.
        self.store.clear_pending_paths_after_attempt(&succeeded)?;
        Ok(succeeded.len())
    }

    /// Long-lived loop: file watcher enqueues durable pending paths; idle poll drains them.
    /// Does not return until cancelled / fatal error.
    pub async fn run_loop(&self) -> anyhow::Result<()> {
        info!(
            "DevMap daemon started for {:?} (batch_limit={})",
            self.root, self.batch_limit
        );

        let store = Arc::clone(&self.store);
        let root = self.root.clone();
        let _watcher = start_file_watcher(root, move |paths| {
            if paths.is_empty() {
                return;
            }
            if let Err(err) = store.enqueue_pending_paths(&paths) {
                warn!("failed to enqueue pending paths: {err}");
            } else {
                info!("enqueued {} changed path(s)", paths.len());
            }
        })?;

        let reconciled = self.reconcile_connect_time()?;
        if reconciled > 0 {
            info!("connect-time sweep enqueued {reconciled} changed path(s)");
        }

        #[cfg(unix)]
        let mut ipc_task = AbortTaskOnDrop({
            let server = crate::protocol::UnixIpcServer::bind(&self.ipc_path)?;
            let store = Arc::clone(&self.store);
            tokio::spawn(server.run(store))
        });

        #[cfg(windows)]
        let mut ipc_task = AbortTaskOnDrop({
            let store = Arc::clone(&self.store);
            let name = self.ipc_path.to_string_lossy().into_owned();
            tokio::spawn(async move { crate::protocol::run_named_pipe(store, &name).await })
        });

        let store = Arc::clone(&self.store);
        let _maintenance_task = AbortTaskOnDrop(tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                match store.checkpoint_wal() {
                    Ok(result) if result.busy != 0 => warn!(
                        "WAL checkpoint remained busy after {:?} fallback: {}/{} frames checkpointed",
                        result.mode, result.checkpointed_frames, result.log_frames
                    ),
                    Ok(_) => {}
                    Err(err) => warn!("WAL checkpoint failed: {err}"),
                }
                if let Err(err) = store.vacuum_if_needed() {
                    warn!("vacuum_if_needed failed: {err}");
                }
            }
        }));

        let mut ticker = tokio::time::interval(self.idle_poll);
        let mut consecutive_failures = 0u32;
        let mut next_attempt = tokio::time::Instant::now();
        loop {
            tokio::select! {
                result = &mut ipc_task.0 => {
                    return result
                        .map_err(|error| anyhow::anyhow!("IPC task join failed: {error}"))?;
                }
                _ = ticker.tick() => {
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
        }
    }
}

#[cfg(unix)]
pub fn default_ipc_path_for(root: &std::path::Path) -> std::path::PathBuf {
    let identity = devmap_extract::content_hash(&root.to_string_lossy());
    std::env::temp_dir()
        .join(format!("devmap-{identity:016x}"))
        .join("ipc.sock")
}

#[cfg(windows)]
pub fn default_ipc_path_for(root: &std::path::Path) -> std::path::PathBuf {
    let identity = devmap_extract::content_hash(&root.to_string_lossy());
    format!(r"\\.\pipe\devmap-{identity:016x}").into()
}

fn stored_path(root: &std::path::Path, candidate: &std::path::Path) -> anyhow::Result<String> {
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| anyhow::anyhow!("path {:?} is outside daemon root {:?}", candidate, root))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        store
            .save_generation(&initial, &resolution, &analysis)
            .unwrap();

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
        store
            .save_generation(&initial, &resolution, &analysis)
            .unwrap();

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
        store
            .save_generation(&initial, &resolution, &analysis)
            .unwrap();
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
}
