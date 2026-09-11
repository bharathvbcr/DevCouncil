//! Host integration: global DevMap MCP registration and project assets.
//!
//! `devmap integrate <cursor|claude|codex>`:
//! - writes/refreshes marker-guarded guides and `.cursor/rules/devmap.mdc`
//! - installs the five embedded DevMap skills into the host's skill layout
//! - registers `devmap mcp` (no `--db`) in the global Cursor / Claude configs
//! - removes stale per-project `--db` entries this tool owns

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use serde_json::{json, Map, Value};

use crate::claude::{self, MCP_SERVER_NAME};
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
}

#[derive(Debug, Default)]
pub struct IntegrateReport {
    pub guides: Vec<devmap_query::guides::GuideOutcome>,
    pub skills_written: Vec<PathBuf>,
    pub skills_differing: Vec<PathBuf>,
    pub global_mcp: Vec<McpMergeOutcome>,
    pub project_mcp: Vec<McpMergeOutcome>,
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

    // Clean per-project configs this tool owns (stale `--db` registrations).
    for path in project_mcp_paths(host, &root) {
        if let Some(outcome) = clean_project_mcp(&path, executable, dry_run || check)? {
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

/// Remove or rewrite a per-project `devmap` entry that still carries `--db`.
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
    if !is_owned_devmap_entry(existing) || !entry_has_db_arg(existing) {
        return Ok(None);
    }
    // Prefer removing the per-project entry once global registration exists;
    // leave a no-`--db` entry only when this is the sole registration surface.
    let replacement = claude::mcp_entry(executable, None, None)?;
    if !read_only {
        servers.insert(MCP_SERVER_NAME.to_string(), replacement);
        write_json_pretty(path, &document)?;
    }
    Ok(Some(McpMergeOutcome {
        path: path.to_path_buf(),
        changed: true,
        removed_stale_db: true,
        note: "replaced per-project --db entry with root-resolving `devmap mcp`".into(),
    }))
}

fn is_owned_devmap_entry(entry: &Value) -> bool {
    let Some(args) = entry.get("args").and_then(Value::as_array) else {
        return false;
    };
    // Owned shapes: `["mcp"]` or `["--db", <path>, "mcp"]` with optional type/command.
    args.last().and_then(Value::as_str) == Some("mcp")
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
        assert_eq!(value["mcpServers"]["devmap"]["args"], json!(["mcp"]));
        let _ = fs::remove_dir_all(&dir);
    }
}
