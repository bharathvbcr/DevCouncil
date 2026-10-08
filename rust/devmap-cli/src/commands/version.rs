use crate::cli::{version_line, Cli};

#[derive(clap::Args)]
#[group(id = "Version")]
pub(crate) struct Args {
    /// Emit machine-readable JSON.
    #[arg(long)]
    pub(crate) json: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { json } = args;
    if *json || cli.json {
        outln!(
            "{{\"ok\":true,\"id\":\"devmap\",\"component\":\"devmap\",\"version\":\"{}\",\"store_schema\":{},\"code_graph_schema\":{},\"build\":\"{}\"}}",
            env!("CARGO_PKG_VERSION"),
            devmap_store::CURRENT_SCHEMA_VERSION,
            devmap_query::CODE_GRAPH_SCHEMA_VERSION,
            env!("DEVMAP_BUILD_ID")
        );
    } else {
        outln!("{}", version_line());
    }
    Ok(())
}
