//! Stateful model checker for `StackedStore`, `DeltaStore`,
//! and `WorktreeOverlay`.
//!
//! Drives multi-lane stacked worktree lifecycles (donor base publication,
//! lane provisioning, file create/edit/delete/rename/revert, single-file &
//! full-worktree delta sync, donor rebuilds, base cache GC, lane reopen &
//! materialization) and checks differential parity against a cold flat
//! `StoreQueryEngine` plus structural invariants (unsynced path tracking,
//! immutable `0444` base snapshots, GC isolation, and honest lower-bound
//! disclosure).
//!
//! Tune campaigns with:
//! - `DEVMAP_STATEFUL_SEED=<u64>` (decimal or `0x` hex)
//! - `DEVMAP_STATEFUL_ROUNDS=<usize>`

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use devmap_extract::paths::{
    registered_common_git_dir, stack_delta_path, stack_lease_path, store_path,
};
use devmap_query::stacked::sha256_file_hex;
use devmap_query::{
    gc_unused_bases, provision_stacked_worktree, publish_base_snapshot, Budget, Cancel, Request,
    StackedStore, StoreQueryEngine,
};
use devmap_store::{GenerationWriteOpts, Store};

#[allow(dead_code)]
mod stateful_fuzz {
    include!("../../testsupport/stateful_fuzz.rs");
}

use stateful_fuzz::{
    bail, check, env_u64, env_usize, format_repro_test, generate_micro_repo, minimize_sequence,
    mutate_file_elements, skip_missing, CheckFailure, HealthTracker, MicroLang, MicroRepo, Source,
    StepOutcome,
};

const DEFAULT_SEED: u64 = 0x57A0_C3D1_2026_0004;
const DEFAULT_ROUNDS: usize = 2;
const STEPS_PER_ROUND: usize = 22;
const MAX_LANES: usize = 2;

const ACT_BUILD_DONOR: &str = "BuildDonorAndPublishBase";
const ACT_PROVISION_LANE: &str = "ProvisionLane";
const ACT_LANE_CREATE: &str = "LaneCreateFile";
const ACT_LANE_EDIT: &str = "LaneEditFile";
const ACT_LANE_DELETE: &str = "LaneDeleteFile";
const ACT_LANE_RENAME: &str = "LaneRenameFile";
const ACT_LANE_REVERT: &str = "LaneRevertToBase";
const ACT_SYNC_FILE: &str = "SyncSingleFile";
const ACT_SYNC_WORKTREE: &str = "SyncWorktree";
const ACT_DONOR_REBUILD: &str = "DonorRebuildAndPublish";
const ACT_GC_BASES: &str = "GcBaseCache";
const ACT_MATERIALIZE_REOPEN: &str = "MaterializeOrReopenLane";
const ACT_QUERY_PARITY: &str = "QueryAndCheckParity";

const ALL_ACTIONS: &[&str] = &[
    ACT_BUILD_DONOR,
    ACT_PROVISION_LANE,
    ACT_LANE_CREATE,
    ACT_LANE_EDIT,
    ACT_LANE_DELETE,
    ACT_LANE_RENAME,
    ACT_LANE_REVERT,
    ACT_SYNC_FILE,
    ACT_SYNC_WORKTREE,
    ACT_DONOR_REBUILD,
    ACT_GC_BASES,
    ACT_MATERIALIZE_REOPEN,
    ACT_QUERY_PARITY,
];

#[derive(Clone, Debug)]
enum Action {
    BuildDonorAndPublishBase,
    ProvisionLane {
        lane_idx: usize,
    },
    LaneCreateFile {
        lane_idx: usize,
        path: String,
        content: String,
    },
    LaneEditFile {
        lane_idx: usize,
        path: String,
        content: String,
    },
    LaneDeleteFile {
        lane_idx: usize,
        path: String,
    },
    LaneRenameFile {
        lane_idx: usize,
        from: String,
        to: String,
    },
    LaneRevertToBase {
        lane_idx: usize,
        path: String,
    },
    SyncSingleFile {
        lane_idx: usize,
        path: String,
    },
    SyncWorktree {
        lane_idx: usize,
    },
    DonorRebuildAndPublish {
        new_commit: bool,
        file_path: String,
        content: String,
    },
    GcBaseCache,
    MaterializeOrReopenLane {
        lane_idx: usize,
        materialize: bool,
    },
    QueryAndCheckParity {
        lane_idx: usize,
    },
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::BuildDonorAndPublishBase => ACT_BUILD_DONOR,
            Self::ProvisionLane { .. } => ACT_PROVISION_LANE,
            Self::LaneCreateFile { .. } => ACT_LANE_CREATE,
            Self::LaneEditFile { .. } => ACT_LANE_EDIT,
            Self::LaneDeleteFile { .. } => ACT_LANE_DELETE,
            Self::LaneRenameFile { .. } => ACT_LANE_RENAME,
            Self::LaneRevertToBase { .. } => ACT_LANE_REVERT,
            Self::SyncSingleFile { .. } => ACT_SYNC_FILE,
            Self::SyncWorktree { .. } => ACT_SYNC_WORKTREE,
            Self::DonorRebuildAndPublish { .. } => ACT_DONOR_REBUILD,
            Self::GcBaseCache => ACT_GC_BASES,
            Self::MaterializeOrReopenLane { .. } => ACT_MATERIALIZE_REOPEN,
            Self::QueryAndCheckParity { .. } => ACT_QUERY_PARITY,
        }
    }

    fn summary(&self) -> String {
        match self {
            Self::BuildDonorAndPublishBase => "BuildDonorAndPublishBase".to_string(),
            Self::ProvisionLane { lane_idx } => format!("ProvisionLane(lane={lane_idx})"),
            Self::LaneCreateFile {
                lane_idx,
                path,
                content,
            } => format!(
                "LaneCreateFile(lane={lane_idx}, path={path:?}, bytes={})",
                content.len()
            ),
            Self::LaneEditFile {
                lane_idx,
                path,
                content,
            } => format!(
                "LaneEditFile(lane={lane_idx}, path={path:?}, bytes={})",
                content.len()
            ),
            Self::LaneDeleteFile { lane_idx, path } => {
                format!("LaneDeleteFile(lane={lane_idx}, path={path:?})")
            }
            Self::LaneRenameFile { lane_idx, from, to } => {
                format!("LaneRenameFile(lane={lane_idx}, from={from:?}, to={to:?})")
            }
            Self::LaneRevertToBase { lane_idx, path } => {
                format!("LaneRevertToBase(lane={lane_idx}, path={path:?})")
            }
            Self::SyncSingleFile { lane_idx, path } => {
                format!("SyncSingleFile(lane={lane_idx}, path={path:?})")
            }
            Self::SyncWorktree { lane_idx } => format!("SyncWorktree(lane={lane_idx})"),
            Self::DonorRebuildAndPublish {
                new_commit,
                file_path,
                content,
            } => format!(
                "DonorRebuildAndPublish(new_commit={new_commit}, path={file_path:?}, bytes={})",
                content.len()
            ),
            Self::GcBaseCache => "GcBaseCache".to_string(),
            Self::MaterializeOrReopenLane {
                lane_idx,
                materialize,
            } => format!("MaterializeOrReopenLane(lane={lane_idx}, materialize={materialize})"),
            Self::QueryAndCheckParity { lane_idx } => {
                format!("QueryAndCheckParity(lane={lane_idx})")
            }
        }
    }
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "devmap-stateful-stacked-{tag}-{}-{id}",
            std::process::id()
        ));
        remove_readonly_tree(&path);
        fs::create_dir_all(&path).expect("create stateful test temp dir");
        Self { path }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        remove_readonly_tree(&self.path);
    }
}

fn make_tree_writable(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.is_dir() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
        }
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_tree_writable(&entry.path());
            }
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o644));
        }
        #[cfg(not(unix))]
        {
            if let Ok(mut perms) = fs::metadata(path).map(|m| m.permissions()) {
                perms.set_readonly(false);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
}

fn remove_readonly_tree(path: &Path) {
    if fs::symlink_metadata(path).is_err() {
        return;
    }
    make_tree_writable(path);
    let _ = fs::remove_dir_all(path);
}

fn run_git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "DevMapTest")
        .env("GIT_AUTHOR_EMAIL", "devmap-test@example.com")
        .env("GIT_COMMITTER_NAME", "DevMapTest")
        .env("GIT_COMMITTER_EMAIL", "devmap-test@example.com")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("git utf8")
        .trim()
        .to_string()
}

fn write_rel(root: &Path, rel: &str, content: &str) {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(&full, content).expect("write source file");
}

fn lang_for_path(path: &str) -> MicroLang {
    if path.ends_with(".ts") {
        MicroLang::TypeScript
    } else if path.ends_with(".go") {
        MicroLang::Go
    } else if path.ends_with(".py") {
        MicroLang::Python
    } else {
        MicroLang::Rust
    }
}

fn index_directory_into(
    repo_root: &Path,
    db_path: &Path,
    files: &BTreeMap<String, String>,
    head: &str,
) {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).expect("create store parent");
    }
    let store = Store::open(db_path).expect("open store");
    let canon = repo_root.canonicalize().expect("canonicalize root");
    store.bind_repo_root(&canon).expect("bind repo root");

    let exts: Vec<_> = files
        .iter()
        .map(|(rel, content)| devmap_extract::extract_file(rel, content))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&exts);
    let resolution = resolver.resolve_all(&exts).expect("resolve_all");
    let analysis = devmap_analyze::analyze(&exts, &resolution);

    store
        .save_generation_with_metadata(
            &exts,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
            head,
        )
        .expect("save generation");
    let _ = store.write_latest_csr_sidecar().expect("write csr sidecar");
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    path: PathBuf,
    sha256: String,
    len: u64,
    mtime_ns: u128,
}

impl FileStamp {
    fn capture(path: &Path) -> Result<Self, String> {
        let meta = fs::metadata(path).map_err(|e| format!("stat {}: {e}", path.display()))?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        let sha256 =
            sha256_file_hex(path).map_err(|e| format!("sha256 {}: {e:#}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            sha256,
            len: meta.len(),
            mtime_ns,
        })
    }

    fn verify_immutable(&self, action: &'static str) -> StepOutcome {
        let meta = match fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "base_immutable_missing",
                    action,
                    format!("base file {} vanished: {e}", self.path.display()),
                ));
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = meta.mode() & 0o7777;
            if mode & 0o222 != 0 {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "base_immutable_writable",
                    action,
                    format!(
                        "base file {} became writable (mode {mode:04o})",
                        self.path.display()
                    ),
                ));
            }
        }
        #[cfg(not(unix))]
        {
            if !meta.permissions().readonly() {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "base_immutable_writable",
                    action,
                    format!("base file {} lost readonly attribute", self.path.display()),
                ));
            }
        }
        let now = match Self::capture(&self.path) {
            Ok(s) => s,
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "base_immutable_stat",
                    action,
                    e,
                ));
            }
        };
        check(
            now == *self,
            "base_immutable_mutated",
            action,
            format!(
                "base snapshot {} mutated: before={self:?}, after={now:?}",
                self.path.display()
            ),
        )
    }
}

struct LaneState {
    root: PathBuf,
    stacked: bool,
    admitted_gen: u32,
    base_sqlite_stamp: Option<FileStamp>,
    base_csr_stamp: Option<FileStamp>,
    base_files: BTreeMap<String, String>,
    working_files: BTreeMap<String, String>,
    /// Tracks what has been synced into `delta.sqlite`:
    /// `Some(content)` = synced file contents, `None` = tombstoned in delta.
    synced_delta: BTreeMap<String, Option<String>>,
    /// Tracks symbols removed in this lane so we can also query deleted names.
    ever_seen_symbols: BTreeSet<String>,
    /// Every file path that has ever existed in this lane (base or created), so
    /// tombstone-leak checks distinguish real deleted files from synthetic graph
    /// hubs like `package:go/micro`.
    ever_seen_files: BTreeSet<String>,
}

impl LaneState {
    fn is_deleted_file(&self, path: &str) -> bool {
        self.ever_seen_files.contains(path) && !self.working_files.contains_key(path)
    }
    fn expected_unsynced(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let all_paths: BTreeSet<&String> = self
            .base_files
            .keys()
            .chain(self.working_files.keys())
            .chain(self.synced_delta.keys())
            .collect();
        for path in all_paths {
            let base = self.base_files.get(path);
            let work = self.working_files.get(path);
            let changed_vs_base = base != work;
            if changed_vs_base {
                let covered = match (work, self.synced_delta.get(path)) {
                    (Some(cur), Some(Some(synced))) => cur == synced,
                    (None, Some(None)) => true,
                    _ => false,
                };
                if !covered {
                    out.insert(path.clone());
                }
            } else if self.synced_delta.contains_key(path) {
                // Reverted to base (or created, synced, then deleted): delta holds a stale row.
                out.insert(path.clone());
            }
        }
        out
    }

    fn verify_base_immutable(&self, action: &'static str) -> StepOutcome {
        if !self.stacked {
            return StepOutcome::Ok;
        }
        if let Some(ref stamp) = self.base_sqlite_stamp {
            let out = stamp.verify_immutable(action);
            if !out.is_ok() {
                return out;
            }
        }
        if let Some(ref stamp) = self.base_csr_stamp {
            let out = stamp.verify_immutable(action);
            if !out.is_ok() {
                return out;
            }
        }
        StepOutcome::Ok
    }
}

struct World {
    _guard: TempDirGuard,
    root: PathBuf,
    donor: PathBuf,
    donor_files: BTreeMap<String, String>,
    donor_built: bool,
    donor_published_count: usize,
    lanes: Vec<LaneState>,
    cold_counter: u64,
}

impl World {
    fn new(tag: &str, initial_files: &MicroRepo) -> Self {
        let guard = TempDirGuard::new(tag);
        let root = guard.path.clone();
        let donor = root.join("donor");
        fs::create_dir_all(&donor).expect("create donor dir");
        fs::write(donor.join(".gitignore"), ".devmap/\n.devcouncil/\n").expect("write gitignore");

        for (rel, content) in initial_files {
            write_rel(&donor, rel, content);
        }

        run_git(&donor, &["init", "-b", "main"]);
        run_git(&donor, &["add", "."]);
        run_git(&donor, &["commit", "-m", "initial"]);

        Self {
            _guard: guard,
            root,
            donor,
            donor_files: initial_files.clone(),
            donor_built: false,
            donor_published_count: 0,
            lanes: Vec::new(),
            cold_counter: 0,
        }
    }

    fn donor_head(&self) -> String {
        run_git(&self.donor, &["rev-parse", "HEAD"])
    }

    fn build_donor_index(&mut self) {
        let db = store_path(&self.donor);
        let head = self.donor_head();
        index_directory_into(&self.donor, &db, &self.donor_files, &head);
        self.donor_built = true;
    }

    fn ensure_lane_worktree(&mut self, lane_idx: usize) {
        while self.lanes.len() <= lane_idx {
            let idx = self.lanes.len();
            let lane_root = self.root.join(format!("lane-{idx}"));
            let branch = format!("lane-{idx}");
            run_git(
                &self.donor,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    lane_root.to_str().expect("utf8 lane path"),
                ],
            );
            self.lanes.push(LaneState {
                root: lane_root,
                stacked: false,
                admitted_gen: 0,
                base_sqlite_stamp: None,
                base_csr_stamp: None,
                base_files: self.donor_files.clone(),
                working_files: self.donor_files.clone(),
                synced_delta: BTreeMap::new(),
                ever_seen_symbols: BTreeSet::new(),
                ever_seen_files: self.donor_files.keys().cloned().collect(),
            });
        }
    }

    fn record_file_symbols(lane: &mut LaneState, path: &str, content: &str) {
        lane.ever_seen_files.insert(path.to_string());
        let ext = devmap_extract::extract_file(path, content);
        for sym in ext.symbols {
            lane.ever_seen_symbols.insert(sym.name);
        }
    }

    fn step(&mut self, action: &Action) -> StepOutcome {
        match action {
            Action::BuildDonorAndPublishBase => {
                self.build_donor_index();
                match publish_base_snapshot(&self.donor) {
                    Ok((dir, manifest)) => {
                        self.donor_published_count += 1;
                        if !dir.join("base.sqlite").is_file() {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "publish_missing_base",
                                ACT_BUILD_DONOR,
                                format!("published base dir {} missing base.sqlite", dir.display()),
                            ));
                        }
                        if manifest.head_sha != self.donor_head() {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "publish_head_mismatch",
                                ACT_BUILD_DONOR,
                                format!(
                                    "manifest head {} != donor head {}",
                                    manifest.head_sha,
                                    self.donor_head()
                                ),
                            ));
                        }
                        self.verify_all_admitted_lanes_intact(ACT_BUILD_DONOR)
                    }
                    Err(err) => StepOutcome::CheckFailed(CheckFailure::new(
                        "publish_base_failed",
                        ACT_BUILD_DONOR,
                        format!("publish_base_snapshot failed on clean indexed donor: {err:#}"),
                    )),
                }
            }

            Action::ProvisionLane { lane_idx } => {
                if !self.donor_built {
                    return skip_missing("donor_not_built");
                }
                if *lane_idx >= MAX_LANES {
                    return bail("lane_idx_out_of_range");
                }
                self.ensure_lane_worktree(*lane_idx);
                let lane_root = self.lanes[*lane_idx].root.clone();
                // If this lane was previously materialized into a standalone store,
                // retire that standalone store so we can re-provision cleanly, or bail
                // if already stacked and active.
                if self.lanes[*lane_idx].stacked {
                    return bail("lane_already_stacked");
                }
                let standalone = store_path(&lane_root);
                if standalone.exists() {
                    let _ = fs::remove_file(&standalone);
                    for suffix in ["-wal", "-shm", "-journal", ".csr", ".artifacts.json"] {
                        let mut os = standalone.as_os_str().to_os_string();
                        os.push(suffix);
                        let _ = fs::remove_file(PathBuf::from(os));
                    }
                }
                // Reset lane worktree HEAD to donor's current HEAD so base_files matches git HEAD.
                let donor_head = self.donor_head();
                run_git(&lane_root, &["reset", "--hard", &donor_head]);
                run_git(&lane_root, &["clean", "-fd"]);

                match provision_stacked_worktree(&self.donor, &lane_root) {
                    Ok(lease) => {
                        let base_sqlite = lease.base_sqlite_path();
                        let sqlite_stamp = match FileStamp::capture(&base_sqlite) {
                            Ok(s) => s,
                            Err(e) => {
                                return StepOutcome::CheckFailed(CheckFailure::new(
                                    "provision_base_missing",
                                    ACT_PROVISION_LANE,
                                    e,
                                ));
                            }
                        };
                        let csr_path = lease.base_dir().join("base.sqlite.csr");
                        let csr_stamp = if csr_path.is_file() {
                            match FileStamp::capture(&csr_path) {
                                Ok(s) => Some(s),
                                Err(e) => {
                                    return StepOutcome::CheckFailed(CheckFailure::new(
                                        "provision_csr_missing",
                                        ACT_PROVISION_LANE,
                                        e,
                                    ));
                                }
                            }
                        } else {
                            None
                        };

                        let donor_files = self.donor_files.clone();
                        let lane = &mut self.lanes[*lane_idx];
                        lane.stacked = true;
                        lane.admitted_gen = lease.generation_id;
                        lane.base_sqlite_stamp = Some(sqlite_stamp);
                        lane.base_csr_stamp = csr_stamp;
                        lane.base_files = donor_files.clone();
                        lane.working_files = donor_files.clone();
                        lane.synced_delta.clear();
                        lane.ever_seen_files.clear();
                        for (p, c) in &donor_files {
                            Self::record_file_symbols(lane, p, c);
                        }

                        // Immediately verify open + empty unsynced_paths + base immutability.
                        let imm = lane.verify_base_immutable(ACT_PROVISION_LANE);
                        if !imm.is_ok() {
                            return imm;
                        }
                        match StackedStore::open(&lane_root) {
                            Ok(stacked) => {
                                let unsynced = match stacked.unsynced_paths() {
                                    Ok(u) => u,
                                    Err(e) => {
                                        return StepOutcome::CheckFailed(CheckFailure::new(
                                            "provision_unsynced_err",
                                            ACT_PROVISION_LANE,
                                            format!("{e:#}"),
                                        ));
                                    }
                                };
                                check(
                                    unsynced.is_empty(),
                                    "provision_clean_unsynced",
                                    ACT_PROVISION_LANE,
                                    format!(
                                        "freshly provisioned clean lane reported unsynced paths: {unsynced:?}"
                                    ),
                                )
                            }
                            Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                                "provision_open_failed",
                                ACT_PROVISION_LANE,
                                format!("StackedStore::open failed right after provision: {e:#}"),
                            )),
                        }
                    }
                    Err(e) => StepOutcome::CheckFailed(CheckFailure::new(
                        "provision_failed",
                        ACT_PROVISION_LANE,
                        format!("provision_stacked_worktree failed: {e:#}"),
                    )),
                }
            }

            Action::LaneCreateFile {
                lane_idx,
                path,
                content,
            } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                if lane.working_files.contains_key(path) {
                    return bail("path_already_exists");
                }
                write_rel(&lane.root, path, content);
                lane.working_files.insert(path.clone(), content.clone());
                Self::record_file_symbols(lane, path, content);
                self.check_unsynced_oracle(*lane_idx, ACT_LANE_CREATE)
            }

            Action::LaneEditFile {
                lane_idx,
                path,
                content,
            } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                let Some(prev) = lane.working_files.get(path) else {
                    return bail("edit_missing_path");
                };
                if prev == content {
                    return bail("edit_noop");
                }
                write_rel(&lane.root, path, content);
                lane.working_files.insert(path.clone(), content.clone());
                Self::record_file_symbols(lane, path, content);
                self.check_unsynced_oracle(*lane_idx, ACT_LANE_EDIT)
            }

            Action::LaneDeleteFile { lane_idx, path } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                if lane.working_files.len() <= 1 {
                    return bail("keep_at_least_one_file");
                }
                if !lane.working_files.contains_key(path) {
                    return bail("delete_missing_path");
                }
                let full = lane.root.join(path);
                if let Err(e) = fs::remove_file(&full) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "lane_delete_remove_failed",
                        ACT_LANE_DELETE,
                        format!("fs::remove_file({}) failed: {e}", full.display()),
                    ));
                }
                lane.working_files.remove(path);
                self.check_unsynced_oracle(*lane_idx, ACT_LANE_DELETE)
            }

            Action::LaneRenameFile { lane_idx, from, to } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                if from == to || lane.working_files.contains_key(to) {
                    return bail("rename_collision");
                }
                let Some(content) = lane.working_files.remove(from) else {
                    return bail("rename_missing_source");
                };
                let src_full = lane.root.join(from);
                let dst_full = lane.root.join(to);
                if let Some(parent) = dst_full.parent() {
                    fs::create_dir_all(parent).expect("create rename target parent");
                }
                fs::rename(&src_full, &dst_full).expect("rename file");
                lane.working_files.insert(to.clone(), content.clone());
                Self::record_file_symbols(lane, to, &content);
                self.check_unsynced_oracle(*lane_idx, ACT_LANE_RENAME)
            }

            Action::LaneRevertToBase { lane_idx, path } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                let base_content = lane.base_files.get(path).cloned();
                let cur_content = lane.working_files.get(path).cloned();
                if base_content == cur_content {
                    return bail("already_matches_base");
                }
                match base_content {
                    Some(orig) => {
                        write_rel(&lane.root, path, &orig);
                        lane.working_files.insert(path.clone(), orig);
                    }
                    None => {
                        let full = lane.root.join(path);
                        if let Err(e) = fs::remove_file(&full) {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "lane_revert_remove_failed",
                                ACT_LANE_REVERT,
                                format!("fs::remove_file({}) failed: {e}", full.display()),
                            ));
                        }
                        lane.working_files.remove(path);
                    }
                }
                self.check_unsynced_oracle(*lane_idx, ACT_LANE_REVERT)
            }

            Action::SyncSingleFile { lane_idx, path } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                if !lane.expected_unsynced().contains(path) {
                    return bail("file_already_synced");
                }
                let mut stacked = match StackedStore::open(&lane.root) {
                    Ok(s) => s,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "sync_file_open_failed",
                            ACT_SYNC_FILE,
                            format!("{e:#}"),
                        ));
                    }
                };
                if let Err(e) = stacked.sync_file_from_disk(path) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "sync_file_failed",
                        ACT_SYNC_FILE,
                        format!("sync_file_from_disk({path:?}) failed: {e:#}"),
                    ));
                }
                // Update model of delta.sqlite for `path`:
                match lane.working_files.get(path) {
                    Some(cur) => {
                        if lane.base_files.get(path) == Some(cur) {
                            lane.synced_delta.remove(path);
                        } else {
                            lane.synced_delta.insert(path.clone(), Some(cur.clone()));
                        }
                    }
                    None => {
                        if lane.base_files.contains_key(path) {
                            lane.synced_delta.insert(path.clone(), None);
                        } else {
                            // NOTE: A created, synced, then deleted file that never existed in
                            // base. `sync_file_from_disk` should leave it synced (not stuck
                            // forever in `unsynced_paths`).
                            lane.synced_delta.remove(path);
                        }
                    }
                }
                let unsynced = match stacked.unsynced_paths() {
                    Ok(u) => u,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "sync_file_unsynced_err",
                            ACT_SYNC_FILE,
                            format!("{e:#}"),
                        ));
                    }
                };
                let expected = lane.expected_unsynced();
                if unsynced.contains(path) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "sync_file_still_unsynced",
                        ACT_SYNC_FILE,
                        format!(
                            "path {path:?} remained in unsynced_paths immediately after sync_file_from_disk: actual={unsynced:?}, expected={expected:?}, in_base={}, on_disk={}",
                            lane.base_files.contains_key(path),
                            lane.working_files.contains_key(path)
                        ),
                    ));
                }
                let imm = lane.verify_base_immutable(ACT_SYNC_FILE);
                if !imm.is_ok() {
                    return imm;
                }
                check(
                    unsynced == expected,
                    "sync_file_unsynced_set_mismatch",
                    ACT_SYNC_FILE,
                    format!("after sync_file_from_disk({path:?}): actual={unsynced:?} != expected={expected:?}"),
                )
            }

            Action::SyncWorktree { lane_idx } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                let mut stacked = match StackedStore::open(&lane.root) {
                    Ok(s) => s,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "sync_wt_open_failed",
                            ACT_SYNC_WORKTREE,
                            format!("{e:#}"),
                        ));
                    }
                };
                let report = match stacked.sync_worktree() {
                    Ok(r) => r,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "sync_wt_failed",
                            ACT_SYNC_WORKTREE,
                            format!("sync_worktree failed: {e:#}"),
                        ));
                    }
                };
                if report.rebased_to_standalone.is_some() {
                    lane.stacked = false;
                    lane.synced_delta.clear();
                    return StepOutcome::Ok;
                }
                // Recompute exact expected delta state after full sync.
                lane.synced_delta.clear();
                for (p, content) in &lane.working_files {
                    if lane.base_files.get(p) != Some(content) {
                        lane.synced_delta.insert(p.clone(), Some(content.clone()));
                    }
                }
                for p in lane.base_files.keys() {
                    if !lane.working_files.contains_key(p) {
                        lane.synced_delta.insert(p.clone(), None);
                    }
                }
                let unsynced = match stacked.unsynced_paths() {
                    Ok(u) => u,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "sync_wt_unsynced_err",
                            ACT_SYNC_WORKTREE,
                            format!("{e:#}"),
                        ));
                    }
                };
                let imm = lane.verify_base_immutable(ACT_SYNC_WORKTREE);
                if !imm.is_ok() {
                    return imm;
                }
                check(
                    unsynced.is_empty(),
                    "sync_wt_not_empty",
                    ACT_SYNC_WORKTREE,
                    format!("unsynced_paths not empty after sync_worktree: {unsynced:?}"),
                )
            }

            Action::DonorRebuildAndPublish {
                new_commit,
                file_path,
                content,
            } => {
                if !self.donor_built {
                    return skip_missing("donor_not_built");
                }
                if *new_commit {
                    write_rel(&self.donor, file_path, content);
                    self.donor_files.insert(file_path.clone(), content.clone());
                    run_git(&self.donor, &["add", "."]);
                    // Guard against identical content making an empty commit.
                    let status = run_git(&self.donor, &["status", "--porcelain"]);
                    if !status.is_empty() {
                        run_git(&self.donor, &["commit", "-m", "donor advance"]);
                    }
                }
                self.build_donor_index();
                if let Err(e) = publish_base_snapshot(&self.donor) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "donor_republish_failed",
                        ACT_DONOR_REBUILD,
                        format!("publish_base_snapshot failed after donor rebuild: {e:#}"),
                    ));
                }
                self.donor_published_count += 1;
                self.verify_all_admitted_lanes_intact(ACT_DONOR_REBUILD)
            }

            Action::GcBaseCache => {
                if self.donor_published_count == 0 {
                    return skip_missing("no_published_base");
                }
                let common = match registered_common_git_dir(&self.donor) {
                    Some(c) => c,
                    None => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "gc_no_common_git_dir",
                            ACT_GC_BASES,
                            "registered_common_git_dir returned None for donor".to_string(),
                        ));
                    }
                };
                if let Err(e) = gc_unused_bases(&common) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "gc_failed",
                        ACT_GC_BASES,
                        format!("gc_unused_bases failed: {e:#}"),
                    ));
                }
                self.verify_all_admitted_lanes_intact(ACT_GC_BASES)
            }

            Action::MaterializeOrReopenLane {
                lane_idx,
                materialize,
            } => {
                let Some(lane) = self.lanes.get_mut(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                let lane_root = lane.root.clone();
                let admitted_gen = lane.admitted_gen;
                let stacked = match StackedStore::open(&lane_root) {
                    Ok(s) => s,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "reopen_failed",
                            ACT_MATERIALIZE_REOPEN,
                            format!("StackedStore::open failed: {e:#}"),
                        ));
                    }
                };
                if !*materialize {
                    if stacked.lease().generation_id != admitted_gen {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "reopen_gen_mismatch",
                            ACT_MATERIALIZE_REOPEN,
                            format!(
                                "reopened lane generation {} != admitted {}",
                                stacked.lease().generation_id,
                                admitted_gen
                            ),
                        ));
                    }
                    let imm = lane.verify_base_immutable(ACT_MATERIALIZE_REOPEN);
                    if !imm.is_ok() {
                        return imm;
                    }
                    return self.check_unsynced_oracle(*lane_idx, ACT_MATERIALIZE_REOPEN);
                }
                let promoted = match stacked.materialize_seeded_store() {
                    Ok(p) => p,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "materialize_failed",
                            ACT_MATERIALIZE_REOPEN,
                            format!("materialize_seeded_store failed: {e:#}"),
                        ));
                    }
                };
                drop(stacked);
                if !promoted.is_file() {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "materialize_missing_db",
                        ACT_MATERIALIZE_REOPEN,
                        format!("materialized db {} missing", promoted.display()),
                    ));
                }
                if stack_lease_path(&lane_root).exists() {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "materialize_lease_not_retired",
                        ACT_MATERIALIZE_REOPEN,
                        "stack_lease.json still exists after materialize".to_string(),
                    ));
                }
                match StackedStore::open_if_leased(&lane_root) {
                    Ok(None) => {}
                    Ok(Some(_)) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "materialize_still_leased",
                            ACT_MATERIALIZE_REOPEN,
                            "open_if_leased returned Some after materialize".to_string(),
                        ));
                    }
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "materialize_open_if_leased_err",
                            ACT_MATERIALIZE_REOPEN,
                            format!("open_if_leased errored after materialize: {e:#}"),
                        ));
                    }
                }
                match Store::open_read_only(&promoted) {
                    Ok(store) => {
                        let gen_id = store.latest_generation_id().ok().flatten();
                        if gen_id != Some(admitted_gen) {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "materialize_gen_mismatch",
                                ACT_MATERIALIZE_REOPEN,
                                format!(
                                    "materialized store gen {gen_id:?} != admitted {admitted_gen}"
                                ),
                            ));
                        }
                    }
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "materialize_store_unreadable",
                            ACT_MATERIALIZE_REOPEN,
                            format!("materialized store unreadable: {e:#}"),
                        ));
                    }
                }
                let lane = &mut self.lanes[*lane_idx];
                lane.stacked = false;
                lane.base_sqlite_stamp = None;
                lane.base_csr_stamp = None;
                lane.synced_delta.clear();
                StepOutcome::Ok
            }

            Action::QueryAndCheckParity { lane_idx } => {
                let Some(lane) = self.lanes.get(*lane_idx) else {
                    return skip_missing("no_lane");
                };
                if !lane.stacked {
                    return skip_missing("lane_not_stacked");
                }
                let unsynced_check = self.check_unsynced_oracle(*lane_idx, ACT_QUERY_PARITY);
                if !unsynced_check.is_ok() {
                    return unsynced_check;
                }
                if !self.lanes[*lane_idx].expected_unsynced().is_empty() {
                    return bail("lane_has_unsynced_changes");
                }
                self.check_full_parity(*lane_idx)
            }
        }
    }

    fn check_unsynced_oracle(&self, lane_idx: usize, action: &'static str) -> StepOutcome {
        let lane = &self.lanes[lane_idx];
        let delta_existed = stack_delta_path(&lane.root).exists();
        let stacked = match StackedStore::open(&lane.root) {
            Ok(s) => s,
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "unsynced_open_failed",
                    action,
                    format!("{e:#}"),
                ));
            }
        };
        let actual = match stacked.unsynced_paths() {
            Ok(u) => u,
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "unsynced_query_failed",
                    action,
                    format!("{e:#}"),
                ));
            }
        };
        if !delta_existed && stack_delta_path(&lane.root).exists() {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "readonly_created_delta",
                action,
                "read-only open + unsynced_paths created delta.sqlite".to_string(),
            ));
        }
        let imm = lane.verify_base_immutable(action);
        if !imm.is_ok() {
            return imm;
        }
        let expected = lane.expected_unsynced();
        check(
            actual == expected,
            "unsynced_paths_mismatch",
            action,
            format!("lane {lane_idx} unsynced_paths mismatch: actual={actual:?} != expected={expected:?}"),
        )
    }

    fn verify_all_admitted_lanes_intact(&self, action: &'static str) -> StepOutcome {
        for (idx, lane) in self.lanes.iter().enumerate() {
            if !lane.stacked {
                continue;
            }
            let imm = lane.verify_base_immutable(action);
            if !imm.is_ok() {
                return imm;
            }
            let stacked = match StackedStore::open(&lane.root) {
                Ok(s) => s,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "admitted_lane_broken",
                        action,
                        format!("admitted lane {idx} failed to open: {e:#}"),
                    ));
                }
            };
            if stacked.lease().generation_id != lane.admitted_gen {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "admitted_lane_gen_changed",
                    action,
                    format!(
                        "admitted lane {idx} gen changed from {} to {}",
                        lane.admitted_gen,
                        stacked.lease().generation_id
                    ),
                ));
            }
            if let Some(ref stamp) = lane.base_sqlite_stamp {
                if stacked.lease().base.sha256 != stamp.sha256 {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "admitted_lane_sha_changed",
                        action,
                        format!(
                            "admitted lane {idx} lease sha {} != {}",
                            stacked.lease().base.sha256,
                            stamp.sha256
                        ),
                    ));
                }
            }
        }
        StepOutcome::Ok
    }

    fn check_full_parity(&mut self, lane_idx: usize) -> StepOutcome {
        self.cold_counter += 1;
        let cold_db = self.root.join(format!(
            "cold-oracle-{lane_idx}-{}.sqlite",
            self.cold_counter
        ));
        let lane = &self.lanes[lane_idx];
        let delta_existed = stack_delta_path(&lane.root).exists();
        let lane_head = run_git(&lane.root, &["rev-parse", "HEAD"]);
        index_directory_into(&lane.root, &cold_db, &lane.working_files, &lane_head);

        let cold_store = match Store::open_read_only(&cold_db) {
            Ok(s) => s,
            Err(e) => {
                let _ = fs::remove_file(&cold_db);
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "cold_store_open",
                    ACT_QUERY_PARITY,
                    format!("{e:#}"),
                ));
            }
        };
        let stacked = match StackedStore::open(&lane.root) {
            Ok(s) => s,
            Err(e) => {
                let _ = fs::remove_file(&cold_db);
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "stacked_open_parity",
                    ACT_QUERY_PARITY,
                    format!("{e:#}"),
                ));
            }
        };

        let result = self.compare_engines(lane_idx, &stacked, &cold_store);
        drop(stacked);
        drop(cold_store);
        let _ = fs::remove_file(&cold_db);
        for suffix in ["-wal", "-shm", "-journal", ".csr"] {
            let mut os = cold_db.as_os_str().to_os_string();
            os.push(suffix);
            let _ = fs::remove_file(PathBuf::from(os));
        }

        if !result.is_ok() {
            return result;
        }
        if !delta_existed && stack_delta_path(&self.lanes[lane_idx].root).exists() {
            return StepOutcome::CheckFailed(CheckFailure::new(
                "readonly_query_created_delta",
                ACT_QUERY_PARITY,
                "read-only parity queries created delta.sqlite".to_string(),
            ));
        }
        self.lanes[lane_idx].verify_base_immutable(ACT_QUERY_PARITY)
    }

    fn compare_engines(
        &self,
        lane_idx: usize,
        stacked: &StackedStore,
        cold_store: &Store,
    ) -> StepOutcome {
        let lane = &self.lanes[lane_idx];
        let stacked_eng = StoreQueryEngine::new(stacked.base_store()).with_stacked(Some(stacked));
        let cold_eng = StoreQueryEngine::new(cold_store);
        let cancel = Cancel::new();

        // 1. Per-file symbol parity (every live file + every deleted base file).
        let all_paths: BTreeSet<&String> = lane
            .base_files
            .keys()
            .chain(lane.working_files.keys())
            .collect();
        for path in &all_paths {
            let s_syms: BTreeSet<(String, String, String, usize, usize)> =
                match stacked.symbols_for_file(path) {
                    Ok(rows) => rows
                        .into_iter()
                        .map(|s| (s.path, s.qualified_name, s.kind, s.span_start, s.span_end))
                        .collect(),
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "symbols_for_file_err",
                            ACT_QUERY_PARITY,
                            format!("{path}: {e:#}"),
                        ));
                    }
                };
            let c_syms: BTreeSet<(String, String, String, usize, usize)> =
                match cold_store.latest_extraction_for_path(path) {
                    Ok(Some(ext)) => ext
                        .symbols
                        .into_iter()
                        .map(|s| {
                            (
                                (*path).clone(),
                                s.qualified_name,
                                s.kind.as_str().to_string(),
                                s.span.start_byte,
                                s.span.end_byte,
                            )
                        })
                        .collect(),
                    Ok(None) => BTreeSet::new(),
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "cold_symbols_err",
                            ACT_QUERY_PARITY,
                            format!("{path}: {e:#}"),
                        ));
                    }
                };
            if s_syms != c_syms {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "symbols_for_file_parity",
                    ACT_QUERY_PARITY,
                    format!(
                        "symbols_for_file({path:?}) mismatch:\nstacked={s_syms:?}\ncold={c_syms:?}"
                    ),
                ));
            }
        }

        // 2. Search parity across live, edited, and deleted symbol names.
        let mut search_queries: Vec<String> = vec![
            "api".to_string(),
            "helper".to_string(),
            "mut_fn".to_string(),
            "dead".to_string(),
        ];
        for sym in lane.ever_seen_symbols.iter().take(6) {
            search_queries.push(sym.clone());
        }
        for q in &search_queries {
            let req = Request {
                query: q.clone(),
                token_budget: Budget::SEARCH * 10,
                min_confidence: 0.0,
                max_depth: 2,
            };
            let s_resp = match stacked_eng.search(req.clone()) {
                Ok(r) => r,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "search_stacked_err",
                        ACT_QUERY_PARITY,
                        format!("query={q:?}: {e:#}"),
                    ));
                }
            };
            let c_resp = match cold_eng.search(req) {
                Ok(r) => r,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "search_cold_err",
                        ACT_QUERY_PARITY,
                        format!("query={q:?}: {e:#}"),
                    ));
                }
            };
            if stacked.has_delta_changes() && s_resp.source_freshness.fresh.is_some() {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "search_freshness_must_be_unverified",
                    ACT_QUERY_PARITY,
                    format!(
                        "dirty stacked search returned verified freshness: {:?}",
                        s_resp.source_freshness
                    ),
                ));
            }
            let s_set: BTreeSet<(String, String, String, (u32, u32))> = s_resp
                .items
                .iter()
                .map(|h| {
                    (
                        h.file_path.clone(),
                        h.symbol_name.clone(),
                        h.kind.clone(),
                        h.span,
                    )
                })
                .collect();
            let c_set: BTreeSet<(String, String, String, (u32, u32))> = c_resp
                .items
                .iter()
                .map(|h| {
                    (
                        h.file_path.clone(),
                        h.symbol_name.clone(),
                        h.kind.clone(),
                        h.span,
                    )
                })
                .collect();
            if s_set != c_set || s_resp.total != c_resp.total {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "search_parity",
                    ACT_QUERY_PARITY,
                    format!(
                        "search({q:?}) mismatch (totals stacked={} vs cold={}):\nstacked={s_set:?}\ncold={c_set:?}",
                        s_resp.total, c_resp.total
                    ),
                ));
            }
        }

        // 3. Dependencies + Callers + Impact parity.
        // Build canonical edge sets for every file (both clean and dirty).
        let canonical_edges = |items: &[devmap_resolve::model::ResolvedEdge]| {
            items
                .iter()
                .map(|e| {
                    (
                        e.source_file.clone(),
                        e.target_file.clone(),
                        e.source_symbol.clone(),
                        e.target_symbol.clone(),
                        format!("{:?}", e.edge_kind),
                    )
                })
                .collect::<BTreeSet<_>>()
        };

        // Check file-level dependencies parity on every file (including deleted files).
        for path in &all_paths {
            let req = Request {
                query: (*path).clone(),
                token_budget: Budget::DEPS * 10,
                min_confidence: 0.0,
                max_depth: 2,
            };
            let s_dep = match stacked_eng.dependencies(req.clone()) {
                Ok(r) => r,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_stacked_err",
                        ACT_QUERY_PARITY,
                        format!("{path:?}: {e:#}"),
                    ));
                }
            };
            let c_dep = match cold_eng.dependencies(req) {
                Ok(r) => r,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_cold_err",
                        ACT_QUERY_PARITY,
                        format!("{path:?}: {e:#}"),
                    ));
                }
            };
            if s_dep.resolution != c_dep.resolution {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "deps_resolution_parity",
                    ACT_QUERY_PARITY,
                    format!(
                        "dependencies({path:?}) resolution mismatch: stacked={:?} vs cold={:?}",
                        s_dep.resolution, c_dep.resolution
                    ),
                ));
            }
            let s_edges = canonical_edges(&s_dep.items);
            let c_edges = canonical_edges(&c_dep.items);
            // Tombstoned/deleted files must never appear as source or target in stacked edges.
            for (src_f, dst_f, _, _, _) in &s_edges {
                if lane.is_deleted_file(src_f) || lane.is_deleted_file(dst_f) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_tombstone_leak",
                        ACT_QUERY_PARITY,
                        format!(
                            "dependencies({path:?}) leaked deleted file edge {src_f} -> {dst_f}"
                        ),
                    ));
                }
            }
            // When clean (no delta) or for `Contains` edges on any file, parity must be exact.
            if !stacked.has_delta_changes() {
                if s_edges != c_edges {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_clean_parity",
                        ACT_QUERY_PARITY,
                        format!(
                            "clean dependencies({path:?}) mismatch:\nstacked={s_edges:?}\ncold={c_edges:?}"
                        ),
                    ));
                }
            } else {
                let s_contains: BTreeSet<_> = s_edges
                    .iter()
                    .filter(|(_, _, _, _, k)| k == "Contains")
                    .cloned()
                    .collect();
                let c_contains: BTreeSet<_> = c_edges
                    .iter()
                    .filter(|(_, _, _, _, k)| k == "Contains")
                    .cloned()
                    .collect();
                if s_contains != c_contains {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_contains_parity",
                        ACT_QUERY_PARITY,
                        format!(
                            "dependencies({path:?}) Contains edges mismatch:\nstacked={s_contains:?}\ncold={c_contains:?}"
                        ),
                    ));
                }
                // Check Calls edges within the same file (SameFile resolution is exact in delta).
                let s_same_file_calls: BTreeSet<_> = s_edges
                    .iter()
                    .filter(|(sf, tf, _, _, k)| k == "Calls" && sf == tf)
                    .cloned()
                    .collect();
                let c_same_file_calls: BTreeSet<_> = c_edges
                    .iter()
                    .filter(|(sf, tf, _, _, k)| k == "Calls" && sf == tf)
                    .cloned()
                    .collect();
                if s_same_file_calls != c_same_file_calls {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "deps_same_file_calls_parity",
                        ACT_QUERY_PARITY,
                        format!(
                            "dependencies({path:?}) same-file Calls mismatch:\nstacked={s_same_file_calls:?}\ncold={c_same_file_calls:?}"
                        ),
                    ));
                }
            }
        }

        // 4. Callers, edges_for_symbol, and Impact checks on live and deleted symbols.
        let mut target_qns: Vec<String> = Vec::new();
        for (path, content) in &lane.working_files {
            let ext = devmap_extract::extract_file(path, content);
            for sym in ext.symbols.into_iter().take(2) {
                target_qns.push(sym.qualified_name);
            }
        }
        // Also check a symbol from a deleted file if one exists.
        for (path, content) in &lane.base_files {
            if !lane.working_files.contains_key(path) {
                let ext = devmap_extract::extract_file(path, content);
                if let Some(sym) = ext.symbols.into_iter().next() {
                    target_qns.push(sym.qualified_name);
                }
            }
        }

        for qn in target_qns.iter().take(6) {
            // callers_page: must never leak tombstoned source or target files/symbols.
            let s_callers = match stacked.callers_page(std::slice::from_ref(qn), "", 0.0, &cancel) {
                Ok(Some(p)) => p.callers,
                Ok(None) => Vec::new(),
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "callers_page_err",
                        ACT_QUERY_PARITY,
                        format!("callers_page({qn:?}): {e:#}"),
                    ));
                }
            };
            for edge in &s_callers {
                if lane.is_deleted_file(&edge.source_file)
                    || lane.is_deleted_file(&edge.target_file)
                {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "callers_tombstone_leak",
                        ACT_QUERY_PARITY,
                        format!(
                            "callers_page({qn:?}) leaked deleted file edge {} -> {}",
                            edge.source_file, edge.target_file
                        ),
                    ));
                }
            }
            if !stacked.has_delta_changes() {
                let c_callers = match cold_store.callers_page(std::slice::from_ref(qn), "", 0.0) {
                    Ok(Some(p)) => p.callers,
                    Ok(None) => Vec::new(),
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "cold_callers_err",
                            ACT_QUERY_PARITY,
                            format!("{e:#}"),
                        ));
                    }
                };
                let s_set: BTreeSet<_> = s_callers
                    .iter()
                    .map(|e| {
                        (
                            &e.source_file,
                            &e.target_file,
                            &e.source_symbol,
                            &e.target_symbol,
                        )
                    })
                    .collect();
                let c_set: BTreeSet<_> = c_callers
                    .iter()
                    .map(|e| {
                        (
                            &e.source_file,
                            &e.target_file,
                            &e.source_symbol,
                            &e.target_symbol,
                        )
                    })
                    .collect();
                if s_set != c_set {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "callers_clean_parity",
                        ACT_QUERY_PARITY,
                        format!("clean callers_page({qn:?}) mismatch: stacked={s_set:?} != cold={c_set:?}"),
                    ));
                }
            }

            // edges_for_symbol: no tombstone leaks and exact parity when clean.
            let s_sym_edges = match stacked.edges_for_symbol(qn) {
                Ok(edges) => edges,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "edges_for_symbol_err",
                        ACT_QUERY_PARITY,
                        format!("edges_for_symbol({qn:?}): {e:#}"),
                    ));
                }
            };
            fn file_prefix(sym: &str) -> &str {
                sym.split_once("::").map(|(f, _)| f).unwrap_or(sym)
            }
            for edge in &s_sym_edges {
                if lane.is_deleted_file(file_prefix(&edge.source))
                    || lane.is_deleted_file(file_prefix(&edge.target))
                {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "edges_for_symbol_tombstone_leak",
                        ACT_QUERY_PARITY,
                        format!(
                            "edges_for_symbol({qn:?}) leaked deleted file edge {} -> {} ({})",
                            edge.source, edge.target, edge.kind
                        ),
                    ));
                }
            }
            if lane.is_deleted_file(file_prefix(qn)) && !s_sym_edges.is_empty() {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "edges_for_symbol_deleted_target_leak",
                    ACT_QUERY_PARITY,
                    format!(
                        "edges_for_symbol({qn:?}) on deleted file returned non-empty: {s_sym_edges:?}"
                    ),
                ));
            }
            if !stacked.has_delta_changes() {
                let s_set: BTreeSet<(String, String, String)> = s_sym_edges
                    .iter()
                    .map(|e| (e.source.clone(), e.target.clone(), e.kind.clone()))
                    .collect();
                let mut c_set: BTreeSet<(String, String, String)> = BTreeSet::new();
                let callers = match cold_store.callers_of(std::slice::from_ref(qn), "", 0.0) {
                    Ok(c) => c,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "cold_edges_for_symbol_callers_err",
                            ACT_QUERY_PARITY,
                            format!("{e:#}"),
                        ));
                    }
                };
                for e in callers {
                    c_set.insert((e.source_symbol, e.target_symbol, e.edge_kind));
                }
                let syms = match cold_store.search_symbols(qn, 64) {
                    Ok(s) => s,
                    Err(e) => {
                        return StepOutcome::CheckFailed(CheckFailure::new(
                            "cold_edges_for_symbol_search_err",
                            ACT_QUERY_PARITY,
                            format!("{e:#}"),
                        ));
                    }
                };
                let mut def_files = BTreeSet::new();
                for sym in syms {
                    if sym.name == *qn || sym.qualified_name == *qn {
                        def_files.insert(sym.path);
                    }
                }
                for file_path in def_files {
                    let file_edges = match cold_store.latest_edges_for_file(&file_path, 0.0) {
                        Ok(fe) => fe,
                        Err(e) => {
                            return StepOutcome::CheckFailed(CheckFailure::new(
                                "cold_edges_for_symbol_file_err",
                                ACT_QUERY_PARITY,
                                format!("{e:#}"),
                            ));
                        }
                    };
                    for e in file_edges {
                        if e.source_symbol == *qn || e.target_symbol == *qn {
                            c_set.insert((e.source_symbol, e.target_symbol, e.edge_kind));
                        }
                    }
                }
                if s_set != c_set {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "edges_for_symbol_clean_parity",
                        ACT_QUERY_PARITY,
                        format!(
                            "clean edges_for_symbol({qn:?}) mismatch:\nstacked={s_set:?}\ncold={c_set:?}"
                        ),
                    ));
                }
            }

            // impact parity.
            let imp_req = Request {
                query: qn.clone(),
                token_budget: Budget::DEPS * 10,
                min_confidence: 0.0,
                max_depth: 3,
            };
            let s_imp = match stacked_eng.impact(imp_req.clone()) {
                Ok(r) => r,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "impact_stacked_err",
                        ACT_QUERY_PARITY,
                        format!("impact({qn:?}): {e:#}"),
                    ));
                }
            };
            let c_imp = match cold_eng.impact(imp_req) {
                Ok(r) => r,
                Err(e) => {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "impact_cold_err",
                        ACT_QUERY_PARITY,
                        format!("cold impact({qn:?}): {e:#}"),
                    ));
                }
            };
            let s_imp_edges = canonical_edges(&s_imp.items);
            let c_imp_edges = canonical_edges(&c_imp.items);
            for (src_f, dst_f, _, _, _) in &s_imp_edges {
                if lane.is_deleted_file(src_f) || lane.is_deleted_file(dst_f) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "impact_tombstone_leak",
                        ACT_QUERY_PARITY,
                        format!("impact({qn:?}) leaked deleted file edge {src_f} -> {dst_f}"),
                    ));
                }
            }
            if !stacked.has_delta_changes() && s_imp_edges != c_imp_edges {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "impact_clean_parity",
                    ACT_QUERY_PARITY,
                    format!(
                        "clean impact({qn:?}) mismatch:\nstacked={s_imp_edges:?}\ncold={c_imp_edges:?}"
                    ),
                ));
            }
        }

        // 5. Dead symbols parity / lower-bound soundness.
        let s_dead = match stacked_eng.dead_symbols(Budget::DEAD * 10) {
            Ok(r) => r,
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "dead_stacked_err",
                    ACT_QUERY_PARITY,
                    format!("{e:#}"),
                ));
            }
        };
        let c_dead = match cold_eng.dead_symbols(Budget::DEAD * 10) {
            Ok(r) => r,
            Err(e) => {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "dead_cold_err",
                    ACT_QUERY_PARITY,
                    format!("{e:#}"),
                ));
            }
        };
        let s_dead_set: BTreeSet<(String, String)> = s_dead
            .items
            .iter()
            .map(|d| (d.file_path.clone(), d.symbol_name.clone()))
            .collect();
        let c_dead_set: BTreeSet<(String, String)> = c_dead
            .items
            .iter()
            .map(|d| (d.file_path.clone(), d.symbol_name.clone()))
            .collect();

        if !stacked.has_delta_changes() {
            if s_dead_set != c_dead_set {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "dead_clean_parity",
                    ACT_QUERY_PARITY,
                    format!(
                        "clean dead_symbols mismatch:\nstacked={s_dead_set:?}\ncold={c_dead_set:?}"
                    ),
                ));
            }
            if s_dead.dead_clusters_incomplete.is_some() {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "dead_clean_clusters_incomplete",
                    ACT_QUERY_PARITY,
                    format!(
                        "clean stacked dead_symbols set dead_clusters_incomplete: {:?}",
                        s_dead.dead_clusters_incomplete
                    ),
                ));
            }
        } else {
            // Dirty stacked lane: dead_page is a documented lower bound over unmodified base
            // symbols; must disclose lower bound, drop clusters honestly, never leak
            // tombstoned/modified files, and never falsely claim a resurrected symbol is dead.
            let has_lower_bound = s_dead
                .walk_incomplete
                .as_deref()
                .is_some_and(|w| w.contains("lower bound"));
            if !has_lower_bound {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "dead_dirty_missing_lower_bound",
                    ACT_QUERY_PARITY,
                    format!(
                        "dirty stacked dead_symbols omitted lower-bound disclosure: {:?}",
                        s_dead.walk_incomplete
                    ),
                ));
            }
            if s_dead.dead_clusters.is_some() || s_dead.dead_clusters_incomplete.is_none() {
                return StepOutcome::CheckFailed(CheckFailure::new(
                    "dead_dirty_clusters_not_dropped",
                    ACT_QUERY_PARITY,
                    format!(
                        "dirty stacked dead_symbols must drop dead_clusters and explain why: clusters={:?}, incomplete={:?}",
                        s_dead.dead_clusters.is_some(),
                        s_dead.dead_clusters_incomplete
                    ),
                ));
            }
            for (file, sym) in &s_dead_set {
                if !lane.working_files.contains_key(file) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "dead_tombstone_leak",
                        ACT_QUERY_PARITY,
                        format!("dead_symbols leaked deleted file symbol {file}::{sym}"),
                    ));
                }
                if lane.synced_delta.contains_key(file) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "dead_modified_file_leak",
                        ACT_QUERY_PARITY,
                        format!("dead_symbols leaked modified delta file symbol {file}::{sym}"),
                    ));
                }
                if !c_dead_set.contains(&(file.clone(), sym.clone())) {
                    return StepOutcome::CheckFailed(CheckFailure::new(
                        "dead_false_positive_resurrected",
                        ACT_QUERY_PARITY,
                        format!(
                            "stacked dead_symbols reported {file}::{sym} dead, but cold flat store says it is live!\nstacked={s_dead_set:?}\ncold={c_dead_set:?}"
                        ),
                    ));
                }
            }
        }

        StepOutcome::Ok
    }
}

fn synth_action(
    src: &mut Source,
    world: &World,
    action_name: &'static str,
    step_idx: usize,
) -> Action {
    let pick_stacked_lane = |src: &mut Source| -> usize {
        let stacked_indices: Vec<usize> = world
            .lanes
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.stacked.then_some(i))
            .collect();
        src.choose(&stacked_indices).copied().unwrap_or(0)
    };

    match action_name {
        ACT_BUILD_DONOR => Action::BuildDonorAndPublishBase,
        ACT_PROVISION_LANE => {
            let unstacked: Vec<usize> = (0..MAX_LANES)
                .filter(|&i| !world.lanes.get(i).is_some_and(|l| l.stacked))
                .collect();
            let lane_idx = src.choose(&unstacked).copied().unwrap_or(0);
            Action::ProvisionLane { lane_idx }
        }
        ACT_LANE_CREATE => {
            let lane_idx = pick_stacked_lane(src);
            let langs = [
                MicroLang::Rust,
                MicroLang::TypeScript,
                MicroLang::Go,
                MicroLang::Python,
            ];
            let lang = *src.choose(&langs).unwrap_or(&MicroLang::Rust);
            let dir = match lang {
                MicroLang::Rust => "src",
                MicroLang::TypeScript => "ts",
                MicroLang::Go => "go",
                MicroLang::Python => "py",
            };
            let tag = src.usize_below(10_000);
            let path = format!("{dir}/added_{step_idx}_{tag}.{}", lang.ext());
            let content = match lang {
                MicroLang::Rust => format!(
                    "pub fn added_fn_{tag}(x: i32) -> i32 {{\n    added_leaf_{tag}(x) + 1\n}}\n\nfn added_leaf_{tag}(x: i32) -> i32 {{\n    x + {step_idx}\n}}\n"
                ),
                MicroLang::TypeScript => format!(
                    "export function addedFn_{tag}(x: number): number {{\n  return addedLeaf_{tag}(x) + 1;\n}}\n\nfunction addedLeaf_{tag}(x: number): number {{\n  return x + {step_idx};\n}}\n"
                ),
                MicroLang::Go => format!(
                    "package micro\n\nfunc AddedFn{tag}(x int) int {{\n\treturn addedLeaf{tag}(x) + 1\n}}\n\nfunc addedLeaf{tag}(x int) int {{\n\treturn x + {step_idx}\n}}\n"
                ),
                MicroLang::Python => format!(
                    "def added_fn_{tag}(x: int) -> int:\n    return added_leaf_{tag}(x) + 1\n\ndef added_leaf_{tag}(x: int) -> int:\n    return x + {step_idx}\n"
                ),
            };
            Action::LaneCreateFile {
                lane_idx,
                path,
                content,
            }
        }
        ACT_LANE_EDIT => {
            let lane_idx = pick_stacked_lane(src);
            let (path, content) = if let Some(lane) = world.lanes.get(lane_idx) {
                let paths: Vec<String> = lane.working_files.keys().cloned().collect();
                if let Some(p) = src.choose(&paths).cloned() {
                    let prev = &lane.working_files[&p];
                    let lang = lang_for_path(&p);
                    let mut mutated = mutate_file_elements(src, lang, prev);
                    if mutated == *prev {
                        mutated = mutate_file_elements(src, lang, &mutated);
                    }
                    (p, mutated)
                } else {
                    (
                        "src/mod_0.rs".to_string(),
                        "pub fn fallback() {}\n".to_string(),
                    )
                }
            } else {
                (
                    "src/mod_0.rs".to_string(),
                    "pub fn fallback() {}\n".to_string(),
                )
            };
            Action::LaneEditFile {
                lane_idx,
                path,
                content,
            }
        }
        ACT_LANE_DELETE => {
            let lane_idx = pick_stacked_lane(src);
            let path = world
                .lanes
                .get(lane_idx)
                .and_then(|l| {
                    let paths: Vec<String> = l.working_files.keys().cloned().collect();
                    src.choose(&paths).cloned()
                })
                .unwrap_or_else(|| "src/mod_0.rs".to_string());
            Action::LaneDeleteFile { lane_idx, path }
        }
        ACT_LANE_RENAME => {
            let lane_idx = pick_stacked_lane(src);
            let (from, to) = if let Some(lane) = world.lanes.get(lane_idx) {
                let paths: Vec<String> = lane.working_files.keys().cloned().collect();
                if let Some(from) = src.choose(&paths).cloned() {
                    let lang = lang_for_path(&from);
                    let dir = Path::new(&from)
                        .parent()
                        .and_then(|p| p.to_str())
                        .unwrap_or("");
                    let tag = src.usize_below(10_000);
                    let to = if dir.is_empty() {
                        format!("renamed_{step_idx}_{tag}.{}", lang.ext())
                    } else {
                        format!("{dir}/renamed_{step_idx}_{tag}.{}", lang.ext())
                    };
                    (from, to)
                } else {
                    ("src/mod_0.rs".to_string(), "src/renamed_0.rs".to_string())
                }
            } else {
                ("src/mod_0.rs".to_string(), "src/renamed_0.rs".to_string())
            };
            Action::LaneRenameFile { lane_idx, from, to }
        }
        ACT_LANE_REVERT => {
            let dirty_lanes: Vec<usize> = world
                .lanes
                .iter()
                .enumerate()
                .filter_map(|(i, l)| {
                    (l.stacked
                        && l.base_files
                            .keys()
                            .chain(l.working_files.keys())
                            .any(|p| l.base_files.get(p) != l.working_files.get(p)))
                    .then_some(i)
                })
                .collect();
            let lane_idx = src
                .choose(&dirty_lanes)
                .copied()
                .unwrap_or_else(|| pick_stacked_lane(src));
            let path = world
                .lanes
                .get(lane_idx)
                .and_then(|l| {
                    let dirty: Vec<String> = l
                        .base_files
                        .keys()
                        .chain(l.working_files.keys())
                        .filter(|p| l.base_files.get(*p) != l.working_files.get(*p))
                        .cloned()
                        .collect();
                    src.choose(&dirty).cloned()
                })
                .unwrap_or_else(|| "src/mod_0.rs".to_string());
            Action::LaneRevertToBase { lane_idx, path }
        }
        ACT_SYNC_FILE => {
            let unsynced_lanes: Vec<usize> = world
                .lanes
                .iter()
                .enumerate()
                .filter_map(|(i, l)| (l.stacked && !l.expected_unsynced().is_empty()).then_some(i))
                .collect();
            let lane_idx = src
                .choose(&unsynced_lanes)
                .copied()
                .unwrap_or_else(|| pick_stacked_lane(src));
            let path = world
                .lanes
                .get(lane_idx)
                .and_then(|l| {
                    let unsynced: Vec<String> = l.expected_unsynced().into_iter().collect();
                    src.choose(&unsynced).cloned()
                })
                .unwrap_or_else(|| "src/mod_0.rs".to_string());
            Action::SyncSingleFile { lane_idx, path }
        }
        ACT_SYNC_WORKTREE => {
            let lane_idx = pick_stacked_lane(src);
            Action::SyncWorktree { lane_idx }
        }
        ACT_DONOR_REBUILD => {
            let new_commit = src.chance(3, 4);
            let tag = src.usize_below(10_000);
            let file_path = format!("src/donor_gen_{step_idx}_{tag}.rs");
            let content =
                format!("pub fn donor_gen_{tag}(x: i32) -> i32 {{\n    x + {step_idx}\n}}\n");
            Action::DonorRebuildAndPublish {
                new_commit,
                file_path,
                content,
            }
        }
        ACT_GC_BASES => Action::GcBaseCache,
        ACT_MATERIALIZE_REOPEN => {
            let lane_idx = pick_stacked_lane(src);
            // Prefer reopen often, materialize occasionally (keeping at least one lane
            // or allowing ProvisionLane to re-stack it).
            let materialize = src.chance(2, 5);
            Action::MaterializeOrReopenLane {
                lane_idx,
                materialize,
            }
        }
        ACT_QUERY_PARITY => {
            let lane_idx = pick_stacked_lane(src);
            Action::QueryAndCheckParity { lane_idx }
        }
        other => unreachable!("unknown action {other}"),
    }
}

fn replay_sequence(initial_repo: &MicroRepo, actions: &[Action]) -> Option<CheckFailure> {
    let mut world = World::new("repro", initial_repo);
    for action in actions {
        if let StepOutcome::CheckFailed(failure) = world.step(action) {
            return Some(failure);
        }
    }
    None
}

#[test]
fn stateful_stacked_store_model_checker() {
    let seed = env_u64("DEVMAP_STATEFUL_SEED", DEFAULT_SEED);
    let rounds = env_usize("DEVMAP_STATEFUL_ROUNDS", DEFAULT_ROUNDS);
    let started = Instant::now();

    let mut root_src = Source::from_seed(seed);
    let mut tracker = HealthTracker::new();

    for round in 0..rounds {
        let round_seed = root_src.fork_seed();
        let mut src = Source::from_seed(round_seed);
        let initial_repo = generate_micro_repo(
            &mut src,
            &[
                MicroLang::Rust,
                MicroLang::TypeScript,
                MicroLang::Go,
                MicroLang::Python,
            ],
            4,
        );
        let mut world = World::new(&format!("r{round}"), &initial_repo);
        let mut history: Vec<Action> = Vec::with_capacity(STEPS_PER_ROUND + 4);
        let mut prev_action: Option<&'static str> = None;

        // Bootstrap donor + first lane so every round reaches deep stacked states quickly.
        let bootstrap = [
            Action::BuildDonorAndPublishBase,
            Action::ProvisionLane { lane_idx: 0 },
        ];
        for act in bootstrap {
            let name = act.name();
            let outcome = world.step(&act);
            tracker.record(name, prev_action, &outcome);
            history.push(act);
            if let StepOutcome::CheckFailed(failure) = outcome {
                let minimal = minimize_sequence(&history, &failure.signature(), 32, |seq| {
                    replay_sequence(&initial_repo, seq)
                });
                let summaries: Vec<String> = minimal.iter().map(Action::summary).collect();
                let repro_failure = CheckFailure::new(
                    failure.oracle,
                    failure.action,
                    format!(
                        "{} (round={round}, round_seed={round_seed:#x})",
                        failure.detail
                    ),
                );
                let repro = format_repro_test(
                    "repro_stacked_store_failure",
                    seed,
                    &repro_failure,
                    &summaries,
                );
                panic!(
                    "stateful stacked-store oracle failed (round={round}, seed={seed:#x}, round_seed={round_seed:#x}): {failure}\n\n{repro}\n\n{}",
                    tracker.probe_report("stacked_store")
                );
            }
            prev_action = Some(name);
        }

        for step_idx in 0..STEPS_PER_ROUND {
            // Filter obviously impossible actions only for missing lanes/donor so the
            // HealthTracker bandit still exercises realistic bail/ok transitions.
            let has_unsynced = world
                .lanes
                .iter()
                .any(|l| l.stacked && !l.expected_unsynced().is_empty());
            let has_dirty_vs_base = world.lanes.iter().any(|l| {
                l.stacked
                    && l.base_files
                        .keys()
                        .chain(l.working_files.keys())
                        .any(|p| l.base_files.get(p) != l.working_files.get(p))
            });
            let candidates: Vec<&'static str> = ALL_ACTIONS
                .iter()
                .copied()
                .filter(|&name| match name {
                    ACT_PROVISION_LANE => {
                        world.lanes.iter().filter(|l| l.stacked).count() < MAX_LANES
                    }
                    ACT_SYNC_FILE => has_unsynced,
                    ACT_LANE_REVERT => has_dirty_vs_base,
                    ACT_LANE_CREATE
                    | ACT_LANE_EDIT
                    | ACT_LANE_DELETE
                    | ACT_LANE_RENAME
                    | ACT_SYNC_WORKTREE
                    | ACT_MATERIALIZE_REOPEN
                    | ACT_QUERY_PARITY => world.lanes.iter().any(|l| l.stacked),
                    _ => true,
                })
                .collect();
            let chosen_name = tracker.pick_weighted(&mut src, &candidates, prev_action);
            let act = synth_action(&mut src, &world, chosen_name, step_idx);
            let name = act.name();
            let outcome = world.step(&act);
            tracker.record(name, prev_action, &outcome);
            history.push(act);
            if let StepOutcome::CheckFailed(failure) = outcome {
                let minimal = minimize_sequence(&history, &failure.signature(), 32, |seq| {
                    replay_sequence(&initial_repo, seq)
                });
                let summaries: Vec<String> = minimal.iter().map(Action::summary).collect();
                let repro_failure = CheckFailure::new(
                    failure.oracle,
                    failure.action,
                    format!(
                        "{} (round={round}, round_seed={round_seed:#x})",
                        failure.detail
                    ),
                );
                let repro = format_repro_test(
                    "repro_stacked_store_failure",
                    seed,
                    &repro_failure,
                    &summaries,
                );
                panic!(
                    "stateful stacked-store oracle failed (round={round}, seed={seed:#x}, round_seed={round_seed:#x}): {failure}\n\n{repro}\n\n{}",
                    tracker.probe_report("stacked_store")
                );
            }
            if outcome.is_ok() {
                prev_action = Some(name);
            }
        }

        // End-of-round sync + parity check on any remaining stacked lane so every round
        // concludes with a full differential verification against a cold flat store.
        for lane_idx in 0..world.lanes.len() {
            if !world.lanes[lane_idx].stacked {
                continue;
            }
            for act in [
                Action::SyncWorktree { lane_idx },
                Action::QueryAndCheckParity { lane_idx },
            ] {
                let name = act.name();
                let outcome = world.step(&act);
                tracker.record(name, prev_action, &outcome);
                history.push(act);
                if let StepOutcome::CheckFailed(failure) = outcome {
                    let minimal = minimize_sequence(&history, &failure.signature(), 32, |seq| {
                        replay_sequence(&initial_repo, seq)
                    });
                    let summaries: Vec<String> = minimal.iter().map(Action::summary).collect();
                    let repro_failure = CheckFailure::new(
                        failure.oracle,
                        failure.action,
                        format!(
                            "{} (round={round}, round_seed={round_seed:#x})",
                            failure.detail
                        ),
                    );
                    let repro = format_repro_test(
                        "repro_stacked_store_failure",
                        seed,
                        &repro_failure,
                        &summaries,
                    );
                    panic!(
                        "stateful stacked-store oracle failed (round={round}, seed={seed:#x}, round_seed={round_seed:#x}): {failure}\n\n{repro}\n\n{}",
                        tracker.probe_report("stacked_store")
                    );
                }
                if outcome.is_ok() {
                    prev_action = Some(name);
                }
            }
        }
    }

    let elapsed = started.elapsed();
    eprintln!(
        "{}\nelapsed={:.2}s (seed={seed:#x}, rounds={rounds})",
        tracker.probe_report("stacked_store"),
        elapsed.as_secs_f64()
    );
    tracker.assert_actions_exercised(ALL_ACTIONS);
}

#[test]
fn sync_file_from_disk_clears_deleted_untracked_file_without_phantom_tombstone() {
    let mut initial = MicroRepo::new();
    initial.insert(
        "src/base.rs".to_string(),
        "pub fn base_fn(x: i32) -> i32 {\n    x + 1\n}\n".to_string(),
    );
    let mut world = World::new("untracked-delete", &initial);
    assert!(world.step(&Action::BuildDonorAndPublishBase).is_ok());
    assert!(world.step(&Action::ProvisionLane { lane_idx: 0 }).is_ok());

    let new_path = "src/ephemeral.rs".to_string();
    assert!(world
        .step(&Action::LaneCreateFile {
            lane_idx: 0,
            path: new_path.clone(),
            content: "pub fn ephemeral() -> i32 { 42 }\n".to_string(),
        })
        .is_ok());
    assert!(world
        .step(&Action::SyncSingleFile {
            lane_idx: 0,
            path: new_path.clone(),
        })
        .is_ok());
    assert!(world
        .step(&Action::LaneDeleteFile {
            lane_idx: 0,
            path: new_path.clone(),
        })
        .is_ok());
    let outcome = world.step(&Action::SyncSingleFile {
        lane_idx: 0,
        path: new_path,
    });
    assert!(
        outcome.is_ok(),
        "sync_file_from_disk on deleted non-base file must clear delta cleanly: {outcome:?}"
    );
    let stacked = StackedStore::open(&world.lanes[0].root).expect("reopen lane");
    assert!(stacked.unsynced_paths().expect("unsynced").is_empty());
    assert!(
        !stacked.has_delta_changes(),
        "deleted non-base file must not leave a phantom tombstone in delta.sqlite"
    );
}

#[test]
fn minimizer_shrinks_stacked_action_sequence_and_emits_repro() {
    let actions = vec![
        Action::BuildDonorAndPublishBase,
        Action::ProvisionLane { lane_idx: 0 },
        Action::LaneCreateFile {
            lane_idx: 0,
            path: "src/a.rs".to_string(),
            content: "pub fn a() {}\n".to_string(),
        },
        Action::SyncWorktree { lane_idx: 0 },
        Action::GcBaseCache,
        Action::LaneCreateFile {
            lane_idx: 0,
            path: "src/trigger.rs".to_string(),
            content: "pub fn trigger() {}\n".to_string(),
        },
        Action::LaneEditFile {
            lane_idx: 0,
            path: "src/a.rs".to_string(),
            content: "pub fn a() -> i32 { 1 }\n".to_string(),
        },
        Action::QueryAndCheckParity { lane_idx: 0 },
    ];
    let failure = CheckFailure::new(
        "synthetic_stacked_oracle",
        ACT_QUERY_PARITY,
        "trigger + parity fault",
    );
    let sig = failure.signature();
    let minimal = minimize_sequence(&actions, &sig, 64, |seq| {
        let has_trigger = seq.iter().any(|a| {
            matches!(
                a,
                Action::LaneCreateFile { path, .. } if path == "src/trigger.rs"
            )
        });
        let has_query = seq
            .iter()
            .any(|a| matches!(a, Action::QueryAndCheckParity { .. }));
        (has_trigger && has_query).then(|| failure.clone())
    });
    assert_eq!(
        minimal.len(),
        2,
        "expected 2-step minimal repro: {minimal:?}"
    );
    let summaries: Vec<String> = minimal.iter().map(Action::summary).collect();
    let repro = format_repro_test(
        "repro_synthetic_stacked",
        DEFAULT_SEED,
        &failure,
        &summaries,
    );
    assert!(repro.contains("#[test]"));
    assert!(repro.contains("synthetic_stacked_oracle|QueryAndCheckParity"));
    assert!(repro.contains("src/trigger.rs"));
}

#[test]
fn sync_worktree_clears_deleted_staged_non_base_file_without_phantom_tombstone() {
    let mut initial = MicroRepo::new();
    initial.insert(
        "src/base.rs".to_string(),
        "pub fn base_fn(x: i32) -> i32 {\n    x + 1\n}\n".to_string(),
    );
    let mut world = World::new("wt-staged-delete", &initial);
    assert!(world.step(&Action::BuildDonorAndPublishBase).is_ok());
    assert!(world.step(&Action::ProvisionLane { lane_idx: 0 }).is_ok());

    let lane_root = world.lanes[0].root.clone();
    let new_path = "src/staged_ephemeral.rs";
    write_rel(&lane_root, new_path, "pub fn temp_fn() -> i32 { 1 }\n");
    run_git(&lane_root, &["add", new_path]);

    let mut stacked = StackedStore::open(&lane_root).expect("open lane");
    let first = stacked.sync_worktree().expect("sync staged file");
    assert_eq!(first.synced_files, 1);
    assert!(stacked.has_delta_changes());

    // Delete from disk while still staged in git index (`AD` in `git status`),
    // so `changed_paths` still includes `new_path` even though it is absent on disk
    // and never existed in base.
    fs::remove_file(lane_root.join(new_path)).expect("remove staged file");
    let second = stacked
        .sync_worktree()
        .expect("sync_worktree after deleting staged non-base file");
    assert_eq!(
        second.deleted_files, 0,
        "non-base file must not be tombstoned: {second:?}"
    );
    assert_eq!(
        second.cleaned_files, 1,
        "deleted non-base file must be cleared from delta: {second:?}"
    );
    assert!(
        !stacked.has_delta_changes(),
        "sync_worktree must not leave a phantom tombstone for a deleted non-base file"
    );
}

#[test]
fn go_same_package_duplicate_shadowing_matches_cold_rebuild_0xcafebabe13370002() {
    let mut initial = MicroRepo::new();
    initial.insert(
        "go/mod_0.go".to_string(),
        concat!(
            "package micro\n\n",
            "type Worker0 struct {\n",
            "\tValue int\n",
            "}\n\n",
            "func (w *Worker0) Step(x int) int {\n",
            "\treturn helper0(w.Value + x)\n",
            "}\n\n",
            "func Api0(x int) int {\n",
            "\tbase := x + 3\n",
            "\tw := &Worker0{Value: base}\n",
            "\treturn w.Step(helper0(base))\n",
            "}\n\n",
            "func helper0(x int) int {\n",
            "\treturn x + 3\n",
            "}\n",
        )
        .to_string(),
    );

    let actions = [
        Action::BuildDonorAndPublishBase,
        Action::ProvisionLane { lane_idx: 0 },
        Action::LaneRenameFile {
            lane_idx: 0,
            from: "go/mod_0.go".to_string(),
            to: "go/renamed_1_4931.go".to_string(),
        },
        Action::LaneRevertToBase {
            lane_idx: 0,
            path: "go/mod_0.go".to_string(),
        },
        Action::SyncWorktree { lane_idx: 0 },
        Action::QueryAndCheckParity { lane_idx: 0 },
    ];

    let mut world = World::new("go-pkg-shadow-repro", &initial);
    for act in &actions {
        let outcome = world.step(act);
        assert!(
            outcome.is_ok(),
            "Go same-package shadowing reproducer failed at {}: {outcome:?}",
            act.summary()
        );
    }

    // Explicitly verify both base (`go/mod_0.go`) and delta (`go/renamed_1_4931.go`)
    // abstain on shadowed `Worker0.Step`, matching cold flat rebuild.
    let stacked = StackedStore::open(&world.lanes[0].root).expect("reopen lane");
    let cancel = Cancel::new();
    for path in ["go/mod_0.go", "go/renamed_1_4931.go"] {
        let edges = stacked
            .file_edges(path, 0.0, &cancel)
            .expect("file_edges")
            .expect("indexed file")
            .edges;
        assert!(
            !edges
                .iter()
                .any(|e| e.edge_kind == "Calls" && e.target_symbol.ends_with("Worker0.Step")),
            "shadowed Worker0.Step call in {path} must abstain like cold build: {edges:#?}"
        );
    }
}
