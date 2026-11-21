//! Which JSON keys carry file text, held to real output.
//!
//! A host's read gate treats the values under
//! `devmap_query::model::CONTENT_BEARING_KEYS` as repository content and every
//! other string as metadata. That only holds if no command puts file text under
//! a key outside the list. This plants distinctive tokens in a fixture's bodies,
//! signatures and literals, runs the commands, and walks every string in every
//! answer. It also pins the `withheld` marker: a secret-path file's content key
//! is empty *and* says why, and an ordinary file has no such key.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use devmap_query::model::{is_content_bearing_path, CONTENT_BEARING_KEYS};
use serde_json::Value;

/// Text that exists only inside a file's bytes: in a body, a literal, a
/// parameter name. None of it is a symbol, path or other metadata devmap names.
const BODY_TOKEN: &str = "bodytoken_zq81";
const LITERAL_TOKEN: &str = "literaltoken_zq82";
const SIGNATURE_TOKEN: &str = "sigtoken_zq83";
const SECRET_TOKEN: &str = "secrettoken_zq84";

const SOURCES: &[(&str, &str)] = &[
    ("go.mod", "module fx\n\ngo 1.22\n"),
    (
        "src/app.go",
        "package src\n\nimport \"fx/secrets\"\n\n// bodytoken_zq81\nfunc LoadConfig(sigtoken_zq83 string) string {\n\tprintln(\"literaltoken_zq82\")\n\treturn secrets.OpenVault()\n}\n\nfunc Main() string {\n\treturn LoadConfig(\"x\")\n}\n",
    ),
    (
        "src/lib.rs",
        "pub fn compute(sigtoken_zq83: u32) -> u32 {\n    sigtoken_zq83 // bodytoken_zq81\n}\n",
    ),
    (
        "src/app_test.go",
        "package src\n\nimport \"testing\"\n\nfunc TestMain1(t *testing.T) {\n\tMain()\n}\n",
    ),
    (
        "secrets/vault.go",
        "package secrets\n\nfunc OpenVault() string {\n\treturn \"secrettoken_zq84\"\n}\n",
    ),
];

fn devmap() -> String {
    env!("CARGO_BIN_EXE_devmap").to_string()
}

fn run(root: &Path, args: &[&str]) -> Output {
    let output = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    assert!(
        output.status.success(),
        "devmap {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn fixture() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "devmap-content-keys-{}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in SOURCES {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "t@example.invalid"],
        vec!["config", "user.name", "t"],
        vec!["add", "-A"],
        vec!["commit", "-qm", "fixture"],
    ] {
        let out = Command::new("git")
            .args(&args)
            .current_dir(&root)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let root = root.canonicalize().unwrap();
    run(&root, &["build"]);
    root
}

fn json(root: &Path, args: &[&str]) -> Value {
    let mut full = vec!["--json"];
    full.extend_from_slice(args);
    let output = run(root, &full);
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("devmap {full:?} is not one JSON value: {error}"))
}

/// Every (key path, key, string) in `value`. A string inside an array is
/// attributed to the key that holds the array.
fn strings<'a>(value: &'a Value, key: &str, path: &str, out: &mut Vec<(String, String, &'a str)>) {
    match value {
        Value::String(text) => out.push((path.to_string(), key.to_string(), text)),
        Value::Array(items) => {
            for item in items {
                strings(item, key, path, out);
            }
        }
        Value::Object(map) => {
            for (name, child) in map {
                strings(child, name, &format!("{path}.{name}"), out);
            }
        }
        _ => {}
    }
}

/// Paths of strings that hold `tokens` under a key outside `CONTENT_BEARING_KEYS`.
fn leaks(value: &Value, tokens: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    strings(value, "", "$", &mut found);
    found
        .into_iter()
        .filter(|(path, _, text)| {
            !is_content_bearing_path(path) && tokens.iter().any(|token| text.contains(token))
        })
        .map(|(path, _, _)| path)
        .collect()
}

/// Paths of every content-bearing key's string that holds one of `tokens`.
fn carried(value: &Value, tokens: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    strings(value, "", "$", &mut found);
    found
        .into_iter()
        .filter(|(path, _, text)| {
            is_content_bearing_path(path) && tokens.iter().any(|token| text.contains(token))
        })
        .map(|(path, _, _)| path)
        .collect()
}

const ALL_TOKENS: &[&str] = &[BODY_TOKEN, LITERAL_TOKEN, SIGNATURE_TOKEN, SECRET_TOKEN];

/// The walker is only worth anything if it can fail: file text under an
/// unlisted key is reported, under a listed key it is not.
#[test]
fn the_walker_reports_file_text_under_an_unlisted_key() {
    let bad: Value = serde_json::json!({"items": [{"note": "x bodytoken_zq81 y"}]});
    assert_eq!(leaks(&bad, ALL_TOKENS), vec!["$.items.note".to_string()]);
    let good: Value = serde_json::json!({"items": [{"source": "x bodytoken_zq81 y"}]});
    assert!(leaks(&good, ALL_TOKENS).is_empty());
}

#[test]
fn file_text_appears_only_under_published_content_keys() {
    let root = fixture();
    let answers: Vec<(&str, Value)> = vec![
        (
            "explore LoadConfig",
            json(&root, &["explore", "LoadConfig"]),
        ),
        ("explore OpenVault", json(&root, &["explore", "OpenVault"])),
        ("impact LoadConfig", json(&root, &["impact", "LoadConfig"])),
        ("impact OpenVault", json(&root, &["impact", "OpenVault"])),
        ("trace", json(&root, &["trace", "Main", "OpenVault"])),
        ("affected", json(&root, &["affected", "LoadConfig"])),
        (
            "ask --evidence",
            json(&root, &["ask", "--evidence", "load config"]),
        ),
        ("search", json(&root, &["search", "LoadConfig"])),
        ("skeleton", json(&root, &["skeleton", "src/lib.rs"])),
        ("literals", json(&root, &["literals", "literaltoken"])),
    ];
    for (label, answer) in &answers {
        let found = leaks(answer, ALL_TOKENS);
        assert!(
            found.is_empty(),
            "{label}: file text under keys outside CONTENT_BEARING_KEYS: {found:?}\n{answer}"
        );
    }
    // The positive side: the listed keys really do carry the text, so the
    // list is not passing by never being exercised.
    let by = |label: &str| &answers.iter().find(|(l, _)| *l == label).unwrap().1;
    assert!(!carried(by("explore LoadConfig"), &[BODY_TOKEN, LITERAL_TOKEN]).is_empty());
    assert!(!carried(by("ask --evidence"), &[BODY_TOKEN, LITERAL_TOKEN]).is_empty());
    assert!(!carried(by("skeleton"), &[SIGNATURE_TOKEN]).is_empty());
    assert!(!carried(by("literals"), &[LITERAL_TOKEN]).is_empty());
    // Graph answers carry no content key at all.
    for label in ["impact LoadConfig", "trace", "affected"] {
        let mut found = Vec::new();
        strings(by(label), "", "$", &mut found);
        assert!(
            found
                .iter()
                .all(|(path, _, _)| !is_content_bearing_path(path)),
            "{label} carries a content-bearing key"
        );
    }
    // `evidence.source` is provenance, not content.
    let mut found = Vec::new();
    strings(by("impact LoadConfig"), "", "$", &mut found);
    let provenance: Vec<&str> = found
        .iter()
        .filter(|(path, _, _)| path.ends_with(".evidence.source"))
        .map(|(_, _, text)| *text)
        .collect();
    assert!(
        !provenance.is_empty(),
        "no evidence.source in impact output"
    );
    for value in provenance {
        assert!(
            ["Resolver", "Stored", "Reconstructed"].contains(&value),
            "evidence.source {value:?} is not provenance"
        );
    }
}

/// A secret-path definition says its text was withheld; an ordinary one does
/// not, and its text is there.
#[test]
fn a_secret_path_definition_carries_the_withheld_marker_and_a_normal_one_does_not() {
    let root = fixture();

    let secret = json(&root, &["explore", "OpenVault"]);
    let item = &secret["definitions"]["items"][0];
    assert_eq!(item["file_path"], "secrets/vault.go");
    assert_eq!(
        item["source"], "",
        "the text of a secret file is not emitted"
    );
    assert_eq!(item["withheld"], "secret_path");

    let plain = json(&root, &["explore", "LoadConfig"]);
    let item = &plain["definitions"]["items"][0];
    assert!(item["source"].as_str().unwrap().contains("func LoadConfig"));
    assert!(
        item.get("withheld").is_none(),
        "an ordinary file carries no withheld key: {item}"
    );

    let evidence = json(&root, &["ask", "--evidence", "open vault"]);
    let units = evidence["files"][0]["units"].as_array().unwrap();
    assert!(!units.is_empty());
    for unit in units {
        assert_eq!(unit["source_span"], "", "{unit}");
        assert_eq!(unit["withheld"], "secret_path", "{unit}");
    }
    let normal = json(&root, &["ask", "--evidence", "load config"]);
    for file in normal["files"].as_array().unwrap() {
        for unit in file["units"].as_array().unwrap() {
            assert!(unit.get("withheld").is_none(), "{unit}");
        }
    }

    for answer in [&secret, &evidence] {
        assert!(
            !answer.to_string().contains(SECRET_TOKEN),
            "secret text reached the answer"
        );
    }
}

/// The doc and the constant name the same keys.
#[test]
fn the_host_integration_doc_lists_every_content_bearing_key() {
    let doc = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/devmap/HOST_INTEGRATION.md"),
    )
    .expect("HOST_INTEGRATION.md is readable");
    let section = doc
        .split("## Content-bearing JSON keys")
        .nth(1)
        .expect("the section is published")
        .split("\n## ")
        .next()
        .unwrap();
    for key in CONTENT_BEARING_KEYS {
        assert!(
            section.contains(&format!("| `{key}` |")),
            "{key} is in CONTENT_BEARING_KEYS but not in the doc table"
        );
    }
    let rows = section.lines().filter(|l| l.starts_with("| `")).count();
    assert_eq!(
        rows,
        CONTENT_BEARING_KEYS.len(),
        "doc rows and constant differ"
    );
}
