//! A secret split across quoted literals on one line is one secret.
//!
//! `"AKIA" + "IOSFODNN7EXAMPLE"` and `"sk-proj-" "…"` read as one literal to
//! anyone reading the line, and a vendor floor never saw it whole. The scan reads
//! the line as written first; a joined copy of it counts only for a match that
//! starts in one literal and ends in a later one, so a key that begins in a later
//! literal is still found at its own quote, and a raw string `r#"…"#` is left
//! alone. Cross-line splits and headerless PEM bodies are accepted misses.
//!
//! Public API only. A failure names the input and what came out.

use dc_redact::{contains_secret, find_secret, redact_secrets};

const AKIA_TAIL: &str = "IOSFODNN7EXAMPLE";
const PROJ_PREFIX: &str = "sk-proj-";
const PROJ_TAIL: &str = "Fx7Qa9Lm2Zt4Wc8Rk3Vn6Yb1Hd5Gj0PsFx7Qa9Lm2Zt4Wc8Rk3Vn6Yb1";

/// The head of `text`, so a failure on a ten-thousand-piece line stays readable.
fn short(text: &str) -> String {
    text.chars().take(160).collect()
}

/// The redaction must remove `tail`, leave a line that is no longer a secret,
/// and be stable on a second pass.
fn assert_redacted(case: &str, line: &str, tail: &str) {
    assert!(
        contains_secret(line),
        "{case}: not detected: {}",
        short(line)
    );
    assert!(
        find_secret(line).is_some(),
        "{case}: find_secret missed: {}",
        short(line)
    );
    let out = redact_secrets(line);
    assert!(
        !out.contains(tail),
        "{case}: tail survived: {}",
        short(&out)
    );
    assert!(
        !contains_secret(&out),
        "{case}: output still a secret: {}",
        short(&out)
    );
    assert_eq!(
        redact_secrets(&out),
        out,
        "{case}: not idempotent: {}",
        short(&out)
    );
}

fn assert_untouched(case: &str, line: &str) {
    assert!(
        !contains_secret(line),
        "{case}: false positive: {}",
        short(line)
    );
    assert_eq!(redact_secrets(line), line, "{case}: rewrote the line");
}

#[test]
fn a_key_split_across_literals_is_found_by_every_entry_point() {
    assert_redacted(
        "plus",
        &format!("id = \"{}\" + \"{AKIA_TAIL}\"", "AKIA"),
        AKIA_TAIL,
    );
    assert_redacted(
        "adjacent",
        &format!("K = \"{PROJ_PREFIX}\" \"{PROJ_TAIL}\""),
        PROJ_TAIL,
    );
    assert_redacted(
        "single quotes",
        &format!("id = '{}'+'{}'", "AKIA", AKIA_TAIL),
        AKIA_TAIL,
    );
    assert_redacted(
        "mixed quotes",
        &format!("id = '{}' + \"{}\"", "AKIA", AKIA_TAIL),
        AKIA_TAIL,
    );
    assert_redacted(
        "tab",
        &format!("id = \"{}\"\t\"{}\"", "AKIA", AKIA_TAIL),
        AKIA_TAIL,
    );
    assert_redacted(
        "three pieces",
        &format!("id = \"{}\" + \"IOSFODNN7\" + \"EXAMPLE\"", "AKIA"),
        AKIA_TAIL,
    );
    assert_redacted("password", "password = \"Tr0u\" + \"b4dor&3\"", "b4dor&3");
    // Python prefixes and an unhashed Rust c/cr/br string are ordinary literals.
    assert_redacted(
        "python r prefix",
        &format!("id = r\"{}\" + \"{}\"", "AKIA", AKIA_TAIL),
        AKIA_TAIL,
    );
    assert_redacted(
        "rust cr",
        &format!("id = cr\"{}\" + \"{}\"", "AKIA", AKIA_TAIL),
        AKIA_TAIL,
    );
}

#[test]
fn the_marker_replaces_the_pieces_and_the_quotes_between_them() {
    assert_eq!(
        redact_secrets(&format!(
            "aws_access_key_id = \"{}\" + \"{}\"",
            "AKIA", AKIA_TAIL
        )),
        "aws_access_key_id = \"AKIA… (20 chars)\""
    );
    assert_eq!(
        redact_secrets("password = \"Tr0u\" + \"b4dor&3\""),
        "password = \"… (11 chars)\""
    );
}

/// A literal before the key must not glue onto it: the quote is a boundary.
#[test]
fn a_key_that_starts_in_a_later_literal_is_still_found() {
    for lead in ["hello", "abc", "x1", ""] {
        let line = format!("greet({lead:?} + \"{}{}\")", "AKIA", AKIA_TAIL);
        assert_redacted(&format!("lead {lead:?}"), &line, AKIA_TAIL);
        let line = format!("greet(\"{lead}\" \"{}{}\")", "AKIA", AKIA_TAIL);
        assert_redacted(&format!("adjacent lead {lead:?}"), &line, AKIA_TAIL);
    }
}

#[test]
fn an_empty_literal_in_the_join_changes_nothing() {
    assert_redacted(
        "empty first",
        &format!("k = \"\" + \"{}{}\"", "AKIA", AKIA_TAIL),
        AKIA_TAIL,
    );
    assert_redacted(
        "empty middle",
        &format!("k = \"{}\" + \"\" + \"{}\"", "AKIA", AKIA_TAIL),
        AKIA_TAIL,
    );
}

#[test]
fn a_line_that_is_only_the_marker_stays_clean() {
    assert_untouched("marker", "\"AKIA… (20 chars)\" + \"… (11 chars)\"");
}

#[test]
fn hashed_raw_strings_are_one_literal_and_are_never_joined() {
    for prefix in ["r", "br", "cr", "bcr", "R", "BR", "Cr"] {
        let line = format!("k = {prefix}#\"{}\"# + \"{}\"", "AKIA", AKIA_TAIL);
        assert!(
            !contains_secret(&line),
            "{prefix}#: joined a raw string: {line}"
        );
        let line = format!("k = {prefix}##\"a\"#\"{}\"## \"{}\"", "AKIA", AKIA_TAIL);
        assert!(
            !contains_secret(&line),
            "{prefix}##: joined a raw string: {line}"
        );
    }
    // An unclosed raw string runs to the end of the line and joins nothing.
    assert!(!contains_secret(&format!(
        "k = r#\"{}\" \"{}\"",
        "AKIA", AKIA_TAIL
    )));
}

#[test]
fn format_verbatim_reference_and_operator_forms_are_not_joined() {
    for (case, line) in [
        (
            "python f-string",
            format!("k = f\"{}\" + \"{}\"", "AKIA", AKIA_TAIL),
        ),
        (
            "rust fr string",
            format!("k = fr\"{}\" + \"{}\"", "AKIA", AKIA_TAIL),
        ),
        (
            "c# verbatim",
            format!("k = @\"{}\" + \"{}\"", "AKIA", AKIA_TAIL),
        ),
        (
            "glued identifier",
            format!("k = xr\"{}\" + \"{}\"", "AKIA", AKIA_TAIL),
        ),
        (
            "increment operator",
            format!("k = \"{}\" ++ \"{}\"", "AKIA", AKIA_TAIL),
        ),
        (
            "compound assignment",
            format!("k = \"{}\" += \"{}\"", "AKIA", AKIA_TAIL),
        ),
        (
            "shell reference piece",
            format!("k = \"{}\" \"${{REST}}\"", "AKIA"),
        ),
        (
            "call between",
            format!("k = \"{}\" + f(\"{}\")", "AKIA", AKIA_TAIL),
        ),
    ] {
        assert!(!contains_secret(&line), "{case}: joined: {line}");
    }
}

#[test]
fn a_reference_beside_a_found_secret_keeps_its_line_byte_for_byte() {
    assert_eq!(
        redact_secrets("curl -H \"X-API-Key: 7d9f3e1a2b4c6d8e0f1a3b5c7d9e1f2a\" \"$URL\""),
        "curl -H \"X-API-Key: … (32 chars)\" \"$URL\""
    );
    assert_untouched(
        "bearer reference",
        "curl -H \"Authorization: Bearer ${TOKEN}\" \"$URL\"",
    );
}

#[test]
fn cross_line_splits_and_headerless_pem_stay_accepted_misses() {
    let split = format!("const k = \"{}\" +\n\"{}\"", "AKIA", AKIA_TAIL);
    assert!(
        !contains_secret(&split),
        "a cross-line split is a documented miss"
    );
    let headerless = format!(
        "{}\n{}",
        "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7",
        "x2VtFq0pU3sLk9mZ1cR8dY4nWb6HeJ5aGv7QoTiXu2NyBfDz3MwKr"
    );
    assert!(
        !contains_secret(&headerless),
        "a headerless PEM body is a documented miss"
    );
}

#[test]
fn hostile_quote_soup_does_not_panic_and_keeps_utf8() {
    for line in [
        "\"\"\"\"\"\"\"\"",
        "'\"'\"'\"+'+\"+",
        "r#\"#\"##\"\"#r\"#",
        "\"é\" + \"AKIA\" + \"日本語\" + \"IOSFODNN7EXAMPLE\"",
        "\"\\\" + \"\\",
        "\"a\"\0\"b\"",
        "+ \"+\" + + \"+\"",
    ] {
        let out = redact_secrets(line);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        let _ = (contains_secret(line), find_secret(line));
    }
    let many = format!(
        "k = {}\"{}\"",
        "\"x\" + ".repeat(10_000),
        "AKIAIOSFODNN7EXAMPLE"
    );
    assert!(contains_secret(&many));
    let split_many = format!(
        "k = \"{}\" {}\"{}\"",
        "AKIA",
        "\"\" ".repeat(10_000),
        AKIA_TAIL
    );
    assert_redacted(
        "ten thousand empty literals between pieces",
        &split_many,
        AKIA_TAIL,
    );
}
