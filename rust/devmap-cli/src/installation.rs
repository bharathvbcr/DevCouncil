//! What `doctor` and `paths` report about this installation: the devmap
//! binaries on disk, MCP registrations, the plugin, and stale servers.

use std::path::{Path, PathBuf};

use crate::{claude, integrate, sha256};

/// What the whole binary inventory may spend hashing, shared across every file
/// it discovers rather than granted to each.
///
/// A health check that can run for an unbounded time is worse than one that
/// reports "unknown": `devmap paths` is what the generated agent guide tells
/// every agent to run first, and hashing this process's own 104 MiB unoptimised
/// executable used to take it past half a minute with nothing to stop it.
///
/// The byte ceiling is the bound that actually decides the common case, and it
/// is set well above any devmap that exists — a release binary is ~61 MiB, an
/// unoptimised one ~104 MiB — so a real install always gets a real digest, on
/// an idle machine and a busy one alike. The clock is the backstop for a
/// filesystem that answers slowly rather than for work that is merely large;
/// eight seconds leaves better than twice the headroom under the twenty this
/// command's own passive-diagnostics test allows a single invocation.
const BINARY_HASH_WALL_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);

const BINARY_HASH_BYTE_BUDGET: u64 = 512 * 1024 * 1024;

/// The three hash fields of one inventory row.
///
/// Three states, never two: hashed, absent (there is no file to hash), and
/// unavailable (there is, and the budget or the filesystem stopped us). The
/// last must not arrive looking like either of the others.
struct BinaryHash {
    sha256: serde_json::Value,
    status: &'static str,
    error: serde_json::Value,
}

/// The hash fields for one row: memo first, budget only on a miss.
///
/// `digests` is asked before `budget` is touched, because a remembered digest
/// costs one `stat` and no bytes. That ordering is what lets the byte ceiling be
/// set for a single cold hash rather than for however many binaries a machine
/// has accumulated.
fn hash_binary(
    resolved: &Path,
    exists: bool,
    digests: &mut crate::digest_cache::BinaryDigests,
    budget: &mut sha256::Budget,
) -> BinaryHash {
    if !exists {
        return BinaryHash {
            sha256: serde_json::Value::Null,
            status: "absent",
            error: serde_json::Value::Null,
        };
    }
    match digests.digest_within(resolved, budget) {
        sha256::FileDigest::Hashed(digest) => BinaryHash {
            sha256: serde_json::json!(digest),
            status: "hashed",
            error: serde_json::Value::Null,
        },
        sha256::FileDigest::Unavailable(reason) => BinaryHash {
            sha256: serde_json::Value::Null,
            status: "unavailable",
            error: serde_json::json!(reason),
        },
    }
}

/// Every `devmap` on `PATH` and referenced from common host MCP configs.
///
/// The digests are the expensive part of this inventory — tens to hundreds of
/// megabytes of SHA-256 — and they change only when a binary is rebuilt, so
/// they are memoised on `(path, size, mtime, ctime)`. See
/// [`crate::digest_cache`]; a memo that cannot be read or written costs a
/// rehash and never an answer.
///
/// `digests` is passed in rather than opened here because the two callers
/// differ on one point: `paths` may leave a memo behind and `doctor` may not.
/// Stating that at the call site keeps the policy where the contract is.
pub(crate) fn inventory_devmap_binaries(
    digests: &mut crate::digest_cache::BinaryDigests,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut rows = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    // One budget for the whole inventory. Per-file budgets would let n binaries
    // cost n times the bound the caller was promised.
    let mut budget = sha256::Budget::new(BINARY_HASH_WALL_BUDGET, BINARY_HASH_BYTE_BUDGET);
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            let candidate = dir.join("devmap");
            let Ok(resolved) = candidate.canonicalize() else {
                continue;
            };
            let key = resolved.display().to_string();
            if !seen.insert(key.clone()) {
                continue;
            }
            let probe = inspect_devmap_identity(&resolved);
            let hash = hash_binary(&resolved, resolved.is_file(), digests, &mut budget);
            rows.push(serde_json::json!({
                "path": key,
                "source": "PATH",
                "version": probe.version,
                "build_id": probe.build_id,
                "probe_error": probe.error,
                "probe_status": probe.status,
                "sha256": hash.sha256,
                "sha256_status": hash.status,
                "sha256_error": hash.error,
                "exists": true,
            }));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(resolved) = exe.canonicalize() {
            let key = resolved.display().to_string();
            if seen.insert(key.clone()) {
                let hash = hash_binary(&resolved, resolved.is_file(), digests, &mut budget);
                rows.push(serde_json::json!({
                    "path": key,
                    "source": "current_exe",
                    "probe_status": "current_process",
                    "probe_error": serde_json::Value::Null,
                    "version": Some(env!("CARGO_PKG_VERSION")),
                    "build_id": Some(env!("DEVMAP_BUILD_ID")),
                    "sha256": hash.sha256,
                    "sha256_status": hash.status,
                    "sha256_error": hash.error,
                    "exists": true,
                }));
            }
        }
    }
    for McpConfigSite {
        label,
        path: config_path,
        ..
    } in host_mcp_config_paths()
    {
        if let Some(command) = read_devmap_command_from_mcp_config(&config_path) {
            let resolved = resolve_devmap_command(&command);
            let key = resolved.display().to_string();
            if seen.insert(key.clone()) {
                let probe = inspect_devmap_identity(&resolved);
                let exists = resolved.is_file();
                let hash = hash_binary(&resolved, exists, digests, &mut budget);
                rows.push(serde_json::json!({
                    "path": key,
                    "source": label,
                    "version": probe.version,
                    "build_id": probe.build_id,
                    "probe_error": probe.error,
                    "probe_status": probe.status,
                    "sha256": hash.sha256,
                    "sha256_status": hash.status,
                    "sha256_error": hash.error,
                    "exists": exists,
                }));
            } else if let Some(existing) = rows.iter_mut().find(|r| r["path"] == key) {
                let source = existing["source"].as_str().unwrap_or("").to_string();
                existing["source"] = serde_json::json!(format!("{source},{label}"));
            }
        }
    }
    // Once, after the whole pass: the same binary is routinely reached through
    // `PATH`, `current_exe` and several MCP configs, and one write per row
    // would publish the memo repeatedly for a single answer.
    digests.save();
    Ok(rows)
}

/// The host that loads an MCP config document.
///
/// Two registrations are duplicates only when one host loads both. Cursor and
/// Claude Code never share a process, so an entry in each is two hosts served
/// once apiece — counting them together reported a duplicate on every machine
/// that had integrated both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum McpHost {
    Cursor,
    ClaudeCode,
}

impl McpHost {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::ClaudeCode => "claude-code",
        }
    }
}

/// One MCP config document a diagnostic inspects.
struct McpConfigSite {
    label: String,
    path: PathBuf,
    /// `None` for a document no host loads as it stands: the plugin bundle a
    /// marketplace installs *from*, a plugin Claude Code has disabled, or a
    /// project server rejected by `disabledMcpjsonServers`. Still inventoried —
    /// it names a binary — but never counted as a registration.
    host: Option<McpHost>,
    /// The enabled Claude Code plugin's own registration. Claude Code drops a
    /// plugin server only when another server has the same command, and the
    /// entries `devmap integrate` writes carry an absolute path that never
    /// matches the plugin's, so beside it even a pinned entry is a second
    /// server for that host.
    plugin: bool,
}

/// Marketplace-root `.mcp.json` files from the bundle layout before the plugin
/// moved into its own `<out>/devmap/` directory. No host loads a file at a
/// marketplace root; the in-place plugin a local marketplace does load is
/// reached through [`claude::claude_plugin_install`].
const PLUGIN_BUNDLE_SOURCES: &[&str] = &[
    ".devcouncil/devmap-plugin/.mcp.json",
    ".devmap/devmap-plugin/.mcp.json",
];

fn host_mcp_config_paths() -> Vec<McpConfigSite> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    host_mcp_config_sites(home.as_deref(), &cwd)
}

fn host_mcp_config_sites(home: Option<&Path>, cwd: &Path) -> Vec<McpConfigSite> {
    let site = |label: String, path: PathBuf, host: Option<McpHost>| McpConfigSite {
        label,
        path,
        host,
        plugin: false,
    };
    let mut out = Vec::new();
    if let Some(home) = home {
        out.push(site(
            "~/.cursor/mcp.json".to_string(),
            home.join(".cursor/mcp.json"),
            Some(McpHost::Cursor),
        ));
        out.push(site(
            "~/.claude.json".to_string(),
            home.join(".claude.json"),
            Some(McpHost::ClaudeCode),
        ));
        // Only the copy Claude Code loads: in place for a local-directory
        // marketplace, else the version its install record names. Every other
        // directory under the cache is a leftover from an earlier install that
        // nothing loads; `plugin_cleanup_note` reports those.
        let install = claude::claude_plugin_install(home);
        let host = (install.enabled == Some(true)).then_some(McpHost::ClaudeCode);
        for plugin in &install.loaded {
            let mcp = plugin.dir.join(".mcp.json");
            if mcp.is_file() {
                out.push(McpConfigSite {
                    plugin: host.is_some(),
                    ..site(mcp.display().to_string(), mcp, host)
                });
            }
        }
        for rel in PLUGIN_BUNDLE_SOURCES {
            let path = home.join(rel);
            if path.is_file() {
                out.push(site(format!("~/{rel}"), path, None));
            }
        }
    }
    out.push(site(
        ".cursor/mcp.json".to_string(),
        cwd.join(".cursor").join("mcp.json"),
        Some(McpHost::Cursor),
    ));
    // Claude Code's project scope. A server there connects only once approved
    // in an interactive session, but `claude -p`, Agent SDK and cloud sessions
    // load it without asking, so an unapproved one still counts. A rejection
    // in `disabledMcpjsonServers` blocks it in every mode.
    let rejected = claude::claude_project_server_rejected(home, cwd, claude::MCP_SERVER_NAME);
    out.push(site(
        if rejected {
            ".mcp.json (rejected by disabledMcpjsonServers)".to_string()
        } else {
            ".mcp.json".to_string()
        },
        cwd.join(".mcp.json"),
        (!rejected).then_some(McpHost::ClaudeCode),
    ));
    for rel in PLUGIN_BUNDLE_SOURCES {
        let path = cwd.join(rel);
        if path.is_file() {
            out.push(site(rel.to_string(), path, None));
        }
    }
    out
}

fn resolve_devmap_command(command: &str) -> PathBuf {
    let path = PathBuf::from(command);
    if command.contains('/') || command.contains('\\') {
        return path.canonicalize().unwrap_or(path);
    }
    if let Some(found) = resolve_on_path(command) {
        return found;
    }
    path
}

fn resolve_on_path(command: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(command);
        if candidate.is_file() {
            return Some(candidate.canonicalize().unwrap_or(candidate));
        }
    }
    None
}

fn read_devmap_command_from_mcp_config(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let servers = value.get("mcpServers")?.as_object()?;
    let entry = servers.get("devmap")?;
    entry.get("command")?.as_str().map(str::to_string)
}

struct BinaryIdentity {
    version: Option<String>,
    build_id: Option<String>,
    error: Option<String>,
    status: &'static str,
}

// Diagnostics describe registrations; opening a checkout never authorizes its
// commands to execute. Even PATH entries can point into the repository. Only
// this already-running process supplies an authenticated build identity.
fn inspect_devmap_identity(path: &Path) -> BinaryIdentity {
    let is_current = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .is_ok_and(|exe| exe == path);
    if is_current {
        return BinaryIdentity {
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            build_id: Some(env!("DEVMAP_BUILD_ID").to_string()),
            error: None,
            status: "current_process",
        };
    }
    BinaryIdentity {
        version: None,
        build_id: None,
        error: Some(
            "execution skipped: diagnostics inspect registrations without running them".into(),
        ),
        status: "skipped",
    }
}

pub(crate) fn build_identity_json() -> serde_json::Value {
    serde_json::json!({
        "id": env!("DEVMAP_BUILD_ID"),
        "git": env!("DEVMAP_GIT_HASH"),
        "dirty": env!("DEVMAP_GIT_DIRTY") == "1",
        "time": env!("DEVMAP_BUILD_TIME"),
    })
}

pub(crate) fn binaries_skew_warning(binaries: &[serde_json::Value]) -> Option<String> {
    let distinct = |key: &str| -> std::collections::BTreeSet<String> {
        binaries
            .iter()
            .filter_map(|b| {
                b.get(key)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .collect()
    };
    let versions = distinct("version");
    let build_ids = distinct("build_id");
    let hashes = distinct("sha256");
    // A hash the inventory could not take is not a hash that agreed. Returning
    // plain `None` from a comparison that never ran is how "these binaries
    // match" comes to mean "nobody looked", so the gap travels with the answer
    // in both directions: alone when nothing else disagrees, and appended to
    // the skew report when something does — because the sha256 list it prints
    // is then a subset of the binaries found.
    let unhashed: Vec<String> = binaries
        .iter()
        .filter(|row| {
            row.get("sha256_status").and_then(serde_json::Value::as_str) == Some("unavailable")
        })
        .filter_map(|row| {
            row.get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect();
    if versions.len() <= 1 && build_ids.len() <= 1 && hashes.len() <= 1 {
        if unhashed.is_empty() {
            return None;
        }
        return Some(format!(
            "binary hash comparison incomplete: {} could not be hashed inside the diagnostic's \
             budget; reported versions and build ids agree, but whether these binaries are \
             byte-identical is unverified",
            unhashed.join(", ")
        ));
    }
    let mut warning = format!(
        "multiple devmap binaries on PATH/host configs: versions [{}], build ids [{}], sha256 [{}]; integrate writes the absolute path of the binary that validated the config — re-run integrate after installing",
        versions.into_iter().collect::<Vec<_>>().join(", "),
        build_ids.into_iter().collect::<Vec<_>>().join(", "),
        hashes.into_iter().take(4).collect::<Vec<_>>().join(", ")
    );
    if !unhashed.is_empty() {
        warning.push_str(&format!(
            "; that sha256 list is incomplete — {} could not be hashed inside the diagnostic's \
             budget",
            unhashed.join(", ")
        ));
    }
    Some(warning)
}

pub(crate) fn missing_binary_warning(binaries: &[serde_json::Value]) -> Option<String> {
    let missing: Vec<String> = binaries
        .iter()
        .filter_map(|row| {
            let path = row.get("path")?.as_str()?;
            let missing = match row.get("exists").and_then(serde_json::Value::as_bool) {
                Some(false) => true,
                Some(true) => false,
                None => !Path::new(path).is_file(),
            };
            missing.then(|| path.to_string())
        })
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "host MCP config names a devmap path that is not a file: {}; install the binary or re-run integrate so the config points at a real executable — this is not version skew",
        missing.join("; ")
    ))
}

fn mcp_entry_looks_like_devmap_mcp(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    let Some(entry) = value.get("mcpServers").and_then(|s| s.get("devmap")) else {
        return false;
    };
    let args = entry
        .get("args")
        .and_then(serde_json::Value::as_array)
        .map(|args| {
            args.iter()
                .filter_map(serde_json::Value::as_str)
                .any(|a| a == "mcp")
        })
        .unwrap_or(false);
    args || entry
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some()
}

fn mcp_entry_is_pinned(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value
        .pointer("/mcpServers/devmap")
        .is_some_and(integrate::entry_is_pinned)
}

/// Registrations of `devmap mcp` across the documents a diagnostic inspects.
///
/// `global` and `pinned` hold what a host loads, each row naming that host;
/// `not_loaded` holds documents that name the server but that no host loads as
/// they stand (a plugin bundle source, a disabled plugin, a project server
/// rejected by `disabledMcpjsonServers`).
pub(crate) fn mcp_registration_inventory() -> serde_json::Value {
    mcp_registration_inventory_of(&host_mcp_config_paths())
}

fn mcp_registration_inventory_of(sites: &[McpConfigSite]) -> serde_json::Value {
    let mut global = Vec::new();
    let mut pinned = Vec::new();
    let mut not_loaded = Vec::new();
    for site in sites {
        if !site.path.is_file() || !mcp_entry_looks_like_devmap_mcp(&site.path) {
            continue;
        }
        let row = serde_json::json!({
            "label": site.label,
            "path": site.path.display().to_string(),
            "host": site.host.map(McpHost::as_str),
            "plugin": site.plugin,
        });
        if site.host.is_none() {
            not_loaded.push(row);
        } else if mcp_entry_is_pinned(&site.path) {
            pinned.push(row);
        } else {
            global.push(row);
        }
    }
    serde_json::json!({ "global": global, "pinned": pinned, "not_loaded": not_loaded })
}

pub(crate) fn duplicate_mcp_registration_warning() -> Option<String> {
    duplicate_registration_message(&mcp_registration_inventory())
}

/// A duplicate is one host loading `devmap mcp` twice — a `~/.claude.json` or
/// project `.mcp.json` entry beside an enabled plugin's `.mcp.json`, or
/// `~/.cursor/mcp.json` beside a project `.cursor/mcp.json`. One registration
/// per host, across several hosts, is the intended shape and says nothing.
///
/// Pinned (`--root`/`--db`) entries count only beside an enabled plugin.
/// Claude Code collapses same-named servers across its own scopes, but keeps a
/// plugin server unless another has the same command, so a pinned entry there
/// is a second server rather than a narrower view of the same one.
fn duplicate_registration_message(inventory: &serde_json::Value) -> Option<String> {
    let rows = |key: &str| {
        inventory
            .get(key)
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let host_of = |row: &serde_json::Value| {
        row.get("host")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let global = rows("global");
    let plugin_hosts: std::collections::BTreeSet<String> = global
        .iter()
        .filter(|row| row.get("plugin").and_then(serde_json::Value::as_bool) == Some(true))
        .filter_map(host_of)
        .collect();
    let (counted_pinned, other_pinned): (Vec<_>, Vec<_>) = rows("pinned")
        .into_iter()
        .partition(|row| host_of(row).is_some_and(|host| plugin_hosts.contains(&host)));
    // Each entry is (server, shown). Claude Code "connects to it once, using
    // the definition from the highest-precedence source" when its own scopes
    // name the same server, so `~/.claude.json` and a project `.mcp.json` are
    // one server between them; only the plugin stands apart.
    let mut by_host: std::collections::BTreeMap<String, Vec<(String, String)>> = Default::default();
    for row in global.iter().chain(&counted_pinned) {
        let (Some(host), Some(label), Some(path)) = (
            host_of(row),
            row.get("label").and_then(serde_json::Value::as_str),
            row.get("path").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let plugin = row.get("plugin").and_then(serde_json::Value::as_bool) == Some(true);
        let server = if host == McpHost::ClaudeCode.as_str() && !plugin {
            "claude-code scopes".to_string()
        } else {
            path.to_string()
        };
        by_host
            .entry(host)
            .or_default()
            .push((server, format!("{label} ({path})")));
    }
    let duplicated: Vec<String> = by_host
        .into_iter()
        .filter(|(_, entries)| {
            entries
                .iter()
                .map(|(server, _)| server)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1
        })
        .map(|(host, entries)| {
            let shown: Vec<String> = entries.into_iter().map(|(_, shown)| shown).collect();
            format!("{host} loads {}", shown.join("; "))
        })
        .collect();
    if duplicated.is_empty() {
        return None;
    }
    let mut message = format!(
        "`devmap mcp` is registered more than once for one host: {}. Each registration is its \
         own server for that host; keep one. Registrations in different hosts are not \
         duplicates, and neither replaces passing repo_path",
        duplicated.join(" | ")
    );
    if !counted_pinned.is_empty() {
        message.push_str(
            ". A pinned (--root/--db) entry beside an enabled plugin is counted: Claude Code \
             keeps a plugin server unless another has the same command",
        );
    }
    if !other_pinned.is_empty() {
        let pinned_list = other_pinned
            .iter()
            .filter_map(|row| row.get("path")?.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        message.push_str(&format!(
            ". Pinned (--root/--db) registrations are listed separately and are not duplicates: \
             {pinned_list}"
        ));
    }
    Some(message)
}

fn stray_state_dir_without_store(dir: &Path) -> bool {
    dir.is_dir() && !dir.join("codeintel").join("devmap.sqlite").is_file()
}

pub(crate) fn stray_state_warning() -> Option<String> {
    let mut hits = Vec::new();
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(home.join(".devcouncil"));
        candidates.push(home.join(".devmap"));
    }
    for base in ["/tmp", "/private/tmp"] {
        candidates.push(PathBuf::from(base).join(".devcouncil"));
        candidates.push(PathBuf::from(base).join(".devmap"));
    }
    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        if !tmpdir.is_empty() {
            let base = PathBuf::from(tmpdir);
            candidates.push(base.join(".devcouncil"));
            candidates.push(base.join(".devmap"));
        }
    }
    for dir in candidates {
        if stray_state_dir_without_store(&dir) {
            hits.push(dir.display().to_string());
        }
    }
    if hits.is_empty() {
        return None;
    }
    Some(format!(
        "state directory exists without a store: {} — leftover from an unresolved MCP session \
         log or a refused index; safe to delete after confirming it is not a repository",
        hits.join(", ")
    ))
}

/// Problems with the plugin Claude Code actually loads, each repaired by
/// regenerating and reinstalling the bundle. SessionStart injects this, so it
/// must never carry anything that remedy does not fix — see
/// [`plugin_cleanup_note`] for what it deliberately leaves out.
pub(crate) fn plugin_warning() -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let issues = plugin_health(&home).issues;
    (!issues.is_empty()).then(|| issues.join("; "))
}

/// Cache directories from earlier installs that Claude Code no longer loads.
///
/// Kept out of [`plugin_warning`] on purpose. Checking every directory under
/// the cache made a leftover `0.2.3` beside an active `0.2.4` read as "installed
/// plugin version 0.2.3 does not match binary 0.2.4" in every session brief,
/// with a remedy (reinstall) that leaves the leftover where it is. Diagnostics
/// still name them, as housekeeping rather than as a fault.
pub(crate) fn plugin_cleanup_note() -> Option<String> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let cleanup = plugin_health(&home).cleanup;
    (!cleanup.is_empty()).then(|| cleanup.join("; "))
}

struct PluginHealth {
    issues: Vec<String>,
    cleanup: Vec<String>,
}

fn plugin_health(home: &Path) -> PluginHealth {
    let mut issues = Vec::new();
    let mut cleanup = Vec::new();
    let binary_version = env!("CARGO_PKG_VERSION");
    let install = claude::claude_plugin_install(home);
    if let Some(error) = &install.record_error {
        issues.push(format!(
            "cannot tell which Dev Map plugin version Claude Code loads ({error}); the \
             installed version is unchecked"
        ));
    }
    if let Some(error) = &install.load_error {
        issues.push(format!(
            "cannot tell whether Claude Code loads the Dev Map plugin in place from its \
             marketplace ({error}); only the install record's copy is checked, and it may not \
             be the one sessions load"
        ));
    }
    if !install.leftovers.is_empty() {
        let names = install
            .leftovers
            .iter()
            .filter_map(|dir| dir.file_name()?.to_str())
            .collect::<Vec<_>>()
            .join(", ");
        let active = if install.loaded.is_empty() {
            "no install record names any version".to_string()
        } else {
            format!(
                "the active install is {}",
                install
                    .loaded
                    .iter()
                    .map(|plugin| plugin.dir.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        cleanup.push(format!(
            "leftover Dev Map plugin cache version(s) {names} under {}: Claude Code does not \
             load them ({active}); safe to delete",
            install.cache.display()
        ));
    }
    // A third state, neither loaded nor a leftover: the record still names a
    // cache copy, but the plugin loads in place. Not deletable — the record
    // names it — and not checked, because no session reads it.
    if install
        .loaded
        .iter()
        .any(|plugin| plugin.origin == claude::LoadOrigin::InPlace)
        && !install.recorded.is_empty()
    {
        cleanup.push(format!(
            "installed_plugins.json names {}, but the {} marketplace is a local directory, so \
             sessions load the plugin in place from {}; the cache copy is not checked",
            install
                .recorded
                .iter()
                .map(|dir| dir.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            claude::MARKETPLACE_NAME,
            install
                .loaded
                .iter()
                .map(|plugin| plugin.dir.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for plugin in &install.loaded {
        let dir = &plugin.dir;
        if !dir.is_dir() {
            let named_by = match plugin.origin {
                claude::LoadOrigin::InstallRecord => "installed_plugins.json names this install",
                claude::LoadOrigin::InPlace => {
                    "the local marketplace loads the plugin in place from here"
                }
            };
            issues.push(format!(
                "{}: {named_by}, and it does not exist",
                dir.display()
            ));
            continue;
        }
        let hooks = dir.join("hooks").join("hooks.json");
        let mcp = dir.join(".mcp.json");
        if !hooks.is_file() || !mcp.is_file() {
            issues.push(format!(
                "{}: installed plugin dir is missing hooks/hooks.json or .mcp.json at root \
                 (malformed layout)",
                dir.display()
            ));
            continue;
        }
        match plugin.version.as_deref() {
            Some(version) if version == binary_version => {}
            Some(version) => issues.push(format!(
                "{}: installed plugin version {version} does not match binary {binary_version}",
                dir.display()
            )),
            None => issues.push(format!(
                "{}: no version could be read for the loaded plugin (`version` in \
                 .claude-plugin/plugin.json for an in-place copy, the directory name for a \
                 cached one); its version is unchecked",
                dir.display()
            )),
        }
        if let Ok(text) = std::fs::read_to_string(&hooks) {
            if text.contains("\"args\"") || text.contains("\"async\"") {
                issues.push(format!(
                    "{}: hooks still use args/async; regenerate with `devmap claude plugin` \
                     (shell-form required for Cursor/Codex)",
                    hooks.display()
                ));
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(groups) = value
                    .pointer("/hooks/SessionEnd")
                    .and_then(|v| v.as_array())
                {
                    for group in groups {
                        if let Some(handlers) = group.get("hooks").and_then(|v| v.as_array()) {
                            for handler in handlers {
                                if let Some(timeout) =
                                    handler.get("timeout").and_then(serde_json::Value::as_f64)
                                {
                                    if timeout > f64::from(claude::SESSION_END_MAX_TIMEOUT_SECS) {
                                        issues.push(format!(
                                            "{}: SessionEnd timeout {timeout} exceeds host budget \
                                             (max {})",
                                            hooks.display(),
                                            claude::SESSION_END_MAX_TIMEOUT_SECS
                                        ));
                                    }
                                }
                                if let Some(command) =
                                    handler.get("command").and_then(serde_json::Value::as_str)
                                {
                                    let trimmed = command.trim();
                                    if !trimmed.contains('/')
                                        && !trimmed.contains('\\')
                                        && !trimmed.contains(' ')
                                    {
                                        issues.push(format!(
                                            "{}: bare-name hook command {trimmed:?}; use an \
                                             absolute shell-form path",
                                            hooks.display()
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Codex trust records naming a missing hooks.json.
    let codex_config = home.join(".codex").join("config.toml");
    if let Ok(text) = std::fs::read_to_string(&codex_config) {
        if let Ok(table) = text.parse::<toml::Table>() {
            if let Some(trust) = table.get("hooks").and_then(|v| v.get("trust")) {
                if let Some(map) = trust.as_table() {
                    for (key, _) in map {
                        let path = PathBuf::from(key);
                        if key.contains("hooks.json") && !path.is_file() {
                            issues.push(format!(
                                "Codex hooks.trust names missing {}: stale trust record",
                                path.display()
                            ));
                        }
                    }
                }
            }
        }
    }

    PluginHealth { issues, cleanup }
}

/// Parse `ps` elapsed time — `[[dd-]hh:]mm:ss` — into a duration.
///
/// In-process on purpose: the caller runs this once per matching line, and the
/// previous spelling spawned `date` there instead. `None` for anything that is
/// not that shape, which the caller skips rather than guessing at.
///
/// `cfg(unix)` because its only caller is `stale_server_warning`'s unix arm:
/// there is no `ps` to parse on Windows, so compiling it there left dead code
/// that `-D warnings` rejected. The tests below carry the same gate.
#[cfg(unix)]
fn parse_etime(text: &str) -> Option<std::time::Duration> {
    let (days, rest) = match text.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let mut fields = [0u64; 3];
    let mut count = 0usize;
    for piece in rest.split(':') {
        if count == 3 || piece.is_empty() {
            return None;
        }
        fields[count] = piece.parse::<u64>().ok()?;
        count += 1;
    }
    let (hours, minutes, seconds) = match count {
        3 => (fields[0], fields[1], fields[2]),
        2 => (0, fields[0], fields[1]),
        _ => return None,
    };
    // `ps` never reports 60+ here; a value that does is a format this parser
    // does not understand, and inventing a duration from it would silently
    // mis-age a process.
    if minutes > 59 || seconds > 59 {
        return None;
    }
    Some(std::time::Duration::from_secs(
        days * 86_400 + hours * 3_600 + minutes * 60 + seconds,
    ))
}

pub(crate) fn stale_server_warning() -> Option<String> {
    // A tail expression, not `return None`: on Windows this arm *is* the whole
    // body, so the `return` was `clippy::needless_return` on the one platform
    // that compiles it — and `-D warnings` failed the build there on a line no
    // other target ever sees. `None` here means "not measured" rather than
    // "nothing is stale"; the check reads `ps`, which Windows does not have.
    #[cfg(not(unix))]
    {
        None
    }
    #[cfg(unix)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return None;
        };
        let Ok(meta) = std::fs::metadata(&exe) else {
            return None;
        };
        let Ok(bin_mtime) = meta.modified() else {
            return None;
        };
        let mut command = std::process::Command::new("ps");
        // `etime=` (elapsed), not `lstart=` (a local-time wall clock). Two
        // reasons, both load-bearing:
        //
        // 1. `lstart` had to be turned into an instant, and that was done by
        //    spawning `date -j -f ...` **once per matching line**. The line
        //    count is the number of running DevMap MCP servers, which is
        //    unbounded and grows with exactly the thing this tool is for:
        //    measured 2026-09-12 on this machine, 67 matches meant 67 child
        //    processes and 2.3-2.8 s inside a function whose own `ps` call is
        //    budgeted at 500 ms. Each spawn was individually bounded and the
        //    fan-out was not, so `devmap paths` got slower the more DevMap
        //    sessions were open.
        // 2. `date -j -f` is BSD-only. On Linux that child failed, and the
        //    whole check returned "unavailable" — so it never worked in CI or
        //    on any Linux host, and said so in a way nothing asserted.
        //
        // Elapsed time needs no timezone and no subprocess.
        command.args(["-axo", "pid=,etime=,command="]);
        // A deadline is a ceiling, not a cost: `run_bounded` returns the moment
        // the child exits, so a wider bound is free in the common case and only
        // widens the worst one.
        //
        // 500 ms was not a ceiling here, it was a coin flip. Measured
        // 2026-09-12 on a developer machine running several agent sessions:
        // `ps -axo` costs 0.40-0.52 s against 1,900 processes, so the check
        // lost its own race about a third of the time on a release build and
        // every time on a debug one — reporting "unavailable" while a genuinely
        // stale MCP server went unmentioned. A check that usually cannot run is
        // worse than no check, because its answer reads the same as a clean one
        // to anything that only looks for a warning string.
        //
        // 4 s is ~8x the measured worst case, in the same spirit as the
        // wall-clock budgets in verify.sh, which are set at ~2.5x measured and
        // documented as catching a real regression rather than a busy machine.
        let output = match devmap_extract::subprocess::run_bounded(&mut command,
            devmap_extract::subprocess::Bounds { deadline: std::time::Duration::from_secs(4), stdout_cap: 4 * 1024 * 1024, stderr_cap: 4096 }) {
            Ok(output) if !output.stdout_truncated && !output.stderr_truncated => output,
            // Two different unavailabilities, and they were reported as one. A
            // `ps` that is not on `PATH` never ran; saying it "did not complete
            // within its bounds" sends the reader looking for a slow machine.
            Ok(_) => return Some("stale-server check unavailable: process inventory did not complete within its time/output bounds".into()),
            Err(failure) => return Some(format!("stale-server check unavailable: process inventory could not be read: {failure}")),
        };
        // The same conflation one line down from where it was just fixed: a `ps`
        // that exited nonzero told the caller `None`, which is the answer a
        // completed clean scan gives. Anything looking for a warning string read
        // "no stale servers" out of a check that never produced a process list.
        if !output.status.success() {
            return Some(format!(
                "stale-server check unavailable: process inventory exited with {}",
                output
                    .status
                    .code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "a signal".to_string())
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // No per-line child process here any more: `etime=` is elapsed time
        // that `parse_etime` reads in process, where this used to ask `ps` for a
        // local-time `lstart=` and then spawn a `date` per matching line to turn
        // it into an instant. That fan-out was the cost worth bounding, and
        // removing it beats capping it.
        let mut stale = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if !line.contains("devmap") || !line.contains("mcp") {
                continue;
            }
            // `ps -axo pid=,etime=,command=` — `1-02:03:04`, `02:03:04`, `03:04`.
            let mut parts = line.split_whitespace();
            let Some(pid) = parts.next() else {
                continue;
            };
            let Some(etime) = parts.next() else {
                continue;
            };
            let Some(elapsed) = parse_etime(etime) else {
                continue;
            };
            let Some(started_at) = std::time::SystemTime::now().checked_sub(elapsed) else {
                continue;
            };
            if started_at < bin_mtime {
                stale.push(pid.to_string());
            }
        }
        if stale.is_empty() {
            // Every line was examined, so an empty list is a real answer.
            return None;
        }
        Some(format!(
            "devmap mcp process(es) started before the installed binary's mtime: pid {}; \
             restart hosts so they pick up the current binary",
            stale.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// `ps` elapsed time, in the three shapes it actually prints.
    ///
    /// This replaced a `date` subprocess spawned once per matching line, so it
    /// is now the only thing standing between a process's age and a wrong
    /// stale-server verdict.
    #[test]
    #[cfg(unix)]
    fn etime_parses_every_shape_ps_prints() {
        use std::time::Duration;
        assert_eq!(parse_etime("03:04"), Some(Duration::from_secs(184)));
        assert_eq!(parse_etime("02:03:04"), Some(Duration::from_secs(7384)));
        assert_eq!(parse_etime("1-02:03:04"), Some(Duration::from_secs(93_784)));
        assert_eq!(
            parse_etime("10-00:00:00"),
            Some(Duration::from_secs(864_000))
        );
        assert_eq!(parse_etime("00:00"), Some(Duration::ZERO));
    }

    /// Anything that is not that shape yields `None`, never a guessed age.
    ///
    /// The caller skips a `None` line. Returning some plausible duration for
    /// unparseable input would silently mis-age a process and could report a
    /// live MCP server as stale — or hide one that is.
    #[test]
    #[cfg(unix)]
    fn etime_refuses_what_it_does_not_understand() {
        for bad in [
            "",
            ":",
            "::",
            "1:2:3:4",
            "abc",
            "-",
            "1-",
            "-01:02",
            "01:60",
            "60:00",
            "1--02:03:04",
            "0x10:00",
            "01:02:",
            ":01:02",
            " 01:02",
            "99999999999999999999:00",
        ] {
            assert_eq!(parse_etime(bad), None, "parse_etime({bad:?}) must refuse");
        }
    }

    #[test]
    fn binary_skew_warning_fires_on_build_id_mismatch_with_equal_versions() {
        let binaries = vec![
            serde_json::json!({
                "path": "/tmp/a/devmap",
                "version": "0.1.1",
                "build_id": "aaa111",
                "sha256": "aa".repeat(32),
            }),
            serde_json::json!({
                "path": "/tmp/b/devmap",
                "version": "0.1.1",
                "build_id": "bbb222",
                "sha256": "aa".repeat(32),
            }),
        ];
        let warning = binaries_skew_warning(&binaries)
            .expect("equal version strings with different build ids are skew");
        assert!(
            warning.contains("build") || warning.contains("aaa111") || warning.contains("bbb222"),
            "{warning}"
        );
    }

    /// A check that could not run must never report the same result as a check
    /// that ran and passed. Two binaries whose versions and build ids agree but
    /// whose bytes were never compared are *unverified*, not *identical*.
    #[test]
    fn a_hash_that_could_not_be_taken_is_not_reported_as_agreement() {
        let binaries = vec![
            serde_json::json!({
                "path": "/tmp/a/devmap",
                "version": "0.2.1",
                "build_id": "aaa111",
                "sha256": "aa".repeat(32),
                "sha256_status": "hashed",
            }),
            serde_json::json!({
                "path": "/tmp/b/devmap",
                "version": "0.2.1",
                "build_id": "aaa111",
                "sha256": serde_json::Value::Null,
                "sha256_status": "unavailable",
                "sha256_error": "not hashed: exceeded the 3s shared budget",
            }),
        ];
        let warning = binaries_skew_warning(&binaries)
            .expect("a hash comparison that never ran must not read as 'no skew'");
        assert!(warning.contains("/tmp/b/devmap"), "{warning}");
        assert!(warning.contains("unverified"), "{warning}");
    }

    /// …and the gap is appended to a real skew report too, because the sha256
    /// list that report prints is then a subset of the binaries found.
    #[test]
    fn a_skew_report_says_when_its_hash_list_is_incomplete() {
        let binaries = vec![
            serde_json::json!({
                "path": "/tmp/a/devmap",
                "version": "0.2.1",
                "build_id": "aaa111",
                "sha256": "aa".repeat(32),
                "sha256_status": "hashed",
            }),
            serde_json::json!({
                "path": "/tmp/b/devmap",
                "version": "0.2.0",
                "build_id": "bbb222",
                "sha256": serde_json::Value::Null,
                "sha256_status": "unavailable",
                "sha256_error": "not hashed: exceeded the 3s shared budget",
            }),
        ];
        let warning = binaries_skew_warning(&binaries).expect("differing versions are skew");
        assert!(warning.contains("0.2.0"), "{warning}");
        assert!(warning.contains("incomplete"), "{warning}");
    }

    #[test]
    fn missing_binary_warning_fires_when_a_listed_path_is_not_a_file() {
        let binaries = vec![
            serde_json::json!({
                "path": "/tmp/a/devmap",
                "source": "PATH",
                "version": "0.2.0",
                "build_id": "aaa111",
                "sha256": "aa".repeat(32),
                "exists": true,
            }),
            serde_json::json!({
                "path": "/no/such/devmap-binary",
                "source": "~/.claude.json",
                "version": serde_json::Value::Null,
                "build_id": serde_json::Value::Null,
                "sha256": serde_json::Value::Null,
                "exists": false,
            }),
        ];
        let warning = missing_binary_warning(&binaries)
            .expect("a host config path that is not a file is a distinct warning from skew");
        assert!(warning.contains("/no/such/devmap-binary"), "{warning}");
        assert!(
            binaries_skew_warning(&binaries).is_none(),
            "one real binary plus a missing path is not version skew"
        );
    }

    #[cfg(unix)]
    #[test]
    fn passive_identity_cannot_wait_for_a_hung_binary() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("devmap-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("devmap");
        std::fs::write(&program, "#!/bin/sh\nexec /bin/sleep 3\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = Instant::now();
        let probe = inspect_devmap_identity(&program);
        assert!(probe.version.is_none());
        assert_eq!(probe.status, "skipped");
        assert!(probe
            .error
            .as_deref()
            .unwrap()
            .contains("execution skipped"));
        std::fs::remove_dir_all(dir).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "passive inspection waited for the hung program"
        );
    }
}
