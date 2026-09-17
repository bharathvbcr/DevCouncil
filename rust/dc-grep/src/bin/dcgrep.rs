//! `dcgrep` — repository search, over a JSON-on-stdio boundary.
//!
//! Same contract as `dcstore` and `dcverify`, for the same reason: the Go
//! execution plane crosses to Rust by process rather than by cgo, so
//! `CGO_ENABLED=0`, simple cross-compilation and the single static binary all
//! survive, and a crash in the search engine cannot take the agent loop with it.
//!
//! It reads one JSON request on stdin and prints one JSON object on stdout.
//! A search it could not run is exit 2 with an error, never an empty match
//! list: an empty list means "this search ran and matched nothing", and no
//! caller may be allowed to read "could not run" as that.

use std::io::Read;
use std::process::ExitCode;

use dc_grep::{IDENTITY, IndexRequest, ListRequest, RankedRequest, Request, SCHEMA_VERSION};

// A disconnected consumer is a transport failure, not a Rust panic. Keep
// diagnostics off this JSON protocol, including the error response path.
macro_rules! respond {
    ($($argument:tt)*) => {{
        use std::io::Write as _;
        let mut out = std::io::stdout().lock();
        if out.write_fmt(format_args!("{}\n", format_args!($($argument)*)))
            .and_then(|()| out.flush()).is_err() {
            return ExitCode::FAILURE;
        }
    }};
}

fn main() -> ExitCode {
    match run() {
        Ok(json) => {
            respond!("{json}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            // Hand-rolled rather than serialised, because this is the path
            // taken when serialisation itself is what failed.
            respond!(
                "{{\"ok\":false,\"error\":{}}}",
                serde_json::to_string(&message)
                    .unwrap_or_else(|_| "\"error message was not renderable\"".to_string())
            );
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<String, String> {
    let args = collect_args()?;
    match args.first().map(String::as_str) {
        // `CARGO_PKG_VERSION` and never a literal: every crate in this
        // workspace takes `version.workspace = true`, so the one number in
        // `rust/Cargo.toml` reaches here and a release has no second place to
        // forget. Until this arm existed the binary answered `--version` with
        // `unknown command "--version"`, so a deployed searcher's version was
        // not observable at all — the package said 0.2.3 and the program could
        // not be asked.
        Some("--version") => Ok(format!(
            "{{\"ok\":true,\"component\":\"{IDENTITY}\",\"version\":\"{}\"}}",
            env!("CARGO_PKG_VERSION")
        )),
        // The ranked fields are what this build *can* read, taken from the one
        // list next to the enum rather than written out here. They were single
        // literals saying "bm25" and "code-v1", which stopped being true the
        // moment a learned index could be published: health would have denied
        // a capability the same binary was exercising.
        Some("health") => Ok(format!(
            "{{\"ok\":true,\"searcher\":\"{IDENTITY}\",\"schema_version\":{SCHEMA_VERSION},\
             \"engine\":\"ripgrep\",\"index_engine\":\"tgrep-core\",\
             \"ranked_engines\":{},\"ranked_vocabularies\":{}}}",
            render(&dc_grep::ranked_engines())?,
            render(&dc_grep::ranked_vocabularies())?,
        )),
        Some("search") | None => search(),
        Some("files") => list(),
        Some("index") => index(),
        Some("rank") => rank(),
        Some(other) => Err(format!(
            "unknown command {other:?} (search, files, index, rank, health, --version)"
        )),
    }
}

/// Reads one request from stdin, bounded.
///
/// Bounded during the read rather than checked after it. An unbounded
/// `read_to_string` let a 64 MiB request produce a 75 MiB resident set before
/// anything looked at it, and nothing on this side decides how much a caller
/// may send.
///
/// One byte over the limit is taken so the cap can be *detected*: a reader
/// stopped exactly at the limit cannot tell a request that fit from one that
/// was truncated, and a truncated request is very likely still valid JSON with
/// a shorter pattern in it — which would run a search nobody asked for.
fn read_request() -> Result<String, String> {
    let mut raw = String::new();
    let read = std::io::stdin()
        .take(dc_grep::MAX_REQUEST_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(|err| format!("could not read the request from stdin: {err}"))?;
    if read as u64 > dc_grep::MAX_REQUEST_BYTES {
        return Err(format!(
            "request is larger than the {}-byte limit",
            dc_grep::MAX_REQUEST_BYTES
        ));
    }
    if raw.trim().is_empty() {
        return Err("no request on stdin".to_string());
    }
    Ok(raw)
}

fn search() -> Result<String, String> {
    let raw = read_request()?;
    let request: Request = serde_json::from_str(&raw)
        .map_err(|err| format!("request is not valid JSON for this schema: {err}"))?;
    let response = dc_grep::search(&request)?;
    serde_json::to_string(&response)
        .map_err(|err| format!("result could not be rendered as JSON: {err}"))
}

/// Lists the files a search would open.
///
/// It shares `read_request` with `search` rather than repeating the bound,
/// because a cap that two call sites each implement is a cap one of them will
/// eventually be missing.
fn list() -> Result<String, String> {
    let raw = read_request()?;
    let request: ListRequest = serde_json::from_str(&raw)
        .map_err(|err| format!("request is not valid JSON for this schema: {err}"))?;
    let response = dc_grep::list_files(&request)?;
    serde_json::to_string(&response)
        .map_err(|err| format!("result could not be rendered as JSON: {err}"))
}

/// Ranks files by how much they are about the query, rather than by whether
/// they contain it.
///
/// A separate command from `search` and not a mode of it, because the two
/// answer different questions and return different shapes: `search` returns
/// lines that definitely contain something, `rank` returns files that are
/// probably about something. Folding them into one reply would give every
/// caller a field that is meaningful in one mode and absent in the other,
/// which is the shape a caller eventually reads without checking which mode
/// produced it.
fn rank() -> Result<String, String> {
    let raw = read_request()?;
    let request: RankedRequest = serde_json::from_str(&raw)
        .map_err(|err| format!("request is not valid JSON for this schema: {err}"))?;
    let response = dc_grep::ranked_search(&request)?;
    serde_json::to_string(&response)
        .map_err(|err| format!("result could not be rendered as JSON: {err}"))
}

/// Renders a value as JSON, so the health line's lists are escaped by the same
/// serialiser every other response goes through rather than by concatenation.
fn render<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value)
        .map_err(|err| format!("result could not be rendered as JSON: {err}"))
}

fn index() -> Result<String, String> {
    let raw = read_request()?;
    let request: IndexRequest = serde_json::from_str(&raw)
        .map_err(|err| format!("request is not valid JSON for this schema: {err}"))?;
    let response = dc_grep::build_index(&request)?;
    serde_json::to_string(&response)
        .map_err(|err| format!("result could not be rendered as JSON: {err}"))
}

/// Collects the command line, refusing an argument that is not valid Unicode
/// rather than dying on it.
///
/// `std::env::args()` panics on such an argument: exit 101, a backtrace on
/// stderr and nothing on stdout, which the Go client can only report as a
/// search that failed with no reason. A search that could not run must stay
/// distinguishable from one that ran.
fn collect_args() -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    for (index, arg) in std::env::args_os().skip(1).enumerate() {
        match arg.into_string() {
            Ok(value) => args.push(value),
            Err(_) => return Err(format!("argument {index} is not valid UTF-8")),
        }
    }
    Ok(args)
}
