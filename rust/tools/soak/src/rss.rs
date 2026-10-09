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

/// Run `program args…` under `/usr/bin/time`, its report written to `log`,
/// and return the peak RSS in bytes. A failed command or an unreadable report
/// is an error.
pub fn measure(flag: &str, log: &Path, program: &Path, args: &[&str]) -> io::Result<u64> {
    let status = Command::new(TIME)
        .arg(flag)
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(log)?)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "{} {} exited with {status}",
            program.display(),
            args.join(" ")
        )));
    }
    let report = std::fs::read_to_string(log)?;
    parse_peak(&report).ok_or_else(|| io::Error::other(format!("no peak RSS in {}", log.display())))
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

    #[cfg(unix)]
    #[test]
    fn this_hosts_time_measures_a_real_process() {
        let flag = time_flag().expect("/usr/bin/time with -l or -v is required by the soak");
        let log = std::env::temp_dir().join(format!("soak-rss-{}.log", std::process::id()));
        let peak = measure(flag, &log, Path::new("/bin/sh"), &["-c", ":"]).expect("measured");
        assert!(peak > 0);
        assert!(measure(flag, &log, Path::new("/bin/sh"), &["-c", "exit 3"]).is_err());
        let _ = std::fs::remove_file(&log);
    }
}
