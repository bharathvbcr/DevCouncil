use crate::cli::Cli;
use crate::output::emit_json;
use crate::session;

#[derive(clap::Args)]
#[group(id = "GapRecord")]
pub(crate) struct Args {
    /// The tool whose answer fell short (`devmap_dead_symbols`, …).
    #[arg(long)]
    pub(crate) tool: String,
    /// Stable id for this gap, so a later session can resolve it.
    #[arg(long = "gap-id")]
    pub(crate) gap_id: String,
    /// What was asked, what came back, and why that is a gap.
    #[arg(long)]
    pub(crate) reason: String,
    /// The repository the gap was observed in, when it is not this one.
    #[arg(long)]
    pub(crate) repo_path: Option<String>,
    /// Mark a previously recorded gap as closed.
    #[arg(long)]
    pub(crate) resolved: bool,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args {
        tool,
        gap_id,
        reason,
        repo_path,
        resolved,
    } = args;
    let payload = session::record_gap(
        &cli.db(),
        tool,
        gap_id,
        reason,
        repo_path.as_deref(),
        *resolved,
    )?;
    if cli.json {
        emit_json(cli, &payload)?;
    } else {
        outln!("recorded gap {gap_id} for {tool}");
    }
    Ok(())
}
