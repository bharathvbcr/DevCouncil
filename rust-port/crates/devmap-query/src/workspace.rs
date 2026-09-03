//! A registry of repositories, and queries that span them.
//!
//! `devmap serve` indexes one root. That is the right unit for a build — a
//! generation describes a tree at a commit — but it is the wrong unit for a
//! question. "Who calls this?" does not stop at a repository boundary when the
//! caller is a sibling service, and answering it per-repository leaves the
//! reader to union the results by hand and to notice, unaided, that they were
//! only ever shown one half.
//!
//! What this does *not* do is invent cross-repository call edges by matching
//! symbol names. Two repositories routinely declare the same `New`, `Client` or
//! `get`, and joining on the name would manufacture edges at a scale that makes
//! the graph worse rather than larger — the same failure the resolver already
//! records at 0.2 confidence within a single repository. Cross-repository links
//! are asserted only where a repository *declares* the module another one
//! imports (see [`link_candidates`]), and they are reported as candidates with
//! their evidence attached.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where a workspace registry lives, relative to the repository it is rooted in.
pub const WORKSPACE_RELPATH: &str = ".devcouncil/workspace.json";

/// One registered repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRepo {
    /// Short name used to label results. Unique within a workspace.
    pub name: String,
    /// Absolute path to the repository root.
    pub root: PathBuf,
    /// Store path, relative to `root`.
    #[serde(default = "default_db_relpath")]
    pub db: String,
}

fn default_db_relpath() -> String {
    ".devcouncil/codeintel/devmap.sqlite".to_string()
}

impl WorkspaceRepo {
    /// Absolute path to this repository's store.
    pub fn db_path(&self) -> PathBuf {
        self.root.join(&self.db)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub repos: Vec<WorkspaceRepo>,
}

fn default_version() -> u32 {
    1
}

/// The registry version this build writes and understands.
pub const WORKSPACE_VERSION: u32 = 1;

impl Workspace {
    /// Read the registry rooted at `root`, or an empty one when none exists.
    ///
    /// A missing file is an empty workspace, not an error: a repository that
    /// has never been added to one is the normal case. A *malformed* file is an
    /// error, because silently treating it as empty would answer a
    /// workspace-wide question with one repository's data and no indication
    /// that the rest were dropped.
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        let path = root.join(WORKSPACE_RELPATH);
        if !path.is_file() {
            return Ok(Self {
                version: WORKSPACE_VERSION,
                repos: Vec::new(),
            });
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
        let workspace: Self = serde_json::from_str(&text).map_err(|error| {
            anyhow::anyhow!("{} is not a valid workspace: {error}", path.display())
        })?;
        if workspace.version > WORKSPACE_VERSION {
            return Err(anyhow::anyhow!(
                "{} declares workspace version {}, which this build does not understand \
                 (it writes version {WORKSPACE_VERSION}); upgrade rather than reading it \
                 as though the fields it adds do not matter",
                path.display(),
                workspace.version
            ));
        }
        Ok(workspace)
    }

    /// Write the registry through the workspace's one atomic writer.
    ///
    /// This built its own temp path — `workspace.json.tmp`, shared by every
    /// concurrent writer — which is the exact collision
    /// [`crate::write_atomic`] exists to prevent: the first `rename` moves the
    /// shared temp away and the second fails with ENOENT. It also skipped
    /// `sync_all`, so a crash between the rename and the flush could leave the
    /// registry's *name* pointing at unwritten bytes. `write_atomic` gives a
    /// per-writer temp name, an fsync before the rename, and cleanup of the
    /// temp on any failure.
    ///
    /// Callers mutating the registry must go through [`Self::update`], which
    /// serialises the read and the write; this on its own is atomic per write,
    /// not per read-modify-write.
    pub fn save(&self, root: &Path) -> anyhow::Result<PathBuf> {
        let path = root.join(WORKSPACE_RELPATH);
        let body = serde_json::to_string_pretty(self)?;
        crate::write_atomic(&path, body.as_bytes())?;
        Ok(path)
    }

    /// Read-modify-write the registry rooted at `root`, under an exclusive
    /// advisory lock.
    ///
    /// `devmap workspace add` loaded the registry, mutated it and saved it with
    /// nothing serialising the three steps, so two concurrent registrations
    /// each read the same registry and each wrote their own entry over the
    /// other's — a silent lost update, on a file whose whole job is to say
    /// which repositories exist. An atomic *write* cannot fix that; only
    /// holding a lock across the read and the write can.
    ///
    /// The lock is an `flock` on a sibling file, the mechanism
    /// `protocol.rs::lock_ipc_endpoint` already uses for the IPC endpoint: the
    /// kernel releases it if the holder dies, so there is no stale lock to
    /// clean up, and it works across processes as well as threads.
    pub fn update<T>(
        root: &Path,
        mutate: impl FnOnce(&mut Self) -> T,
    ) -> anyhow::Result<(T, PathBuf)> {
        let _guard = RegistryLock::acquire(root)?;
        let mut workspace = Self::load(root)?;
        let outcome = mutate(&mut workspace);
        let written = workspace.save(root)?;
        Ok((outcome, written))
    }

    /// Register a repository. Replaces any entry with the same name.
    pub fn add(&mut self, name: String, root: PathBuf) {
        let entry = WorkspaceRepo {
            name,
            root,
            db: default_db_relpath(),
        };
        match self.repos.iter_mut().find(|repo| repo.name == entry.name) {
            Some(existing) => *existing = entry,
            None => self.repos.push(entry),
        }
        self.repos.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove a repository by name. Returns whether one was removed, so a
    /// caller can tell "removed" from "was never there".
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.repos.len();
        self.repos.retain(|repo| repo.name != name);
        self.repos.len() != before
    }
}

/// Longest a writer waits for the registry lock before giving up.
///
/// Bounded rather than a blocking `flock`: every holder of this lock does one
/// small read and one small write, so waiting seconds already means something
/// is wrong, and a writer that hangs forever on a wedged holder is a worse
/// failure than one that says so.
const REGISTRY_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How often the lock is retried while waiting.
const REGISTRY_LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(5);

/// An exclusive advisory lock over one workspace registry, held for as long as
/// the value lives. The kernel drops the `flock` when the file closes, which
/// includes the holder dying, so no stale-lock cleanup is ever needed.
struct RegistryLock {
    /// Never read: the lock lives exactly as long as this file handle, and is
    /// released by closing it. Named like `UnixIpcServer::_lock` for the same
    /// reason.
    _lock: std::fs::File,
}

impl RegistryLock {
    fn acquire(root: &Path) -> anyhow::Result<Self> {
        let path = registry_lock_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                anyhow::anyhow!("cannot open workspace lock {}: {error}", path.display())
            })?;

        let deadline = std::time::Instant::now() + REGISTRY_LOCK_TIMEOUT;
        loop {
            if file.try_lock().is_ok() {
                return Ok(Self { _lock: file });
            }
            if std::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "workspace registry lock {} was held by another writer for \
                     longer than {REGISTRY_LOCK_TIMEOUT:?}",
                    path.display()
                );
            }
            std::thread::sleep(REGISTRY_LOCK_POLL);
        }
    }
}

/// Where the registry's advisory lock lives: beside the registry itself, so a
/// workspace rooted anywhere carries its own.
fn registry_lock_path(root: &Path) -> PathBuf {
    root.join(format!("{WORKSPACE_RELPATH}.lock"))
}

/// Derive a workspace name from a repository path.
pub fn name_for(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "repo".to_string())
}

/// A search hit, labelled with the repository it came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedHit {
    pub repo: String,
    #[serde(flatten)]
    pub hit: crate::model::SymbolHit,
}

/// A repository that could not be queried, and why.
///
/// Carried in the response rather than logged. A federated answer assembled
/// from three of five repositories is not a complete answer, and a reader who
/// cannot see which two were missing has no way to know that.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoUnavailable {
    pub repo: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedSearch {
    pub items: Vec<FederatedHit>,
    pub repos_queried: usize,
    pub unavailable: Vec<RepoUnavailable>,
    /// Hits found across all repositories before the budget was applied.
    pub total: u32,
    pub shown: u32,
    pub hidden: u32,
    pub truncated: bool,
}

/// A module one repository imports and another declares.
///
/// This is the only cross-repository relation asserted here, because it is the
/// only one with evidence that does not reduce to "two files used the same
/// word". `evidence` records what matched — a Go module path from `go.mod`, or
/// a top-level package directory — so a reader can judge the claim instead of
/// taking it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkCandidate {
    pub from_repo: String,
    pub from_file: String,
    pub module_specifier: String,
    pub to_repo: String,
    pub evidence: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("devmap-ws-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_registry_is_an_empty_workspace_not_an_error() {
        let dir = scratch("missing");
        let workspace = Workspace::load(&dir).expect("absent registry loads");
        assert!(workspace.repos.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A malformed registry must not read as an empty one. Treating it as empty
    /// answers a workspace-wide question with nothing, and reports that as a
    /// complete answer.
    #[test]
    fn a_malformed_registry_is_an_error_not_an_empty_workspace() {
        let dir = scratch("malformed");
        std::fs::create_dir_all(dir.join(".devcouncil")).unwrap();
        std::fs::write(dir.join(WORKSPACE_RELPATH), "{ not json").unwrap();
        let error = Workspace::load(&dir).expect_err("malformed registry must fail");
        assert!(
            error.to_string().contains("not a valid workspace"),
            "unhelpful error: {error}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A registry written by a newer build may carry fields this one drops on
    /// read. Refusing beats silently round-tripping it into a lossy rewrite.
    #[test]
    fn a_future_registry_version_is_refused() {
        let dir = scratch("future");
        std::fs::create_dir_all(dir.join(".devcouncil")).unwrap();
        std::fs::write(dir.join(WORKSPACE_RELPATH), r#"{"version":99,"repos":[]}"#).unwrap();
        let error = Workspace::load(&dir).expect_err("future version must fail");
        assert!(error.to_string().contains("version 99"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_is_idempotent_by_name_and_round_trips() {
        let dir = scratch("roundtrip");
        let mut workspace = Workspace::load(&dir).unwrap();
        workspace.add("beta".into(), PathBuf::from("/tmp/beta"));
        workspace.add("alpha".into(), PathBuf::from("/tmp/alpha"));
        workspace.add("alpha".into(), PathBuf::from("/tmp/alpha-moved"));
        assert_eq!(workspace.repos.len(), 2, "re-adding a name duplicated it");
        // Sorted, so the file does not churn on unrelated edits.
        assert_eq!(workspace.repos[0].name, "alpha");
        assert_eq!(workspace.repos[0].root, PathBuf::from("/tmp/alpha-moved"));

        workspace.save(&dir).unwrap();
        let reloaded = Workspace::load(&dir).unwrap();
        assert_eq!(reloaded.repos, workspace.repos);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_distinguishes_removed_from_never_registered() {
        let mut workspace = Workspace::default();
        workspace.add("one".into(), PathBuf::from("/tmp/one"));
        assert!(workspace.remove("one"));
        assert!(
            !workspace.remove("one"),
            "a second removal reported success"
        );
    }

    #[test]
    fn a_repo_name_defaults_to_its_directory() {
        assert_eq!(name_for(Path::new("/a/b/MyService")), "myservice");
    }
}
