use std::path::{Path, PathBuf};

use crate::cli::{default_root_hint, Cli};
use crate::output::emit_json;
use crate::{claude, hook, integrate};

#[derive(clap::Args)]
#[group(id = "Integrate")]
pub(crate) struct Args {
    /// Host to integrate.
    // The accepted names are clap's own, read off `integrate::Host`, so
    // this help cannot advertise a set the integrator does not accept —
    // which is what a hand-written list here did, naming three of six.
    #[arg(value_enum)]
    pub(crate) host: integrate::Host,
    /// Repository root (defaults to the current worktree).
    #[arg(long, default_value_os_t = default_root_hint())]
    pub(crate) project_root: PathBuf,
    /// Report what would change without writing.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Exit non-zero when on-disk assets differ from the expected install.
    #[arg(long)]
    pub(crate) check: bool,
    /// Command path written into MCP entries. Unset: this binary.
    #[arg(long)]
    pub(crate) binary: Option<PathBuf>,
    /// Also register the servers named on stdin, in each host's own
    /// document shape: `{"servers": [{"name", "command", "args"?, "env"?,
    /// "cwd"?}]}`. `command` and `cwd` are absolute; `devmap` is reserved.
    /// DevCouncil passes its own server this way.
    #[arg(long)]
    pub(crate) servers_stdin: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        host,
        project_root,
        dry_run,
        check,
        binary,
        servers_stdin,
    } = args;
    run_integrate(
        cli,
        *host,
        project_root,
        *dry_run,
        *check,
        binary.as_deref(),
        *servers_stdin,
    )
}

fn run_integrate(
    cli: &Cli,
    host: integrate::Host,
    project_root: &Path,
    dry_run: bool,
    check: bool,
    binary: Option<&Path>,
    servers_stdin: bool,
) -> anyhow::Result<()> {
    // Read and validated before anything is written: a malformed request must
    // not leave DevMap's half applied and the caller's half missing.
    let servers = if servers_stdin {
        let bytes = hook::read_stdin_limited(integrate::MAX_SERVER_REQUEST_BYTES)?;
        integrate::parse_server_request(&bytes)?
    } else {
        Vec::new()
    };
    let executable = claude::plugin_command(&std::env::current_exe()?, binary);
    let db = cli.db();
    let (map, map_rel, graph_rel, store_rel) = integrate_map_context(project_root, &db)?;
    let report = integrate::integrate(
        host,
        project_root,
        &executable,
        &map,
        &map_rel,
        &graph_rel,
        &store_rel,
        &servers,
        dry_run,
        check,
    )?;
    if cli.json {
        emit_json(
            cli,
            &serde_json::json!({
                "host": host.as_str(),
                "guides": report.guides.iter().map(|g| serde_json::json!({
                    "path": g.path.display().to_string(),
                    "disposition": match g.disposition {
                        devmap_query::guides::GuideDisposition::Created => "created",
                        devmap_query::guides::GuideDisposition::Updated => "updated",
                        devmap_query::guides::GuideDisposition::Unchanged => "unchanged",
                        devmap_query::guides::GuideDisposition::NotOurs => "not_ours",
                    },
                })).collect::<Vec<_>>(),
                "skills_written": report.skills_written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "skills_differing": report.skills_differing.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "global_mcp": report.global_mcp.iter().map(|m| serde_json::json!({
                    "path": m.path.display().to_string(),
                    "changed": m.changed,
                    "removed_stale_db": m.removed_stale_db,
                    "note": m.note,
                })).collect::<Vec<_>>(),
                "project_mcp": report.project_mcp.iter().map(|m| serde_json::json!({
                    "path": m.path.display().to_string(),
                    "changed": m.changed,
                    "removed_stale_db": m.removed_stale_db,
                    "note": m.note,
                })).collect::<Vec<_>>(),
                "hooks": report.hooks.iter().map(|m| serde_json::json!({
                    "path": m.path.display().to_string(),
                    "changed": m.changed,
                    "note": m.note,
                })).collect::<Vec<_>>(),
                "servers": report.servers.iter().map(|m| serde_json::json!({
                    "path": m.path.display().to_string(),
                    "changed": m.changed,
                    "note": m.note,
                })).collect::<Vec<_>>(),
                "notes": report.notes,
                "dry_run": dry_run,
                "check": check,
                "check_ok": report.check_ok,
            }),
        )?;
    } else {
        outln!(
            "integrate {}:{}",
            host.as_str(),
            if dry_run {
                " dry-run"
            } else if check {
                " check"
            } else {
                ""
            }
        );
        for guide in &report.guides {
            if guide.changed()
                || !matches!(
                    guide.disposition,
                    devmap_query::guides::GuideDisposition::Unchanged
                )
            {
                outln!("  guide {}: {:?}", guide.path.display(), guide.disposition);
            }
        }
        if !report.skills_written.is_empty() {
            outln!("  skills wrote {}", report.skills_written.len());
        } else if !report.skills_differing.is_empty() {
            outln!("  skills differing {}", report.skills_differing.len());
        }
        for mcp in report
            .global_mcp
            .iter()
            .chain(report.project_mcp.iter())
            .chain(report.hooks.iter())
            .chain(report.servers.iter())
        {
            outln!("  {}: {}", mcp.path.display(), mcp.note);
        }
        for note in &report.notes {
            outln!("  note: {note}");
        }
    }
    // Printed first, then refused: the report is what says which asset differs.
    if check && !report.check_ok {
        anyhow::bail!("integrate --check: assets differ from the expected installation");
    }
    Ok(())
}

/// Map context for integrate: prefer an on-disk repo map, else an empty shell
/// so guides still name the canonical relative paths.
fn integrate_map_context(
    project_root: &Path,
    db: &Path,
) -> anyhow::Result<(serde_json::Value, String, String, String)> {
    let map_path = devmap_extract::paths::repo_map_path(project_root);
    let graph_path = devmap_extract::paths::code_graph_path(project_root);
    let relative = |absolute: &Path| -> String {
        let text = absolute
            .strip_prefix(project_root)
            .unwrap_or(absolute)
            .to_string_lossy()
            .replace('\\', "/");
        text.strip_prefix("./").unwrap_or(&text).to_string()
    };
    let map_rel = relative(&map_path);
    let graph_rel = relative(&graph_path);
    let store_rel = relative(db);
    let map = if map_path.is_file() {
        let text = std::fs::read_to_string(&map_path)?;
        serde_json::from_str(&text).unwrap_or_else(|_| integrate::empty_map())
    } else {
        integrate::empty_map()
    };
    Ok((map, map_rel, graph_rel, store_rel))
}
