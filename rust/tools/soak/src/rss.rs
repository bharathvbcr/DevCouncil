//! Peak resident-set measurement through `/usr/bin/time`.
//!
//! Peak RSS is measured, not assumed. A 10.45 GiB peak on a 4.7k-file
//! repository survived every green run of the gate suite because nothing
//! measured memory. A check that cannot run must never report what a check
//! that ran and passed reports, so an unavailable reading is a failure.
//!
//! `/usr/bin/time` differs between the two platforms the gates run on:
//! BSD/macOS spells the verbose flag `-l` and reports bytes, GNU spells it
//! `-v` and reports kibibytes. The flag is probed once against `true` rather
//! than by retrying the real command, which would run the measured build
//! twice. `tools/peak_rss.sh` is the same probe for the shell gates.

use std::fs::File;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const TIME: &str = "/usr/bin/time";

/// The verbose flag this platform's `/usr/bin/time` accepts, if any.
pub fn time_flag() -> Option<&'static str> {
    if !Path::new(TIME).exists() {
        return None;
    }
    ["-l", "-v"].into_iter().find(|flag| {
        Command::new(TIME)
            .args([*flag, "true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Peak RSS in bytes from a `/usr/bin/time` report: BSD's
/// `<bytes>  maximum resident set size`, else GNU's
/// `Maximum resident set size (kbytes): <KiB>`.
pub fn parse_peak(report: &str) -> Option<u64> {
    if let Some(line) = report
        .lines()
        .find(|l| l.contains("maximum resident set size"))
    {
        return line.split_whitespace().next()?.parse().ok();
    }
    let line = report
        .lines()
        .find(|l| l.contains("Maximum resident set size"))?;
    let (_, kib) = line.split_once(": ")?;
    kib.trim().parse::<u64>().ok().map(|k| k * 1024)
}

/// One measured run: the peak RSS and the wall time of the whole subprocess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measured {
    pub peak: u64,
    /// Parent-side wall time from spawn to exit, so it includes
    /// `/usr/bin/time`'s own exec (a few milliseconds) and the measured
    /// command's start-up, its writer-lock wait and its teardown.
    pub wall: Duration,
}

/// Run `program args…` under `/usr/bin/time`, its report written to `log`,
/// and return the peak RSS in bytes. A failed command or an unreadable report
/// is an error.
pub fn measure(flag: &str, log: &Path, program: &Path, args: &[&str]) -> io::Result<u64> {
    measure_timed(flag, log, program, args).map(|m| m.peak)
}

/// The longest any measured build may run. A wedged build (an orphaned
/// writer lock, the extraction retry cap) must end the soak with a failure
/// rather than hang it past its own duration.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(900);

/// [`measure`], and the wall time of the run, bounded by [`BUILD_TIMEOUT`].
/// On a failed command the report stays in `log` for the caller to read.
pub fn measure_timed(
    flag: &str,
    log: &Path,
    program: &Path,
    args: &[&str],
) -> io::Result<Measured> {
    measure_bounded(flag, log, program, args, BUILD_TIMEOUT)
}

/// [`measure_timed`] with an explicit bound. The command leads a process
/// group of its own, so on timeout the whole group (`/usr/bin/time` and the
/// build under it) is killed, not just the wrapper: a killed wrapper would
/// leave the build running and holding the writer lock.
pub fn measure_bounded(
    flag: &str,
    log: &Path,
    program: &Path,
    args: &[&str],
    limit: Duration,
) -> io::Result<Measured> {
    // The exit is timed where it happens: a thread blocks in `wait()` and
    // stamps the instant it returns, while this thread only enforces the
    // deadline. A poll loop would round every reading up by up to its
    // interval, against a 120 ms p95 target.
    let started = Instant::now();
    let mut command = Command::new(TIME);
    command
        .arg(flag)
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(log)?);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command.spawn()?;
    let leader = child.id();
    let (done, exited) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let status = child.wait();
        let _ = done.send((status, Instant::now()));
    });
    let outcome = match exited.recv_timeout(limit.saturating_sub(started.elapsed())) {
        Ok(outcome) => Ok(outcome),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            #[cfg(unix)]
            let _ = dc_proc::signal_group(leader, dc_proc::Signal::Kill);
            let _ = dc_proc::signal_process(leader, dc_proc::Signal::Kill);
            // The kill ends the wait, so the join is prompt.
            let _ = waiter.join();
            return Err(io::Error::other(format!(
                "{} {} timed out after {limit:?} and was killed",
                program.display(),
                args.join(" ")
            )));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(io::Error::other("the build's wait thread died"))
        }
    };
    let _ = waiter.join();
    let (status, exited_at) = outcome?;
    let status = status?;
    let wall = exited_at.saturating_duration_since(started);
    if !status.success() {
        return Err(io::Error::other(format!(
            "{} {} exited with {status}",
            program.display(),
            args.join(" ")
        )));
    }
    let report = std::fs::read_to_string(log)?;
    let peak = parse_peak(&report)
        .ok_or_else(|| io::Error::other(format!("no peak RSS in {}", log.display())))?;
    Ok(Measured { peak, wall })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bsd_reports_bytes() {
        let report = "        0.01 real         0.00 user\n  1630208  maximum resident set size\n";
        assert_eq!(parse_peak(report), Some(1_630_208));
    }

    #[test]
    fn gnu_reports_kibibytes() {
        let report =
            "\tCommand being timed: \"true\"\n\tMaximum resident set size (kbytes): 1536\n";
        assert_eq!(parse_peak(report), Some(1536 * 1024));
    }

    #[test]
    fn a_report_without_the_line_is_no_reading() {
        assert_eq!(parse_peak("real 0.01\nuser 0.00\n"), None);
        assert_eq!(parse_peak("  x  maximum resident set size\n"), None);
    }

    /// The wall time of a command that exits at once is the cost of starting
    /// it, a few milliseconds. A 20 ms poll rounds every reading up to 20 ms
    /// or more, against a 120 ms p95 target, so no run of the old poll loop
    /// can come in under it. Min-of-N, because one run can land on a loaded
    /// scheduler slice.
    #[cfg(unix)]
    #[test]
    fn wall_time_is_not_rounded_up_to_the_poll_interval() {
        let flag = time_flag().expect("/usr/bin/time with -l or -v is required by the soak");
        let log = std::env::temp_dir().join(format!("soak-rss-wall-{}.log", std::process::id()));
        let walls: Vec<Duration> = (0..15)
            .map(|_| {
                measure_timed(flag, &log, Path::new("/bin/sh"), &["-c", ":"])
                    .expect("measured")
                    .wall
            })
            .collect();
        let fastest = walls.iter().min().copied().expect("15 readings");
        assert!(
            fastest < Duration::from_millis(15),
            "fastest of 15 instant commands took {fastest:?}: {walls:?}"
        );
        let _ = std::fs::remove_file(&log);
    }

    #[cfg(unix)]
    #[test]
    fn this_hosts_time_measures_a_real_process() {
        let flag = time_flag().expect("/usr/bin/time with -l or -v is required by the soak");
        let log = std::env::temp_dir().join(format!("soak-rss-{}.log", std::process::id()));
        let peak = measure(flag, &log, Path::new("/bin/sh"), &["-c", ":"]).expect("measured");
        assert!(peak > 0);
        let timed = measure_timed(flag, &log, Path::new("/bin/sh"), &["-c", "sleep 0.2"])
            .expect("measured");
        assert!(
            timed.wall >= Duration::from_millis(200) && timed.wall < Duration::from_secs(20),
            "wall {:?}",
            timed.wall
        );
        assert!(measure(flag, &log, Path::new("/bin/sh"), &["-c", "exit 3"]).is_err());
        // A command that outlives its bound is killed, group and all, and is
        // an error: the grandchild `sleep` must not survive the wrapper.
        let pid_file = format!("{}.pid", log.display());
        let script = format!("sleep 31 & echo $! > {pid_file}; wait");
        let started = Instant::now();
        let err = measure_bounded(
            flag,
            &log,
            Path::new("/bin/sh"),
            &["-c", &script],
            Duration::from_millis(500),
        )
        .expect_err("a 500 ms bound on a 31 s command");
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        let pid = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
            .expect("the script recorded its child pid");
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            dc_proc::signal_process(pid, dc_proc::Signal::Probe).is_err(),
            "the killed command's child {pid} is still alive"
        );
        let _ = std::fs::remove_file(&pid_file);
        let _ = std::fs::remove_file(&log);
    }
}
