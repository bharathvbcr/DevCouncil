//! Host integration: global DevMap MCP registration and project assets.
//!
//! `devmap integrate <host>` (the hosts are [`Host`]'s variants):
//! - writes/refreshes marker-guarded guides and `.cursor/rules/devmap.mdc`
//! - installs the five embedded DevMap skills into the host's skill layout
//! - registers `devmap mcp` (no `--db`) in the global Cursor / Claude configs,
//!   except that Claude's user-scope entry is withheld — and an owned unpinned
//!   one removed — while the enabled Dev Map plugin registers the same server
//! - rewrites per-project owned entries to `--root <abs>` (belt-and-braces;
//!   insufficient for multi-tab Cursor — callers must still pass `repo_path`),
//!   except that Claude's project `.mcp.json` gets the same withholding as its
//!   user-scope entry while the plugin is enabled
//! - with `--servers-stdin`, registers a caller's servers (DevCouncil's
//!   `devcouncil`) in each host's project document via [`register_server`]

use std::collections::BTreeMap;
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
    /// Tied to clap's own accepted values by a test below. DevCouncil's Go host
    /// keeps no copy of this list: it passes host names through to `devmap
    /// integrate`, whose refusal of an unknown one prints clap's list.
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
    /// The caller's servers from `--servers-stdin`, one outcome each.
    pub servers: Vec<McpMergeOutcome>,
    pub notes: Vec<String>,
    /// Under `--check`, whether every asset already matched. The report is
    /// returned either way, so a caller can see *which* file differs.
    pub check_ok: bool,
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
    servers: &[ServerSpec],
    dry_run: bool,
    check: bool,
) -> anyhow::Result<IntegrateReport> {
    let root = project_root
        .canonicalize()
        .with_context(|| format!("project root {}", project_root.display()))?;

    // Every caller server is planned before DevMap writes anything: a
    // document that refuses the caller's entry must not leave DevMap's half
    // applied and the caller's missing.
    for spec in servers {
        register_server(host, &root, spec, true)?;
    }

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

    // An enabled Dev Map plugin already registers `devmap` for Claude Code. A
    // user- or project-scope entry beside it is a second server for the same
    // host: Claude Code collapses same-named servers only across its own
    // scopes, and drops a plugin server only when another has the same
    // command, which the absolute path these entries carry never matches.
    // Writing either one is what kept bringing the duplicate back.
    let plugin_registration = match host {
        Host::Claude => {
            let install = claude::claude_plugin_install(&dirs_home()?);
            let registration = claude_plugin_registration(&install);
            if registration.is_none() {
                // Unknown is not "no plugin": say the writes rest on it.
                for error in install.record_error.iter().chain(&install.load_error) {
                    report.notes.push(format!(
                        "claude: could not tell whether the Dev Map plugin registers devmap \
                         ({error}); the user- and project-scope entries were managed as if it \
                         does not"
                    ));
                }
            }
            registration
        }
        _ => None,
    };

    if host.registers_global_mcp() {
        for path in global_mcp_paths(host)? {
            let outcome = match &plugin_registration {
                Some(plugin_mcp) => {
                    retire_beside_plugin(&path, plugin_mcp, "user-scope", dry_run || check)?
                }
                None => merge_global_mcp(&path, executable, dry_run || check)?,
            };
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
    // has no DevMap entry. `--root` is insufficient for multi-tab Cursor. A
    // document the host loads is instead kept clear of `devmap` while the
    // enabled plugin registers it.
    for site in project_mcp_sites(host, &root) {
        if let (true, Some(plugin_mcp)) = (site.loaded, &plugin_registration) {
            report.project_mcp.push(retire_beside_plugin(
                &site.path,
                plugin_mcp,
                "project-scope",
                dry_run || check,
            )?);
            continue;
        }
        if let Some(outcome) = clean_project_mcp(&site.path, executable, dry_run || check)? {
            report.project_mcp.push(outcome);
            continue;
        }
        if site.loaded {
            if let Some(outcome) = offer_project_mcp(&site.path, executable, dry_run || check)? {
                report.project_mcp.push(outcome);
            }
        }
    }

    // The caller's servers go into the same project document, through the
    // same per-host table, as DevMap's own entry.
    for spec in servers {
        report
            .servers
            .push(register_server(host, &root, spec, dry_run || check)?);
    }

    let guides_dirty = report.guides.iter().any(|g| g.changed());
    let skills_dirty = !report.skills_differing.is_empty();
    let mcp_dirty = report
        .global_mcp
        .iter()
        .chain(report.project_mcp.iter())
        .chain(report.hooks.iter())
        .chain(report.servers.iter())
        .any(|m| m.changed);
    report.check_ok = !(guides_dirty || skills_dirty || mcp_dirty || !skills_check_ok);

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

/// One per-project `mcpServers` document the merge inspects.
struct ProjectMcpSite {
    path: PathBuf,
    /// Whether the host loads this document. Only a loaded one is offered a
    /// new entry, and only a loaded one duplicates an enabled plugin.
    loaded: bool,
}

fn project_mcp_sites(host: Host, root: &Path) -> Vec<ProjectMcpSite> {
    let site = |path: PathBuf, loaded: bool| ProjectMcpSite { path, loaded };
    match host {
        Host::Cursor => vec![site(root.join(".cursor").join("mcp.json"), true)],
        // Claude Code documents `<root>/.mcp.json` as its only project-scope
        // file. `<root>/.claude/mcp.json` is named nowhere, so creating one
        // left a file no host reads; an entry already there is still cleaned
        // of a stale `--db`, but nothing new is written to it.
        Host::Claude => vec![
            site(root.join(".mcp.json"), true),
            site(root.join(".claude").join("mcp.json"), false),
        ],
        // The three below are written by `host_mcp_document`, which knows each
        // one's container key and entry shape. They are deliberately absent
        // here: this list feeds the `mcpServers` merge, and two of them do not
        // use that key at all.
        Host::Codex | Host::Antigravity | Host::OpenCode | Host::Warp => Vec::new(),
    }
}

/// How a host's project document is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocFormat {
    Json,
    /// Codex. Written by [`merge_toml_server`], which keeps every byte
    /// outside the one table it owns.
    Toml,
}

/// Where one host keeps its project server list, and how that document is
/// shaped.
///
/// This is the one table of per-host documents. DevCouncil's Go host used to
/// carry a copy for the server it registers, pinned to this one by a test that
/// parsed this file; it now hands its server to `devmap integrate
/// --servers-stdin` instead, so both servers are written from here.
///
/// The shapes are load-bearing: Antigravity nests servers under `mcpServers`,
/// OpenCode under `mcp` with argv-array entries, Warp's file *is* the server
/// map, and Codex is TOML under `mcp_servers`.
struct HostMcpDoc {
    /// Path relative to the repository root.
    rel: &'static str,
    format: DocFormat,
    /// Key the server map lives under, or `None` when the document root is it.
    container: Option<&'static str>,
    /// Extra top-level keys to establish when creating the file.
    preamble: &'static [(&'static str, &'static str)],
    /// Whether entries name the program as an argv array (`command: [...]`)
    /// instead of `command` + `args`.
    argv_form: bool,
    /// Whether the host honours a `cwd` key on an entry. Antigravity resolves
    /// relative paths against its own working directory, and Codex documents
    /// `cwd` for stdio servers; the others name no such key.
    accepts_cwd: bool,
    /// Whether [`merge_host_mcp`] writes DevMap's own entry here. Cursor and
    /// Claude carry it through [`offer_project_mcp`], which also withholds it
    /// beside the enabled plugin, and Codex in the user-level config.
    carries_devmap: bool,
}

impl Host {
    /// The project-scoped server document this host reads.
    fn project_document(self) -> HostMcpDoc {
        let json = |rel, container| HostMcpDoc {
            rel,
            format: DocFormat::Json,
            container,
            preamble: &[],
            argv_form: false,
            accepts_cwd: false,
            carries_devmap: false,
        };
        match self {
            Self::Cursor => json(".cursor/mcp.json", Some("mcpServers")),
            Self::Claude => json(".mcp.json", Some("mcpServers")),
            // Read by Codex for trusted projects only; the user-level
            // `~/.codex/config.toml` is where DevMap's own entry goes.
            Self::Codex => HostMcpDoc {
                format: DocFormat::Toml,
                accepts_cwd: true,
                ..json(".codex/config.toml", Some("mcp_servers"))
            },
            Self::Antigravity => HostMcpDoc {
                accepts_cwd: true,
                carries_devmap: true,
                ..json(".agents/mcp_config.json", Some("mcpServers"))
            },
            Self::OpenCode => HostMcpDoc {
                // OpenCode validates against this schema; a file created
                // without it loses editor completion for every other key.
                preamble: &[("$schema", "https://opencode.ai/config.json")],
                argv_form: true,
                carries_devmap: true,
                ..json("opencode.json", Some("mcp"))
            },
            Self::Warp => HostMcpDoc {
                carries_devmap: true,
                ..json(".devcouncil/integrations/warp-mcp.json", None)
            },
        }
    }

    /// The project document that carries DevMap's own entry, if this host has
    /// one written by [`merge_host_mcp`].
    fn mcp_document(self) -> Option<HostMcpDoc> {
        Some(self.project_document()).filter(|doc| doc.carries_devmap)
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
    let entry = host_mcp_entry(&doc, executable, root)?;
    merge_json_server(&root.join(doc.rel), &doc, MCP_SERVER_NAME, entry, dry_run).map(Some)
}

/// Fold one named server entry into a host's JSON project document.
fn merge_json_server(
    path: &Path,
    doc: &HostMcpDoc,
    name: &str,
    entry: Value,
    dry_run: bool,
) -> anyhow::Result<McpMergeOutcome> {
    let path = path.to_path_buf();
    let mut document = match read_project_doc(&path)? {
        None => empty_map(),
        Some(text) if text.trim().is_empty() => empty_map(),
        Some(text) => serde_json::from_str::<Value>(&text).map_err(|err| {
            // Refuse rather than overwrite: an unparseable config is far more
            // likely to be a file worth keeping than one worth replacing.
            // serde_json reads no comments, so a commented file lands here
            // too rather than losing them in the rewrite.
            anyhow!(
                "{}: not JSON ({err}); refuse to overwrite a host config this \
                 module cannot read",
                path.display()
            )
        })?,
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

    let before = server_of(&document, doc, name).cloned();
    if before.as_ref() == Some(&entry) {
        return Ok(McpMergeOutcome {
            path,
            changed: false,
            removed_stale_db: false,
            note: format!("{} already current", doc.rel),
        });
    }
    // Reported rather than folded into "changed": replacing a pinned store
    // path is the repair this exists for, and a receipt that does not
    // distinguish it from a first-time write cannot show the repair happened.
    let removed_stale_db =
        name == MCP_SERVER_NAME && before.as_ref().is_some_and(entry_names_a_store);

    servers_mut(&mut document, doc)?.insert(name.to_string(), entry);

    if !dry_run {
        // No `create_dir_all` first: it would follow a linked parent out of
        // the repository. `write_atomic` creates missing directories through
        // pinned handles and refuses a link on the way.
        write_json_pretty(&path, &document)?;
    }
    Ok(McpMergeOutcome {
        path,
        changed: true,
        removed_stale_db,
        note: if removed_stale_db {
            format!("{}: replaced a pinned --db entry with --root", doc.rel)
        } else {
            format!("{}: registered {name}", doc.rel)
        },
    })
}

/// Read a project document without following a link at any component.
///
/// These files arrive with a clone, and so can a symlink in their place: a
/// read that followed one would copy an outside file's contents into the merged
/// result and then into the repository.
fn read_project_doc(path: &Path) -> anyhow::Result<Option<String>> {
    use devmap_extract::safe_fs::{Access, Creation, SafeFile};
    match SafeFile::open(path, Access::Read, Creation::Never) {
        Ok(mut file) => file
            .read_text(MAX_HOST_CONFIG_BYTES)
            .map(Some)
            .with_context(|| format!("reading {}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(anyhow!("{}: {err}", path.display())),
    }
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
    let args = standard
        .get("args")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned();
    Ok(argv_entry(command, args, None))
}

/// OpenCode's entry: one argv array, a declared local type, and its own name
/// for the environment block.
fn argv_entry(
    command: &str,
    args: impl IntoIterator<Item = Value>,
    environment: Option<&BTreeMap<String, String>>,
) -> Value {
    let mut argv = vec![json!(command)];
    argv.extend(args);
    let mut entry = json!({
        "type": "local",
        "command": Value::Array(argv),
        "enabled": true,
        "timeout": 10_000,
    });
    if let Some(environment) = environment.filter(|env| !env.is_empty()) {
        entry["environment"] = json!(environment);
    }
    entry
}

/// The most servers one `--servers-stdin` request may register.
const MAX_REQUESTED_SERVERS: usize = 8;
/// Bound on the request document itself.
pub const MAX_SERVER_REQUEST_BYTES: usize = 64 * 1024;
const MAX_SERVER_ARGS: usize = 64;
const MAX_SERVER_ENV: usize = 64;
const MAX_SERVER_STRING: usize = 4096;

/// A server another tool asks `devmap integrate` to register beside DevMap's
/// own — DevCouncil's `devcouncil` server is the caller this exists for.
///
/// DevMap owns every host's document shape; the caller owns only what its
/// server runs. That split is what lets one table describe each host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSpec {
    pub name: String,
    /// Absolute path of the program. A bare name would hand the host whichever
    /// build wins on PATH, which is the contract DevMap's own entry refuses.
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Working directory, written only where the host honours one.
    pub cwd: Option<String>,
}

/// Parse and validate `{"servers": [{name, command, args?, env?, cwd?}]}`.
///
/// Unknown keys are refused rather than ignored: a field the caller believes
/// was written and this reader dropped is a silent misconfiguration.
pub fn parse_server_request(bytes: &[u8]) -> anyhow::Result<Vec<ServerSpec>> {
    if bytes.len() > MAX_SERVER_REQUEST_BYTES {
        bail!("server request exceeds {MAX_SERVER_REQUEST_BYTES} bytes");
    }
    let request: Value = serde_json::from_slice(bytes)
        .map_err(|err| anyhow!("server request is not JSON: {err}"))?;
    let object = request
        .as_object()
        .ok_or_else(|| anyhow!("server request must be a JSON object"))?;
    refuse_unknown_keys("server request", object, &["servers"])?;
    let servers = object
        .get("servers")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("server request needs a `servers` array"))?;
    if servers.is_empty() || servers.len() > MAX_REQUESTED_SERVERS {
        bail!("server request must name 1–{MAX_REQUESTED_SERVERS} servers");
    }
    let mut specs: Vec<ServerSpec> = Vec::with_capacity(servers.len());
    for server in servers {
        let spec = parse_server_spec(server)?;
        if specs.iter().any(|seen| seen.name == spec.name) {
            bail!("server {:?} is named twice", spec.name);
        }
        specs.push(spec);
    }
    Ok(specs)
}

fn parse_server_spec(value: &Value) -> anyhow::Result<ServerSpec> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("each server must be a JSON object"))?;
    refuse_unknown_keys("server", object, &["name", "command", "args", "env", "cwd"])?;
    let string = |key: &str| -> anyhow::Result<Option<String>> {
        match object.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(checked_string(key, text)?)),
            Some(other) => bail!("server `{key}` is {}, not a string", kind_of(other)),
        }
    };
    let name = string("name")?.ok_or_else(|| anyhow!("server needs a `name`"))?;
    let valid_name = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && name.as_bytes()[0].is_ascii_alphanumeric();
    if !valid_name {
        bail!("server name {name:?} must be 1–64 of [a-z0-9_-], starting alphanumeric");
    }
    if name == MCP_SERVER_NAME {
        bail!("server name {name:?} is DevMap's own entry, which `devmap integrate` writes itself");
    }
    let command = string("command")?.ok_or_else(|| anyhow!("server {name:?} needs a `command`"))?;
    if !Path::new(&command).is_absolute() {
        bail!("server {name:?}: command {command:?} must be an absolute path");
    }
    let cwd = string("cwd")?;
    if let Some(cwd) = &cwd {
        if !Path::new(cwd).is_absolute() {
            bail!("server {name:?}: cwd {cwd:?} must be an absolute path");
        }
    }
    let args = match object.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => {
            if items.len() > MAX_SERVER_ARGS {
                bail!("server {name:?}: more than {MAX_SERVER_ARGS} args");
            }
            items
                .iter()
                .map(|item| match item {
                    Value::String(text) => checked_string("args", text),
                    other => bail!(
                        "server {name:?}: an arg is {}, not a string",
                        kind_of(other)
                    ),
                })
                .collect::<anyhow::Result<_>>()?
        }
        Some(other) => bail!(
            "server {name:?}: `args` is {}, not an array",
            kind_of(other)
        ),
    };
    let env = match object.get("env") {
        None => BTreeMap::new(),
        Some(Value::Object(vars)) => {
            if vars.len() > MAX_SERVER_ENV {
                bail!("server {name:?}: more than {MAX_SERVER_ENV} env entries");
            }
            let mut env = BTreeMap::new();
            for (key, value) in vars {
                let valid_key = key.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                }) && !key.is_empty();
                if !valid_key {
                    bail!("server {name:?}: env name {key:?} is not an identifier");
                }
                let Value::String(text) = value else {
                    bail!(
                        "server {name:?}: env {key} is {}, not a string",
                        kind_of(value)
                    );
                };
                env.insert(key.clone(), checked_string("env", text)?);
            }
            env
        }
        Some(other) => bail!(
            "server {name:?}: `env` is {}, not an object",
            kind_of(other)
        ),
    };
    Ok(ServerSpec {
        name,
        command,
        args,
        env,
        cwd,
    })
}

fn checked_string(key: &str, text: &str) -> anyhow::Result<String> {
    if text.len() > MAX_SERVER_STRING || text.contains('\0') {
        bail!("server `{key}` value is over {MAX_SERVER_STRING} bytes or contains NUL");
    }
    Ok(text.to_string())
}

fn refuse_unknown_keys(
    what: &str,
    object: &Map<String, Value>,
    known: &[&str],
) -> anyhow::Result<()> {
    match object.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => bail!("{what}: unknown key {key:?}"),
        None => Ok(()),
    }
}

/// Register a caller's server in `host`'s project document.
pub fn register_server(
    host: Host,
    root: &Path,
    spec: &ServerSpec,
    dry_run: bool,
) -> anyhow::Result<McpMergeOutcome> {
    let doc = host.project_document();
    let path = root.join(doc.rel);
    let cwd = spec.cwd.as_ref().filter(|_| doc.accepts_cwd);
    match doc.format {
        DocFormat::Json => {
            let entry = if doc.argv_form {
                argv_entry(
                    &spec.command,
                    spec.args.iter().map(|a| json!(a)),
                    Some(&spec.env),
                )
            } else {
                let mut entry = json!({"command": spec.command, "args": spec.args});
                if !spec.env.is_empty() {
                    entry["env"] = json!(spec.env);
                }
                if let Some(cwd) = cwd {
                    entry["cwd"] = json!(cwd);
                }
                entry
            };
            merge_json_server(&path, &doc, &spec.name, entry, dry_run)
        }
        DocFormat::Toml => {
            let mut entry = toml::Table::new();
            entry.insert("command".into(), toml::Value::String(spec.command.clone()));
            entry.insert(
                "args".into(),
                toml::Value::Array(spec.args.iter().cloned().map(toml::Value::String).collect()),
            );
            if !spec.env.is_empty() {
                let env: toml::Table = spec
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), toml::Value::String(v.clone())))
                    .collect();
                entry.insert("env".into(), toml::Value::Table(env));
            }
            if let Some(cwd) = cwd {
                entry.insert("cwd".into(), toml::Value::String(cwd.clone()));
            }
            merge_toml_server(&path, &doc, &spec.name, entry, dry_run)
        }
    }
}

/// Fold one `[<container>.<name>]` table into a TOML document, keeping every
/// byte outside that table.
///
/// The document is the user's: `.codex/config.toml` holds their model, their
/// profiles and their other servers. Re-serialising it would drop comments
/// and reorder what remains, so the table is appended — or, when present,
/// replaced in place along with its own subtables — and the result is parsed
/// back and compared with the original plus our entry. Any difference beyond
/// our table, or a document whose layout keeps our table from being found as
/// one block (an inline `mcp_servers = {…}`, dotted keys), is a refusal.
fn merge_toml_server(
    path: &Path,
    doc: &HostMcpDoc,
    name: &str,
    entry: toml::Table,
    dry_run: bool,
) -> anyhow::Result<McpMergeOutcome> {
    let container = doc
        .container
        .ok_or_else(|| anyhow!("{}: a TOML document names its server table", doc.rel))?;
    let text = read_project_doc(path)?.unwrap_or_default();
    let parsed: toml::Table = text
        .parse()
        .map_err(|err| anyhow!("{}: not TOML ({err}); refuse to rewrite it", path.display()))?;
    let current = match parsed.get(container) {
        None => None,
        Some(toml::Value::Table(servers)) => servers.get(name),
        Some(_) => bail!("{}: `{container}` is not a table", path.display()),
    };
    let wanted = toml::Value::Table(entry.clone());
    if current == Some(&wanted) {
        return Ok(McpMergeOutcome {
            path: path.to_path_buf(),
            changed: false,
            removed_stale_db: false,
            note: format!("{} already current", doc.rel),
        });
    }

    let mut wrapper = toml::Table::new();
    let mut servers = toml::Table::new();
    servers.insert(name.to_string(), wanted.clone());
    wrapper.insert(container.to_string(), toml::Value::Table(servers));
    let block = toml::to_string(&wrapper).map_err(|err| anyhow!("serialize {name}: {err}"))?;

    let updated = if current.is_some() {
        replace_toml_table(&text, container, name, &block).ok_or_else(|| {
            anyhow!(
                "{}: [{container}.{name}] is not one block this command can replace \
                 (inline table or dotted keys); edit it by hand",
                path.display()
            )
        })?
    } else {
        let mut out = text.clone();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.trim().is_empty() {
            out.push('\n');
        }
        out.push_str(&block);
        out
    };

    let reparsed: toml::Table = updated.parse().map_err(|err| {
        anyhow!(
            "{}: adding [{container}.{name}] would not parse ({err}); refuse to rewrite it",
            path.display()
        )
    })?;
    let mut expected = parsed.clone();
    if let toml::Value::Table(servers) = expected
        .entry(container.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
    {
        servers.insert(name.to_string(), wanted);
    }
    if reparsed != expected {
        bail!(
            "{}: adding [{container}.{name}] would change other settings; refuse to rewrite it",
            path.display()
        );
    }

    if !dry_run {
        devmap_query::write_atomic(path, updated.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(McpMergeOutcome {
        path: path.to_path_buf(),
        changed: true,
        removed_stale_db: false,
        note: format!("{}: registered {name}", doc.rel),
    })
}

/// Replace the `[<container>.<name>]` block and its subtables with `block`.
///
/// `None` when the header is not found exactly once. The caller re-parses the
/// result, so a layout this misreads is refused rather than written.
fn replace_toml_table(text: &str, container: &str, name: &str, block: &str) -> Option<String> {
    let own = format!("{container}.{name}");
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let header_of = |line: &str| -> Option<(bool, String)> {
        let trimmed = line.trim();
        let (array, inner) = if let Some(rest) = trimmed.strip_prefix("[[") {
            (true, rest.split_once("]]")?.0)
        } else {
            (false, trimmed.strip_prefix('[')?.split_once(']')?.0)
        };
        let key = inner
            .split('.')
            .map(|part| part.trim().trim_matches(|c| c == '"' || c == '\''))
            .collect::<Vec<_>>()
            .join(".");
        Some((array, key))
    };
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| matches!(header_of(line), Some((false, ref key)) if *key == own))
        .map(|(index, _)| index)
        .collect();
    let [start] = starts.as_slice() else {
        return None;
    };
    let nested = format!("{own}.");
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(
            |(_, line)| matches!(header_of(line), Some((_, ref key)) if !key.starts_with(&nested)),
        )
        .map_or(lines.len(), |(index, _)| index);
    let mut out: String = lines[..*start].concat();
    out.push_str(block);
    if end < lines.len() {
        out.push('\n');
    }
    out.push_str(&lines[end..].concat());
    Some(out)
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

fn server_of<'a>(document: &'a Value, doc: &HostMcpDoc, name: &str) -> Option<&'a Value> {
    match doc.container {
        None => document.get(name),
        Some(key) => document.get(key).and_then(|map| map.get(name)),
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

/// The `.mcp.json` of the Dev Map plugin Claude Code loads, when the plugin is
/// enabled and that file registers `devmap`.
///
/// Read through the same resolution `devmap doctor` uses, so integrate and the
/// duplicate check agree on which copy is loaded. A plugin that is installed
/// but whose loaded copy registers no `devmap` server leaves the user-scope
/// entry as the only registration, and it is still written.
fn claude_plugin_registration(install: &claude::ClaudePluginInstall) -> Option<PathBuf> {
    if install.enabled != Some(true) {
        return None;
    }
    install
        .loaded
        .iter()
        .map(|plugin| plugin.dir.join(".mcp.json"))
        .find(|mcp| {
            read_host_config(mcp)
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|document| {
                    document
                        .get("mcpServers")
                        .and_then(|servers| servers.get(MCP_SERVER_NAME))
                        .is_some()
                })
        })
}

/// Keep a Claude Code config at `path` — user-scope `~/.claude.json` or a
/// project `.mcp.json`, named by `scope` in the notes — from registering
/// `devmap` a second time beside the enabled plugin's registration at
/// `plugin_mcp`.
///
/// Removes only an entry this installer writes in its unpinned form. A pinned
/// (`--root`/`--db`) entry or one this installer did not write is a choice
/// someone made; it is reported and left, and the note says Claude Code still
/// loads it. Every other key in the file survives.
pub fn retire_beside_plugin(
    path: &Path,
    plugin_mcp: &Path,
    scope: &str,
    read_only: bool,
) -> anyhow::Result<McpMergeOutcome> {
    let outcome = |changed: bool, note: String| McpMergeOutcome {
        path: path.to_path_buf(),
        changed,
        removed_stale_db: false,
        note,
    };
    let plugin = plugin_mcp.display();
    let skipped = || {
        outcome(
            false,
            format!(
                "not registered here: the enabled Dev Map plugin registers devmap for Claude \
                 Code at {plugin}, and a {scope} entry would load a second server beside it"
            ),
        )
    };
    if !path.exists() {
        return Ok(skipped());
    }
    let mut document = read_json_object(path)?;
    let Some(existing) = document
        .get("mcpServers")
        .and_then(|servers| servers.get(MCP_SERVER_NAME))
    else {
        return Ok(skipped());
    };
    if !is_owned_devmap_entry(existing) {
        return Ok(outcome(
            false,
            format!(
                "devmap entry present but not owned by this installer; left unchanged, though \
                 the enabled Dev Map plugin also registers devmap at {plugin}"
            ),
        ));
    }
    if entry_is_pinned(existing) {
        return Ok(outcome(
            false,
            format!(
                "pinned (--root/--db) devmap entry left unchanged, but Claude Code can load it as a \
                 second server beside the enabled Dev Map plugin's at {plugin}; remove one"
            ),
        ));
    }
    if !read_only {
        mcp_servers_mut(&mut document)?.remove(MCP_SERVER_NAME);
        write_json_pretty(path, &document)?;
    }
    Ok(outcome(
        true,
        format!(
            "{} the {scope} devmap entry: the enabled Dev Map plugin registers devmap at \
             {plugin}, and Claude Code would load both",
            if read_only { "would remove" } else { "removed" }
        ),
    ))
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

/// Whether a `devmap` server entry is pinned to one repository or store by
/// `--root` or `--db`. Doctor counts pinned entries apart from the unpinned
/// server, which is the one a second registration duplicates.
pub fn entry_is_pinned(entry: &Value) -> bool {
    entry
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .any(|a| a == "--root" || a == "--db")
        })
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
pub(crate) fn read_host_config(path: &Path) -> io::Result<String> {
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
        note: "wrote marker-owned .cursor/hooks.json (sessionStart, postToolUse, afterFileEdit, sessionEnd)"
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

    /// Cursor `preToolUse` / `beforeReadFile` are permission hooks: a schema
    /// mismatch blocks the tool. The first-nav nudge therefore lives on
    /// `postToolUse`, whose documented output is `additional_context`.
    #[test]
    fn cursor_hooks_inject_first_navigation_on_post_tool_use_never_on_permission_events() {
        let dir = scratch("cursor-nav-hooks");
        let path = dir.join(".cursor").join("hooks.json");
        merge_cursor_hooks(&path, Path::new("/tmp/fake-devmap"), false).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        for forbidden in [
            "preToolUse",
            "beforeReadFile",
            "beforeShellExecution",
            "beforeMCPExecution",
            "beforeTabFileRead",
            "subagentStart",
        ] {
            assert!(
                value["hooks"].get(forbidden).is_none(),
                "{forbidden} is a permission hook; installing there can block the agent"
            );
        }
        let nav = &value["hooks"]["postToolUse"][0];
        let cmd = nav["command"].as_str().unwrap();
        assert!(
            cmd.contains("hook pre-tool-use"),
            "first-nav must run the existing pre-tool-use handler: {cmd}"
        );
        assert_eq!(nav["matcher"], "Read|Grep");
        assert!(
            value.get("failClosed").is_none() && nav.get("failClosed").is_none(),
            "failClosed would turn a crash into a blocked Read: {value}"
        );
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

    // --- Servers a caller registers (`--servers-stdin`) -------------------
    //
    // These carry the contracts DevCouncil's Go integrator held for its own
    // `devcouncil` entry before that entry moved here: the neighbours in a
    // host's document survive, each host gets its own spelling, a file this
    // module cannot read is kept, and a link is never followed.

    fn every_host() -> Vec<Host> {
        <Host as clap::ValueEnum>::value_variants().to_vec()
    }

    fn devcouncil_spec(root: &Path) -> ServerSpec {
        let root = root.to_str().unwrap().to_string();
        ServerSpec {
            name: "devcouncil".into(),
            command: "/opt/devcouncil/bin/devcouncil".into(),
            args: vec!["mcp".into()],
            env: BTreeMap::from([("DEVCOUNCIL_PROJECT_ROOT".into(), root.clone())]),
            cwd: Some(root),
        }
    }

    /// The registered entry, read back through the host's own document shape.
    fn registered(host: Host, root: &Path, name: &str) -> Option<Value> {
        let doc = host.project_document();
        let text = fs::read_to_string(root.join(doc.rel)).ok()?;
        let document: Value = match doc.format {
            DocFormat::Json => serde_json::from_str(&text).unwrap(),
            DocFormat::Toml => {
                let table: toml::Table = text.parse().unwrap();
                serde_json::to_value(table).unwrap()
            }
        };
        match doc.container {
            None => document.get(name).cloned(),
            Some(key) => document.get(key)?.get(name).cloned(),
        }
    }

    /// Each host's skills land where that host reads them. DevCouncil's Go
    /// integrator kept its own copy of this map, which sent Codex, OpenCode and
    /// Warp DevMap's three defaults; the one map is here.
    #[test]
    fn each_host_installs_skills_where_it_reads_them() {
        for (host, want) in [
            (Host::Cursor, &[".cursor/skills"][..]),
            (Host::Claude, &[".claude/skills"][..]),
            (Host::Codex, &[".agents/skills"][..]),
            (Host::Antigravity, &[".agents/skills"][..]),
            (Host::OpenCode, &[][..]),
            (Host::Warp, &[][..]),
        ] {
            assert_eq!(host.skill_destinations(), want, "{host:?}");
            for dest in host.skill_destinations() {
                assert!(
                    !dest.starts_with('/'),
                    "{host:?}: {dest} is not project-relative"
                );
            }
        }
    }

    #[test]
    fn every_host_has_one_project_document_and_no_two_share_a_path() {
        let mut seen = std::collections::BTreeMap::new();
        for host in every_host() {
            let doc = host.project_document();
            if let Some(other) = seen.insert(doc.rel, host) {
                panic!("{host:?} and {other:?} both write {}", doc.rel);
            }
            assert_eq!(
                doc.format == DocFormat::Toml,
                doc.rel.ends_with(".toml"),
                "{host:?}: format disagrees with the file name {}",
                doc.rel
            );
        }
        // DevMap's own entry reaches Cursor and Claude through the plugin-aware
        // merge and Codex through the user-level config; only these three
        // carry it in the project document.
        let carrying: Vec<Host> = every_host()
            .into_iter()
            .filter(|host| host.mcp_document().is_some())
            .collect();
        assert_eq!(carrying, [Host::Antigravity, Host::OpenCode, Host::Warp]);
    }

    #[test]
    fn a_registered_server_keeps_every_neighbour_on_every_host() {
        for host in every_host() {
            let root = scratch(&format!("neighbours-{}", host.as_str()));
            let doc = host.project_document();
            let path = root.join(doc.rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let before = match (doc.format, doc.container) {
                (DocFormat::Toml, _) => "model = \"o3\" # keep this comment\n\n\
                    [mcp_servers.theirs]\ncommand = \"/opt/theirs\"\n"
                    .to_string(),
                (DocFormat::Json, Some(key)) => json!({
                    key: {"theirs": {"command": "/opt/theirs"}},
                    "unrelated": "keep me",
                })
                .to_string(),
                (DocFormat::Json, None) => {
                    json!({"theirs": {"command": "/opt/theirs"}}).to_string()
                }
            };
            fs::write(&path, &before).unwrap();

            let outcome = register_server(host, &root, &devcouncil_spec(&root), false).unwrap();
            assert!(outcome.changed, "{host:?}: reported no change");
            assert!(
                registered(host, &root, "devcouncil").is_some(),
                "{host:?}: our server is missing"
            );
            assert!(
                registered(host, &root, "theirs").is_some(),
                "{host:?}: dropped a neighbouring server"
            );
            let after = fs::read_to_string(&path).unwrap();
            match doc.format {
                // Byte-for-byte: comments and layout are the user's.
                DocFormat::Toml => assert!(
                    after.starts_with(&before),
                    "{host:?}: rewrote the user's TOML:\n{after}"
                ),
                DocFormat::Json if doc.container.is_some() => assert_eq!(
                    serde_json::from_str::<Value>(&after).unwrap()["unrelated"],
                    "keep me",
                    "{host:?}: dropped an unrelated top-level key"
                ),
                DocFormat::Json => {}
            }
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn a_registered_server_is_spelled_the_way_each_host_reads_it() {
        for host in every_host() {
            let root = scratch(&format!("shape-{}", host.as_str()));
            let spec = devcouncil_spec(&root);
            register_server(host, &root, &spec, false).unwrap();
            let entry = registered(host, &root, "devcouncil").unwrap();
            let doc = host.project_document();
            if doc.argv_form {
                assert_eq!(entry["type"], "local", "{host:?}: {entry}");
                assert_eq!(entry["command"], json!([spec.command, "mcp"]), "{host:?}");
                assert_eq!(entry["environment"], json!(spec.env), "{host:?}");
                assert!(entry.get("env").is_none(), "{host:?}: {entry}");
            } else {
                assert_eq!(entry["command"], spec.command, "{host:?}");
                assert_eq!(entry["args"], json!(["mcp"]), "{host:?}");
                assert_eq!(entry["env"], json!(spec.env), "{host:?}");
            }
            assert_eq!(
                entry.get("cwd").is_some(),
                doc.accepts_cwd,
                "{host:?}: cwd written where the host does not read it, or missing where it does: {entry}"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn a_second_registration_changes_nothing_and_a_check_agrees() {
        for host in every_host() {
            let root = scratch(&format!("idempotent-{}", host.as_str()));
            let spec = devcouncil_spec(&root);
            register_server(host, &root, &spec, false).unwrap();
            let path = root.join(host.project_document().rel);
            let first = fs::read(&path).unwrap();
            assert!(
                !register_server(host, &root, &spec, false).unwrap().changed,
                "{host:?}: a second apply reported a change"
            );
            assert_eq!(fs::read(&path).unwrap(), first, "{host:?}: bytes moved");
            assert!(
                !register_server(host, &root, &spec, true).unwrap().changed,
                "{host:?}: a check reported drift on a current file"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn a_dry_run_registration_writes_nothing() {
        for host in every_host() {
            let root = scratch(&format!("dry-{}", host.as_str()));
            let outcome = register_server(host, &root, &devcouncil_spec(&root), true).unwrap();
            assert!(outcome.changed, "{host:?}: a missing entry is a change");
            assert_eq!(
                fs::read_dir(&root).unwrap().count(),
                0,
                "{host:?}: dry run wrote into the tree"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn opencode_preamble_is_established_not_imposed() {
        let root = scratch("preamble");
        fs::write(
            root.join("opencode.json"),
            r#"{"$schema":"https://opencode.ai/config-v2.json"}"#,
        )
        .unwrap();
        register_server(Host::OpenCode, &root, &devcouncil_spec(&root), false).unwrap();
        assert_eq!(
            read(&root.join("opencode.json"))["$schema"],
            "https://opencode.ai/config-v2.json"
        );
        let fresh = scratch("preamble-fresh");
        register_server(Host::OpenCode, &fresh, &devcouncil_spec(&fresh), false).unwrap();
        assert_eq!(
            read(&fresh.join("opencode.json"))["$schema"],
            "https://opencode.ai/config.json"
        );
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&fresh).ok();
    }

    #[test]
    fn a_document_this_module_cannot_read_is_kept_not_replaced() {
        for (host, body) in [
            (Host::OpenCode, "{ this is not json"),
            (Host::OpenCode, r#"{"mcp": "not an object"}"#),
            (
                Host::OpenCode,
                "{\n  // a comment a rewrite would drop\n  \"mcp\": {}\n}",
            ),
            (Host::Claude, "[]"),
            (Host::Codex, "this is = = not toml"),
            // An inline server map cannot take a `[mcp_servers.devcouncil]`
            // table beside it; the append would not parse.
            (
                Host::Codex,
                "mcp_servers = { theirs = { command = \"/opt/theirs\" } }\n",
            ),
            (Host::Codex, "mcp_servers = 3\n"),
        ] {
            let root = scratch(&format!("unreadable-{}", host.as_str()));
            let path = root.join(host.project_document().rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            assert!(
                register_server(host, &root, &devcouncil_spec(&root), false).is_err(),
                "{host:?}: accepted {body:?}"
            );
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                body,
                "{host:?}: rewrote {body:?}"
            );
            fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn codex_replaces_only_its_own_table_and_keeps_every_other_byte() {
        let root = scratch("codex-replace");
        let path = root.join(".codex/config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let head = "# my settings\nmodel = \"o3\"\n\n";
        let ours_stale =
            "[mcp_servers.devcouncil]\ncommand = \"/old/devcouncil\"\nargs = [\"mcp\"]\n\n\
                          [mcp_servers.devcouncil.env]\nSTALE = \"1\"\n\n";
        let tail = "[mcp_servers.theirs] # keep\ncommand = \"/opt/theirs\"\n";
        fs::write(&path, format!("{head}{ours_stale}{tail}")).unwrap();

        let spec = devcouncil_spec(&root);
        assert!(
            register_server(Host::Codex, &root, &spec, false)
                .unwrap()
                .changed
        );
        let after = fs::read_to_string(&path).unwrap();
        assert!(after.starts_with(head), "lost the head:\n{after}");
        assert!(after.ends_with(tail), "lost the tail:\n{after}");
        assert!(!after.contains("STALE"), "kept a stale subtable:\n{after}");
        let entry = registered(Host::Codex, &root, "devcouncil").unwrap();
        assert_eq!(entry["command"], spec.command);
        assert_eq!(entry["env"], json!(spec.env));
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_document_or_parent_is_refused_and_nothing_lands_outside() {
        use std::os::unix::fs::symlink;
        for host in every_host() {
            let doc = host.project_document();
            // A linked leaf: the outside file must neither be disclosed into
            // the merge nor replaced.
            let base = scratch(&format!("link-leaf-{}", host.as_str()));
            let root = base.join("repo");
            let outside = base.join("outside");
            fs::create_dir_all(&root).unwrap();
            fs::create_dir_all(&outside).unwrap();
            let secret = outside.join("private");
            let secret_body = match doc.format {
                DocFormat::Json => r#"{"apiKey":"sk-SUPER-SECRET"}"#,
                DocFormat::Toml => "api_key = \"sk-SUPER-SECRET\"\n",
            };
            fs::write(&secret, secret_body).unwrap();
            let path = root.join(doc.rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            symlink(&secret, &path).unwrap();
            assert!(
                register_server(host, &root, &devcouncil_spec(&root), false).is_err(),
                "{host:?}: followed a linked {}",
                doc.rel
            );
            assert_eq!(fs::read_to_string(&secret).unwrap(), secret_body);
            assert!(fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink());
            fs::remove_dir_all(&base).ok();

            // A linked parent directory, where the document has one.
            let Some((parent, _)) = doc.rel.split_once('/') else {
                continue;
            };
            let base = scratch(&format!("link-parent-{}", host.as_str()));
            let root = base.join("repo");
            let outside = base.join("outside");
            fs::create_dir_all(&root).unwrap();
            fs::create_dir_all(&outside).unwrap();
            symlink(&outside, root.join(parent)).unwrap();
            assert!(
                register_server(host, &root, &devcouncil_spec(&root), false).is_err(),
                "{host:?}: wrote through a linked {parent}"
            );
            assert_eq!(
                fs::read_dir(&outside).unwrap().count(),
                0,
                "{host:?}: wrote outside the repository"
            );
            fs::remove_dir_all(&base).ok();
        }
    }

    #[test]
    fn a_server_request_is_validated_before_anything_is_written() {
        let ok = br#"{"servers":[{"name":"devcouncil","command":"/opt/dc","args":["mcp"],
                      "env":{"DEVCOUNCIL_PROJECT_ROOT":"/repo"},"cwd":"/repo"}]}"#;
        let specs = parse_server_request(ok).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].env["DEVCOUNCIL_PROJECT_ROOT"], "/repo");

        for (label, body) in [
            ("not json", "{".to_string()),
            ("not an object", "[]".to_string()),
            ("no servers", "{}".to_string()),
            ("empty", r#"{"servers":[]}"#.to_string()),
            ("unknown top key", r#"{"servers":[],"extra":1}"#.to_string()),
            (
                "unknown server key",
                r#"{"servers":[{"name":"a","command":"/x","shell":true}]}"#.to_string(),
            ),
            (
                "relative command",
                r#"{"servers":[{"name":"a","command":"devcouncil"}]}"#.to_string(),
            ),
            (
                "relative cwd",
                r#"{"servers":[{"name":"a","command":"/x","cwd":"repo"}]}"#.to_string(),
            ),
            (
                "devmap is reserved",
                r#"{"servers":[{"name":"devmap","command":"/x"}]}"#.to_string(),
            ),
            (
                "bad name",
                r#"{"servers":[{"name":"../x","command":"/x"}]}"#.to_string(),
            ),
            (
                "upper name",
                r#"{"servers":[{"name":"DevCouncil","command":"/x"}]}"#.to_string(),
            ),
            (
                "duplicate",
                r#"{"servers":[{"name":"a","command":"/x"},{"name":"a","command":"/y"}]}"#
                    .to_string(),
            ),
            (
                "arg not string",
                r#"{"servers":[{"name":"a","command":"/x","args":[1]}]}"#.to_string(),
            ),
            (
                "env not string",
                r#"{"servers":[{"name":"a","command":"/x","env":{"K":1}}]}"#.to_string(),
            ),
            (
                "env bad key",
                r#"{"servers":[{"name":"a","command":"/x","env":{"1K":"v"}}]}"#.to_string(),
            ),
            (
                "nul",
                "{\"servers\":[{\"name\":\"a\",\"command\":\"/x\\u0000\"}]}".to_string(),
            ),
            (
                "too many",
                format!(
                    r#"{{"servers":[{}]}}"#,
                    (0..=MAX_REQUESTED_SERVERS)
                        .map(|i| format!(r#"{{"name":"s{i}","command":"/x"}}"#))
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            ),
            ("oversized", " ".repeat(MAX_SERVER_REQUEST_BYTES + 1)),
        ] {
            assert!(
                parse_server_request(body.as_bytes()).is_err(),
                "{label}: accepted {body:?}"
            );
        }
    }

    #[test]
    fn a_server_its_document_refuses_stops_integrate_before_any_write() {
        let root = scratch("integrate-preflight");
        fs::write(root.join("opencode.json"), "{ not json").unwrap();
        let result = integrate(
            Host::OpenCode,
            &root,
            Path::new("/opt/devmap/bin/devmap"),
            &empty_map(),
            "m",
            "g",
            "s",
            std::slice::from_ref(&devcouncil_spec(&root)),
            false,
            false,
        );
        assert!(
            result.is_err(),
            "integrated past an unreadable opencode.json"
        );
        let mut left: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            ["opencode.json"],
            "DevMap's half was written before the refusal"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn integrate_writes_the_callers_server_and_check_reports_instead_of_failing() {
        let root = scratch("integrate-servers");
        let spec = devcouncil_spec(&root);
        let exe = PathBuf::from("/opt/devmap/bin/devmap");
        let map = empty_map();
        // Warp: no user-level document and no skills, so nothing here reaches
        // the user's home directory.
        let check = integrate(
            Host::Warp,
            &root,
            &exe,
            &map,
            "m",
            "g",
            "s",
            std::slice::from_ref(&spec),
            false,
            true,
        )
        .unwrap();
        assert!(!check.check_ok, "a missing server is drift");
        assert!(check.servers.iter().all(|s| s.changed));
        assert!(
            registered(Host::Warp, &root, "devcouncil").is_none(),
            "check wrote"
        );

        integrate(
            Host::Warp,
            &root,
            &exe,
            &map,
            "m",
            "g",
            "s",
            std::slice::from_ref(&spec),
            false,
            false,
        )
        .unwrap();
        assert!(registered(Host::Warp, &root, "devcouncil").is_some());
        assert!(registered(Host::Warp, &root, MCP_SERVER_NAME).is_some());
        let again = integrate(
            Host::Warp,
            &root,
            &exe,
            &map,
            "m",
            "g",
            "s",
            std::slice::from_ref(&spec),
            false,
            true,
        )
        .unwrap();
        assert!(
            again.check_ok,
            "a current install reported drift: {again:?}"
        );
        fs::remove_dir_all(&root).ok();
    }
}
