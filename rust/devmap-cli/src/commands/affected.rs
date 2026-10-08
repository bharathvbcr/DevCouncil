use devmap_query::{ResolutionAvailability, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{
    emit_blast_radius, emit_json, emit_truncation, emit_unavailable, open_for_read,
};

#[derive(clap::Args)]
#[group(id = "Affected")]
pub(crate) struct Args {
    #[arg(required = true, num_args = 1..)]
    pub(crate) targets: Vec<String>,
    #[arg(short, long, default_value_t = devmap_query::Budget::AFFECTED)]
    pub(crate) budget: u32,
    #[arg(long, default_value_t = 3)]
    pub(crate) depth: usize,
    #[arg(long, default_value_t = 0.0)]
    pub(crate) min_confidence: f32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        targets,
        budget,
        depth,
        min_confidence,
    } = args;
    let store = open_for_read(cli)?;
    let report =
        StoreQueryEngine::new(&store).affected_tests(targets, *budget, *min_confidence, *depth)?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_affected(&report);
    }
    Ok(())
}

fn emit_affected(report: &devmap_query::AffectedTestsReport) {
    if let ResolutionAvailability::Unavailable { reason } = &report.tests.resolution {
        emit_unavailable(reason);
        return;
    }
    for test in &report.tests.items {
        outln!(
            "{}  depth {}  {} reached symbol(s)",
            test.path,
            test.depth,
            test.reached_symbols
        );
    }
    emit_truncation(
        report.tests.shown,
        report.tests.hidden,
        report.tests.total,
        report.tests.truncated,
    );
    if let Some(reason) = &report.tests.walk_incomplete {
        outln!("warning: {reason}");
    }
    emit_blast_radius(&report.blast_radius);
}
