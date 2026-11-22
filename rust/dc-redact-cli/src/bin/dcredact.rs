//! `dcredact` — see the crate documentation for the contract.

use std::io::Read;
use std::process::ExitCode;

use dc_proc::cli::{collect_args, version_object, wants_version};
use dc_redact_cli::{IDENTITY, MAX_REQUEST_BYTES, MAX_TEXT_BYTES, Request, SCHEMA_VERSION};

const COMPONENT_ID: &str = "dcredact";

fn main() -> ExitCode {
    match run() {
        Ok(json) => {
            dc_proc::respond!("{json}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            // Hand-rolled rather than serialised, because this is the path
            // taken when serialisation itself is what failed.
            dc_proc::respond!(
                "{{\"ok\":false,\"redaction_ran\":false,\"error\":{}}}",
                dc_proc::cli::quote(&message)
            );
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<String, String> {
    let args = collect_args()?;
    if wants_version(&args) {
        return Ok(version_object(
            COMPONENT_ID,
            IDENTITY,
            env!("CARGO_PKG_VERSION"),
            &[],
        ));
    }
    match args.first().map(String::as_str) {
        Some("health") => Ok(format!(
            "{{\"ok\":true,\"id\":\"{COMPONENT_ID}\",\"component\":\"{IDENTITY}\",\"version\":\"{}\",\"schema_version\":{SCHEMA_VERSION},\"limits\":{{\"max_request_bytes\":{MAX_REQUEST_BYTES},\"max_text_bytes\":{MAX_TEXT_BYTES}}}}}",
            env!("CARGO_PKG_VERSION"),
        )),
        Some("redact") | None => redact(),
        Some(other) => Err(format!(
            "unknown command {other:?} (redact, health, --version, -V, -v, version)"
        )),
    }
}

/// Reads one request from stdin, bounded during the read (one byte over, so a
/// truncated request is detected rather than run).
fn read_request() -> Result<String, String> {
    let mut raw = String::new();
    let read = std::io::stdin()
        .take(MAX_REQUEST_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(|err| format!("could not read the request from stdin: {err}"))?;
    if read as u64 > MAX_REQUEST_BYTES {
        return Err(format!(
            "request is larger than the {MAX_REQUEST_BYTES}-byte limit; nothing was redacted"
        ));
    }
    if raw.trim().is_empty() {
        return Err("no request on stdin".to_string());
    }
    Ok(raw)
}

fn redact() -> Result<String, String> {
    let raw = read_request()?;
    let request: Request = serde_json::from_str(&raw)
        .map_err(|err| format!("request is not valid JSON for this schema: {err}"))?;
    let response = dc_redact_cli::redact(&request)?;
    serde_json::to_string(&response)
        .map_err(|err| format!("result could not be rendered as JSON: {err}"))
}
