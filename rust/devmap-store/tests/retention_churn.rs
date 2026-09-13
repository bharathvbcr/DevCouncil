//! Retention must bound interned paths as well as generation payloads.
//! The old fixed-path growth probe could not observe rename/delete churn.

use devmap_analyze::analyze;
use devmap_extract::extract_file;
use devmap_resolve::Resolver;
use devmap_store::Store;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    store: Store,
    inspect: Connection,
    // Fields drop in declaration order: close both connections before
    // removing the directory, including on platforms that lock open files.
    _scratch: Scratch,
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("retained failing store at {}", self.0.display());
            return;
        }
        fs::remove_dir_all(&self.0).unwrap();
    }
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "devmap-retention-churn-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        let database = root.join("store.sqlite");
        Self {
            store: Store::open(&database).unwrap(),
            inspect: Connection::open(&database).unwrap(),
            _scratch: Scratch(root),
        }
    }

    fn commit(&self, files: &[(&str, &str)]) -> u32 {
        let extractions: Vec<_> = files
            .iter()
            .map(|(path, body)| extract_file(path, body))
            .collect();
        let mut resolver = Resolver::new();
        resolver.index_extractions(&extractions);
        let resolution = resolver.resolve_all(&extractions);
        let analysis = analyze(&extractions, &resolution);
        self.store
            .save_generation(&extractions, &resolution, &analysis)
            .unwrap()
    }

    fn paths(&self) -> Vec<String> {
        self.inspect
            .prepare("SELECT path FROM paths ORDER BY path")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn assert_integrity(&self) {
        // Verify committed disk state through a fresh connection. SQLite
        // 3.53.2's FTS integrity hook can hold stale segment metadata after
        // another connection optimizes the index; actual persistent search
        // is exercised separately below.
        let inspect = Connection::open(self._scratch.0.join("store.sqlite")).unwrap();
        let errors: i64 = inspect
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(errors, 0);
        let integrity: String = inspect
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok", "SQLite {}", rusqlite::version());
    }
}

#[test]
fn a_persistent_search_reader_observes_renames_after_fts_pruning() {
    let fixture = Fixture::new();
    fixture.commit(&[("initial.py", "def old_symbol(): return 1\n")]);
    let reader = Store::open_read_only(fixture._scratch.0.join("store.sqlite")).unwrap();
    assert!(!reader.search_fts("old_symbol", 10).unwrap().is_empty());
    for round in 0..32 {
        let path = format!("new_{round}.py");
        let source = format!("def new_symbol_{round}(): return {round}\n");
        fixture.commit(&[(&path, &source)]);
        fixture.store.prune_generations_except_latest(1).unwrap();
        assert!(reader.search_fts("old_symbol", 10).unwrap().is_empty());
        let found = reader
            .search_fts(&format!("new_symbol_{round}"), 10)
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, format!("new_symbol_{round}"));
        assert_eq!(found[0].2, path);
        fixture.assert_integrity();
    }
}

#[test]
fn repeated_renames_bound_paths_to_the_retained_generations() {
    let fixture = Fixture::new();
    for round in 0..64 {
        let path = format!("renamed_{round:03}.py");
        fixture.commit(&[(
            &path,
            "def entry():\n    return target()\ndef target(): return 1\n",
        )]);
        fixture.store.prune_generations_except_latest(2).unwrap();
    }
    assert_eq!(
        fixture.paths(),
        ["renamed_062.py", "renamed_063.py"],
        "paths must plateau with retained files; every historical rename used to survive"
    );
    fixture.assert_integrity();
}

#[test]
fn deleting_every_file_releases_paths_after_the_last_referencing_generation() {
    let fixture = Fixture::new();
    fixture.commit(&[("deleted.py", "def old(): return 1\n")]);
    fixture.commit(&[]);
    assert_eq!(fixture.store.prune_generations_except_latest(2).unwrap(), 0);
    assert_eq!(fixture.paths(), ["deleted.py"]);
    fixture.commit(&[]);
    assert_eq!(fixture.store.prune_generations_except_latest(2).unwrap(), 1);
    assert!(
        fixture.paths().is_empty(),
        "deleted files remain interned forever"
    );
    fixture.assert_integrity();
}

#[test]
fn path_retirement_failure_rolls_back_the_entire_generation_prune() {
    let fixture = Fixture::new();
    for path in ["old.py", "middle.py", "current.py"] {
        fixture.commit(&[(path, "def same(): return 1\n")]);
    }
    let before: (i64, i64, i64, i64) = fixture
        .inspect
        .query_row(
            "SELECT (SELECT count(*) FROM generations),
                (SELECT count(*) FROM generation_nodes),
                (SELECT count(*) FROM file_payloads),
                (SELECT count(*) FROM nodes_fts)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(before.0, 3);
    assert!(before.1 > 0 && before.2 > 0 && before.3 > 0);
    fixture
        .inspect
        .execute_batch(
            "CREATE TRIGGER refuse_path_retirement BEFORE DELETE ON paths
         BEGIN SELECT RAISE(ABORT, 'injected path retirement failure'); END;",
        )
        .unwrap();
    let error = fixture
        .store
        .prune_generations_except_latest(1)
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("injected path retirement failure"));
    let counts: (i64, i64, i64, i64) = fixture
        .inspect
        .query_row(
            "SELECT (SELECT count(*) FROM generations),
                (SELECT count(*) FROM generation_nodes),
                (SELECT count(*) FROM file_payloads),
                (SELECT count(*) FROM nodes_fts)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(counts, before);
    assert_eq!(fixture.paths(), ["current.py", "middle.py", "old.py"]);
    fixture
        .inspect
        .execute_batch("DROP TRIGGER refuse_path_retirement")
        .unwrap();
    assert_eq!(fixture.store.prune_generations_except_latest(1).unwrap(), 2);
    assert_eq!(fixture.paths(), ["current.py"]);
    fixture.assert_integrity();
}

#[test]
fn an_active_reader_keeps_its_paths_until_its_snapshot_ends() {
    let fixture = Fixture::new();
    fixture.commit(&[("old.py", "def old(): return 1\n")]);
    fixture.inspect.execute_batch("BEGIN").unwrap();
    assert_eq!(fixture.paths(), ["old.py"]);
    fixture.commit(&[("new.py", "def new(): return 1\n")]);
    fixture.store.prune_generations_except_latest(1).unwrap();
    assert_eq!(fixture.paths(), ["old.py"]);
    fixture.inspect.execute_batch("COMMIT").unwrap();
    assert_eq!(fixture.paths(), ["new.py"]);
    fixture.assert_integrity();
}

#[test]
fn edge_only_paths_live_until_their_last_retained_edge_range_ends() {
    let fixture = Fixture::new();
    fixture.commit(&[("old.py", "def old(): return 1\n")]);
    let latest = fixture.commit(&[("current.py", "def current(): return 1\n")]);
    let source = fixture
        .store
        .get_or_create_path_id("edge_source.py")
        .unwrap();
    let target = fixture
        .store
        .get_or_create_path_id("edge_target.py")
        .unwrap();
    fixture
        .inspect
        .execute(
            "INSERT INTO edge_rows (source_file_id,target_file_id,source_symbol,target_symbol,
                               edge_kind,confidence,valid_from,valid_to)
         VALUES (?1,?2,'source','target','call',1,?3,NULL)",
            rusqlite::params![source, target, latest],
        )
        .unwrap();
    fixture.store.prune_generations_except_latest(1).unwrap();
    assert_eq!(
        fixture.paths(),
        ["current.py", "edge_source.py", "edge_target.py"]
    );
    fixture.assert_integrity();
    fixture.commit(&[("current.py", "def current(): return 1\n")]);
    fixture.store.prune_generations_except_latest(1).unwrap();
    assert_eq!(fixture.paths(), ["current.py"]);
    fixture.assert_integrity();
}
