//! One presentation scope for finite commands. Protocols and exports opt out
//! explicitly. Results stream through the existing stdout owner; no pipe,
//! process-wide descriptor swap, unbounded capture, or second renderer.

use std::cell::{Cell, RefCell};
use std::fmt::Write;
use std::io::IsTerminal;
use std::rc::Rc;
use std::time::Instant;

use super::{progress, Cli, Commands};
use crate::commands::claude::ClaudeAction;

thread_local! {
    // The CLI entry future and its synchronous result emitters run on this
    // thread. Worker tasks never inherit a terminal presentation scope.
    static ACTIVE: RefCell<Option<Rc<Session>>> = const { RefCell::new(None) };
}

pub(super) struct Session {
    pub(super) display: progress::Display,
    title: &'static str,
    started: Instant,
    human: bool,
    finished: Cell<bool>,
}

impl Session {
    pub(super) fn start(cli: &Cli) -> Option<Rc<Self>> {
        let (title, activity) = profile(&cli.command)?;
        let display = progress::Display::new(cli.progress, cli.json, cli.verbose);
        let human = !cli.json
            && display.enabled()
            && std::io::stdout().is_terminal()
            && std::io::stderr().is_terminal()
            && std::env::var_os("TERM").is_none_or(|term| term != "dumb");
        display.stage(0, activity);
        let session = Rc::new(Self {
            display,
            title,
            started: Instant::now(),
            human,
            finished: Cell::new(false),
        });
        ACTIVE.with(|active| *active.borrow_mut() = Some(Rc::clone(&session)));
        Some(session)
    }

    pub(super) fn finish(&self, succeeded: bool) {
        if self.finished.replace(true) {
            return;
        }
        self.display.finish("");
        ACTIVE.with(|active| active.borrow_mut().take());
        if self.human && succeeded {
            self.display.summary(
                self.title,
                &[format!(
                    "Finished in {}",
                    progress::duration(self.started.elapsed().as_secs_f64())
                )],
            );
        }
    }
}

/// Exhaustive command policy: a new enum variant must make an explicit choice.
fn profile(command: &Commands) -> Option<(&'static str, &'static str)> {
    Some(match command {
        Commands::Build(_)
        | Commands::Serve(_)
        | Commands::Mcp(_)
        | Commands::Version(_)
        | Commands::Hook(_) => return None,
        Commands::Export(crate::commands::export::Args { out, .. })
            if out.as_deref() == Some(std::path::Path::new("-")) =>
        {
            return None
        }
        Commands::Claude(crate::commands::claude::Args {
            action: ClaudeAction::Hooks { .. } | ClaudeAction::Plugin { dry_run: true, .. },
        }) => return None,
        Commands::Suspects(_) => ("Suspects", "Asking what could have caused this"),
        Commands::Blast(_) => ("Blast radius", "Following what this change reaches"),
        Commands::Search(_) => ("Search", "Looking for the right thread"),
        Commands::Literals(_) => ("Literals", "Finding where a string is written"),
        Commands::DocDrift(_) => ("Doc drift", "Checking the docs against the code"),
        Commands::Deps(_) => ("Dependencies", "Following the connections"),
        Commands::Impact(_) => ("Impact", "Tracing the ripples"),
        Commands::Neighbors(_) => ("Neighbors", "Meeting the neighbors"),
        Commands::Trace(_) => ("Call trail", "Following the call trail"),
        Commands::Dead(_) => ("Dead-code candidates", "Looking for loose ends"),
        Commands::Explore(_) => ("Explore", "Unfolding the map"),
        Commands::Affected(_) => ("Affected tests", "Following the test trails"),
        Commands::Preview(_) => ("Edit preview", "Trying the next shape"),
        Commands::Workspace(_) => ("Workspace", "Connecting your repositories"),
        Commands::Savings(_) => ("Savings", "Counting the shortcuts"),
        Commands::Clones(_) => ("Similar code", "Finding familiar shapes"),
        Commands::Manifest(_) => ("Manifest", "Packing the essentials"),
        Commands::MapHtml(_) | Commands::Html(_) => {
            ("Map visualization", "Giving the graph a view")
        }
        Commands::Freshness(_) => ("Freshness", "Checking what changed"),
        Commands::Status(_) => ("Status", "Taking the pulse"),
        Commands::Doctor => ("Diagnostics", "Checking the moving parts"),
        Commands::SessionReport(_) => ("Session report", "Retracing the session"),
        Commands::GapRecord(_) => ("Gap recorded", "Writing down what could not be answered"),
        Commands::Paths(_) => ("Paths", "Getting our bearings"),
        Commands::History(_) => ("Build history", "Turning back the pages"),
        Commands::Repair(_) => ("Store repair", "Mending the map"),
        Commands::Snapshots(_) => ("Snapshots", "Gathering the snapshots"),
        Commands::Pdg(_) => ("Data flow", "Following the data"),
        Commands::Cypher(_) => ("Graph query", "Asking the graph"),
        Commands::Ast(_) => ("Syntax", "Reading the structure"),
        Commands::Export(_) => ("Graph export", "Packing the graph"),
        Commands::Routes(_) => ("Routes", "Following the routes"),
        Commands::ShapeCheck(_) => ("Shape check", "Checking the fit"),
        Commands::LeanTcb(_) => ("Lean TCB audit", "Auditing proof boundaries"),
        Commands::ApiImpact(_) => ("API impact", "Tracing the API ripples"),
        Commands::Claude(_) => ("Claude integration", "Checking the connections"),
        Commands::Skills(_) => ("Skills", "Packing your toolkit"),
        Commands::Agents(_) => ("Agents", "Installing host agents"),
        Commands::Integrate(_) => ("Host integration", "Connecting your tools"),
        Commands::Ask(_) => ("Ask", "Finding symbols by what they do"),
        Commands::Skeleton(_) => ("Skeleton", "Listing signatures without bodies"),
        Commands::Seed(_) => ("Seed", "Cloning worktree index"),
    })
}

pub(super) fn write_human(message: std::fmt::Arguments<'_>) -> bool {
    let session = ACTIVE.with(|active| active.borrow().clone());
    let Some(session) = session else {
        return false;
    };
    // Clear the optional animation before the first result byte, including
    // redirected/JSON output. Later result writes do not restart the worker.
    session.display.finish("");
    if !session.human {
        return false;
    }
    let mut writer = HumanText {
        buffer: String::with_capacity(4096),
        color: progress::color_enabled(),
        ascii: progress::ascii_output(),
    };
    // HumanText cannot fail: the canonical stdout owner handles actual I/O
    // failures with a nonzero exit, rather than discarding fmt::Result.
    std::fmt::write(&mut writer, message).expect("HumanText formatting is infallible");
    writer.flush();
    true
}

/// Whether a terminal or a bidirectional renderer would act on `c`: the shared
/// agent-visible escape set ([`devmap_query::bounded::is_agent_unsafe`]).
/// Newline is text structure and is not one.
pub(super) fn is_terminal_unsafe(c: char) -> bool {
    c != '\n' && devmap_query::bounded::is_agent_unsafe(c)
}

/// `c` spelled `\uXXXX`, or `\u{X}` above the BMP so the digits never run on
/// into the text that follows. The spelling the hook line and the pipe share.
pub(crate) fn unicode_escape(c: char) -> String {
    match u32::from(c) {
        bmp @ 0..=0xffff => format!("\\u{bmp:04x}"),
        cp => format!("\\u{{{cp:x}}}"),
    }
}

/// `text` with each character [`is_terminal_unsafe`] written as `\uXXXX`, or
/// `None` when it needs no change. Tab is kept: tab-separated output is a
/// format, and a terminal gives it no meaning beyond spacing.
pub(super) fn escape_for_pipe(text: &str) -> Option<String> {
    let unsafe_char = |c: char| c != '\t' && is_terminal_unsafe(c);
    if !text.chars().any(unsafe_char) {
        return None;
    }
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if unsafe_char(c) {
            out.push_str(&unicode_escape(c));
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// Bounded formatting even if one result contains a very long source line.
struct HumanText {
    buffer: String,
    color: bool,
    ascii: bool,
}

impl HumanText {
    fn flush(&mut self) {
        if self.color {
            super::write_stdout_styled(format_args!("{}", progress::accent_numbers(&self.buffer)));
        } else {
            super::write_stdout_styled(format_args!("{}", self.buffer));
        }
        self.buffer.clear();
    }
}

impl Write for HumanText {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        for c in text.chars() {
            if is_terminal_unsafe(c) {
                self.buffer.extend(c.escape_default());
            } else if self.ascii && !c.is_ascii() {
                self.buffer.extend(c.escape_unicode());
            } else {
                self.buffer.push(c);
            }
            if self.buffer.len() >= 4096 {
                self.flush();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod escape_tests {
    use super::escape_for_pipe;

    /// The terminal sink escapes the same agent-visible set as the hook and
    /// `bounded_text` (zero-width and tag characters included); newline and
    /// tab stay, and RTL letters are untouched.
    #[test]
    fn escape_for_pipe_escapes_zero_width_and_tag_characters() {
        for c in [
            '\u{200B}',
            '\u{200C}',
            '\u{200D}',
            '\u{2060}',
            '\u{FEFF}',
            '\u{2028}',
            '\u{E0041}',
        ] {
            let out = escape_for_pipe(&format!("a{c}b"))
                .unwrap_or_else(|| panic!("U+{:04X} kept", c as u32));
            assert!(
                !out.contains(c) && out.starts_with("a\\u"),
                "U+{:04X}: {out:?}",
                c as u32
            );
        }
        assert_eq!(escape_for_pipe("a\nb\tc"), None);
        for text in devmap_query::bounded::RTL_LETTER_FIXTURES {
            assert_eq!(escape_for_pipe(text), None);
        }
    }
}
