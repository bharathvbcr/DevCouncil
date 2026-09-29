//! `Store::latest_symbols_for_file`: one file's symbols from one generation.
//!
//! GitPulse's code-intel panel lists the symbols of the file a diff touches and
//! compares the generation's `head_sha` with the commit it is showing. These
//! pin the contract that comparison relies on: the rows and the SHA come from
//! the same generation, an empty store is `None` rather than an empty page, an
//! unindexed path is an empty page rather than an error, and a corrupt span is
//! refused the way every other symbol reader refuses it.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::{extract_file, Extraction};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-symbols-for-file-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(store: &Store, extractions: &[Extraction], head_sha: &str) -> u32 {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    store
        .save_generation_with_metadata(
            extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
            head_sha,
        )
        .unwrap()
}

fn corpus(lib_source: &str) -> Vec<Extraction> {
    vec![
        extract_file("src/lib.py", lib_source),
        extract_file("src/other.py", "def elsewhere():\n    return 0\n"),
    ]
}

#[test]
fn an_empty_store_has_no_page_rather_than_an_empty_one() {
    let store = Store::open_in_memory().unwrap();
    assert!(
        store
            .latest_symbols_for_file("src/lib.py")
            .unwrap()
            .is_none(),
        "no generation means no map, which is not the same as a file with no symbols"
    );
}

#[test]
fn rows_are_the_files_own_symbols_in_source_order_with_their_generation() {
    let store = Store::open_in_memory().unwrap();
    let generation = save(
        &store,
        &corpus("def second():\n    return 2\n\ndef first():\n    return 1\n"),
        "abc123",
    );

    let page = store
        .latest_symbols_for_file("src/lib.py")
        .unwrap()
        .expect("a generation exists");
    assert_eq!(page.generation, generation);
    assert_eq!(page.head_sha, "abc123");

    let names: Vec<&str> = page.rows.iter().map(|row| row.name.as_str()).collect();
    assert!(
        names.contains(&"second") && names.contains(&"first"),
        "both declarations must be listed: {names:?}"
    );
    assert!(
        !names.contains(&"elsewhere"),
        "another file's symbol leaked into the page: {names:?}"
    );
    assert!(
        page.rows.iter().all(|row| row.path == "src/lib.py"),
        "every row must belong to the requested file: {:?}",
        page.rows
    );
    assert!(
        page.rows
            .windows(2)
            .all(|pair| pair[0].span_start <= pair[1].span_start),
        "rows must be in source order: {:?}",
        page.rows
    );
    assert!(
        page.rows.iter().all(|row| row.span_end > row.span_start),
        "spans must be real: {:?}",
        page.rows
    );

    // The same rows every other symbol reader returns for this file.
    let mut from_all: Vec<_> = store
        .all_symbols()
        .unwrap()
        .into_iter()
        .filter(|row| row.path == "src/lib.py")
        .collect();
    let mut from_page = page.rows.clone();
    from_all.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    from_page.sort_by(|a, b| a.qualified_name.cmp(&b.qualified_name));
    assert_eq!(from_page, from_all);
}

#[test]
fn an_unindexed_path_is_an_empty_page_not_a_missing_map() {
    let store = Store::open_in_memory().unwrap();
    let generation = save(&store, &corpus("def a():\n    return 1\n"), "abc123");

    let page = store
        .latest_symbols_for_file("src/never_indexed.py")
        .unwrap()
        .expect("the generation exists even though this path is not in it");
    assert_eq!(page.generation, generation);
    assert_eq!(page.head_sha, "abc123");
    assert!(page.rows.is_empty(), "{:?}", page.rows);
}

#[test]
fn the_page_describes_the_latest_generation_only() {
    let store = Store::open_in_memory().unwrap();
    save(&store, &corpus("def old_name():\n    return 1\n"), "first");
    let latest = save(&store, &corpus("def new_name():\n    return 1\n"), "second");

    let page = store
        .latest_symbols_for_file("src/lib.py")
        .unwrap()
        .expect("a generation exists");
    assert_eq!(page.generation, latest);
    assert_eq!(
        page.head_sha, "second",
        "the SHA must name the generation the rows came from"
    );
    let names: Vec<&str> = page.rows.iter().map(|row| row.name.as_str()).collect();
    assert!(names.contains(&"new_name"), "{names:?}");
    assert!(
        !names.contains(&"old_name"),
        "a superseded generation's symbol was returned: {names:?}"
    );
}

/// Same policy as S-11 in `store_hardening.rs`: a negative stored span is
/// refused, never clamped to 0 and published as real.
#[test]
fn a_corrupt_span_is_refused_rather_than_published_as_zero() {
    let dir = tmp_dir("corrupt-span");
    let db_path = dir.join("index.sqlite");
    let store = Store::open(&db_path).unwrap();
    save(&store, &corpus("def a():\n    return 1\n"), "abc123");
    assert!(!store
        .latest_symbols_for_file("src/lib.py")
        .unwrap()
        .expect("a generation exists")
        .rows
        .is_empty());

    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE generation_nodes SET span_start = -5", [])
            .unwrap();
    }

    assert!(
        store.latest_symbols_for_file("src/lib.py").is_err(),
        "a corrupt span must fail the read"
    );
    let _ = fs::remove_dir_all(&dir);
}
