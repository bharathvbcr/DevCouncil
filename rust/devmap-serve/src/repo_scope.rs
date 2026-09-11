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

/// Bounded LRU of open stores, keyed by canonical store path.
#[derive(Default)]
pub struct StoreLru {
    entries: Vec<(PathBuf, Arc<Store>, RepositoryRef)>,
}

impl StoreLru {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_store_path(&self) -> Option<PathBuf> {
        self.entries.last().map(|(key, _, _)| key.clone())
    }

    pub fn get(&mut self, key: &Path) -> Option<(Arc<Store>, RepositoryRef)> {
        let pos = self.entries.iter().position(|(k, _, _)| k == key)?;
        let entry = self.entries.remove(pos);
        let result = (Arc::clone(&entry.1), entry.2.clone());
        self.entries.push(entry);
        Some(result)
    }

    pub fn insert(&mut self, key: PathBuf, store: Arc<Store>, attr: RepositoryRef) {
        if let Some(pos) = self.entries.iter().position(|(k, _, _)| k == &key) {
            self.entries.remove(pos);
        }
        if self.entries.len() >= STORE_CACHE_CAP {
            self.entries.remove(0);
        }
        self.entries.push((key, store, attr));
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
        Err(err) => {
            let text = err.to_string();
            if text.contains("schema")
                || text.contains("user_version")
                || text.contains("migrate")
            {
                Err(format!(
                    "rebuild_required: the DevMap store at {} is not this binary's schema and \
will not be migrated in-place. Run `devmap build` in that repository. {text}",
                    path.display()
                ))
            } else {
                Err(format!(
                    "devmap index at {} could not be opened: {err}",
                    path.display()
                ))
            }
        }
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
        store: canonicalize_path(&store),
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
