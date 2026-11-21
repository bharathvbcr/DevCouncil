//! A store created at a path that already holds a `.csr` or `.artifacts.json`
//! removes them: each describes the one store it sat beside, and an earlier
//! store at that path wrote these, not the new one.
//!
//! The case that matters is the same-generation orphan (attack `atk-s`,
//! `sameorphan`): readers accept a CSR on its generation number alone, so an
//! orphan stamped with the generation the new store also reaches is served as
//! the new store's graph, with the old store's nodes and edges.
//!
//! Generations are written by SQL, as `head_stamp_without_grammars.rs` does, so
//! this target needs no parsing frontend.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use devmap_store::csr::{CsrGraphWriter, CsrNodeRecord};
use devmap_store::seed::sidecar;
use devmap_store::Store;

fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-foreign-sidecars-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

/// An orphan CSR stamped `generation` holding `nodes` nodes in a chain.
fn write_orphan_csr(csr: &Path, generation: u64, nodes: u32) {
    let mut writer = CsrGraphWriter::new(generation);
    for _ in 0..nodes {
        writer.add_node(CsrNodeRecord::default());
    }
    for i in 1..nodes {
        writer.add_edge(i - 1, i, 90);
    }
    writer.write_to_file(csr).unwrap();
}

fn give_generation_one(db: &Path) {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.execute(
        "INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
         VALUES (1, 1.0, 'abc', NULL, '{\"total_files\":0,\"total_symbols\":0,
                 \"total_edges\":0,\"dead_symbols\":[],\"communities\":[],\"status\":\"Ok\"}')",
        [],
    )
    .unwrap();
}

#[test]
fn creating_a_store_removes_the_csr_and_artifact_stamp_it_did_not_write() {
    let dir = tmp_dir("create");
    let db = dir.join("devmap.sqlite");
    let csr = sidecar(&db, ".csr");
    let stamp = sidecar(&db, ".artifacts.json");
    write_orphan_csr(&csr, 1, 7);
    fs::write(&stamp, b"{\"orphan\":true}").unwrap();

    drop(Store::open(&db).unwrap());

    assert!(!csr.exists(), "the orphan CSR must be gone");
    assert!(!stamp.exists(), "the orphan artifact stamp must be gone");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_same_generation_orphan_csr_is_never_served_as_the_new_stores_graph() {
    let dir = tmp_dir("sameorphan");
    let db = dir.join("devmap.sqlite");
    // The earlier store at this path got to generation 1 with a 7-node graph
    // and left its CSR; the store itself was deleted.
    write_orphan_csr(&sidecar(&db, ".csr"), 1, 7);

    drop(Store::open(&db).unwrap());
    give_generation_one(&db);

    let store = Store::open(&db).unwrap();
    assert_eq!(store.latest_generation_id().unwrap(), Some(1));
    let reader = store
        .open_latest_csr_sidecar()
        .unwrap()
        .expect("a store with a generation has a graph");
    assert_eq!(
        reader.node_count(),
        0,
        "the 7-node orphan was served as the 0-node store's graph"
    );
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn a_sidecar_that_is_a_symlink_is_unlinked_and_its_target_is_left_alone() {
    let dir = tmp_dir("symlink");
    let db = dir.join("devmap.sqlite");
    let elsewhere = dir.join("elsewhere.bin");
    fs::write(&elsewhere, b"not ours").unwrap();
    std::os::unix::fs::symlink(&elsewhere, sidecar(&db, ".csr")).unwrap();

    drop(Store::open(&db).unwrap());

    assert!(fs::symlink_metadata(sidecar(&db, ".csr")).is_err());
    assert_eq!(fs::read(&elsewhere).unwrap(), b"not ours");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn reopening_an_existing_store_keeps_its_sidecars() {
    let dir = tmp_dir("reopen");
    let db = dir.join("devmap.sqlite");
    drop(Store::open(&db).unwrap());
    let csr = sidecar(&db, ".csr");
    let stamp = sidecar(&db, ".artifacts.json");
    write_orphan_csr(&csr, 1, 3);
    fs::write(&stamp, b"{}").unwrap();

    drop(Store::open(&db).unwrap());

    assert!(csr.is_file(), "an existing store's CSR is its own");
    assert!(stamp.is_file(), "an existing store's stamp is its own");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_sidecar_that_cannot_be_removed_refuses_the_new_store() {
    let dir = tmp_dir("stuck");
    let db = dir.join("devmap.sqlite");
    // A directory at the sidecar's name cannot be unlinked as a file.
    fs::create_dir(sidecar(&db, ".csr")).unwrap();

    let error = Store::open(&db)
        .err()
        .expect("must refuse, not serve the obstruction");
    assert!(format!("{error:#}").contains(".csr"), "{error:#}");
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn a_fifo_or_a_dangling_symlink_at_a_sidecar_name_is_unlinked() {
    let dir = tmp_dir("fifo-dangling");
    let db = dir.join("devmap.sqlite");
    let made = std::process::Command::new("mkfifo")
        .arg(sidecar(&db, ".csr"))
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo");
    std::os::unix::fs::symlink(dir.join("nowhere"), sidecar(&db, ".artifacts.json")).unwrap();

    drop(Store::open(&db).unwrap());

    assert!(fs::symlink_metadata(sidecar(&db, ".csr")).is_err(), "fifo survived");
    assert!(
        fs::symlink_metadata(sidecar(&db, ".artifacts.json")).is_err(),
        "dangling link survived"
    );
    assert!(!dir.join("nowhere").exists(), "the dangling target was created");
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn a_refused_creation_leaves_no_store_behind_and_the_obstruction_untouched() {
    let dir = tmp_dir("refused-clean");
    let db = dir.join("devmap.sqlite");
    let obstruction = sidecar(&db, ".csr");
    fs::create_dir(&obstruction).unwrap();
    fs::write(obstruction.join("keep"), b"x").unwrap();

    assert!(Store::open(&db).is_err());

    assert!(!db.exists(), "a refused creation left a store file behind");
    assert_eq!(fs::read(obstruction.join("keep")).unwrap(), b"x");
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn a_store_created_through_a_dangling_symlink_does_not_inherit_an_orphan_csr() {
    let dir = tmp_dir("dangling-db");
    let real = dir.join("real.sqlite");
    let link = dir.join("devmap.sqlite");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    // The orphan sits beside whichever name the consumers derive their CSR from.
    write_orphan_csr(&sidecar(&link, ".csr"), 1, 7);
    write_orphan_csr(&sidecar(&real, ".csr"), 1, 7);

    let opened = Store::open(&link);
    if opened.is_err() {
        let _ = fs::remove_dir_all(dir);
        return; // refusing is acceptable
    }
    drop(opened);
    give_generation_one(&link);
    let store = Store::open(&link).unwrap();
    if let Some(reader) = store.open_latest_csr_sidecar().unwrap() {
        assert_eq!(reader.node_count(), 0, "an orphan CSR was served as the new store's graph");
    }
    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn a_read_only_parent_with_an_orphan_sidecar_refuses_and_changes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp_dir("readonly-parent");
    let db = dir.join("devmap.sqlite");
    let csr = sidecar(&db, ".csr");
    write_orphan_csr(&csr, 1, 3);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    if fs::write(dir.join("probe"), b"x").is_ok() {
        // Running as a user the mode does not bind (root): nothing to probe.
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(dir);
        return;
    }
    let result = Store::open(&db);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err(), "a store was created beside an orphan it could not remove");
    assert!(csr.is_file(), "the orphan was changed");
    assert!(!db.exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn opening_a_store_that_is_not_there_read_only_removes_nothing() {
    let dir = tmp_dir("readonly-absent");
    let db = dir.join("devmap.sqlite");
    let csr = sidecar(&db, ".csr");
    write_orphan_csr(&csr, 1, 3);
    let _ = Store::open_read_only(&db);
    let _ = Store::open_existing(&db);
    assert!(csr.is_file(), "a non-creating open removed a sidecar");
    assert!(!db.exists(), "a non-creating open created the store");
    let _ = fs::remove_dir_all(dir);
}

/// `seed::stage_donor_csr` falls through to `write_latest_csr_sidecar_strict` when
/// the donor's CSR is absent or rejected, and a sidecar already at the staged
/// path makes the donor copy place nothing. A symlink planted there must not be
/// written through to its target.
#[cfg(unix)]
#[test]
fn writing_the_fresh_csr_does_not_follow_a_symlink_planted_at_the_sidecar() {
    let dir = tmp_dir("write-through");
    let db = dir.join("devmap.sqlite");
    drop(Store::open(&db).unwrap());
    give_generation_one(&db);
    let store = Store::open(&db).unwrap();
    let outside = dir.join("outside.bin");
    fs::write(&outside, b"precious").unwrap();
    std::os::unix::fs::symlink(&outside, sidecar(&db, ".csr")).unwrap();

    let _ = store.write_latest_csr_sidecar_strict();

    assert_eq!(
        fs::read(&outside).unwrap(),
        b"precious",
        "the CSR write went through the symlink into its target"
    );
    let _ = fs::remove_dir_all(dir);
}
