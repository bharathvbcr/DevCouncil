//! Attack round on lane `attack-honesty` (head 69877bd2), F04 recovery, carried
//! to the detect-and-refuse cut of `damaged-store-recovery`.
//!
//! The lane's tests smash pages of the full-text index and of `generation_nodes`
//! and `file_payloads`. These start from other shapes of the same damaged file:
//! a table the unchanged-build path and `status` never read, and a store that
//! belongs to a different live worktree. Every "was moved aside" assertion of
//! the original round is now "refused, nothing moved, the file is identical".

use crate::a_damaged_store_is_reported_and_refused::{
    assert_untouched, devmap_at, drop_sidecars, fixture, identity, smash_page_of,
};
use std::fs;

const OWNERSHIP_REFUSAL: &str = "one store cannot serve two live worktrees";

/// FOLLOW-UP (pre-existing, identical on base `dbd0193a`; the lane's commit and
/// test names say "a damaged store" but cover the full-text index and the tables
/// its build happens to read). A smashed page of `unresolved_rows` leaves
/// `status` at `query_ready: true, is_fresh: true`, makes `impact` fail with
/// "database disk image is malformed", and makes `build` answer "unchanged":
/// F04's loop one table over.
#[test]
#[ignore = "card ft-72758783: a probe for `unresolved_rows` is a page scan of a table with \
            hundreds of thousands of rows on every status and build, not a further shadow \
            table of the search index; it needs its own cost decision"]
fn a_damaged_ledger_page_is_not_ready_and_build_does_not_say_unchanged() {
    let root = fixture("ledger");
    let db = root.join("devmap.sqlite");
    assert!(devmap_at(&root, &db, &["build", "."]).ok);
    smash_page_of(&db, "unresolved_rows");

    let impact = devmap_at(&root, &db, &["impact", "widget_1"]);
    let status = devmap_at(&root, &db, &["status"]);
    let built = devmap_at(&root, &db, &["build", "."]);

    assert!(
        !impact.ok && impact.stderr.contains("malformed"),
        "premise: the damage must break a query ({}, {})",
        impact.ok,
        impact.stderr
    );
    assert!(
        !(status.ok && status.json["query_ready"] == true),
        "status says query_ready over a store whose impact fails: {}",
        status.json
    );
    assert!(
        !built.ok,
        "build leaves a damaged store in place without saying so: {}",
        built.json
    );
}

/// The store belongs to worktree A, which still exists. A build of worktree B
/// pointed at it is refused while the store is healthy, and stays refused once a
/// page of the search index is damaged: ownership is decided before the damage
/// is, and nothing of A's is moved or written.
#[test]
fn a_damaged_store_of_another_live_worktree_is_refused_not_set_aside() {
    let a = fixture("owner-a");
    let b = fixture("owner-b");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).ok);
    let refused = devmap_at(&b, &db, &["build", "."]);
    assert!(
        !refused.ok && refused.stderr.contains(OWNERSHIP_REFUSAL),
        "premise: a healthy store of a live worktree is refused: {}",
        refused.stderr
    );

    smash_page_of(&db, "nodes_fts_idx");
    let before = identity(&db);
    let built = devmap_at(&b, &db, &["build", "."]);
    assert!(
        !built.ok && built.stderr.contains(OWNERSHIP_REFUSAL),
        "worktree B must still be refused for ownership, got ok={} out={} err={}",
        built.ok,
        built.json,
        built.stderr
    );
    assert_untouched("A's store after B's build", &db, &before);
}

/// The store's own path is a symlink to another live worktree's store. The
/// writer lock refuses a symlinked store path outright (so this one is refused
/// on base as well, for that reason and not for ownership), and nothing is moved.
#[test]
fn a_damaged_store_reached_through_a_symlinked_file_is_refused_not_set_aside() {
    let a = fixture("link-a");
    let b = fixture("link-b");
    let owned = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &owned, &["build", "."]).ok);
    smash_page_of(&owned, "nodes_fts_idx");
    let link = b.join("devmap.sqlite");
    std::os::unix::fs::symlink(&owned, &link).unwrap();
    let before = identity(&owned);

    let built = devmap_at(&b, &link, &["build", "."]);
    assert!(
        !built.ok,
        "B built through a symlink to A's store: {} {}",
        built.json, built.stderr
    );
    assert_untouched("A's store", &owned, &before);
    assert!(
        crate::a_damaged_store_is_reported_and_refused::set_aside(&b).is_empty(),
        "B's directory holds a set-aside copy"
    );
    assert!(link.is_symlink(), "B's path is still the link it was given");
}

/// The store's *directory* is a symlink to another live worktree's state
/// directory. The managed-directory opener refuses a link in the path (also on
/// base, so again not an ownership refusal), and nothing is moved.
#[test]
fn a_damaged_store_reached_through_a_symlinked_directory_is_refused_not_set_aside() {
    let a = fixture("dirlink-a");
    let b = fixture("dirlink-b");
    let owned = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &owned, &["build", "."]).ok);
    smash_page_of(&owned, "nodes_fts_idx");
    let alias = b.join("state");
    std::os::unix::fs::symlink(&a, &alias).unwrap();
    let before = identity(&owned);

    let built = devmap_at(&b, &alias.join("devmap.sqlite"), &["build", "."]);
    assert!(
        !built.ok,
        "B built through a symlinked directory: {} {}",
        built.json, built.stderr
    );
    assert_untouched("A's store through the alias", &owned, &before);
}

/// A damaged store whose file cannot be written is refused and stays where it
/// is, unchanged.
#[test]
fn a_damaged_read_only_store_is_refused_not_set_aside() {
    use std::os::unix::fs::PermissionsExt;
    let a = fixture("readonly");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).ok);
    smash_page_of(&db, "nodes_fts_idx");
    let before = identity(&db);
    let sidecars = ["devmap.sqlite", "devmap.sqlite-wal", "devmap.sqlite-shm"];
    for name in sidecars {
        let path = a.join(name);
        if path.exists() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        }
    }
    let built = devmap_at(&a, &db, &["build", "."]);
    for name in sidecars {
        let path = a.join(name);
        if path.exists() {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
        }
    }
    assert!(
        !built.ok,
        "a read-only store cannot be rebuilt: {} {}",
        built.json, built.stderr
    );
    assert_untouched("the read-only store", &db, &before);
}

/// The page that holds the owner is the damaged one. Whose store this is cannot
/// be read, and a check that could not run is not a check that passed: B is
/// refused and A's store is untouched.
#[test]
fn a_store_whose_owner_cannot_be_read_is_not_touched_by_another_worktree() {
    let a = fixture("owner-page-a");
    let b = fixture("owner-page-b");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).ok);
    smash_page_of(&db, "pending_state");
    let before = identity(&db);

    let built = devmap_at(&b, &db, &["build", "."]);
    assert!(
        !built.ok,
        "B built over a store it cannot show is its own: {}",
        built.json
    );
    assert_untouched("A's store after B's build", &db, &before);
}

/// The same damage in the owner's own store: it is refused rather than touched,
/// with the remedy named, because a store whose owner cannot be read cannot be
/// shown to be this worktree's.
#[test]
fn a_store_with_an_unreadable_owner_page_is_left_for_its_owner_to_remove() {
    let a = fixture("owner-page-self");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).ok);
    smash_page_of(&db, "pending_state");
    drop_sidecars(&db);
    let before = identity(&db);

    let built = devmap_at(&a, &db, &["build", "."]);
    assert!(!built.ok, "{}", built.json);
    assert!(
        built.stderr.contains("left where it is")
            && built
                .stderr
                .contains("move or delete it and run `devmap build`"),
        "the refusal names the remedy: {}",
        built.stderr
    );
    assert_untouched("the owner's own store", &db, &before);
}
