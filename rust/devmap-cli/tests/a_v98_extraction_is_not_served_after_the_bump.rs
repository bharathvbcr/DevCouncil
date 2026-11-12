//! Extraction schema v99 closes the extract-v98 bundle. `EXTRACTION_SCHEMA_VERSION`
//! moved to 98 at the bundle's scaffold, before any lane filled a side table,
//! and stayed there through every extractor change that landed after it. So a
//! v98 row may hold whatever any of those builds wrote. The cache key carried
//! no way to tell those rows apart, so a store built by an early v98 binary was
//! served unchanged to a later one.
//!
//! The file is named for the identity it refuses, v98, the way the earlier bump
//! tests are. The payload is the Svelte one that made the debt visible: a lone
//! identifier in markup (`{size}`, `onclick={save}`) was a Call in an early v98
//! row and is a Name reference now.

use devmap_extract::cache::{CacheKey, ANALYZER_VERSION, EXTRACTION_SCHEMA_VERSION};
use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind};
use devmap_store::Store;

const SVELTE: &str = "<script>\n  let size = 1;\n  function save() {}\n</script>\n\n<p>{size}</p>\n<button onclick={save}>Save</button>\n";

fn schema_version() -> u32 {
    EXTRACTION_SCHEMA_VERSION
        .parse()
        .expect("the extraction schema version is a number")
}

fn bare_calls(e: &Extraction, name: &str) -> usize {
    e.calls
        .iter()
        .filter(|c| c.callee_name == name && c.receiver_expr.is_none())
        .count()
}

fn name_refs(e: &Extraction, name: &str) -> usize {
    e.references
        .iter()
        .filter(|r| r.kind == ReferenceKind::Name && r.name == name && r.receiver_expr.is_none())
        .count()
}

#[test]
fn the_current_extraction_reads_a_lone_markup_identifier_as_a_name() {
    assert!(schema_version() >= 99, "{EXTRACTION_SCHEMA_VERSION}");
    let e = extract_file("Panel.svelte", SVELTE);
    for name in ["size", "save"] {
        assert_eq!(bare_calls(&e, name), 0, "{name} is a read: {:?}", e.calls);
        assert!(
            name_refs(&e, name) >= 1,
            "{name} has no Name reference: {:?}",
            e.references
        );
    }
}

#[test]
fn a_payload_stored_under_the_v98_identity_is_not_served_to_a_v99_key() {
    let store = Store::open_in_memory().unwrap();
    let payload = extract_file("Panel.svelte", SVELTE);
    let current = CacheKey::for_extraction(&payload);
    let mut v98 = current.clone();
    v98.analyzer_version = format!("{ANALYZER_VERSION}:extract-v98");
    assert_ne!(v98, current, "the bump must change the stored identity");
    store.admit_cached_extraction(&v98, &payload).unwrap();

    assert!(
        store.try_get_cached_extraction(&v98).unwrap().is_some(),
        "control: the v98 row is stored"
    );
    let served = store.try_get_cached_extraction(&current).unwrap();
    assert!(served.is_none(), "a v98 payload was served to a v99 key");
}

#[test]
fn the_stored_identity_names_v99() {
    let key = CacheKey::for_extraction(&extract_file("Panel.svelte", SVELTE));
    let expected = format!(":extract-v{EXTRACTION_SCHEMA_VERSION}");
    assert!(
        key.analyzer_version.ends_with(&expected),
        "{} does not end with {expected}",
        key.analyzer_version
    );
    assert!(schema_version() >= 99, "{EXTRACTION_SCHEMA_VERSION}");
}
