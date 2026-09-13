//! One presentation scope for finite commands. Protocols and exports opt out
//! explicitly. Results stream through the existing stdout owner; no pipe,
//! process-wide descriptor swap, unbounded capture, or second renderer.

use std::cell::{Cell, RefCell};
use std::fmt::Write;
use std::io::IsTerminal;
use std::rc::Rc;
use std::time::Instant;

use super::{progress, ClaudeAction, Cli, Commands};

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
        Commands::Build { .. }
        | Commands::Serve { .. }
        | Commands::Mcp { .. }
        | Commands::Hook { .. } => return None,
        Commands::Export { out, .. } if out.as_deref() == Some(std::path::Path::new("-")) => {
            return None
        }
        Commands::Claude {
            action: ClaudeAction::Hooks { .. } | ClaudeAction::Plugin { dry_run: true, .. },
        } => return None,
        Commands::Search { .. } => ("Search", "Looking for the right thread"),
        Commands::Deps { .. } => ("Dependencies", "Following the connections"),
        Commands::Impact { .. } => ("Impact", "Tracing the ripples"),
        Commands::Neighbors { .. } => ("Neighbors", "Meeting the neighbors"),
        Commands::Trace { .. } => ("Call trail", "Following the call trail"),
        Commands::Dead { .. } => ("Dead-code candidates", "Looking for loose ends"),
        Commands::Explore { .. } => ("Explore", "Unfolding the map"),
        Commands::Affected { .. } => ("Affected tests", "Following the test trails"),
        Commands::Preview { .. } => ("Edit preview", "Trying the next shape"),
        Commands::Workspace { .. } => ("Workspace", "Connecting your repositories"),
        Commands::Savings { .. } => ("Savings", "Counting the shortcuts"),
        Commands::Clones { .. } => ("Similar code", "Finding familiar shapes"),
        Commands::Manifest { .. } => ("Manifest", "Packing the essentials"),
        Commands::MapHtml { .. } | Commands::Html { .. } => {
            ("Map visualization", "Giving the graph a view")
        }
        Commands::Freshness { .. } => ("Freshness", "Checking what changed"),
        Commands::Status { .. } => ("Status", "Taking the pulse"),
        Commands::Doctor => ("Diagnostics", "Checking the moving parts"),
        Commands::SessionReport { .. } => ("Session report", "Retracing the session"),
        Commands::Paths { .. } => ("Paths", "Getting our bearings"),
        Commands::History { .. } => ("Build history", "Turning back the pages"),
        Commands::Repair { .. } => ("Store repair", "Mending the map"),
        Commands::Snapshots { .. } => ("Snapshots", "Gathering the snapshots"),
        Commands::Pdg { .. } => ("Data flow", "Following the data"),
        Commands::Cypher { .. } => ("Graph query", "Asking the graph"),
        Commands::Ast { .. } => ("Syntax", "Reading the structure"),
        Commands::Export { .. } => ("Graph export", "Packing the graph"),
        Commands::Routes { .. } => ("Routes", "Following the routes"),
        Commands::ShapeCheck { .. } => ("Shape check", "Checking the fit"),
        Commands::ApiImpact { .. } => ("API impact", "Tracing the API ripples"),
        Commands::Claude { .. } => ("Claude integration", "Checking the connections"),
        Commands::Skills { .. } => ("Skills", "Packing your toolkit"),
        Commands::Integrate { .. } => ("Host integration", "Connecting your tools"),
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
