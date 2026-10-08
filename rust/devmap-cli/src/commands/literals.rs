use devmap_query::StoreQueryEngine;

use crate::cli::Cli;
use crate::output::{emit_json, emit_truncation, open_for_read};

#[derive(clap::Args)]
#[group(id = "Literals")]
pub(crate) struct Args {
    pub(crate) query: String,
    /// Require the value to equal the query.
    #[arg(long)]
    pub(crate) exact: bool,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        query,
        exact,
        budget,
    } = args;
    let store = open_for_read(cli)?;
    let report = StoreQueryEngine::new(&store).literals(query, *exact, *budget)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_literals(&report);
    }
    Ok(())
}

fn emit_literals(report: &devmap_query::LiteralReport) {
    let mode = if report.exact { "exact" } else { "prefix" };
    outln!("literals {mode} {:?}", report.query);
    for site in &report.items {
        let symbol = if site.qualified_name.is_empty() {
            site.symbol_name.as_str()
        } else {
            site.qualified_name.as_str()
        };
        outln!(
            "{}:{}  {}  {}",
            site.file_path,
            site.line,
            symbol,
            site.value
        );
    }
    emit_truncation(report.shown, report.hidden, report.total, report.truncated);
    if let Some(reason) = &report.walk_incomplete {
        outln!("warning: {reason}");
    }
}
