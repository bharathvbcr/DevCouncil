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
    for table in ["generation_nodes", "paths", "generations", "reader_compat"] {
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
        assert!(
            !built.ok
                && (built
                    .stderr
                    .contains("one store cannot serve two live worktrees")
                    || built.stderr.contains(&format!(
                        "the devmap store {} is damaged",
                        relative.display()
                    ))),
            "{table}: B must be refused, ok={} {}",
            built.ok,
            built.stderr
        );
        assert_untouched(table, &db, &before);
    }
}

/// F3, pre-existing on base `dbd0193a` (run with `DEVMAP_ATTACK_BIN` to see it).
/// `Store::open` migrates before anything asks whose store it is, so a build of
/// worktree B that is then refused for ownership has already rewritten worktree
/// A's store: the schema stamp moves forward under a worktree that may still be
/// running an older binary.
#[test]
#[ignore = "F3, pre-existing on base; card ft-6bcae99cd8aea8538b677a3660c18cbe \
            (task_id store-ownership-first-and-quarantine-resume): the refused build has \
            already migrated the other worktree's store; Store::open runs before any \
            ownership proof"]
fn a_build_refused_for_ownership_has_not_migrated_the_other_worktrees_store() {
    let a = fixture("migrate-a");
    let b = fixture("migrate-b");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).ok);
    let version_of = |db: &Path| -> i64 {
        let conn = rusqlite::Connection::open(db).unwrap();
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    };
    let current = version_of(&db);
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        conn.execute_batch(&format!("PRAGMA user_version = {};", current - 1))
            .unwrap();
    }
    let built = devmap_at(&b, &db, &["build", "."]);
    assert!(
        !built.ok
            && built
                .stderr
                .contains("one store cannot serve two live worktrees"),
        "premise: B is refused: {}",
        built.stderr
    );
    assert_eq!(
        version_of(&db),
        current - 1,
        "B's refused build migrated A's store"
    );
}
