//! `testdata/accuracy/record.json` is committed, and its labelled rows come
//! from corpora that are private repositories. A row there may say that a
//! symbol was labelled and how (`verdict`, `published`), never which symbol or
//! where it lives.
//!
//! For every corpus but DevCouncil's own (public, so its rows stay readable):
//!
//! * `symbol` is a hash token, `h:` plus twelve lowercase hex digits - the
//!   first twelve of the SHA-256 of the original id. Anything else is a name
//!   from a private repository.
//! * `evidence` is one category token (`rg_declaration_only`, ...), never the
//!   prose the labeller wrote. A string with a `/`, a `\`, a `::` or a source
//!   file extension is a path or a qualified symbol and fails by name; a
//!   token outside the vocabulary (prose included) fails too. The hash shape
//!   and the vocabulary are `devmap_analyze::record_identity`, the same
//!   definitions the corpus runner writes rows with.
//!
//! The check fails closed. A row that is not an object, or lacks a string
//! `symbol` or `evidence`, is a failure and not a pass: a row this test could
//! not read is not a row it cleared. A corpus without a name, or a record with
//! no `corpora` array, fails for the same reason.

use std::path::Path;

use devmap_analyze::record_identity::{is_evidence_category, is_identity_hash};
use serde_json::Value;

/// Extensions that make a bare token a file name.
const SOURCE_EXTENSIONS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "py", "pyi", "go", "rs", "kt", "kts", "swift", "java",
    "c", "h", "cc", "cpp", "hpp", "m", "mm", "rb", "php", "cs", "json", "toml", "yaml", "yml",
    "md", "sh", "sql", "gradle", "xml", "html", "css",
];

const OWN_CORPUS: &str = "DevCouncil";

fn record_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("devmap-cli sits under the workspace root")
        .join("testdata/accuracy/record.json")
}

/// A `.ext` suffix on any run of identifier characters.
fn has_source_extension(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' {
            let rest = &s[i + 1..];
            let ext: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if SOURCE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Why a string looks like a private path or qualified symbol, if it does.
fn path_like(s: &str) -> Option<&'static str> {
    if s.contains('/') || s.contains('\\') {
        Some("contains a path separator")
    } else if s.contains("::") {
        Some("is `::`-qualified")
    } else if has_source_extension(s) {
        Some("names a source file extension")
    } else {
        None
    }
}

fn check_field(
    where_: &str,
    row: &serde_json::Map<String, Value>,
    field: &str,
    out: &mut Vec<String>,
) {
    let Some(text) = row.get(field).and_then(Value::as_str) else {
        out.push(format!(
            "{where_}: `{field}` is missing or not a string, so it cannot be shown clean"
        ));
        return;
    };
    if let Some(why) = path_like(text) {
        out.push(format!(
            "{where_}: `{field}` {why} (a private identity in a committed record)"
        ));
    } else if field == "symbol" && !is_identity_hash(text) {
        out.push(format!(
            "{where_}: `symbol` is not an `h:<12 hex>` hash token"
        ));
    } else if field == "evidence" && !is_evidence_category(text) {
        out.push(format!(
            "{where_}: `evidence` is not one of the README's category tokens"
        ));
    }
}

/// Every offence in `record`, each naming its corpus, language and row.
fn leaks(record: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(corpora) = record.get("corpora").and_then(Value::as_array) else {
        return vec!["record has no `corpora` array".to_string()];
    };
    for (ci, corpus) in corpora.iter().enumerate() {
        let Some(name) = corpus.get("name").and_then(Value::as_str) else {
            out.push(format!("corpora[{ci}] has no string `name`"));
            continue;
        };
        if name == OWN_CORPUS {
            continue;
        }
        let Some(languages) = corpus.get("languages").and_then(Value::as_object) else {
            out.push(format!("{name}: `languages` is missing or not an object"));
            continue;
        };
        for (lang, entry) in languages {
            // `unsure` rows live in `labelled_rows` too (verdict `unsure`);
            // the sibling `unsure` field is only their count.
            let Some(rows) = entry.get("labelled_rows") else {
                continue;
            };
            let Some(rows) = rows.as_array() else {
                out.push(format!("{name} {lang}: `labelled_rows` is not an array"));
                continue;
            };
            for (ri, row) in rows.iter().enumerate() {
                let where_ = format!("{name} {lang} labelled_rows[{ri}]");
                let Some(row) = row.as_object() else {
                    out.push(format!("{where_}: row is not an object"));
                    continue;
                };
                check_field(&where_, row, "symbol", &mut out);
                check_field(&where_, row, "evidence", &mut out);
            }
        }
    }
    out
}

#[test]
fn the_committed_record_names_no_private_symbol_or_path() {
    let raw = std::fs::read_to_string(record_path()).expect("record.json is readable");
    let record: Value = serde_json::from_str(&raw).expect("record.json is JSON");
    let found = leaks(&record);
    assert!(
        found.is_empty(),
        "record.json names private identity in {} place(s); hash the symbol and replace the \
         evidence with a category token:\n  {}",
        found.len(),
        found.join("\n  ")
    );
    // A walk that found no private row would make the pass vacuous.
    let scanned: usize = record["corpora"]
        .as_array()
        .expect("corpora")
        .iter()
        .filter(|c| c["name"] != OWN_CORPUS)
        .flat_map(|c| {
            c["languages"]
                .as_object()
                .into_iter()
                .flat_map(|l| l.values())
        })
        .map(|l| l["labelled_rows"].as_array().map_or(0, Vec::len))
        .sum();
    assert!(
        scanned > 0,
        "no labelled row of a private corpus was scanned, so a clean result proves nothing"
    );
}

fn record_with(corpus: &str, row: Value) -> Value {
    serde_json::json!({ "corpora": [ {
        "name": corpus,
        "languages": { "go": { "labelled_rows": [ row ] } }
    } ] })
}

fn clean_row() -> Value {
    serde_json::json!({
        "symbol": "h:0123456789ab",
        "evidence": "rg_declaration_only",
        "verdict": "dead_true",
        "published": "0.9"
    })
}

#[test]
fn a_hashed_symbol_with_a_category_token_passes() {
    assert_eq!(
        leaks(&record_with("Private", clean_row())),
        Vec::<String>::new()
    );
}

#[test]
fn empty_rows_and_empty_languages_pass() {
    let none = serde_json::json!({ "corpora": [ { "name": "Private", "languages": {
        "go": { "labelled_rows": [] }, "rust": {} } } ] });
    assert!(leaks(&none).is_empty());
}

#[test]
fn a_path_or_qualified_symbol_in_a_private_corpus_fails_naming_the_row() {
    for symbol in [
        "pkg/server/handler.go::Serve",
        "Handler::serve",
        "handler.ts",
        "C:\\src\\main.rs",
    ] {
        let mut row = clean_row();
        row["symbol"] = symbol.into();
        let found = leaks(&record_with("Private", row));
        assert_eq!(found.len(), 1, "{symbol}: {found:?}");
        assert!(
            found[0].contains("Private go labelled_rows[0]") && found[0].contains("`symbol`"),
            "{found:?}"
        );
        assert!(
            !found[0].contains(symbol),
            "the offending string must not be echoed"
        );
    }
}

#[test]
fn a_path_in_evidence_fails_even_with_a_hashed_symbol() {
    for evidence in [
        "rg hit at src/lib.rs:12",
        "lib.py",
        "a::b",
        "rg_declaration_only/extra",
    ] {
        let mut row = clean_row();
        row["evidence"] = evidence.into();
        let found = leaks(&record_with("Private", row));
        assert!(
            found.iter().any(|f| f.contains("`evidence`")),
            "{evidence}: {found:?}"
        );
    }
}

#[test]
fn a_token_outside_the_vocabulary_fails_though_it_looks_harmless() {
    let mut row = clean_row();
    row["evidence"] = "some_made_up_token".into();
    assert_eq!(leaks(&record_with("Private", row)).len(), 1);
}

#[test]
fn prose_evidence_and_a_bare_private_name_fail() {
    let mut row = clean_row();
    row["evidence"] = "declaration only, nothing imports it".into();
    assert_eq!(leaks(&record_with("Private", row)).len(), 1);
    let mut row = clean_row();
    row["symbol"] = "YearData".into();
    assert_eq!(leaks(&record_with("Private", row)).len(), 1);
    for near_hash in [
        "h:0123456789A",
        "h:0123456789abc",
        "h:0123456789ag",
        "0123456789ab",
    ] {
        let mut row = clean_row();
        row["symbol"] = near_hash.into();
        assert_eq!(leaks(&record_with("Private", row)).len(), 1, "{near_hash}");
    }
}

#[test]
fn devcouncils_own_rows_may_carry_paths() {
    let row = serde_json::json!({
        "symbol": "rust/devmap-analyze/src/lib.rs::analyze",
        "evidence": "rg -uu -n -w analyze rust/devmap-cli/src/main.rs:12 calls it",
        "verdict": "dead_false", "published": "0.4"
    });
    assert!(leaks(&record_with("DevCouncil", row)).is_empty());
}

#[test]
fn a_row_the_check_cannot_read_fails_instead_of_passing() {
    for row in [
        serde_json::json!({ "evidence": "rg_declaration_only" }),
        serde_json::json!({ "symbol": "h:0123456789ab" }),
        serde_json::json!({ "symbol": 7, "evidence": null }),
        serde_json::json!("not an object"),
        serde_json::json!(null),
    ] {
        assert!(
            !leaks(&record_with("Private", row.clone())).is_empty(),
            "{row} passed"
        );
    }
    assert!(!leaks(&serde_json::json!({})).is_empty());
    assert!(!leaks(&serde_json::json!({ "corpora": [ { "languages": {} } ] })).is_empty());
    assert!(
        !leaks(&serde_json::json!({ "corpora": [ { "name": "P", "languages": 3 } ] })).is_empty()
    );
    assert!(!leaks(
        &serde_json::json!({ "corpora": [ { "name": "P", "languages": {
        "go": { "labelled_rows": "x" } } } ] })
    )
    .is_empty());
}

#[test]
fn an_unsure_row_is_held_to_the_same_rule() {
    let mut row = clean_row();
    row["verdict"] = "unsure".into();
    row["symbol"] = "a/b.go::c".into();
    assert!(!leaks(&record_with("Private", row)).is_empty());
}
