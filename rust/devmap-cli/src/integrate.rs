//! Host integration: global DevMap MCP registration and project assets.
//!
//! `devmap integrate <cursor|claude|codex>`:
//! - writes/refreshes marker-guarded guides and `.cursor/rules/devmap.mdc`
//! - installs the five embedded DevMap skills into the host's skill layout
//! - registers `devmap mcp` (no `--db`) in the global Cursor / Claude configs
//! - rewrites per-project owned entries to `--root <abs>` (belt-and-braces;
//!   insufficient for multi-tab Cursor — callers must still pass `repo_path`)

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use serde_json::{json, Map, Value};

use crate::claude::{self, CURSOR_HOOKS_MARKER, MCP_SERVER_NAME};
use crate::skills;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Cursor,
    Claude,
    Codex,
}

impl Host {
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        match name {
            "cursor" => Ok(Self::Cursor),
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            other => bail!("unsupported host {other:?}; expected cursor, claude, or codex"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn skill_destinations(self) -> &'static [&'static str] {
        match self {
            Self::Cursor => &[".cursor/skills"],
            Self::Claude => &[".claude/skills"],
            Self::Codex => &[".agents/skills"],
        }
    }

    fn registers_global_mcp(self) -> bool {
        matches!(self, Self::Cursor | Self::Claude)
    }

    fn registers_codex_mcp(self) -> bool {
        matches!(self, Self::Codex)
    }
}

#[derive(Debug, Default)]
pub struct IntegrateReport {
    pub guides: Vec<devmap_query::guides::GuideOutcome>,
    pub skills_written: Vec<PathBuf>,
    pub skills_differing: Vec<PathBuf>,
    pub global_mcp: Vec<McpMergeOutcome>,
    pub project_mcp: Vec<McpMergeOutcome>,
    pub hooks: Vec<McpMergeOutcome>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpMergeOutcome {
    pub path: PathBuf,
    pub changed: bool,
    pub removed_stale_db: bool,
    pub note: String,
}

/// Apply (or check / dry-run) DevMap host assets for one repository.
#[allow(clippy::too_many_arguments)]
pub fn integrate(
    host: Host,
    project_root: &Path,
    executable: &Path,
    map: &Value,
    map_rel: &str,
    graph_rel: &str,
    store_rel: &str,
    dry_run: bool,
    check: bool,
) -> anyhow::Result<IntegrateReport> {
    let root = project_root
        .canonicalize()
        .with_context(|| format!("project root {}", project_root.display()))?;

    let mut report = IntegrateReport::default();

    if dry_run || check {
        // Guides: compare without writing.
        let guide_text =
            devmap_query::guides::agent_guide_text(map, map_rel, graph_rel, store_rel) + "\n";
        for name in devmap_query::guides::GUIDE_FILENAMES {
            let path = root.join(name);
            let disposition = match fs::read_to_string(&path) {
                Ok(existing)
                    if existing.contains(devmap_query::guides::AGENT_GUIDE_MARKER)
                        || existing.contains(devmap_query::guides::LEGACY_AGENT_GUIDE_MARKER) =>
                {
                    if existing == guide_text {
                        devmap_query::guides::GuideDisposition::Unchanged
                    } else {
                        devmap_query::guides::GuideDisposition::Updated
                    }
                }
                Ok(_) => devmap_query::guides::GuideDisposition::NotOurs,
                Err(_) if !path.exists() => devmap_query::guides::GuideDisposition::Created,
                Err(_) => devmap_query::guides::GuideDisposition::NotOurs,
            };
            if matches!(
                disposition,
                devmap_query::guides::GuideDisposition::Created
                    | devmap_query::guides::GuideDisposition::Updated
            ) {
                report
                    .guides
                    .push(devmap_query::guides::GuideOutcome { path, disposition });
            }
        }
        let rule_path = root.join(devmap_query::guides::CURSOR_RULE_REL);
        let rule_text = devmap_query::guides::cursor_rule_text(map_rel);
        let rule_differs = fs::read_to_string(&rule_path)
            .map(|existing| existing != rule_text)
            .unwrap_or(true);
        if rule_differs {
            report.guides.push(devmap_query::guides::GuideOutcome {
                path: rule_path.clone(),
                disposition: if rule_path.exists() {
                    devmap_query::guides::GuideDisposition::Updated
                } else {
                    devmap_query::guides::GuideDisposition::Created
                },
            });
        }
    } else {
        report.guides =
            devmap_query::guides::write_agent_guides(&root, map, map_rel, graph_rel, store_rel)?;
    }

    let skill_report =
        skills::install_devmap_skills(&root, host.skill_destinations(), dry_run, check)?;
    report.skills_written = skill_report.written;
    report.skills_differing = skill_report.differing;

    if host.registers_global_mcp() {
        for path in global_mcp_paths(host)? {
            let outcome = merge_global_mcp(&path, executable, dry_run || check)?;
            report.global_mcp.push(outcome);
        }
    }

    if host.registers_codex_mcp() {
        let path = dirs_home()?.join(".codex").join("config.toml");
        let outcome = merge_codex_mcp(&path, executable, dry_run || check)?;
        report.global_mcp.push(outcome);
        report.notes.push(
            "Codex hooks require trust via `/hooks` after install (hash-based; \
             regenerating hooks re-requires trust)"
                .into(),
        );
    }

    if host == Host::Cursor {
        let path = root.join(".cursor").join("hooks.json");
        let outcome = merge_cursor_hooks(&path, executable, dry_run || check)?;
        report.hooks.push(outcome);
    }

    if host == Host::Codex {
        let plugin_dir = root.join(".codex-plugin");
        let hooks_dir = root.join("hooks");
        let outcome =
            write_codex_plugin_assets(&plugin_dir, &hooks_dir, executable, dry_run || check)?;
        report.hooks.extend(outcome);
    }

    // Clean stale `--db` registrations, then offer `--root` when the project
    // has no DevMap entry. `--root` is insufficient for multi-tab Cursor.
    for path in project_mcp_paths(host, &root) {
        if let Some(outcome) = clean_project_mcp(&path, executable, dry_run || check)? {
            report.project_mcp.push(outcome);
            continue;
        }
        if let Some(outcome) = offer_project_mcp(&path, executable, dry_run || check)? {
            report.project_mcp.push(outcome);
        }
    }

    if check {
        let guides_dirty = report.guides.iter().any(|g| g.changed());
        let skills_dirty = !report.skills_differing.is_empty();
        let mcp_dirty = report
            .global_mcp
            .iter()
            .chain(report.project_mcp.iter())
            .chain(report.hooks.iter())
            .any(|m| m.changed);
        if guides_dirty || skills_dirty || mcp_dirty || !skill_report.check_ok {
            bail!("integrate --check: assets differ from the expected DevMap installation");
        }
    }

    Ok(report)
}

fn global_mcp_paths(host: Host) -> anyhow::Result<Vec<PathBuf>> {
    let home = dirs_home()?;
    Ok(match host {
        Host::Cursor => vec![home.join(".cursor").join("mcp.json")],
        Host::Claude => vec![home.join(".claude.json")],
        Host::Codex => Vec::new(),
    })
}

fn project_mcp_paths(host: Host, root: &Path) -> Vec<PathBuf> {
    match host {
        Host::Cursor => vec![root.join(".cursor").join("mcp.json")],
        Host::Claude => vec![
            root.join(".mcp.json"),
            root.join(".claude").join("mcp.json"),
        ],
        Host::Codex => Vec::new(),
    }
}

fn dirs_home() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is unset; cannot register global MCP"))
}

/// Merge `devmap mcp` (no `--db`) into a global host config without clobbering
/// unrelated servers.
pub fn merge_global_mcp(
    path: &Path,
    executable: &Path,
    read_only: bool,
) -> anyhow::Result<McpMergeOutcome> {
    let entry = claude::mcp_entry(executable, None, None)?;
    let mut document = read_json_object(path)?;
    let servers = mcp_servers_mut(&mut document)?;
    let mut removed_stale_db = false;
    let mut changed = false;

    match servers.get(MCP_SERVER_NAME) {
        Some(existing) if existing == &entry => {}
        Some(existing) if is_owned_devmap_entry(existing) => {
            // Replace our stale `--db` form (or older absolute-path args).
            if entry_has_db_arg(existing) {
                removed_stale_db = true;
            }
            if !read_only {
                servers.insert(MCP_SERVER_NAME.to_string(), entry.clone());
            }
            changed = true;
        }
        Some(_) => {
            // Foreign `devmap` entry — leave it, report.
            return Ok(McpMergeOutcome {
                path: path.to_path_buf(),
                changed: false,
                removed_stale_db: false,
                note: "devmap entry present but not owned by this installer; left unchanged".into(),
            });
        }
        None => {
            if !read_only {
                servers.insert(MCP_SERVER_NAME.to_string(), entry);
            }
            changed = true;
        }
    }

    if changed && !read_only {
        write_json_pretty(path, &document)?;
    }

    Ok(McpMergeOutcome {
        path: path.to_path_buf(),
        changed,
        removed_stale_db,
        note: if removed_stale_db {
            "registered global devmap mcp without --db (replaced stale --db entry)".into()
        } else if changed {
            "registered global devmap mcp without --db".into()
        } else {
            "global devmap mcp already current".into()
        },
    })
}

/// Rewrite a per-project owned `devmap` entry to `devmap --root <abs> mcp`.
///
/// Covers stale `--db` and the belt-and-braces `["mcp"]` form. `--root` is
/// insufficient for multi-tab Cursor; callers must still pass `repo_path`.
pub fn clean_project_mcp(
    path: &Path,
    executable: &Path,
    read_only: bool,
) -> anyhow::Result<Option<McpMergeOutcome>> {
    if !path.is_file() {
        return Ok(None);
    }
    let mut document = read_json_object(path)?;
    let Some(servers) = document
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
    else {
        return Ok(None);
    };
    let Some(existing) = servers.get(MCP_SERVER_NAME) else {
        return Ok(None);
    };
    if !is_owned_devmap_entry(existing) {
        return Ok(None);
    }
    let project_root = project_root_from_mcp_path(path)?;
    let replacement = claude::mcp_entry_with_root(executable, &project_root, None)?;
    if existing == &replacement {
        return Ok(None);
    }
    let removed_stale_db = entry_has_db_arg(existing);
    if !read_only {
        servers.insert(MCP_SERVER_NAME.to_string(), replacement);
        write_json_pretty(path, &document)?;
    }
    Ok(Some(McpMergeOutcome {
        path: path.to_path_buf(),
        changed: true,
        removed_stale_db,
        note: format!(
            "replaced per-project DevMap entry with `--root {}`; `--root` is insufficient for \
multi-tab Cursor (one shared MCP process) — always pass repo_path",
            project_root.display()
        ),
    }))
}

/// Belt-and-braces: write `devmap --root <abs> mcp` when the project has no
/// DevMap entry. Insufficient for multi-tab Cursor — that host shares one
/// process, so callers must still pass `repo_path`.
pub fn offer_project_mcp(
    path: &Path,
    executable: &Path,
    read_only: bool,
) -> anyhow::Result<Option<McpMergeOutcome>> {
    let existed = path.is_file();
    let mut document = if existed {
        read_json_object(path)?
    } else {
        json!({"mcpServers": {}})
    };
    {
        let servers = match document.get("mcpServers").and_then(Value::as_object) {
            Some(servers) => servers,
            None => return Ok(None),
        };
        if servers.get(MCP_SERVER_NAME).is_some() {
            return Ok(None);
        }
    }
    let project_root = project_root_from_mcp_path(path)?;
    let replacement = claude::mcp_entry_with_root(executable, &project_root, None)?;
    if !read_only {
        let servers = mcp_servers_mut(&mut document)?;
        servers.insert(MCP_SERVER_NAME.to_string(), replacement);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_json_pretty(path, &document)?;
    }
    Ok(Some(McpMergeOutcome {
        path: path.to_path_buf(),
        changed: true,
        removed_stale_db: false,
        note: format!(
            "wrote per-project `--root {}`; `--root` is insufficient for multi-tab Cursor \
(one shared MCP process) — always pass repo_path",
            project_root.display()
        ),
    }))
}

fn is_owned_devmap_entry(entry: &Value) -> bool {
    let Some(args) = entry.get("args").and_then(Value::as_array) else {
        return false;
    };
    // Owned shapes: `["mcp"]` or `["--db", <path>, "mcp"]` with optional type/command.
    args.last().and_then(Value::as_str) == Some("mcp")
}

fn project_root_from_mcp_path(path: &Path) -> anyhow::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        anyhow!(
            "{}: cannot infer project root from mcp.json path",
            path.display()
        )
    })?;
    let config_dir = parent
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let root = if matches!(config_dir, ".cursor" | ".claude") {
        parent.parent().ok_or_else(|| {
            anyhow!(
                "{}: cannot infer project root from mcp.json path",
                path.display()
            )
        })?
    } else {
        parent
    };
    Ok(root.canonicalize().unwrap_or_else(|_| root.to_path_buf()))
}

fn entry_has_db_arg(entry: &Value) -> bool {
    entry
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| args.iter().any(|a| a.as_str() == Some("--db")))
}

fn read_json_object(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({"mcpServers": {}}));
    }
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value: Value = serde_json::from_str(&text).map_err(|err| {
        anyhow!(
            "{}: not strict JSON ({err}); JSON-with-comments is refused rather than rewritten",
            path.display()
        )
    })?;
    if !value.is_object() {
        bail!("{}: top-level JSON must be an object", path.display());
    }
    Ok(value)
}

fn mcp_servers_mut(document: &mut Value) -> anyhow::Result<&mut Map<String, Value>> {
    if document.get("mcpServers").is_none() {
        document
            .as_object_mut()
            .unwrap()
            .insert("mcpServers".into(), json!({}));
    }
    document
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("mcpServers must be an object"))
}

fn write_json_pretty(path: &Path, value: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(devmap_query::write_atomic(path, text.as_bytes()).map(|_| ())?)
}

/// Marker-owned Cursor `.cursor/hooks.json` merge.
pub fn merge_cursor_hooks(
    path: &Path,
    executable: &Path,
    read_only: bool,
) -> anyhow::Result<McpMergeOutcome> {
    let expected = claude::cursor_hooks_document(executable)?;
    if path.is_file() {
        let existing_text = fs::read_to_string(path)?;
        let existing: Value = serde_json::from_str(&existing_text).map_err(|err| {
            anyhow!(
                "{}: not JSON ({err}); refuse to overwrite a broken hooks file",
                path.display()
            )
        })?;
        if existing.get("generatedBy").and_then(Value::as_str) != Some(CURSOR_HOOKS_MARKER) {
            return Ok(McpMergeOutcome {
                path: path.to_path_buf(),
                changed: false,
                removed_stale_db: false,
                note: "hooks.json present but not owned by DevMap; left unchanged".into(),
            });
        }
        if existing == expected {
            return Ok(McpMergeOutcome {
                path: path.to_path_buf(),
                changed: false,
                removed_stale_db: false,
                note: "cursor hooks already current".into(),
            });
        }
    }
    if !read_only {
        write_json_pretty(path, &expected)?;
    }
    Ok(McpMergeOutcome {
        path: path.to_path_buf(),
        changed: true,
        removed_stale_db: false,
        note: "wrote marker-owned .cursor/hooks.json (sessionStart, afterFileEdit, sessionEnd)"
            .into(),
    })
}

/// Merge `[mcp_servers.devmap]` into `~/.codex/config.toml` without touching
/// other servers.
pub fn merge_codex_mcp(
    path: &Path,
    executable: &Path,
    read_only: bool,
) -> anyhow::Result<McpMergeOutcome> {
    let command = executable
        .canonicalize()
        .unwrap_or_else(|_| executable.to_path_buf());
    let command = command
        .to_str()
        .ok_or_else(|| anyhow!("executable path is not UTF-8"))?;

    let existing = if path.is_file() {
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };

    let mut table: toml::Table = if existing.trim().is_empty() {
        toml::Table::new()
    } else {
        existing
            .parse()
            .map_err(|err| anyhow!("{}: not TOML ({err})", path.display()))?
    };

    let servers = table
        .entry("mcp_servers")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let servers = servers
        .as_table_mut()
        .ok_or_else(|| anyhow!("{}: mcp_servers must be a table", path.display()))?;

    let mut entry = toml::Table::new();
    entry.insert("command".into(), toml::Value::String(command.to_string()));
    entry.insert(
        "args".into(),
        toml::Value::Array(vec![toml::Value::String("mcp".into())]),
    );
    let entry_val = toml::Value::Table(entry);

    let changed = match servers.get("devmap") {
        Some(existing) if existing == &entry_val => false,
        _ => {
            if !read_only {
                servers.insert("devmap".into(), entry_val);
            }
            true
        }
    };

    if changed && !read_only {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(&toml::Value::Table(table))
            .map_err(|err| anyhow!("serialize Codex config: {err}"))?;
        fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    }

    Ok(McpMergeOutcome {
        path: path.to_path_buf(),
        changed,
        removed_stale_db: false,
        note: if changed {
            "merged [mcp_servers.devmap] into Codex config.toml".into()
        } else {
            "Codex mcp_servers.devmap already current".into()
        },
    })
}

/// Write `.codex-plugin/plugin.json` and `.codex-plugin/hooks/hooks.json`.
pub fn write_codex_plugin_assets(
    plugin_dir: &Path,
    _hooks_dir: &Path,
    executable: &Path,
    read_only: bool,
) -> anyhow::Result<Vec<McpMergeOutcome>> {
    let mut out = Vec::new();
    let version = env!("CARGO_PKG_VERSION");
    let manifest = claude::codex_plugin_manifest(Some(version))?;
    let manifest_path = plugin_dir.join("plugin.json");
    let hooks = claude::hooks_block(executable, &["hook".to_string()])?;
    let hooks_path = plugin_dir.join("hooks").join("hooks.json");

    for (path, value, note) in [
        (&manifest_path, &manifest, "wrote .codex-plugin/plugin.json"),
        (&hooks_path, &hooks, "wrote .codex-plugin/hooks/hooks.json"),
    ] {
        let changed = if path.is_file() {
            let existing = fs::read_to_string(path).unwrap_or_default();
            let pretty = {
                let mut text = serde_json::to_string_pretty(value)?;
                text.push('\n');
                text
            };
            existing != pretty
        } else {
            true
        };
        if changed && !read_only {
            write_json_pretty(path, value)?;
        }
        if changed {
            out.push(McpMergeOutcome {
                path: path.to_path_buf(),
                changed: true,
                removed_stale_db: false,
                note: note.into(),
            });
        }
    }
    Ok(out)
}

/// Build a minimal map value for integrate when the store is unavailable.
pub fn empty_map() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "devmap-integrate-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn merge_registers_without_db_and_preserves_neighbors() {
        let dir = scratch("merge");
        let path = dir.join("mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"type":"stdio","command":"x","args":[]}}}"#,
        )
        .unwrap();
        let exe = PathBuf::from("/tmp/fake-devmap-bin");
        let outcome = merge_global_mcp(&path, &exe, false).unwrap();
        assert!(outcome.changed);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["mcpServers"]["other"].is_object());
        assert_eq!(value["mcpServers"]["devmap"]["args"], json!(["mcp"]));
        assert!(value["mcpServers"]["devmap"]
            .get("args")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a.as_str() != Some("--db")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_replaces_owned_stale_db_entry() {
        let dir = scratch("stale-db");
        let path = dir.join("mcp.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"devmap":{"type":"stdio","command":"/old/devmap","args":["--db","/repo/.devcouncil/codeintel/devmap.sqlite","mcp"]}}}"#,
        )
        .unwrap();
        let exe = PathBuf::from("/tmp/new-devmap");
        let outcome = merge_global_mcp(&path, &exe, false).unwrap();
        assert!(outcome.changed);
        assert!(outcome.removed_stale_db);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["mcpServers"]["devmap"]["args"], json!(["mcp"]));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_project_rewrites_stale_db() {
        let dir = scratch("project-db");
        let path = dir.join(".cursor").join("mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"mcpServers":{"devmap":{"type":"stdio","command":"/x","args":["--db","y","mcp"]},"keep":{"type":"stdio","command":"z","args":[]}}}"#,
        )
        .unwrap();
        let outcome = clean_project_mcp(&path, Path::new("/x"), false)
            .unwrap()
            .unwrap();
        assert!(outcome.removed_stale_db);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["mcpServers"]["keep"].is_object());
        let args = value["mcpServers"]["devmap"]["args"]
            .as_array()
            .expect("args");
        assert_eq!(args[0], "--root", "{value}");
        assert_eq!(args[2], "mcp", "{value}");
        let root = Path::new(args[1].as_str().unwrap());
        assert!(
            root.is_absolute(),
            "project --root must be absolute: {root:?}"
        );
        assert!(
            outcome.note.to_lowercase().contains("multi-tab")
                || outcome.note.to_lowercase().contains("insufficient"),
            "must document that --root is insufficient for multi-tab: {}",
            outcome.note
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn offer_project_mcp_creates_root_entry_when_missing() {
        let dir = scratch("offer-mcp");
        let path = dir.join(".cursor").join("mcp.json");
        let outcome = offer_project_mcp(&path, Path::new("/x"), false)
            .unwrap()
            .unwrap();
        assert!(outcome.changed);
        assert!(!outcome.removed_stale_db);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let args = value["mcpServers"]["devmap"]["args"]
            .as_array()
            .expect("args");
        assert_eq!(args[0], "--root", "{value}");
        assert_eq!(args[2], "mcp", "{value}");
        assert!(
            outcome.note.to_lowercase().contains("multi-tab")
                || outcome.note.to_lowercase().contains("insufficient"),
            "must document that --root is insufficient for multi-tab: {}",
            outcome.note
        );
        assert!(offer_project_mcp(&path, Path::new("/x"), false)
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn owned_bare_mcp_project_entry_is_rewritten_to_root() {
        let dir = scratch("bare-mcp");
        let path = dir.join(".cursor").join("mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"mcpServers":{"devmap":{"type":"stdio","command":"/x","args":["mcp"]},"keep":{"type":"stdio","command":"z","args":[]}}}"#,
        )
        .unwrap();
        let outcome = clean_project_mcp(&path, Path::new("/x"), false)
            .unwrap()
            .expect("owned [\"mcp\"] project entry must become --root");
        assert!(outcome.changed);
        assert!(!outcome.removed_stale_db);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(value["mcpServers"]["keep"].is_object());
        let args = value["mcpServers"]["devmap"]["args"]
            .as_array()
            .expect("args");
        assert_eq!(args[0], "--root", "{value}");
        assert_eq!(args[2], "mcp", "{value}");
        assert!(
            outcome.note.to_lowercase().contains("multi-tab")
                || outcome.note.to_lowercase().contains("insufficient"),
            "must document that --root is insufficient for multi-tab: {}",
            outcome.note
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cursor_hooks_are_marker_owned_camel_case() {
        let dir = scratch("cursor-hooks");
        let path = dir.join(".cursor").join("hooks.json");
        let outcome = merge_cursor_hooks(&path, Path::new("/tmp/fake-devmap"), false).unwrap();
        assert!(outcome.changed);
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["generatedBy"], "devmap");
        assert_eq!(value["version"], 1);
        assert!(value["hooks"]["sessionStart"].is_array());
        assert!(value["hooks"]["afterFileEdit"].is_array());
        assert!(value["hooks"]["sessionEnd"].is_array());
        let cmd = value["hooks"]["sessionStart"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("hook session-start"), "{cmd}");
        assert!(!cmd.contains("\"args\""), "{cmd}");
        // Second merge is a no-op.
        let again = merge_cursor_hooks(&path, Path::new("/tmp/fake-devmap"), false).unwrap();
        assert!(!again.changed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_mcp_merge_preserves_other_servers() {
        let dir = scratch("codex-mcp");
        let path = dir.join("config.toml");
        fs::write(&path, "[mcp_servers.other]\ncommand = \"x\"\nargs = []\n").unwrap();
        let outcome = merge_codex_mcp(&path, Path::new("/tmp/fake-devmap"), false).unwrap();
        assert!(outcome.changed);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("mcp_servers"));
        assert!(text.contains("devmap"));
        assert!(text.contains("other"));
        assert!(text.contains("mcp"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_plugin_assets_are_written() {
        let dir = scratch("codex-plugin");
        let plugin = dir.join(".codex-plugin");
        let hooks = dir.join("hooks");
        let outcomes =
            write_codex_plugin_assets(&plugin, &hooks, Path::new("/tmp/fake-devmap"), false)
                .unwrap();
        assert!(!outcomes.is_empty());
        assert!(plugin.join("plugin.json").is_file());
        assert!(plugin.join("hooks").join("hooks.json").is_file());
        let hooks_doc: Value = serde_json::from_str(
            &fs::read_to_string(plugin.join("hooks").join("hooks.json")).unwrap(),
        )
        .unwrap();
        assert!(hooks_doc["hooks"]["PostToolUse"].is_array());
        let _ = fs::remove_dir_all(&dir);
    }
}
