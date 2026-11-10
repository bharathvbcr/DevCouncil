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
use devmap_serve::repo_scope::{DamageKind, DamageStamp};
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

/// Overwrite only the root pages of the objects `filter` selects from
/// `sqlite_master`, so every other read succeeds and the damage is met inside
/// those objects alone.
fn damage_roots(db: &Path, filter: &str) -> Vec<u8> {
    let original = std::fs::read(db).unwrap();
    let page = page_size(&original);
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
    for root in roots {
        overwrite(db, (root as usize - 1) * page, page);
    }
    original
}

/// The full-text index's tables.
fn damage_full_text_index(db: &Path) -> Vec<u8> {
    damage_roots(db, "name LIKE 'nodes_fts%'")
}

/// The literal sites, which no search reads through the full-text index.
fn damage_payload_literals(db: &Path) -> Vec<u8> {
    damage_roots(db, "tbl_name = 'payload_literals'")
}

/// A store whose one file carries many literal sites, so `payload_literals`
/// spans pages of its own.
fn plant_literal_store(root: &Path) -> PathBuf {
    let store = devmap_extract::paths::store_path(root);
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    let mut extraction = devmap_extract::extract_file("app.py", "def helper():\n    return 1\n");
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
    let writer = Store::open(&store).expect("create store");
    writer
        .save_generation(&extractions, &resolution, &analysis)
        .expect("generation");
    drop(writer);
    store.canonicalize().unwrap()
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

/// Corruption met only inside the full-text index refuses search, names the
/// index and its real repair (`devmap repair --fts`), and leaves every other
/// tool answering from the same handle. (Expectation changed: this used to
/// assert the whole-store `devmap doctor` / `devmap build` text and that every
/// call, status included, was refused as "has not changed since".)
#[tokio::test]
async fn damage_met_only_in_the_full_text_index_refuses_search_and_nothing_else() {
    let root = scratch("fts");
    let db = plant_indexed_store(&root);
    let slot = Arc::new(StoreSlot::new(&db));
    let cached = slot.get().expect("opens and caches the store");
    let original = damage_full_text_index(&db);

    let text = error_text(&call(&slot, "devmap_search", json!({"query": "helper"})).await);
    assert!(
        text.contains(&db.display().to_string()),
        "must name the store {}: {text}",
        db.display()
    );
    assert!(
        text.contains("full-text index") && text.contains("devmap repair --fts"),
        "must name the index and its repair: {text}"
    );
    assert!(
        text.contains("clears when the file changes, or after 30 seconds"),
        "must say how the refusal clears: {text}"
    );
    assert!(
        text.contains("the first failure on this call was"),
        "the failure the call first met is kept beside the one that replaced it: {text}"
    );

    // The same handle still answers the tools that do not read the index.
    let status = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(status["result"]["isError"], json!(false), "{status}");
    let graph = call(&slot, "devmap_impact", json!({"target": "helper"})).await;
    assert_eq!(graph["result"]["isError"], json!(false), "{graph}");

    // A second search is refused with the same words, and the handle was not
    // replaced on the way (no reopen per refused call).
    let again = error_text(&call(&slot, "devmap_search", json!({"query": "helper"})).await);
    assert!(again.contains("devmap repair --fts"), "{again}");
    assert!(
        Arc::ptr_eq(&cached, &slot.get().unwrap()),
        "a full-text refusal must keep the working handle"
    );

    // Repair clears it.
    restore_in_place(&db, &original);
    let healed = call(&slot, "devmap_search", json!({"query": "helper"})).await;
    assert_eq!(healed["result"]["isError"], json!(false), "{healed}");
    std::fs::remove_dir_all(root).unwrap();
}

/// A server that has never held the store (a restart, the ten-minute idle
/// eviction, another `devmap mcp` process) opens a file whose full-text index
/// is damaged. Opening validates `nodes_fts` too, and that failed the whole
/// open: every tool was refused with the whole-store text. The store opens
/// degraded instead, the tools that do not read the index answer, and search
/// is refused naming the index and its repair.
#[tokio::test]
async fn a_store_first_opened_with_a_damaged_full_text_index_answers_everything_but_search() {
    let root = scratch("fts-fresh");
    let db = plant_indexed_store(&root);
    let original = damage_full_text_index(&db);
    let slot = Arc::new(StoreSlot::new(&db)); // no prior get()

    let status = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(status["result"]["isError"], json!(false), "{status}");
    let graph = call(&slot, "devmap_impact", json!({"target": "helper"})).await;
    assert_eq!(graph["result"]["isError"], json!(false), "{graph}");

    let text = error_text(&call(&slot, "devmap_search", json!({"query": "helper"})).await);
    assert!(text.contains(&db.display().to_string()), "{text}");
    assert!(
        text.contains("full-text index") && text.contains("devmap repair --fts"),
        "must name the index and its repair: {text}"
    );
    assert!(
        text.contains("clears when the file changes, or after 30 seconds"),
        "must say how the refusal clears: {text}"
    );

    // The refusal does not reopen the file: the handle stays.
    let held = slot.get().expect("the degraded handle is cached");
    let again = error_text(&call(&slot, "devmap_search", json!({"query": "helper"})).await);
    assert!(again.contains("devmap repair --fts"), "{again}");
    assert!(
        Arc::ptr_eq(&held, &slot.get().unwrap()),
        "a refused search must keep the handle"
    );

    restore_in_place(&db, &original);
    let healed = call(&slot, "devmap_search", json!({"query": "helper"})).await;
    assert_eq!(healed["result"]["isError"], json!(false), "{healed}");
    std::fs::remove_dir_all(root).unwrap();
}

/// `devmap_literals` never reads `nodes_fts`. Damage in the literal sites is
/// damage to the file, not to the index: the refusal must not send the agent to
/// `devmap repair --fts`, which cannot touch it, nor promise that every other
/// tool still answers.
#[tokio::test]
async fn damage_in_the_literal_sites_is_file_damage_and_not_index_damage() {
    let root = scratch("literals");
    let db = plant_literal_store(&root);
    let slot = Arc::new(StoreSlot::new(&db));
    slot.get().expect("opens and caches the store");
    damage_payload_literals(&db);

    let text = error_text(
        &call(
            &slot,
            "devmap_literals",
            json!({"query": "literal-value-02000", "exact": true}),
        )
        .await,
    );
    assert!(text.contains(&db.display().to_string()), "{text}");
    assert!(
        !text.contains("repair --fts") && !text.contains("every other tool still answers"),
        "damage outside the index must not be called index damage: {text}"
    );
    assert!(
        text.contains("devmap doctor") && text.contains("devmap build"),
        "the whole-file remedy: {text}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// The verdict is remembered only if the file is in the state it was in before
/// the attempt that failed. A rebuild that finished in between replaced the
/// file the attempt read; refusing the new file on the old verdict would be wrong.
#[tokio::test]
async fn damage_is_remembered_only_for_the_file_state_the_attempt_started_from() {
    let root = scratch("stamp");
    let db = plant_store(&root);
    let slot = Arc::new(StoreSlot::new(&db));

    // The file changed after the stamp was taken: the verdict is dropped.
    let stamp = DamageStamp::of(&db);
    assert!(stamp.is_some());
    let file = std::fs::OpenOptions::new().write(true).open(&db).unwrap();
    let modified = file.metadata().unwrap().modified().unwrap();
    file.set_modified(modified + std::time::Duration::from_secs(5))
        .unwrap();
    drop(file);
    slot.remember_damage(
        &db,
        DamageKind::File,
        stamp,
        "damage seen before the rebuild",
    );
    let answered = call(&slot, "devmap_status", json!({})).await;
    assert_eq!(
        answered["result"]["isError"],
        json!(false),
        "a verdict about a file that has since changed is not applied: {answered}"
    );

    // The file is as it was: the verdict stands and is stated with its cause.
    let stamp = DamageStamp::of(&db);
    slot.remember_damage(&db, DamageKind::File, stamp, "the page that was bad");
    let refused = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(refused.contains("the page that was bad"), "{refused}");
    std::fs::remove_dir_all(root).unwrap();
}

/// A reopen that fails keeps the failure the call first saw, and the refusal
/// that follows carries the remembered cause and what clears it.
#[tokio::test]
async fn a_second_failure_keeps_the_first_and_a_remembered_refusal_keeps_the_cause() {
    let root = scratch("first-failure");
    let db = plant_store(&root);
    let slot = Arc::new(StoreSlot::new(&db));
    slot.get().expect("opens and caches the store");
    damage_in_place(&db);

    let first = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(
        first.contains("the first failure on this call was"),
        "the first failure is kept when the reopen fails too: {first}"
    );
    let cause = first
        .split("SQLITE_CORRUPT (")
        .nth(1)
        .and_then(|rest| rest.split("), ").next())
        .unwrap_or_else(|| panic!("no cause in {first}"))
        .to_string();

    let remembered = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(
        remembered.contains(&cause),
        "the remembered refusal carries the cause {cause:?}: {remembered}"
    );
    assert!(
        remembered.contains("clears when the file changes, or after 30 seconds"),
        "{remembered}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// The closest real test of "the file was replaced while the call was
/// failing": the retry race itself cannot be arranged from outside the server,
/// so this replaces the damaged file by rename (a new inode, as `devmap build`
/// does) after the failure was reported and remembered. The very next call
/// reads the new file; nothing remembered about the old one is applied to it.
#[tokio::test]
async fn a_damaged_store_replaced_by_rename_is_read_again_at_once() {
    let root = scratch("replaced");
    let db = plant_store(&root);
    let healthy_copy = root.join("healthy-copy.sqlite");
    std::fs::copy(&db, &healthy_copy).unwrap();
    let slot = Arc::new(StoreSlot::new(&db));
    slot.get().expect("opens and caches the store");
    damage_in_place(&db);

    let text = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(text.contains(&db.display().to_string()), "{text}");
    // A second refusal is served from the memory and carries the cause and
    // what clears it.
    let again = error_text(&call(&slot, "devmap_status", json!({})).await);
    assert!(
        again.contains("has not changed since")
            && again.contains("clears when the file changes, or after 30 seconds"),
        "the remembered refusal says that it clears when the file changes: {again}"
    );

    std::fs::rename(&healthy_copy, &db).unwrap();
    let healed = call(&slot, "devmap_status", json!({})).await;
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
