//! Resolve which repository an MCP session should open.
//!
//! A global `devmap mcp` registration cannot bake a per-project `--db` path:
//! hosts expand placeholders inconsistently (`${CLAUDE_PROJECT_DIR}` is the
//! measured failure), and one process must answer for whichever workspace the
//! client has open.
//!
//! Precedence, when every source is present:
//!
//! 1. MCP `roots/list` — the workspace the host says is open.
//! 2. The process working directory — hosts launch stdio servers with cwd set
//!    to the project root.
//! 3. An explicit `--db` — override for tests and legacy per-project configs.
//!
//! When nothing resolves, the error names all three attempts. An empty list and
//! a missing file are different facts and stay distinct in that message.

use std::path::{Path, PathBuf};

use devmap_extract::paths;

/// One resolution attempt, for the error that names every one that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveAttempt {
    pub source: &'static str,
    pub detail: String,
}

/// Inputs the MCP server collects before opening a store.
#[derive(Debug, Clone)]
pub struct RootResolveInput {
    /// Absolute or relative roots from MCP `roots/list`, when the client
    /// answered. Empty means "asked and got nothing" when `roots_queried` is
    /// true; `None` means "never asked / client has no roots capability".
    pub mcp_roots: Option<Vec<PathBuf>>,
    /// Process cwd when the server started (or when the slot was built).
    pub client_cwd: PathBuf,
    /// Explicit `--db` from the CLI. When set and the higher-precedence sources
    /// fail, this is the override; when set and a higher source succeeds, the
    /// higher source still wins — `--db` is the last attempt, not a force.
    ///
    /// Callers that need a hard override (tests that pin a store) pass only
    /// `--db` and leave roots/cwd empty or pointing nowhere.
    pub explicit_db: Option<PathBuf>,
}

impl RootResolveInput {
    /// Resolve a store path, or an error that names every attempt.
    pub fn resolve(&self) -> Result<PathBuf, String> {
        let mut attempts = Vec::new();

        match &self.mcp_roots {
            None => attempts.push(ResolveAttempt {
                source: "MCP roots/list",
                detail: "not available (client did not advertise roots, or roots were not queried)"
                    .into(),
            }),
            Some(roots) if roots.is_empty() => attempts.push(ResolveAttempt {
                source: "MCP roots/list",
                detail: "client returned an empty roots list".into(),
            }),
            Some(roots) => {
                // Cap what we walk: an oversized list is a DoS against path
                // resolution, not against the graph. The rest are named in the
                // attempt detail so the operator sees the truncation.
                const MAX_ROOTS: usize = 64;
                let considered = roots.len().min(MAX_ROOTS);
                let truncated = roots.len() > MAX_ROOTS;
                let with_stores = roots_with_stores(roots.iter().take(considered));
                if with_stores.len() > 1 {
                    return Err(ambiguous_roots_error(&with_stores));
                }
                if with_stores.len() == 1 {
                    return Ok(canonical_store_path(store_for_root(&with_stores[0])));
                }
                for root in roots.iter().take(considered) {
                    let candidate = store_for_root(root);
                    attempts.push(ResolveAttempt {
                        source: "MCP roots/list",
                        detail: format!(
                            "no store at {} (root {})",
                            candidate.display(),
                            root.display()
                        ),
                    });
                }
                if truncated {
                    attempts.push(ResolveAttempt {
                        source: "MCP roots/list",
                        detail: format!(
                            "client returned {} roots; only the first {MAX_ROOTS} were considered",
                            roots.len()
                        ),
                    });
                }
            }
        }

        let cwd_store = store_for_root(&self.client_cwd);
        if cwd_store.is_file() {
            return Ok(canonical_store_path(cwd_store));
        }
        attempts.push(ResolveAttempt {
            source: "client cwd",
            detail: format!(
                "no store at {} (cwd {})",
                cwd_store.display(),
                self.client_cwd.display()
            ),
        });

        match &self.explicit_db {
            None => attempts.push(ResolveAttempt {
                source: "--db",
                detail: "not set (global registration does not pass --db)".into(),
            }),
            Some(db) if db.is_file() => return Ok(canonical_store_path(db.clone())),
            Some(db) => attempts.push(ResolveAttempt {
                source: "--db",
                detail: format!("no store at {}", db.display()),
            }),
        }

        Err(format_resolve_error(&attempts))
    }

    /// Force `--db` when the caller asked for a pinned store and discovery
    /// should not run. Used by the CLI when `--db` is the only intended source
    /// (legacy per-project MCP configs).
    pub fn resolve_with_db_override(&self) -> Result<PathBuf, String> {
        if let Some(db) = &self.explicit_db {
            if db.is_file() {
                return Ok(canonical_store_path(db.clone()));
            }
            // Fall through so the error still names roots and cwd: a bad --db
            // with a working root is recoverable by dropping the flag.
        }
        self.resolve()
    }

    /// MCP roots that currently hold a readable store, de-duplicated by
    /// canonical path. Used by `devmap_status` to report `candidate_roots`.
    pub fn candidate_roots(&self) -> Vec<PathBuf> {
        match &self.mcp_roots {
            Some(roots) => roots_with_stores(roots.iter().take(64)),
            None => Vec::new(),
        }
    }
}

/// Bound on `repo_path` / `root`, matching the GitPulse `MAX_ARG_CHARS` shape.
pub const MAX_REPO_PATH_BYTES: usize = 4096;

/// Canonicalise an existing directory. Relative paths are refused.
pub fn validate_repo_path(raw: &str) -> Result<PathBuf, String> {
    if raw.is_empty() {
        return Err("repo_path is empty".into());
    }
    if raw.len() > MAX_REPO_PATH_BYTES {
        return Err(format!(
            "repo_path is {} bytes, over the {MAX_REPO_PATH_BYTES}-byte limit",
            raw.len()
        ));
    }
    if looks_like_windows_path(raw) {
        return Err(
            "repo_path looks like a Windows path; this server only opens unix filesystem paths"
                .into(),
        );
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(format!(
            "repo_path must be an absolute directory, not a relative path ({raw})"
        ));
    }
    let meta = std::fs::metadata(path).map_err(|err| {
        format!("invalid repo_path {}: {err}", path.display())
    })?;
    if !meta.is_dir() {
        return Err(format!(
            "repo_path {} is not a directory",
            path.display()
        ));
    }
    path.canonicalize().map_err(|err| {
        format!(
            "repo_path {} could not be canonicalised: {err}",
            path.display()
        )
    })
}

/// MCP `roots/list` URI → a unix filesystem path, or `None` to skip.
///
/// Windows-style (`file:///C:/…`) and non-file URIs are skipped rather than
/// reinterpreted. Percent-decoding refuses invalid UTF-8 rather than using
/// lossy replacement.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = if let Some(rest) = uri.strip_prefix("file://") {
        if rest.starts_with('/') {
            rest.to_string()
        } else {
            let stripped = rest.strip_prefix("localhost")?;
            if stripped.starts_with('/') {
                stripped.to_string()
            } else {
                return None;
            }
        }
    } else if uri.starts_with('/') {
        uri.to_string()
    } else {
        return None;
    };
    if looks_like_windows_path(&path) {
        return None;
    }
    let decoded = percent_decode_strict(&path)?;
    if looks_like_windows_path(&decoded) || !decoded.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(decoded))
}

pub fn looks_like_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    if path.contains('\\') {
        return true;
    }
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }
    bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':'
}

fn percent_decode_strict(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn store_for_root(root: &Path) -> PathBuf {
    paths::store_path(root)
}

fn canonical_store_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// MCP roots that have a store, de-duplicated by canonical directory path.
pub fn roots_with_stores<'a>(roots: impl Iterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for root in roots {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        if !store_for_root(&canonical).is_file() {
            continue;
        }
        if seen.insert(canonical.clone()) {
            out.push(canonical);
        }
    }
    out
}

fn ambiguous_roots_error(roots: &[PathBuf]) -> String {
    let mut lines = vec![
        "more than one MCP root has a DevMap store; this process is shared across workspace tabs so first-wins is not safe. Pass repo_path with the absolute repository path:"
            .to_string(),
    ];
    for root in roots {
        lines.push(format!("  - {}", root.display()));
    }
    lines.join("\n")
}

fn format_resolve_error(attempts: &[ResolveAttempt]) -> String {
    let mut lines = vec![
        "could not resolve a devmap store from MCP roots/list, client cwd, or --db:".to_string(),
    ];
    for attempt in attempts {
        lines.push(format!("  - {}: {}", attempt.source, attempt.detail));
    }
    lines.push(
        "run `devmap build` in the repository, open it as an MCP root, or pass an absolute --db"
            .into(),
    );
    lines.join("\n")
}

/// Whether a `degraded_reason` (or schema status) means SessionStart should
/// rebuild rather than only report stale.
pub fn rebuild_required_reason(
    degraded: Option<&str>,
    schema_outdated: bool,
) -> Option<&'static str> {
    if schema_outdated {
        return Some("schema-behind");
    }
    let reason = degraded?;
    if reason.contains("stored extraction payload is obsolete") {
        return Some("payload-obsolete");
    }
    if reason.contains("run `devmap build` to migrate")
        || (reason.contains("store schema is") && reason.contains("this binary speaks"))
    {
        // Migratable schema-behind surfaces through the schema_outdated branch
        // above when status probes the version; this catches the string when a
        // caller only has degraded_reason.
        if reason.contains("migrate") {
            return Some("schema-behind");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "devmap-root-resolve-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn plant_store(root: &Path) -> PathBuf {
        let store = paths::store_path(root);
        fs::create_dir_all(store.parent().unwrap()).unwrap();
        fs::write(&store, b"not-a-real-sqlite").unwrap();
        store
    }

    #[test]
    fn roots_win_over_cwd_and_db() {
        let via_root = scratch("via-root");
        let via_cwd = scratch("via-cwd");
        let via_db = scratch("via-db");
        let want = plant_store(&via_root);
        plant_store(&via_cwd);
        let db = plant_store(&via_db);

        let resolved = RootResolveInput {
            mcp_roots: Some(vec![via_root]),
            client_cwd: via_cwd,
            explicit_db: Some(db),
        }
        .resolve()
        .expect("roots should win");
        assert_eq!(
            resolved.canonicalize().unwrap_or(resolved),
            want.canonicalize().unwrap_or(want)
        );
    }

    #[test]
    fn cwd_wins_when_roots_absent() {
        let via_cwd = scratch("cwd-only");
        let want = plant_store(&via_cwd);
        let resolved = RootResolveInput {
            mcp_roots: None,
            client_cwd: via_cwd,
            explicit_db: None,
        }
        .resolve()
        .expect("cwd should resolve");
        assert_eq!(
            resolved.canonicalize().unwrap_or(resolved),
            want.canonicalize().unwrap_or(want)
        );
    }

    #[test]
    fn db_is_last_attempt_when_higher_sources_miss() {
        let empty_cwd = scratch("empty-cwd");
        let via_db = scratch("db-fallback");
        let want = plant_store(&via_db);
        let resolved = RootResolveInput {
            mcp_roots: Some(vec![scratch("empty-root")]),
            client_cwd: empty_cwd,
            explicit_db: Some(want.clone()),
        }
        .resolve()
        .expect("--db should be the last attempt");
        assert_eq!(
            resolved.canonicalize().unwrap_or(resolved),
            want.canonicalize().unwrap_or(want)
        );
    }

    #[test]
    fn failure_names_all_three_attempts() {
        let err = RootResolveInput {
            mcp_roots: Some(vec![]),
            client_cwd: scratch("no-cwd-store"),
            explicit_db: None,
        }
        .resolve()
        .expect_err("nothing planted");
        assert!(err.contains("MCP roots/list"), "{err}");
        assert!(err.contains("client cwd"), "{err}");
        assert!(err.contains("--db"), "{err}");
        assert!(err.contains("empty roots list"), "{err}");
        assert!(err.contains("not set"), "{err}");
    }

    #[test]
    fn oversized_roots_list_is_capped_and_named() {
        let cwd = scratch("cap-cwd");
        let mut roots = Vec::new();
        for i in 0..70 {
            roots.push(scratch(&format!("cap-root-{i}")));
        }
        let err = RootResolveInput {
            mcp_roots: Some(roots),
            client_cwd: cwd,
            explicit_db: None,
        }
        .resolve()
        .expect_err("no stores");
        assert!(err.contains("only the first 64 were considered"), "{err}");
    }

    #[test]
    fn db_override_skips_discovery_when_file_exists() {
        let via_root = scratch("override-root");
        let via_db = scratch("override-db");
        plant_store(&via_root);
        let want = plant_store(&via_db);
        let resolved = RootResolveInput {
            mcp_roots: Some(vec![via_root]),
            client_cwd: scratch("override-cwd"),
            explicit_db: Some(want.clone()),
        }
        .resolve_with_db_override()
        .expect("override");
        assert_eq!(
            resolved.canonicalize().unwrap_or(resolved),
            want.canonicalize().unwrap_or(want)
        );
    }

    #[test]
    fn rebuild_required_detects_payload_obsolete_and_schema() {
        assert_eq!(
            rebuild_required_reason(
                Some("stored extraction payload is obsolete; rebuild with the current analyzer"),
                false
            ),
            Some("payload-obsolete")
        );
        assert_eq!(
            rebuild_required_reason(Some("source tree differs"), false),
            None
        );
        assert_eq!(rebuild_required_reason(None, true), Some("schema-behind"));
    }

    #[test]
    fn two_roots_with_stores_are_an_error_naming_both() {
        let first = scratch("two-a");
        let second = scratch("two-b");
        plant_store(&first);
        plant_store(&second);
        let err = RootResolveInput {
            mcp_roots: Some(vec![first.clone(), second.clone()]),
            client_cwd: scratch("two-cwd"),
            explicit_db: None,
        }
        .resolve()
        .expect_err("two stores must not first-wins");
        assert!(
            err.contains(&first.display().to_string()),
            "error must name the first root: {err}"
        );
        assert!(
            err.contains(&second.display().to_string()),
            "error must name the second root: {err}"
        );
        assert!(
            err.to_lowercase().contains("repo_path")
                || err.to_lowercase().contains("ambiguous")
                || err.contains("more than one"),
            "error must tell the caller how to disambiguate: {err}"
        );
    }

    #[test]
    fn windows_file_uri_is_not_reinterpreted_as_a_unix_path() {
        assert!(file_uri_to_path("file:///C:/Users/nobody/project").is_none());
        assert!(file_uri_to_path("file://localhost/C:/Users/nobody").is_none());
        assert!(file_uri_to_path("https://example.com/repo").is_none());
        let unix = file_uri_to_path("file:///Users/example/project").unwrap();
        assert_eq!(unix, PathBuf::from("/Users/example/project"));
        let local = file_uri_to_path("file://localhost/Users/example/project").unwrap();
        assert_eq!(local, PathBuf::from("/Users/example/project"));
        let encoded = file_uri_to_path("file:///tmp/foo%20bar").unwrap();
        assert_eq!(encoded, PathBuf::from("/tmp/foo bar"));
    }
}
