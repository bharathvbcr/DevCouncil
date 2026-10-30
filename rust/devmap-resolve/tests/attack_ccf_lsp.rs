//! Adversarial round on the stderr tail of a failed language server (lane
//! compiler-cheap-fixes, dac91244).
//!
//! Servers are `sh -c <script>` stand-ins. Unix only, and `perl` (which ships
//! with macOS and every CI image here) stands in for a descendant that leaves
//! the server's process group.
#![cfg(unix)]

use std::time::{Duration, Instant};

use devmap_extract::subprocess::Resolved;
use devmap_resolve::lsp::{LspDidNotRun, LspServerSpec, LspSession, LspSessionOptions};

fn start(script: &str, deadline: Duration) -> (Result<LspSession, LspDidNotRun>, Duration) {
    let script: &'static str = Box::leak(script.to_string().into_boxed_str());
    let args: &'static [&'static str] = Box::leak(Box::new(["-c", script]));
    let spec = LspServerSpec {
        binary: "sh",
        args,
        extensions: &[".rs"],
    };
    let started = Instant::now();
    let result = LspSession::start(
        &spec,
        &Resolved::as_given("sh"),
        &std::env::temp_dir(),
        started + deadline,
        &LspSessionOptions::default(),
    );
    (result, started.elapsed())
}

fn exit_report(result: Result<LspSession, LspDidNotRun>) -> LspDidNotRun {
    match result {
        Err(reason) => reason,
        Ok(_) => panic!("a server that exits at start-up started"),
    }
}

/// The failed server's descendant left the group and still holds stderr: the
/// caller's own deadline is 2 s, and the readout of a failed start must not
/// take 5 s on top of it.
#[test]
fn a_failed_start_returns_within_the_callers_deadline_when_a_descendant_holds_stderr() {
    // `perl` becomes a session leader (outside the server's group), keeps
    // stderr, and sleeps 8 s so it ends on its own. Its stdout is closed so
    // the server's stdout reaches EOF when `sh` exits.
    let script = "perl -e 'use POSIX; POSIX::setsid(); exec \"sleep\", \"8\"' >/dev/null &\nsleep 1\nexit 3\n";
    let (result, elapsed) = start(script, Duration::from_secs(2));
    let reason = exit_report(result);
    eprintln!("elapsed={elapsed:?} reason={reason:?}");
    assert!(
        matches!(reason, LspDidNotRun::NonZeroExit { .. }),
        "{reason:?}"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "start() was given 2 s and took {elapsed:?}; {reason:?}"
    );
}

/// A server that wrote 20 KB on one line and one that wrote nothing must not
/// produce the same report: the first said something and the readout threw it
/// away without a word.
#[test]
fn a_server_that_wrote_a_long_line_is_not_reported_as_one_that_said_nothing() {
    let quiet = exit_report(start("exit 3\n", Duration::from_secs(10)).0);
    let noisy = exit_report(
        start(
            "head -c 20000 /dev/zero | tr '\\0' x >&2\nexit 3\n",
            Duration::from_secs(10),
        )
        .0,
    );
    assert_ne!(
        format!("{quiet:?}"),
        format!("{noisy:?}"),
        "20000 bytes of stderr left no trace in the outcome"
    );
}

/// The last line of a tail is shown even with no trailing newline, and a tail
/// whose first line was cut does not show the cut line.
#[test]
fn a_tail_without_a_trailing_newline_keeps_its_last_line() {
    let noisy = exit_report(
        start(
            "printf 'first line\\nfatal: no toolchain' >&2\nexit 3\n",
            Duration::from_secs(10),
        )
        .0,
    );
    match noisy {
        LspDidNotRun::NonZeroExit {
            stderr: Some(text),
            stderr_complete: true,
            ..
        } => assert!(text.contains("fatal: no toolchain"), "{text}"),
        other => panic!("{other:?}"),
    }
}
