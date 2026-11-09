//! A cached store connection that reports `SQLITE_CORRUPT` is never served a
//! second time.
//!
//! On 2026-11-09 agents querying a long-lived `devmap mcp` got "database disk
//! image is malformed" for a store the CLI read without complaint. Whatever
//! the cause on the day, the server's contract is the same: evict the
//! connection that saw damage, reopen the file once, retry; and when the file
//! itself is damaged, name the store and the next step instead of forwarding
//! SQLite's words alone.
//!
//! The damage here is real, not simulated: pages of an on-disk store are
//! overwritten in place through a second handle (same inode, so the cache's
//! identity check still passes), which makes SQLite itself return
//! `SQLITE_CORRUPT` for any page the connection has not already cached.

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

/// Overwrite every page after the first with 0xFF, in place. Returns the
/// original bytes so a test can put them back.
fn damage_in_place(db: &Path) -> Vec<u8> {
    use std::io::{Seek, SeekFrom, Write};
    let original = std::fs::read(db).unwrap();
    // The page size is two big-endian bytes at offset 16 of the header; 1 means 65536.
    let raw = u16::from_be_bytes([original[16], original[17]]);
    let page = if raw == 1 { 65_536 } else { raw as usize };
    assert!(
        original.len() > 2 * page,
        "precondition: the store has pages beyond the first"
    );
    let mut file = std::fs::OpenOptions::new().write(true).open(db).unwrap();
    file.seek(SeekFrom::Start(page as u64)).unwrap();
    file.write_all(&vec![0xFF; original.len() - page]).unwrap();
    file.sync_all().unwrap();
    original
}

fn restore_in_place(db: &Path, original: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().write(true).open(db).unwrap();
    file.write_all(original).unwrap();
    file.sync_all().unwrap();
}

/// The connection is the damaged party; the file at the path is whole (a
/// handle that holds pages the file no longer has). The call must evict it,
/// reopen once and answer from the file.
#[tokio::test]
async fn a_corrupt_cached_connection_is_evicted_and_the_call_retried_on_a_fresh_one() {
    let root = scratch("handle");
    let db = plant_store(&root);
    let damaged_copy = root.join("damaged.sqlite");
    std::fs::copy(&db, &damaged_copy).unwrap();
    let stale_handle = Arc::new(Store::open_read_only(&damaged_copy).expect("opens while whole"));
    damage_in_place(&damaged_copy);

    let slot = Arc::new(StoreSlot::ready(&db, stale_handle));
    let response = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(
        response["result"]["isError"],
        json!(false),
        "the whole file at the path must answer after one reopen: {response}"
    );
    // The damaged handle is gone: a second call is an ordinary hit on the
    // connection the retry opened.
    let again = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(again["result"]["isError"], json!(false), "{again}");
    std::fs::remove_dir_all(root).unwrap();
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

    let response = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(response["result"]["isError"], json!(true), "{response}");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
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
