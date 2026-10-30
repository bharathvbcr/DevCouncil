//! A store whose pages are damaged is reported as not ready, and `devmap build`
//! refuses it, names it, and leaves it alone, instead of answering "unchanged".
//!
//! Damage to a page of the full-text index's shadow tables (not an SQL-level
//! edit: the bytes of the page are overwritten, as a failing disk or a torn
//! write would) made the three commands an operator tries disagree:
//!
//! - `status` said `query_ready: true` over a store whose `search` errors;
//! - `build` compared hashes, found nothing changed and exited 0;
//! - both remedies the store's own message names failed with the same
//!   "database disk image is malformed": `repair --fts` and `build --full`.
//!
//! Setting the damaged file aside and building again was tried and parked
//! (`docs/devmap/design/auto-quarantine-parked.md`), so the two honesty halves
//! are what this file pins: `status` is not ready, and `build` says what it
//! found, where, and what to do, without writing to the store.
//!
//! `DEVMAP_ATTACK_BIN` points every test that runs `devmap` at another binary
//! (base `dbd0193a`), which is how the fail-before of the two halves is shown.
//!
//! The helpers are shared with the attack tests of the same shape
//! (`attack_ah_damaged_store`, `attack_dsr_quarantine_ownership`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// A worktree with a few Python symbols, in a directory of its own.
pub(crate) fn fixture(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "devmap-damaged-store-{label}-{}-{}",
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

/// What one `devmap` run said.
pub(crate) struct Run {
    pub(crate) ok: bool,
    pub(crate) json: serde_json::Value,
    pub(crate) stderr: String,
}

/// Run `devmap` from `cwd` against the store at `db`.
pub(crate) fn devmap_at(cwd: &Path, db: &Path, args: &[&str]) -> Run {
    let bin = std::env::var_os("DEVMAP_ATTACK_BIN").map_or_else(
        || PathBuf::from(env!("CARGO_BIN_EXE_devmap")),
        PathBuf::from,
    );
    let output = Command::new(bin)
        .current_dir(cwd)
        .env("DEVMAP_AUTOSPAWN", "0")
        .args(["--json", "--progress", "never", "--db"])
        .arg(db)
        .args(args)
        .output()
        .expect("run devmap");
    Run {
        ok: output.status.success(),
        json: serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Overwrite the one page of `table` with bytes that are not a b-tree page.
pub(crate) fn smash_page_of(db: &Path, table: &str) {
    use std::io::{Seek, SeekFrom, Write};
    let (root_page, page_size) = {
        let conn = rusqlite::Connection::open(db).unwrap();
        // Everything in the WAL must be in the main file before its bytes are
        // overwritten, or the damage lands in a file the reader never reads.
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

/// Remove the `-wal` and `-shm` beside `db`, so the damage is in the file a
/// reader opens and nothing newer shadows it.
pub(crate) fn drop_sidecars(db: &Path) {
    for ext in ["-wal", "-shm"] {
        let mut side = db.as_os_str().to_os_string();
        side.push(ext);
        let _ = fs::remove_file(PathBuf::from(side));
    }
}

/// Everything in `dir` that looks like a set-aside copy of a store.
pub(crate) fn set_aside(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".corrupt-"))
        })
        .collect()
}

/// A store file as it is on disk: its identity for "nothing touched it".
///
/// The bytes, and the (device, inode) the name resolves to, so a rename and
/// rewrite that happens to produce the same bytes is still a difference.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    bytes: Vec<u8>,
    dev_ino: (u64, u64),
}

pub(crate) fn identity(db: &Path) -> Identity {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(db).unwrap();
    Identity {
        bytes: fs::read(db).unwrap(),
        dev_ino: (meta.dev(), meta.ino()),
    }
}

/// The store at `db` is where it was, byte for byte and inode for inode, and
/// nothing beside it is a set-aside copy.
pub(crate) fn assert_untouched(what: &str, db: &Path, before: &Identity) {
    assert!(db.is_file(), "{what}: the store is not where it was");
    assert_eq!(
        &identity(db),
        before,
        "{what}: the store's bytes or inode changed"
    );
    let dir = db.parent().unwrap();
    assert!(
        set_aside(dir).is_empty(),
        "{what}: a copy was set aside: {:?}",
        set_aside(dir)
    );
}

/// Every shadow table of the full-text index. A page of any of them can be the
/// one that breaks: the term index and postings (`_data`, `_idx`) fail a search,
/// the rest (`_content`, `_docsize`, `_config`) fail the structure around it, and
/// `nodes_fts_map` is the generation-to-row map beside it.
const INDEX_TABLES: [&str; 6] = [
    "nodes_fts_data",
    "nodes_fts_idx",
    "nodes_fts_content",
    "nodes_fts_docsize",
    "nodes_fts_config",
    "nodes_fts_map",
];

/// Honesty half 1: `status` does not call a store whose index is damaged ready.
#[test]
fn status_is_not_ready_over_a_smashed_search_index_page() {
    for table in INDEX_TABLES {
        let root = fixture(table);
        let db = root.join("devmap.sqlite");
        assert!(devmap_at(&root, &db, &["build", "."]).ok);
        let healthy = devmap_at(&root, &db, &["status"]).json;
        assert_eq!(healthy["query_ready"], true, "{healthy}");
        assert_eq!(healthy["search_index_damaged"], false, "{healthy}");

        smash_page_of(&db, table);

        // Either it says so with the probe's finding, or (a damaged config page
        // fails the index's own constructor) it refuses to answer at all.
        let status = devmap_at(&root, &db, &["status"]);
        if status.ok {
            assert_eq!(
                status.json["query_ready"], false,
                "{table}: {}",
                status.json
            );
            assert_eq!(
                status.json["search_index_damaged"], true,
                "{table}: {}",
                status.json
            );
            assert!(
                status.json["degraded_reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("full-text index")),
                "{table}: {}",
                status.json
            );
        } else {
            assert!(
                status.stderr.contains("malformed"),
                "{table}: {}",
                status.stderr
            );
        }
        let _ = fs::remove_dir_all(&root);
    }
}

/// Honesty half 2: `build` does not answer "unchanged" over the damage, and does
/// not write to it. It refuses, naming the store, the finding and what to do.
#[test]
fn build_refuses_a_smashed_search_index_page_and_leaves_the_store_alone() {
    for table in INDEX_TABLES {
        let root = fixture(table);
        let db = root.join("devmap.sqlite");
        assert!(devmap_at(&root, &db, &["build", "."]).ok);
        smash_page_of(&db, table);
        let before = identity(&db);

        let built = devmap_at(&root, &db, &["build", "."]);
        assert!(
            !built.ok,
            "{table}: build answered over a damaged index: {}",
            built.json
        );
        assert_ne!(built.json["unchanged"], true, "{table}: {}", built.json);
        assert!(
            built.stderr.contains(&db.display().to_string())
                && built.stderr.contains("is damaged")
                && built.stderr.contains("left where it is")
                && built.stderr.contains("`devmap build`"),
            "{table}: the refusal names the store, the damage and the remedy: {}",
            built.stderr
        );
        assert_untouched(table, &db, &before);
        let _ = fs::remove_dir_all(&root);
    }
}

/// `build --full` died on the damage with a bare "malformed"; it now refuses the
/// same way, naming the store.
#[test]
fn build_full_refuses_the_damage_it_used_to_die_on() {
    let root = fixture("full");
    let db = root.join("devmap.sqlite");
    assert!(devmap_at(&root, &db, &["build", "."]).ok);
    smash_page_of(&db, "nodes_fts_idx");
    let before = identity(&db);
    let built = devmap_at(&root, &db, &["build", ".", "--full"]);
    assert!(!built.ok, "{}", built.json);
    assert!(
        built.stderr.contains(&db.display().to_string()) && built.stderr.contains("is damaged"),
        "{}",
        built.stderr
    );
    assert_untouched("build --full", &db, &before);
    let _ = fs::remove_dir_all(&root);
}

/// The refusal names the store's actual path, for each shape of damage the
/// attack rounds used: the page that holds the owner, the page that holds the
/// generations, and a page of the full-text index. The path is the one the
/// caller gave, never one derived from it.
#[test]
fn the_refusal_names_the_actual_store_path_for_each_damage_shape() {
    for (shape, table) in [
        ("owner page", "pending_state"),
        ("generations page", "generations"),
        ("full-text index page", "nodes_fts_idx"),
    ] {
        let root = fixture("shape");
        let db = root.join("devmap.sqlite");
        assert!(devmap_at(&root, &db, &["build", "."]).ok);
        smash_page_of(&db, table);
        drop_sidecars(&db);
        let before = identity(&db);

        let built = devmap_at(&root, &db, &["build", "."]);
        assert!(!built.ok, "{shape}: build went through: {}", built.json);
        let path = db.display().to_string();
        assert!(
            built
                .stderr
                .contains(&format!("the devmap store {path} is damaged")),
            "{shape}: the refusal does not name {path}: {}",
            built.stderr
        );
        assert!(
            built.stderr.contains("left where it is")
                && built
                    .stderr
                    .contains("move or delete it and run `devmap build`"),
            "{shape}: the refusal does not say what to do: {}",
            built.stderr
        );
        assert!(
            !built.stderr.contains(".corrupt-"),
            "{shape}: a derived path leaked into the refusal: {}",
            built.stderr
        );
        assert_untouched(shape, &db, &before);
        let _ = fs::remove_dir_all(&root);
    }
}

/// A healthy store is never refused or touched, and the build result carries no
/// recovery key: nothing is recovered any more.
#[test]
fn a_healthy_store_is_never_refused() {
    let root = fixture("healthy");
    let db = root.join("devmap.sqlite");
    assert!(devmap_at(&root, &db, &["build", "."]).ok);
    let again = devmap_at(&root, &db, &["build", "."]);
    assert!(again.ok);
    assert_eq!(again.json["unchanged"], true, "{}", again.json);
    let full = devmap_at(&root, &db, &["build", ".", "--full"]);
    assert!(full.ok, "{}", full.stderr);
    assert!(full.json.get("store_recovery").is_none(), "{}", full.json);
    assert!(set_aside(&root).is_empty());
    let _ = fs::remove_dir_all(&root);
}
