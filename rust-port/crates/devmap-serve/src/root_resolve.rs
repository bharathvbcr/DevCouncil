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
                for root in roots.iter().take(considered) {
                    let candidate = store_for_root(root);
                    if candidate.is_file() {
                        return Ok(candidate);
                    }
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
            return Ok(cwd_store);
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
            Some(db) if db.is_file() => return Ok(db.clone()),
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
                return Ok(db.clone());
            }
            // Fall through so the error still names roots and cwd: a bad --db
            // with a working root is recoverable by dropping the flag.
        }
        self.resolve()
    }
}

fn store_for_root(root: &Path) -> PathBuf {
    paths::store_path(root)
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
        assert_eq!(resolved, want);
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
        assert_eq!(resolved, want);
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
        assert_eq!(resolved, want);
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
        assert_eq!(resolved, want);
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
}
