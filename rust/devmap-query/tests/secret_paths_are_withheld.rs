//! A file DevCouncil's gates treat as credentials is indexed for its structure
//! and never answers with its values.
//!
//! `policy.SecretPathPatterns` is the list the write gate refuses and the read
//! gate will not show. Before this, `literals` returned an AWS key from
//! `credentials/`, `ask --evidence` printed `secrets/` source verbatim, and the
//! store kept both at rest. Every query below runs against files that plant a
//! value in each shape the audit used, and asserts no answer carries one and
//! each withheld item says so.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_extract::secret::{WITHHELD_LITERAL_VALUE, WITHHELD_SECRET_PATH};
use devmap_query::{Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::{Path, PathBuf};

const PLANTED: &[&str] = &[
    "AKIAIOSFODNN7EXAMPLQ",
    "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
    "xoxb-123456789012-abcdefghijkl",
    "hunter2",
];

const DB: &str = r#"DB_PASSWORD = "hunter2"


def load_prod_database_password():
    return "hunter2"
"#;

const AWS: &str = r#"AWS_KEY = "AKIAIOSFODNN7EXAMPLQ"
GH_TOKEN = "ghp_abcdefghijklmnopqrstuvwxyz0123456789"


def aws_credentials():
    return "xoxb-123456789012-abcdefghijkl"
"#;

const FILES: &[(&str, &str)] = &[
    ("secrets/db.py", DB),
    ("credentials/aws.py", AWS),
    (".env", "DB_PASSWORD=hunter2\nAWS=AKIAIOSFODNN7EXAMPLQ\n"),
    (
        "deploy/server.key",
        "-----BEGIN PRIVATE KEY-----\nhunter2\nghp_abcdefghijklmnopqrstuvwxyz0123456789\n-----END PRIVATE KEY-----\n",
    ),
    (
        "src/app.py",
        "def load_config():\n    return \"plain-value\"\n",
    ),
];

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-secret-paths-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    for (path, body) in FILES {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    dir
}

fn save(store: &Store, root: &Path, extractions: &[Extraction]) {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = devmap_analyze::analyze(extractions, &resolution);
    store
        .save_generation_with_opts(
            extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..GenerationWriteOpts::default()
            },
        )
        .unwrap();
}

fn extract_all() -> Vec<Extraction> {
    FILES
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect()
}

#[track_caller]
fn assert_no_planted_value(label: &str, text: &str) {
    for value in PLANTED {
        assert!(!text.contains(value), "{label} carries {value:?}: {text}");
    }
}

fn is_secret(path: &str) -> bool {
    !path.starts_with("src/")
}

#[test]
fn literals_search_ask_explore_and_skeleton_withhold_secret_values() {
    let root = scratch("queries");
    let store = Store::open(root.join("devmap.sqlite")).unwrap();
    save(&store, &root, &extract_all());
    let engine = StoreQueryEngine::new(&store);

    for query in ["AKIA", "ghp_", "xoxb", "hunter2"] {
        let report = engine.literals(query, false, 8_000).unwrap();
        assert_no_planted_value(
            &format!("literals {query}"),
            // The items, not the report: it echoes the query it was asked.
            &serde_json::to_string(&report.items).unwrap(),
        );
    }
    // The sites are still there: located, marked, and without their text.
    let marked = engine.literals("<withheld", false, 8_000).unwrap();
    for file in ["secrets/db.py", "credentials/aws.py"] {
        assert!(
            marked.items.iter().any(|site| site.file_path == file),
            "{file} keeps its literal sites: {marked:?}"
        );
    }
    for site in &marked.items {
        assert_eq!(site.value, WITHHELD_LITERAL_VALUE);
        assert_eq!(site.withheld.as_deref(), Some(WITHHELD_SECRET_PATH));
    }

    let pack = engine
        .ask_evidence("load prod database password", 8_000, 0.0)
        .unwrap();
    let pack_json = serde_json::to_string(&pack).unwrap();
    assert_no_planted_value("ask --evidence", &pack_json);
    assert!(
        pack_json.contains("load_prod_database_password")
            && pack_json.contains(r#""withheld":"secret_path""#),
        "the secret file is found and marked: {pack_json}"
    );

    for name in [
        "load_prod_database_password",
        "aws_credentials",
        "load_config",
    ] {
        let hits = engine
            .search(Request {
                query: name.to_string(),
                token_budget: 8_000,
                min_confidence: 0.0,
                max_depth: 1,
            })
            .unwrap();
        assert_no_planted_value(
            &format!("search {name}"),
            &serde_json::to_string(&hits).unwrap(),
        );
        let hit = hits
            .items
            .iter()
            .find(|hit| hit.symbol_name == name)
            .unwrap_or_else(|| panic!("search {name} finds it: {hits:?}"));
        if is_secret(&hit.file_path) {
            assert_eq!(hit.withheld.as_deref(), Some(WITHHELD_SECRET_PATH));
            assert!(hit.source_span.is_empty());
            assert!(hit.span != (0, 0), "the location is still reported");
        } else {
            assert_eq!(hit.withheld, None);
            assert!(hit.source_span.contains("plain-value"), "{hit:?}");
        }

        let explored = engine.explore(name, 5, 8_000, 0.0, 2).unwrap();
        assert_no_planted_value(
            &format!("explore {name}"),
            &serde_json::to_string(&explored).unwrap(),
        );
    }

    for file in ["secrets/db.py", "credentials/aws.py"] {
        let skeleton = engine.skeleton(file, 8_000).unwrap();
        assert_no_planted_value(
            &format!("skeleton {file}"),
            &serde_json::to_string(&skeleton).unwrap(),
        );
        assert!(skeleton.items.iter().all(|item| item.withheld.as_deref()
            == Some(WITHHELD_SECRET_PATH)
            && item.signature.is_none()));
    }

    drop(engine);
    drop(store);
    assert_store_holds_no_planted_value(&root.join("devmap.sqlite"));
}

/// Every text and blob column of every table, read back as bytes.
fn assert_store_holds_no_planted_value(db: &Path) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut scanned = 0usize;
    for table in tables {
        let Ok(mut stmt) = conn.prepare(&format!("SELECT * FROM \"{table}\"")) else {
            continue;
        };
        let columns = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for index in 0..columns {
                let bytes: Vec<u8> = match row.get_ref(index).unwrap() {
                    rusqlite::types::ValueRef::Text(text) => text.to_vec(),
                    rusqlite::types::ValueRef::Blob(blob) => blob.to_vec(),
                    _ => continue,
                };
                scanned += 1;
                let text = String::from_utf8_lossy(&bytes);
                for value in PLANTED {
                    assert!(
                        !text.contains(value),
                        "{table} persists {value:?} at rest: {text}"
                    );
                }
            }
        }
    }
    assert!(scanned > 100, "only {scanned} cells were read");
}

/// A store written before values were withheld still holds them. Its next
/// build must not carry them forward, and a query against it before that
/// build must not show them.
#[test]
fn a_store_written_before_the_rule_is_withheld_on_read_and_scrubbed_on_write() {
    let root = scratch("stale");
    let db = root.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    save(&store, &root, &extract_all());

    // What the pre-fix writer left: raw values in the literal rows and the
    // payload of a secret file.
    let raw = {
        let mut raw = extract_file("src/db.py", DB);
        raw.file_path = "secrets/db.py".to_string();
        raw.source_code = None;
        serde_json::to_string(&raw).unwrap()
    };
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let file_id: i64 = conn
            .query_row(
                "SELECT id FROM paths WHERE path = 'secrets/db.py'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let changed = conn
            .execute(
                "UPDATE generation_literals SET value = 'hunter2' WHERE file_id = ?1",
                [file_id],
            )
            .unwrap();
        assert!(changed > 0, "the fixture planted literal rows");
        let changed = conn
            .execute(
                "UPDATE file_payloads SET extraction_json = ?2 WHERE file_id = ?1",
                rusqlite::params![file_id, raw],
            )
            .unwrap();
        assert!(changed > 0, "the fixture planted a payload");
    }

    let engine = StoreQueryEngine::new(&store);
    let before = engine.literals("hunter2", false, 8_000).unwrap();
    assert!(before.total > 0, "the stale rows still match: {before:?}");
    assert_no_planted_value(
        "literals against a stale store",
        &serde_json::to_string(&before.items).unwrap(),
    );
    assert!(before
        .items
        .iter()
        .all(|site| site.withheld.as_deref() == Some(WITHHELD_SECRET_PATH)));
    drop(engine);

    // The next build, of an unchanged tree.
    save(&store, &root, &extract_all());
    drop(store);
    assert_store_holds_no_planted_value(&db);
}
