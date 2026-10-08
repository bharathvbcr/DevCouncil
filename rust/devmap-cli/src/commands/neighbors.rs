use devmap_query::StoreQueryEngine;

use crate::cli::Cli;
use crate::output::{emit_edges, emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Neighbors")]
pub(crate) struct Args {
    #[arg(required = true, num_args = 1..)]
    pub(crate) targets: Vec<String>,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
    #[arg(long, default_value_t = 1)]
    pub(crate) depth: usize,
    #[arg(long, default_value_t = 0.0)]
    pub(crate) min_confidence: f32,
    /// Keep only edges at or above a named rung on the resolution ladder:
    /// `deterministic`, `high` or `speculative`. Omitted filters nothing.
    ///
    /// Accepted here because this command *is* `impact` and `deps` composed,
    /// and both take the floor. A composition that drops a filter its parts
    /// accept answers the narrower question its caller did not ask.
    #[arg(long)]
    pub(crate) min_rung: Option<String>,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        targets,
        budget,
        depth,
        min_confidence,
        min_rung,
    } = args;
    let store = open_for_read(cli)?;
    let engine = StoreQueryEngine::new(&store);
    // `check_rung` above has already refused an unparseable name, so a
    // `None` here means no floor was asked for and never that one was
    // asked for and dropped.
    let answers = engine.neighbors_at_rung(
        targets,
        *budget,
        *min_confidence,
        *depth,
        min_rung.as_deref().and_then(devmap_query::Rung::parse),
    )?;
    if cli.json {
        emit_json(cli, &serde_json::json!({ "neighbors": answers }))?;
    } else {
        for entry in &answers {
            outln!("{}", entry.target);
            outln!("  callers:");
            emit_edges(&entry.callers);
            outln!("  callees:");
            emit_edges(&entry.callees);
        }
    }
    Ok(())
}
