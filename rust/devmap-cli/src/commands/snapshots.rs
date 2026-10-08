use devmap_query::{semantic_snapshots, Request};

use crate::cli::Cli;
use crate::output::{emit_json, open_for_read};

#[derive(clap::Args)]
#[group(id = "Snapshots")]
pub(crate) struct Args {
    #[arg(default_value = "")]
    pub(crate) file: String,
    #[arg(short, long, default_value_t = 2000)]
    pub(crate) budget: u32,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { file, budget } = args;
    let store = open_for_read(cli)?;
    let extractions = store.latest_extractions()?;
    let resp = semantic_snapshots(
        &extractions,
        Request {
            query: file.clone(),
            token_budget: *budget,
            min_confidence: 0.0,
            max_depth: 1,
        },
    );
    emit_json(cli, &serde_json::to_value(&resp)?)?;
    Ok(())
}
