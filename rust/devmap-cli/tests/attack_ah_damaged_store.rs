//! Attack round on lane `attack-honesty` (head 69877bd2), F04 recovery.
//!
//! The lane's tests smash pages of the full-text index and of `generation_nodes`
//! and `file_payloads`. These two start from other shapes of the same damaged
//! file: a table the unchanged-build path and `status` never read, and a store
//! that belongs to a different live worktree.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn fixture(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "devmap-attack-ah-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    let source: String = (0..30)
        .map(|n| format!("def widget_{n}():\n    return {n}\n\n"))
        .collect();
    fs::write(root.join("src/widgets.py"), source).unwrap();
    fs::write(
        root.join("src/use.py"),
        "from widgets import widget_1\n\n\ndef go():\n    return widget_1()\n",
    )
    .unwrap();
    root
}

fn devmap_at(root: &Path, db: &Path, args: &[&str]) -> (bool, serde_json::Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .env("DEVMAP_AUTOSPAWN", "0")
        .args(["--json", "--progress", "never", "--db"])
        .arg(db)
        .args(args)
        .output()
        .expect("run devmap");
    let json = serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
    (
        output.status.success(),
        json,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn smash_page_of(db: &Path, table: &str) {
    use std::io::{Seek, SeekFrom, Write};
    let (root_page, page_size) = {
        let conn = rusqlite::Connection::open(db).unwrap();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        let root_page: i64 = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        (root_page, page_size)
    };
    let mut file = fs::OpenOptions::new().write(true).open(db).unwrap();
    file.seek(SeekFrom::Start(((root_page - 1) * page_size) as u64))
        .unwrap();
    file.write_all(&vec![0xA5u8; page_size as usize]).unwrap();
    file.sync_all().unwrap();
}

fn quarantined(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.contains(".corrupt-") && !name.ends_with("-wal") && !name.ends_with("-shm")
            })
        })
        .collect()
}

/// FOLLOW-UP (pre-existing, identical on base `dbd0193a`; the lane's commit and
/// test names say "a damaged store" but cover the full-text index and the tables
/// its build happens to read). A smashed page of `unresolved_rows` leaves
/// `status` at `query_ready: true, is_fresh: true`, makes `impact` fail with
/// "database disk image is malformed", and makes `build` answer "unchanged":
/// F04's loop one table over.
#[test]
#[ignore = "F04 follow-up (carded): a probe for `unresolved_rows` is a page scan of a \
            table with hundreds of thousands of rows on every status and build, not a \
            further shadow table of the search index; it needs its own cost decision, \
            so it is not the integrity.rs search-index seam this lane owns"]
fn a_damaged_ledger_page_is_not_ready_and_build_does_not_say_unchanged() {
    let root = fixture("ledger");
    let db = root.join("devmap.sqlite");
    assert!(devmap_at(&root, &db, &["build", "."]).0);
    smash_page_of(&db, "unresolved_rows");

    let (impact_ok, _, impact_err) = devmap_at(&root, &db, &["impact", "widget_1"]);
    let (status_ok, status, _) = devmap_at(&root, &db, &["status"]);
    let (build_ok, built, _) = devmap_at(&root, &db, &["build", "."]);

    assert!(
        !impact_ok && impact_err.contains("malformed"),
        "premise: the damage must break a query ({impact_ok}, {impact_err})"
    );
    let ready = status_ok && status["query_ready"] == true;
    let unchanged = build_ok && built["store_recovery"].is_null();
    assert!(
        !ready,
        "status says query_ready over a store whose impact fails: {status}"
    );
    assert!(
        !unchanged,
        "build leaves a damaged store in place without saying so: {built}"
    );
}

/// FIX-BEFORE-MERGE (new in the lane; base refuses). The store belongs to
/// worktree A, which still exists. A build of worktree B pointed at it is
/// refused while the store is healthy. Once a page of the search index is
/// damaged, `build` probes and quarantines *before* `bind_repo_root` runs, so
/// B moves A's store aside and builds its own rows at A's path.
#[test]
fn a_damaged_store_of_another_live_worktree_is_refused_not_set_aside() {
    let a = fixture("owner-a");
    let b = fixture("owner-b");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).0);
    let (refused_ok, _, refused_err) = devmap_at(&b, &db, &["build", "."]);
    assert!(
        !refused_ok && refused_err.contains("one store cannot serve two live worktrees"),
        "premise: a healthy store of a live worktree is refused: {refused_err}"
    );

    smash_page_of(&db, "nodes_fts_idx");
    let (ok, out, err) = devmap_at(&b, &db, &["build", "."]);
    assert!(
        !ok && err.contains("one store cannot serve two live worktrees"),
        "worktree B must still be refused, got ok={ok} out={out} err={err}"
    );
    assert!(
        quarantined(&a).is_empty(),
        "A's store was moved aside by B's build: {:?}",
        quarantined(&a)
    );
}

/// The store's own path is a symlink to another live worktree's store. The
/// writer lock refuses a symlinked store path outright (so this one is refused
/// on base as well, for that reason and not for ownership), and nothing is moved.
#[test]
fn a_damaged_store_reached_through_a_symlinked_file_is_refused_not_set_aside() {
    let a = fixture("link-a");
    let b = fixture("link-b");
    let owned = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &owned, &["build", "."]).0);
    smash_page_of(&owned, "nodes_fts_idx");
    let link = b.join("devmap.sqlite");
    std::os::unix::fs::symlink(&owned, &link).unwrap();

    let (ok, out, err) = devmap_at(&b, &link, &["build", "."]);
    assert!(!ok, "B built through a symlink to A's store: {out} {err}");
    assert!(
        quarantined(&a).is_empty() && quarantined(&b).is_empty(),
        "a store was moved aside: {:?} {:?}",
        quarantined(&a),
        quarantined(&b)
    );
    assert!(owned.is_file(), "A's store is where it was");
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
    assert!(devmap_at(&a, &owned, &["build", "."]).0);
    smash_page_of(&owned, "nodes_fts_idx");
    let alias = b.join("state");
    std::os::unix::fs::symlink(&a, &alias).unwrap();

    let (ok, out, err) = devmap_at(&b, &alias.join("devmap.sqlite"), &["build", "."]);
    assert!(!ok, "B built through a symlinked directory: {out} {err}");
    assert!(
        quarantined(&a).is_empty(),
        "A's store was moved aside through the alias: {:?}",
        quarantined(&a)
    );
    assert!(owned.is_file(), "A's store is where it was");
}

/// A damaged store whose file cannot be written is refused as read-only and
/// stays where it is; moving a file needs only the directory's write bit.
#[test]
fn a_damaged_read_only_store_is_refused_not_set_aside() {
    use std::os::unix::fs::PermissionsExt;
    let a = fixture("readonly");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).0);
    smash_page_of(&db, "nodes_fts_idx");
    for name in ["devmap.sqlite", "devmap.sqlite-wal", "devmap.sqlite-shm"] {
        let path = a.join(name);
        if path.exists() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        }
    }
    let (ok, out, err) = devmap_at(&a, &db, &["build", "."]);
    for name in ["devmap.sqlite", "devmap.sqlite-wal", "devmap.sqlite-shm"] {
        let path = a.join(name);
        if path.exists() {
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
        }
    }
    assert!(
        quarantined(&a).is_empty(),
        "a read-only store was moved aside: {:?} (ok={ok} out={out} err={err})",
        quarantined(&a)
    );
    assert!(!ok, "a read-only store cannot be rebuilt: {out} {err}");
    assert!(db.is_file(), "the store is where it was");
}

/// The page that holds the owner is the damaged one. Whose store this is cannot
/// be read, and a check that could not run is not a check that passed: B is
/// refused and A's store stays.
#[test]
fn a_store_whose_owner_cannot_be_read_is_not_set_aside_by_another_worktree() {
    let a = fixture("owner-page-a");
    let b = fixture("owner-page-b");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).0);
    smash_page_of(&db, "pending_state");

    let (ok, out, err) = devmap_at(&b, &db, &["build", "."]);
    assert!(
        quarantined(&a).is_empty(),
        "A's store was moved aside by B's build: {:?} (ok={ok} out={out} err={err})",
        quarantined(&a)
    );
    assert!(!ok, "B built over a store it cannot show is its own: {out}");
    assert!(db.is_file(), "A's store is where it was");
}

/// The same damage in the owner's own store: it is refused rather than moved,
/// with the remedy named, because a store whose owner cannot be read cannot be
/// shown to be this worktree's.
#[test]
fn a_store_with_an_unreadable_owner_page_is_left_for_its_owner_to_remove() {
    let a = fixture("owner-page-self");
    let db = a.join("devmap.sqlite");
    assert!(devmap_at(&a, &db, &["build", "."]).0);
    smash_page_of(&db, "pending_state");

    let (ok, out, err) = devmap_at(&a, &db, &["build", "."]);
    assert!(!ok, "{out}");
    assert!(
        err.contains("is left where it is") && err.contains("move or delete it"),
        "the refusal names the remedy: {err}"
    );
    assert!(quarantined(&a).is_empty());
}
