//! `self.name = callee(..)` gained `assigned_to = "self.name"` in extraction
//! schema v95. A cached v94 payload carries `None` there, so serving it would
//! leave every unchanged Python file without field initializers: the fix would
//! only show on files that happened to be edited.

use devmap_extract::cache::{CacheKey, ANALYZER_VERSION, EXTRACTION_SCHEMA_VERSION};
use devmap_extract::extract_file;
use devmap_store::Store;

const SOURCE: &str = "from a import A\n\nclass H:\n    def __init__(self):\n        self.e = A()\n";

fn field_binding(ext: &devmap_extract::model::Extraction) -> bool {
    ext.references
        .iter()
        .any(|reference| reference.assigned_to.as_deref() == Some("self.e"))
}

#[test]
fn the_current_extraction_carries_the_field_binding() {
    assert!(field_binding(&extract_file("h.py", SOURCE)));
    assert_eq!(EXTRACTION_SCHEMA_VERSION, "97");
}

#[test]
fn a_payload_stored_under_the_v94_identity_is_not_served_to_a_v95_key() {
    let store = Store::open_in_memory().unwrap();
    let mut old = extract_file("h.py", SOURCE);
    for reference in &mut old.references {
        reference.assigned_to = None;
    }
    let current = CacheKey::for_extraction(&old);
    let mut v94 = current.clone();
    v94.analyzer_version = format!("{ANALYZER_VERSION}:extract-v94");
    store.admit_cached_extraction(&v94, &old).unwrap();

    assert!(
        store.try_get_cached_extraction(&v94).unwrap().is_some(),
        "control: the v94 row is stored"
    );
    let served = store.try_get_cached_extraction(&current).unwrap();
    assert!(served.is_none(), "a v94 payload was served to a v95 key");
}

#[test]
fn the_stored_identity_names_the_current_version() {
    let key = CacheKey::for_extraction(&extract_file("h.py", SOURCE));
    assert!(key.analyzer_version.ends_with(":extract-v97"), "{}", key.analyzer_version);
}
