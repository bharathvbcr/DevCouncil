//! Claude Code agent definitions whose tool grant leaves DevMap out.
//!
//! A subagent type with a `tools:` line is handed exactly what that line names
//! and nothing else, MCP tools included. A list such as `Read, Grep, Glob`
//! therefore gives every subagent of that type zero `devmap_*` tools, while the
//! instructions it carries tell it to ask DevMap first; the only symptom is a
//! subagent that navigates by grep. Nothing in Claude Code reports the gap, so
//! `devmap doctor` does: it reads the agent definitions Claude Code loads from
//! `~/.claude/agents/` and `<project>/.claude/agents/` and names each file
//! whose grant excludes DevMap, with the exact entries to add.
//!
//! The check reads and never writes: these files are hand-authored by the
//! person who owns them, and the remedy is one line they add themselves.

use std::fs;
use std::path::{Path, PathBuf};

use crate::claude::{MCP_SERVER_NAME, PLUGIN_NAME};

/// Every server a `devmap_*` tool is served under in Claude Code, as the
/// `mcp__<server>` stem its tool IDs carry.
///
/// One `devmap mcp` server reaches a session under up to three names: the
/// user-scope `devmap` entry, the `devmap` plugin's own `.mcp.json`, and the
/// `gitpulse` plugin, which serves the same `devmap_*` tools beside its own.
/// `scripts/sync-agent-defs.mjs::MCP_TOOL_PREFIXES` declares the same three for
/// the agents DevCouncil generates, and `scripts/mcp-served-tools.mjs` fails
/// when that declaration no longer matches the live hosts.
fn devmap_servers() -> [String; 3] {
    [
        format!("mcp__{MCP_SERVER_NAME}"),
        format!("mcp__plugin_{PLUGIN_NAME}_{MCP_SERVER_NAME}"),
        "mcp__plugin_gitpulse_gitpulse".to_string(),
    ]
}

/// The servers a whole-server grant may name to mean DevMap.
///
/// The gitpulse plugin is left out: granting it whole hands an agent the task
/// board's write tools too, which is not what a DevMap remedy should ask for.
/// A gitpulse-served `devmap_*` tool named singly still counts.
fn devmap_only_servers() -> [String; 2] {
    let [user, plugin, _] = devmap_servers();
    [user, plugin]
}

/// The entries the warning tells the owner to append to a `tools:` line.
///
/// Claude Code's subagent docs: "`mcp__<server>` or `mcp__<server>__*` grants
/// or removes every tool from the named server." Naming both DevMap servers
/// covers a session that loads the plugin and one that loads only the
/// user-scope entry; a server the session does not serve is inert.
pub fn remedy_entries() -> String {
    let [user, plugin] = devmap_only_servers();
    format!("{plugin}, {user}")
}

/// Most agent files read from one directory. A person's agents directory holds
/// a handful; past this the scan says it stopped rather than reading on.
const MAX_AGENT_FILES_PER_DIR: usize = 256;

/// Largest agent definition read. Agent files are a frontmatter block and a
/// prompt; one larger than this is not one Claude Code is meant to load.
const MAX_AGENT_FILE_BYTES: u64 = 256 * 1024;

/// One agent definition that cannot reach DevMap, or could not be checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToolGap {
    pub path: PathBuf,
    pub reason: String,
}

/// What a scan found, including what it could not look at.
#[derive(Debug, Default)]
pub struct AgentToolScan {
    pub gaps: Vec<AgentToolGap>,
    /// Directories cut at [`MAX_AGENT_FILES_PER_DIR`]; their remaining files
    /// were not checked.
    pub capped: Vec<PathBuf>,
}

/// The agent directories Claude Code loads for a session rooted at `project`.
pub fn agent_dirs(home: Option<&Path>, project: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home {
        dirs.push(home.join(".claude").join("agents"));
    }
    let project_dir = project.join(".claude").join("agents");
    if !dirs.contains(&project_dir) {
        dirs.push(project_dir);
    }
    dirs
}

/// Check every `*.md` agent definition directly in `dirs`.
///
/// A missing directory is the ordinary case and is not reported. A file that
/// exists but cannot be read is reported as a gap: an unchecked grant must not
/// read the same as a grant that was checked and found to reach DevMap.
pub fn scan(dirs: &[PathBuf]) -> AgentToolScan {
    let mut out = AgentToolScan::default();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect();
        files.sort();
        if files.len() > MAX_AGENT_FILES_PER_DIR {
            files.truncate(MAX_AGENT_FILES_PER_DIR);
            out.capped.push(dir.clone());
        }
        for path in files {
            let reason = match read_bounded(&path) {
                Ok(text) => grant_gap(&text),
                Err(error) => Some(format!(
                    "could not be read, so its grant is unchecked: {error}"
                )),
            };
            if let Some(reason) = reason {
                out.gaps.push(AgentToolGap { path, reason });
            }
        }
    }
    out
}

fn read_bounded(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut text = String::new();
    file.take(MAX_AGENT_FILE_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_AGENT_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("larger than the {MAX_AGENT_FILE_BYTES}-byte agent file bound"),
        ));
    }
    Ok(text)
}

/// Why this agent definition cannot reach DevMap, or `None` when it can or
/// was written not to.
///
/// The failure this exists for is an allowlist that forgot DevMap. An agent
/// that removes DevMap in `disallowedTools` said so on purpose — a docs or web
/// researcher has no code to navigate — and warning about it on every run
/// would leave a warning nobody can clear. With no `tools` field the agent
/// inherits every tool the session has, which includes DevMap.
fn grant_gap(text: &str) -> Option<String> {
    let frontmatter = frontmatter(text)?;
    if list_field(frontmatter, "disallowedTools")
        .is_some_and(|denied| denied.iter().any(|entry| denies_devmap(entry)))
    {
        return None;
    }
    let tools = list_field(frontmatter, "tools")?;
    if tools
        .iter()
        .any(|entry| entry == "*" || grants_devmap(entry))
    {
        return None;
    }
    Some("its `tools:` list names no DevMap tool".to_string())
}

/// The `---`-delimited block a definition opens with, or `None` without one.
fn frontmatter(text: &str) -> Option<&str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some(&rest[..offset]);
        }
        offset += line.len();
    }
    None
}

/// The entries of one list-valued frontmatter key, in any form YAML writes a
/// tool list in: a comma-separated scalar (`tools: Read, Grep`), a flow
/// sequence (`tools: [Read, Grep]`), a block sequence (`- Read` lines), or a
/// scalar folded over indented continuation lines. `None` when the key is
/// absent, which for `tools` means "inherit everything".
fn list_field(frontmatter: &str, key: &str) -> Option<Vec<String>> {
    let mut lines = frontmatter.lines();
    let mut parts: Vec<String> = Vec::new();
    let mut found = false;
    for line in lines.by_ref() {
        if let Some(value) = line
            .strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(':'))
        {
            parts.push(value.to_string());
            found = true;
            break;
        }
    }
    if !found {
        return None;
    }
    for line in lines {
        if !line.starts_with([' ', '\t']) && !line.trim_start().starts_with("- ") {
            break;
        }
        parts.push(line.to_string());
    }
    let entries = parts
        .iter()
        .flat_map(|part| {
            let part = part.trim();
            let part = part.strip_prefix("- ").unwrap_or(part);
            part.split(',').map(str::to_string).collect::<Vec<_>>()
        })
        .map(|entry| {
            entry
                .trim()
                .trim_matches(|c| matches!(c, '[' | ']'))
                .trim()
                .trim_matches(|c| matches!(c, '"' | '\''))
                .to_string()
        })
        .filter(|entry| !entry.is_empty())
        .collect();
    Some(entries)
}

/// The bare `mcp__<server>` an entry grants whole, if it is a server grant.
fn whole_server(entry: &str) -> Option<&str> {
    let stem = entry.strip_suffix("__*").unwrap_or(entry);
    (stem.starts_with("mcp__") && !stem[5..].contains("__")).then_some(stem)
}

fn grants_devmap(entry: &str) -> bool {
    if let Some(server) = whole_server(entry) {
        return devmap_only_servers().iter().any(|s| s == server);
    }
    devmap_servers().iter().any(|server| {
        entry
            .strip_prefix(server.as_str())
            .and_then(|rest| rest.strip_prefix("__"))
            .is_some_and(|tool| tool.starts_with("devmap_"))
    })
}

fn denies_devmap(entry: &str) -> bool {
    if entry == "mcp__*" {
        return true;
    }
    // The gitpulse server is not DevMap's to opt out of: denying it whole
    // removes gitpulse's own tools, and DevMap's own servers still serve.
    whole_server(entry).is_some_and(|server| devmap_only_servers().iter().any(|s| s == server))
}

/// The doctor warning for a scan, or `None` when every definition reaches
/// DevMap and nothing was left unchecked.
pub fn warning(scan: &AgentToolScan) -> Option<String> {
    if scan.gaps.is_empty() && scan.capped.is_empty() {
        return None;
    }
    let mut message = String::new();
    if !scan.gaps.is_empty() {
        let named = scan
            .gaps
            .iter()
            .map(|gap| format!("{} ({})", gap.path.display(), gap.reason))
            .collect::<Vec<_>>()
            .join("; ");
        message.push_str(&format!(
            "agent definition(s) whose tool grant excludes DevMap, so subagents of that type \
             cannot call devmap_* and navigate by grep: {named}. Append to each `tools:` line: \
             `{}`; for an agent that should not navigate code, add `disallowedTools: {}` \
             instead to record that it is deliberate",
            remedy_entries(),
            remedy_entries()
        ));
    }
    if !scan.capped.is_empty() {
        if !message.is_empty() {
            message.push_str(". ");
        }
        let dirs = scan
            .capped
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        message.push_str(&format!(
            "only the first {MAX_AGENT_FILES_PER_DIR} agent files were checked in {dirs}; the \
             rest are unchecked"
        ));
    }
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gap(frontmatter: &str) -> Option<String> {
        grant_gap(&format!("---\nname: a\n{frontmatter}---\nbody\n"))
    }

    #[test]
    fn an_explicit_list_without_devmap_is_a_gap() {
        assert!(gap("tools: Read, Edit, Write, Grep, Glob, Bash\n").is_some());
        assert!(gap("tools: [Read, Grep]\n").is_some());
        assert!(gap("tools:\n  - Read\n  - Grep\n").is_some());
    }

    #[test]
    fn no_list_or_a_devmap_grant_is_not_a_gap() {
        assert_eq!(gap("model: opus\n"), None);
        assert_eq!(gap("tools: *\n"), None);
        assert_eq!(gap("tools: Read, mcp__plugin_devmap_devmap\n"), None);
        assert_eq!(gap("tools: Read, mcp__devmap__*\n"), None);
        assert_eq!(
            gap("tools:\n  - Read\n  - mcp__devmap__devmap_search\n"),
            None
        );
        assert_eq!(
            gap("tools: Read,\n  mcp__plugin_gitpulse_gitpulse__devmap_impact\nmodel: opus\n"),
            None
        );
    }

    #[test]
    fn a_namesake_that_is_not_devmap_is_still_a_gap() {
        // The gitpulse server whole is not a DevMap remedy, and a gitpulse
        // tool is not a devmap tool.
        assert!(gap("tools: Read, mcp__plugin_gitpulse_gitpulse\n").is_some());
        assert!(gap("tools: mcp__plugin_gitpulse_gitpulse__gitpulse_insights\n").is_some());
        // A server whose name merely starts with devmap's is another server.
        assert!(gap("tools: mcp__devmapx__devmap_search, mcp__devmapx\n").is_some());
        // `tools` must be the key itself, not a key that starts with it.
        assert_eq!(gap("toolset: Read\n"), None);
    }

    #[test]
    fn an_explicit_devmap_denial_is_a_deliberate_opt_out() {
        assert_eq!(
            gap("tools: Read, WebFetch\ndisallowedTools: mcp__*\n"),
            None
        );
        assert_eq!(
            gap("tools: Read\ndisallowedTools: mcp__plugin_devmap_devmap__*\n"),
            None
        );
        // Denying something else is not an opt-out of DevMap.
        assert!(
            gap("tools: Read\ndisallowedTools: Bash, mcp__plugin_gitpulse_gitpulse\n").is_some()
        );
        assert!(gap("tools: Read\ndisallowedTools: Bash\n").is_some());
    }

    #[test]
    fn a_file_without_frontmatter_declares_no_list() {
        assert_eq!(grant_gap("just a prompt\n"), None);
        assert_eq!(grant_gap("---\ntools: Read\nno terminator\n"), None);
    }

    #[test]
    fn the_remedy_is_a_grant_the_predicate_accepts() {
        let line = format!("tools: Read, {}\n", remedy_entries());
        assert_eq!(gap(&line), None, "{line}");
    }
}
