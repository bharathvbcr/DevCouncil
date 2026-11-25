//! Cross-repo org graph federation CLI (`devmap federate status|impact`).

use std::path::PathBuf;

use clap::Subcommand;
use devmap_query::{
    federate_impact_with_opts, federate_status_with_opts, workspace::workspace_path,
    FederateOptions, DEFAULT_FEDERATE_LIMIT,
};

use crate::cli::{root_parser, Cli};
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Federate")]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) action: FederateAction,
}

#[derive(Subcommand)]
pub(crate) enum FederateAction {
    /// Inspect health and degradation state of every repository declared in a workspace manifest.
    Status {
        /// Path to multi-repo workspace.json (defaults to this repository's workspace.json).
        #[arg(long, value_parser = root_parser())]
        workspace: Option<PathBuf>,
        /// Allow repository roots outside the workspace manifest's parent directory.
        #[arg(long, default_value_t = false)]
        allow_external_roots: bool,
    },
    /// Find cross-repo references, imports, and namesakes for an exported symbol across sibling repos.
    Impact {
        /// Exported symbol name (bare or qualified, e.g. `CalculateInvoiceTotal` or `pkg::Foo`).
        symbol: String,
        /// Path to multi-repo workspace.json (defaults to this repository's workspace.json).
        #[arg(long, value_parser = root_parser())]
        workspace: Option<PathBuf>,
        /// Optional source repository name to treat as the symbol's origin.
        #[arg(long)]
        source_repo: Option<String>,
        /// Maximum hits to return (bounded completeness envelope; default 200, max 1000).
        #[arg(long, default_value_t = DEFAULT_FEDERATE_LIMIT)]
        limit: usize,
        /// Allow repository roots outside the workspace manifest's parent directory.
        #[arg(long, default_value_t = false)]
        allow_external_roots: bool,
    },
}

fn resolve_workspace_path(cli: &Cli, explicit: Option<&PathBuf>) -> PathBuf {
    if let Some(p) = explicit {
        return p.clone();
    }
    let root = match &cli.db {
        Some(db) => devmap_extract::paths::repo_root_from_store(db)
            .unwrap_or_else(|| cli.root_hint().to_path_buf()),
        None => cli.root_hint().to_path_buf(),
    };
    let root = root.canonicalize().unwrap_or(root);
    workspace_path(&root)
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    match &args.action {
        FederateAction::Status {
            workspace,
            allow_external_roots,
        } => {
            let ws_path = resolve_workspace_path(cli, workspace.as_ref());
            let opts = FederateOptions {
                allow_external_roots: *allow_external_roots,
            };
            let report = federate_status_with_opts(&ws_path, opts)?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&report)?)?;
            } else {
                outln!(
                    "workspace={} queried={}/{} degraded={} walk_incomplete={}",
                    report.workspace_path,
                    report.repos_queried,
                    report.repos_total,
                    report.repos_degraded.len(),
                    report.walk_incomplete
                );
                for repo in &report.repos {
                    if repo.healthy {
                        outln!(
                            "  ok       {:<20} schema={} gen={} symbols={} edges={}",
                            repo.repo,
                            repo.schema_version.unwrap_or(0),
                            repo.generation.unwrap_or(0),
                            repo.symbols.unwrap_or(0),
                            repo.edges.unwrap_or(0),
                        );
                    } else {
                        outln!(
                            "  degraded {:<20} {}",
                            repo.repo,
                            repo.degraded_reason.as_deref().unwrap_or("unknown")
                        );
                    }
                }
            }
        }
        FederateAction::Impact {
            symbol,
            workspace,
            source_repo,
            limit,
            allow_external_roots,
        } => {
            let ws_path = resolve_workspace_path(cli, workspace.as_ref());
            let opts = FederateOptions {
                allow_external_roots: *allow_external_roots,
            };
            let report =
                federate_impact_with_opts(&ws_path, symbol, source_repo.as_deref(), *limit, opts)?;
            if cli.json {
                emit_json(cli, &serde_json::to_value(&report)?)?;
            } else {
                outln!(
                    "target={} repos_queried={} degraded={} shown={}/{} truncated={} walk_incomplete={}",
                    report.target,
                    report.repos_queried,
                    report.repos_degraded.len(),
                    report.shown,
                    report.total,
                    report.truncated,
                    report.walk_incomplete
                );
                for def in &report.defining_repos {
                    outln!(
                        "  defined  [{}] {}: {} ({})",
                        def.repo,
                        def.file_path,
                        def.qualified_name,
                        def.kind
                    );
                }
                for hit in &report.hits {
                    outln!(
                        "  hit      [{}] {} ({:?}, conf={:.2}): {}",
                        hit.repo,
                        hit.file_path,
                        hit.kind,
                        hit.confidence,
                        hit.evidence
                    );
                }
                for deg in &report.repos_degraded {
                    outln!("  degraded [{}] {}", deg.repo, deg.degraded_reason);
                }
            }
        }
    }
    Ok(())
}
