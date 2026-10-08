// Every byte this binary puts on stdout goes through `write_stdout`: it ends
// quietly on a closed pipe and escapes control characters on a terminal.
// `print!`/`println!` do neither, and panic on `EPIPE`.
#![deny(clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{CommandFactory, FromArgMatches};
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

static DIAGNOSTICS: std::sync::OnceLock<progress::DiagnosticSink> = std::sync::OnceLock::new();

fn diagnostic(message: std::fmt::Arguments<'_>) {
    if let Some(sink) = DIAGNOSTICS.get() {
        sink.diagnostic(message);
    } else {
        let fallback = progress::Display::new(ProgressMode::Never, false, false);
        fallback.diagnostic(message);
        fallback.finish("");
    }
}

// All one-shot stdout uses this path, including JSON and large exports.
// Windows has no SIGPIPE; a consumer closing its pipe must not panic the CLI.
fn write_stdout(message: std::fmt::Arguments<'_>) {
    if !presentation::write_human(message) {
        write_stdout_raw(message);
    }
}

fn write_stdout_raw(message: std::fmt::Arguments<'_>) {
    use std::io::Write;
    if let Err(error) = std::io::stdout().lock().write_fmt(message) {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        // A diagnostic sink may itself be gone; the failure exit still survives.
        let display = progress::Display::new(ProgressMode::Never, false, false);
        display.diagnostic(format_args!("DevMap could not write stdout: {error}"));
        display.finish("");
        std::process::exit(1);
    }
}

macro_rules! outln {
    ($($argument:tt)*) => {
        $crate::write_stdout(format_args!("{}\n", format_args!($($argument)*)))
    };
}

mod agents;
mod claude;
mod cli;
mod commands;
mod digest_cache;
mod hook;
mod installation;
mod integrate;
mod output;
mod presentation;
mod progress;
mod reporter;
mod session;
mod sha256;
mod skills;

use crate::cli::{validate_limits, version_line, Cli, Commands, ProgressMode};
use crate::reporter::ProgressReporter;

/// One rendering of an error chain for the human line and the JSON line.
///
/// `{:#}` prints every link joined by `: `. The store carries its refusals in
/// a rusqlite variant that displays its boxed error *and* returns it as
/// `source()`, so the chain says the same sentence twice and every store
/// refusal read as two (measured: the index-gate refusal against this
/// repository's store, 2026-09-07). A link whose text equals the one before it
/// adds nothing and is dropped; every other link is kept in order.
fn render_error(error: &anyhow::Error) -> String {
    let mut rendered = String::new();
    let mut previous: Option<String> = None;
    for cause in error.chain() {
        let text = cause.to_string();
        if previous.as_deref() == Some(text.as_str()) {
            continue;
        }
        if !rendered.is_empty() {
            rendered.push_str(": ");
        }
        rendered.push_str(&text);
        previous = Some(text);
    }
    rendered
}

/// The path a root-taking subcommand names must be a directory that exists.
///
/// Measured through the release binary: `manifest <missing path>` created
/// `<missing path>/.devmap/` and wrote the artifacts of the store's *other*
/// repository into it; `routes`, `shape-check` and `api-impact` answered from
/// the store's recorded root and never said the path they were given does not
/// exist; `build <file>` walked the file as an empty tree. A path the caller
/// named and this binary could not examine is not a repository root, and
/// answering — or writing — as if it were is the check that could not run
/// reporting as one that ran. Only [`Cli::root_hint`]'s subcommands carry a
/// root, and the default `.` always exists, so only a path the caller actually
/// spelled can fail here.
fn validate_root(cli: &Cli) -> Result<(), String> {
    let root = cli.root_hint();
    match std::fs::metadata(&root) {
        Ok(metadata) if metadata.is_dir() => refuse_unsafe_index_root(cli),
        Ok(_) => Err(format!(
            "{}: not a directory; the path a subcommand names must be a repository root",
            root.display()
        )),
        Err(error) => Err(format!("{}: {error}", root.display())),
    }
}

fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_unsafe_index_root(root: &Path) -> bool {
    let root = canonical_or_self(root);
    let mut banned = vec![PathBuf::from("/"), std::env::temp_dir()];
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            banned.push(PathBuf::from(home));
        }
    }
    for extra in ["/tmp", "/private/tmp", "/var/tmp"] {
        banned.push(PathBuf::from(extra));
    }
    banned.iter().any(|path| canonical_or_self(path) == root)
}

/// `build` / `manifest` / `serve` must not treat $HOME, `/`, or the temp dir
/// as a repository unless the caller named `--root` or set `DEVMAP_HOME`.
fn refuse_unsafe_index_root(cli: &Cli) -> Result<(), String> {
    let indexes = matches!(
        cli.command,
        Commands::Build(_) | Commands::Manifest(_) | Commands::Serve(_)
    );
    if !indexes {
        return Ok(());
    }
    if cli.root.is_some() {
        return Ok(());
    }
    if std::env::var_os("DEVMAP_HOME").is_some_and(|value| !value.is_empty()) {
        return Ok(());
    }
    let root = canonical_or_self(&cli.root_hint());
    if is_unsafe_index_root(&root) {
        return Err(format!(
            "refusing to index {}: this path is $HOME, `/`, or the temp directory, not a git \
worktree. Pass `--root <repository>` to name one, or set DEVMAP_HOME if this really is the \
state directory you want.",
            root.display()
        ));
    }
    Ok(())
}

/// Let a one-shot command end the way every other CLI does when its reader
/// goes away.
///
/// Rust's runtime ignores `SIGPIPE` at startup so that a write to a closed
/// pipe surfaces as `EPIPE` — and `println!` answers `EPIPE` with a panic.
/// `devmap export -o - | head` therefore printed `failed printing to stdout:
/// Broken pipe` and a backtrace hint, where `git`, `sqlite3` and `rg` end
/// silently. Restoring the default disposition for one-shot commands makes
/// the kernel behave like them: the process is terminated by the signal the
/// moment the reader is gone, with nothing written after it and nothing left
/// half-done that a later run cannot recover (a build killed at any point
/// leaves the store consistent — `test_process_recovery` and the crash gate
/// are the evidence). Only one-shot commands: see [`Commands::serves`].
///
#[cfg(unix)]
fn restore_default_sigpipe() {
    // SAFETY: `signal(2)` with `SIG_DFL` installs the default action for a
    // signal this process is not otherwise handling; it is called once on
    // the entry thread before a one-shot command writes its result.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_default_sigpipe() {}

/// Clap's generated command builder reserves nearly 1 MiB in debug builds.
/// Both argument parsing and capability introspection use it, so both need a
/// bounded larger stack than Windows' executable entry stack. Keep command I/O
/// on the entry thread to preserve Unix's synchronous SIGPIPE behavior.
fn on_command_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let thread = std::thread::Builder::new()
        .name("devmap-arguments".into())
        .stack_size(8 * 1024 * 1024)
        .spawn(work)
        .unwrap_or_else(|error| {
            diagnostic(format_args!(
                "DevMap could not start command introspection: {error}"
            ));
            std::process::exit(1);
        });
    thread
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Held to the end of `main`: a detached hook build on Windows removes its
    // coalescing lock when it finishes (see `hook::spawn_detached_with_cleanup`).
    let _owned_lock = hook::OwnedLock::from_env();
    let started = Instant::now();
    let (cli, matches) = on_command_stack(|| {
        let matches = Cli::command().get_matches();
        let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| {
            // Cursor/Codex treat exit 2 as "block the agent". A clap usage
            // error under `devmap hook` must never surface as 2.
            let hookish = std::env::args().nth(1).as_deref() == Some("hook")
                || matches.subcommand_name() == Some("hook");
            if hookish {
                let _ = error.print();
                std::process::exit(1);
            }
            error.exit()
        });
        (cli, matches)
    });
    if !cli.command.serves() {
        restore_default_sigpipe();
    }
    let progress = matches!(cli.command, Commands::Build(_))
        .then(|| ProgressReporter::new(cli.progress, cli.json, cli.verbose));
    let presentation = presentation::Session::start(&cli);
    let fallback = (progress.is_none() && presentation.is_none())
        .then(|| progress::Display::new(ProgressMode::Never, cli.json, cli.verbose));
    let display = progress
        .as_ref()
        .map(|reporter| &reporter.display)
        .or_else(|| presentation.as_ref().map(|session| &session.display))
        .or(fallback.as_ref())
        .expect("every command has a diagnostic output scope");
    let sink = display.diagnostic_sink();
    if DIAGNOSTICS.set(sink.clone()).is_err() {
        display.diagnostic("DevMap diagnostic output was already initialized");
    }
    // No worker or server request can block on a log write. JSON-RPC stdout
    // remains owned by its transport; presentation owns all ANSI styling.
    let subscriber = FmtSubscriber::builder()
        .with_max_level(if cli.verbose {
            Level::DEBUG
        } else {
            Level::INFO
        })
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    if let Err(error) = tracing::subscriber::set_global_default(subscriber) {
        diagnostic(format_args!("DevMap logging unavailable: {error}"));
    }
    let outcome = match validate_limits(&cli.command).and_then(|()| validate_root(&cli)) {
        Ok(()) => commands::run(&cli, progress.as_ref()).await,
        Err(message) => Err(anyhow::anyhow!(message)),
    };
    match outcome {
        Ok(()) => {
            if let Some(presentation) = &presentation {
                presentation.finish(true);
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            // Capture the open stage before closing it as failed. Do not log
            // query arguments, preview buffers, or environment variables.
            let root = std::path::absolute(cli.root_hint())
                .unwrap_or_else(|_| cli.root_hint().to_path_buf());
            let db = std::path::absolute(cli.db()).unwrap_or_else(|_| cli.db());
            let context = serde_json::json!({
                "command": matches.subcommand_name(),
                "binary_version": version_line(),
                "binary_path": std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned()),
                "pid": std::process::id(),
                "unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis()),
                "root": root.to_string_lossy(),
                "root_path_lossy": root.to_str().is_none(),
                "db_path": db.to_string_lossy(),
                "db_path_lossy": db.to_str().is_none(),
                "elapsed_ms": started.elapsed().as_millis(),
                "stage": progress.as_ref().and_then(|p| p.open.borrow().as_ref().map(|(label, _, _)| label.clone())),
            });
            // Attempt the human line on stderr, where every other diagnostic
            // this binary writes goes. Under `--json`, the same failure *also*
            // goes out as one line of JSON on stdout, because that is what
            // `--json` promises on every exit and the failing paths are the
            // ones a caller most needs to handle: returning the error from
            // `main` left stdout empty, so a caller reading one line and
            // parsing it saw an empty string and could not tell a failure from
            // a command that answered nothing. Two channels, one message —
            // stdout stays exactly one JSON line either way.
            if let Some(progress) = progress.as_ref() {
                progress.close_open_stage(false);
                progress.display.diagnostic(format_args!(
                    "Error: {} — build stopped before completion; DevMap context: {context}",
                    render_error(&error)
                ));
                progress.display.finish("");
                if cli.json {
                    outln!(
                        "{}",
                        serde_json::json!({ "error": render_error(&error), "diagnostic_context": context,
                        "timings": progress.timings_json(), "progress_output": progress.display.output_json() })
                    );
                } else {
                    // stderr may be blocked or gone. Preserve the retained
                    // diagnostic on the plain primary output without waiting
                    // longer on the optional renderer.
                    let receipt = progress.display.output_json();
                    for line in receipt["diagnostics"]["unrendered"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if let Some(line) = line.as_str() {
                            outln!("{line}");
                        }
                    }
                }
            } else {
                display.diagnostic(format_args!(
                    "Error: {}; DevMap context: {context}",
                    render_error(&error)
                ));
                display.finish("");
                if let Some(session) = &presentation {
                    session.finish(false);
                }
                if cli.json {
                    outln!(
                        "{}",
                        serde_json::json!({ "error": render_error(&error), "diagnostic_context": context,
                            "progress_output": display.output_json() })
                    );
                } else {
                    for line in display.output_json()["diagnostics"]["unrendered"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if let Some(line) = line.as_str() {
                            outln!("{line}");
                        }
                    }
                }
            }
            std::process::ExitCode::FAILURE
        }
    }
}
