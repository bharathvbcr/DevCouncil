use devmap_query::{Request, StoreQueryEngine};

use crate::cli::Cli;
use crate::output::{emit_blast_radius, emit_edges, emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Impact")]
pub(crate) struct Args {
    pub(crate) target: String,
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
    /// Also band the reached symbols by distance from the target.
    ///
    /// The flat edge list says *what* reaches the target; it cannot say how
    /// far, because an edge does not carry the hop the walk found it at. A
    /// consumer that needs "3 call it directly and 39 are reached through
    /// those 3" gets it from the kernel here rather than inventing it.
    ///
    /// Refused together with `--min-rung`: the band walk filters on
    /// confidence and has no rung, so honouring one would narrow the edges
    /// and leave the bands wide.
    #[arg(long)]
    pub(crate) layers: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        target,
        budget,
        depth,
        min_rung,
        layers,
    } = args;
    let store = open_for_read(cli)?;
    let engine = StoreQueryEngine::new(&store);
    let req = Request {
        query: target.clone(),
        token_budget: *budget,
        min_confidence: 0.0,
        max_depth: *depth,
    };
    if *layers {
        // `--min-rung` with `--layers` was refused in validation, so
        // dropping the floor here cannot silently widen an answer a
        // caller asked to narrow.
        let resp = engine.impact_layered(req)?;
        if cli.json {
            emit_json(cli, &serde_json::to_value(&resp)?)?;
        } else {
            emit_edges(&resp.edges);
            emit_blast_radius(&resp.blast_radius);
        }
    } else {
        let resp = engine.impact_at_rung(
            req,
            // Already validated above, so `None` here means "none was
            // asked for", never "one was asked for and did not parse".
            min_rung.as_deref().and_then(devmap_query::Rung::parse),
        )?;
        if cli.json {
            emit_json(cli, &serde_json::to_value(&resp)?)?;
        } else {
            emit_edges(&resp);
        }
    }
    Ok(())
}
