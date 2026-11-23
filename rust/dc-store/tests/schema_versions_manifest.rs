//! `protocols/schema-versions/v1.json` is the machine-readable copy of the schema numbers
//! this crate owns; consumers read it. It must equal the crate's own constants.
//!
//! dc-store has no JSON dependency, so the integer under a key inside a named
//! object is read with a small std-only scan; the manifest is flat and ours.

use dc_store::{SCHEMA_VERSION, workbench::WORKBENCH_SCHEMA};

fn number(text: &str, group: &str, key: &str) -> i64 {
    let at = text
        .find(&format!("\"{group}\""))
        .unwrap_or_else(|| panic!("protocols/schema-versions/v1.json: object {group} is missing"));
    let body = &text[at..];
    let end = body.find('}').unwrap_or(body.len());
    let body = &body[..end];
    let k = body
        .find(&format!("\"{key}\""))
        .unwrap_or_else(|| panic!("protocols/schema-versions/v1.json: {group}.{key} is missing"));
    let rest = body[k + key.len() + 2..]
        .trim_start()
        .strip_prefix(':')
        .unwrap_or_else(|| panic!("protocols/schema-versions/v1.json: {group}.{key} has no value"));
    let rest = rest.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let next = rest[digits.len()..].trim_start().chars().next();
    // Digits alone are not an integer value: "11.5" and "11x" must not read as 11.
    if digits.is_empty() || !matches!(next, Some(',' | '}') | None) {
        panic!("protocols/schema-versions/v1.json: {group}.{key} is not an integer");
    }
    digits.parse().unwrap_or_else(|_| {
        panic!("protocols/schema-versions/v1.json: {group}.{key} is not an integer")
    })
}

#[test]
fn manifest_matches_dc_store_constants() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocols/schema-versions/v1.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    assert_eq!(
        number(&text, "dc_store", "schema_version"),
        i64::from(SCHEMA_VERSION),
        "protocols/schema-versions/v1.json dc_store.schema_version disagrees with SCHEMA_VERSION"
    );
    assert_eq!(
        number(&text, "dc_store", "workbench_schema"),
        WORKBENCH_SCHEMA,
        "protocols/schema-versions/v1.json dc_store.workbench_schema disagrees with WORKBENCH_SCHEMA"
    );
}

#[test]
fn number_reads_a_plain_integer() {
    assert_eq!(number(r#"{"g": {"k": 11, "j": 2}}"#, "g", "k"), 11);
    assert_eq!(number(r#"{"g": {"j": 2, "k": 11 }}"#, "g", "k"), 11);
}

#[test]
#[should_panic(expected = "is not an integer")]
fn number_rejects_a_decimal() {
    number(r#"{"g": {"k": 11.5}}"#, "g", "k");
}

#[test]
#[should_panic(expected = "is not an integer")]
fn number_rejects_trailing_garbage() {
    number(r#"{"g": {"k": 11x}}"#, "g", "k");
}
