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
        Commands::ApiImpact(_) => ("API impact", "Tracing the API ripples"),
        Commands::Claude(_) => ("Claude integration", "Checking the connections"),
        Commands::Skills(_) => ("Skills", "Packing your toolkit"),
        Commands::Integrate(_) => ("Host integration", "Connecting your tools"),
        Commands::Ask(_) => ("Ask", "Finding symbols by what they do"),
        Commands::Skeleton(_) => ("Skeleton", "Listing signatures without bodies"),
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

/// Bounded formatting even if one result contains a very long source line.
struct HumanText {
    buffer: String,
    color: bool,
    ascii: bool,
}

impl HumanText {
    fn flush(&mut self) {
        if self.color {
            super::write_stdout_raw(format_args!("{}", progress::accent_numbers(&self.buffer)));
        } else {
            super::write_stdout_raw(format_args!("{}", self.buffer));
        }
        self.buffer.clear();
    }
}

impl Write for HumanText {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        for c in text.chars() {
            if c != '\n'
                && (c.is_control()
                    || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
            {
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
