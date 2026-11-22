//! `protocols/schema-versions/v1.json` is the machine-readable copy of the schema numbers
//! this crate owns; consumers read it. It must equal the crate's own constants.

use devmap_store::schema::{CURRENT_SCHEMA_VERSION, MIN_READER_SCHEMA_VERSION};

fn manifest() -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../protocols/schema-versions/v1.json");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn number(m: &serde_json::Value, group: &str, key: &str) -> i64 {
    m[group][key]
        .as_i64()
        .unwrap_or_else(|| panic!("protocols/schema-versions/v1.json: {group}.{key} is missing or not an integer"))
}

#[test]
fn manifest_matches_devmap_store_constants() {
    let m = manifest();
    assert_eq!(
        number(&m, "devmap_store", "current_schema_version"),
        i64::from(CURRENT_SCHEMA_VERSION),
        "protocols/schema-versions/v1.json devmap_store.current_schema_version disagrees with CURRENT_SCHEMA_VERSION"
    );
    assert_eq!(
        number(&m, "devmap_store", "min_reader_schema_version"),
        i64::from(MIN_READER_SCHEMA_VERSION),
        "protocols/schema-versions/v1.json devmap_store.min_reader_schema_version disagrees with MIN_READER_SCHEMA_VERSION"
    );
}
