//! Extraction schema v98 adds six side tables to `Extraction`: `jvm_decls`,
//! `package_scopes`, `local_extents`, `decl_gaps`, `recovered_decl_ranges` and
//! `dynamic_dispatch`. A
//! cached v97 payload has none of them, and an empty list cannot say "not
//! collected", so serving it would leave every unchanged file without the fact:
//! the extractor's new output would only show on files that happened to be
//! edited.
//!
//! The file is named for the identity it refuses, v97, the way the earlier
//! bump tests are.
//!
//! At the scaffold commit no extractor fills the tables yet, so the first test
//! pins the version and the wire shape: an extraction without a fact serializes
//! without the key (old payloads and new ones read alike), and a populated one
//! round-trips. The facts' own tests land with the lane that emits each one.

use devmap_extract::cache::{CacheKey, ANALYZER_VERSION, EXTRACTION_SCHEMA_VERSION};
use devmap_extract::extract_file;
use devmap_extract::model::{
    DynamicDispatch, DynamicDispatchKind, Extraction, JvmDeclFact, JvmDeclKind, JvmModifiers,
    LocalExtent, PackageScope, TextRange,
};
use devmap_store::Store;

const KOTLIN: &str = "package app\n\nclass Repo {\n    fun load() {}\n}\n";
const FIELDS: [&str; 6] = [
    "jvm_decls",
    "package_scopes",
    "local_extents",
    "decl_gaps",
    "recovered_decl_ranges",
    "dynamic_dispatch",
];

fn schema_version() -> u32 {
    EXTRACTION_SCHEMA_VERSION
        .parse()
        .expect("the extraction schema version is a number")
}

fn range(start_byte: usize, end_byte: usize) -> TextRange {
    TextRange {
        start_byte,
        end_byte,
    }
}

fn populated(mut extraction: Extraction) -> Extraction {
    extraction.jvm_decls = vec![JvmDeclFact {
        qualified_name: "Repo.kt::Repo".into(),
        start_byte: 12,
        kind: JvmDeclKind::Type,
        modifiers: JvmModifiers::INNER.with(JvmModifiers::PRIVATE),
        type_params: vec!["T".into()],
        header: Some(range(12, 30)),
        body: Some(range(31, 60)),
        alias_target: None,
        companion_name: None,
    }];
    extraction.package_scopes = vec![PackageScope {
        name: "app".into(),
        span: range(0, 100),
    }];
    extraction.local_extents = vec![LocalExtent {
        scope: "Repo.kt::Repo.load".into(),
        name: "x".into(),
        extent: range(40, 55),
    }];
    extraction.decl_gaps = vec![range(70, 90)];
    extraction.recovered_decl_ranges = vec![range(95, 120)];
    extraction.dynamic_dispatch = vec![DynamicDispatch {
        caller_symbol: Some("h.py::run".into()),
        start_byte: 20,
        kind: DynamicDispatchKind::Getattr,
        receiver: Some("self".into()),
        name_prefix: Some("cmd_".into()),
        name_is_literal: false,
    }];
    extraction
}

#[test]
fn the_current_extraction_names_v98_and_carries_the_new_tables_on_the_wire() {
    assert!(schema_version() >= 98, "{EXTRACTION_SCHEMA_VERSION}");

    // A default extraction writes none of the six keys, so a payload stored
    // before the tables existed and one stored without a fact are the same bytes.
    let plain = extract_file("Repo.kt", KOTLIN);
    let json = serde_json::to_value(&plain).unwrap();
    for field in FIELDS {
        assert!(
            json.get(field).is_none(),
            "`{field}` was written when empty"
        );
    }
    // ... and a payload without the keys reads back with the tables empty.
    let back: Extraction = serde_json::from_value(json).unwrap();
    assert!(back.jvm_decls.is_empty() && back.dynamic_dispatch.is_empty());

    // A populated extraction writes every key and reads them back unchanged.
    let full = populated(plain);
    let json = serde_json::to_value(&full).unwrap();
    for field in FIELDS {
        assert!(json.get(field).is_some(), "`{field}` was dropped");
    }
    let back: Extraction = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back.jvm_decls, full.jvm_decls);
    assert_eq!(back.package_scopes, full.package_scopes);
    assert_eq!(back.local_extents, full.local_extents);
    assert_eq!(back.decl_gaps, full.decl_gaps);
    assert_eq!(back.recovered_decl_ranges, full.recovered_decl_ranges);
    assert_eq!(back.dynamic_dispatch, full.dynamic_dispatch);
    assert_eq!(serde_json::to_value(&back).unwrap(), json);
}

#[test]
fn a_payload_stored_under_the_v97_identity_is_not_served_to_a_v98_key() {
    let store = Store::open_in_memory().unwrap();
    // Strip what v98 adds. Nothing fills the tables yet, so this clears nothing
    // today; it is the shape each lane's test keeps once its fact exists.
    let mut old = extract_file("Repo.kt", KOTLIN);
    old.jvm_decls.clear();
    old.package_scopes.clear();
    old.local_extents.clear();
    old.decl_gaps.clear();
    old.recovered_decl_ranges.clear();
    old.dynamic_dispatch.clear();
    let current = CacheKey::for_extraction(&old);
    let mut v97 = current.clone();
    v97.analyzer_version = format!("{ANALYZER_VERSION}:extract-v97");
    store.admit_cached_extraction(&v97, &old).unwrap();

    assert!(
        store.try_get_cached_extraction(&v97).unwrap().is_some(),
        "control: the v97 row is stored"
    );
    let served = store.try_get_cached_extraction(&current).unwrap();
    assert!(served.is_none(), "a v97 payload was served to a v98 key");
}

#[test]
fn the_stored_identity_names_the_current_extraction_schema() {
    let key = CacheKey::for_extraction(&extract_file("Repo.kt", KOTLIN));
    let expected = format!(":extract-v{EXTRACTION_SCHEMA_VERSION}");
    assert!(
        key.analyzer_version.ends_with(&expected),
        "{} does not end with {expected}",
        key.analyzer_version
    );
    assert!(schema_version() >= 98, "{EXTRACTION_SCHEMA_VERSION}");
}
