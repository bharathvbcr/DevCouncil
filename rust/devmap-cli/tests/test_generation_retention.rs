//! SC1 — generation retention is bounded on the real write paths.
//!
//! `Store::prune_generations_except_latest` existed and was correct long before
//! this test, but no production code called it: the only callers were unit
//! tests that invoked it directly. A repository therefore accumulated one full
//! carry-forward copy of itself per build forever — measured at +327 MiB per
//! one-line edit on a 4,731-file repository.
//!
//! These tests drive the `devmap` binary so they exercise the wired build path.
//! A test that calls `prune_generations_except_latest` directly cannot fail when
//! the call site is missing, which is exactly how the defect survived.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::{Store, GENERATION_RETENTION};

fn temp_root(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-retention-{tag}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    root
}

fn build(db: &std::path::Path, root: &std::path::Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--progress", "never", "--db"])
        .arg(db)
        .arg("build")
        .arg(root)
        .output()
        .expect("run build");
    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Row counts per generation-scoped table have no equivalent on the public
/// `Store` API, and adding a raw-SQL escape hatch to production code for tests
/// would be worse than reading the database directly here.
fn scalar(db: &std::path::Path, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).expect("open database for inspection");
    conn.query_row(sql, [], |row| row.get(0))
        .unwrap_or_else(|err| panic!("query failed: {sql}: {err}"))
}

/// Repeated builds must not accumulate generations without bound.
///
/// Fails against the pre-fix tree: without the prune call in the CLI build
/// path, six builds leave six generations.
#[test]
fn repeated_builds_retain_a_bounded_number_of_generations() {
    let root = temp_root("bounded");
    let db = root.join("index.sqlite");

    for round in 0..6 {
        // Change content each round so a real new generation is committed
        // rather than a no-op rebuild.
        fs::write(
            root.join("src/main.py"),
            format!("def main():\n    return {round}\n"),
        )
        .expect("write fixture");
        build(&db, &root);
    }

    let generations = scalar(&db, "SELECT COUNT(*) FROM generations");
    assert_eq!(
        generations, GENERATION_RETENTION as i64,
        "six builds must leave exactly {GENERATION_RETENTION} generations, found {generations}"
    );
}

/// Pruning must actually remove the rows, not merely the `generations` row.
///
/// The pre-existing prune tests asserted only the returned count, so a prune
/// that deleted the parent row and orphaned every child table would have passed
/// them. Each generation-scoped table is checked independently.
#[test]
fn pruned_generations_leave_no_rows_behind_in_any_table() {
    let root = temp_root("norows");
    let db = root.join("index.sqlite");

    for round in 0..5 {
        fs::write(
            root.join("src/main.py"),
            format!("def helper_{round}():\n    return {round}\n\ndef main():\n    return helper_{round}()\n"),
        )
        .expect("write fixture");
        build(&db, &root);
    }

    let surviving = scalar(&db, "SELECT MIN(id) FROM generations");
    assert!(surviving > 1, "expected early generations to be pruned");

    // Every generation-scoped table, plus the FTS index and its rowid map.
    for table in [
        "generation_files",
        "generation_nodes",
        "generation_edges",
        "generation_dead_symbols",
        "nodes_fts_map",
    ] {
        let orphans = scalar(
            &db,
            &format!("SELECT COUNT(*) FROM {table} WHERE generation_id < {surviving}"),
        );
        assert_eq!(
            orphans, 0,
            "{table} retained {orphans} rows for pruned generations below {surviving}"
        );
    }

    // The FTS index is deleted by rowid through nodes_fts_map. If the map were
    // dropped before the index, the FTS rows would be orphaned permanently and
    // unreachable — invisible to the per-table check above.
    let fts_rows = scalar(&db, "SELECT COUNT(*) FROM nodes_fts");
    let mapped_rows = scalar(&db, "SELECT COUNT(*) FROM nodes_fts_map");
    assert_eq!(
        fts_rows, mapped_rows,
        "nodes_fts holds {fts_rows} rows but nodes_fts_map maps {mapped_rows}; \
         pruning orphaned FTS rows"
    );
}

/// Pruning generations must not prune build history.
///
/// `build_history` is deliberately independent — no foreign key to
/// `generations`, its own 500-row cap — so `devmap history` keeps reporting
/// builds whose generations are long gone. This pins that independence.
#[test]
fn build_history_survives_generation_pruning() {
    let root = temp_root("history");
    let db = root.join("index.sqlite");

    const ROUNDS: i64 = 5;
    for round in 0..ROUNDS {
        fs::write(
            root.join("src/main.py"),
            format!("def main():\n    return {round}\n"),
        )
        .expect("write fixture");
        build(&db, &root);
    }

    let history = scalar(&db, "SELECT COUNT(*) FROM build_history");
    assert_eq!(
        history, ROUNDS,
        "history must record all {ROUNDS} builds after pruning, found {history}"
    );

    let generations = scalar(&db, "SELECT COUNT(*) FROM generations");
    assert!(
        history > generations,
        "history ({history}) should outlive retained generations ({generations})"
    );
}

/// SC7 — superseded cache entries must not accumulate forever.
///
/// `extraction_cache` is keyed by content hash, so every edit adds a row and
/// nothing ever deleted from the table. Five edits to one file left five rows;
/// on a 4,742-file repository the table reached 198 MiB of a 525 MiB database.
///
/// Fails against the pre-fix tree: without `prune_extraction_cache`, this
/// asserts 1 cached entry for the churned file and finds 6.
#[test]
fn superseded_extraction_cache_entries_are_evicted() {
    let root = temp_root("cache");
    let db = root.join("index.sqlite");

    // One file that never changes, one edited on every build.
    fs::write(root.join("src/stable.py"), "def stable():\n    return 1\n")
        .expect("write stable fixture");

    for round in 0..6 {
        fs::write(
            root.join("src/churn.py"),
            format!("def churn():\n    return {round}\n"),
        )
        .expect("write churn fixture");
        build(&db, &root);
    }

    // Retention keeps GENERATION_RETENTION generations, so at most that many
    // distinct contents of the churned file are still referenced.
    let churn_entries = scalar(
        &db,
        "SELECT COUNT(*) FROM extraction_cache WHERE payload_json LIKE '%churn.py%'",
    );
    assert!(
        churn_entries <= GENERATION_RETENTION as i64,
        "six edits left {churn_entries} cached extractions for one file; \
         superseded entries are not being evicted"
    );

    // Eviction must never discard the extraction the next build needs. The
    // stable file is the sharp case: its entry is the oldest in the store and
    // also the one still required, so a recency-based policy would drop exactly
    // the wrong one.
    //
    // Asserted on *retrievability*, not on which table holds it. Since SC8 the
    // canonical copy lives in `generation_files` and the cache row is evicted
    // as redundant — a strictly better outcome that an assertion pinned to
    // `extraction_cache` would have read as a regression. What must stay true
    // is that the payload is still there to be found.
    let stable_available = scalar(
        &db,
        "SELECT COUNT(*) FROM generation_files gf
         JOIN paths p ON p.id = gf.file_id
         WHERE p.path LIKE '%stable.py'
           AND gf.grammar_version IS NOT NULL
           AND gf.analyzer_version IS NOT NULL",
    );
    assert!(
        stable_available >= 1,
        "the unchanged file's extraction must remain retrievable with a full \
         cache identity after eviction, found {stable_available}"
    );

    // And nothing may be left holding a payload with no recorded identity,
    // which could never be served and is therefore pure waste.
    let unusable = scalar(
        &db,
        "SELECT COUNT(*) FROM generation_files
         WHERE grammar_version IS NULL OR analyzer_version IS NULL",
    );
    assert_eq!(
        unusable, 0,
        "{unusable} generation rows carry no cache identity and can never be reused"
    );
}

/// The latest generation must remain fully queryable after a prune.
///
/// Pruning is only safe because the differential builder carries every
/// surviving row forward into the new generation. If carry-forward and pruning
/// ever disagree, the newest generation loses rows it still needs.
///
/// Unlike the three tests above, this one **passes against the pre-fix tree**:
/// an unpruned store also has a complete latest generation. It is a guard
/// against pruning breaking carry-forward in future, not evidence that pruning
/// happens. The other three are the falsifying tests.
#[test]
fn latest_generation_remains_complete_after_pruning() {
    let root = temp_root("complete");
    let db = root.join("index.sqlite");

    fs::write(
        root.join("src/stable.py"),
        "def stable_helper():\n    return 1\n",
    )
    .expect("write stable fixture");

    for round in 0..5 {
        fs::write(
            root.join("src/churn.py"),
            format!("import stable\n\ndef churn():\n    return {round}\n"),
        )
        .expect("write churn fixture");
        build(&db, &root);
    }

    let store = Store::open(&db).expect("reopen store");

    // stable.py was written once, before generation 1, and never touched again.
    // It survives only via carry-forward, so it is the file most likely to be
    // lost if pruning removed a generation the latest one still depended on.
    assert!(
        store
            .latest_file("src/stable.py")
            .expect("query latest file")
            .is_some(),
        "carried-forward file vanished from the latest generation after pruning"
    );

    let latest = store
        .latest_generation_id()
        .expect("query latest generation")
        .expect("a generation exists");
    let files = scalar(
        &db,
        &format!("SELECT COUNT(*) FROM generation_files WHERE generation_id = {latest}"),
    );
    assert_eq!(
        files, 2,
        "latest generation must hold both fixture files, found {files}"
    );
}
