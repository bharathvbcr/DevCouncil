//! Shared by the tests that write to the stdin of a child that has gone.

use std::os::fd::{AsFd, AsRawFd};
use std::time::{Duration, Instant};

/// Wait until the kernel reports that nothing holds the read end of the pipe
/// `writer` writes to, so the next write is certain to meet a closed pipe.
///
/// Reaping the child is not that moment. On macOS, the read end of a reaped
/// `/bin/sh` child was seen open for up to 14 ms after the reap, even when the
/// child had closed its stdin first. `/bin/sh` is a trampoline that `execv`s the
/// selected shell, as `/usr/bin/env bash` is. That a second `exec` is the cause is
/// inferred from the controls below; the kernel mechanism was not identified. Under load (8
/// processes, 36 CPU burners on 18 cores), writes that these tests assert are
/// `BrokenPipe` succeeded in 62 of 6400 trials for `/bin/sh` and 140 of 6400 for
/// `env bash`. They succeeded in none of 6400 each for `/bin/bash`, `/bin/dash`
/// or `/bin/zsh` started directly, bash run as `sh` or `--posix`, or a child that
/// is this binary. `poll` reports the condition itself, `POLLHUP` on macOS and
/// `POLLERR` on Linux. Waiting for it took the `/bin/sh` count to 0 of 6400, so
/// the wait is for the state the assertion needs, not for time to pass.
///
/// Panics if the reader is still there after `within`: the scenario's premise
/// failed, which is a different finding from the write contract failing.
#[allow(
    unsafe_code,
    reason = "one poll(2) on a descriptor the caller keeps open"
)]
pub fn wait_until_the_reader_is_gone(writer: &impl AsFd, within: Duration) {
    let fd = writer.as_fd().as_raw_fd();
    let deadline = Instant::now() + within;
    loop {
        let mut entry = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one `pollfd` for a descriptor `writer` keeps open for the
        // call; a zero timeout never blocks.
        let ready = unsafe { libc::poll(&mut entry, 1, 0) };
        assert!(
            ready >= 0,
            "poll failed: {}",
            std::io::Error::last_os_error()
        );
        if entry.revents & (libc::POLLHUP | libc::POLLERR) != 0 {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the pipe's reader was still open {within:?} after the child was reaped"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
