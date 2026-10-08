//! One module per `devmap` subcommand. Each owns its arguments and its `run`;
//! [`run`] only dispatches.

use crate::cli::{Cli, Commands};
use crate::reporter::ProgressReporter;

pub(crate) mod affected;
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

pub(crate) async fn run(cli: &Cli, progress: Option<&ProgressReporter>) -> anyhow::Result<()> {
    match &cli.command {
        Commands::Build(args) => build::run(cli, progress, args),
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
        Commands::ApiImpact(args) => api_impact::run(cli, args),
        Commands::Html(args) => html::run(cli, args),
        Commands::Hook(args) => hook::run(cli, args),
        Commands::Claude(args) => claude::run(cli, args),
        Commands::Skills(args) => skills::run(cli, args),
        Commands::Integrate(args) => integrate::run(cli, args),
        Commands::Version(args) => version::run(cli, args),
    }
}
