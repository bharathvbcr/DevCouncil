//! A cached store connection that reports `SQLITE_CORRUPT` is never served a
//! second time.
//!
//! On 2026-11-09 agents querying a long-lived `devmap mcp` got "database disk
//! image is malformed" for a store the CLI read without complaint. Whatever
//! the cause on the day, the server's contract is the same: evict the
//! connection that saw damage, reopen the file once, retry; and when the file
//! itself is damaged, name the store and the next step instead of forwarding
//! SQLite's words alone, and do not re-read a file that has not changed.
//!
//! The damage here is real, not simulated: pages of an on-disk store are
//! overwritten in place through a second handle (same inode, so the cache's
//! identity check still passes), which makes SQLite itself return
//! `SQLITE_CORRUPT` for any page the connection has not already cached. The
//! retry-succeeds path is covered in `mcp.rs` (`damage_recovery_tests`),
//! because a connection that is stale over a whole file cannot be made in one
//! call from outside the server.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-mcp-corrupt-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn plant_store(root: &Path) -> PathBuf {
    let store = devmap_extract::paths::store_path(root);
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let _ = Store::open(&store).expect("create store");
    store.canonicalize().unwrap()
}

/// A store holding one real generation, so `devmap_search` reads the
/// full-text index.
fn plant_indexed_store(root: &Path) -> PathBuf {
    let store = devmap_extract::paths::store_path(root);
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let extractions = vec![devmap_extract::extract_file(
        "app.py",
        "def helper():\n    return 1\n\n\ndef main():\n    return helper()\n",
    )];
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let writer = Store::open(&store).expect("create store");
    writer
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");
    drop(writer);
    store.canonicalize().unwrap()
}

async fn call(store: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    handle_line(store, &frame.to_string())
        .await
        .expect("a request must produce a response frame")
}

fn error_text(response: &Value) -> String {
    assert_eq!(response["result"]["isError"], json!(true), "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn page_size(bytes: &[u8]) -> usize {
    // Two big-endian bytes at offset 16 of the header; 1 means 65536.
    let raw = u16::from_be_bytes([bytes[16], bytes[17]]);
    if raw == 1 {
        65_536
    } else {
        raw as usize
    }
}

fn overwrite(db: &Path, from: usize, len: usize) {
    use std::io::{Seek, SeekFrom, Write};
    let mut file = std::fs::OpenOptions::new().write(true).open(db).unwrap();
    file.seek(SeekFrom::Start(from as u64)).unwrap();
    file.write_all(&vec![0xFF; len]).unwrap();
    file.sync_all().unwrap();
}

/// Overwrite every page after the first with 0xFF, in place. Returns the
/// original bytes so a test can put them back.
fn damage_in_place(db: &Path) -> Vec<u8> {
    let original = std::fs::read(db).unwrap();
    let page = page_size(&original);
    assert!(
        original.len() > 2 * page,
        "precondition: the store has pages beyond the first"
    );
    overwrite(db, page, original.len() - page);
    original
}

/// Overwrite only the root pages of the full-text index's tables, so every
/// other read in a search succeeds and the damage is met inside `nodes_fts`.
fn damage_full_text_index(db: &Path) -> Vec<u8> {
    let original = std::fs::read(db).unwrap();
    let page = page_size(&original);
    let roots: Vec<i64> = {
        let conn =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT rootpage FROM sqlite_master WHERE name LIKE 'nodes_fts%' AND rootpage > 1",
            )
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert!(!roots.is_empty(), "precondition: the index has root pages");
    for root in roots {
        overwrite(db, (root as usize - 1) * page, page);
    }
    original
}

fn restore_in_place(db: &Path, original: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open(db).unwrap();
    file.write_all(original).unwrap();
    file.sync_all().unwrap();
}

/// The file itself is damaged. The agent is told which store and what to run,
/// not only SQLite's words; and once the file is whole again the next call
/// answers, because nothing damaged stayed cached.
#[tokio::test]
async fn a_damaged_store_file_is_reported_by_name_with_the_next_step_and_not_cached() {
    let root = scratch("file");
    let db = plant_store(&root);
    let slot = Arc::new(StoreSlot::new(&db));
    slot.get().expect("opens and caches the store");
    let original = damage_in_place(&db);

    let text = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(
        text.contains(&db.display().to_string()),
        "must name the store {}: {text}",
        db.display()
    );
    assert!(
        text.contains("devmap doctor") && text.contains("devmap build"),
        "must name the next step: {text}"
    );

    restore_in_place(&db, &original);
    let healed = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(
        healed["result"]["isError"],
        json!(false),
        "a repaired file is answered from, not refused from a cached failure: {healed}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// Corruption met inside the full-text index used to reach the agent as the
/// `repair --fts` text with no store path, no eviction and no retry, because
/// the store re-worded the SQLite error and dropped its code.
#[tokio::test]
async fn damage_met_in_the_full_text_index_is_evicted_retried_and_named() {
    let root = scratch("fts");
    let db = plant_indexed_store(&root);
    let slot = Arc::new(StoreSlot::new(&db));
    slot.get().expect("opens and caches the store");
    let original = damage_full_text_index(&db);

    let first = call(&slot, "devmap_search", json!({"query": "helper"})).await;
    let text = error_text(&first);
    assert!(
        text.contains(&db.display().to_string()),
        "must name the store {}: {text}",
        db.display()
    );
    assert!(
        text.contains("devmap doctor") && text.contains("devmap build"),
        "must name the next step: {text}"
    );
    assert!(
        text.contains("opened afresh"),
        "must say the retry on a fresh connection also failed: {text}"
    );

    // The file has not changed: the next call is refused without reading it.
    let again = error_text(&call(&slot, "devmap_search", json!({"query": "helper"})).await);
    assert!(
        again.contains("has not changed since"),
        "a file already found damaged and unchanged is not opened and queried again: {again}"
    );

    // A change to the file clears the memory and the next call reads it.
    restore_in_place(&db, &original);
    let healed = call(&slot, "devmap_search", json!({"query": "helper"})).await;
    assert_eq!(healed["result"]["isError"], json!(false), "{healed}");
    std::fs::remove_dir_all(root).unwrap();
}

/// A store the caller handed in already open may exist nowhere else. Its
/// corruption is reported, and it is neither evicted nor swapped for a reopen
/// of whatever is at its path.
#[tokio::test]
async fn a_caller_owned_store_that_reports_corrupt_is_reported_and_kept() {
    let root = scratch("owned");
    let db = plant_store(&root);
    let damaged_copy = root.join("damaged.sqlite");
    std::fs::copy(&db, &damaged_copy).unwrap();
    let handed_in = Arc::new(Store::open_read_only(&damaged_copy).expect("opens while whole"));
    damage_in_place(&damaged_copy);

    let slot = Arc::new(StoreSlot::ready(&db, handed_in));
    let text = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(text.contains("handed"), "{text}");
    assert!(text.contains(&db.display().to_string()), "{text}");
    // Kept, not evicted: the whole file at the path was not swapped in.
    let again = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(again.contains("handed"), "{again}");
    std::fs::remove_dir_all(root).unwrap();
}
