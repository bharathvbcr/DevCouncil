//! Google OAuth tokens with nothing in front of them.
//!
//! A labelled token — `access_token=…`, `"refresh_token": "…"`,
//! `Authorization: Bearer …` — is found by its context. A CLI that talks to
//! Google (`gcloud`, `firebase`) can also print one bare: in an error message,
//! a debug line, a URL it logs. GitPulse copies such output into a diagnostics
//! report, and only the vendor prefix can name it there.
//!
//! Every fixture is built from pieces with `concat!`, so no line of this file
//! is a credential to any scanner that reads it, and every body is
//! `FAKE`-marked so it is never mistaken for a real token.

use dc_redact::{contains_secret, find_secret, redact_secrets};

/// An access token whose body uses every character Google's published
/// examples do: letters, digits, `-`, `_` and `.`.
const ACCESS: &str = concat!("ya", "29.", "a0FAKEfake-Token_notReal.0123456789abcdefXYZ");
/// The downscoped shape, `ya29.dr.…`.
const DOWNSCOPED: &str = concat!("ya", "29.", "dr.", "FAKEfake-0123456789abcdefghijKLMN");
/// A refresh token, `1//` and a URL-safe body.
const REFRESH: &str = concat!("1/", "/", "0gFAKEfakeRefresh-Token_notReal0123456789ab");

const ACCESS_NAME: &str = "google oauth access token";
const REFRESH_NAME: &str = "google oauth refresh token";

/// The places a bare token turns up in tool output, none of them labelled.
fn carriers(token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("own line", token.to_string()),
        (
            "json value",
            format!(r#"{{"detail":"{token}","code":401}}"#),
        ),
        ("json array", format!(r#"["{token}"]"#)),
        (
            "url query",
            format!("GET https://example.invalid/cb?state=abc&v={token}&scope=email HTTP/1.1"),
        ),
        (
            "prose",
            format!("Request failed: the value {token} has expired."),
        ),
        ("parenthesised", format!("retrying with ({token}) now")),
        ("after a colon", format!("error: {token}")),
    ]
}

fn assert_redacted(token: &str, name: &str) {
    let mut failures = Vec::new();
    for (place, line) in carriers(token) {
        let found = find_secret(&line).map(|m| m.name);
        let out = redact_secrets(&line);
        // Failure text names the carrier, never the line: interpolating the
        // line would copy the credential into the test log.
        if found != Some(name) {
            failures.push(format!("{place}: found {found:?}"));
        }
        if out.contains(token) {
            failures.push(format!("{place}: the token survived redaction"));
        }
        if !contains_secret(&line) {
            failures.push(format!("{place}: contains_secret said no"));
        }
        // The existing marker: the vendor prefix, then the length.
        let marker = format!(
            "{}… ({} chars)",
            &token[..token.find('.').map_or(3, |i| i + 1)],
            token.len()
        );
        if !out.contains(&marker) {
            failures.push(format!("{place}: no `prefix… (n chars)` marker"));
        }
        if redact_secrets(&out) != out {
            failures.push(format!("{place}: redaction is not idempotent"));
        }
        if contains_secret(&out) {
            failures.push(format!(
                "{place}: the redacted line still reads as a secret"
            ));
        }
    }
    assert!(failures.is_empty(), "{name}: {failures:#?}");
}

#[test]
fn a_bare_google_access_token_is_redacted_wherever_it_appears() {
    assert_redacted(ACCESS, ACCESS_NAME);
}

#[test]
fn a_bare_downscoped_google_access_token_is_redacted() {
    assert_redacted(DOWNSCOPED, ACCESS_NAME);
}

#[test]
fn a_bare_google_refresh_token_is_redacted_wherever_it_appears() {
    assert_redacted(REFRESH, REFRESH_NAME);
}

/// Only the text after the token's run is kept: the rest of the line is
/// diagnostics someone needs to read.
#[test]
fn redaction_stops_at_the_end_of_the_token() {
    let line = format!("GET https://example.invalid/cb?state=abc&v={ACCESS}&scope=email HTTP/1.1");
    let out = redact_secrets(&line);
    assert!(
        out.starts_with("GET https://example.invalid/cb?state=abc&v="),
        "{out}"
    );
    assert!(out.ends_with("&scope=email HTTP/1.1"), "{out}");
}

/// `ya29` only names a Google token at the start of a token, followed by its
/// dot and a body at least as long as the floor. A word that merely contains
/// it is ordinary text and must come back byte for byte.
#[test]
fn text_that_merely_contains_ya29_is_left_alone() {
    let ordinary = [
        concat!("ya", "29"),
        concat!(
            "the ",
            "ya",
            "29 prefix is how Google marks an access token"
        ),
        concat!("ya", "29-is-not-the-prefix-of-anything-at-all-in-this-line"),
        concat!("ya", "29_build_identifier_long_enough_to_pass_any_floor"),
        concat!("ya", "290.", "0123456789abcdefghijklmnopqrstuvwxyz"),
        concat!("YA", "29.", "UPPERCASE0123456789abcdefghijklmnop"),
        // Glued to the word before it: not a token start.
        concat!("ma", "ya", "29.", "abcdefghijklmnopqrstuvwxyz0123456789"),
        concat!("kaya", "29.", "abcdefghijklmnopqrstuvwxyz0123456789"),
        concat!("x_", "ya", "29.", "abcdefghijklmnopqrstuvwxyz0123456789"),
    ];
    for line in ordinary {
        assert_eq!(find_secret(line), None, "{line}");
        assert_eq!(redact_secrets(line), line, "{line}");
    }
}

/// Short and truncated values are handled the way every prefixed family
/// handles them: below the length floor the prefix alone is not a credential,
/// at the floor it is.
#[test]
fn short_and_truncated_ya29_values_follow_the_length_floor() {
    for short in [
        concat!("ya", "29."),
        concat!("ya", "29.txt"),
        concat!("ya", "29.short"),
        concat!("open ", "ya", "29.AHES6ZRN3-… in the console"),
    ] {
        assert_eq!(find_secret(short), None, "{short}");
        assert_eq!(redact_secrets(short), short, "{short}");
    }
    // 29 characters with the prefix: one under the floor.
    let under = concat!("ya", "29.", "abcdefghijklmnopqrstuvwx");
    assert_eq!(under.len(), 29);
    assert_eq!(find_secret(under), None);
    // 30: the floor itself.
    let at = concat!("ya", "29.", "abcdefghijklmnopqrstuvwxy");
    assert_eq!(at.len(), 30);
    assert_eq!(find_secret(at).map(|m| m.name), Some(ACCESS_NAME));
}

/// Google documents no alphabet for an access token, only a size ceiling and
/// examples. The body is therefore left open: a token whose body held a
/// character the pattern refused would fail the body check and pass whole,
/// which is the leak. The price is that a long run beginning `ya29.` at a token
/// start — a dotted name, a path — is redacted too. That is fail-closed, and it
/// is pinned here so a later narrowing is a decision, not an accident.
#[test]
fn a_long_dotted_or_slashed_run_after_ya29_is_redacted_on_purpose() {
    let run = concat!("ya", "29.", "example.com/some/long/path/segment");
    assert_eq!(find_secret(run).map(|m| m.name), Some(ACCESS_NAME));
    assert!(!redact_secrets(run).contains("example.com"));
}

/// A Google OAuth client secret: `GOCSPX-` and a URL-safe body. Real secrets
/// are 35 characters; the fixture is too, `FAKE`-marked.
const CLIENT_SECRET: &str = concat!("GOC", "SPX-", "FAKEfake-Secret_notReal01234");
const CLIENT_SECRET_NAME: &str = "google oauth client secret";

/// `client_secret.json` labels it, but `gcloud` and `firebase` errors, and a
/// pasted `--client-secret` value, carry it bare.
#[test]
fn a_bare_google_client_secret_is_redacted_wherever_it_appears() {
    assert_eq!(CLIENT_SECRET.len(), 35);
    let marker = concat!("GOC", "SPX-… (35 chars)");
    let mut failures = Vec::new();
    for (place, line) in carriers(CLIENT_SECRET) {
        let out = redact_secrets(&line);
        if find_secret(&line).map(|m| m.name) != Some(CLIENT_SECRET_NAME) {
            failures.push(format!("{place}: not named a client secret"));
        }
        if out.contains(CLIENT_SECRET) {
            failures.push(format!("{place}: the secret survived redaction"));
        }
        if !out.contains(marker) {
            failures.push(format!("{place}: no `prefix… (n chars)` marker"));
        }
        if redact_secrets(&out) != out || contains_secret(&out) {
            failures.push(format!("{place}: redaction is not final"));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// Google publishes no length for the body, and scanners disagree (24 and 28
/// characters both appear), so the floor sits under both: a real secret one
/// shape shorter than expected must not pass whole. Under the floor, in a
/// word, or in another case, it is not one.
#[test]
fn the_client_secret_floor_admits_every_published_shape() {
    for body in [
        "FAKEfake-Secret_notReal0",        // 24
        "FAKEfake-Secret_notReal012",      // 26, Google's codelab placeholder length
        "FAKEfake-Secret_notReal01234",    // 28
        "FAKEfake-Secret_notReal01234567", // longer
    ] {
        let secret = format!("{}{body}", concat!("GOC", "SPX-"));
        assert_eq!(
            find_secret(&secret).map(|m| m.name),
            Some(CLIENT_SECRET_NAME),
            "a {}-character body",
            body.len()
        );
    }
    for ordinary in [
        concat!("GOC", "SPX-"),
        concat!("GOC", "SPX-short"),
        concat!("GOC", "SPX-FAKEfake-Secret_notRea"),
        concat!("x", "GOC", "SPX-FAKEfake-Secret_notReal01234"),
        concat!("goc", "spx-FAKEfake-Secret_notReal01234"),
    ] {
        assert_eq!(find_secret(ordinary), None, "{ordinary}");
        assert_eq!(redact_secrets(ordinary), ordinary, "{ordinary}");
    }
}
