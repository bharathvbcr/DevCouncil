use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::{default_root_hint, Cli};
use crate::output::emit_json;
use crate::skills;

#[derive(clap::Args)]
#[group(id = "Skills")]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) action: SkillsAction,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    run_skills(cli, &args.action)
}

#[derive(Subcommand)]
pub(crate) enum SkillsAction {
    /// Write the five DevMap skills under each `--destination`.
    Install {
        /// Repository root that owns the skill directories and receipt.
        #[arg(long, default_value_os_t = default_root_hint())]
        project_root: PathBuf,
        /// Skill layout root, relative to the project (repeatable).
        ///
        /// Defaults to `.claude/skills`, `.cursor/skills`, and `.agents/skills`
        /// when omitted.
        #[arg(long = "destination")]
        destinations: Vec<String>,
        /// List differing paths without writing.
        #[arg(long)]
        dry_run: bool,
        /// Exit 0 only when every selected file already matches.
        #[arg(long)]
        check: bool,
    },
}

fn run_skills(cli: &Cli, action: &SkillsAction) -> anyhow::Result<()> {
    match action {
        SkillsAction::Install {
            project_root,
            destinations,
            dry_run,
            check,
        } => {
            let dests: Vec<&str> = if destinations.is_empty() {
                skills::DEFAULT_DESTINATIONS.to_vec()
            } else {
                destinations.iter().map(String::as_str).collect()
            };
            let report = skills::install_devmap_skills(project_root, &dests, *dry_run, *check)?;
            if *check && !report.check_ok {
                anyhow::bail!(
                    "skills install --check: {} file(s) missing or differ",
                    report.differing.len()
                );
            }
            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "written": report.written.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                        "differing": report.differing.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                        "receipt": report.receipt.display().to_string(),
                        "check_ok": report.check_ok,
                        "dry_run": dry_run,
                        "check": check,
                    }),
                )?;
            } else if *dry_run {
                if report.differing.is_empty() {
                    outln!("skills install: nothing to change");
                } else {
                    outln!(
                        "skills install would write {} file(s):",
                        report.differing.len()
                    );
                    for path in &report.differing {
                        outln!("  {}", path.display());
                    }
                }
            } else if *check {
                outln!(
                    "skills install --check: {} (receipt {})",
                    if report.check_ok { "ok" } else { "drift" },
                    report.receipt.display()
                );
            } else {
                outln!(
                    "skills install: wrote {} file(s); receipt {}",
                    report.written.len(),
                    report.receipt.display()
                );
            }
            Ok(())
        }
    }
}
