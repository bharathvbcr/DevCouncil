use devmap_query::{ResolutionAvailability, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{
    emit_blast_radius, emit_json, emit_scope, emit_truncation, emit_unavailable, open_for_read,
};

#[derive(clap::Args)]
#[group(id = "Explore")]
pub(crate) struct Args {
    pub(crate) query: String,
    /// Definitions to consider. The budget decides how many are packed;
    /// both numbers are reported.
    #[arg(short, long, default_value_t = 20)]
    pub(crate) limit: usize,
    #[arg(short, long, default_value_t = devmap_query::Budget::EXPLORE)]
    pub(crate) budget: u32,
    #[arg(long, default_value_t = 3)]
    pub(crate) depth: usize,
    #[arg(long, default_value_t = 0.0)]
    pub(crate) min_confidence: f32,
    /// Definitions under this repository-relative path prefix (repeatable).
    /// A prefix matching no indexed file is refused.
    #[arg(long = "path", value_name = "PREFIX")]
    pub(crate) paths: Vec<String>,
    /// Definitions in this language, as the index labels it (repeatable).
    #[arg(long = "language", value_name = "LANGUAGE")]
    pub(crate) languages: Vec<String>,
    /// Definitions of this kind (repeatable). Case is ignored.
    #[arg(long = "kind", value_name = "KIND")]
    pub(crate) kinds: Vec<String>,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        query,
        limit,
        budget,
        depth,
        min_confidence,
        paths,
        languages,
        kinds,
    } = args;
    let filter = devmap_query::NameQueryFilter::new(paths, languages, kinds)?;
    let store = open_for_read(cli)?;
    let report = StoreQueryEngine::new(&store).explore_filtered(
        query,
        *limit,
        *budget,
        *min_confidence,
        *depth,
        filter.as_ref(),
    )?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&report)?)?;
    } else {
        emit_explore(&report);
    }
    Ok(())
}

fn emit_explore(report: &devmap_query::ExploreReport) {
    if let ResolutionAvailability::Unavailable { reason } = &report.definitions.resolution {
        emit_unavailable(reason);
        return;
    }
    emit_scope(report.scope.as_ref());
    for definition in &report.definitions.items {
        outln!(
            "{}:{}-{}  {}  {}",
            definition.file_path,
            definition.span.0,
            definition.span.1,
            definition.kind,
            definition.id
        );
        // Class A: an unreadable file is reported as unread, never as a symbol
        // whose body happens to be empty.
        match &definition.source_unavailable_reason {
            Some(reason) => outln!("  source unavailable: {reason}"),
            None => {
                for line in definition.source.lines() {
                    outln!("  {line}");
                }
                if let Some(omitted) = definition.source_omitted_bytes {
                    outln!("  ... {omitted} bytes omitted to fit the budget");
                }
            }
        }
        outln!(
            "  callers: {} of {}{}   callees: {} of {}{}",
            definition.callers.shown,
            definition.callers.total,
            if definition.callers.truncated {
                " (truncated)"
            } else {
                ""
            },
            definition.callees.shown,
            definition.callees.total,
            if definition.callees.truncated {
                " (truncated)"
            } else {
                ""
            },
        );
    }
    emit_truncation(
        report.definitions.shown,
        report.definitions.hidden,
        report.definitions.total,
        report.definitions.truncated,
    );
    outln!(
        "budget: {} total = {} definitions + {} per edge direction + {} blast radius",
        report.budget.total,
        report.budget.definitions,
        report.budget.edges_per_direction,
        report.budget.blast_radius
    );
    emit_blast_radius(&report.blast_radius);
}
