//! The reader floor: a store newer than an embedding reader is readable when,
//! and only when, the writer that stamped it says a reader this old can read
//! it.
//!
//! Before the floor, `open_read_only` required `user_version` to equal its own
//! schema exactly, so every bump — v26 only *added* `generation_literals` —
//! left GitPulse, which links this crate read-only, unable to open any store
//! until it was re-vendored. These tests hold both halves: the newer store a
//! floor admits is read, and everything else that was refused still is.

use devmap_store::{
    Store, CURRENT_SCHEMA_VERSION, MIN_READER_SCHEMA_VERSION, SCHEMA_READER_FLOORS,
};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "devmap-reader-floor-{}-{stamp}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn db(&self) -> PathBuf {
        self.0.join("devmap.sqlite")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A store this binary wrote, then restamped by hand as if a later binary had
/// written it. `floor: None` removes the floor table entirely, the shape of a
/// store written before the floor existed.
fn store_stamped(stamped: i32, floor: Option<i64>) -> Scratch {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    let conn = Connection::open(temp.db()).unwrap();
    match floor {
        None => conn.execute_batch("DROP TABLE reader_compat").unwrap(),
        Some(floor) => {
            // Rebuilt without its CHECK so a test can record a floor a real
            // writer could not, and see the reader refuse it.
            conn.execute_batch(
                "DROP TABLE reader_compat;
                 CREATE TABLE reader_compat (singleton INTEGER, min_reader_schema);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO reader_compat (singleton, min_reader_schema) VALUES (1, ?1)",
                [floor],
            )
            .unwrap();
        }
    }
    conn.execute(&format!("PRAGMA user_version = {stamped}"), [])
        .unwrap();
    temp
}

fn recorded_floor(db: &PathBuf) -> Option<i64> {
    let conn = Connection::open(db).unwrap();
    conn.query_row("SELECT min_reader_schema FROM reader_compat", [], |row| {
        row.get(0)
    })
    .ok()
}

fn user_version(db: &PathBuf) -> i32 {
    Connection::open(db)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn a_fresh_store_records_this_writers_floor() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    assert_eq!(
        recorded_floor(&temp.db()),
        Some(i64::from(MIN_READER_SCHEMA_VERSION))
    );
}

#[test]
fn a_store_written_before_the_floor_gains_it_at_the_next_writer_open() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    Connection::open(temp.db())
        .unwrap()
        .execute_batch("DROP TABLE reader_compat")
        .unwrap();
    assert_eq!(recorded_floor(&temp.db()), None);

    drop(Store::open(temp.db()).unwrap());
    assert_eq!(
        recorded_floor(&temp.db()),
        Some(i64::from(MIN_READER_SCHEMA_VERSION)),
        "the ladder's end must stamp the floor, not only the fresh path"
    );
    assert_eq!(user_version(&temp.db()), CURRENT_SCHEMA_VERSION);
}

#[test]
fn a_stale_floor_is_overwritten_by_the_writer_that_owns_the_stamp() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    Connection::open(temp.db())
        .unwrap()
        .execute("UPDATE reader_compat SET min_reader_schema = 3", [])
        .unwrap();
    drop(Store::open(temp.db()).unwrap());
    assert_eq!(
        recorded_floor(&temp.db()),
        Some(i64::from(MIN_READER_SCHEMA_VERSION))
    );
}

#[test]
fn a_newer_store_whose_floor_reaches_this_reader_is_read() {
    let newer = CURRENT_SCHEMA_VERSION + 1;
    let temp = store_stamped(newer, Some(i64::from(CURRENT_SCHEMA_VERSION)));
    let store = Store::open_read_only(temp.db())
        .unwrap_or_else(|err| panic!("a floor at this reader's schema must admit it: {err}"));
    assert!(store.is_read_only());
    store
        .latest_generation_id()
        .expect("an admitted store answers reads");
}

#[test]
fn admitting_a_newer_store_never_writes_to_it() {
    let newer = CURRENT_SCHEMA_VERSION + 1;
    let temp = store_stamped(newer, Some(i64::from(CURRENT_SCHEMA_VERSION)));
    drop(Store::open_read_only(temp.db()).unwrap());
    assert_eq!(user_version(&temp.db()), newer);
    assert_eq!(
        recorded_floor(&temp.db()),
        Some(i64::from(CURRENT_SCHEMA_VERSION))
    );
}

#[test]
fn a_writer_still_refuses_a_newer_store_whatever_its_floor_says() {
    let newer = CURRENT_SCHEMA_VERSION + 1;
    let temp = store_stamped(newer, Some(i64::from(MIN_READER_SCHEMA_VERSION)));
    let err = match Store::open(temp.db()) {
        Ok(_) => panic!("a writer must never open a store newer than itself"),
        Err(err) => err,
    };
    assert_eq!(
        Store::unsupported_schema_versions(&err),
        Some((newer, CURRENT_SCHEMA_VERSION))
    );
    assert_eq!(user_version(&temp.db()), newer, "the refusal must not write");
}

#[test]
fn a_newer_store_whose_floor_is_above_this_reader_is_refused_by_name() {
    let newer = CURRENT_SCHEMA_VERSION + 1;
    let temp = store_stamped(newer, Some(i64::from(newer)));
    let err = match Store::open_read_only(temp.db()) {
        Ok(_) => panic!("a floor above this reader must refuse it"),
        Err(err) => err,
    };
    let message = err.to_string();
    assert!(
        message.contains(&format!("reader at schema {newer} or newer")),
        "the refusal must name the floor: {message}"
    );
}

#[test]
fn a_newer_store_with_no_floor_is_refused_exactly_as_before() {
    let newer = CURRENT_SCHEMA_VERSION + 1;
    let temp = store_stamped(newer, None);
    let err = match Store::open_read_only(temp.db()) {
        Ok(_) => panic!("no floor must mean exact match"),
        Err(err) => err,
    };
    assert_eq!(
        Store::unsupported_schema_versions(&err),
        Some((newer, CURRENT_SCHEMA_VERSION)),
        "the typed refusal readers already downcast must be unchanged"
    );
}

#[test]
fn an_older_store_is_refused_whatever_its_floor_says() {
    let older = CURRENT_SCHEMA_VERSION - 1;
    let temp = store_stamped(older, Some(3));
    let err = match Store::open_read_only(temp.db()) {
        Ok(_) => panic!("a reader cannot migrate an older store"),
        Err(err) => err,
    };
    assert_eq!(
        Store::unsupported_schema_versions(&err),
        Some((older, CURRENT_SCHEMA_VERSION))
    );
}

/// A damaged floor is a refusal, never a missing one: "missing" means exact
/// match, which would be safe — but quietly reading damage as absence is how a
/// check that could not run comes to look like one that passed.
#[test]
fn a_damaged_floor_fails_closed() {
    let newer = CURRENT_SCHEMA_VERSION + 1;
    let damaged: &[(&str, &str)] = &[
        ("text", "INSERT INTO reader_compat VALUES (1, 'twenty-six')"),
        ("real", "INSERT INTO reader_compat VALUES (1, 26.5)"),
        ("null", "INSERT INTO reader_compat VALUES (1, NULL)"),
        ("below the oldest schema", "INSERT INTO reader_compat VALUES (1, 2)"),
        ("negative", "INSERT INTO reader_compat VALUES (1, -1)"),
        (
            "above the store's own stamp",
            "INSERT INTO reader_compat VALUES (1, 9999)",
        ),
        (
            "two rows",
            "INSERT INTO reader_compat VALUES (1, 26); INSERT INTO reader_compat VALUES (2, 26)",
        ),
    ];
    for (label, insert) in damaged {
        let temp = Scratch::new();
        drop(Store::open(temp.db()).unwrap());
        let conn = Connection::open(temp.db()).unwrap();
        conn.execute_batch(&format!(
            "DROP TABLE reader_compat;
             CREATE TABLE reader_compat (singleton INTEGER, min_reader_schema);
             {insert};
             PRAGMA user_version = {newer};"
        ))
        .unwrap();
        drop(conn);
        let err = match Store::open_read_only(temp.db()) {
            Ok(_) => panic!("{label}: a damaged floor admitted a newer store"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("reader_compat"),
            "{label}: the refusal must name the damaged floor, got: {err}"
        );
    }
}

#[test]
fn a_floor_that_admits_a_store_missing_what_this_reader_selects_is_still_refused() {
    let newer = CURRENT_SCHEMA_VERSION + 1;
    let temp = store_stamped(newer, Some(i64::from(CURRENT_SCHEMA_VERSION)));
    // A later writer that claimed compatibility while dropping a column this
    // reader selects: the floor admits, `validate_schema` must not.
    Connection::open(temp.db())
        .unwrap()
        .execute_batch("DROP TABLE pending_state")
        .unwrap();
    assert!(
        Store::open_read_only(temp.db()).is_err(),
        "admission is not the last word; schema validation still runs"
    );
}

/// Every (stamp, floor) pair a store can carry, against the one rule.
#[test]
fn admission_matrix() {
    let current = CURRENT_SCHEMA_VERSION;
    let stamps = [
        -1,
        0,
        2,
        current - 2,
        current - 1,
        current,
        current + 1,
        current + 2,
        current + 40,
        i32::MAX,
    ];
    let floors: Vec<Option<i64>> = vec![
        None,
        Some(3),
        Some(i64::from(MIN_READER_SCHEMA_VERSION)),
        Some(i64::from(current)),
        Some(i64::from(current + 1)),
        Some(i64::from(current + 2)),
        Some(i64::from(current + 40)),
    ];
    let mut checked = 0;
    for &stamped in &stamps {
        for &floor in &floors {
            let temp = store_stamped(stamped, floor);
            let admitted = Store::open_read_only(temp.db()).is_ok();
            let valid_floor = floor.filter(|f| *f >= 3 && *f <= i64::from(stamped));
            let expected = stamped == current
                || (stamped > current && valid_floor.is_some_and(|f| f <= i64::from(current)));
            assert_eq!(
                admitted, expected,
                "stamp {stamped}, floor {floor:?}: admitted={admitted}, expected={expected}"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, stamps.len() * floors.len());
}

#[test]
fn every_schema_since_the_floor_records_a_deliberate_floor() {
    let (last_schema, last_floor) = *SCHEMA_READER_FLOORS
        .last()
        .expect("the floor history is never empty");
    assert_eq!(
        (last_schema, last_floor),
        (CURRENT_SCHEMA_VERSION, MIN_READER_SCHEMA_VERSION),
        "CURRENT_SCHEMA_VERSION moved without a decision about the reader floor: add a row \
         to SCHEMA_READER_FLOORS and set MIN_READER_SCHEMA_VERSION (see its docs)"
    );
    for window in SCHEMA_READER_FLOORS.windows(2) {
        let ((a, fa), (b, fb)) = (window[0], window[1]);
        assert!(b > a, "schemas must ascend: {a} then {b}");
        assert!(fb >= fa, "a floor never moves down: {fa} then {fb}");
    }
    for &(schema, floor) in SCHEMA_READER_FLOORS {
        assert!(
            (3..=schema).contains(&floor),
            "floor {floor} for schema {schema}"
        );
    }
}

/// Every writer open stamps the floor inside the migration transaction, so
/// concurrent openers must converge on one row, not race into two or none.
#[test]
fn concurrent_writer_opens_converge_on_one_floor() {
    let temp = Scratch::new();
    drop(Store::open(temp.db()).unwrap());
    Connection::open(temp.db())
        .unwrap()
        .execute_batch("DROP TABLE reader_compat")
        .unwrap();
    let db = temp.db();
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let db = db.clone();
            std::thread::spawn(move || {
                for _ in 0..5 {
                    drop(Store::open(&db).expect("concurrent writer open"));
                    drop(Store::open_read_only(&db).expect("concurrent reader open"));
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let rows: i64 = Connection::open(&db)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM reader_compat", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(
        recorded_floor(&db),
        Some(i64::from(MIN_READER_SCHEMA_VERSION))
    );
}
