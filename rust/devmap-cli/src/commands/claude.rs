use std::path::{Path, PathBuf};

use clap::Subcommand;

use crate::claude;
use crate::cli::Cli;
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Claude")]
pub(crate) struct Args {
    #[command(subcommand)]
    pub(crate) action: ClaudeAction,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    run_claude(cli, &args.action)
}

#[derive(Subcommand)]
pub(crate) enum ClaudeAction {
    /// Print the `hooks` block for `.claude/settings.json`.
    ///
    /// Printed, not installed: a settings file is the user's, and merging into
    /// it is their edit to make. Everything needed to make it is here.
    Hooks {
        /// Command to write into the emitted handlers.
        ///
        /// Unset, `devmap` is emitted when that name on `PATH` resolves to this
        /// binary, and this binary's absolute path otherwise. Set it when the
        /// install location is known but not yet populated — packaging.
        #[arg(long)]
        binary: Option<PathBuf>,
    },

    /// List every documented hook event beside what Dev Map does about it.
    ///
    /// Coverage stated as a decision per event, so "not handled" is on the
    /// record with its reason rather than being an omission nobody counted.
    Events,

    /// Write the installable plugin bundle: marketplace, manifest, hooks, MCP.
    Plugin {
        /// Directory the bundle is written under.
        ///
        /// Unset, resolved to `<state dir>/devmap-plugin`. Deliberately *not*
        /// `<state dir>/claude-plugin`, which is DevCouncil's own bundle: the
        /// two emitters write a single-repo marketplace to the same
        /// `.claude-plugin/marketplace.json`, under different names
        /// (`devcouncil-local` and `devmap-local`), so sharing the directory
        /// meant whichever ran last silently replaced the other's registration.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Render to stdout instead of writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Command to write into the emitted hooks and MCP entry. See
        /// `claude hooks --binary`.
        #[arg(long)]
        binary: Option<PathBuf>,
    },

    /// Check an existing hook config, plugin manifest, or marketplace file.
    Validate {
        path: PathBuf,
        /// Treat warnings as errors, as `claude plugin validate --strict` does.
        #[arg(long)]
        strict: bool,
    },
}

/// `devmap claude …` — emission and validation of Dev Map's Claude Code surface.
///
/// Nothing here opens the store or starts anything: a user configures a host
/// before the repository has ever been indexed, so every one of these must work
/// on a fresh clone. Same contract as `serve --print-socket-path` and
/// `mcp --print-config`.
fn run_claude(cli: &Cli, action: &ClaudeAction) -> anyhow::Result<()> {
    let subcommands = claude::known_subcommands::<Cli>();
    match action {
        ClaudeAction::Hooks { binary } => {
            let executable = claude::plugin_command(&std::env::current_exe()?, binary.as_deref());
            let block = claude::hooks_block(&executable, &subcommands)?;
            emit_json(cli, &block)
        }
        ClaudeAction::Events => {
            let coverage = claude::event_coverage();
            if cli.json {
                let rows: Vec<serde_json::Value> = coverage
                    .iter()
                    .map(|(event, hook, reason)| {
                        serde_json::json!({
                            "event": event,
                            "handled": hook.is_some(),
                            // The authorization surface, named in the data so a
                            // consumer checking it reads the same list the
                            // writer enforces rather than a copy of it.
                            "decides_permission":
                                claude::PERMISSION_DECIDING_EVENTS.contains(event),
                            "matcher": hook.map(|h| h.matcher),
                            "hook_event": hook.map(|h| h.hook_event),
                            "reason": reason,
                        })
                    })
                    .collect();
                let handled = coverage.iter().filter(|(_, h, _)| h.is_some()).count();
                // Both numbers, always: "2 handled" beside a list of two reads
                // as complete coverage of a surface that has 33 events.
                emit_json(
                    cli,
                    &serde_json::json!({
                        "events_total": coverage.len(),
                        "events_handled": handled,
                        "events": rows,
                    }),
                )
            } else {
                for (event, hook, reason) in &coverage {
                    match hook {
                        Some(hook) => outln!(
                            "{event:<20} handled   devmap hook {} (matcher {:?})\n{:22}{reason}",
                            hook.hook_event,
                            hook.matcher,
                            ""
                        ),
                        None => outln!("{event:<20} -\n{:22}{reason}", ""),
                    }
                }
                let handled = coverage.iter().filter(|(_, h, _)| h.is_some()).count();
                outln!("\n{handled} of {} events handled", coverage.len());
                Ok(())
            }
        }
        ClaudeAction::Plugin {
            out,
            dry_run,
            binary,
        } => {
            let executable = claude::plugin_command(&std::env::current_exe()?, binary.as_deref());
            let version = env!("CARGO_PKG_VERSION");
            let out = &out
                .clone()
                .unwrap_or_else(|| devmap_extract::paths::plugin_dir(Path::new(".")));
            if *dry_run {
                let rendered = claude::render_plugin_bundle(
                    &executable,
                    &cli.db(),
                    Some(version),
                    &subcommands,
                )?;
                let mut files: serde_json::Map<String, serde_json::Value> = rendered
                    .into_iter()
                    .map(|(path, body)| {
                        let key = path
                            .to_str()
                            .ok_or_else(|| {
                                anyhow::anyhow!("bundle path is not valid UTF-8: {path:?}")
                            })?
                            .to_string();
                        let value = if path.extension().and_then(|e| e.to_str()) == Some("json") {
                            serde_json::from_str::<serde_json::Value>(&body)?
                        } else {
                            serde_json::Value::String(body)
                        };
                        Ok((key, value))
                    })
                    .collect::<anyhow::Result<_>>()?;
                // Binary assets are described, not inlined: a dry run that
                // omitted them entirely would report a bundle the writer does
                // not produce, and one that inlined the bytes would be a PNG
                // in a terminal.
                for (path, bytes) in claude::plugin_binary_assets() {
                    let key = path
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("asset path is not valid UTF-8: {path:?}"))?
                        .to_string();
                    files.insert(key, serde_json::json!({ "bytes": bytes.len() }));
                }
                return emit_json(cli, &serde_json::Value::Object(files));
            }
            let written = claude::write_plugin_bundle(
                out,
                &executable,
                &cli.db(),
                Some(version),
                &subcommands,
            )?;
            let changed = written.iter().filter(|f| f.changed).count();
            if cli.json {
                emit_json(
                    cli,
                    &serde_json::json!({
                        "out": out,
                        "files": written.iter().map(|f| serde_json::json!({
                            "path": f.path,
                            "changed": f.changed,
                        })).collect::<Vec<_>>(),
                        "changed": changed,
                    }),
                )
            } else {
                for file in &written {
                    outln!(
                        "{} {}",
                        if file.changed { "wrote  " } else { "current" },
                        file.path.display()
                    );
                }
                outln!(
                    "\n{changed} of {} file(s) changed. Install with:\n  \
                     claude plugin marketplace add {}\n  claude plugin install {}@{}",
                    written.len(),
                    out.display(),
                    claude::PLUGIN_NAME,
                    claude::MARKETPLACE_NAME,
                );
                Ok(())
            }
        }
        ClaudeAction::Validate { path, strict } => {
            let report = claude::validate_file(path, *strict)?;
            if cli.json {
                emit_json(cli, &report.to_json())?;
            } else {
                for diagnostic in &report.diagnostics {
                    outln!("{diagnostic}");
                }
                outln!(
                    "{}: {} error(s), {} warning(s){}",
                    if report.ok() { "ok" } else { "FAILED" },
                    report.errors().count(),
                    report.warnings().count(),
                    if report.strict {
                        " (strict: warnings block)"
                    } else {
                        ""
                    },
                );
            }
            // A validator that exits 0 on a failed document is worse than none:
            // a CI step reads the code, not the prose.
            if !report.ok() {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}
