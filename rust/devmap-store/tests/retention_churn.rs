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
        let resolution = resolver.resolve_all(&extractions).unwrap();
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

// ---------------------------------------------------------------------------
// Pool retirement at ledger scale.
//
// `unresolved_rows` declares three foreign keys (`source_file_id` -> `paths`,
// `reason_id` and `classification_id` -> `unresolved_texts`) and indexes none of
// them as a leading column. With `foreign_keys = ON`, every `paths` or
// `unresolved_texts` row the prune retires makes SQLite scan the whole child
// table to prove nothing still names it, so retiring K orphans costs K full
// scans of the ledger. That is the shape of the 835 s prune on a large tree
// after a rename or branch switch.

/// Plant a ledger of `rows` unresolved calls over `texts` interned texts and
/// `paths` interned paths, by raw SQL.
///
/// The first half of the key space (`k % texts < texts / 2`) is closed at
/// generation 2, so a prune that keeps one generation deletes those rows. Row
/// `k` names path `(k % texts) * paths / texts`, reason `k % texts` and
/// classification `(k + 1) % texts`: the closed half therefore strands about
/// `texts / 2` texts and `paths / 2` paths, and the open half keeps the rest.
fn plant_ledger(inspect: &Connection, rows: usize, texts: usize, paths: usize) {
    inspect
        .execute_batch(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < {texts})
             INSERT INTO unresolved_texts (text) SELECT 'planted-text-' || i FROM n;
             WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < {paths})
             INSERT INTO paths (path) SELECT 'planted/path-' || i || '.py' FROM n;
             WITH RECURSIVE n(k) AS (SELECT 0 UNION ALL SELECT k + 1 FROM n WHERE k + 1 < {rows})
             INSERT INTO unresolved_rows
                 (source_file_id, source_symbol, callee_name, reason_id, classification_id,
                  receiver, valid_from, valid_to)
             SELECT (SELECT id FROM paths
                      WHERE path = 'planted/path-' || ((k % {texts}) * {paths} / {texts}) || '.py'),
                    'sym', 'callee_' || (k % 97),
                    (SELECT id FROM unresolved_texts WHERE text = 'planted-text-' || (k % {texts})),
                    (SELECT id FROM unresolved_texts
                      WHERE text = 'planted-text-' || ((k + 1) % {texts})),
                    NULL, 1,
                    CASE WHEN (k % {texts}) < {texts} / 2 THEN 2 ELSE NULL END
               FROM n;"
        ))
        .unwrap();
}

/// The ids a prune that keeps one generation retires from a ledger planted by
/// [`plant_ledger`], worked out from the layout and not from the prune's own
/// predicates. Rows with `k % texts < texts / 2` are closed. They name reasons
/// `0..texts/2` and classifications `1..=texts/2`; the open rows name reasons
/// `texts/2..texts` and classifications `texts/2+1..texts` plus text `0` (row
/// `texts - 1` wraps), so the texts named only by closed rows are
/// `1..texts/2`: `texts / 2 - 1` of them. Path `i` is named by the rows with
/// `k % texts` in `[i * texts / paths, (i + 1) * texts / paths)`, so the first
/// `paths / 2` paths are named only by closed rows. Needs `texts % paths == 0`
/// and an even `paths`.
fn expected_orphans(texts: i64, paths: i64) -> (i64, i64) {
    assert!(texts % paths == 0 && paths % 2 == 0);
    (texts / 2 - 1, paths / 2)
}

/// Run `prune_generations_except_latest(keep)` on its own connection and give up
/// waiting after `budget`. A detached thread keeps running if the budget is
/// exceeded (the test binary exits when the harness finishes), so a regression
/// fails in `budget` instead of hanging the suite. `Store` exposes no SQLite
/// interrupt handle, and a wall-clock bound with no way to stop the work would
/// only measure the load average.
fn prune_within(database: PathBuf, keep: usize, budget: std::time::Duration) -> Option<usize> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let store = Store::open(&database).unwrap();
        let _ = sender.send(store.prune_generations_except_latest(keep).unwrap());
    });
    receiver.recv_timeout(budget).ok()
}

#[test]
fn retiring_thousands_of_orphans_does_not_scan_the_ledger_once_per_orphan() {
    let fixture = Fixture::new();
    for round in 0..3 {
        fixture.commit(&[(&format!("kept_{round}.py"), "def entry(): return 1\n")]);
    }
    plant_ledger(&fixture.inspect, 100_000, 10_000, 400);
    let (orphan_texts, orphan_paths) = expected_orphans(10_000, 400);
    let total_before: i64 = fixture
        .inspect
        .query_row("SELECT count(*) FROM unresolved_rows", [], |row| row.get(0))
        .unwrap();

    let database = fixture._scratch.0.join("store.sqlite");
    let pruned = prune_within(database, 1, std::time::Duration::from_secs(10));
    assert_eq!(
        pruned,
        Some(2),
        "prune did not finish inside its budget: retiring {orphan_texts} texts and \
         {orphan_paths} paths scanned `unresolved_rows` once per orphan"
    );

    let (texts_left, paths_left): (i64, i64) = fixture
        .inspect
        .query_row(
            "SELECT (SELECT count(*) FROM unresolved_texts WHERE text LIKE 'planted-text-%'),
                    (SELECT count(*) FROM paths WHERE path LIKE 'planted/%')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(texts_left, 10_000 - orphan_texts);
    assert_eq!(paths_left, 400 - orphan_paths);
    let ledger_left: i64 = fixture
        .inspect
        .query_row("SELECT count(*) FROM unresolved_rows", [], |row| row.get(0))
        .unwrap();
    assert_eq!(ledger_left, total_before / 2, "open rows must survive whole");
    fixture.assert_integrity();
}

/// With foreign keys off for the prune, the constraint no longer refuses a
/// retirement that leaves a row naming a deleted parent. The prune checks for
/// that itself before it commits, and rolls back whole.
#[test]
fn a_retirement_that_would_dangle_a_ledger_row_fails_loudly_and_rolls_back() {
    let fixture = Fixture::new();
    for path in ["old.py", "middle.py", "current.py"] {
        fixture.commit(&[(path, "def same(): return 1\n")]);
    }
    // Models an anti-join that forgot a relation: the retirement of any path
    // leaves a ledger row naming it.
    fixture
        .inspect
        .execute_batch(
            "INSERT INTO unresolved_texts (id, text) VALUES (1000000, 'dangler');
             CREATE TRIGGER dangle AFTER DELETE ON paths
             BEGIN
                 INSERT INTO unresolved_rows
                     (source_file_id, source_symbol, callee_name, reason_id,
                      classification_id, receiver, valid_from, valid_to)
                 VALUES (OLD.id, 's', 'c', 1000000, 1000000, NULL, 1, NULL);
             END;",
        )
        .unwrap();
    let error = fixture
        .store
        .prune_generations_except_latest(1)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("foreign_key_check") && error.contains("paths"),
        "the refusal must say which check failed and which parent dangled: {error}"
    );
    let generations: i64 = fixture
        .inspect
        .query_row("SELECT count(*) FROM generations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(generations, 3, "a refused prune must roll back whole");
    assert_eq!(fixture.paths(), ["current.py", "middle.py", "old.py"]);
    fixture.inspect.execute_batch("DROP TRIGGER dangle").unwrap();
    fixture.assert_integrity();
}

/// The two retirement predicates as `prune_generations_except_latest` issues
/// them, read from its source, so the tests below cannot drift from the code
/// they describe. Whitespace is collapsed so the statement can be laid out as
/// the code wishes.
fn issued_predicate(table: &str) -> String {
    let source = include_str!("../src/db/maintenance.rs");
    let start = source
        .find("pub fn prune_generations_except_latest")
        .expect("prune is defined in maintenance.rs");
    let body = &source[start..];
    let opener = format!("\"DELETE FROM {table} WHERE ");
    let at = body
        .find(&opener)
        .unwrap_or_else(|| panic!("prune no longer issues `DELETE FROM {table} WHERE ...`"));
    let rest = &body[at + opener.len()..];
    let end = rest.find('"').expect("the statement ends");
    rest[..end].split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The predicates before the rewrite: `NOT IN` over a `UNION` of the id columns.
const UNION_PATHS: &str = "id NOT IN (
    SELECT file_id FROM file_payloads UNION SELECT file_id FROM file_rows
    UNION SELECT file_id FROM digest_rows UNION SELECT source_file_id FROM edge_rows
    UNION SELECT target_file_id FROM edge_rows UNION SELECT source_file_id FROM unresolved_rows)";
const UNION_TEXTS: &str = "id NOT IN (
    SELECT reason_id FROM unresolved_rows UNION SELECT classification_id FROM unresolved_rows)";

fn plan_steps(conn: &Connection, sql: &str) -> Vec<String> {
    conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The union form makes SQLite build a sorted, de-duplicated temp b-tree of the
/// ids before it can answer a single membership test. Separate `NOT IN`s each
/// probe a rowid-ordered ephemeral index that SQLite fills only when it is first
/// consulted, so a path the first test already keeps never reaches the later
/// ones. The lazy materialisation is planner behaviour the interleaved A/B
/// measures, not something `EXPLAIN QUERY PLAN` can show; what the plan can pin
/// is that the sort is gone.
#[test]
fn the_pool_retirement_statements_plan_without_a_union_or_a_temp_btree() {
    let fixture = Fixture::new();
    let sorted = |steps: &[String]| {
        steps
            .iter()
            .any(|step| step.contains("UNION") || step.contains("TEMP B-TREE"))
    };
    for (table, union, issued) in [
        ("paths", UNION_PATHS, issued_predicate("paths")),
        ("unresolved_texts", UNION_TEXTS, issued_predicate("unresolved_texts")),
    ] {
        let old = plan_steps(&fixture.inspect, &format!("DELETE FROM {table} WHERE {union}"));
        assert!(
            sorted(&old),
            "premise: the union form of {table} is expected to plan a sort, got {old:?}"
        );
        let new = plan_steps(&fixture.inspect, &format!("DELETE FROM {table} WHERE {issued}"));
        assert!(
            !sorted(&new),
            "prune retires {table} with a sorted union, got {new:?} for `{issued}`"
        );
    }
}

fn ids(conn: &Connection, table: &str, predicate: &str) -> Vec<i64> {
    conn.prepare(&format!("SELECT id FROM {table} WHERE {predicate} ORDER BY id"))
        .unwrap_or_else(|error| panic!("{predicate}: {error}"))
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Every relation that keeps a path or a text alive, each alone, and one id of
/// each kind that nothing keeps: the rewritten predicates must select the same
/// ids the union forms did, and exactly the unreferenced ones.
#[test]
fn the_rewritten_anti_joins_select_the_ids_the_union_forms_selected() {
    let fixture = Fixture::new();
    let conn = &fixture.inspect;
    conn.execute_batch(
        "INSERT INTO paths (id, path) VALUES
            (1, 'p/payload.py'), (2, 'p/file_rows.py'), (3, 'p/digest.py'),
            (4, 'p/edge_source.py'), (5, 'p/edge_target.py'), (6, 'p/unresolved.py'),
            (7, 'p/none.py');
         INSERT INTO unresolved_texts (id, text) VALUES
            (1, 'reason only'), (2, 'classification only'), (3, 'both'), (4, 'none');
         INSERT INTO file_payloads
            (payload_id, file_id, content_hash, language, grammar_version,
             analyzer_version, parse_outcome_json, engine_json, extraction_json)
            VALUES (1, 1, 1, 'python', 'g', 'a', '\"Clean\"', '\"TreeSitter\"', '{}');
         INSERT INTO file_rows (file_id, payload_id, valid_from, valid_to) VALUES (2, 1, 1, NULL);
         INSERT INTO digest_rows
            (file_id, edge_rows, edge_lo, edge_hi, unresolved_rows, unresolved_lo,
             unresolved_hi, valid_from, valid_to) VALUES (3, 0, 0, 0, 0, 0, 0, 1, NULL);
         INSERT INTO edge_rows (source_file_id, target_file_id, source_symbol, target_symbol,
                                edge_kind, confidence, valid_from, valid_to)
            VALUES (4, 5, 's', 't', 'call', 1, 1, NULL);
         INSERT INTO unresolved_rows (source_file_id, source_symbol, callee_name, reason_id,
                                      classification_id, receiver, valid_from, valid_to)
            VALUES (6, 's', 'a', 1, 3, NULL, 1, NULL), (6, 's', 'b', 3, 2, NULL, 1, NULL);",
    )
    .unwrap();

    let paths = issued_predicate("paths");
    assert_eq!(ids(conn, "paths", &paths), ids(conn, "paths", UNION_PATHS));
    assert_eq!(ids(conn, "paths", &paths), [7], "only the unreferenced path");
    let texts = issued_predicate("unresolved_texts");
    assert_eq!(
        ids(conn, "unresolved_texts", &texts),
        ids(conn, "unresolved_texts", UNION_TEXTS)
    );
    assert_eq!(
        ids(conn, "unresolved_texts", &texts),
        [4],
        "only the unreferenced text"
    );
}

/// A text can become unreferenced only when a ledger row naming it is deleted,
/// so a prune that deletes none has nothing to retire from the pool and does not
/// scan for it. (A text interned by a write whose row was then deduplicated
/// waits for the next prune that does delete ledger rows; the pool stays
/// bounded by that prune, and the early return for a prune that drops no
/// generation already works the same way.)
#[test]
fn a_prune_that_deletes_no_ledger_row_leaves_the_text_pool_alone() {
    let fixture = Fixture::new();
    for _ in 0..3 {
        fixture.commit(&[("same.py", "def same(): return 1\n")]);
    }
    fixture
        .inspect
        .execute("INSERT INTO unresolved_texts (text) VALUES ('planted orphan')", [])
        .unwrap();
    assert_eq!(fixture.store.prune_generations_except_latest(1).unwrap(), 2);
    let kept: i64 = fixture
        .inspect
        .query_row(
            "SELECT count(*) FROM unresolved_texts WHERE text = 'planted orphan'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1, "the pool was scanned although no ledger row was deleted");
    fixture.assert_integrity();
}

#[test]
fn a_prune_that_deletes_closed_ledger_rows_retires_the_texts_and_paths_only_they_named() {
    let fixture = Fixture::new();
    for round in 0..3 {
        fixture.commit(&[(&format!("kept_{round}.py"), "def entry(): return 1\n")]);
    }
    plant_ledger(&fixture.inspect, 400, 40, 8);
    let (orphan_texts, orphan_paths) = expected_orphans(40, 8);
    assert_eq!(fixture.store.prune_generations_except_latest(1).unwrap(), 2);
    let (texts_left, paths_left): (i64, i64) = fixture
        .inspect
        .query_row(
            "SELECT (SELECT count(*) FROM unresolved_texts WHERE text LIKE 'planted-text-%'),
                    (SELECT count(*) FROM paths WHERE path LIKE 'planted/%')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(texts_left, 40 - orphan_texts);
    assert_eq!(paths_left, 8 - orphan_paths);
    fixture.assert_integrity();
}
