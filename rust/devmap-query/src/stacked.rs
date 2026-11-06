//! Glean-style `StackedStore` combining an immutable `chmod 0444` base snapshot
//! (`<common_git_dir>/devmap/bases/gen-<G>/base.sqlite?immutable=1` + lazy mmap
//! `base.sqlite.csr`) with a per-worktree `DeltaStore` (`delta.sqlite`) and
//! `WorktreeOverlay` (`RoaringBitmap` dirty-node masking) (`F1`, `F4`, `F6`,
//! `F10`, `OQ1`, `OQ2`).

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use devmap_extract::model::Extraction;
use devmap_extract::paths::{
    code_graph_path, compact_code_graph_path, csr_path_for_store, registered_common_git_dir,
    registered_worktrees_of_common_dir, repo_map_path, same_registered_repo, stack_delta_path,
    stack_lease_path, state_dir, store_path,
};
use devmap_store::csr::CsrGraphReader;
use devmap_store::delta::{DeltaEdgeRecord, DeltaStore};
use devmap_store::schema::CURRENT_SCHEMA_VERSION;
use devmap_store::seed::{
    discard_staged, prepare_immutable_snapshot, promote_immutable_base_to_store, seed_artifact,
    seed_directory_cow, set_file_readonly, set_file_writable, stage_seed_healing_wal,
};
use devmap_store::{Store, StoredSymbol};
use serde::{Deserialize, Serialize};

use crate::artifacts::write_atomic;
use crate::cas::{blake3_hash, CAS_TOMBSTONE};
use crate::overlay::{OverlayEdge, RoaringBitmap, WorktreeOverlay};

pub const STACK_FORMAT_VERSION: u32 = 1;
pub const DEFAULT_REBASE_DIRTY_FILES: usize = 250;
pub const DEFAULT_REBASE_DELTA_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaseManifest {
    pub format_version: u32,
    pub generation_id: u32,
    pub schema_version: i32,
    pub source_repo_root: String,
    /// The donor's git `HEAD` the snapshot was taken at. Part of the base's
    /// identity: two donors (or one donor rebuilt) can reuse a generation id.
    pub head_sha: String,
    pub created_at_unix_ms: u64,
    pub base_sqlite_sha256: String,
    /// `(len, mtime_ns, ino, dev)` of `base.sqlite` at publish. `open` runs per
    /// MCP call, so it compares these instead of re-hashing the file; a swap
    /// by rename changes the inode.
    pub base_sqlite_len: u64,
    pub base_sqlite_mtime_ns: u64,
    pub base_sqlite_ino: u64,
    pub base_sqlite_dev: u64,
    #[serde(default)]
    pub has_csr: bool,
}

/// What names a published base: generation id, donor `HEAD`, donor root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseKey {
    pub generation_id: u32,
    pub head_sha: String,
    pub source_repo_root: String,
}

impl BaseKey {
    pub fn new(generation_id: u32, head_sha: &str, source_root: &Path) -> Result<Self> {
        if !is_commit_sha(head_sha) {
            bail!(
                "donor generation {generation_id} carries head_sha {head_sha:?}, not a git commit; \
                 a stacked base must be taken from a built, committed checkout"
            );
        }
        Ok(Self {
            generation_id,
            head_sha: head_sha.to_string(),
            source_repo_root: source_root.to_string_lossy().into_owned(),
        })
    }

    /// `gen-<G>-<head12>-<src8>`.
    pub fn dir_name(&self) -> String {
        format!(
            "gen-{}-{}-{}",
            self.generation_id,
            &self.head_sha[..12],
            &sha256_str_hex(&self.source_repo_root)[..8]
        )
    }

    fn of_manifest(manifest: &BaseManifest) -> Result<Self> {
        Self::new(
            manifest.generation_id,
            &manifest.head_sha,
            Path::new(&manifest.source_repo_root),
        )
    }
}

fn is_commit_sha(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Parse `gen-<G>-<head12>-<src8>`; `None` for legacy `gen-<G>` and anything else.
fn parse_base_dir_name(name: &str) -> Option<(u32, String)> {
    let rest = name.strip_prefix("gen-")?;
    let mut parts = rest.split('-');
    let gen_id = parts.next()?.parse::<u32>().ok()?;
    let head12 = parts.next()?;
    let src8 = parts.next()?;
    let hex = |t: &str, n: usize| {
        t.len() == n && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    };
    if parts.next().is_some() || !hex(head12, 12) || !hex(src8, 8) {
        return None;
    }
    Some((gen_id, src8.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackLease {
    pub format_version: u32,
    pub generation_id: u32,
    pub base_dir: String,
    pub worktree_root: String,
    pub leased_at_unix_ms: u64,
    pub rebases_after_dirty_files: usize,
    pub rebases_after_delta_bytes: u64,
}

impl StackLease {
    pub fn base_sqlite_path(&self) -> PathBuf {
        PathBuf::from(&self.base_dir).join("base.sqlite")
    }

    pub fn read_from(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("cannot read stack lease {}", path.display()))?;
        let lease: Self = serde_json::from_str(&raw)
            .with_context(|| format!("cannot parse stack lease {}", path.display()))?;
        Ok(lease)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcBasesReport {
    pub retained_generations: Vec<u32>,
    pub removed_generations: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackedStatus {
    pub worktree_root: String,
    pub base_generation_id: u32,
    pub base_dir: String,
    pub dirty_files: usize,
    pub tombstoned_files: usize,
    pub delta_symbols: usize,
    pub delta_edges: usize,
    pub delta_bytes: u64,
    pub needs_rebase: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackedSyncReport {
    pub synced_files: usize,
    pub deleted_files: usize,
    pub cleaned_files: usize,
    pub rebased_to_standalone: Option<PathBuf>,
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub fn bases_root_dir(common_git_dir: &Path) -> PathBuf {
    common_git_dir.join("devmap").join("bases")
}

pub fn base_generation_dir(common_git_dir: &Path, key: &BaseKey) -> PathBuf {
    bases_root_dir(common_git_dir).join(key.dir_name())
}

/// Publish or reuse an immutable `chmod 0444` base snapshot at
/// `<common_git_dir>/devmap/bases/gen-<G>-<head12>-<src8>/` from `source_root`'s
/// latest generation (`OQ1`).
///
/// The donor must be a built, committed, clean checkout whose latest generation
/// was indexed at its current `HEAD`: those three facts together mean the base
/// holds exactly `HEAD`'s tree, so the delta (worktree against `HEAD`) is the
/// only difference a lane can have from it.
pub fn publish_base_snapshot(source_root: &Path) -> Result<(PathBuf, BaseManifest)> {
    let source_canon = source_root
        .canonicalize()
        .with_context(|| format!("cannot canonicalize {}", source_root.display()))?;
    let common_git_dir = registered_common_git_dir(&source_canon).ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not a registered git checkout; refusing base snapshot publish",
            source_canon.display()
        )
    })?;
    let source_db = store_path(&source_canon);
    if !source_db.is_file() {
        bail!(
            "donor checkout {} has no devmap store at {}",
            source_canon.display(),
            source_db.display()
        );
    }
    let key = {
        let store = Store::open_read_only(&source_db)
            .with_context(|| format!("cannot read donor store {}", source_db.display()))?;
        let generation_id = store.latest_generation_id()?.ok_or_else(|| {
            anyhow::anyhow!(
                "donor store {} has no indexed generation",
                source_db.display()
            )
        })?;
        let head = store.latest_generation_head_sha()?.unwrap_or_default();
        let key = BaseKey::new(generation_id, &head, &source_canon)?;
        assert_donor_is_head_tree(&source_canon, &store, &key.head_sha)?;
        key
    };

    let bases_dir = bases_root_dir(&common_git_dir);
    fs::create_dir_all(&bases_dir)
        .with_context(|| format!("cannot create {}", bases_dir.display()))?;
    let final_dir = base_generation_dir(&common_git_dir, &key);
    if fs::symlink_metadata(&final_dir).is_ok() {
        let existing = verify_base_dir(&final_dir, &key, true)?;
        return Ok((final_dir, existing));
    }

    let stage_dir = bases_dir.join(format!(
        ".stage-{}-{}-{}",
        key.dir_name(),
        std::process::id(),
        now_unix_ms()
    ));
    let _ = remove_readonly_tree(&stage_dir);
    fs::create_dir_all(&stage_dir)
        .with_context(|| format!("cannot create {}", stage_dir.display()))?;

    let staged_res = (|| -> Result<BaseManifest> {
        let staged_sqlite = stage_dir.join("base.sqlite");
        stage_seed_healing_wal(&source_db, &staged_sqlite)?;
        prepare_immutable_snapshot(&staged_sqlite)?;
        // The donor may have been edited or rebuilt while it was staged:
        // re-ask both questions, and ask the staged copy what it holds.
        {
            let staged = Store::open_immutable_snapshot(&staged_sqlite, None)
                .context("staged base snapshot failed admission")?;
            let staged_gen = staged.latest_generation_id()?;
            let staged_head = staged.latest_generation_head_sha()?;
            if staged_gen != Some(key.generation_id)
                || staged_head.as_deref() != Some(key.head_sha.as_str())
            {
                bail!(
                    "donor {} changed while its base was being staged (expected generation {} at {}, staged copy holds {:?} at {:?}); retry",
                    source_canon.display(),
                    key.generation_id,
                    key.head_sha,
                    staged_gen,
                    staged_head
                );
            }
        }
        assert_donor_is_clean_head(&source_canon, &key.head_sha)?;
        let sha256 = sha256_file_hex(&staged_sqlite)?;
        let identity = file_identity(&staged_sqlite)?;

        let src_csr = csr_path_for_store(&source_db);
        let staged_csr = csr_path_for_store(&staged_sqlite);
        let has_csr = if src_csr.is_file() {
            seed_artifact(&src_csr, &staged_csr)?.is_some() && {
                set_file_readonly(&staged_csr)?;
                true
            }
        } else {
            false
        };

        let manifest = BaseManifest {
            format_version: STACK_FORMAT_VERSION,
            generation_id: key.generation_id,
            schema_version: CURRENT_SCHEMA_VERSION,
            source_repo_root: key.source_repo_root.clone(),
            head_sha: key.head_sha.clone(),
            created_at_unix_ms: now_unix_ms(),
            base_sqlite_sha256: sha256,
            base_sqlite_len: identity.len,
            base_sqlite_mtime_ns: identity.mtime_ns,
            base_sqlite_ino: identity.ino,
            base_sqlite_dev: identity.dev,
            has_csr,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        write_atomic(&stage_dir.join("manifest.json"), &manifest_bytes)?;
        set_file_readonly(&stage_dir.join("manifest.json"))?;
        Ok(manifest)
    })();

    let manifest = match staged_res {
        Ok(m) => m,
        Err(err) => {
            let _ = remove_readonly_tree(&stage_dir);
            return Err(err);
        }
    };

    match fs::rename(&stage_dir, &final_dir) {
        Ok(()) => {
            // A swap by rename now needs an explicit chmod first.
            set_dir_mode(&final_dir, 0o555)?;
            Ok((final_dir, manifest))
        }
        Err(err) => {
            let _ = remove_readonly_tree(&stage_dir);
            if fs::symlink_metadata(&final_dir).is_ok() {
                // A concurrent publisher won the rename: it must be the same key.
                let existing = verify_base_dir(&final_dir, &key, true)?;
                Ok((final_dir, existing))
            } else {
                Err(err).with_context(|| {
                    format!(
                        "cannot publish staged base snapshot to {}",
                        final_dir.display()
                    )
                })
            }
        }
    }
}

#[cfg(unix)]
fn set_dir_mode(dir: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set mode {mode:04o} on {}", dir.display()))
}

#[cfg(not(unix))]
fn set_dir_mode(_dir: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

struct FileIdentity {
    len: u64,
    mtime_ns: u64,
    ino: u64,
    dev: u64,
}

fn file_identity(path: &Path) -> Result<FileIdentity> {
    let meta =
        fs::symlink_metadata(path).with_context(|| format!("cannot stat {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mtime_ns = u64::try_from(meta.mtime())
            .unwrap_or(0)
            .saturating_mul(1_000_000_000)
            .saturating_add(u64::try_from(meta.mtime_nsec()).unwrap_or(0));
        Ok(FileIdentity {
            len: meta.len(),
            mtime_ns,
            ino: meta.ino(),
            dev: meta.dev(),
        })
    }
    #[cfg(not(unix))]
    {
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0);
        Ok(FileIdentity {
            len: meta.len(),
            mtime_ns,
            ino: 0,
            dev: 0,
        })
    }
}

fn read_base_manifest(base_dir: &Path) -> Result<BaseManifest> {
    let manifest_path = base_dir.join("manifest.json");
    let raw = fs::read(&manifest_path).with_context(|| {
        format!(
            "cannot read base manifest {}; re-run `devmap seed --stacked`",
            manifest_path.display()
        )
    })?;
    serde_json::from_slice(&raw).with_context(|| {
        format!(
            "base manifest {} is legacy or malformed; re-run `devmap seed --stacked`",
            manifest_path.display()
        )
    })
}

/// Compare a base's recorded identity with the one a caller expects. Never
/// returns `Ok` on a mismatch and never republishes over one: a different value
/// under the expected name is tampering or a bug.
fn check_manifest_matches(manifest: &BaseManifest, expected: &BaseKey, dir: &Path) -> Result<()> {
    let mismatch = |field: &str, recorded: String, wanted: String| {
        anyhow::anyhow!(
            "base snapshot {} does not match its key: {field} is {recorded} but {wanted} was expected; \
             refusing to reuse it (remove it and re-run `devmap seed --stacked`)",
            dir.display()
        )
    };
    if manifest.format_version != STACK_FORMAT_VERSION {
        return Err(mismatch(
            "format_version",
            manifest.format_version.to_string(),
            STACK_FORMAT_VERSION.to_string(),
        ));
    }
    if manifest.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(mismatch(
            "schema_version",
            manifest.schema_version.to_string(),
            CURRENT_SCHEMA_VERSION.to_string(),
        ));
    }
    if manifest.generation_id != expected.generation_id {
        return Err(mismatch(
            "generation_id",
            manifest.generation_id.to_string(),
            expected.generation_id.to_string(),
        ));
    }
    if manifest.head_sha != expected.head_sha {
        return Err(mismatch(
            "head_sha",
            manifest.head_sha.clone(),
            expected.head_sha.clone(),
        ));
    }
    if manifest.source_repo_root != expected.source_repo_root {
        return Err(mismatch(
            "source_repo_root",
            manifest.source_repo_root.clone(),
            expected.source_repo_root.clone(),
        ));
    }
    Ok(())
}

fn check_identity_matches(manifest: &BaseManifest, base_sqlite: &Path) -> Result<()> {
    let now = file_identity(base_sqlite)?;
    let recorded = (
        manifest.base_sqlite_len,
        manifest.base_sqlite_mtime_ns,
        manifest.base_sqlite_ino,
        manifest.base_sqlite_dev,
    );
    let current = (now.len, now.mtime_ns, now.ino, now.dev);
    if recorded != current {
        bail!(
            "base snapshot {} was replaced or modified after it was published \
             (recorded len/mtime_ns/ino/dev {recorded:?}, now {current:?}); \
             remove it and re-run `devmap seed --stacked`",
            base_sqlite.display()
        );
    }
    Ok(())
}

/// Reuse check for a base directory that already exists under `expected`'s
/// name: manifest fields, the recomputed sha256 of `base.sqlite` (when
/// `full_hash`), and SQLite admission. `Err` on any disagreement.
fn verify_base_dir(base_dir: &Path, expected: &BaseKey, full_hash: bool) -> Result<BaseManifest> {
    let manifest = read_base_manifest(base_dir)?;
    check_manifest_matches(&manifest, expected, base_dir)?;
    let base_sqlite = base_dir.join("base.sqlite");
    if full_hash {
        let actual = sha256_file_hex(&base_sqlite)?;
        if actual != manifest.base_sqlite_sha256 {
            bail!(
                "base snapshot {} fails its sha256 check: recorded {} but the file hashes to {actual}; \
                 refusing to reuse it (remove it and re-run `devmap seed --stacked`)",
                base_sqlite.display(),
                manifest.base_sqlite_sha256
            );
        }
    }
    // Verify the snapshot satisfies Store::open_immutable_snapshot invariants (`0444`, no sidecars).
    let _ = Store::open_immutable_snapshot(&base_sqlite, None).with_context(|| {
        format!(
            "published base snapshot {} failed admission",
            base_sqlite.display()
        )
    })?;
    Ok(manifest)
}

const GIT_BOUNDS: devmap_extract::subprocess::Bounds = devmap_extract::subprocess::Bounds {
    deadline: std::time::Duration::from_secs(10),
    stdout_cap: 8 * 1024 * 1024,
    stderr_cap: 64 * 1024,
};

/// Run git in `root` under bounds. Every failure (spawn, deadline, non-success,
/// output cap) is an `Err`: a git that could not answer must never read as an
/// empty change set.
fn run_git_checked(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    use devmap_extract::subprocess::{git, run_bounded};
    let mut cmd = git(root, args.iter().copied())
        .map_err(|e| anyhow::anyhow!("git {args:?} in {}: {e}", root.display()))?;
    let out = run_bounded(&mut cmd, GIT_BOUNDS)
        .map_err(|e| anyhow::anyhow!("git {args:?} in {}: {e}", root.display()))?;
    if !out.status.success() {
        bail!(
            "git {args:?} failed in {} ({}): {}",
            root.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    if out.stdout_truncated {
        bail!(
            "git {args:?} in {} produced more than {} bytes; the answer would be incomplete",
            root.display(),
            GIT_BOUNDS.stdout_cap
        );
    }
    Ok(out.stdout)
}

fn git_head_sha(root: &Path) -> Result<String> {
    let out = run_git_checked(root, &["rev-parse", "HEAD"])?;
    let head = String::from_utf8(out)
        .map_err(|_| anyhow::anyhow!("git rev-parse HEAD in {} is not UTF-8", root.display()))?
        .trim()
        .to_string();
    if !is_commit_sha(&head) {
        bail!("git rev-parse HEAD in {} returned {head:?}", root.display());
    }
    Ok(head)
}

/// Repository-relative paths `git status` lists in `root`, tracked and
/// untracked, with rename and copy origins. A non-UTF-8 path is an `Err`.
fn git_status_paths(root: &Path) -> Result<BTreeSet<String>> {
    let out = run_git_checked(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut paths = BTreeSet::new();
    let mut entries = out.split(|&b| b == 0);
    while let Some(entry) = entries.next() {
        if entry.is_empty() {
            continue;
        }
        if entry.len() < 4 {
            bail!(
                "git status in {} returned a malformed entry {:?}",
                root.display(),
                String::from_utf8_lossy(entry)
            );
        }
        let status = &entry[..2];
        paths.insert(status_path(root, &entry[3..])?);
        if status.contains(&b'R') || status.contains(&b'C') {
            let orig = entries.next().ok_or_else(|| {
                anyhow::anyhow!(
                    "git status in {} lists a rename with no origin path",
                    root.display()
                )
            })?;
            paths.insert(status_path(root, orig)?);
        }
    }
    Ok(paths)
}

fn status_path(root: &Path, raw: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(raw).map_err(|_| {
        anyhow::anyhow!(
            "git lists a non-UTF-8 path in {} ({:?}); it cannot be tracked in a stacked delta",
            root.display(),
            String::from_utf8_lossy(raw)
        )
    })?;
    normalize_rel_path(text)
}

fn is_tool_state_path(path: &str) -> bool {
    path == ".devmap"
        || path == ".devcouncil"
        || path.starts_with(".devmap/")
        || path.starts_with(".devcouncil/")
}

/// Refuse unless `source`'s `HEAD` is `expected_head` and its tracked and
/// untracked files are clean against it (tool-owned state excepted).
fn assert_donor_is_clean_head(source: &Path, expected_head: &str) -> Result<()> {
    let remedy = format!(
        "commit or stash in {}, run devmap build there, or pass --from a clean checkout",
        source.display()
    );
    let head = git_head_sha(source)?;
    if head != expected_head {
        bail!(
            "donor {} is at HEAD {head} but its index was built at {expected_head}; {remedy}",
            source.display()
        );
    }
    let dirty: Vec<String> = git_status_paths(source)?
        .into_iter()
        .filter(|p| !is_tool_state_path(p))
        .collect();
    if !dirty.is_empty() {
        let shown: Vec<&str> = dirty.iter().take(8).map(String::as_str).collect();
        bail!(
            "donor {} has {} uncommitted path(s) ({}{}); a stacked base must equal HEAD's tree; {remedy}",
            source.display(),
            dirty.len(),
            shown.join(", "),
            if dirty.len() > shown.len() { ", ..." } else { "" }
        );
    }
    Ok(())
}

/// The three facts that together mean "the base's indexed content is `HEAD`'s
/// tree": the donor is clean against `HEAD`, its latest generation matches the
/// working tree, and that generation's `head_sha` is `HEAD`.
fn assert_donor_is_head_tree(source: &Path, store: &Store, generation_head: &str) -> Result<()> {
    let remedy = format!(
        "commit or stash in {}, run devmap build there, or pass --from a clean checkout",
        source.display()
    );
    assert_donor_is_clean_head(source, generation_head)?;
    match store.latest_generation_repo_root()? {
        Some(root) if Path::new(&root).canonicalize().ok().as_deref() == Some(source) => {}
        other => bail!(
            "donor store at {} was indexed for {other:?}, not for this checkout; {remedy}",
            source.display()
        ),
    }
    #[cfg(feature = "parse")]
    match store.latest_generation_matches_working_tree() {
        Ok(true) => Ok(()),
        Ok(false) => bail!(
            "donor {} has changed since its index was built (the latest generation does not match the working tree); {remedy}",
            source.display()
        ),
        Err(err) => bail!(
            "cannot confirm donor {}'s index matches its working tree: {err}; {remedy}",
            source.display()
        ),
    }
    #[cfg(not(feature = "parse"))]
    bail!(
        "this build cannot confirm donor {}'s index matches its working tree (parse feature off); {remedy}",
        source.display()
    )
}

/// Provision a Glean-style stacked worktree lane at `worktree_root` backed by
/// `source_root`'s immutable base snapshot (`P2.1`, `P2.2`, Critique 1).
pub fn provision_stacked_worktree(source_root: &Path, worktree_root: &Path) -> Result<StackLease> {
    let source_canon = source_root
        .canonicalize()
        .with_context(|| format!("cannot canonicalize {}", source_root.display()))?;
    let wt_canon = worktree_root
        .canonicalize()
        .with_context(|| format!("cannot canonicalize {}", worktree_root.display()))?;
    if !same_registered_repo(&source_canon, &wt_canon) {
        bail!(
            "refusing stacked provisioning: {} and {} are not registered worktrees of the same repository",
            source_canon.display(),
            wt_canon.display()
        );
    }

    let (base_dir, manifest) = publish_base_snapshot(&source_canon)?;
    let delta_path = stack_delta_path(&wt_canon);
    let _delta = DeltaStore::open(&delta_path)?;

    let lease = StackLease {
        format_version: STACK_FORMAT_VERSION,
        generation_id: manifest.generation_id,
        base_dir: base_dir.to_string_lossy().into_owned(),
        worktree_root: wt_canon.to_string_lossy().into_owned(),
        leased_at_unix_ms: now_unix_ms(),
        rebases_after_dirty_files: DEFAULT_REBASE_DIRTY_FILES,
        rebases_after_delta_bytes: DEFAULT_REBASE_DELTA_BYTES,
    };
    let lease_bytes = serde_json::to_vec_pretty(&lease)?;
    write_atomic(&stack_lease_path(&wt_canon), &lease_bytes)?;

    seed_derived_artifacts_and_reuse_caches(&source_canon, &wt_canon)?;

    Ok(lease)
}

/// Clone warm derived artifacts (`repo_map.json`, `code_graph.json`,
/// `compact_code_graph.json`) and any `resolution-reuse-*` directory from
/// `source_root` to `worktree_root` without copying `<db>.artifacts.json` (`F9`).
pub fn seed_derived_artifacts_and_reuse_caches(
    source_root: &Path,
    worktree_root: &Path,
) -> Result<()> {
    let _ = seed_artifact(&repo_map_path(source_root), &repo_map_path(worktree_root))?;
    let _ = seed_artifact(
        &code_graph_path(source_root),
        &code_graph_path(worktree_root),
    )?;
    let _ = seed_artifact(
        &compact_code_graph_path(source_root),
        &compact_code_graph_path(worktree_root),
    )?;
    let src_state = state_dir(source_root);
    let dst_state = state_dir(worktree_root);
    if let Ok(entries) = fs::read_dir(&src_state) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(s) = name.to_str() {
                if s.starts_with("resolution-reuse-") && entry.path().is_dir() {
                    let _ = seed_directory_cow(&entry.path(), &dst_state.join(s))?;
                }
            }
        }
    }
    Ok(())
}

/// Garbage-collect unleased `gen-<old>` snapshots under `<common_git_dir>/devmap/bases/`.
///
/// Fails closed (`Err`) without deleting any base directory if ANY registered
/// worktree carries an unreadable or malformed `stack_lease.json` (`OQ2`).
pub fn gc_unused_bases(common_git_dir: &Path) -> Result<GcBasesReport> {
    let common_canon = common_git_dir
        .canonicalize()
        .with_context(|| format!("cannot canonicalize {}", common_git_dir.display()))?;
    let bases_dir = bases_root_dir(&common_canon);
    if !bases_dir.is_dir() {
        return Ok(GcBasesReport {
            retained_generations: Vec::new(),
            removed_generations: Vec::new(),
        });
    }

    let mut leased_dirs: BTreeSet<String> = BTreeSet::new();
    for wt_root in registered_worktrees_of_common_dir(&common_canon) {
        let lease_file = stack_lease_path(&wt_root);
        match fs::symlink_metadata(&lease_file) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => {
                bail!(
                    "refusing base GC: cannot inspect lease {} ({err})",
                    lease_file.display()
                );
            }
            Ok(meta) if !meta.is_file() => {
                bail!(
                    "refusing base GC: lease {} is not a regular file",
                    lease_file.display()
                );
            }
            Ok(_) => {}
        }
        let raw = fs::read(&lease_file).with_context(|| {
            format!(
                "refusing base GC: cannot read worktree lease {}",
                lease_file.display()
            )
        })?;
        let lease: StackLease = serde_json::from_slice(&raw).with_context(|| {
            format!(
                "refusing base GC: malformed stack_lease.json at {}",
                lease_file.display()
            )
        })?;
        if lease.format_version != STACK_FORMAT_VERSION {
            bail!(
                "refusing base GC: unsupported stack_lease format_version {} at {}",
                lease.format_version,
                lease_file.display()
            );
        }
        if let Some(name) = Path::new(&lease.base_dir)
            .file_name()
            .and_then(|n| n.to_str())
        {
            leased_dirs.insert(name.to_string());
        }
    }

    // Only `gen-<G>-<head12>-<src8>` directories are ours to judge; legacy
    // `gen-<G>` directories and anything else are left alone.
    let mut discovered: Vec<(u32, String, String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&bases_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if let Some((gen_id, src8)) = parse_base_dir_name(name_str) {
            discovered.push((gen_id, src8, name_str.to_string(), entry.path()));
        }
    }
    discovered.sort_by(|a, b| (a.0, &a.2).cmp(&(b.0, &b.2)));
    // The newest generation of each donor stays published.
    let mut newest_per_source: std::collections::BTreeMap<String, u32> =
        std::collections::BTreeMap::new();
    for (gen_id, src8, _, _) in &discovered {
        let slot = newest_per_source.entry(src8.clone()).or_insert(*gen_id);
        *slot = (*slot).max(*gen_id);
    }

    let mut retained_generations = Vec::new();
    let mut removed_generations = Vec::new();
    for (gen_id, src8, name, dir_path) in discovered {
        if leased_dirs.contains(&name) || newest_per_source.get(&src8) == Some(&gen_id) {
            retained_generations.push(gen_id);
        } else {
            remove_readonly_tree(&dir_path)
                .with_context(|| format!("cannot remove unleased base {}", dir_path.display()))?;
            removed_generations.push(gen_id);
        }
    }

    Ok(GcBasesReport {
        retained_generations,
        removed_generations,
    })
}

fn remove_readonly_tree(dir: &Path) -> Result<()> {
    if fs::symlink_metadata(dir).is_err() {
        return Ok(());
    }
    make_tree_writable_best_effort(dir);
    fs::remove_dir_all(dir)?;
    Ok(())
}

fn make_tree_writable_best_effort(path: &Path) {
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
                make_tree_writable_best_effort(&entry.path());
            }
        }
    } else {
        let _ = set_file_writable(path);
    }
}

/// The per-open check: cheap enough for every MCP call, strong enough to catch
/// a legacy directory, a lease pointing at another base, and a swapped file.
/// The full sha256 is verified when a base is reused at publish.
fn verify_leased_base(lease: &StackLease, base_dir: &Path, base_sqlite: &Path) -> Result<()> {
    let manifest = read_base_manifest(base_dir)?;
    let key = BaseKey::of_manifest(&manifest)
        .with_context(|| format!("base manifest in {} is invalid", base_dir.display()))?;
    check_manifest_matches(&manifest, &key, base_dir)?;
    if manifest.generation_id != lease.generation_id {
        bail!(
            "stack lease names generation {} but base {} holds generation {}; re-run `devmap seed --stacked`",
            lease.generation_id,
            base_dir.display(),
            manifest.generation_id
        );
    }
    let dir_name = base_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if dir_name != key.dir_name() {
        bail!(
            "base directory {} is not named for its manifest (expected {}); re-run `devmap seed --stacked`",
            base_dir.display(),
            key.dir_name()
        );
    }
    check_identity_matches(&manifest, base_sqlite)
}

pub struct StackedStore {
    worktree_root: PathBuf,
    lease: StackLease,
    base_sqlite_path: PathBuf,
    base_csr_path: Option<PathBuf>,
    base: Store,
    csr: Mutex<Option<Arc<CsrGraphReader>>>,
    delta: DeltaStore,
    overlay: WorktreeOverlay,
}

impl std::fmt::Debug for StackedStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StackedStore")
            .field("worktree_root", &self.worktree_root)
            .field("generation_id", &self.lease.generation_id)
            .field("dirty_files", &self.overlay.modified_files.len())
            .finish()
    }
}

impl StackedStore {
    /// Open the stacked worktree store declared by `<worktree_root>/<state>/codeintel/stack_lease.json`.
    pub fn open(worktree_root: &Path) -> Result<Self> {
        let wt_canon = worktree_root
            .canonicalize()
            .with_context(|| format!("cannot canonicalize {}", worktree_root.display()))?;
        let lease_path = stack_lease_path(&wt_canon);
        let raw_lease = fs::read(&lease_path)
            .with_context(|| format!("cannot read stack lease {}", lease_path.display()))?;
        let lease: StackLease = serde_json::from_slice(&raw_lease)
            .with_context(|| format!("invalid stack lease {}", lease_path.display()))?;
        if lease.format_version != STACK_FORMAT_VERSION {
            bail!(
                "unsupported stack_lease format_version {} in {}",
                lease.format_version,
                lease_path.display()
            );
        }

        let base_dir = PathBuf::from(&lease.base_dir);
        let base_sqlite_path = base_dir.join("base.sqlite");
        verify_leased_base(&lease, &base_dir, &base_sqlite_path)?;
        let wt_root_str = wt_canon.to_string_lossy().into_owned();
        let base = Store::open_immutable_snapshot(&base_sqlite_path, Some(wt_root_str.clone()))
            .with_context(|| {
                format!(
                    "cannot open immutable base snapshot {}",
                    base_sqlite_path.display()
                )
            })?;

        let csr_candidate = csr_path_for_store(&base_sqlite_path);
        let base_csr_path = csr_candidate.is_file().then_some(csr_candidate);
        let delta = DeltaStore::open(stack_delta_path(&wt_canon))?;
        let overlay = build_overlay_from_delta(&wt_root_str, lease.generation_id, &base, &delta)?;

        Ok(Self {
            worktree_root: wt_canon,
            lease,
            base_sqlite_path,
            base_csr_path,
            base,
            csr: Mutex::new(None),
            delta,
            overlay,
        })
    }

    pub fn open_if_leased(worktree_root: &Path) -> Result<Option<Self>> {
        if !stack_lease_path(worktree_root).is_file() {
            return Ok(None);
        }
        Self::open(worktree_root).map(Some)
    }

    pub fn worktree_root(&self) -> &Path {
        &self.worktree_root
    }

    pub fn lease(&self) -> &StackLease {
        &self.lease
    }

    pub fn base_store(&self) -> &Store {
        &self.base
    }

    pub fn delta_store(&self) -> &DeltaStore {
        &self.delta
    }

    pub fn overlay(&self) -> &WorktreeOverlay {
        &self.overlay
    }

    /// Lazily map `base.sqlite.csr` on first traversal (`F6` — zero eager edge scan at open).
    pub fn csr_reader(&self) -> Result<Option<Arc<CsrGraphReader>>> {
        let mut guard = self
            .csr
            .lock()
            .map_err(|_| anyhow::anyhow!("StackedStore CSR mutex poisoned"))?;
        if let Some(ref reader) = *guard {
            return Ok(Some(Arc::clone(reader)));
        }
        let Some(ref csr_path) = self.base_csr_path else {
            return Ok(None);
        };
        let reader = Arc::new(
            CsrGraphReader::open_mmap(csr_path)
                .map_err(|e| anyhow::anyhow!("cannot mmap {}: {e}", csr_path.display()))?,
        );
        *guard = Some(Arc::clone(&reader));
        Ok(Some(reader))
    }

    /// Record a parsed `Extraction` for `rel_path` in the worktree's private
    /// `delta.sqlite` and refresh the `WorktreeOverlay` (`F4`).
    ///
    /// Never invokes regex fallback and never writes to any shared/anchor CAS.
    pub fn record_extracted_file(
        &mut self,
        rel_path: &str,
        source_bytes: &[u8],
        extraction: &Extraction,
    ) -> Result<()> {
        let normalized = normalize_rel_path(rel_path)?;
        let cas_hash = blake3_hash(source_bytes);
        let edges = extract_delta_edges(&normalized, extraction);
        self.delta.upsert_extracted_file(
            &normalized,
            &cas_hash,
            extraction,
            &edges,
            now_unix_ms(),
        )?;
        self.reload_overlay()
    }

    /// Mark `rel_path` as deleted in the worktree's private `delta.sqlite` and
    /// mask all of its base symbols in `WorktreeOverlay` (`F4`).
    pub fn record_deleted_file(&mut self, rel_path: &str) -> Result<()> {
        let normalized = normalize_rel_path(rel_path)?;
        self.delta
            .tombstone_file(&normalized, CAS_TOMBSTONE, now_unix_ms())?;
        self.reload_overlay()
    }

    /// Clear `rel_path` from `delta.sqlite` when it matches the base snapshot again.
    pub fn clear_reverted_file(&mut self, rel_path: &str) -> Result<()> {
        let normalized = normalize_rel_path(rel_path)?;
        self.delta.remove_clean_file(&normalized)?;
        self.reload_overlay()
    }

    /// Re-extract `rel_path` from `<worktree_root>/<rel_path>` using the real
    /// Tree-sitter frontend (`devmap_extract::extract_file`) — never regex!
    #[cfg(feature = "parse")]
    pub fn sync_file_from_disk(&mut self, rel_path: &str) -> Result<()> {
        let normalized = normalize_rel_path(rel_path)?;
        if devmap_extract::languages::is_ignored_path(&normalized) {
            return Ok(());
        }
        let full = self.worktree_root.join(&normalized);
        match fs::symlink_metadata(&full) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.record_deleted_file(&normalized)
            }
            Err(err) => Err(err).with_context(|| format!("cannot stat {}", full.display())),
            Ok(meta) if !meta.is_file() => self.record_deleted_file(&normalized),
            Ok(_) => {
                if !devmap_extract::languages::is_indexable_source(&normalized) {
                    return Ok(());
                }
                let bytes =
                    fs::read(&full).with_context(|| format!("cannot read {}", full.display()))?;
                let Ok(source_str) = std::str::from_utf8(&bytes) else {
                    return Ok(());
                };
                if let Some(base_file) = self.base.latest_file(&normalized)? {
                    let lang = devmap_extract::languages::detect_language(Path::new(&normalized));
                    let key = devmap_extract::cache::CacheKey::for_source(lang, source_str);
                    if key.content_hash == base_file.content_hash {
                        return self.clear_reverted_file(&normalized);
                    }
                }
                let extraction = devmap_extract::extract_file(&normalized, source_str);
                self.record_extracted_file(&normalized, &bytes, &extraction)
            }
        }
    }

    /// Synchronize uncommitted/diverged git paths in `worktree_root` into `delta.sqlite`
    /// with real Tree-sitter extraction, auto-rebasing to a standalone CoW
    /// `devmap.sqlite` when thresholds (`250` dirty files / `16 MiB`) are exceeded.
    #[cfg(feature = "parse")]
    pub fn sync_worktree(&mut self) -> Result<StackedSyncReport> {
        let changed_paths = git_changed_paths_against_head(&self.worktree_root, &self.base)?;
        let existing_delta: BTreeSet<String> = self
            .delta
            .load_snapshot()?
            .files
            .into_iter()
            .map(|f| f.path)
            .collect();

        let mut synced_files = 0usize;
        let mut deleted_files = 0usize;
        let mut cleaned_files = 0usize;

        for rel_path in &changed_paths {
            if devmap_extract::languages::is_ignored_path(rel_path) {
                continue;
            }
            let full = self.worktree_root.join(rel_path);
            if full.is_file() {
                if !devmap_extract::languages::is_indexable_source(rel_path) {
                    continue;
                }
                self.sync_file_from_disk(rel_path)?;
                synced_files += 1;
            } else if self.base.latest_path_is_indexed(rel_path)?
                || existing_delta.contains(rel_path)
            {
                self.record_deleted_file(rel_path)?;
                deleted_files += 1;
            }
        }

        for prev in existing_delta {
            if changed_paths.contains(&prev) {
                continue;
            }
            let full = self.worktree_root.join(&prev);
            if !full.is_file() {
                continue;
            }
            let bytes = match fs::read(&full) {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    return Err(err).with_context(|| format!("cannot read {}", full.display()))
                }
            };
            let src = std::str::from_utf8(&bytes)
                .with_context(|| format!("{} is not valid UTF-8", full.display()))?;
            if let Some(base_file) = self.base.latest_file(&prev)? {
                let lang = devmap_extract::languages::detect_language(Path::new(&prev));
                let key = devmap_extract::cache::CacheKey::for_source(lang, src);
                if key.content_hash == base_file.content_hash {
                    self.clear_reverted_file(&prev)?;
                    cleaned_files += 1;
                }
            }
        }

        let rebased_to_standalone = if self.needs_rebase()? {
            Some(self.materialize_seeded_store()?)
        } else {
            None
        };

        Ok(StackedSyncReport {
            synced_files,
            deleted_files,
            cleaned_files,
            rebased_to_standalone,
        })
    }

    pub fn needs_rebase(&self) -> Result<bool> {
        let dirty = self.delta.dirty_file_count()?;
        let bytes = self.delta.bytes_on_disk();
        Ok(dirty > self.lease.rebases_after_dirty_files
            || bytes > self.lease.rebases_after_delta_bytes)
    }

    /// Promote this stacked worktree into a standalone writable CoW `devmap.sqlite`
    /// and retire `stack_lease.json` + `delta.sqlite` (`OQ2`).
    pub fn materialize_seeded_store(&self) -> Result<PathBuf> {
        self.materialize_seeded_store_waiting(Store::WRITER_LOCK_WAIT)
    }

    /// [`Self::materialize_seeded_store`] with the writer-lock wait named, so a
    /// test can watch the refusal without waiting out a minute.
    pub fn materialize_seeded_store_waiting(&self, wait: std::time::Duration) -> Result<PathBuf> {
        let target_db = store_path(&self.worktree_root);
        if let Some(parent) = target_db.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        // Another writer (a build, a drain) may be placing this store: hold the
        // same lock they hold for the whole promotion.
        let _writer = Store::lock_writer_at(&target_db, wait)?;
        if fs::symlink_metadata(&target_db).is_ok() {
            bail!(
                "refusing to materialize over an existing store {}",
                target_db.display()
            );
        }
        promote_immutable_base_to_store(&self.base_sqlite_path, &target_db, &self.worktree_root)?;
        if let Some(ref src_csr) = self.base_csr_path {
            let dst_csr = csr_path_for_store(&target_db);
            seed_artifact(src_csr, &dst_csr)?;
            if dst_csr.is_file() {
                set_file_writable(&dst_csr)?;
            }
        }
        match fs::remove_file(stack_lease_path(&self.worktree_root)) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "store placed at {} but the stack lease could not be removed",
                        target_db.display()
                    )
                })
            }
        }
        discard_staged(self.delta.path());
        Ok(target_db)
    }

    pub fn symbols_for_file(&self, rel_path: &str) -> Result<Vec<StoredSymbol>> {
        let normalized = normalize_rel_path(rel_path)?;
        let path_ref = Path::new(&normalized);
        if self.overlay.is_file_tombstoned(path_ref) {
            return Ok(Vec::new());
        }
        if let Some(ext) = self.overlay.added_extractions.get(&normalized) {
            return Ok(ext
                .symbols
                .iter()
                .map(|s| StoredSymbol {
                    name: s.name.clone(),
                    qualified_name: s.qualified_name.clone(),
                    kind: s.kind.as_str().to_string(),
                    path: normalized.clone(),
                    span_start: s.span.start_byte,
                    span_end: s.span.end_byte,
                    is_exported: s.is_exported,
                    content_hash: ext.content_hash,
                })
                .collect());
        }
        let Some(ext) = self.base.latest_extraction_for_path(&normalized)? else {
            return Ok(Vec::new());
        };
        Ok(ext
            .symbols
            .into_iter()
            .filter(|s| !self.overlay.is_symbol_tombstoned(&s.qualified_name))
            .map(|s| StoredSymbol {
                name: s.name,
                qualified_name: s.qualified_name,
                kind: s.kind.as_str().to_string(),
                path: normalized.clone(),
                span_start: s.span.start_byte,
                span_end: s.span.end_byte,
                is_exported: s.is_exported,
                content_hash: ext.content_hash,
            })
            .collect())
    }

    pub fn search_symbols(&self, query: &str, limit: usize) -> Result<Vec<StoredSymbol>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let needle = query.trim().to_ascii_lowercase();
        let mut results = Vec::new();
        let mut seen = BTreeSet::new();

        let mut modified_paths: Vec<&String> = self.overlay.added_extractions.keys().collect();
        modified_paths.sort();
        for path in modified_paths {
            if let Some(ext) = self.overlay.added_extractions.get(path) {
                for sym in &ext.symbols {
                    if needle.is_empty()
                        || sym.name.to_ascii_lowercase().contains(&needle)
                        || sym.qualified_name.to_ascii_lowercase().contains(&needle)
                    {
                        if seen.insert((path.clone(), sym.qualified_name.clone())) {
                            results.push(StoredSymbol {
                                name: sym.name.clone(),
                                qualified_name: sym.qualified_name.clone(),
                                kind: sym.kind.as_str().to_string(),
                                path: path.clone(),
                                span_start: sym.span.start_byte,
                                span_end: sym.span.end_byte,
                                is_exported: sym.is_exported,
                                content_hash: ext.content_hash,
                            });
                            if results.len() >= limit {
                                return Ok(results);
                            }
                        }
                    }
                }
            }
        }

        let base_rows = if needle.is_empty() {
            self.base
                .all_symbols_page()?
                .map(|p| p.rows)
                .unwrap_or_default()
        } else {
            self.base.search_symbols(
                query,
                limit
                    .saturating_add(self.overlay.modified_files.len() * 8)
                    .max(64),
            )?
        };
        for sym in base_rows {
            let p = Path::new(&sym.path);
            if self.overlay.is_file_tombstoned(p)
                || self.overlay.modified_files.contains_key(p)
                || self.overlay.is_symbol_tombstoned(&sym.qualified_name)
            {
                continue;
            }
            if seen.insert((sym.path.clone(), sym.qualified_name.clone())) {
                results.push(sym);
                if results.len() >= limit {
                    break;
                }
            }
        }
        Ok(results)
    }

    pub fn edges_for_symbol(&self, symbol: &str) -> Result<Vec<OverlayEdge>> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for e in self
            .overlay
            .delta_edges
            .iter()
            .filter(|e| e.source == symbol || e.target == symbol)
        {
            if seen.insert((e.source.clone(), e.target.clone(), e.kind.clone())) {
                out.push(e.clone());
            }
        }

        let mut base_edges = self.base.callers_of(&[symbol.to_string()], "", 0.0)?;
        let mut defining_files = BTreeSet::new();
        for sym in self.base.search_symbols(symbol, 64)? {
            if sym.name == symbol || sym.qualified_name == symbol {
                defining_files.insert(sym.path);
            }
        }
        for file_path in defining_files {
            for edge in self.base.latest_edges_for_file(&file_path, 0.0)? {
                if edge.source_symbol == symbol || edge.target_symbol == symbol {
                    base_edges.push(edge);
                }
            }
        }

        for edge in base_edges {
            let src_path = Path::new(&edge.source_file);
            let dst_path = Path::new(&edge.target_file);
            if self.overlay.is_file_tombstoned(src_path)
                || self.overlay.modified_files.contains_key(src_path)
                || self.overlay.is_file_tombstoned(dst_path)
                || self.overlay.is_symbol_tombstoned(&edge.source_symbol)
                || self.overlay.is_symbol_tombstoned(&edge.target_symbol)
            {
                continue;
            }
            if seen.insert((
                edge.source_symbol.clone(),
                edge.target_symbol.clone(),
                edge.edge_kind.clone(),
            )) {
                out.push(OverlayEdge {
                    source: edge.source_symbol,
                    target: edge.target_symbol,
                    kind: edge.edge_kind,
                    confidence: (edge.confidence * 100.0).round().clamp(0.0, 100.0) as u8,
                });
            }
        }
        Ok(out)
    }

    pub fn status(&self) -> Result<StackedStatus> {
        let delta_bytes = self.delta.bytes_on_disk();
        let dirty_files = self.overlay.modified_files.len();
        let delta_symbols: usize = self
            .overlay
            .added_extractions
            .values()
            .map(|e| e.symbols.len())
            .sum();
        Ok(StackedStatus {
            worktree_root: self.worktree_root.to_string_lossy().into_owned(),
            base_generation_id: self.lease.generation_id,
            base_dir: self.lease.base_dir.clone(),
            dirty_files,
            tombstoned_files: self.overlay.tombstoned_files.len(),
            delta_symbols,
            delta_edges: self.overlay.delta_edges.len(),
            delta_bytes,
            needs_rebase: dirty_files > self.lease.rebases_after_dirty_files
                || delta_bytes > self.lease.rebases_after_delta_bytes,
        })
    }

    fn reload_overlay(&mut self) -> Result<()> {
        let wt_str = self.worktree_root.to_string_lossy().into_owned();
        self.overlay =
            build_overlay_from_delta(&wt_str, self.lease.generation_id, &self.base, &self.delta)?;
        Ok(())
    }
}

fn build_overlay_from_delta(
    worktree_id: &str,
    generation_id: u32,
    base: &Store,
    delta: &DeltaStore,
) -> Result<WorktreeOverlay> {
    let snap = delta.load_snapshot()?;
    let mut overlay = WorktreeOverlay::new(worktree_id.to_string(), u64::from(generation_id));
    overlay.dirty_nodes = RoaringBitmap::new();

    for (idx, file_rec) in snap.files.into_iter().enumerate() {
        let dirty_id = u32::try_from(idx).unwrap_or(u32::MAX);
        let base_symbols: BTreeSet<String> = base
            .latest_extraction_for_path(&file_rec.path)?
            .map(|ext| ext.symbols.into_iter().map(|s| s.qualified_name).collect())
            .unwrap_or_default();

        if file_rec.tombstoned {
            overlay.mark_file_tombstone(&file_rec.path, Some(dirty_id));
            for old_sym in base_symbols {
                overlay.mark_symbol_tombstone(old_sym, Some(dirty_id));
            }
        } else {
            overlay.record_modified_file(&file_rec.path, &file_rec.content_hash, Some(dirty_id));
            if let Some(ext) = file_rec.extraction {
                let current_symbols: BTreeSet<String> = ext
                    .symbols
                    .iter()
                    .map(|s| s.qualified_name.clone())
                    .collect();
                for removed_sym in base_symbols.difference(&current_symbols) {
                    overlay.mark_symbol_tombstone(removed_sym.clone(), Some(dirty_id));
                }
                overlay.record_extraction(file_rec.path.clone(), ext, dirty_id);
            }
        }
    }

    for edge in snap.edges {
        overlay.add_edge(OverlayEdge {
            source: edge.source_symbol,
            target: edge.target_symbol,
            kind: edge.edge_kind,
            confidence: edge.confidence,
        });
    }

    Ok(overlay)
}

fn extract_delta_edges(rel_path: &str, extraction: &Extraction) -> Vec<DeltaEdgeRecord> {
    let mut edges = Vec::new();
    let mut seen = BTreeSet::new();
    for call in &extraction.calls {
        let src = call
            .caller_symbol
            .clone()
            .unwrap_or_else(|| rel_path.to_string());
        let dst = match &call.receiver_expr {
            Some(recv) if !recv.is_empty() => format!("{recv}.{}", call.callee_name),
            _ => call.callee_name.clone(),
        };
        if seen.insert((src.clone(), dst.clone(), "Calls")) {
            edges.push(DeltaEdgeRecord {
                path: rel_path.to_string(),
                source_symbol: src,
                target_symbol: dst,
                edge_kind: "Calls".to_string(),
                confidence: 85,
            });
        }
    }
    for imp in &extraction.imports {
        if !imp.module_specifier.is_empty()
            && seen.insert((
                rel_path.to_string(),
                imp.module_specifier.clone(),
                "Imports",
            ))
        {
            edges.push(DeltaEdgeRecord {
                path: rel_path.to_string(),
                source_symbol: rel_path.to_string(),
                target_symbol: imp.module_specifier.clone(),
                edge_kind: "Imports".to_string(),
                confidence: 95,
            });
        }
    }
    edges
}

fn normalize_rel_path(rel_path: &str) -> Result<String> {
    let trimmed = rel_path.trim().replace('\\', "/");
    let trimmed = trimmed.trim_start_matches("./");
    if trimmed.is_empty() || trimmed.starts_with('/') {
        bail!("overlay path must be non-empty and repository-relative: {rel_path:?}");
    }
    for comp in Path::new(trimmed).components() {
        if matches!(comp, std::path::Component::ParentDir) {
            bail!("overlay path must not traverse '..': {rel_path:?}");
        }
    }
    Ok(trimmed.to_string())
}

/// Paths that differ from the base: `git status` plus `base_head..HEAD`.
/// Every git failure is an `Err`; an empty set only ever means "clean".
fn git_changed_paths_against_head(worktree_root: &Path, base: &Store) -> Result<BTreeSet<String>> {
    let mut paths = git_status_paths(worktree_root)?;
    let base_head = base.latest_generation_head_sha()?.ok_or_else(|| {
        anyhow::anyhow!("base snapshot records no head_sha; re-run `devmap seed --stacked`")
    })?;
    if !is_commit_sha(&base_head) {
        bail!(
            "base snapshot head_sha {base_head:?} is not a commit; re-run `devmap seed --stacked`"
        );
    }
    let diff = run_git_checked(
        worktree_root,
        &["diff", "--name-only", "-z", &base_head, "HEAD"],
    )?;
    for raw in diff.split(|&b| b == 0) {
        if !raw.is_empty() {
            paths.insert(status_path(worktree_root, raw)?);
        }
    }
    Ok(paths)
}

/// Streaming sha256 of a file, 64 KiB at a time.
pub fn sha256_file_hex(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut file = fs::File::open(path)
        .with_context(|| format!("cannot open {} for sha256", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("cannot read {} for sha256", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(crate::digest::hex(&hasher.finalize()))
}

fn sha256_str_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    crate::digest::hex(&Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "parse")]
    use std::process::Command;
    #[cfg(feature = "parse")]
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(feature = "parse")]
    fn temp_repo_dir(tag: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "devmap-stacked-test-{}-{}-{}",
            tag,
            std::process::id(),
            id
        ));
        let _ = remove_readonly_tree(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(feature = "parse")]
    fn run_git_ok(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[cfg(feature = "parse")]
    fn seed_indexed_git_repo(anchor: &Path) {
        fs::create_dir_all(anchor.join("src")).unwrap();
        fs::write(
            anchor.join("src/svc.go"),
            "package svc\n\ntype Server struct{}\n\nfunc (s *Server) Handle() int { return 1 }\nfunc Helper() int { return 2 }\n",
        )
        .unwrap();
        fs::write(
            anchor.join("src/routes.ts"),
            "export const API_ROUTE = '/v1/items';\nexport class Router {\n  dispatch(): string { return API_ROUTE; }\n}\n",
        )
        .unwrap();
        fs::write(
            anchor.join("src/limits.rs"),
            "pub const MAX_BATCH: usize = 64;\nmacro_rules! check_cap { ($x:expr) => { $x < MAX_BATCH }; }\npub fn within_cap(v: usize) -> bool { check_cap!(v) }\n",
        )
        .unwrap();

        run_git_ok(anchor, &["init", "-b", "main"]);
        run_git_ok(anchor, &["add", "."]);
        run_git_ok(anchor, &["commit", "-m", "initial"]);

        index_tree(anchor, &["src/svc.go", "src/routes.ts", "src/limits.rs"]);
    }

    #[cfg(feature = "parse")]
    fn git_head(cwd: &Path) -> String {
        let out = Command::new("git")
            .current_dir(cwd)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git rev-parse");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// Index `rels` (the whole committed tree) as a new generation stamped with
    /// the real `HEAD`, as `devmap build` does. `save_generation` stamps
    /// `"unknown"`, which a stacked base now refuses.
    #[cfg(feature = "parse")]
    fn index_tree(anchor: &Path, rels: &[&str]) {
        let db_path = store_path(anchor);
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let store = Store::open(&db_path).unwrap();
        let anchor_canon = anchor.canonicalize().unwrap();
        store.bind_repo_root(&anchor_canon).unwrap();
        let exts: Vec<_> = rels
            .iter()
            .map(|rel| {
                devmap_extract::extract_file(rel, &fs::read_to_string(anchor.join(rel)).unwrap())
            })
            .collect();
        let mut resolver = devmap_resolve::Resolver::new();
        resolver.index_extractions(&exts);
        let res = resolver.resolve_all(&exts).unwrap();
        let analysis = devmap_analyze::analyze(&exts, &res);
        store
            .save_generation_with_metadata(
                &exts,
                &res,
                &analysis,
                devmap_store::GenerationWriteOpts::default(),
                &git_head(anchor),
            )
            .unwrap();
        let _ = store.write_latest_csr_sidecar().unwrap();
    }

    #[cfg(feature = "parse")]
    #[test]
    fn stacked_store_preserves_go_receiver_ts_const_and_rust_const_edits_and_refuses_writable_base()
    {
        let root = temp_repo_dir("ast-fidelity");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);

        let lane = root.join("lane-1");
        run_git_ok(
            &anchor,
            &["worktree", "add", "-b", "lane-1", lane.to_str().unwrap()],
        );

        // Adversarial 1: forged `.git` pointer in a fake checkout MUST be refused!
        let forged = root.join("forged");
        fs::create_dir_all(&forged).unwrap();
        let common = registered_common_git_dir(&anchor).unwrap();
        fs::write(
            forged.join(".git"),
            format!("gitdir: {}\n", common.join("worktrees/lane-1").display()),
        )
        .unwrap();
        assert!(
            provision_stacked_worktree(&anchor, &forged).is_err(),
            "forged .git file must be refused by provision_stacked_worktree"
        );

        let lease = provision_stacked_worktree(&anchor, &lane).unwrap();
        assert_eq!(lease.generation_id, 1);

        // Edit Go receiver method, TS `export const` + class method, and Rust `pub const` + macro (`F4`)!
        fs::write(
            lane.join("src/svc.go"),
            "package svc\n\ntype Server struct{}\n\nfunc (s *Server) Handle() int { return 99 }\nfunc (s *Server) Shutdown() {}\n",
        )
        .unwrap();
        fs::write(
            lane.join("src/routes.ts"),
            "export const API_ROUTE = '/v2/items';\nexport class Router {\n  dispatch(): string { return API_ROUTE; }\n  health(): boolean { return true; }\n}\n",
        )
        .unwrap();
        fs::write(
            lane.join("src/limits.rs"),
            "pub const MAX_BATCH: usize = 128;\nmacro_rules! check_cap { ($x:expr) => { $x <= MAX_BATCH }; }\npub fn within_cap(v: usize) -> bool { check_cap!(v) }\n",
        )
        .unwrap();

        let mut stacked = StackedStore::open(&lane).unwrap();
        let sync = stacked.sync_worktree().unwrap();
        assert_eq!(sync.synced_files, 3);
        assert!(sync.rebased_to_standalone.is_none());

        // Go receiver methods `Handle` AND `Shutdown` must be live; deleted `Helper` must be masked!
        let go_syms = stacked.symbols_for_file("src/svc.go").unwrap();
        let go_names: Vec<&str> = go_syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            go_names.contains(&"Handle"),
            "Go receiver method Handle lost: {go_names:?}"
        );
        assert!(
            go_names.contains(&"Shutdown"),
            "Go receiver method Shutdown lost: {go_names:?}"
        );
        assert!(
            !go_names.contains(&"Helper"),
            "Removed Helper should be tombstoned: {go_names:?}"
        );
        assert!(
            stacked.search_symbols("Helper", 10).unwrap().is_empty(),
            "Removed Helper must not leak from immutable base"
        );

        // TS `API_ROUTE` (`export const`) and `dispatch`/`health` class methods must survive!
        let ts_syms = stacked.symbols_for_file("src/routes.ts").unwrap();
        let ts_names: Vec<&str> = ts_syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            ts_names.contains(&"API_ROUTE"),
            "TS export const lost: {ts_names:?}"
        );
        assert!(
            ts_names.contains(&"dispatch"),
            "TS class method lost: {ts_names:?}"
        );
        assert!(
            ts_names.contains(&"health"),
            "TS new class method lost: {ts_names:?}"
        );

        // Rust `MAX_BATCH` (`pub const`) and `check_cap` (`macro_rules!`) must survive!
        let rs_syms = stacked.symbols_for_file("src/limits.rs").unwrap();
        let rs_names: Vec<&str> = rs_syms.iter().map(|s| s.name.as_str()).collect();
        assert!(
            rs_names.contains(&"MAX_BATCH"),
            "Rust pub const lost: {rs_names:?}"
        );
        assert!(
            rs_names.contains(&"check_cap"),
            "Rust macro_rules! lost: {rs_names:?}"
        );

        // Lazy CSR mmap works without scanning SQLite edges!
        assert!(stacked.csr_reader().unwrap().is_some());
        drop(stacked);

        // Adversarial 2: flipping `base.sqlite` to `0644` MUST cause `StackedStore::open` to fail closed!
        let base_sqlite = PathBuf::from(&lease.base_dir).join("base.sqlite");
        set_file_writable(&base_sqlite).unwrap();
        let err = StackedStore::open(&lane)
            .expect_err("writable base.sqlite must be refused")
            .to_string();
        assert!(err.contains("immutable"), "unexpected error: {err}");
        set_file_readonly(&base_sqlite).unwrap();

        let _ = remove_readonly_tree(&root);
    }

    #[cfg(feature = "parse")]
    #[test]
    fn gc_unused_bases_fails_closed_on_corrupt_lease_and_autorebases_above_dirty_threshold() {
        let root = temp_repo_dir("gc-and-rebase");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);

        let lane_a = root.join("lane-a");
        let lane_b = root.join("lane-b");
        run_git_ok(
            &anchor,
            &["worktree", "add", "-b", "lane-a", lane_a.to_str().unwrap()],
        );
        run_git_ok(
            &anchor,
            &["worktree", "add", "-b", "lane-b", lane_b.to_str().unwrap()],
        );

        let lease_a_head = git_head(&anchor);
        let lease_a = provision_stacked_worktree(&anchor, &lane_a).unwrap();
        assert_eq!(lease_a.generation_id, 1);

        // Advance anchor to generation 2 (committed, indexed at the new HEAD) and
        // provision lane-b on it.
        fs::write(anchor.join("src/extra.rs"), "pub fn gen2() {}\n").unwrap();
        run_git_ok(&anchor, &["add", "."]);
        run_git_ok(&anchor, &["commit", "-m", "gen2"]);
        index_tree(
            &anchor,
            &[
                "src/svc.go",
                "src/routes.ts",
                "src/limits.rs",
                "src/extra.rs",
            ],
        );
        let lease_b = provision_stacked_worktree(&anchor, &lane_b).unwrap();
        assert_eq!(lease_b.generation_id, 2);

        let common = registered_common_git_dir(&anchor).unwrap();

        // Both gen-1 (leased by lane-a) and gen-2 (leased by lane-b + newest) are retained!
        let gc1 = gc_unused_bases(&common).unwrap();
        assert_eq!(gc1.retained_generations, vec![1, 2]);
        assert!(gc1.removed_generations.is_empty());

        // Corrupt lane-a's stack_lease.json -> GC MUST FAIL CLOSED without deleting gen-1!
        fs::write(stack_lease_path(&lane_a), b"{corrupt-json").unwrap();
        assert!(
            gc_unused_bases(&common).is_err(),
            "corrupt stack_lease.json must fail closed"
        );
        assert!(base_generation_dir(
            &common,
            &BaseKey::new(1, &lease_a_head, &anchor.canonicalize().unwrap()).unwrap()
        )
        .is_dir());

        // Restore valid lease with low dirty-file threshold (2 files) to exercise auto-rebase!
        let mut custom_lease = lease_a.clone();
        custom_lease.rebases_after_dirty_files = 2;
        fs::write(
            stack_lease_path(&lane_a),
            serde_json::to_vec_pretty(&custom_lease).unwrap(),
        )
        .unwrap();

        for idx in 0..3 {
            fs::write(
                lane_a.join(format!("src/new_{idx}.rs")),
                format!("pub fn f_{idx}() -> usize {{ {idx} }}\n"),
            )
            .unwrap();
        }
        let mut stacked_a = StackedStore::open(&lane_a).unwrap();
        let sync_report = stacked_a.sync_worktree().unwrap();
        let promoted_db = sync_report
            .rebased_to_standalone
            .expect("3 dirty files > threshold 2 must auto-rebase to standalone store");
        assert!(promoted_db.is_file());
        assert!(!stack_lease_path(&lane_a).exists());
        drop(stacked_a);

        // Now lane-a no longer leases gen-1 -> GC cleanly removes gen-1 while retaining gen-2!
        let gc2 = gc_unused_bases(&common).unwrap();
        assert_eq!(gc2.removed_generations, vec![1]);
        assert_eq!(gc2.retained_generations, vec![2]);

        let _ = remove_readonly_tree(&root);
    }

    #[cfg(feature = "parse")]
    fn add_lane(anchor: &Path, root: &Path, name: &str) -> PathBuf {
        let lane = root.join(name);
        run_git_ok(
            anchor,
            &["worktree", "add", "-b", name, lane.to_str().unwrap()],
        );
        lane
    }

    #[cfg(feature = "parse")]
    fn make_tree_writable(dir: &Path) {
        make_tree_writable_best_effort(dir);
    }

    #[cfg(feature = "parse")]
    #[test]
    fn base_key_includes_source_and_head() {
        let root = temp_repo_dir("base-key");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);
        let sib = add_lane(&anchor, &root, "sib");
        fs::write(sib.join("src/sib_only.rs"), "pub fn only_in_sibling() {}\n").unwrap();
        run_git_ok(&sib, &["add", "."]);
        run_git_ok(&sib, &["commit", "-m", "sibling"]);
        index_tree(
            &sib,
            &[
                "src/svc.go",
                "src/routes.ts",
                "src/limits.rs",
                "src/sib_only.rs",
            ],
        );
        let lane_main = add_lane(&anchor, &root, "lane-main");
        let lane_sib = add_lane(&sib, &root, "lane-sib");

        let lease_main = provision_stacked_worktree(&anchor, &lane_main).unwrap();
        let lease_sib = provision_stacked_worktree(&sib, &lane_sib).unwrap();
        assert_eq!(
            lease_main.generation_id, lease_sib.generation_id,
            "fixture must reproduce the colliding generation id"
        );
        assert_ne!(
            lease_main.base_dir, lease_sib.base_dir,
            "two donors with the same generation id must not share a base directory"
        );
        let stacked = StackedStore::open(&lane_sib).unwrap();
        let names: Vec<String> = stacked
            .symbols_for_file("src/sib_only.rs")
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert!(
            names.iter().any(|n| n == "only_in_sibling"),
            "lane provisioned from the sibling must read the sibling's symbols, got {names:?}"
        );
        drop(stacked);
        let _ = remove_readonly_tree(&root);
    }

    #[cfg(feature = "parse")]
    #[test]
    fn reuse_refuses_manifest_or_sha_mismatch() {
        let root = temp_repo_dir("reuse-sha");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);
        let lane_a = add_lane(&anchor, &root, "lane-a");
        let lane_b = add_lane(&anchor, &root, "lane-b");
        let lease = provision_stacked_worktree(&anchor, &lane_a).unwrap();

        let base_dir = PathBuf::from(&lease.base_dir);
        let base_sqlite = base_dir.join("base.sqlite");
        make_tree_writable(&base_dir);
        let mut bytes = fs::read(&base_sqlite).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        fs::write(&base_sqlite, &bytes).unwrap();
        set_file_readonly(&base_sqlite).unwrap();

        let err = provision_stacked_worktree(&anchor, &lane_b)
            .expect_err("a base whose bytes no longer hash to the manifest must not be reused")
            .to_string();
        assert!(err.contains("sha256"), "unexpected error: {err}");

        // A manifest that names another HEAD under the expected directory name is refused too.
        let manifest_path = base_dir.join("manifest.json");
        let mut manifest: BaseManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest.head_sha = "0".repeat(40);
        set_file_writable(&manifest_path).unwrap();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let err = provision_stacked_worktree(&anchor, &lane_b)
            .expect_err("a manifest for another head must not be reused")
            .to_string();
        assert!(err.contains("head_sha"), "unexpected error: {err}");
        let _ = remove_readonly_tree(&root);
    }

    #[cfg(feature = "parse")]
    #[test]
    fn open_refuses_swapped_base() {
        let root = temp_repo_dir("swap");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);
        let lane = add_lane(&anchor, &root, "lane");
        let lease = provision_stacked_worktree(&anchor, &lane).unwrap();
        let base_dir = PathBuf::from(&lease.base_dir);
        let base_sqlite = base_dir.join("base.sqlite");

        // Without the explicit chmod, the published directory cannot be swapped into.
        let impostor = root.join("impostor.sqlite");
        fs::copy(&base_sqlite, &impostor).unwrap();
        set_file_readonly(&impostor).unwrap();
        assert!(
            fs::rename(&impostor, &base_sqlite).is_err() || cfg!(not(unix)),
            "published base directory must not accept a rename"
        );

        // With it, an identical-looking, valid, 0444 file under another inode is still refused.
        make_tree_writable(&base_dir);
        fs::rename(&impostor, &base_sqlite).unwrap();
        set_file_readonly(&base_sqlite).unwrap();
        let err = StackedStore::open(&lane)
            .expect_err("a swapped base.sqlite must be refused")
            .to_string();
        assert!(
            err.contains("replaced or modified"),
            "unexpected error: {err}"
        );
        let _ = remove_readonly_tree(&root);
    }

    #[cfg(feature = "parse")]
    #[test]
    fn sync_reports_git_failure_not_clean() {
        let root = temp_repo_dir("sync-git");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);
        let lane = add_lane(&anchor, &root, "lane");
        provision_stacked_worktree(&anchor, &lane).unwrap();
        fs::write(lane.join("src/svc.go"), "package svc\n\nfunc Edited() {}\n").unwrap();
        let mut stacked = StackedStore::open(&lane).unwrap();
        // Break git for the lane: a gitlink to nowhere.
        fs::write(lane.join(".git"), "gitdir: /nonexistent/devmap-test\n").unwrap();
        let err = stacked
            .sync_worktree()
            .expect_err("a git failure must not read as a clean tree");
        assert!(
            err.to_string().contains("git"),
            "error must name git: {err}"
        );
        drop(stacked);
        let _ = remove_readonly_tree(&root);
    }

    #[cfg(feature = "parse")]
    #[test]
    fn materialize_waits_for_the_writer_lock_and_then_refuses() {
        let root = temp_repo_dir("materialize-lock");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);
        let lane = add_lane(&anchor, &root, "lane");
        provision_stacked_worktree(&anchor, &lane).unwrap();
        let stacked = StackedStore::open(&lane).unwrap();

        let target_db = store_path(&stacked.worktree_root);
        fs::create_dir_all(target_db.parent().unwrap()).unwrap();
        let held = Store::lock_writer_at(&target_db, std::time::Duration::from_secs(5)).unwrap();
        let err = stacked
            .materialize_seeded_store_waiting(std::time::Duration::from_millis(300))
            .expect_err("a held writer lock must refuse the promotion")
            .to_string();
        assert!(
            !target_db.exists(),
            "nothing may be placed under a held lock"
        );
        assert!(
            stack_lease_path(&lane).is_file(),
            "the lease must survive a refusal"
        );
        assert!(!err.is_empty());
        drop(held);

        let placed = stacked.materialize_seeded_store().unwrap();
        assert!(placed.is_file());
        let again = stacked
            .materialize_seeded_store()
            .expect_err("must not overwrite a store");
        assert!(again.to_string().contains("existing store"), "{again}");
        drop(stacked);
        let _ = remove_readonly_tree(&root);
    }

    #[cfg(feature = "parse")]
    #[test]
    fn publish_refuses_a_dirty_or_unbuilt_donor() {
        let root = temp_repo_dir("dirty-donor");
        let anchor = root.join("anchor");
        seed_indexed_git_repo(&anchor);
        let lane = add_lane(&anchor, &root, "lane");

        fs::write(
            anchor.join("src/svc.go"),
            "package svc\n\nfunc Dirty() {}\n",
        )
        .unwrap();
        let err = provision_stacked_worktree(&anchor, &lane)
            .expect_err("a donor with uncommitted edits must be refused")
            .to_string();
        assert!(err.contains("uncommitted"), "unexpected error: {err}");
        assert!(
            err.contains("src/svc.go"),
            "must name the dirty path: {err}"
        );

        run_git_ok(&anchor, &["commit", "-am", "move head without reindexing"]);
        let err = provision_stacked_worktree(&anchor, &lane)
            .expect_err("a donor whose index predates HEAD must be refused")
            .to_string();
        assert!(err.contains("HEAD"), "unexpected error: {err}");
        let _ = remove_readonly_tree(&root);
    }
}
