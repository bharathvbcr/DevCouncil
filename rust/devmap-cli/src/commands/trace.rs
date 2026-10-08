use devmap_query::{Request, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_edges, emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Trace")]
pub(crate) struct Args {
    pub(crate) from: String,
    pub(crate) to: Option<String>,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
    #[arg(long, default_value_t = 3)]
    pub(crate) depth: usize,
    /// Keep only edges at or above a named rung on the resolution ladder:
    /// `deterministic`, `high` or `speculative`. Omitted filters nothing.
    ///
    /// A name rather than a `--min-confidence` float, because the ladder
    /// has named rungs and a caller wanting deterministic edges should not
    /// have to know that means 1.0. The answer carries a `rungs` histogram
    /// of the population *before* the cut, so a short list is never
    /// mistaken for a sparse graph.
    #[arg(long)]
    pub(crate) min_rung: Option<String>,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        from,
        to,
        budget,
        depth,
        min_rung,
    } = args;
    let store = open_for_read(cli)?;
    let engine = StoreQueryEngine::new(&store);
    let rung = min_rung.as_deref().and_then(devmap_query::Rung::parse);
    let resp = if let Some(destination) = to {
        // The path variant answers with one path rather than an edge
        // population, so there is nothing for a histogram to describe
        // and the floor goes to the walk as a confidence. Exact all the
        // same: `floor_millis / 1000.0` reconstructs the float the
        // confidence constants were built from, bit for bit.
        engine.trace_between(Request {
            query: (from.clone(), destination.clone()),
            token_budget: *budget,
            min_confidence: rung.map_or(0.0, |r| r.floor_millis() as f32 / 1000.0),
            max_depth: *depth,
        })?
    } else {
        engine.trace_at_rung(
            Request {
                query: from.clone(),
                token_budget: *budget,
                min_confidence: 0.0,
                max_depth: *depth,
            },
            rung,
        )?
    };
    if cli.json {
        emit_json(cli, &serde_json::to_value(&resp)?)?;
    } else {
        emit_edges(&resp);
    }
    Ok(())
}
