//! One module per `devmap` subcommand. Each owns its arguments and its `run`;
//! [`run`] only dispatches.

use crate::cli::{Cli, Commands};
use crate::reporter::ProgressReporter;

pub(crate) mod affected;
pub(crate) mod agents;
pub(crate) mod api_impact;
pub(crate) mod ask;
pub(crate) mod ast;
pub(crate) mod blast;
pub(crate) mod build;
pub(crate) mod claude;
pub(crate) mod clones;
pub(crate) mod cypher;
pub(crate) mod dead;
pub(crate) mod deps;
pub(crate) mod doc_drift;
pub(crate) mod doctor;
pub(crate) mod explore;
pub(crate) mod export;
pub(crate) mod freshness;
pub(crate) mod gap_record;
pub(crate) mod history;
pub(crate) mod hook;
pub(crate) mod html;
pub(crate) mod impact;
pub(crate) mod integrate;
pub(crate) mod lean_tcb;
pub(crate) mod literals;
pub(crate) mod manifest;
pub(crate) mod map_html;
pub(crate) mod mcp;
pub(crate) mod neighbors;
pub(crate) mod paths;
pub(crate) mod pdg;
pub(crate) mod preview;
pub(crate) mod repair;
pub(crate) mod routes;
pub(crate) mod savings;
pub(crate) mod search;
pub(crate) mod seed;
pub(crate) mod serve;
pub(crate) mod session_report;
pub(crate) mod shape_check;
pub(crate) mod skeleton;
pub(crate) mod skills;
pub(crate) mod snapshots;
pub(crate) mod status;
pub(crate) mod suspects;
pub(crate) mod trace;
pub(crate) mod version;
pub(crate) mod workspace;

/// Run the command and, when it is one an agent uses to query the index, put
/// it on the session's record the way the MCP server does (live-log row and
/// queried marker), so a session that asks through the CLI is not told it has
/// asked nothing. One place covers text output, `--json` and the error path;
/// a failed write never changes the command's outcome.
pub(crate) async fn run(cli: &Cli, progress: Option<&ProgressReporter>) -> anyhow::Result<()> {
    let Some(tag) = cli.command.query_tag() else {
        return dispatch(cli, progress).await;
    };
    let started = std::time::Instant::now();
    let outcome = dispatch(cli, progress).await;
    let tool = devmap_serve::tool_for_command(tag)
        .map(str::to_string)
        .unwrap_or_else(|| format!("devmap_{tag}"));
    let error = outcome.as_ref().err().map(|error| error.to_string());
    let answer = crate::output::take_answered();
    devmap_serve::session_log::append_query_from(
        Some(devmap_serve::session_log::ORIGIN_CLI),
        &cli.db(),
        devmap_serve::session_log::host_session_id(),
        &tool,
        None,
        answer.as_ref(),
        error.as_deref(),
        started.elapsed().as_millis() as u64,
    );
    outcome
}

async fn dispatch(cli: &Cli, progress: Option<&ProgressReporter>) -> anyhow::Result<()> {
    match &cli.command {
        Commands::Build(args) => build::run(cli, progress, args),
        Commands::Seed(args) => seed::run(cli, args),
        Commands::Search(args) => search::run(cli, args),
        Commands::Ask(args) => ask::run(cli, args),
        Commands::Deps(args) => deps::run(cli, args),
        Commands::Impact(args) => impact::run(cli, args),
        Commands::Neighbors(args) => neighbors::run(cli, args),
        Commands::Trace(args) => trace::run(cli, args),
        Commands::Dead(args) => dead::run(cli, args),
        Commands::Skeleton(args) => skeleton::run(cli, args),
        Commands::Suspects(args) => suspects::run(cli, args),
        Commands::Blast(args) => blast::run(cli, args),
        Commands::Explore(args) => explore::run(cli, args),
        Commands::Literals(args) => literals::run(cli, args),
        Commands::DocDrift(args) => doc_drift::run(cli, args),
        Commands::Affected(args) => affected::run(cli, args),
        Commands::Preview(args) => preview::run(cli, args),
        Commands::Workspace(args) => workspace::run(cli, args),
        Commands::Savings(args) => savings::run(cli, args),
        Commands::Clones(args) => clones::run(cli, args),
        Commands::Manifest(args) => manifest::run(cli, args),
        Commands::MapHtml(args) => map_html::run(cli, args),
        Commands::Freshness(args) => freshness::run(cli, args),
        Commands::Status(args) => status::run(cli, args),
        Commands::Doctor => doctor::run(cli),
        Commands::SessionReport(args) => session_report::run(cli, args),
        Commands::GapRecord(args) => gap_record::run(cli, args),
        Commands::Paths(args) => paths::run(cli, args),
        Commands::History(args) => history::run(cli, args),
        Commands::Repair(args) => repair::run(cli, args),
        Commands::Snapshots(args) => snapshots::run(cli, args),
        Commands::Serve(args) => serve::run(cli, args).await,
        Commands::Mcp(args) => mcp::run(cli, args).await,
        Commands::Pdg(args) => pdg::run(cli, args),
        Commands::Cypher(args) => cypher::run(cli, args),
        Commands::Ast(args) => ast::run(cli, args),
        Commands::Export(args) => export::run(cli, args),
        Commands::Routes(args) => routes::run(cli, args),
        Commands::ShapeCheck(args) => shape_check::run(cli, args),
        Commands::LeanTcb(args) => lean_tcb::run(cli, args),
        Commands::ApiImpact(args) => api_impact::run(cli, args),
        Commands::Html(args) => html::run(cli, args),
        Commands::Hook(args) => hook::run(cli, args),
        Commands::Claude(args) => claude::run(cli, args),
        Commands::Skills(args) => skills::run(cli, args),
        Commands::Agents(args) => agents::run(cli, args),
        Commands::Integrate(args) => integrate::run(cli, args),
        Commands::Version(args) => version::run(cli, args),
    }
}
