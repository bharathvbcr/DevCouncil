//! Host integration: global DevMap MCP registration and project assets.
//!
//! `devmap integrate <cursor|claude|codex>`:
//! - writes/refreshes marker-guarded guides and `.cursor/rules/devmap.mdc`
//! - installs the five embedded DevMap skills into the host's skill layout
//! - registers `devmap mcp` (no `--db`) in the global Cursor / Claude configs
//! - rewrites per-project owned entries to `--root <abs>` (belt-and-braces;
//!   insufficient for multi-tab Cursor — callers must still pass `repo_path`)

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use serde_json::{json, Map, Value};

use crate::claude::{self, CURSOR_HOOKS_MARKER, MCP_SERVER_NAME};
use crate::skills;

/// A host `devmap integrate` can configure.
///
/// The variants are the single list of *names*: clap derives the positional's
/// accepted values and its `--help` from them, and [`Host::parse`] and the
/// refusal it returns are generated from the same list, so none of the three
/// can advertise a host the others do not accept.
///
/// A new host still needs its arm in each match below — that is per-host
/// behaviour, and the compiler asks for it. What it no longer needs is for
/// anyone to remember a list of names written somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Host {
    Cursor,
    Claude,
    Codex,
    /// Google Antigravity CLI. Reads `.agents/mcp_config.json`, the same
    /// `mcpServers` document shape Cursor and Claude use, and shares
    /// `.agents/skills` with Codex.
    Antigravity,
    /// OpenCode. Reads `opencode.json` at the repository root — a *user-owned*
    /// file, not a dot-directory this tool owns — under an `mcp` key whose
    /// entries name the program as an argv array rather than command+args.
    // clap's default rename would spell this `open-code`; the host is one word.
    #[value(name = "opencode")]
    OpenCode,
    /// Warp / Oz. Reads `.devcouncil/integrations/warp-mcp.json`, a file
    /// DevCouncil writes and passes explicitly via `oz agent run --mcp`, so
    /// the document is a bare server map with no wrapper key.
    Warp,
}

impl Host {
    /// Parse a host name, for callers holding a string rather than a parsed
    /// argument. `devmap integrate` itself gets its `Host` from clap.
    ///
    /// Acceptance and refusal are both generated from the variant list, so the
    /// message cannot name a set of hosts the parser does not accept — the way
    /// the `--help` text came to promise three of six.
    ///
    /// Retained, not wired: since the positional is typed, the only in-tree
    /// caller is the test below. The attribute states that rather than letting
    /// a blanket allow hide a later unwiring.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        <Self as clap::ValueEnum>::from_str(name, false).map_err(|_| {
            anyhow!(
                "unsupported host {name:?}; expected {}",
                Self::accepted_names()
            )
        })
    }

    /// The accepted names as prose: `a, b, or c`.
    #[cfg_attr(not(test), allow(dead_code))]
    fn accepted_names() -> String {
        let names: Vec<&str> = <Self as clap::ValueEnum>::value_variants()
            .iter()
            .map(|host| host.as_str())
            .collect();
        match names.as_slice() {
            // Unreachable while the enum has variants, but a refusal that had
            // nothing to name must say so rather than trail off after "expected".
            [] => "no host: this build accepts none".to_string(),
            [only] => (*only).to_string(),
            [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
        }
    }

    /// The canonical name of this host, as typed on the command line.
    ///
    /// Tied to clap's own accepted values by a test below, and read out of this
    /// source by the Go integrator's drift check
    /// (`backend/go_orchestrator/devcouncil/integrate/host_selection_test.go`),
    /// which needs the arms of this match to stay literal.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
            Self::OpenCode => "opencode",
            Self::Warp => "warp",
        }
    }

    fn skill_destinations(self) -> &'static [&'static str] {
        match self {
            Self::Cursor => &[".cursor/skills"],
            Self::Claude => &[".claude/skills"],
            // Antigravity reads the same `.agents` tree Codex does, so the
            // skills a Codex integration already installed are the skills it
            // sees; writing them twice would be the same bytes.
            Self::Codex | Self::Antigravity => &[".agents/skills"],
            // Neither host documents a project skill directory. Writing one on
            // a guess would leave files no host reads.
            Self::OpenCode | Self::Warp => &[],
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
            let disposition = match read_host_config(&path) {
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
        let rule_differs = read_host_config(&rule_path)
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

    // A host with no documented project skill directory installs no skills.
    // That is a state, not a malformed request: the installer's 1-16
    // destination bound guards against a caller passing nonsense, and handing
    // it an empty slice to mean "nowhere" tripped that guard instead.
    // Vacuously satisfied where nothing is installed: a check that had no
    // destinations to inspect must not report the same failure as one that
    // inspected a destination and found it wrong.
    let mut skills_check_ok = true;
    if host.skill_destinations().is_empty() {
        report.notes.push(format!(
            "{}: no project skill directory is documented for this host; \
             skills were not installed",
            host.as_str()
        ));
    } else {
        let skill_report =
            skills::install_devmap_skills(&root, host.skill_destinations(), dry_run, check)?;
        report.skills_written = skill_report.written;
        report.skills_differing = skill_report.differing;
        skills_check_ok = skill_report.check_ok;
    }

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

    // Hosts whose project document this module owns end-to-end. Cursor and
    // Claude are handled by the `mcpServers` merge further down, which also
    // reaches their user-level files; these three have no user-level document.
    if let Some(outcome) = merge_host_mcp(host, &root, executable, dry_run || check)? {
        report.project_mcp.push(outcome);
    }

    // OpenCode has no hooks document: it loads an ES module named in
    // `opencode.json`. Antigravity and Warp are absent here on purpose —
    // neither documents a project hook mechanism, and a handler written on a
    // guess is a file the host never calls.
    if host == Host::OpenCode {
        report
            .hooks
            .extend(merge_opencode_plugin(&root, executable, dry_run || check)?);
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
        if guides_dirty || skills_dirty || mcp_dirty || !skills_check_ok {
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
        // Project-scoped only. Antigravity and OpenCode keep their server list
        // beside the repository, and Warp's file is handed to `oz` per run, so
        // there is no user-level document to merge into for any of them.
        Host::Codex | Host::Antigravity | Host::OpenCode | Host::Warp => Vec::new(),
    })
}

fn project_mcp_paths(host: Host, root: &Path) -> Vec<PathBuf> {
    match host {
        Host::Cursor => vec![root.join(".cursor").join("mcp.json")],
        Host::Claude => vec![
            root.join(".mcp.json"),
            root.join(".claude").join("mcp.json"),
        ],
        // The three below are written by `host_mcp_document`, which knows each
        // one's container key and entry shape. They are deliberately absent
        // here: this list feeds the `mcpServers` merge, and two of them do not
        // use that key at all.
        Host::Codex | Host::Antigravity | Host::OpenCode | Host::Warp => Vec::new(),
    }
}

/// Where one host keeps its server list, and how that document is shaped.
///
/// Three hosts, three shapes, and the differences are load-bearing:
/// Antigravity nests servers under `mcpServers`, OpenCode under `mcp`, and
/// Warp's file *is* the server map with no wrapper at all. Declaring the shape
/// once, here, is what lets a single writer serve all of them rather than
/// three near-copies that drift.
struct HostMcpDoc {
    /// Path relative to the repository root.
    rel: &'static str,
    /// Key the server map lives under, or `None` when the document root is it.
    container: Option<&'static str>,
    /// Extra top-level keys to establish when creating the file.
    preamble: &'static [(&'static str, &'static str)],
    /// Whether entries name the program as an argv array (`command: [...]`)
    /// instead of `command` + `args`.
    argv_form: bool,
}

impl Host {
    /// The project-scoped server document this host reads, if this module
    /// writes one for it.
    ///
    /// Cursor and Claude are absent because their documents go through the
    /// `mcpServers` merge in [`offer_project_mcp`], which also handles their
    /// global counterparts. Codex registers through its own TOML writer.
    fn mcp_document(self) -> Option<HostMcpDoc> {
        match self {
            Self::Cursor | Self::Claude | Self::Codex => None,
            Self::Antigravity => Some(HostMcpDoc {
                rel: ".agents/mcp_config.json",
                container: Some("mcpServers"),
                preamble: &[],
                argv_form: false,
            }),
            Self::OpenCode => Some(HostMcpDoc {
                rel: "opencode.json",
                container: Some("mcp"),
                // OpenCode validates against this schema; a file created
                // without it loses editor completion for every other key.
                preamble: &[("$schema", "https://opencode.ai/config.json")],
                argv_form: true,
            }),
            Self::Warp => Some(HostMcpDoc {
                rel: ".devcouncil/integrations/warp-mcp.json",
                container: None,
                preamble: &[],
                argv_form: false,
            }),
        }
    }
}

/// Register `devmap` in one host's project server document.
///
/// The entry is always [`claude::mcp_entry_with_root`]'s shape — the absolute
/// path of the binary that was validated, and `--root <abs>` rather than a
/// pinned `--db`. Both halves matter and both were wrong in the configurations
/// this replaces: a bare `devmap` hands the host whichever build wins on PATH,
/// and a baked store path fixes the state layout as of the day it was written,
/// which is not a property of the binary but of the repository.
///
/// Every other key in the file survives. `opencode.json` in particular is a
/// *user's* configuration that happens to hold a server list, not a file this
/// tool owns, so a write that dropped an unrelated key would be a bug even if
/// the server entry came out right.
pub fn merge_host_mcp(
    host: Host,
    root: &Path,
    executable: &Path,
    dry_run: bool,
) -> anyhow::Result<Option<McpMergeOutcome>> {
    let Some(doc) = host.mcp_document() else {
        return Ok(None);
    };
    let path = root.join(doc.rel);

    let mut document = match read_host_config(&path) {
        Ok(text) if text.trim().is_empty() => empty_map(),
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(|err| {
            // Refuse rather than overwrite: an unparseable config is far more
            // likely to be a file worth keeping than one worth replacing.
            anyhow!(
                "{}: not JSON ({err}); refuse to overwrite a host config this \
                 module cannot read",
                path.display()
            )
        })?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => empty_map(),
        Err(err) => return Err(anyhow!("{}: {err}", path.display())),
    };
    if !document.is_object() {
        bail!(
            "{}: top level is {}, not a JSON object",
            path.display(),
            kind_of(&document)
        );
    }

    for (key, value) in doc.preamble {
        document
            .as_object_mut()
            .expect("object checked above")
            .entry(key.to_string())
            .or_insert_with(|| json!(value));
    }

    let entry = host_mcp_entry(&doc, executable, root)?;
    let before = servers_of(&document, &doc).cloned();
    if before.as_ref() == Some(&entry) {
        return Ok(Some(McpMergeOutcome {
            path,
            changed: false,
            removed_stale_db: false,
            note: format!("{} already current", doc.rel),
        }));
    }
    // Reported rather than folded into "changed": replacing a pinned store
    // path is the repair this exists for, and a receipt that does not
    // distinguish it from a first-time write cannot show the repair happened.
    let removed_stale_db = before.as_ref().is_some_and(entry_names_a_store);

    servers_mut(&mut document, &doc)?.insert(MCP_SERVER_NAME.to_string(), entry);

    if !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        write_json_pretty(&path, &document)?;
    }
    Ok(Some(McpMergeOutcome {
        path,
        changed: true,
        removed_stale_db,
        note: if removed_stale_db {
            format!("{}: replaced a pinned --db entry with --root", doc.rel)
        } else {
            format!("{}: registered devmap", doc.rel)
        },
    }))
}

/// The devmap entry in this host's spelling.
fn host_mcp_entry(doc: &HostMcpDoc, executable: &Path, root: &Path) -> anyhow::Result<Value> {
    let standard = claude::mcp_entry_with_root(executable, root, None)?;
    if !doc.argv_form {
        return Ok(standard);
    }
    // OpenCode names the program and its arguments as one argv array. Build it
    // from the standard entry rather than re-deriving the path and flags, so
    // the two spellings cannot disagree about what gets run.
    let command = standard
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("mcp entry has no command"))?;
    let mut argv = vec![json!(command)];
    for arg in standard
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        argv.push(arg.clone());
    }
    Ok(json!({
        "type": "local",
        "command": Value::Array(argv),
        "enabled": true,
        "timeout": 10_000,
    }))
}

/// True when an existing entry pins a store path, in either spelling.
fn entry_names_a_store(entry: &Value) -> bool {
    if entry_has_db_arg(entry) {
        return true;
    }
    entry
        .get("command")
        .and_then(Value::as_array)
        .is_some_and(|argv| argv.iter().any(|a| a.as_str() == Some("--db")))
}

fn servers_of<'a>(document: &'a Value, doc: &HostMcpDoc) -> Option<&'a Value> {
    match doc.container {
        None => document.get(MCP_SERVER_NAME),
        Some(key) => document.get(key).and_then(|map| map.get(MCP_SERVER_NAME)),
    }
}

fn servers_mut<'a>(
    document: &'a mut Value,
    doc: &HostMcpDoc,
) -> anyhow::Result<&'a mut Map<String, Value>> {
    let Some(key) = doc.container else {
        return document
            .as_object_mut()
            .ok_or_else(|| anyhow!("document root is not an object"));
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| anyhow!("document root is not an object"))?;
    let slot = root.entry(key.to_string()).or_insert_with(empty_map);
    if !slot.is_object() {
        bail!("`{key}` is {}, not a JSON object", kind_of(slot));
    }
    slot.as_object_mut()
        .ok_or_else(|| anyhow!("`{key}` is not an object"))
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// OpenCode reaches hooks through an ES module named in `opencode.json`'s
/// `plugin` key, not through a hooks document.
///
/// The module name is DevMap's claim on that file: a plugin the user wrote
/// keeps its own name and is left alone.
const OPENCODE_PLUGIN_FILE: &str = "opencode_devmap_plugin.mjs";

/// The one event with evidence behind it.
///
/// The retired DevCouncil plugin this shape is recovered from registered
/// `tool.execute.after` and nothing else. Session start and end are not
/// emitted here — not because a refresh at those moments would be useless, but
/// because no handler name for them is documented, and a listener registered
/// under a guessed key is a file the host silently never calls.
const OPENCODE_AFTER_TOOL: &str = "tool.execute.after";

/// The plugin body. `devmap hook post-tool-use` returns within its own budget
/// and detaches the rebuild itself, so a synchronous spawn here does not hold
/// the agent's tool loop open — that bound is the reason the native dispatch
/// exists.
fn opencode_plugin_source(executable: &Path) -> anyhow::Result<String> {
    let exe = serde_json::to_string(&claude::utf8_path(
        "the devmap executable path",
        executable,
    )?)?;
    Ok(format!(
        r#"// Written by `devmap integrate opencode`. Edits here are overwritten.
//
// Refreshes the DevMap index after a tool writes, so the next question is
// answered against the current tree rather than the tree as it was at session
// start. `hook post-tool-use` returns immediately and detaches the rebuild.
import {{ spawnSync }} from "node:child_process";

const projectRoot = process.env.DEVMAP_PROJECT_ROOT || process.cwd();

export const DevMapOpenCodeHook = async () => ({{
  "{event}": async () => {{
    spawnSync({exe}, ["--root", projectRoot, "hook", "post-tool-use"], {{
      input: JSON.stringify({{ cwd: projectRoot }}),
      encoding: "utf-8",
    }});
  }},
}});
"#,
        event = OPENCODE_AFTER_TOOL,
        exe = exe,
    ))
}

/// Install the OpenCode plugin and list it in `opencode.json`.
///
/// Two files, and both must agree: the module on disk, and the `plugin` entry
/// that makes OpenCode load it. Writing one without the other yields either a
/// module nothing calls or a reference to a file that is not there.
pub fn merge_opencode_plugin(
    root: &Path,
    executable: &Path,
    dry_run: bool,
) -> anyhow::Result<Vec<McpMergeOutcome>> {
    let mut outcomes = Vec::new();
    let plugin_abs = devmap_extract::paths::state_dir(root)
        .join("integrations")
        .join(OPENCODE_PLUGIN_FILE);
    // Listed relative to the repository root: `opencode.json` is committed in
    // some projects, and an absolute path there names one developer's machine.
    let plugin_rel = plugin_abs
        .strip_prefix(root)
        .map(|rel| rel.to_path_buf())
        .unwrap_or_else(|_| plugin_abs.clone());
    let plugin_ref = claude::utf8_path("the opencode plugin path", &plugin_rel)?.replace('\\', "/");

    let source = opencode_plugin_source(executable)?;
    let current = fs::read_to_string(&plugin_abs).ok();
    let module_changed = current.as_deref() != Some(source.as_str());
    if module_changed && !dry_run {
        if let Some(parent) = plugin_abs.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&plugin_abs, &source)
            .with_context(|| format!("writing {}", plugin_abs.display()))?;
    }
    outcomes.push(McpMergeOutcome {
        path: plugin_abs,
        changed: module_changed,
        removed_stale_db: false,
        note: if module_changed {
            format!("wrote {plugin_ref}")
        } else {
            format!("{plugin_ref} already current")
        },
    });

    // Now the reference. `plugin` is a string in some configs and an array in
    // others; both are read, and an array is what gets written back.
    let config = root.join("opencode.json");
    let mut document = match read_host_config(&config) {
        Ok(text) if text.trim().is_empty() => empty_map(),
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|err| anyhow!("{}: not JSON ({err})", config.display()))?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => empty_map(),
        Err(err) => return Err(anyhow!("{}: {err}", config.display())),
    };
    let object = document
        .as_object_mut()
        .ok_or_else(|| anyhow!("{}: top level is not a JSON object", config.display()))?;

    let mut entries: Vec<Value> = match object.get("plugin") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(one)) => vec![json!(one)],
        Some(Value::Array(many)) => many.clone(),
        Some(other) => bail!(
            "{}: `plugin` is {}, not a string or array",
            config.display(),
            kind_of(other)
        ),
    };
    let already = entries.iter().any(|entry| {
        entry
            .as_str()
            .is_some_and(|text| is_our_plugin_ref(text, &plugin_ref))
    });
    let reference_changed = !already;
    if reference_changed {
        entries.push(json!(plugin_ref));
        object.insert("plugin".into(), Value::Array(entries));
        if !dry_run {
            write_json_pretty(&config, &document)?;
        }
    }
    outcomes.push(McpMergeOutcome {
        path: config,
        changed: reference_changed,
        removed_stale_db: false,
        note: if reference_changed {
            "opencode.json: listed the DevMap plugin".into()
        } else {
            "opencode.json: plugin already listed".into()
        },
    });
    Ok(outcomes)
}

/// Whether an existing `plugin` entry already names our module.
///
/// Accepts the spellings the host tolerates — a `file:` URI, a `./` prefix,
/// backslashes — so a second run appends nothing. Matching on the file name
/// alone would be wrong: another project's plugin may share it.
fn is_our_plugin_ref(entry: &str, ours: &str) -> bool {
    let normalize = |text: &str| {
        let text = text.replace('\\', "/");
        let text = text.strip_prefix("file://").unwrap_or(&text).to_string();
        let text = text.strip_prefix("./").unwrap_or(&text).to_string();
        text.trim_start_matches('/').to_string()
    };
    normalize(entry) == normalize(ours)
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

/// The most a host config this module inspects may weigh.
///
/// Matches the Go host's `maxHostConfigBytes`: these are settings files, and a
/// file that large is a wedged or hostile one, not a config to merge into.
const MAX_HOST_CONFIG_BYTES: u64 = 1 << 20;

/// Read one host config with a bound and a file-type check.
///
/// Every read of an existing config in this module goes through here. A plain
/// `read_to_string` had no ceiling and no notion of what it opened, so a
/// character device or an oversized file at a known config name was read until
/// it stopped or the process did.
fn read_host_config(path: &Path) -> io::Result<String> {
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{}: not a regular file", path.display()),
        ));
    }
    if meta.len() > MAX_HOST_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: {} bytes exceeds the {MAX_HOST_CONFIG_BYTES}-byte host config bound",
                path.display(),
                meta.len()
            ),
        ));
    }
    let mut text = String::new();
    // Bounded independently of the stat above: the file can grow between them.
    file.take(MAX_HOST_CONFIG_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_HOST_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: grew past the {MAX_HOST_CONFIG_BYTES}-byte host config bound",
                path.display()
            ),
        ));
    }
    Ok(text)
}

fn read_json_object(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({"mcpServers": {}}));
    }
    let text = read_host_config(path).with_context(|| format!("reading {}", path.display()))?;
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
        let existing_text = read_host_config(path)?;
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
        read_host_config(path).with_context(|| format!("reading {}", path.display()))?
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
        // The JSON configs next door publish through safe_fs; a plain
        // `fs::write` here truncated whatever the name resolved to, link or
        // not, and left a half-written config behind on a failed write.
        devmap_query::write_atomic(path, text.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
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
            let existing = read_host_config(path).unwrap_or_default();
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
    use clap::ValueEnum;
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

    /// The name clap accepts for a host and the name this code prints for it
    /// are the same string, for every host.
    ///
    /// Iterating `value_variants()` rather than a list written here is the
    /// point: clap derives that list from the enum, so a host added tomorrow is
    /// checked by this test without anyone remembering to add it. A variant
    /// whose clap spelling drifts from `as_str` — `OpenCode` renaming itself to
    /// `open-code`, say — would make `devmap integrate opencode` refuse a name
    /// the rest of this file, the Go integrator and the receipts all use.
    #[test]
    fn clap_and_as_str_spell_every_host_the_same_way() {
        for host in <Host as clap::ValueEnum>::value_variants() {
            let possible = host
                .to_possible_value()
                .expect("every host is selectable on the command line");
            assert_eq!(
                possible.get_name(),
                host.as_str(),
                "{host:?} is typed one way and printed another"
            );
            assert_eq!(
                Host::parse(host.as_str()).unwrap(),
                *host,
                "{host:?} does not survive a round trip through its own name"
            );
        }
    }

    /// The refusal names every host the parser actually accepts.
    ///
    /// This is the check the old hand-written message could not pass: it
    /// promised six names from a `match` that was free to accept a different
    /// set. Built from `value_variants()`, the two cannot disagree.
    #[test]
    fn an_unknown_host_is_refused_by_naming_the_real_ones() {
        let error = Host::parse("banana")
            .expect_err("banana is not a host")
            .to_string();
        assert!(error.contains("banana"), "{error}");
        for host in <Host as clap::ValueEnum>::value_variants() {
            assert!(
                error.contains(host.as_str()),
                "refusal does not name {}: {error}",
                host.as_str()
            );
        }
    }

    /// Case is not a spelling this accepts.
    ///
    /// `Host::parse` is the string door into the same values clap parses, and
    /// clap is case-sensitive here; a door that quietly took `CURSOR` would
    /// write receipts under a name the CLI would then refuse.
    #[test]
    fn a_host_name_is_matched_exactly() {
        for name in ["Cursor", "CURSOR", "open-code", " cursor", ""] {
            assert!(
                Host::parse(name).is_err(),
                "{name:?} was accepted as a host name"
            );
        }
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

    // ---- project server documents for antigravity / opencode / warp --------
    //
    // These three configurations existed on disk before this module wrote
    // them, left behind by the retired Python installer, and both of the
    // registrations it produced violated the contract this module enforces
    // everywhere else: a bare `devmap` (whichever build wins on PATH) and a
    // pinned `--db` (a store layout frozen on the day it was written).

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn devmap_entry(host: Host, root: &Path) -> Value {
        let doc = host.mcp_document().expect("host writes a document");
        let file = read(&root.join(doc.rel));
        match doc.container {
            None => file.get("devmap").cloned().unwrap(),
            Some(key) => file.get(key).unwrap().get("devmap").cloned().unwrap(),
        }
    }

    /// No `--db` anywhere, and the program is an absolute path.
    fn assert_contract(entry: &Value, exe: &Path) {
        let text = entry.to_string();
        assert!(!text.contains("--db"), "still pins a store: {text}");
        assert!(text.contains("--root"), "does not scope by root: {text}");
        let program = entry
            .get("command")
            .and_then(|c| match c {
                Value::String(s) => Some(s.clone()),
                Value::Array(a) => a.first().and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .expect("an entry names a program");
        assert_eq!(
            Path::new(&program),
            exe,
            "program is not the validated absolute path"
        );
    }

    // ---- OpenCode plugin -------------------------------------------------
    //
    // OpenCode has no hooks document: it loads an ES module listed in
    // `opencode.json`. Two artifacts must agree — the module on disk and the
    // reference that makes the host load it — so every test here checks both.

    fn plugin_list(root: &Path) -> Vec<String> {
        read(&root.join("opencode.json"))
            .get("plugin")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| e.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn plugin_module(root: &Path) -> String {
        fs::read_to_string(root.join(".devmap/integrations/opencode_devmap_plugin.mjs"))
            .or_else(|_| {
                fs::read_to_string(root.join(".devcouncil/integrations/opencode_devmap_plugin.mjs"))
            })
            .expect("the plugin module was written")
    }

    #[test]
    fn the_opencode_plugin_is_written_and_listed() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("oc-plugin");
        merge_opencode_plugin(&root, &exe, false).unwrap();

        let source = plugin_module(&root);
        // The native dispatch, not a legacy subcommand: a plugin calling
        // `devmap build` directly would bypass the coalescing and the budget
        // that make a post-write refresh safe to run on every tool call.
        assert!(source.contains("hook"), "{source}");
        assert!(source.contains("post-tool-use"), "{source}");
        assert!(
            !source.contains("\"build\""),
            "calls build directly: {source}"
        );
        assert!(source.contains("DevMapOpenCodeHook"), "{source}");
        assert!(source.contains("tool.execute.after"), "{source}");
        assert!(source.contains("/opt/devmap/bin/devmap"), "{source}");

        let listed = plugin_list(&root);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert!(
            listed[0].ends_with("opencode_devmap_plugin.mjs"),
            "{listed:?}"
        );
        // Relative to the repository: `opencode.json` is committed in some
        // projects, and an absolute path there names one developer's machine.
        assert!(
            !listed[0].starts_with('/'),
            "absolute path listed: {listed:?}"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_second_run_adds_no_duplicate_plugin_entry() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("oc-idem");
        merge_opencode_plugin(&root, &exe, false).unwrap();
        let outcomes = merge_opencode_plugin(&root, &exe, false).unwrap();
        assert!(
            outcomes.iter().all(|o| !o.changed),
            "second run reported a change: {:?}",
            outcomes.iter().map(|o| &o.note).collect::<Vec<_>>()
        );
        assert_eq!(plugin_list(&root).len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn another_projects_plugins_are_kept_in_every_spelling() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        // `plugin` is a bare string in some configs and an array in others.
        for existing in [
            json!("./theirs.mjs"),
            json!(["./theirs.mjs", "file:///abs/other.mjs"]),
        ] {
            let root = scratch("oc-keep");
            fs::write(
                root.join("opencode.json"),
                serde_json::to_string(&json!({"plugin": existing, "theme": "dark"})).unwrap(),
            )
            .unwrap();
            merge_opencode_plugin(&root, &exe, false).unwrap();
            let listed = plugin_list(&root);
            assert!(
                listed.iter().any(|p| p.contains("theirs.mjs")),
                "dropped a user plugin: {listed:?}"
            );
            assert!(
                listed
                    .iter()
                    .any(|p| p.ends_with("opencode_devmap_plugin.mjs")),
                "{listed:?}"
            );
            assert_eq!(read(&root.join("opencode.json"))["theme"], json!("dark"));
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn our_entry_is_recognised_however_it_is_spelled() {
        // A config hand-edited to a `file:` URI or a `./` prefix still names
        // our module; appending a second reference would load it twice.
        let ours = ".devmap/integrations/opencode_devmap_plugin.mjs";
        for spelling in [
            ".devmap/integrations/opencode_devmap_plugin.mjs",
            "./.devmap/integrations/opencode_devmap_plugin.mjs",
            ".devmap\\integrations\\opencode_devmap_plugin.mjs",
        ] {
            assert!(is_our_plugin_ref(spelling, ours), "{spelling}");
        }
        assert!(!is_our_plugin_ref("other/opencode_devmap_plugin.mjs", ours));
        assert!(!is_our_plugin_ref("./theirs.mjs", ours));
    }

    #[test]
    fn a_wrongly_shaped_plugin_key_is_refused_not_replaced() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("oc-bad");
        let body = r#"{"plugin": {"not": "a list"}}"#;
        fs::write(root.join("opencode.json"), body).unwrap();
        assert!(merge_opencode_plugin(&root, &exe, false).is_err());
        assert_eq!(
            fs::read_to_string(root.join("opencode.json")).unwrap(),
            body
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_opencode_dry_run_writes_neither_artifact() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("oc-dry");
        let outcomes = merge_opencode_plugin(&root, &exe, true).unwrap();
        assert!(outcomes.iter().any(|o| o.changed), "dry run reports intent");
        assert!(!root.join("opencode.json").exists(), "wrote the config");
        assert!(
            !root
                .join(".devmap/integrations/opencode_devmap_plugin.mjs")
                .exists()
                && !root
                    .join(".devcouncil/integrations/opencode_devmap_plugin.mjs")
                    .exists(),
            "wrote the module"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_module_is_rewritten_when_the_binary_moves() {
        // The executable path is baked into the module. An install that moved
        // must refresh it, or the plugin spawns a binary that is gone.
        let root = scratch("oc-move");
        merge_opencode_plugin(&root, Path::new("/old/devmap"), false).unwrap();
        assert!(plugin_module(&root).contains("/old/devmap"));
        let outcomes = merge_opencode_plugin(&root, Path::new("/new/devmap"), false).unwrap();
        assert!(
            outcomes.iter().any(|o| o.changed),
            "module was not refreshed"
        );
        let source = plugin_module(&root);
        assert!(source.contains("/new/devmap"), "{source}");
        assert!(!source.contains("/old/devmap"), "{source}");
        // The reference does not change, so it must not be appended twice.
        assert_eq!(plugin_list(&root).len(), 1);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_pinned_db_registration_is_repaired_for_every_host() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        for (host, rel, stale) in [
            (
                Host::Antigravity,
                ".agents/mcp_config.json",
                json!({"mcpServers": {
                    "devcouncil": {"command": "devcouncil", "args": ["mcp-server"]},
                    "devmap": {"command": "devmap", "args": ["--db", "/old/devmap.sqlite", "mcp"]},
                }}),
            ),
            (
                Host::OpenCode,
                "opencode.json",
                json!({
                    "$schema": "https://opencode.ai/config.json",
                    "theme": "tokyonight",
                    "mcp": {
                        "devcouncil": {"type": "local", "command": ["devcouncil", "mcp-server"]},
                        "devmap": {"type": "local",
                                   "command": ["devmap", "--db", "/old/devmap.sqlite", "mcp"]},
                    },
                }),
            ),
            (
                Host::Warp,
                ".devcouncil/integrations/warp-mcp.json",
                json!({"devcouncil": {"command": "devcouncil", "args": ["mcp-server"]}}),
            ),
        ] {
            let root = scratch(host.as_str());
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, serde_json::to_string_pretty(&stale).unwrap()).unwrap();

            let outcome = merge_host_mcp(host, &root, &exe, false)
                .unwrap()
                .expect("this host writes a document");
            assert!(outcome.changed, "{host:?} reported no change");
            assert_contract(&devmap_entry(host, &root), &exe);

            // The other server in the file is not ours to touch.
            let after = read(&path);
            let servers = match host.mcp_document().unwrap().container {
                None => after.clone(),
                Some(key) => after.get(key).cloned().unwrap(),
            };
            assert!(
                servers.get("devcouncil").is_some(),
                "{host:?} dropped the devcouncil server: {after}"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn only_the_two_hosts_that_pinned_a_store_report_that_repair() {
        // Warp carried no devmap entry at all, so registering one is a first
        // write, not a repair. A receipt that called both the same could not
        // show that the stale registrations were actually replaced.
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("warp-fresh");
        let outcome = merge_host_mcp(Host::Warp, &root, &exe, false)
            .unwrap()
            .unwrap();
        assert!(outcome.changed);
        assert!(!outcome.removed_stale_db, "{}", outcome.note);
        assert!(
            outcome.note.contains("registered devmap"),
            "{}",
            outcome.note
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_opencode_file_keeps_every_key_that_is_not_ours() {
        // `opencode.json` is a user's configuration that happens to hold a
        // server list. Losing an unrelated key is a bug even when the server
        // entry comes out right.
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("opencode-keys");
        fs::write(
            root.join("opencode.json"),
            serde_json::to_string_pretty(&json!({
                "$schema": "https://opencode.ai/config.json",
                "theme": "tokyonight",
                "model": "anthropic/claude-opus-5",
                "keybinds": {"leader": "ctrl+x"},
            }))
            .unwrap(),
        )
        .unwrap();
        merge_host_mcp(Host::OpenCode, &root, &exe, false).unwrap();
        let after = read(&root.join("opencode.json"));
        assert_eq!(after["theme"], json!("tokyonight"));
        assert_eq!(after["model"], json!("anthropic/claude-opus-5"));
        assert_eq!(after["keybinds"]["leader"], json!("ctrl+x"));
        assert_eq!(after["$schema"], json!("https://opencode.ai/config.json"));
        // argv form, not command+args.
        assert!(after["mcp"]["devmap"]["command"].is_array());
        assert_eq!(after["mcp"]["devmap"]["type"], json!("local"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        for host in [Host::Antigravity, Host::OpenCode, Host::Warp] {
            let root = scratch(&format!("idem-{}", host.as_str()));
            merge_host_mcp(host, &root, &exe, false).unwrap();
            let first = read(&root.join(host.mcp_document().unwrap().rel));
            let again = merge_host_mcp(host, &root, &exe, false).unwrap().unwrap();
            assert!(!again.changed, "{host:?} rewrote an identical file");
            assert!(again.note.contains("already current"), "{}", again.note);
            assert_eq!(
                first,
                read(&root.join(host.mcp_document().unwrap().rel)),
                "{host:?} changed bytes while reporting no change"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("dry");
        let outcome = merge_host_mcp(Host::Warp, &root, &exe, true)
            .unwrap()
            .unwrap();
        assert!(outcome.changed, "a dry run still reports what it would do");
        assert!(
            !root.join(".devcouncil/integrations/warp-mcp.json").exists(),
            "dry run created the file"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unreadable_or_wrongly_shaped_config_is_refused_not_overwritten() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        for (label, body) in [
            ("not-json", "{ this is not json"),
            ("top-level-array", "[1, 2, 3]"),
            ("top-level-string", "\"hello\""),
            ("container-is-a-string", "{\"mcp\": \"not an object\"}"),
        ] {
            let root = scratch(label);
            let path = root.join("opencode.json");
            fs::write(&path, body).unwrap();
            let result = merge_host_mcp(Host::OpenCode, &root, &exe, false);
            assert!(result.is_err(), "{label} was accepted");
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                body,
                "{label} was overwritten despite being refused"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn an_empty_file_is_treated_as_an_empty_document() {
        // A zero-byte config is what an interrupted write leaves behind. It
        // carries no user keys to lose, so it is filled in rather than refused.
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("empty");
        fs::write(root.join("opencode.json"), "   \n").unwrap();
        merge_host_mcp(Host::OpenCode, &root, &exe, false).unwrap();
        let after = read(&root.join("opencode.json"));
        assert_eq!(after["$schema"], json!("https://opencode.ai/config.json"));
        assert!(after["mcp"]["devmap"]["command"].is_array());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_hosts_without_their_own_document_are_left_to_the_existing_merge() {
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let root = scratch("nodoc");
        for host in [Host::Cursor, Host::Claude, Host::Codex] {
            assert!(
                merge_host_mcp(host, &root, &exe, false).unwrap().is_none(),
                "{host:?} must keep going through offer_project_mcp"
            );
        }
        fs::remove_dir_all(&root).ok();
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
