//! `devmap agents install`: the one writer of `.agents/agents`, `.agents/rules`
//! and `.devcouncil-agents.json`.

use std::path::PathBuf;

use clap::Subcommand;

use crate::agent_install::{self, MAX_DOCUMENT_BYTES};
use crate::cli::{default_root_hint, Cli};
use crate::hook;
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Agents")]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) action: AgentsAction,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    run_agents(cli, &args.action)
}

#[derive(Subcommand)]
pub(crate) enum AgentsAction {
    /// Write the agents and rules named on stdin.
    Install {
        /// Repository root that owns `.agents/` and the receipt.
        #[arg(long, default_value_os_t = default_root_hint())]
        project_root: PathBuf,
        /// List differing paths without writing.
        #[arg(long)]
        dry_run: bool,
        /// Exit 0 only when every selected file already matches.
        #[arg(long)]
        check: bool,
        /// Read `{"agents":[{"name","content"}],"rules":[{"name","content"}]}`.
        ///
        /// Required. This command has no embedded library of its own; DevCouncil
        /// passes the one it embeds.
        #[arg(long, required = true)]
        agents_stdin: bool,
    },
}

fn run_agents(cli: &Cli, action: &AgentsAction) -> anyhow::Result<()> {
    match action {
        AgentsAction::Install {
            project_root,
            dry_run,
            check,
            agents_stdin,
        } => {
            if !agents_stdin {
                anyhow::bail!("agents install needs --agents-stdin");
            }
            let bytes = hook::read_stdin_limited(MAX_DOCUMENT_BYTES)?;
            let document = agent_install::parse_document(&bytes)?;
            let report = agent_install::install_agents(
                project_root,
                &document.agents,
                &document.rules,
                *dry_run,
                *check,
            )?;
            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "written": report.written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                        "differing": report.differing.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                        "managed": report.managed.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                        "receipt": report.receipt.display().to_string(),
                        "check_ok": report.check_ok,
                        "dry_run": dry_run,
                        "check": check,
                    }),
                )?;
            } else if *dry_run {
                if report.differing.is_empty() {
                    outln!("agents install: nothing to change");
                } else {
                    outln!(
                        "agents install would write {} file(s):",
                        report.differing.len()
                    );
                    for path in &report.differing {
                        outln!("  {}", path.display());
                    }
                }
            } else if *check {
                outln!(
                    "agents install --check: {} (receipt {})",
                    if report.check_ok { "ok" } else { "drift" },
                    report.receipt.display()
                );
            } else {
                outln!(
                    "agents install: wrote {} file(s); receipt {}",
                    report.written.len(),
                    report.receipt.display()
                );
            }
            if *check && !report.check_ok {
                anyhow::bail!(
                    "agents install --check: {} file(s) missing or differ",
                    report.differing.len()
                );
            }
            Ok(())
        }
    }
}
