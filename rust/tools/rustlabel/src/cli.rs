//! The command line: `golabel`'s three flags (`-root`, `-dead`, `-o`) and the
//! bounds this oracle adds. Both `-flag` and `--flag` are accepted.

use std::path::PathBuf;
use std::time::Duration;

use crate::server::ServerConfig;

pub const USAGE: &str = "usage: rustlabel -root <checkout> [-dead <dead.json>] [-o <labels.json>] [-report <report.json>] [-trace <trace.txt>]\n                [-max-sites N] [-deadline-secs S] [-request-secs S] [-load-secs S]\n                 [-target-dir DIR] [-no-references] [-no-build-scripts] [-online]";

#[derive(Debug)]
pub struct Options {
    pub root: PathBuf,
    pub dead: Option<PathBuf>,
    pub out: Option<PathBuf>,
    pub report: Option<PathBuf>,
    /// Where to write the per-site trace; setting it turns tracing on.
    pub trace: Option<PathBuf>,
    pub config: ServerConfig,
}

fn number(flag: &str, value: Option<&String>) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("{flag} needs a value"))?;
    match value.parse::<u64>() {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("{flag} wants a positive integer, got {value:?}")),
    }
}

/// Parse `args` (without the program name). `Err` is a usage error.
pub fn parse(args: &[String]) -> Result<Options, String> {
    let mut config = ServerConfig::default();
    let (mut root, mut dead, mut out, mut report, mut trace) = (None, None, None, None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let flag = arg.trim_start_matches('-');
        if flag.len() == arg.len() || arg.starts_with("---") {
            return Err(format!("unexpected argument {arg:?}"));
        }
        let mut value = || it.next().cloned();
        match flag {
            "root" => root = value().map(PathBuf::from),
            "dead" => dead = value().map(PathBuf::from),
            "o" => out = value().map(PathBuf::from),
            "report" => report = value().map(PathBuf::from),
            "trace" => trace = value().map(PathBuf::from),
            "target-dir" => config.target_dir = value().map(PathBuf::from),
            "max-sites" => config.max_sites = number(arg, value().as_ref())? as usize,
            "deadline-secs" => {
                config.overall_budget = Duration::from_secs(number(arg, value().as_ref())?)
            }
            "request-secs" => {
                config.request_budget = Duration::from_secs(number(arg, value().as_ref())?)
            }
            "load-secs" => config.load_budget = Duration::from_secs(number(arg, value().as_ref())?),
            "no-references" => config.references = false,
            "no-build-scripts" => config.build_scripts = false,
            "online" => config.offline = false,
            _ => return Err(format!("unknown flag {arg:?}")),
        }
    }
    let root = root.ok_or("-root is required")?;
    config.trace = trace.is_some();
    Ok(Options {
        root,
        dead,
        out,
        report,
        trace,
        config,
    })
}
