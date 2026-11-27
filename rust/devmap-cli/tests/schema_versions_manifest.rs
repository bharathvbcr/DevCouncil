//! `protocols/schema-versions/v1.json` is the machine-readable manifest of every
//! schema and format version DevCouncil owns. Every DevMap constant must stay
//! pinned to this manifest so cross-repo parity checks (such as Waylode's
//! `scripts/check-schema-parity.mjs`) never drift from the Rust kernel.

use devmap_extract::cache::EXTRACTION_SCHEMA_VERSION;
use devmap_query::code_graph::CODE_GRAPH_SCHEMA_VERSION;
use devmap_query::stacked::STACK_FORMAT_VERSION;
use devmap_resolve::resolver::RESOLUTION_REUSE_EPOCH;
use devmap_store::schema::{CURRENT_SCHEMA_VERSION, MIN_READER_SCHEMA_VERSION};
use serde_json::Value;

fn manifest() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocols/schema-versions/v1.json"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

#[test]
fn manifest_pins_every_devmap_schema_and_format_constant() {
    let m = manifest();
    let description = m["description"].as_str().unwrap_or("");
    assert!(
        !description.contains("No consumer reads this file yet"),
        "manifest description still claims no consumer reads v1.json: {description}"
    );

    assert_eq!(
        m["devmap_store"]["current_schema_version"].as_i64(),
        Some(i64::from(CURRENT_SCHEMA_VERSION)),
        "devmap_store.current_schema_version disagrees with CURRENT_SCHEMA_VERSION"
    );
    assert_eq!(
        m["devmap_store"]["min_reader_schema_version"].as_i64(),
        Some(i64::from(MIN_READER_SCHEMA_VERSION)),
        "devmap_store.min_reader_schema_version disagrees with MIN_READER_SCHEMA_VERSION"
    );
    assert_eq!(
        m["devmap_formats"]["extraction_schema_version"].as_str(),
        Some(EXTRACTION_SCHEMA_VERSION),
        "devmap_formats.extraction_schema_version disagrees with EXTRACTION_SCHEMA_VERSION"
    );
    assert_eq!(
        m["devmap_formats"]["code_graph_schema_version"].as_u64(),
        Some(u64::from(CODE_GRAPH_SCHEMA_VERSION)),
        "devmap_formats.code_graph_schema_version disagrees with CODE_GRAPH_SCHEMA_VERSION"
    );
    assert_eq!(
        m["devmap_formats"]["resolution_reuse_epoch"].as_str(),
        Some(RESOLUTION_REUSE_EPOCH),
        "devmap_formats.resolution_reuse_epoch disagrees with RESOLUTION_REUSE_EPOCH"
    );
    assert_eq!(
        m["devmap_formats"]["stack_lease_format"].as_u64(),
        Some(u64::from(STACK_FORMAT_VERSION)),
        "devmap_formats.stack_lease_format disagrees with STACK_FORMAT_VERSION"
    );
}
