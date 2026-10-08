use crate::cli::Cli;
use crate::hook;
use crate::output::emit_json;

#[derive(clap::Args)]
#[group(id = "Hook")]
pub(crate) struct Args {
    /// `session-start`, `pre-tool-use`, `post-tool-use`, or `session-end`.
    ///
    /// `pre-tool-use` is the one event `devmap integrate` will not install
    /// for you. On Claude Code and Cursor that event decides an
    /// authorization outcome, and `claude::hooks_block` refuses to emit a
    /// handler onto any such event: a code index has nothing to contribute
    /// to a permission decision, and a table that could place one there is
    /// one edit away from widening what this tool can approve.
    ///
    /// The handler itself only ever emits `additionalContext` — never a
    /// decision field, which `pre_tool_use_never_emits_a_permission_decision`
    /// asserts against the document it actually produces. So it is safe to
    /// wire by hand, deliberately, in your own host configuration:
    ///
    ///   "PreToolUse": [{ "matcher": "Read|Grep|Glob", "hooks": [
    ///     { "type": "command", "timeout": 5,
    ///       "command": "\"/abs/path/to/devmap\" hook pre-tool-use" }]}]
    ///
    /// It restates the DevMap directive once per session, at the first
    /// navigation tool call, and stays silent when the index cannot answer.
    pub(crate) event: String,
}

pub(crate) fn run(cli: &Cli, args: &Args) -> anyhow::Result<()> {
    let Args { event } = args;
    let code = run_hook_command(cli, event)?;
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// Write one hook diagnostic straight to stderr.
///
/// Not through [`diagnostic`]. That path hands the line to an asynchronous
/// writer, and a failing hook ends in `std::process::exit`, which runs no
/// destructors and waits for nothing — so the message was dropped on exactly
/// the paths that had something to report. Measured 2026-09-12:
/// `devmap hook bogus-event` exited 1 with completely empty stdout and stderr,
/// while every exit-0 path printed its note normally.
///
/// A hook has one bounded line to say and no progress display to share it
/// with, so it writes and flushes that line itself.
fn hook_diagnostic(message: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{message}");
    let _ = err.flush();
}

fn run_hook_command(cli: &Cli, event_name: &str) -> anyhow::Result<i32> {
    let Some(event) = hook::HookEvent::parse(event_name) else {
        hook_diagnostic(format_args!(
            "devmap hook: unknown event {event_name:?}; expected session-start, \
             pre-tool-use, post-tool-use, or session-end"
        ));
        return Ok(1);
    };
    let stdin = hook::read_stdin_bounded().unwrap_or_default();
    let executable = std::env::current_exe()?;
    let outcome = hook::run_hook(event, &stdin, &executable, cli.root.as_deref());
    if let Some(line) = &outcome.stderr_line {
        hook_diagnostic(format_args!("{line}"));
    }
    if let Some(stdout) = &outcome.stdout {
        if cli.json {
            emit_json(cli, stdout)?;
        } else {
            let payload: serde_json::Value =
                serde_json::from_slice(&stdin).unwrap_or(serde_json::Value::Null);
            if let Some(rendered) = hook::render_host_stdout(event, &payload, stdout) {
                outln!("{rendered}");
            }
        }
    }
    Ok(outcome.exit_code)
}
