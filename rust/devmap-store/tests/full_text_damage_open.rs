//! Opening a store whose full-text index is damaged.
//!
//! Opening validates every table the readers name, `nodes_fts` included, and
//! asking `nodes_fts` for its columns reads the index's configuration. So damage
//! confined to the index used to fail the whole open, and a reader that had not
//! already held the store was refused everything, not only search.
//!
//! The default open stays strict, and says in the typed full-text error what the
//! damage is. A reader may ask to tolerate it ([`ReaderMemory::tolerate_full_text_damage`]);
//! it then gets a store that knows its index is damaged and answers what does not
//! read it. Damage that is not provably the index's alone is never served.
//!
//! The damage is real: root pages of the named objects are overwritten in place.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use devmap_store::{ReaderMemory, Store};

fn indexed_store(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-fts-open-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("devmap.sqlite");
    let mut extraction = devmap_extract::extract_file(
        "app.py",
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n",
    );
    extraction.literals = (0..4000)
        .map(|index| devmap_extract::ExtractedLiteral {
            value: format!("literal-value-{index:05}"),
            start_byte: 10 + index,
            line: 1,
            enclosing_qualified_name: String::new(),
            enclosing_name: String::new(),
        })
        .collect();
    let extractions = vec![extraction];
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open(&db).unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();
    drop(store);
    db
}

/// Overwrite the root page of every object `filter` selects from `sqlite_master`.
fn damage(db: &Path, filter: &str) {
    let page = {
        let header = std::fs::read(db).unwrap();
        let raw = u16::from_be_bytes([header[16], header[17]]);
        if raw == 1 {
            65_536
        } else {
            raw as usize
        }
    };
    let roots: Vec<i64> = {
        let conn =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT rootpage FROM sqlite_master WHERE ({filter}) AND rootpage > 1"
            ))
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(!roots.is_empty(), "precondition: {filter} has root pages");
    let mut file = std::fs::OpenOptions::new().write(true).open(db).unwrap();
    for root in roots {
        file.seek(SeekFrom::Start((root as u64 - 1) * page as u64))
            .unwrap();
        file.write_all(&vec![0xFF; page]).unwrap();
    }
    file.sync_all().unwrap();
}

/// The error of an open that must fail.
fn refused(result: rusqlite::Result<Store>, what: &str) -> rusqlite::Error {
    match result {
        Ok(_) => panic!("{what}"),
        Err(error) => error,
    }
}

fn cleanup(db: &Path) {
    std::fs::remove_dir_all(db.parent().unwrap()).unwrap();
}

#[test]
fn the_strict_open_of_a_damaged_index_is_typed_as_full_text_damage_with_its_repair() {
    let db = indexed_store("strict");
    damage(&db, "name LIKE 'nodes_fts%'");

    let error = refused(
        Store::open_read_only(&db),
        "the strict open refuses a damaged index",
    );
    assert!(
        Store::sqlite_error_is_full_text_damage(&error),
        "must be typed as full-text damage: {error:?}"
    );
    assert!(
        error.to_string().contains("devmap repair --fts"),
        "must name the repair: {error}"
    );
    cleanup(&db);
}

#[test]
fn a_reader_that_tolerates_it_opens_a_store_with_a_damaged_index_and_search_alone_refuses() {
    let db = indexed_store("degraded");
    damage(&db, "name LIKE 'nodes_fts%'");

    let store =
        Store::open_read_only_with(&db, ReaderMemory::CURRENT.tolerating_full_text_damage())
            .expect("a store whose only damage is its index opens degraded");
    assert!(store.full_text_damage().is_some());
    assert!(
        store.memory_settings().unwrap().tolerate_full_text_damage,
        "the setting in effect is read back"
    );
    assert!(store.latest_generation_id().unwrap().is_some());
    assert!(
        store
            .search_literals("literal-value-02000", true, 5)
            .unwrap()
            .is_some(),
        "literal search never reads the index and answers"
    );

    let refused = store.search_fts("alpha", 5).expect_err("search is refused");
    assert!(
        Store::sqlite_error_is_full_text_damage(&refused),
        "refused with the typed full-text error: {refused:?}"
    );
    cleanup(&db);
}

#[test]
fn a_healthy_store_opened_tolerantly_is_not_degraded() {
    let db = indexed_store("healthy");
    let store =
        Store::open_read_only_with(&db, ReaderMemory::CURRENT.tolerating_full_text_damage())
            .unwrap();
    assert!(store.full_text_damage().is_none());
    assert_eq!(store.search_fts("alpha", 5).unwrap().len(), 1);
    cleanup(&db);
}

/// A corrupt statement that reads the index among other tables is index damage
/// only if the index alone cannot be read. Damage to the literal sites is the
/// file's, and `search_literals` never reads the index in any case.
#[test]
fn damage_outside_the_index_is_not_called_index_damage() {
    let db = indexed_store("literals");
    damage(&db, "tbl_name = 'payload_literals'");

    let store = Store::open_read_only(&db).expect("the index is whole, so the open succeeds");
    let Err(error) = store.search_literals("literal-value-02000", true, 5) else {
        panic!("the damaged literal sites cannot be read");
    };
    assert!(Store::sqlite_error_is_corrupt(&error));
    assert!(
        !Store::sqlite_error_is_full_text_damage(&error),
        "damage in the literal sites is not damage to the index: {error:?}"
    );
    // The index still searches.
    assert_eq!(store.search_fts("alpha", 5).unwrap().len(), 1);
    cleanup(&db);
}

/// Damage the index does not account for is never served, tolerant or not:
/// when the index is damaged and so is a table every read needs, the open is
/// refused as damage to the file, untyped.
#[test]
fn damage_wider_than_the_index_is_refused_even_by_a_tolerant_open() {
    let db = indexed_store("wide");
    damage(&db, "name LIKE 'nodes_fts%' OR name = 'pending_state'");

    for memory in [
        ReaderMemory::CURRENT,
        ReaderMemory::CURRENT.tolerating_full_text_damage(),
    ] {
        let error = refused(
            Store::open_read_only_with(&db, memory),
            "damage beyond the index is refused",
        );
        assert!(Store::sqlite_error_is_corrupt(&error), "{error:?}");
        assert!(
            !Store::sqlite_error_is_full_text_damage(&error),
            "must not be called index damage: {error:?}"
        );
    }
    cleanup(&db);
}
