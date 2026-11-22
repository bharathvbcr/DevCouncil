//! `dcredact` — credential redaction, over a JSON-on-stdio boundary.
//!
//! Same contract as `dcgrep`, `dcstore` and `dcverify`: one JSON request on
//! stdin, exactly one JSON object on stdout, exit 0 on success and exit 2 with
//! `{"ok":false,"error":…}` on anything it could not do. The Go host and
//! other apps (Manvi) reach `dc-redact` by process, so a crash here cannot take
//! the caller with it.
//!
//! Fail closed. A request this binary could not redact is never answered with
//! the text it was given: an oversize or malformed request is an error, and
//! every success carries `redaction_ran: true` so a caller can tell "redacted
//! and nothing was found" from "never examined". The redacted text is
//! re-scanned before it is returned, and a result that still trips the gate is
//! refused rather than handed back.

use serde::{Deserialize, Serialize};

/// Bumped when a field changes meaning. Reported by `health`.
pub const SCHEMA_VERSION: u32 = 1;

/// The library this binary fronts, as every component reports it.
pub const IDENTITY: &str = "dc-redact";

/// Largest request read from stdin. Bounded during the read, with one byte
/// over taken so the cap can be detected rather than silently truncating a
/// request into a shorter, still-valid one.
pub const MAX_REQUEST_BYTES: u64 = 8 * 1024 * 1024;

/// Largest `text` accepted. Smaller than the request bound because JSON
/// escaping can multiply a text's size by up to six.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// What a caller sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The text to redact.
    pub text: String,
}

/// What a successful redaction answers.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Response {
    pub ok: bool,
    pub id: &'static str,
    pub component: &'static str,
    pub schema_version: u32,
    /// Always true in a success: the text was examined and this is the result.
    pub redaction_ran: bool,
    /// `dc_redact::redact_secrets(text)`, byte for byte.
    pub text: String,
    /// Lines that carried a credential and were rewritten; one line with two
    /// keys counts once. See `redacted_spans` for the exact replacement count.
    pub redacted_lines: usize,
    /// Credential spans replaced, exactly: a line with two keys counts two.
    pub redacted_spans: usize,
    /// Every vendor shape a detection matched, sorted and de-duplicated.
    pub kinds: Vec<String>,
    pub input_bytes: usize,
    pub output_bytes: usize,
}

/// Redacts `text`, or says why it could not.
pub fn redact(request: &Request) -> Result<Response, String> {
    let text = request.text.as_str();
    if text.len() > MAX_TEXT_BYTES {
        return Err(format!(
            "text is {} bytes, larger than the {MAX_TEXT_BYTES}-byte limit; nothing was redacted",
            text.len()
        ));
    }
    let report = dc_redact::redact_secrets_report(text);
    let redacted = report.text;
    if dc_redact::contains_secret(&redacted) {
        return Err(
            "redaction did not converge: the redacted text still carries a credential, \
             so it was not returned"
                .to_string(),
        );
    }
    let redacted_lines = text
        .split_inclusive('\n')
        .zip(redacted.split_inclusive('\n'))
        .filter(|(before, after)| before != after)
        .count();
    Ok(Response {
        ok: true,
        id: "dcredact",
        component: IDENTITY,
        schema_version: SCHEMA_VERSION,
        redaction_ran: true,
        input_bytes: text.len(),
        output_bytes: redacted.len(),
        text: redacted,
        redacted_lines,
        redacted_spans: report.spans,
        kinds: report.kinds.into_iter().map(str::to_string).collect(),
    })
}
