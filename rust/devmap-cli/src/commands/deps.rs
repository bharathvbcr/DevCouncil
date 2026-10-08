use devmap_query::{Request, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_edges, emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Deps")]
pub(crate) struct Args {
    pub(crate) file: String,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
    #[arg(long, default_value_t = 0.0)]
    pub(crate) min_confidence: f32,
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
        file,
        budget,
        min_confidence,
        min_rung,
    } = args;
    let store = open_for_read(cli)?;
    let engine = StoreQueryEngine::new(&store);
    // Both floors apply, at different places: `min_confidence` goes to
    // the store, which drops rows before the engine sees them, and the
    // rung is applied over what came back — so only the rung's cut is
    // countable in the histogram.
    let resp = engine.dependencies_at_rung(
        Request {
            query: file.clone(),
            token_budget: *budget,
            min_confidence: *min_confidence,
            max_depth: 1,
        },
        min_rung.as_deref().and_then(devmap_query::Rung::parse),
    )?;
    if cli.json {
        emit_json(cli, &serde_json::to_value(&resp)?)?;
    } else {
        emit_edges(&resp);
    }
    Ok(())
}
