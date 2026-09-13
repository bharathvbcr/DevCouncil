//! Bounded, observational build output. The build never waits on terminal I/O.
//! Frames may be dropped under pressure; diagnostics are retained with explicit
//! shown/total accounting in the JSON result when they could not be rendered.

use std::cell::{Cell, RefCell};
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::ProgressMode;
mod terminal;

const TICK: Duration = Duration::from_millis(80);
// Fast builds get a first frame too; only the persistent phase trail waits.
const TRAIL_DELAY: Duration = Duration::from_millis(150);
const FINISH_WAIT: Duration = Duration::from_millis(100);
const DIAGNOSTIC_LIMIT: usize = 64;
const TEXT_LIMIT: usize = 4096;
pub(super) const TOTAL_STAGES: usize = 5;

struct Diagnostic {
    text: String,
    rendered: AtomicBool,
}

enum Event {
    Stage(usize, String),
    Detail(String),
    Files(String, Arc<devmap_extract::progress::FileProgress>),
    Closed(String, f64, bool),
    Line(Arc<Diagnostic>),
    Text(String),
    Finish(String),
}

#[derive(Default)]
struct OutputStats {
    dropped_updates: AtomicUsize,
    failed_writes: AtomicUsize,
    timed_out: AtomicBool,
    diagnostic_total: AtomicUsize,
    diagnostic_rendered: AtomicUsize,
    diagnostics: Mutex<Vec<Arc<Diagnostic>>>,
    error: Mutex<Option<String>>,
}

impl OutputStats {
    fn fail(&self, error: impl std::fmt::Display) {
        self.failed_writes.fetch_add(1, Ordering::Relaxed);
        let mut first = self
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if first.is_none() {
            *first = Some(bounded(&error.to_string()));
        }
    }
}

struct Worker {
    sender: SyncSender<Event>,
    done: Receiver<()>,
    stopped: Arc<AtomicBool>,
}

pub(super) struct Display {
    enabled: bool,
    live: bool,
    verbose: bool,
    worker: RefCell<Option<Worker>>,
    finished: Cell<bool>,
    stats: Arc<OutputStats>,
}

/// Cloneable logging endpoint for worker threads and tracing. It shares the
/// display's bounded queue and delivery receipt; it never owns terminal I/O.
#[derive(Clone)]
pub(super) struct DiagnosticSink {
    sender: Option<SyncSender<Event>>,
    stats: Arc<OutputStats>,
}

impl DiagnosticSink {
    pub(super) fn diagnostic(&self, message: impl std::fmt::Display) {
        self.stats.diagnostic_total.fetch_add(1, Ordering::Relaxed);
        let diagnostic = Arc::new(Diagnostic {
            text: bounded(&message.to_string()),
            rendered: AtomicBool::new(false),
        });
        let mut retained = self
            .stats
            .diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if retained.len() < DIAGNOSTIC_LIMIT {
            retained.push(Arc::clone(&diagnostic));
        }
        drop(retained);
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.try_send(Event::Line(diagnostic)).is_err())
        {
            self.stats.dropped_updates.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl std::io::Write for DiagnosticSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let retained = bytes.len().min(TEXT_LIMIT);
        let text = String::from_utf8_lossy(&bytes[..retained]);
        self.diagnostic(text.trim_end_matches(['\r', '\n']));
        if retained < bytes.len() {
            self.stats.dropped_updates.fetch_add(1, Ordering::Relaxed);
        }
        Ok(bytes.len()) // Optional logging; delivery failures live in the receipt.
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Display {
    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Called after the renderer has stopped and the writer lock is released.
    /// Redirected stdout keeps the compact, unstyled report even with a live stderr.
    pub(super) fn summary(&self, heading: &str, rows: &[String]) {
        let width = terminal_width(false);
        let output = if self.live && std::io::stdout().is_terminal() && width > 1 {
            summary_card(heading, rows, width, color_enabled(), ascii_output())
        } else {
            format!("{}\n", rows.join("\n"))
        };
        super::write_stdout_raw(format_args!("{output}"));
    }
    pub(super) fn new(mode: ProgressMode, json: bool, verbose: bool) -> Self {
        let is_terminal = std::io::stderr().is_terminal();
        let enabled = match mode {
            ProgressMode::Auto => is_terminal && !json,
            ProgressMode::Always => true,
            ProgressMode::Never => false,
        };
        let live = cfg!(unix)
            && enabled
            && is_terminal
            && std::env::var_os("TERM").is_none_or(|t| t != "dumb");
        let color = color_enabled();
        let ascii = ascii_output();
        let stats = Arc::new(OutputStats::default());
        let (sender, receiver) = mpsc::sync_channel(32);
        let (done_tx, done) = mpsc::sync_channel(1);
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stats = Arc::clone(&stats);
        let worker_stop = Arc::clone(&stopped);
        // Opening/reopening an output handle is also outside the build thread.
        // No path in the build can acquire stdio's global output lock.
        let worker = match thread::Builder::new()
            .name("devmap-progress".into())
            .spawn(move || {
                match terminal::Sink::stderr(is_terminal) {
                    Ok(sink) => animate(
                        receiver,
                        sink,
                        live,
                        color,
                        ascii,
                        &worker_stats,
                        &worker_stop,
                    ),
                    Err(error) => worker_stats.fail(error),
                }
                // Sink destruction precedes this notification. There is no join:
                // even a thread delayed after notifying cannot delay the build.
                if done_tx.send(()).is_err() {
                    worker_stop.store(true, Ordering::Release);
                }
            }) {
            Ok(handle) => {
                drop(handle);
                Some(Worker {
                    sender,
                    done,
                    stopped,
                })
            }
            Err(error) => {
                stats.fail(error);
                None
            }
        };
        Self {
            enabled,
            live,
            verbose,
            worker: RefCell::new(worker),
            finished: Cell::new(false),
            stats,
        }
    }

    pub(super) fn files(&self, label: &str, files: Arc<devmap_extract::progress::FileProgress>) {
        if self.enabled {
            self.emit(Event::Files(bounded(label), files));
        }
    }

    pub(super) fn report_loss(&self) {
        let receipt = self.output_json();
        if receipt["incomplete"] == true {
            super::write_stdout(format_args!("  Progress output incomplete ({} dropped updates, {} failed writes; details: --json)\n", receipt["dropped_updates"], receipt["failed_writes"]));
        }
    }

    fn emit(&self, event: Event) {
        if let Some(worker) = self.worker.borrow().as_ref() {
            if worker.sender.try_send(event).is_err() {
                self.stats.dropped_updates.fetch_add(1, Ordering::Relaxed);
            }
        } else {
            self.stats.dropped_updates.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn stage(&self, current: usize, label: &str) {
        if self.enabled {
            self.emit(Event::Stage(current, bounded(label)));
        }
    }
    pub(super) fn detail(&self, label: &str) {
        if self.enabled {
            self.emit(Event::Detail(bounded(label)));
        }
    }
    pub(super) fn closed(&self, label: &str, seconds: f64, succeeded: bool) {
        if self.enabled {
            self.emit(Event::Closed(bounded(label), seconds, succeeded));
        }
    }
    pub(super) fn phase(&self, label: &str, seconds: f64) {
        if self.enabled && self.verbose && !self.live {
            self.emit(Event::Text(format!(
                "      {} (+{})",
                bounded(label),
                duration(seconds)
            )));
        }
    }

    pub(super) fn diagnostic_sink(&self) -> DiagnosticSink {
        DiagnosticSink {
            sender: self
                .worker
                .borrow()
                .as_ref()
                .map(|worker| worker.sender.clone()),
            stats: Arc::clone(&self.stats),
        }
    }
    pub(super) fn diagnostic(&self, message: impl std::fmt::Display) {
        self.diagnostic_sink().diagnostic(message);
    }
    pub(super) fn note(&self, message: impl std::fmt::Display) {
        if self.enabled && self.verbose {
            self.diagnostic(format_args!("      {message}"));
        }
    }

    pub(super) fn finish(&self, message: impl std::fmt::Display) {
        if self.finished.replace(true) {
            return;
        }
        self.emit(Event::Finish(if self.enabled {
            bounded(&message.to_string())
        } else {
            String::new()
        }));
        if let Some(worker) = self.worker.borrow_mut().take() {
            match worker.done.recv_timeout(FINISH_WAIT) {
                Ok(()) => {}
                Err(RecvTimeoutError::Timeout) => {
                    self.stats.timed_out.store(true, Ordering::Release);
                    self.stats
                        .fail("progress output did not drain within 100ms");
                }
                Err(RecvTimeoutError::Disconnected) => self
                    .stats
                    .fail("progress renderer exited before completing"),
            }
            worker.stopped.store(true, Ordering::Release);
        }
    }

    pub(super) fn output_json(&self) -> serde_json::Value {
        let diagnostics = self
            .stats
            .diagnostics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let pending: Vec<_> = diagnostics
            .iter()
            .filter(|entry| !entry.rendered.load(Ordering::Acquire))
            .map(|entry| entry.text.as_str())
            .collect();
        let total = self.stats.diagnostic_total.load(Ordering::Relaxed);
        let rendered = self.stats.diagnostic_rendered.load(Ordering::Acquire);
        let unrendered = total.saturating_sub(rendered);
        let omitted = unrendered.saturating_sub(pending.len());
        let dropped = self.stats.dropped_updates.load(Ordering::Relaxed);
        let failures = self.stats.failed_writes.load(Ordering::Relaxed);
        let error = self
            .stats
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        serde_json::json!({
            "mode": if self.live { "animated" } else if self.enabled { "plain" } else { "disabled" },
            "incomplete": failures > 0 || dropped > 0 || !pending.is_empty() || omitted > 0,
            "dropped_updates": dropped,
            "failed_writes": failures,
            "shutdown_timed_out": self.stats.timed_out.load(Ordering::Acquire),
            "error": *error,
            "diagnostics": { "total": total, "rendered": rendered, "retained": diagnostics.len(), "omitted": omitted, "unrendered_total": unrendered, "unrendered": pending }
        })
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        self.finish("build stopped before completion");
    }
}

fn bounded(text: &str) -> String {
    if text.len() <= TEXT_LIMIT {
        return text.to_string();
    }
    let mut end = TEXT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} [truncated]", &text[..end])
}

pub(super) fn count(value: usize, noun: &str) -> String {
    format!("{value} {noun}{}", if value == 1 { "" } else { "s" })
}

pub(super) fn duration(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{:.0}ms", seconds * 1000.0)
    } else if seconds < 60.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{}m {:02}s", seconds as u64 / 60, seconds as u64 % 60)
    }
}

// Escape controls in repository paths/diagnostics so a newline or ESC cannot
// escape the one-row display. Non-ASCII glyphs are conservatively budgeted at
// two cells; this may leave spare space but never wraps a wide path.
fn ascii_locale(locale: Option<&str>) -> bool {
    locale.is_some_and(|locale| {
        let lower = locale.to_ascii_lowercase();
        !lower.contains("utf-8") && !lower.contains("utf8")
    })
}

pub(super) fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

pub(super) fn ascii_output() -> bool {
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty());
    // Windows byte writes cannot assume a UTF-8 console code page.
    cfg!(windows) || ascii_locale(locale.as_deref())
}

fn ascii_text(text: &str, ascii: bool) -> String {
    if !ascii {
        return text.to_string();
    }
    text.chars()
        .flat_map(|c| {
            if c.is_ascii() {
                c.to_string().chars().collect::<Vec<_>>()
            } else {
                c.escape_unicode().collect()
            }
        })
        .collect()
}

fn draw_due(revealed: bool, since_frame: Duration) -> bool {
    !revealed || since_frame >= TICK
}

fn safe_text(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

fn fit(text: &str, width: usize) -> String {
    let text = safe_text(text);
    let mut used = 0;
    let mut result = String::new();
    for c in text.chars() {
        let cells = if c.is_ascii() { 1 } else { 2 };
        if used + cells > width {
            // Reserve one cell for a truncation marker without splitting UTF-8.
            while used >= width && !result.is_empty() {
                let c = result.pop().expect("nonempty result");
                used -= if c.is_ascii() { 1 } else { 2 };
            }
            if width > 0 {
                result.push('~');
            }
            break;
        }
        result.push(c);
        used += cells;
    }
    result
}

fn cells(text: &str) -> usize {
    text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// Wrap result details instead of truncating counts or coverage disclosures.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in text.split_whitespace() {
        if !row.is_empty() && cells(&row) + 1 + cells(word) > width {
            rows.push(std::mem::take(&mut row));
        }
        if !row.is_empty() {
            row.push(' ');
        }
        for c in word.chars() {
            if cells(&row) + if c.is_ascii() { 1 } else { 2 } > width && !row.is_empty() {
                rows.push(std::mem::take(&mut row));
            }
            row.push(c);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

pub(super) fn accent_numbers(text: &str) -> String {
    let mut output = String::new();
    let mut number = false;
    for c in text.chars() {
        if c.is_ascii_digit() != number {
            number = c.is_ascii_digit();
            output.push_str(if number { "\x1b[1;36m" } else { "\x1b[0m" });
        }
        output.push(c);
    }
    if number {
        output.push_str("\x1b[0m");
    }
    output
}

fn summary_card(heading: &str, rows: &[String], width: usize, color: bool, ascii: bool) -> String {
    if width <= 1 {
        return String::new();
    }
    // Tiny terminals have no room for a gutter or wide glyphs.
    let ascii = ascii || width < 24;
    let gutter = if width < 24 {
        ""
    } else if ascii {
        "  | "
    } else {
        "  │ "
    };
    let available = width.saturating_sub(1).max(1);
    let room = available.saturating_sub(cells(gutter)).max(1);
    let heading = format!("{} devmap / {heading}", if ascii { "*" } else { "◆" });
    let mut output = String::from("\n");
    for (index, text) in std::iter::once(&heading).chain(rows).enumerate() {
        let text = ascii_text(&safe_text(text), ascii);
        for line in wrap(&text, room) {
            if color {
                output.push_str(if index == 0 { "\x1b[1;32m" } else { "\x1b[2m" });
            }
            output.push_str(gutter);
            if color && index > 0 {
                output.push_str("\x1b[0m");
            }
            if color && index > 0 {
                output.push_str(&accent_numbers(&line));
            } else {
                output.push_str(&line);
            }
            if color {
                output.push_str("\x1b[0m");
            }
            output.push('\n');
        }
    }
    output.push('\n');
    output
}

fn terminal_width(_stderr: bool) -> usize {
    #[cfg(unix)]
    {
        let mut size = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let fd = if _stderr {
            libc::STDERR_FILENO
        } else {
            libc::STDOUT_FILENO
        };
        // SAFETY: size is writable and the inherited fd is queried, not owned.
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut size) } == 0 && size.ws_col > 0 {
            return usize::from(size.ws_col);
        }
    }
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(80)
}

struct Frame {
    current: usize,
    label: String,
    detail: String,
    started: Instant,
    files: Option<Arc<devmap_extract::progress::FileProgress>>,
}

fn phase_title(current: usize, label: &str) -> &str {
    match current {
        1 => "Reading the terrain",
        2 if label.ends_with(" unchanged") => "Already mapped",
        2 => "Connecting the dots",
        3 => "Finding the patterns",
        4 => "Saving your map",
        5 => "Packing the essentials",
        _ => label,
    }
}

fn phase_detail(detail: &str) -> &str {
    match detail {
        "reading" => "Reading source files",
        "checking content hashes" => "Spotting what changed",
        "persist:write" => "Writing the graph",
        "persist:prune_generations" => "Tidying older maps",
        "persist:prune_extractions" => "Tidying cached files",
        "persist:vacuum" => "Reclaiming space",
        _ => detail,
    }
}

impl Frame {
    fn render(&self, tick: usize, width: usize, color: bool, ascii: bool) -> String {
        let spinner = if ascii {
            ['|', '/', '-', '\\'][tick % 4]
        } else {
            ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'][tick % 10]
        };
        let current = if self.current == 0 {
            (tick / 2) % TOTAL_STAGES + 1
        } else {
            self.current.clamp(1, TOTAL_STAGES)
        };
        let nodes: Vec<&str> = (1..=TOTAL_STAGES)
            .map(|n| match (ascii, n.cmp(&current)) {
                (true, std::cmp::Ordering::Less) => "o",
                (true, std::cmp::Ordering::Equal) => ["*", "+", "*", "o"][tick % 4],
                (true, std::cmp::Ordering::Greater) => ".",
                (false, std::cmp::Ordering::Less) => "●",
                (false, std::cmp::Ordering::Equal) => ["◉", "◎", "○", "◎"][tick % 4],
                (false, std::cmp::Ordering::Greater) => "·",
            })
            .collect();
        let trail = nodes.join(if ascii { "-" } else { "─" });
        let elapsed = duration(self.started.elapsed().as_secs_f64());
        let stage = if self.current == 0 {
            String::new()
        } else {
            format!("{current}/{TOTAL_STAGES} ")
        };
        let prefix = if width >= 72 {
            format!("{spinner} devmap  {trail}  {stage}")
        } else if width >= 48 {
            format!("{spinner} devmap {stage}")
        } else {
            format!("{spinner} {stage}")
        };
        // Stage numbers describe the pipeline, never a made-up work percentage.
        // An unchanged build's second stage does no resolution at all.
        let title = phase_title(self.current, &self.label);
        let detail = phase_detail(&self.detail);
        let label = if width >= 110 && self.current != 0 {
            format!(
                "{title} / {}",
                if self.detail.is_empty() {
                    &self.label
                } else {
                    detail
                }
            )
        } else if !self.detail.is_empty() {
            detail.to_string()
        } else {
            title.to_string()
        };
        let label = ascii_text(&label, ascii);
        let measured = self.files.as_ref().map(|files| files.snapshot());
        let tail =
            if let Some(files) = measured.filter(|files| files.valid && files.total.is_some()) {
                let mut text = format!("  {}/{}", files.completed, files.total.unwrap_or(0));
                if let Some(percent) = files.percent {
                    text.push_str(&format!(" {percent:.0}%"));
                }
                if width >= 100 {
                    if let Some(rate) = files.files_per_second {
                        text.push_str(&format!(" {rate:.0}/s"));
                    }
                    if let Some(eta) = files.eta_seconds {
                        text.push_str(&format!(" ~{} left", duration(eta)));
                    }
                }
                text.push_str(&format!("  {elapsed}"));
                text
            } else {
                format!("  {elapsed}")
            };
        let available = width.saturating_sub(1); // Avoid the terminal's autowrap column.
        let room = available.saturating_sub(cells(&prefix) + cells(&tail));
        let label = fit(&label, room);
        let line = fit(&format!("{prefix}{label}{tail}"), available);
        if color {
            if cells(&prefix) + cells(&label) + cells(&tail) <= available {
                format!("\x1b[36m{prefix}\x1b[0m{label}\x1b[2m{tail}\x1b[0m")
            } else {
                format!("\x1b[36m{line}\x1b[0m")
            }
        } else {
            line
        }
    }
}

fn animate(
    receiver: Receiver<Event>,
    mut sink: terminal::Sink,
    live: bool,
    color: bool,
    ascii: bool,
    stats: &OutputStats,
    stopped: &AtomicBool,
) {
    let started = Instant::now();
    let mut last_frame = started;
    let mut frame: Option<Frame> = None;
    let mut painted = false;
    let mut revealed = false;
    let mut tick: usize = 0;
    while !stopped.load(Ordering::Acquire) {
        let event = receiver.recv_timeout(TICK);
        if stopped.load(Ordering::Acquire) {
            break;
        }
        let mut line = String::new();
        let mut diagnostic = None;
        let mut finished = false;
        match event {
            Ok(Event::Stage(current, label)) => {
                if !live {
                    line = if current == 0 {
                        format!("[devmap] {label}\n")
                    } else {
                        format!("[{current}/{TOTAL_STAGES}] {label}\n")
                    };
                }
                frame = Some(Frame {
                    current,
                    label,
                    detail: String::new(),
                    started: Instant::now(),
                    files: None,
                });
            }
            Ok(Event::Detail(label)) => {
                if let Some(frame) = frame.as_mut() {
                    frame.detail = label;
                    frame.files = None;
                }
            }
            Ok(Event::Files(label, files)) => {
                if let Some(frame) = frame.as_mut() {
                    frame.detail = label;
                    frame.files = Some(files);
                }
            }
            Ok(Event::Closed(label, seconds, succeeded)) => {
                if !live {
                    line = format!(
                        "      {label} {} {}\n",
                        if succeeded { "took" } else { "failed after" },
                        duration(seconds)
                    );
                } else if revealed && (!succeeded || started.elapsed() >= TRAIL_DELAY) {
                    let current = frame.as_ref().map_or(1, |f| f.current);
                    let label = if succeeded {
                        phase_title(current, &label)
                    } else {
                        &label
                    };
                    line = format!(
                        "  {} [{current}/{TOTAL_STAGES}] {label}{} · {}\n",
                        if succeeded { "✓" } else { "!" },
                        if succeeded { "" } else { " failed" },
                        duration(seconds)
                    );
                }
                frame = None;
            }
            Ok(Event::Line(entry)) => {
                line = format!("{}\n", entry.text);
                diagnostic = Some(entry);
            }
            Ok(Event::Text(text)) => {
                line = format!("{text}\n");
            }
            Ok(Event::Finish(text)) => {
                if !text.is_empty() {
                    line = format!("{text}\n");
                }
                finished = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let draw = live && !finished && frame.is_some() && draw_due(revealed, last_frame.elapsed());
        let clear = painted && (!line.is_empty() || draw || finished || frame.is_none());
        let mut output = String::new();
        if clear {
            output.push_str("\r\x1b[0m\x1b[2K");
        }
        if !line.is_empty() {
            // Keep framing newlines separate from untrusted message text.
            output.push_str(&ascii_text(&safe_text(line.trim_end_matches('\n')), ascii));
            output.push('\n');
        }
        if draw {
            if !clear {
                output.push_str("\r\x1b[2K");
            }
            output.push_str(&frame.as_ref().expect("draw has a frame").render(
                tick,
                terminal_width(true),
                color,
                ascii,
            ));
            last_frame = Instant::now();
            tick = tick.wrapping_add(1);
        }
        if !output.is_empty() {
            match sink.write(output.as_bytes(), stopped) {
                Ok(()) => {
                    if let Some(entry) = diagnostic {
                        entry.rendered.store(true, Ordering::Release);
                        stats.diagnostic_rendered.fetch_add(1, Ordering::Release);
                    }
                    painted = draw;
                    revealed |= draw;
                }
                Err(error) => stats.fail(error),
            }
        }
        if finished {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fit, safe_text, Frame};
    use std::time::Instant;

    #[test]
    fn result_cards_wrap_without_losing_counts_or_terminal_safety() {
        let rows = vec![
            "Built generation #1 · 36 files · 158 symbols · 466 edges · 60ms".into(),
            "Unresolved: 0 unattributed, 2169 uninferred receivers (details: --verbose)".into(),
            "path 世界\n\x1b[31m".into(),
        ];
        for width in [0, 1, 2, 8, 23, 24, 40, 80, 120] {
            for ascii in [false, true] {
                let card = super::summary_card("Map ready", &rows, width, false, ascii);
                for line in card.lines() {
                    assert!(super::cells(line) < width, "width={width}: {line}");
                    assert!(!line.chars().any(char::is_control), "{line}");
                }
                if width > 1 {
                    let rejoined = card
                        .replace(['│', '|'], "")
                        .split_whitespace()
                        .collect::<String>();
                    assert!(rejoined.contains("2169uninferredreceivers"), "{card}");
                    assert!(rejoined.contains("36files"), "{card}");
                    assert!(rejoined.contains("details:--verbose"), "{card}");
                }
                if ascii || width < 24 {
                    assert!(card.is_ascii(), "{card}");
                }
            }
        }
        assert!(super::summary_card("Map ready", &rows, 80, true, false).contains("\x1b[1;32m"));
        let accented = super::accent_numbers("36 files / 2169 receivers");
        assert_eq!(accented.matches("\x1b[1;36m").count(), 2);
        assert_eq!(
            accented.replace("\x1b[1;36m", "").replace("\x1b[0m", ""),
            "36 files / 2169 receivers"
        );
    }

    #[test]
    fn phase_titles_and_node_motion_follow_actual_work() {
        for (stage, label, expected) in [
            (1, "scanning", "Reading the terrain"),
            (2, "resolving 36 files", "Connecting the dots"),
            (2, "36 files unchanged", "Already mapped"),
            (3, "analyzing", "Finding the patterns"),
            (4, "persisting", "Saving your map"),
            (5, "writing consumer artifacts", "Packing the essentials"),
        ] {
            let frame = Frame {
                current: stage,
                label: label.into(),
                detail: String::new(),
                started: Instant::now(),
                files: None,
            };
            let first = frame.render(0, 120, false, false);
            let next = frame.render(1, 120, false, false);
            assert!(first.contains(expected), "{first}");
            assert!(
                first.contains('◉') && next.contains('◎'),
                "{first} / {next}"
            );
            assert!(
                !first.contains('%'),
                "unknown work cannot claim a percentage"
            );
        }
    }

    #[test]
    fn animated_rows_fit_narrow_and_wide_terminals_and_escape_paths() {
        let frame = Frame {
            current: 2,
            label: "extracting 世界\n\x1b[31m".repeat(20),
            detail: "phase".into(),
            started: Instant::now(),
            files: None,
        };
        for width in [0, 1, 8, 20, 40, 80, 120, 300] {
            let line = frame.render(0, width, false, false);
            let cells: usize = line.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
            assert!(cells <= width.saturating_sub(1), "width={width}: {line}");
            assert!(!line.chars().any(char::is_control));
        }
        assert_ne!(
            frame.render(0, 80, false, false),
            frame.render(1, 80, false, false)
        );
        // Rows are sized to their content and nothing pads, but the width they
        // are handed must still be spent. A detail longer than any window here
        // fills each one bar the autowrap column, so a wider terminal buys more
        // of the path rather than leaving the space unused.
        let overflowing = Frame {
            current: 1,
            label: "scanning".into(),
            detail: format!("/var/folders/{}", "deeply/nested/".repeat(30)),
            started: Instant::now(),
            files: None,
        };
        for width in [24, 48, 72, 80, 110, 120, 300] {
            let line = overflowing.render(0, width, false, false);
            let cells: usize = line.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
            assert_eq!(cells, width - 1, "width={width}: {line}");
        }
        assert!(frame.render(0, 80, true, false).contains("\x1b[36m"));
        assert_eq!(safe_text("a\r\nb\x1b"), "a\\r\\nb\\u{1b}");
        assert_eq!(fit("世界x", 4), "世~");
        assert!(frame.render(0, 80, false, true).is_ascii());
        assert!(super::ascii_locale(Some("C")));
        assert!(super::ascii_locale(Some("POSIX")));
        assert!(super::ascii_locale(Some("en_US.ISO-8859-1")));
        assert!(!super::ascii_locale(Some("en_US.UTF-8")));
        assert_eq!(safe_text("a\u{202e}b"), "a\\u{202e}b");
    }

    #[test]
    fn first_frame_is_immediate_but_frame_floods_remain_throttled() {
        use std::time::Duration;
        assert!(super::draw_due(false, Duration::ZERO));
        for milliseconds in 0..80 {
            assert!(!super::draw_due(true, Duration::from_millis(milliseconds)));
        }
        assert!(super::draw_due(true, Duration::from_millis(80)));
    }

    #[test]
    fn a_stalled_renderer_bounds_queue_payloads_retention_and_finish() {
        use super::{Display, OutputStats, Worker};
        use std::cell::{Cell, RefCell};
        use std::sync::{atomic::AtomicBool, mpsc, Arc};
        let (sender, receiver) = mpsc::sync_channel(32);
        let (_done_tx, done) = mpsc::sync_channel(1);
        let display = Display {
            enabled: true,
            live: false,
            verbose: true,
            worker: RefCell::new(Some(Worker {
                sender,
                done,
                stopped: Arc::new(AtomicBool::new(false)),
            })),
            finished: Cell::new(false),
            stats: Arc::new(OutputStats::default()),
        };
        let large = "世界".repeat(3000);
        for _ in 0..100_000 {
            display.detail(&large);
        }
        for _ in 0..1000 {
            display.diagnostic(&large);
        }
        assert_eq!(receiver.try_iter().count(), 32);
        let started = Instant::now();
        display.finish("done");
        assert!(started.elapsed().as_secs_f64() < 2.0);
        let receipt = display.output_json();
        assert_eq!(receipt["shutdown_timed_out"], true);
        assert_eq!(receipt["diagnostics"]["total"], 1000);
        assert_eq!(receipt["diagnostics"]["retained"], 64);
        assert_eq!(receipt["diagnostics"]["omitted"], 936);
        assert_eq!(receipt["diagnostics"]["unrendered_total"], 1000);
        for text in receipt["diagnostics"]["unrendered"].as_array().unwrap() {
            assert!(text.as_str().unwrap().len() <= super::TEXT_LIMIT + 12);
        }
        display.finish("second finish");
        assert_eq!(display.output_json(), receipt);
    }
}
