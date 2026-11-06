//! Attack round on lane `damaged-store-recovery` (head 0f254876, which set a
//! damaged store aside): whose store does `build` touch?
//!
//! The round found that the mover moved another live worktree's store when its
//! owner lived somewhere the ownership check did not read. The mover is parked
//! (`docs/devmap/design/auto-quarantine-parked.md`) and the lane is detect and
//! refuse only, so each "was moved aside" assertion of the round is now "refused,
//! nothing moved, the file is identical byte for byte and inode for inode".
//!
//! `DEVMAP_ATTACK_BIN` points the same tests at another binary (base
//! `dbd0193a`), which is how each finding was classified against base.

use crate::a_damaged_store_is_reported_and_refused::{
    assert_untouched, devmap_at, drop_sidecars, fixture, identity, smash_page_of,
};
use std::path::{Path, PathBuf};

/// F1 (the BLOCK; new in the mover, base refuses). The store belongs to
/// worktree A, which still exists, and was written by a kernel older than
/// schema 20, before `pending_state` existed. The owner of such a store is in
/// `generations.repo_root` (schema 7 onward), which is where the v19 -> v20
/// migration copies it from. The mover's ownership check only looked at
/// `pending_state.repo_root`, read "no such column" as "this store predates the
/// recorded owner", and passed; the migration that would have shown the owner
/// died on the damaged page and rolled back. B then moved A's store aside and
/// built its own rows at A's path.
///
/// Now there is no mover: B is refused and A's store is untouched.
#[test]
fn a_live_worktrees_store_from_before_pending_state_is_not_set_aside_by_another_worktree() {
    let a = fixture("pre-owner-a");
    let b = fixture("pre-owner-b");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).ok);
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        let owner: String = conn
            .query_row(
                "SELECT repo_root FROM generations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            owner.contains("pre-owner-a"),
            "premise: the pre-v20 owner record names A: {owner}"
        );
        conn.execute_batch(
            "DROP TABLE pending_state; \
             ALTER TABLE pending_paths DROP COLUMN revision; \
             PRAGMA user_version = 19;",
        )
        .unwrap();
    }
    drop_sidecars(&db);
    smash_page_of(&db, "generations");
    drop_sidecars(&db);
    let before = identity(&db);

    let built = devmap_at(&b, &db, &["build", "."]);
    assert!(
        !built.ok,
        "B built over a store that belongs to a live worktree: {}",
        built.json
    );
    assert!(
        built.stderr.contains(&db.display().to_string()),
        "the refusal names A's store: {}",
        built.stderr
    );
    assert_untouched("A's pre-v20 store after B's build", &db, &before);

    // And A's own build is refused the same way, with the same file untouched.
    let own = devmap_at(&a, &db, &["build", "."]);
    assert!(!own.ok, "{}", own.json);
    assert_untouched("A's pre-v20 store after A's build", &db, &before);
}

/// The damage makes `Store::open` itself fail `SQLITE_CORRUPT`, the store is at
/// the current schema and names A, and B reaches it by a relative `--db` from a
/// different working directory. Refused, nothing moved, bytes untouched. The
/// refusal is either the ownership one or the damaged-store one, and names the
/// store either way.
#[test]
fn a_store_that_fails_to_open_corrupt_is_refused_for_another_worktree_by_a_relative_db() {
    // `generation_nodes` is a view since schema 28; its rows live in `file_rows`
    // (which payload a generation reads) and `payload_nodes` (the payload's symbols).
    for table in [
        "file_rows",
        "payload_nodes",
        "paths",
        "generations",
        "reader_compat",
    ] {
        let a = fixture("retry-a");
        let b = fixture("retry-b");
        let db = a.join("devmap.sqlite");
        assert!(devmap_at(&a, &db, &["build", "."]).ok);
        smash_page_of(&db, table);
        drop_sidecars(&db);
        let before = identity(&db);

        let relative: PathBuf = Path::new("..")
            .join(a.file_name().unwrap())
            .join("devmap.sqlite");
        // B's cwd is a sibling directory of A, so `../<a>/devmap.sqlite` is A's.
        let built = devmap_at(&b, &relative, &["build", "."]);
        let relative_damage = format!("the devmap store {} is damaged", relative.display());
        let damage_names_the_anchored_store = built.stderr.contains(" is damaged")
            && built
                .stderr
                .contains(a.file_name().unwrap().to_str().unwrap())
            && !built.stderr.contains(&relative_damage);
        assert!(
            !built.ok
                && (built
                    .stderr
                    .contains("one store cannot serve two live worktrees")
                    || damage_names_the_anchored_store),
            "{table}: B must be refused by the anchored store path, ok={} {}",
            built.ok,
            built.stderr
        );
        assert!(
            !built.stderr.contains(&relative_damage),
            "{table}: the refusal kept the relative --db spelling: {}",
            built.stderr
        );
        assert_untouched(table, &db, &before);
    }
}

/// The schema-27 shape of the store, which is what an installed binary of the
/// previous release wrote and still reads. The reduction is the one the
/// migration ladder's own tests use.
#[path = "../../devmap-store/tests/support/v27_shape.rs"]
mod v27_shape;

/// What a refused build must leave of another worktree's schema-27 store.
struct Schema27Store {
    db: PathBuf,
    owner: PathBuf,
}

/// Worktree A builds its store with this binary, and the store is then put back
/// into the shape a schema-27 binary wrote: the flat per-generation tables, the
/// reader floor of that release, and every extraction as JSON text (the form the
/// v29 rung compresses). Nothing here is read by the build under test, only
/// stamped, so a migration of it shows as a changed stamp, a changed column type
/// or a new column.
fn a_worktrees_schema_27_store(label: &str) -> Schema27Store {
    let owner = fixture(label);
    let db = owner.join("devmap.sqlite");
    assert!(devmap_at(&owner, &db, &["build", "."]).ok);
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        conn.execute_batch(v27_shape::REDUCE_V29_TO_V28).unwrap();
        conn.execute_batch(v27_shape::REDUCE_V28_TO_V27).unwrap();
        conn.execute_batch(
            "UPDATE file_payloads SET extraction_json = '{\"wiring\":[]}';
             UPDATE reader_compat SET min_reader_schema = 25;
             PRAGMA user_version = 27;",
        )
        .unwrap();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
    }
    drop_sidecars(&db);
    Schema27Store { db, owner }
}

fn scalar_of(db: &Path, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// What a migration of the store would have changed, read without writing.
fn assert_still_schema_27(what: &str, db: &Path) {
    assert_eq!(
        scalar_of(db, "PRAGMA user_version"),
        27,
        "{what}: the schema stamp moved"
    );
    assert_eq!(
        scalar_of(
            db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) = 'text'"
        ),
        scalar_of(db, "SELECT COUNT(*) FROM file_payloads"),
        "{what}: an extraction was compressed"
    );
    assert!(
        scalar_of(db, "SELECT COUNT(*) FROM file_payloads") > 0,
        "{what}: premise: the store holds payloads"
    );
    assert_eq!(
        scalar_of(
            db,
            "SELECT COUNT(*) FROM pragma_table_info('file_payloads') \
              WHERE name = 'entry_wiring_json'"
        ),
        0,
        "{what}: the v29 column was added"
    );
    assert_eq!(
        scalar_of(db, "SELECT min_reader_schema FROM reader_compat"),
        25,
        "{what}: the reader floor moved, so the previous release's binary would refuse its \
         own store"
    );
}

/// F3, pre-existing on base `dbd0193a` and carried to 27 -> 29 by schema 28 and
/// 29. `Store::open` migrates before anything asks whose store it is, so a build
/// of worktree B that is then refused for ownership had already rewritten
/// worktree A's store: the schema stamp moved forward under a worktree that may
/// still be running the previous release's binary, which then refuses to read
/// its own store. Over 27 -> 29 that is also every stored extraction rewritten
/// as a zstd frame.
///
/// `devmap build` now reads the owner over a read-only connection before it
/// opens the store for writing (`card ft-6bcae99c`, the rule "ownership first" in
/// `docs/devmap/design/auto-quarantine-parked.md`).
#[test]
fn a_build_refused_for_ownership_has_not_migrated_the_other_worktrees_store() {
    let a = a_worktrees_schema_27_store("migrate-a");
    let b = fixture("migrate-b");
    assert_still_schema_27("premise", &a.db);
    let before = identity(&a.db);

    let built = devmap_at(&b, &a.db, &["build", "."]);
    assert!(
        !built.ok
            && built
                .stderr
                .contains("one store cannot serve two live worktrees"),
        "premise: B is refused for ownership: {}",
        built.stderr
    );
    assert_still_schema_27("B's refused build", &a.db);
    assert_untouched("A's schema-27 store after B's build", &a.db, &before);
}

/// The proof is not a refusal that also stops the owner: A's own build of its
/// schema-27 store goes through and migrates it to the current schema.
#[test]
fn the_owners_own_build_still_migrates_its_schema_27_store() {
    let a = a_worktrees_schema_27_store("owner-migrates");
    let built = devmap_at(&a.owner, &a.db, &["build", "."]);
    assert!(built.ok, "{}", built.stderr);
    assert!(
        scalar_of(&a.db, "PRAGMA user_version") > 27,
        "the owner's build left the store at schema 27"
    );
    assert_eq!(
        scalar_of(
            &a.db,
            "SELECT COUNT(*) FROM file_payloads WHERE typeof(extraction_json) <> 'blob'"
        ),
        0,
        "the owner's migration left an extraction uncompressed"
    );
}

/// "Unprovable means refuse": a schema-27 store whose owner record cannot be
/// read is not migrated by a worktree that cannot show it is the owner. The
/// refusal names the store and leaves it byte for byte.
#[test]
fn a_schema_27_store_whose_owner_cannot_be_read_is_not_migrated_by_another_worktree() {
    let a = a_worktrees_schema_27_store("owner-unreadable");
    let b = fixture("owner-unreadable-b");
    smash_page_of(&a.db, "pending_state");
    drop_sidecars(&a.db);
    let before = identity(&a.db);

    let built = devmap_at(&b, &a.db, &["build", "."]);
    assert!(
        !built.ok
            && built.stderr.contains(&a.db.display().to_string())
            && built.stderr.contains("left where it is"),
        "B must be refused, naming A's store: ok={} {}",
        built.ok,
        built.stderr
    );
    assert_untouched("A's schema-27 store after B's build", &a.db, &before);
}
