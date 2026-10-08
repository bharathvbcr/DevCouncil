use crate::cli::Cli;
use crate::output::emit_json;
use crate::session;

#[derive(clap::Args)]
#[group(id = "SessionReport")]
pub(crate) struct Args {
    /// Print the previous report instead of writing a new one.
    #[arg(long)]
    pub(crate) last: bool,
    /// Host session id, when the hook has one.
    #[arg(long)]
    pub(crate) session_id: Option<String>,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { last, session_id } = args;
    let payload = session::run(&cli.db(), *last, session_id.as_deref(), cli.json)?;
    if cli.json {
        emit_json(cli, &payload)?;
    }
    Ok(())
}
