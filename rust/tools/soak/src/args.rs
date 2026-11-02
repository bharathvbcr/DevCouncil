//! The soak's command line.
//!
//! `soak <workdir> [cycles] [--daemon]` is the contract `verify.sh` depends on
//! (it runs `soak <dir> 5` and reads the verdict line), so that form parses
//! exactly as it always did. The readers mode is a second, disjoint form:
//! `soak <workdir> --readers N [--duration SECS]`.

pub const USAGE: &str =
    "usage: soak <workdir> [cycles] [--daemon] | soak <workdir> --readers N [--duration SECS]";

/// Readers mode bounds: a reader is a process per query, so the count is
/// capped well below anything a laptop could usefully run, and the duration is
/// capped at an hour so a typo cannot start an unbounded run.
pub const MAX_READERS: u64 = 64;
pub const MAX_DURATION_SECS: u64 = 3600;
pub const DEFAULT_DURATION_SECS: u64 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Edit, rebuild, query, restore: `devmap build` per cycle.
    Build,
    /// The same cycles against a long-lived `devmap serve`.
    Daemon,
    /// One writer cycling edit -> incremental build while `readers` threads
    /// query the same store for `duration_secs`.
    Readers { readers: u64, duration_secs: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub root: String,
    /// Cycles for the build and daemon modes. Readers mode runs by duration.
    pub cycles: u64,
    pub mode: Mode,
    /// The label printed on the baseline line: `build`, `--daemon` or
    /// `readers`.
    pub label: String,
}

/// Why the command line was refused: a usage error exits 2, a value that is
/// well-formed but unusable (a zero cycle count) exits 1 with a verdict line.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Usage(String),
    Fail(String),
}

fn usage() -> Refusal {
    Refusal::Usage(USAGE.into())
}

fn bounded_flag(flag: &str, raw: Option<&String>, lo: u64, hi: u64) -> Result<u64, Refusal> {
    let refuse = || Refusal::Usage(format!("{USAGE}; {flag} must be an integer in {lo}..={hi}"));
    let raw = raw.ok_or_else(refuse)?;
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(refuse());
    }
    let value: u64 = raw.parse().map_err(|_| refuse())?;
    if value < lo || value > hi {
        return Err(refuse());
    }
    Ok(value)
}

/// Parse the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Parsed, Refusal> {
    let root = args.first().filter(|r| !r.is_empty()).ok_or_else(usage)?;
    if args.iter().any(|a| a == "--readers" || a == "--duration") {
        return parse_readers(root, &args[1..]);
    }
    if args.len() > 3 {
        return Err(usage());
    }
    let cycles = match args.get(1) {
        None => 40,
        Some(c) => c.parse::<u64>().ok().filter(|c| *c >= 1).ok_or_else(|| {
            Refusal::Fail(format!(
                "SOAK FAIL: cycles must be a positive integer, got {c:?}"
            ))
        })?,
    };
    let label = args.get(2).cloned().unwrap_or_else(|| "build".into());
    let mode = match label.as_str() {
        "build" => Mode::Build,
        "--daemon" => Mode::Daemon,
        other => {
            return Err(Refusal::Usage(format!(
                "usage: soak <workdir> [cycles] [--daemon]; unknown mode {other:?}"
            )))
        }
    };
    Ok(Parsed {
        root: root.clone(),
        cycles,
        mode,
        label,
    })
}

fn parse_readers(root: &str, rest: &[String]) -> Result<Parsed, Refusal> {
    let mut readers = None;
    let mut duration = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--readers" if readers.is_none() => {
                readers = Some(bounded_flag("--readers", rest.get(i + 1), 1, MAX_READERS)?);
                i += 2;
            }
            "--duration" if duration.is_none() => {
                duration = Some(bounded_flag(
                    "--duration",
                    rest.get(i + 1),
                    1,
                    MAX_DURATION_SECS,
                )?);
                i += 2;
            }
            _ => return Err(usage()),
        }
    }
    // `--duration` alone would silently run zero readers: say so.
    let readers = readers.ok_or_else(usage)?;
    Ok(Parsed {
        root: root.to_owned(),
        cycles: 0,
        mode: Mode::Readers {
            readers,
            duration_secs: duration.unwrap_or(DEFAULT_DURATION_SECS),
        },
        label: "readers".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_owned()).collect()
    }

    #[test]
    fn the_verify_sh_form_parses_as_it_always_did() {
        let p = parse(&v(&["dir", "5"])).unwrap();
        assert_eq!(
            (p.root.as_str(), p.cycles, &p.mode, p.label.as_str()),
            ("dir", 5, &Mode::Build, "build")
        );
        let p = parse(&v(&["dir"])).unwrap();
        assert_eq!((p.cycles, p.mode), (40, Mode::Build));
        let p = parse(&v(&["dir", "7", "--daemon"])).unwrap();
        assert_eq!(
            (p.cycles, p.mode, p.label.as_str()),
            (7, Mode::Daemon, "--daemon")
        );
        assert!(parse(&v(&["dir", "7", "build"])).is_ok());
    }

    #[test]
    fn the_old_refusals_are_unchanged() {
        assert!(matches!(parse(&v(&[])), Err(Refusal::Usage(_))));
        assert!(matches!(
            parse(&v(&["dir", "1", "build", "x"])),
            Err(Refusal::Usage(_))
        ));
        assert!(matches!(parse(&v(&["dir", "0"])), Err(Refusal::Fail(_))));
        assert!(matches!(parse(&v(&["dir", "abc"])), Err(Refusal::Fail(_))));
        match parse(&v(&["dir", "3", "--weird"])) {
            Err(Refusal::Usage(m)) => assert!(m.contains("unknown mode \"--weird\""), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn readers_mode_takes_both_flags_in_either_order() {
        let p = parse(&v(&["dir", "--readers", "8", "--duration", "600"])).unwrap();
        assert_eq!(
            p.mode,
            Mode::Readers {
                readers: 8,
                duration_secs: 600
            }
        );
        let q = parse(&v(&["dir", "--duration", "30", "--readers", "2"])).unwrap();
        assert_eq!(
            q.mode,
            Mode::Readers {
                readers: 2,
                duration_secs: 30
            }
        );
        assert_eq!(p.label, "readers");
    }

    #[test]
    fn readers_mode_defaults_the_duration_and_requires_readers() {
        let p = parse(&v(&["dir", "--readers", "4"])).unwrap();
        assert_eq!(
            p.mode,
            Mode::Readers {
                readers: 4,
                duration_secs: DEFAULT_DURATION_SECS
            }
        );
        assert!(matches!(
            parse(&v(&["dir", "--duration", "5"])),
            Err(Refusal::Usage(_))
        ));
    }

    #[test]
    fn readers_mode_bounds_every_value_and_refuses_strays() {
        for bad in [
            vec!["dir", "--readers", "0"],
            vec!["dir", "--readers", "65"],
            vec!["dir", "--readers", "x"],
            vec!["dir", "--readers"],
            vec!["dir", "--readers", "2", "--duration", "0"],
            vec!["dir", "--readers", "2", "--duration", "3601"],
            vec!["dir", "--readers", "2", "--readers", "3"],
            vec!["dir", "5", "--readers", "2"],
            vec!["dir", "--readers", "2", "--daemon"],
            vec!["dir", "--readers", "-1"],
        ] {
            assert!(
                matches!(parse(&v(&bad)), Err(Refusal::Usage(_))),
                "{bad:?} was accepted"
            );
        }
        assert!(parse(&v(&["dir", "--readers", "64", "--duration", "3600"])).is_ok());
    }
}
