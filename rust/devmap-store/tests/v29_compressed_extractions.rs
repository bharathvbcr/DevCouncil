//! Schema v29: a payload's extraction document is stored as a zstd frame.
//!
//! What a bump that changes the storage class of the biggest column in the
//! store can get wrong, one test each:
//!
//! * a store opened at 29 holds frames, not text, and says its readers need 29;
//! * every reader answers the same after a v28 store is migrated as it did on
//!   a store written natively (`latest_extractions`, `latest_extraction_for_path`,
//!   `latest_test_entry_symbols`, the extraction cache's generation fallback),
//!   and the payload digests, which are payload identity, do not move;
//! * a migration that meets a payload it cannot convert refuses the store and
//!   leaves it at 28, as it was;
//! * a store left half converted (text rows, or no entry wiring) converges on
//!   the next writer open, and the build after it reuses every payload;
//! * a frame that is damaged, truncated, forged to declare an absurd size,
//!   or chained to a second frame is a refusal naming the path, never an empty
//!   or partial extraction;
//! * a payload with no stored entry wiring, as a foreign writer leaves it,
//!   still answers `latest_test_entry_symbols` correctly.
//!
//! **What each test proves against the schema-28 tree.** This file needs the
//! `zstd` crate (to put a v28 shape back by hand and to forge frames) and
//! `entry_wiring_json`, neither of which exists there, so it does not compile
//! on the base, and every test in it is *precondition-only* as a fail-before:
//! it cannot run there, whatever it asserts. The behavioural fail-before
//! claims, which need no v29 API, live in `v29_size_and_hot_path.rs` (stored
//! documents are a fraction of the JSON they hold; the test-entry query reads no
//! document) and were run against the schema-28 tree. What this file adds is
//! what only the v29 API can state: the migration, the heal, the refusals.
//!
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store, CURRENT_SCHEMA_VERSION, MIN_READER_SCHEMA_VERSION};
use rusqlite::{params, Connection};

const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-v29-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Python, because it is the cheapest language to extract with every kind of
/// document this schema stores: a test function (entry wiring), a literal, a
/// class, and a module the others import.
fn sources() -> Vec<(String, String)> {
    let mut files = vec![
        (
            "ledger.py".to_string(),
            "LEDGER_NAME = \"ledger\"\n\n\ndef ledger_total(rows):\n    return sum(rows)\n"
                .to_string(),
        ),
        (
            "checks.py".to_string(),
            "from ledger import ledger_total\n\n\ndef test_sums():\n    \
             assert ledger_total([1]) == 1\n\n\ndef summary(rows):\n    return ledger_total(rows)\n"
                .to_string(),
        ),
        (
            "prose.py".to_string(),
            "def note():\n    \"\"\"RuntimeEntryPoint is only prose here.\"\"\"\n    return 1\n"
                .to_string(),
        ),
    ];
    for index in 0..12 {
        files.push((
            format!("pkg/mod_{index}.py"),
            format!(
                "LABEL_{index} = \"label-{index}\"\n\n\ndef leaf_{index}():\n    return {index}\n\n\n\
                 class Holder{index}:\n    def get(self):\n        return leaf_{index}()\n"
            ),
        ));
    }
    files
}

fn extractions() -> Vec<Extraction> {
    sources()
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect()
}

fn commit(store: &Store, extractions: &[Extraction]) -> u32 {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions).unwrap();
    let analysis = analyze(extractions, &resolution);
    store
        .save_generation_with_opts(
            extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts::default(),
        )
        .unwrap()
}

fn written_store(label: &str) -> (PathBuf, PathBuf) {
    let dir = tmp_dir(label);
    let db = dir.join("devmap.sqlite");
    let store = Store::open(&db).unwrap();
    commit(&store, &extractions());
    drop(store);
    (dir, db)
}

fn one<T: rusqlite::types::FromSql>(db: &Path, sql: &str) -> T {
    Connection::open(db)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

/// What every reader answers, in a form two stores can be compared by.
#[derive(Debug, PartialEq)]
struct Answers {
    extractions: Vec<String>,
    for_checks: String,
    test_entry_symbols: BTreeSet<String>,
    cached: Option<String>,
}

fn answers(db: &Path) -> Answers {
    let store = Store::open_read_only(db).unwrap();
    let extractions = store
        .latest_extractions()
        .unwrap()
        .iter()
        .map(|extraction| serde_json::to_string(extraction).unwrap())
        .collect();
    let for_checks = serde_json::to_string(
        &store
            .latest_extraction_for_path("checks.py")
            .unwrap()
            .expect("checks.py is indexed"),
    )
    .unwrap();
    let key = devmap_extract::cache::CacheKey::for_extraction(&extract_file(
        "checks.py",
        &sources()[1].1,
    ));
    let cached = store
        .try_get_cached_extraction(&key)
        .unwrap()
        .map(|extraction| serde_json::to_string(&extraction).unwrap());
    Answers {
        extractions,
        for_checks,
        test_entry_symbols: store
            .latest_test_entry_symbols()
            .unwrap()
            .into_iter()
            .collect(),
        cached,
    }
}

fn digests(db: &Path) -> Vec<(String, Vec<u8>)> {
    let conn = Connection::open(db).unwrap();
    let rows = conn
        .prepare(
            "SELECT p.path, fp.payload_digest FROM file_payloads fp
               JOIN paths p ON p.id = fp.file_id ORDER BY fp.payload_id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows
}

/// Put every payload back to the text v28 wrote, remove the v29 column and
/// stamp the store 28: the store a v28 binary left.
fn reduce_to_v28(db: &Path) {
    let conn = Connection::open(db).unwrap();
    let rows: Vec<(i64, Vec<u8>)> = conn
        .prepare("SELECT payload_id, extraction_json FROM file_payloads")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for (id, frame) in rows {
        let text = String::from_utf8(zstd::bulk::decompress(&frame, 64 << 20).unwrap()).unwrap();
        conn.execute(
            "UPDATE file_payloads SET extraction_json = ?2 WHERE payload_id = ?1",
            params![id, text],
        )
        .unwrap();
    }
    conn.execute_batch(
        "ALTER TABLE file_payloads DROP COLUMN entry_wiring_json;
         UPDATE reader_compat SET min_reader_schema = 28;
         PRAGMA user_version = 28;",
    )
    .unwrap();
}

#[test]
fn a_new_store_holds_every_extraction_as_a_frame_with_its_entry_wiring() {
    let (dir, db) = written_store("frames");
    assert_eq!(CURRENT_SCHEMA_VERSION, 29);
    assert_eq!(MIN_READER_SCHEMA_VERSION, 29);
    assert_eq!(
        one::<i64>(&db, "PRAGMA user_version"),
        i64::from(CURRENT_SCHEMA_VERSION)
    );
    assert_eq!(
        one::<i64>(&db, "SELECT min_reader_schema FROM reader_compat"),
        29,
        "a v28 reader cannot read a compressed payload, so the floor is the new schema"
    );
    let payloads = one::<i64>(&db, "SELECT COUNT(*) FROM file_payloads");
    assert_eq!(payloads, sources().len() as i64);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) = 'blob'"
        ),
        payloads,
        "every extraction is a BLOB"
    );
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads
              WHERE substr(extraction_json, 1, 4) = x'28b52ffd'"
        ),
        payloads,
        "and a zstd frame (magic 28 b5 2f fd)"
    );
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE entry_wiring_json IS NULL"
        ),
        0,
        "every payload says whether it has entry wiring: '[]' is an answer, NULL is not"
    );
    assert_eq!(
        one::<String>(
            &db,
            "SELECT fp.entry_wiring_json FROM file_payloads fp JOIN paths p ON p.id = fp.file_id
              WHERE p.path = 'ledger.py'"
        ),
        "[]"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_readers_answer_the_same_after_a_v28_store_is_migrated() {
    let (dir, db) = written_store("parity");
    let native = answers(&db);
    let native_digests = digests(&db);
    assert!(
        native
            .test_entry_symbols
            .iter()
            .any(|symbol| symbol.ends_with("test_sums")),
        "the fixture must have an entry point for the comparison to mean anything: {native:?}"
    );
    assert!(
        !native
            .test_entry_symbols
            .iter()
            .any(|symbol| symbol.contains("note")),
        "prose that mentions RuntimeEntryPoint is not one"
    );
    assert!(native.cached.is_some(), "the cache fallback must hit");

    reduce_to_v28(&db);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) = 'text'"
        ),
        sources().len() as i64,
        "the reduction leaves the text v28 stored"
    );

    drop(Store::open(&db).expect("a v28 store migrates"));

    assert_eq!(one::<i64>(&db, "PRAGMA user_version"), 29);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) = 'blob'
               AND entry_wiring_json IS NOT NULL"
        ),
        sources().len() as i64
    );
    assert_eq!(answers(&db), native, "every reader answers as it did");
    assert_eq!(
        digests(&db),
        native_digests,
        "a payload's identity is the digest of its text; compressing it moves nothing"
    );
    assert_eq!(
        one::<i64>(&db, "PRAGMA freelist_count"),
        0,
        "the pages the rewrite freed went back to the filesystem, not to a freelist"
    );
    assert_eq!(
        one::<i64>(&db, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
        0
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_migration_that_meets_an_unreadable_payload_leaves_the_v28_store_as_it_was() {
    let (dir, db) = written_store("atomic");
    reduce_to_v28(&db);
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE file_payloads SET extraction_json = '{not json'
              WHERE file_id = (SELECT id FROM paths WHERE path = 'checks.py')",
            [],
        )
        .unwrap();
    }
    let error = Store::open(&db)
        .err()
        .expect("a payload that is not JSON cannot be migrated")
        .to_string();
    assert!(error.contains("schema 29"), "{error}");
    assert!(
        error.contains("checks.py"),
        "the refusal names the file: {error}"
    );
    assert_eq!(
        one::<i64>(&db, "PRAGMA user_version"),
        28,
        "the stamp did not advance"
    );
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM pragma_table_info('file_payloads')
              WHERE name = 'entry_wiring_json'"
        ),
        0,
        "the rung's column rolled back with it"
    );
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) = 'text'"
        ),
        sources().len() as i64,
        "no row was compressed: the migration is one transaction"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_store_left_half_converted_converges_on_open_and_the_next_build_reuses_every_payload() {
    for left in ["text rows", "no entry wiring", "both"] {
        let (dir, db) = written_store("resume");
        let native = answers(&db);
        let native_digests = digests(&db);
        {
            let conn = Connection::open(&db).unwrap();
            if left != "no entry wiring" {
                // The first half of the payloads back to text, as a pass
                // interrupted between rows would leave them.
                let rows: Vec<(i64, Vec<u8>)> = conn
                    .prepare("SELECT payload_id, extraction_json FROM file_payloads")
                    .unwrap()
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<Result<_, _>>()
                    .unwrap();
                for (id, frame) in rows.into_iter().take(sources().len() / 2) {
                    let text = String::from_utf8(zstd::bulk::decompress(&frame, 64 << 20).unwrap())
                        .unwrap();
                    conn.execute(
                        "UPDATE file_payloads SET extraction_json = ?2 WHERE payload_id = ?1",
                        params![id, text],
                    )
                    .unwrap();
                }
            }
            if left != "text rows" {
                conn.execute("UPDATE file_payloads SET entry_wiring_json = NULL", [])
                    .unwrap();
            }
        }
        // The readers never wait for the heal: a half-converted store is read
        // correctly as it stands.
        assert_eq!(answers(&db), native, "{left}: read before the heal");

        drop(Store::open(&db).unwrap());
        assert_eq!(
            one::<i64>(
                &db,
                "SELECT COUNT(*) FROM file_payloads
                  WHERE typeof(extraction_json) <> 'blob' OR entry_wiring_json IS NULL"
            ),
            0,
            "{left}: the open converges the store"
        );
        assert_eq!(answers(&db), native, "{left}: read after the heal");
        assert_eq!(digests(&db), native_digests, "{left}: identity unchanged");

        let before = one::<i64>(&db, "SELECT COUNT(*) FROM file_payloads");
        let store = Store::open(&db).unwrap();
        commit(&store, &extractions());
        drop(store);
        assert_eq!(
            one::<i64>(&db, "SELECT COUNT(*) FROM file_payloads"),
            before,
            "{left}: an unchanged tree written again reuses every payload"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

#[test]
fn a_payload_without_stored_entry_wiring_still_answers_the_test_entry_query() {
    let (dir, db) = written_store("foreign");
    let native = answers(&db);
    Connection::open(&db)
        .unwrap()
        .execute("UPDATE file_payloads SET entry_wiring_json = NULL", [])
        .unwrap();
    // Read-only: no writer open heals it, so the reader itself has to answer.
    assert_eq!(answers(&db).test_entry_symbols, native.test_entry_symbols);
    let _ = fs::remove_dir_all(&dir);
}

fn replace_frame(db: &Path, path: &str, frame: &[u8]) {
    let changed = Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE file_payloads SET extraction_json = ?2
              WHERE file_id = (SELECT id FROM paths WHERE path = ?1)",
            params![path, frame],
        )
        .unwrap();
    assert_eq!(changed, 1);
}

fn frame_of(db: &Path, path: &str) -> Vec<u8> {
    Connection::open(db)
        .unwrap()
        .query_row(
            "SELECT extraction_json FROM file_payloads
              WHERE file_id = (SELECT id FROM paths WHERE path = ?1)",
            params![path],
            |row| row.get(0),
        )
        .unwrap()
}

/// Every reader of the damaged payload refuses, by path, and none returns an
/// extraction: a payload that will not decode is a fault, not an empty file.
fn assert_refused(db: &Path, what: &str, expect: &str) {
    let store = Store::open_read_only(db).unwrap();
    let one_path = store
        .latest_extraction_for_path("checks.py")
        .err()
        .unwrap_or_else(|| panic!("{what}: latest_extraction_for_path returned an answer"))
        .to_string();
    assert!(one_path.contains("checks.py"), "{what}: {one_path}");
    assert!(one_path.contains(expect), "{what}: {one_path}");
    let all = store
        .latest_extractions()
        .err()
        .unwrap_or_else(|| panic!("{what}: latest_extractions returned an answer"))
        .to_string();
    assert!(all.contains("checks.py"), "{what}: {all}");
}

#[test]
fn a_truncated_or_corrupt_frame_is_refused_naming_the_path() {
    let (dir, db) = written_store("corrupt");
    let frame = frame_of(&db, "checks.py");
    assert!(frame.starts_with(&ZSTD_MAGIC));

    replace_frame(&db, "checks.py", &frame[..frame.len() / 2]);
    assert_refused(&db, "truncated", "did not decompress");

    let mut flipped = frame.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0xFF;
    flipped[middle + 1] ^= 0xFF;
    replace_frame(&db, "checks.py", &flipped);
    assert_refused(&db, "bit-flipped", "decompress");

    replace_frame(&db, "checks.py", &[0x01, 0x02, 0x03]);
    assert_refused(&db, "not a frame", "not a zstd frame");

    replace_frame(&db, "checks.py", &[]);
    assert_refused(&db, "empty blob", "zstd frame");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_frame_declaring_more_than_the_limit_is_refused_before_anything_is_allocated() {
    let (dir, db) = written_store("bomb");
    // A header that declares 1 TiB: magic, a descriptor with an 8-byte frame
    // content size and the single-segment flag, the size, an empty last block.
    let mut forged = ZSTD_MAGIC.to_vec();
    forged.push(0xE0);
    forged.extend_from_slice(&(1u64 << 40).to_le_bytes());
    forged.extend_from_slice(&[0x01, 0x00, 0x00]);
    replace_frame(&db, "checks.py", &forged);
    assert_refused(&db, "forged 1 TiB header", "limit");

    // A real frame, 96 MiB of zeros in a few KB.
    let zeros = vec![0u8; 96 << 20];
    let bomb = zstd::bulk::compress(&zeros, 1).unwrap();
    assert!(bomb.len() < 64 << 10, "{} bytes", bomb.len());
    replace_frame(&db, "checks.py", &bomb);
    assert_refused(&db, "real 96 MiB frame", "limit");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_frame_that_lies_about_its_size_or_is_chained_to_another_is_refused() {
    let (dir, db) = written_store("lying");
    let honest = frame_of(&db, "checks.py");
    // A genuine small frame, then its header patched to declare fewer bytes
    // than it holds: single-segment, so the descriptor's size field is the
    // byte(s) right after it.
    let document = vec![b'a'; 200];
    let mut lying = zstd::bulk::compress(&document, 1).unwrap();
    // (No checksum on this one: the patch must be caught by the declared size.)
    // Descriptor 0x20: no size-field flag, single segment, which makes the size
    // one byte, and the byte is 200.
    assert_eq!((lying[4], lying[5]), (0x20, 200));
    lying[5] = 10;
    replace_frame(&db, "checks.py", &lying);
    assert_refused(&db, "declared 10, holds 200", "decompress");

    let mut chained = honest.clone();
    chained.extend_from_slice(&honest);
    replace_frame(&db, "checks.py", &chained);
    assert_refused(&db, "two frames", "decompress");

    let mut trailing = honest;
    trailing.extend_from_slice(b"junk");
    replace_frame(&db, "checks.py", &trailing);
    assert_refused(&db, "trailing bytes", "decompress");
    let _ = fs::remove_dir_all(&dir);
}

/// The part of `degraded_reason` about payload encoding, if any. The fixture
/// stores have no repository root, so `status` also says its source freshness is
/// unverified; that is not what is asked here.
fn status_reason(db: &Path) -> Option<String> {
    let store = Store::open_read_only(db).unwrap();
    store
        .status(&db.to_string_lossy())
        .unwrap()
        .degraded_reason
        .filter(|reason| reason.contains("schema-29 form"))
}

/// **A heal that could not convert says so.** A writer open that meets a payload
/// it cannot convert still succeeds (the readers refuse the row by name, and a
/// store with one bad document is otherwise usable), but `status` names it in
/// `degraded_reason`, so "opened clean" is no longer indistinguishable from
/// "opened with an unreadable payload". A healthy store reports nothing, and a
/// store only ever opened read-only is described the same way.
#[test]
fn an_open_that_could_not_convert_a_payload_says_so_in_status_and_stays_usable() {
    let (dir, db) = written_store("heal-loud");
    assert_eq!(status_reason(&db), None, "a healthy store reports nothing");

    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE file_payloads SET extraction_json = '{not json'
              WHERE file_id = (SELECT id FROM paths WHERE path = 'checks.py')",
            [],
        )
        .unwrap();
    // Read-only: nothing has tried to convert it yet, and status already says
    // what is wrong and where.
    let before = status_reason(&db).expect("an unconverted payload is a degraded store");
    assert!(before.contains("checks.py"), "{before}");
    assert!(before.contains("schema-29"), "{before}");

    // A writer open converts what it can and does not brick the store.
    let store =
        Store::open(&db).expect("a payload that cannot be converted does not brick the open");
    let after = store
        .status(&db.to_string_lossy())
        .unwrap()
        .degraded_reason
        .filter(|reason| reason.contains("schema-29 form"))
        .expect("the open could not convert it, and the store says so");
    assert!(after.contains("checks.py"), "{after}");
    assert_eq!(
        store
            .latest_extraction_for_path("ledger.py")
            .unwrap()
            .expect("the other payloads still read")
            .file_path,
        "ledger.py"
    );
    assert!(store
        .latest_extraction_for_path("checks.py")
        .err()
        .expect("and the bad one is refused by name")
        .to_string()
        .contains("checks.py"));
    drop(store);

    // Replacing it with a document that decodes clears the report: the status
    // is the store's state, not a memory of an open.
    let honest = serde_json::to_string(&extract_file("checks.py", &sources()[1].1)).unwrap();
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE file_payloads SET extraction_json = ?1
              WHERE file_id = (SELECT id FROM paths WHERE path = 'checks.py')",
            params![honest],
        )
        .unwrap();
    assert_eq!(
        status_reason(&db),
        None,
        "a readable text document is not a fault, and is converted by the next writer"
    );
    drop(Store::open(&db).unwrap());
    assert_eq!(status_reason(&db), None);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) <> 'blob'"
        ),
        0
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Status examines a bounded number of unconverted payloads and says so when it
/// did not reach the rest: "not examined" is never reported as "fine". 70
/// readable text documents beside the real ones are unconverted but not faults,
/// and more than a status will examine.
#[test]
fn status_does_not_read_unexamined_unconverted_payloads_as_clean() {
    let (dir, db) = written_store("heal-cap");
    {
        let conn = Connection::open(&db).unwrap();
        for index in 0..70 {
            conn.execute(
                "INSERT INTO paths (id, path) VALUES (?1, ?2)",
                params![1000 + index, format!("extra/f{index}.py")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO file_payloads
                   (file_id, content_hash, language, grammar_version, analyzer_version,
                    parse_outcome_json, engine_json, extraction_json)
                 VALUES (?1, ?1, 'python', 'g', 'a', '\"Clean\"', '\"TreeSitter\"', '{}')",
                params![1000 + index],
            )
            .unwrap();
        }
    }
    let reason = status_reason(&db).expect("70 unconverted payloads cannot be certified");
    assert!(reason.contains("not examined"), "{reason}");
    assert!(reason.contains("first 64"), "{reason}");
    assert!(
        !reason.contains("cannot be converted"),
        "none of the examined ones is unreadable: {reason}"
    );
    drop(Store::open(&db).unwrap());
    assert_eq!(status_reason(&db), None, "a writer converts them all");
    let _ = fs::remove_dir_all(&dir);
}

/// **What status covers is stated, and the gap is real and pinned.** A damaged
/// frame that already has its entry wiring is outside the rows the heal picks:
/// status does not examine it (checking every frame costs three times a whole
/// status), the reader refuses it by name, and every reason status does give
/// says its scope so that silence is not read as "all frames are fine".
#[test]
fn status_states_its_scope_and_does_not_examine_a_damaged_frame_that_has_its_wiring() {
    let (dir, db) = written_store("status-scope");
    let frame = frame_of(&db, "checks.py");
    replace_frame(&db, "checks.py", &frame[..frame.len() / 2]);
    assert_eq!(
        one::<i64>(
            &db,
            "SELECT COUNT(*) FROM file_payloads WHERE entry_wiring_json IS NULL"
        ),
        0
    );
    assert_eq!(
        status_reason(&db),
        None,
        "documented: a damaged frame with its wiring is found by the reader, not by status"
    );
    assert!(Store::open_read_only(&db)
        .unwrap()
        .latest_extraction_for_path("checks.py")
        .err()
        .expect("the reader refuses it")
        .to_string()
        .contains("checks.py"));

    // Any reason status does give names its scope.
    Connection::open(&db)
        .unwrap()
        .execute(
            "UPDATE file_payloads SET extraction_json = '{not json'
              WHERE file_id = (SELECT id FROM paths WHERE path = 'ledger.py')",
            [],
        )
        .unwrap();
    let reason = status_reason(&db).expect("an unreadable text payload is reported");
    assert!(reason.contains("ledger.py"), "{reason}");
    assert!(
        reason.contains("covers only payloads a writer open would convert"),
        "{reason}"
    );
    assert!(
        reason.contains("found only when a reader decompresses it"),
        "{reason}"
    );
    let _ = fs::remove_dir_all(&dir);
}
