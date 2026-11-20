//! Ordinary code around a split key must not hide it.
//!
//! An apostrophe in a comment, a lifetime, a literal glued in front of the key
//! and an escaped quote are all common on the line a secret is written on. Two
//! shapes stay accepted misses and are pinned as misses: a stray double quote
//! earlier on the line (it shifts which quotes pair up, and which pairing is
//! right cannot be told from the line), and Rust's `concat!` (a macro call,
//! not a literal run).
use dc_redact::{contains_secret, redact_secrets};

fn aws() -> String {
    ["AKIA", "QJZXVBNMLKPOIUYT"].concat()
}

fn must_find(case: &str, line: &str, tail: &str) {
    assert!(contains_secret(line), "{case}: missed: {line}");
    assert!(
        !redact_secrets(line).contains(tail),
        "{case}: tail survived: {line}"
    );
}

#[test]
fn an_apostrophe_in_prose_before_the_join_does_not_hide_it() {
    let k = aws();
    must_find(
        "comment",
        &format!("// don't leak: k = \"{}\" + \"{}\"", &k[..4], &k[4..]),
        &k[4..],
    );
    must_find(
        "odd tick",
        &format!("let d = '; let k = \"{}\" + \"{}\";", &k[..4], &k[4..]),
        &k[4..],
    );
}

#[test]
fn a_glued_literal_before_the_key_start_does_not_hide_it() {
    let k = aws();
    must_find(
        "hello + AKIA",
        &format!("k = \"hello\" + \"{}\" + \"{}\"", &k[..4], &k[4..]),
        &k[4..],
    );
    must_find(
        "escaped quote lead",
        &format!("k = \"a\\\"b\" + \"{}\" + \"{}\"", &k[..4], &k[4..]),
        &k[4..],
    );
}

#[test]
fn accepted_miss_a_stray_double_quote_earlier_on_the_line() {
    let k = aws();
    let line = format!("\" k = \"{}\" + \"{}\"", &k[..4], &k[4..]);
    assert!(
        !contains_secret(&line),
        "an accepted miss started to match: update the docs"
    );
}

#[test]
fn accepted_miss_rust_concat_macro() {
    let k = aws();
    let line = format!("const K: &str = concat!(\"{}\", \"{}\");", &k[..4], &k[4..]);
    assert!(
        !contains_secret(&line),
        "an accepted miss started to match: update the docs"
    );
}
