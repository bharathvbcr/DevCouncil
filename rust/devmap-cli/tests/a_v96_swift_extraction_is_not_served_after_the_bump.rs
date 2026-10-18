//! A Swift stored or computed property became a `Field` symbol in extraction
//! schema v97. A cached v96 payload has no such symbol, so serving it would
//! leave every unchanged Swift file with no property to bind a read to: the fix
//! would only show on files that happened to be edited.

use devmap_extract::cache::{CacheKey, ANALYZER_VERSION, EXTRACTION_SCHEMA_VERSION};
use devmap_extract::extract_file;
use devmap_extract::model::SymbolKind;
use devmap_store::Store;

const SOURCE: &str = "struct S {\n    var count: Int\n    var doubled: Int { count * 2 }\n}\n";

fn property_symbols(ext: &devmap_extract::model::Extraction) -> usize {
    ext.symbols
        .iter()
        .filter(|symbol| symbol.kind == SymbolKind::Field)
        .count()
}

#[test]
fn the_current_extraction_carries_the_property_symbols() {
    assert_eq!(property_symbols(&extract_file("s.swift", SOURCE)), 2);
    assert_eq!(EXTRACTION_SCHEMA_VERSION, "97");
}

#[test]
fn a_payload_stored_under_the_v96_identity_is_not_served_to_a_v97_key() {
    let store = Store::open_in_memory().unwrap();
    let mut old = extract_file("s.swift", SOURCE);
    old.symbols
        .retain(|symbol| symbol.kind != SymbolKind::Field);
    assert_eq!(property_symbols(&old), 0);
    let current = CacheKey::for_extraction(&old);
    let mut v96 = current.clone();
    v96.analyzer_version = format!("{ANALYZER_VERSION}:extract-v96");
    store.admit_cached_extraction(&v96, &old).unwrap();

    assert!(
        store.try_get_cached_extraction(&v96).unwrap().is_some(),
        "control: the v96 row is stored"
    );
    let served = store.try_get_cached_extraction(&current).unwrap();
    assert!(served.is_none(), "a v96 payload was served to a v97 key");
}

#[test]
fn the_stored_identity_names_v97() {
    let key = CacheKey::for_extraction(&extract_file("s.swift", SOURCE));
    assert!(
        key.analyzer_version.ends_with(":extract-v97"),
        "{}",
        key.analyzer_version
    );
}
