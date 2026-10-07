//! The store hardening cases that need no grammar: the pending queue, schema
//! migration and refusal, the vacuum policy, and path admission.
//!
//! Split from `store_hardening.rs`, whose other cases build their generations
//! by extracting source and so are declared `required-features = ["parse"]`.
//! These are the ones a build without the parsing frontend still runs.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use devmap_store::{Store, CURRENT_SCHEMA_VERSION};

/// A 1 us `SystemTime` tick is not a unique key: same-microsecond callers used
/// to collide on one path, and the `remove_dir_all` below then deleted a live
/// sibling test's fixture. pid plus a monotonic counter make the name unique.
fn tmp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "devmap-{label}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pending_queue_survives_reopen() {
    let dir = tmp_dir("pending");
    let db = dir.join("index.sqlite");
    {
        let store = Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&["a.py".into(), "b.py".into()])
            .unwrap();
    }
    let store = Store::open(&db).unwrap();
    let pending = store.get_pending_paths().unwrap();
    assert_eq!(pending.len(), 2);
    store.clear_pending_paths(&["a.py".into()]).unwrap();
    assert_eq!(store.get_pending_paths().unwrap(), vec!["b.py".to_string()]);
    let _ = fs::remove_dir_all(&dir);
}

/// A watcher event arriving mid-drain is newer work and must survive the
/// acknowledgement of the batch it interrupted.
///
/// The guard used to be `attempts > 0`, which forced the drain to charge an
/// attempt to every path it was about to succeed at just to make the delete
/// fire — and that accounting is what let one store-level failure quarantine a
/// whole batch (K1(d)). The guard is now the `queued_at` the row was claimed
/// at, which re-enqueueing moves. Same invariant, without the side effect.
#[test]
fn requeued_path_survives_post_attempt_acknowledgement() {
    let store = Store::open_in_memory().unwrap();
    store.enqueue_pending_paths(&["a.py".to_string()]).unwrap();
    let claimed = store.claim_pending_batch(16).unwrap();
    assert_eq!(claimed.len(), 1);

    // A watcher event arriving during the rebuild represents newer work and
    // moves the queue timestamp. Acknowledging the old claim must preserve it.
    std::thread::sleep(std::time::Duration::from_millis(5));
    store.enqueue_pending_paths(&["a.py".to_string()]).unwrap();
    assert_eq!(store.clear_claimed_pending_paths(&claimed).unwrap(), 0);
    assert_eq!(store.get_pending_paths().unwrap(), vec!["a.py".to_string()]);
    assert_eq!(
        store.pending_attempts("a.py").unwrap(),
        Some(0),
        "claiming and acknowledging must not charge a failed attempt"
    );
}

#[test]
fn test_s7_quarantined_poison_path_stops_hot_loop_until_a_new_event() {
    let store = Store::open_in_memory().unwrap();
    let path = "poison.py".to_string();
    store
        .enqueue_pending_paths(std::slice::from_ref(&path))
        .unwrap();
    for _ in 0..5 {
        store
            .bump_pending_attempts(&store.claim_pending_batch(1).unwrap())
            .unwrap();
    }

    assert!(
        store.get_pending_paths().unwrap().is_empty(),
        "a quarantined path must not be selected for another immediate retry"
    );
    let status = store.status(":memory:").unwrap();
    assert_eq!(status.pending_count, 1);
    assert_eq!(status.quarantined_count, 1);

    // A new filesystem event is new evidence and resets the retry state.
    store
        .enqueue_pending_paths(std::slice::from_ref(&path))
        .unwrap();
    assert_eq!(store.get_pending_paths().unwrap(), [path]);
}

#[test]
fn test_s2_migration_v4_to_v5_preserves_generations_and_adds_analysis() {
    let dir = tmp_dir("migration-v5");
    let db_path = dir.join("legacy-v4.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE generations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at REAL NOT NULL,
                head_sha TEXT
            );
            INSERT INTO generations (created_at, head_sha) VALUES (1.0, 'legacy-head');
            PRAGMA user_version = 4;",
        )
        .unwrap();
    }
    {
        let store = Store::open(&db_path).unwrap();
        assert_eq!(store.latest_generation_id().unwrap(), Some(1));
        let analysis = store.latest_analysis().unwrap().unwrap();
        assert_eq!(analysis.total_files, 0);
        assert!(analysis.dead_symbols.is_empty());
    }
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_s2_initial_schema_failure_rolls_back_every_prior_statement() {
    let dir = tmp_dir("initial-schema-rollback");
    let db_path = dir.join("conflict.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE VIEW generations AS SELECT 1 AS id;")
            .unwrap();
    }

    assert!(
        Store::open(&db_path).is_err(),
        "a conflicting schema object must fail initialization"
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 0, "failed initialization must not bump the schema");
    let paths_created: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'paths'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        paths_created, 0,
        "statements before the injected conflict must roll back atomically"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn future_schema_version_is_rejected_fail_closed() {
    let dir = tmp_dir("future-schema");
    let db_path = dir.join("future.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 99", []).unwrap();
    }

    assert!(
        Store::open(&db_path).is_err(),
        "a newer schema must not be opened with an older binary"
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let created_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table', 'index', 'trigger') AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        created_tables, 0,
        "rejecting a future schema must not mutate it before failing"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The vacuum policy itself, asserted directly.
///
/// Every case below corresponds to a mutant that survived when the policy was
/// only reachable through `vacuum_if_needed`'s side effect: `&&` → `||`, `/` →
/// `*`, `/` → `%`, and the `>` boundaries. A full VACUUM takes an exclusive
/// lock and rewrites the whole file, so vacuuming when it is not warranted is
/// a real defect that produces the same page counts as behaving correctly.
#[test]
fn vacuum_policy_triggers_only_above_the_freelist_ratio() {
    // Empty database: no pages, nothing to reclaim. Kills `&&` -> `||`, which
    // would vacuum on ratio alone with a zero page count.
    assert!(
        !Store::should_vacuum(0, 0),
        "empty database must not vacuum"
    );
    assert!(
        !Store::should_vacuum(10, 0),
        "a zero page count must never vacuum, whatever the freelist says"
    );

    // Below threshold: 5 free of 1000 pages is 0.5%.
    assert!(
        !Store::should_vacuum(5, 1000),
        "0.5% free must not justify an exclusive-lock rewrite"
    );

    // Above threshold: 100 free of 1000 pages is 10%.
    assert!(
        Store::should_vacuum(100, 1000),
        "10% free must reclaim space"
    );

    // Exactly at the threshold is not above it. Kills `>` -> `>=`.
    let at_threshold = (1000.0 * Store::VACUUM_FREELIST_RATIO) as i64;
    assert!(
        !Store::should_vacuum(at_threshold, 1000),
        "the ratio bound is exclusive: exactly {}% must not vacuum",
        Store::VACUUM_FREELIST_RATIO * 100.0
    );
    assert!(
        Store::should_vacuum(at_threshold + 1, 1000),
        "one page past the bound must vacuum"
    );

    // `/` -> `*` and `/` -> `%` both explode on a small freelist against a
    // large page count, so this single case pins the operator.
    assert!(
        !Store::should_vacuum(1, 100_000),
        "one free page in 100k must not vacuum; the ratio must be a division"
    );
}

/// S-10: a backslash is a legal character in a Unix filename, not a separator.
///
/// `canonical_pending_entry` normalised `\` to `/` unconditionally, so the
/// queued path `a\b.py` became `a/b.py`: a *different* file, which then failed
/// classification because nothing is at that path, and was deleted from the
/// queue as garbage. The file was never indexed and `status` still reported
/// fresh — the queue's own record of the outstanding work was destroyed by the
/// normalisation meant to canonicalise it.
#[cfg(unix)]
#[test]
fn s10_a_unix_filename_containing_a_backslash_is_not_rewritten_into_another_path() {
    let dir = tmp_dir("s10-backslash");
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    let odd = "a\\b.py";
    fs::write(root.join(odd), "def a(): pass\n").unwrap();
    assert!(
        root.join(odd).exists(),
        "the fixture must be one file whose name contains a backslash"
    );

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let report = store
        .enqueue_pending_paths_under_root(&root, &[odd.to_string()])
        .unwrap();
    assert_eq!(
        report.enqueued,
        vec![odd.to_string()],
        "the queued spelling must be the filename, not a two-component path"
    );

    let outcome = store.reconcile_pending_paths(&root).unwrap();
    assert!(
        outcome.dropped.is_empty(),
        "a real indexable source must not be dropped: {:?}",
        outcome.dropped
    );
    assert_eq!(
        store.get_pending_paths().unwrap(),
        vec![odd.to_string()],
        "the row must survive reconcile under its own name"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// S-3: a stat that could not run must not delete the queued path.
///
/// `classify_pending_entry` matched every `symlink_metadata` error as "the
/// file is absent", so ELOOP from a symlink loop in a parent — or EACCES from
/// a parent that lost `+x`, or EIO/ESTALE from a network mount — deleted the
/// row exactly as a genuine ENOENT did. The queued file was then never
/// indexed, the deletion was reported as garbage collection, and `status`
/// reported fresh. Only `ErrorKind::NotFound` is evidence of absence; every
/// other errno is transient, so the row is kept, retried, and eventually
/// quarantined — which is visible.
#[cfg(unix)]
#[test]
fn s3_a_stat_that_could_not_run_keeps_the_pending_row_instead_of_deleting_it() {
    let dir = tmp_dir("s3-eloop");
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    // A self-referential symlink: `symlink_metadata` on anything *below* it
    // fails with ELOOP while resolving the parent, which is not ENOENT.
    std::os::unix::fs::symlink("loop", root.join("loop")).unwrap();
    let probe = std::fs::symlink_metadata(root.join("loop/x.py"))
        .expect_err("the fixture must make the stat fail");
    assert_ne!(
        probe.kind(),
        std::io::ErrorKind::NotFound,
        "the fixture must fail for a reason other than absence, got {probe:?}"
    );

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    store
        .enqueue_pending_paths(&["loop/x.py".to_string(), "gone.py".to_string()])
        .unwrap();

    let outcome = store.reconcile_pending_paths(&root).unwrap();
    let remaining = store.get_pending_paths().unwrap();
    assert!(
        remaining.contains(&"loop/x.py".to_string()),
        "a path whose stat failed is unknown, not absent, and must be retried: \
         {remaining:?} (dropped {:?})",
        outcome.dropped
    );
    // Positive control: the fix must not turn every drop into a retain. A path
    // that is genuinely absent with nothing indexed under it is still garbage.
    assert!(
        !remaining.contains(&"gone.py".to_string()),
        "a genuine ENOENT with nothing indexed under it is still dropped: \
         {remaining:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn an_unexamined_cache_marker_keeps_previously_queued_work() {
    let dir = tmp_dir("cache-marker-pending");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("pkg/CACHEDIR.TAG")).unwrap();
    fs::write(root.join("pkg/main.py"), "def f(): pass\n").unwrap();
    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let paths = ["pkg/main.py".to_string()];
    store.enqueue_pending_paths(&paths).unwrap();
    let admission = store
        .enqueue_pending_paths_under_root(&root, &paths)
        .unwrap();
    assert!(admission.enqueued.is_empty());
    assert_eq!(admission.refused.len(), 1);
    assert!(admission.refused[0].1.contains("CACHEDIR.TAG"));
    let outcome = store.reconcile_pending_paths(&root).unwrap();
    assert!(outcome.dropped.is_empty(), "{outcome:?}");
    assert_eq!(store.get_pending_paths().unwrap(), paths);

    // A successfully examined cache marker is definitive and still allows
    // obsolete source work to be dropped.
    fs::remove_dir(root.join("pkg/CACHEDIR.TAG")).unwrap();
    fs::write(
        root.join("pkg/CACHEDIR.TAG"),
        devmap_extract::CACHEDIR_TAG_SIGNATURE,
    )
    .unwrap();
    let outcome = store.reconcile_pending_paths(&root).unwrap();
    assert_eq!(outcome.dropped.len(), 1);
    assert!(store.get_pending_paths().unwrap().is_empty());
    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

/// A control token is not a path, and admission must not apply path semantics
/// to it.
///
/// `canonical_pending_entry` already short-circuits control tokens — a leading
/// NUL cannot begin a real filesystem path, which is what makes the namespace
/// safe — but the K7 build-cache check then ran on the result anyway. The OS
/// cannot be handed `root.join("\0devmap:git-head-changed")` at all, because a
/// path string cannot carry an interior NUL, so `is_cache_directory` returned
/// `Err`, the verdict was `Unreadable`, and the token was refused at the door:
///
/// ```text
/// watcher emitted a path outside the tree: "\0devmap:git-head-changed"
///   (cannot examine \u{0}devmap:git-head-changed/CACHEDIR.TAG:
///    file name contained an unexpected NUL byte)
/// enqueued 0 changed path(s)
/// ```
///
/// That token is the daemon's only signal that the checkout moved. Dropping it
/// leaves a commit, branch switch or rebase invisible: the index goes on
/// describing a tree that no longer exists and `status` still answers fresh.
/// It reached CI as `rust.yml`'s `Native worktree capacity and process
/// recovery` step failing on macOS and Ubuntu alike, the harness waiting out
/// its full 90 s deadline for a stored HEAD that could never move.
///
/// Every existing test of the sentinel called `enqueue_pending_paths`, which
/// has no cache check, while the daemon calls
/// `enqueue_pending_paths_under_root`, which does — so the whole suite was
/// green against a producer the daemon does not use.
/// `reconcile_pending_paths_with` already guards with `is_control_token`
/// before applying path semantics; this asserts the same guard at the other
/// producer, and the two positive controls below keep the K7 refusal and the
/// unreadable-marker refusal intact for entries that really are paths.
#[test]
fn a_control_token_is_admitted_without_being_walked_as_a_path() {
    const GIT_HEAD_SENTINEL: &str = "\u{0}devmap:git-head-changed";

    let dir = tmp_dir("control-token-admission");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.py"), "def f(): pass\n").unwrap();
    // A real tagged build cache, and a marker that cannot be examined.
    fs::create_dir_all(root.join("build/CACHEDIR.TAG")).unwrap();
    fs::write(root.join("build/artifact.py"), "def g(): pass\n").unwrap();
    fs::create_dir_all(root.join("cache")).unwrap();
    fs::write(
        root.join("cache/CACHEDIR.TAG"),
        devmap_extract::CACHEDIR_TAG_SIGNATURE,
    )
    .unwrap();
    fs::write(root.join("cache/artifact.py"), "def h(): pass\n").unwrap();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let admission = store
        .enqueue_pending_paths_under_root(
            &root,
            &[
                GIT_HEAD_SENTINEL.to_string(),
                "src/main.py".to_string(),
                "cache/artifact.py".to_string(),
                "build/artifact.py".to_string(),
            ],
        )
        .unwrap();

    assert!(
        admission.enqueued.contains(&GIT_HEAD_SENTINEL.to_string()),
        "the git-HEAD sentinel must reach the queue; refused: {:?}",
        admission.refused
    );
    assert!(
        admission.enqueued.contains(&"src/main.py".to_string()),
        "ordinary source must still be admitted: {:?}",
        admission.enqueued
    );
    // Positive control: the guard must exempt control tokens, not disable K7.
    let refused_for = |raw: &str| -> String {
        admission
            .refused
            .iter()
            .find(|(path, _)| path == raw)
            .unwrap_or_else(|| panic!("{raw} must be refused: {:?}", admission.refused))
            .1
            .clone()
    };
    assert!(
        refused_for("cache/artifact.py").contains("CACHEDIR.TAG"),
        "a tagged build cache is still refused"
    );
    // Positive control: an unreadable marker is still not a cleared path.
    assert!(
        refused_for("build/artifact.py").contains("CACHEDIR.TAG"),
        "a marker that could not be examined is still refused"
    );

    // The token survives the reconcile sweep too, so the daemon can still see
    // it after a connect-time repair.
    let outcome = store.reconcile_pending_paths(&root).unwrap();
    assert!(
        outcome
            .dropped
            .iter()
            .all(|(path, _)| path != GIT_HEAD_SENTINEL),
        "reconcile must retain the sentinel: {outcome:?}"
    );
    assert!(
        store
            .get_pending_paths()
            .unwrap()
            .contains(&GIT_HEAD_SENTINEL.to_string()),
        "the sentinel must still be queued after reconcile"
    );

    drop(store);
    fs::remove_dir_all(dir).unwrap();
}

/// S-3, one level up: a `canonicalize` that could not run is not "outside the
/// repository root".
///
/// `canonical_pending_entry` reached for `root.canonicalize().ok()` to rescue
/// absolute rows written by the old watcher — a symlinked checkout makes the
/// lexical prefix test fail on paths that are genuinely inside the tree. When
/// that canonicalize *failed*, the `.ok()` turned "I cannot tell" into `None`,
/// and `reconcile_pending_paths` deletes a `None` as a row that escapes the
/// root. The containment test never ran, and the row was destroyed on its
/// verdict.
#[cfg(unix)]
#[test]
fn s3_an_uncanonicalizable_root_does_not_delete_rows_as_escaping_it() {
    let dir = tmp_dir("s3-canon");
    let canonical_dir = dir.canonicalize().unwrap();
    fs::create_dir_all(dir.join("real")).unwrap();
    fs::write(dir.join("real/a.py"), "def a(): pass\n").unwrap();
    std::os::unix::fs::symlink("real", dir.join("link")).unwrap();
    let root = dir.join("link");
    // An absolute row of the shape the old watcher produced: inside the tree,
    // but only reachable through the root's canonical spelling.
    let absolute = canonical_dir.join("real/a.py").display().to_string();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    store
        .enqueue_pending_paths(std::slice::from_ref(&absolute))
        .unwrap();

    // Control: while the root canonicalizes, the row is recognised as inside
    // and rewritten to its canonical relative spelling.
    let rescued = store.reconcile_pending_paths(&root).unwrap();
    assert!(
        rescued.dropped.is_empty(),
        "a row inside the tree must not be dropped: {:?}",
        rescued.dropped
    );
    assert_eq!(store.get_pending_paths().unwrap(), vec!["a.py".to_string()]);

    // Now break the canonicalize itself, leaving the row's containment
    // genuinely undecidable.
    store
        .enqueue_pending_paths(std::slice::from_ref(&absolute))
        .unwrap();
    fs::remove_file(dir.join("link")).unwrap();
    std::os::unix::fs::symlink("link", dir.join("link")).unwrap();
    assert!(
        root.canonicalize().is_err(),
        "the fixture must make the root uncanonicalizable"
    );

    let error = store.reconcile_pending_paths(&root).unwrap_err();
    assert!(
        error.to_string().contains("cannot resolve worktree root"),
        "{error}"
    );
    let remaining = store.get_pending_paths().unwrap();
    assert!(
        remaining.contains(&absolute),
        "an unreadable root must preserve pending work: {remaining:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// S-3, the enqueue door: the same undecidable case must not be *reported* as
/// a containment failure either.
///
/// `enqueue_pending_paths_under_root` surfaces refusals to its caller, and
/// "outside the repository root" is a positive claim. Made on a canonicalize
/// that failed, it sends the reader looking for a misconfigured watcher
/// instead of at the unreadable root that actually caused it.
#[cfg(unix)]
#[test]
fn s3_enqueue_refusal_names_the_failed_check_rather_than_claiming_containment() {
    let dir = tmp_dir("s3-enqueue");
    let canonical_dir = dir.canonicalize().unwrap();
    fs::create_dir_all(dir.join("real")).unwrap();
    std::os::unix::fs::symlink("link", dir.join("link")).unwrap();
    let root = dir.join("link");
    let absolute = canonical_dir.join("real/a.py").display().to_string();

    let store = Store::open_in_memory().unwrap();
    let report = store
        .enqueue_pending_paths_under_root(&root, std::slice::from_ref(&absolute))
        .unwrap();
    assert_eq!(report.refused.len(), 1, "{report:?}");
    let (_, reason) = &report.refused[0];
    assert!(
        !reason.contains("outside the repository root"),
        "a check that could not run must not claim the path is outside: {reason}"
    );
    assert!(
        reason.contains("could not be checked"),
        "the refusal must say the check failed: {reason}"
    );

    // Positive control: a path that really is outside is still refused as
    // outside, so the honest message is not the only message.
    let outside = canonical_dir
        .parent()
        .unwrap()
        .join("elsewhere/x.py")
        .display()
        .to_string();
    let real_root = dir.join("real");
    let refused_outside = store
        .enqueue_pending_paths_under_root(&real_root, &[outside])
        .unwrap();
    assert_eq!(refused_outside.refused.len(), 1);
    assert!(
        refused_outside.refused[0]
            .1
            .contains("outside the repository root"),
        "{:?}",
        refused_outside.refused
    );
    let _ = fs::remove_dir_all(&dir);
}

/// S-7: reading the schema version must wait for a lock like every other
/// connection in this crate.
///
/// This one is a **pin, not a repair**. `stored_schema_version` opens its own
/// connection and never passed through `configure_connection`, so the audit
/// read it as the one connection with no `busy_timeout` — but rusqlite 0.31
/// calls `sqlite3_busy_timeout(db, 5000)` on every connection it opens
/// (`inner_connection.rs:121`), so the wait was inherited rather than absent
/// and this test passed against the unmodified tree. What was missing is the
/// *statement*: the crate's five-second contention policy lived in
/// `configure_connection` and was silently supplied by a dependency default at
/// the other opener. Swap that default out (`busy_timeout(0)`) and this test
/// fails with `database is locked`, which is what it exists to catch.
#[test]
fn s7_reading_the_schema_version_waits_for_a_lock_like_every_other_reader() {
    let dir = tmp_dir("s7-busy-version");
    let db_path = dir.join("legacy.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA user_version = 7; CREATE TABLE t (x);")
            .unwrap();
    }

    // Positive control: uncontended, it answers immediately and correctly.
    assert_eq!(Store::stored_schema_version(&db_path).unwrap(), Some(7));

    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let locked = db_path.clone();
    let holder = std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(&locked).unwrap();
        conn.execute_batch("BEGIN EXCLUSIVE; INSERT INTO t VALUES (1);")
            .unwrap();
        ready_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        conn.execute_batch("COMMIT").unwrap();
    });
    ready_rx.recv().unwrap();

    let started = Instant::now();
    let outcome = Store::stored_schema_version(&db_path);
    let waited = started.elapsed();
    holder.join().unwrap();

    let version = outcome.unwrap_or_else(|error| {
        panic!("a momentary lock made the version read fail instead of wait: {error}")
    });
    assert_eq!(version, Some(7));
    assert!(
        waited >= std::time::Duration::from_millis(200),
        "the read returned in {waited:?}, so it cannot have waited for the lock"
    );

    let _ = fs::remove_dir_all(&dir);
}

/// S-8: the schema gate must assert the whole schema it claims to assert.
///
/// `REQUIRED` omitted `generation_files.grammar_version`/`analyzer_version`
/// (v8) and `generation_unresolved.classification`/`receiver` (v10/v11), so a
/// store stamped at the current version without them opened clean and failed at
/// the first write instead of at the gate.
#[test]
fn s8_the_schema_gate_refuses_a_store_missing_a_column_its_writers_require() {
    for (table, column) in [
        ("generation_files", "grammar_version"),
        ("generation_files", "analyzer_version"),
        ("generation_unresolved", "classification"),
        ("generation_unresolved", "receiver"),
    ] {
        let dir = tmp_dir(&format!("s8-{table}-{column}"));
        let db_path = dir.join("index.sqlite");
        // Positive control: the store this binary just created opens cleanly.
        Store::open(&db_path).expect("a freshly created store must open");
        Store::open(&db_path).expect("reopening an intact store must succeed");

        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            // SQLite refuses `DROP COLUMN` while any index references the
            // column, so the fixture has to clear them first. Derived from
            // `sqlite_master` rather than named: this used to drop one index by
            // name, and the next migration to add an index over one of these
            // four columns broke the fixture with
            // `error in index … after drop column`, which reads like a store
            // fault rather than a test that went stale.
            let indexes: Vec<String> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT name FROM sqlite_master
                         WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL",
                    )
                    .unwrap();
                let names = stmt
                    .query_map([table], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                names
                    .into_iter()
                    .filter(|index| {
                        let mut info = conn
                            .prepare(&format!("PRAGMA index_info({index})"))
                            .unwrap();
                        let columns: Vec<String> = info
                            .query_map([], |row| row.get::<_, Option<String>>(2))
                            .unwrap()
                            .filter_map(|entry| entry.ok().flatten())
                            .collect();
                        columns.iter().any(|name| name == column)
                    })
                    .collect()
            };
            for index in indexes {
                conn.execute_batch(&format!("DROP INDEX IF EXISTS {index}"))
                    .unwrap();
            }
            // `generation_files` became a *view* in v17, and a view has no
            // column to drop. What the gate checks is unchanged — does the
            // relation carry the columns its readers name — so the fixture
            // removes the column the way a view loses one: by being redefined
            // without it. Derived from `sqlite_master` rather than written out,
            // so a column added to the view later is covered without anyone
            // remembering this fixture exists.
            if relation_is_view(&conn, table) {
                let definition: String = conn
                    .query_row(
                        "SELECT sql FROM sqlite_master WHERE name = ?1 AND type = 'view'",
                        [table],
                        |row| row.get(0),
                    )
                    .unwrap();
                let projection: Vec<String> = definition
                    .lines()
                    .filter(|line| line.contains(" AS ") && line.trim_end().ends_with(','))
                    .map(|line| {
                        // The first projected column shares its line with the
                        // `SELECT` keyword.
                        line.trim()
                            .trim_start_matches("SELECT ")
                            .trim()
                            .trim_end_matches(',')
                            .to_string()
                    })
                    .filter(|line| !line.ends_with(&format!(" AS {column}")))
                    .collect();
                assert!(
                    !projection.is_empty(),
                    "could not re-project the view {table} without {column}: {definition}"
                );
                let from = definition
                    .split_once("  FROM ")
                    .unwrap_or_else(|| panic!("view {table} has no FROM clause: {definition}"))
                    .1;
                conn.execute_batch(&format!(
                    "DROP VIEW {table};\nCREATE VIEW {table} AS SELECT {}\n  FROM {from}",
                    projection.join(", ")
                ))
                .unwrap();
            } else {
                conn.execute(&format!("ALTER TABLE {table} DROP COLUMN {column}"), [])
                    .unwrap();
            }
        }

        let error = Store::open(&db_path)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| {
                panic!(
                    "a store missing {table}.{column} opened clean; the gate is narrower \
                     than the schema it asserts, so the failure lands on the first write"
                )
            });
        assert!(
            error.contains(column) && error.contains(table),
            "the refusal must name the missing column: {error}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

/// A store written before the payload split migrates, and keeps its rows.
///
/// This step had no coverage. Every other test creates a store fresh, and a
/// fresh store is built from the current schema and never walks the chain — so
/// the migration's `file_payloads` could omit the `file_id` that the fresh
/// shape declares, that its own `INSERT` names, and that every runtime probe
/// keys on, and 1,842 tests still passed. What failed was the first real v16
/// store the binary was pointed at, which is every existing installation.
#[test]
fn a_v16_store_migrates_to_the_payload_split_and_keeps_its_rows() {
    let dir = tmp_dir("migration-v17");
    let db_path = dir.join("legacy-v16.sqlite");

    // A real v16 store: build one at the current schema, then reverse the v17
    // split — materialise the view back into a table and drop the split ones.
    // Synthesising the old DDL by hand would only test a schema this project
    // never wrote.
    {
        let store = Store::open(&db_path).unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT INTO paths (id, path) VALUES (1, 'a.py'), (2, 'twin.py')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO generations (id, created_at, head_sha, repo_root, analysis_json)
             VALUES (1, 0, 'deadbeef', '/tmp/probe', '{}')",
            [],
        )
        .unwrap();
        // Two files with byte-identical content. Keyed on content alone they
        // would collapse into one payload and both report the same path.
        for (file_id, path) in [(1, "a.py"), (2, "twin.py")] {
            conn.execute(
                "INSERT INTO file_payloads
                   (file_id, content_hash, language, grammar_version, analyzer_version,
                    parse_outcome_json, engine_json, extraction_json)
                 VALUES (?1, 99, 'python', 'g1', 'a1', '\"Clean\"', '\"TreeSitter\"', ?2)",
                rusqlite::params![file_id, format!("{{\"file_path\":\"{path}\"}}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO generation_file_rows (generation_id, file_id, payload_id)
                 VALUES (1, ?1, ?1)",
                rusqlite::params![file_id],
            )
            .unwrap();
        }
        // Now collapse it back to the pre-split shape.
        conn.execute_batch(
            "CREATE TABLE gf_flat AS SELECT * FROM generation_files;
             DROP VIEW generation_files;
             DROP TABLE generation_file_rows;
             DROP TABLE file_payloads;
             ALTER TABLE gf_flat RENAME TO generation_files;
             PRAGMA user_version = 16;",
        )
        .unwrap();
    }

    let store = Store::open(&db_path).expect("a v16 store must migrate, not fail to open");
    drop(store);

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        version, CURRENT_SCHEMA_VERSION,
        "the chain must reach the current schema"
    );

    // The identity carries the file, so byte-identical twins stay two payloads.
    let payloads: i64 = conn
        .query_row("SELECT COUNT(*) FROM file_payloads", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        payloads, 2,
        "content-addressing alone would collapse these to 1"
    );

    // And both rows still name their own file through the view.
    let mut stmt = conn
        .prepare(
            "SELECT p.path, f.extraction_json
               FROM generation_files f JOIN paths p ON p.id = f.file_id
              WHERE f.generation_id = 1 ORDER BY p.path",
        )
        .unwrap();
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 2, "both membership rows must survive");
    for (path, payload) in &rows {
        assert!(
            payload.contains(path.as_str()),
            "{path} reports a payload belonging to another file: {payload}"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

/// Every step in the migration chain is re-entrant, at every version.
///
/// The v16→v17 step shipped unable to run: its `file_payloads` omitted the
/// `file_id` the fresh-create schema declares and its own `INSERT` names, and
/// 1,842 tests passed over it because **a fresh store is built from the current
/// schema and never walks the chain**. What failed was the first real store the
/// binary opened, which is every existing installation.
///
/// That blind spot is not specific to one step, so this is not a test of one
/// step. A store carrying the modern shape but stamped at an older
/// `user_version` walks every migration from there to the top, and each one
/// meets a database where its work is already done — which is exactly what each
/// step's idempotency probe exists to detect. `ADD COLUMN` is not repeatable,
/// `CREATE INDEX` on a renamed relation is not repeatable, and a step that
/// re-runs its work against a live view raises rather than no-ops. Any step
/// whose probe is wrong fails here, at the version it is wrong for, instead of
/// on a user's machine.
///
/// Ranged over `CURRENT_SCHEMA_VERSION` rather than a literal, so a v18 added
/// tomorrow is covered the day it lands. `MIN_MIGRATABLE` is 3 because the
/// chain's own entry point is 3: below it the store is a foreign Python
/// `index.sqlite`, which `adversarial_store` covers separately.
#[test]
fn every_migration_step_is_re_entrant_from_every_version() {
    const MIN_MIGRATABLE: i32 = 3;
    let dir = tmp_dir("migration-reentrancy");
    let mut checked = 0;

    for stamped in MIN_MIGRATABLE..=CURRENT_SCHEMA_VERSION {
        let db_path = dir.join(format!("stamped-{stamped}.sqlite"));
        {
            // A store at the current shape, then backdated. Every step from
            // `stamped` up meets work already done.
            let store = Store::open(&db_path).unwrap();
            drop(store);
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute(&format!("PRAGMA user_version = {stamped}"), [])
                .unwrap();
        }

        let store = Store::open(&db_path).unwrap_or_else(|error| {
            panic!(
                "a store stamped at v{stamped} must migrate to \
                 v{CURRENT_SCHEMA_VERSION}, not fail to open: {error}"
            )
        });
        drop(store);

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version, CURRENT_SCHEMA_VERSION,
            "a store stamped at v{stamped} stopped at v{version}"
        );
        // `Store::open` runs `validate_schema` at the end of the chain, so
        // reaching here is the shape check. This re-asserts the one relation
        // the chain's last step creates, because a migration that silently
        // skipped would leave the version right and the shape wrong.
        let is_view: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE name = 'generation_files' AND type = 'view'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            is_view, 1,
            "from v{stamped}, generation_files must end as a view"
        );
        checked += 1;
    }

    assert_eq!(
        checked,
        CURRENT_SCHEMA_VERSION - MIN_MIGRATABLE + 1,
        "the sweep must cover every version, not a subset"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Every SQL statement the gate scripts hand-write must parse against the
/// schema this binary creates.
///
/// `verify.sh` and `tools/*.sh` query the store with `sqlite3` and literal SQL.
/// Nothing compiles that SQL and, until this test, nothing ran it outside a
/// full gate pass -- so a schema change could leave a gate script naming a
/// column that no longer existed and the tree would still be green through
/// `cargo test`, `cargo clippy` and every crate suite.
///
/// The scripts are discovered rather than listed, so one added later is
/// covered without anyone remembering this test exists.
///
/// That is not hypothetical. Schema v22 interned the unresolved ledger's
/// `reason` into `unresolved_texts`, and
/// `tools/memory_model_probe.sh` kept counting
/// `unresolved_rows WHERE reason LIKE 'AmbiguousGlobal%'`. The whole workspace
/// suite -- 303 binaries -- passed; the breakage surfaced only as a step-6
/// failure several release builds into a gate run. It surfaced *at all* only
/// because that script checks `sqlite3`'s exit status. A probe that had
/// swallowed the error would have counted zero rows and reported a gate that
/// never examined anything as a gate that passed.
///
/// Preparing is the whole check and it is enough: SQLite resolves every
/// relation and column name at prepare time, which is exactly this class of
/// breakage, and it does so without needing a populated store.
#[test]
fn every_sql_statement_the_gate_scripts_embed_parses_against_the_current_schema() {
    // `CARGO_MANIFEST_DIR` is `<workspace>/devmap-store`.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives one level below the workspace root")
        .to_path_buf();

    // Globbed, then pinned. Both directions of drift are real and they want
    // opposite things: a named list stops covering a gate script added
    // tomorrow, and a bare glob stops covering one that is renamed or moved
    // out of `tools/` without anyone noticing the set shrank. So the set is
    // discovered -- `verify.sh` plus every `.sh` under `tools/` -- and then
    // checked against the files known to carry SQL today.
    let mut scripts: Vec<std::path::PathBuf> = vec![root.join("verify.sh")];
    let tools = root.join("tools");
    let listing = fs::read_dir(&tools)
        .unwrap_or_else(|error| panic!("{} is unreadable: {error}", tools.display()));
    for entry in listing {
        let path = entry.expect("a readable directory entry").path();
        if path.extension().is_some_and(|extension| extension == "sh") {
            scripts.push(path);
        }
    }
    scripts.sort();

    let mut sources: Vec<(String, String)> = Vec::new();
    for path in &scripts {
        let relative = path
            .strip_prefix(&root)
            .expect("every script was joined onto root")
            .to_string_lossy()
            .replace('\\', "/");
        let text = fs::read_to_string(path).unwrap_or_else(|error| {
            panic!("gate script {} is unreadable: {error}", path.display())
        });
        sources.push((relative, text));
    }

    // The files that carry SQL today. Not the whole expected set -- new
    // scripts are meant to be picked up without editing this -- but a rename
    // that moved one of these out of the glob's reach would otherwise take its
    // coverage with it and leave every assertion below still passing.
    for required in ["verify.sh", "tools/memory_model_probe.sh", "tools/soak.sh"] {
        assert!(
            sources.iter().any(|(name, _)| name == required),
            "{required} is no longer among the discovered gate scripts {:?}; if it \
             moved, the glob above has to follow it",
            sources.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
    }

    // Shell double-quoted SQL, so the statement runs to the next `"`. None of
    // these embed an escaped double quote -- SQL string literals in them are
    // single-quoted -- and the count assertion below is what notices if one
    // ever does.
    let mut found: Vec<(String, String)> = Vec::new();
    for (name, text) in &sources {
        let mut rest = text.as_str();
        while let Some(open) = rest.find("\"SELECT").or_else(|| rest.find("\"WITH")) {
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else {
                panic!("{name}: an embedded SQL string is never closed");
            };
            found.push((name.clone(), after[..close].to_string()));
            rest = &after[close + 1..];
        }
    }

    // Non-vacuity, proved against the tree rather than against a constant: the
    // extractor must account for every statement opener in the sources. An
    // extractor that quietly matched nothing would make this test pass while
    // checking no SQL at all, which is the failure it exists to prevent.
    let openers: usize = sources
        .iter()
        .map(|(_, text)| text.matches("\"SELECT").count() + text.matches("\"WITH").count())
        .sum();
    assert!(
        openers > 0,
        "no embedded SQL found in the gate scripts at all; the extractor or the scripts moved"
    );
    assert_eq!(
        found.len(),
        openers,
        "the extractor found {} of {openers} embedded statements, so it is dropping some",
        found.len()
    );
    // The specific coupling that broke, pinned so a future change that stops
    // reading the ledger from a gate script has to say so here.
    assert!(
        found.iter().any(|(_, sql)| sql.contains("unresolved_rows")),
        "no gate script reads the unresolved ledger any more; this test was added \
         because tools/memory_model_probe.sh does, so confirm that on purpose"
    );

    let dir = tmp_dir("gate-script-sql");
    let db_path = dir.join("index.sqlite");
    Store::open(&db_path).expect("a freshly created store must open");
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    for (name, sql) in &found {
        conn.prepare(sql).unwrap_or_else(|error| {
            panic!("{name} embeds SQL the current schema cannot parse: {error}\n{sql}")
        });
    }

    // `fanout.sql` is a whole file rather than a quoted string, and it builds
    // scratch relations its later statements read, so it is checked by running
    // it: preparing its statements one at a time would fail on the scratch
    // relations the earlier ones create. The store is a throwaway.
    let fanout_path = root.join("tools/fanout.sql");
    let fanout = fs::read_to_string(&fanout_path)
        .unwrap_or_else(|error| panic!("{} is unreadable: {error}", fanout_path.display()));
    conn.execute_batch(&fanout).unwrap_or_else(|error| {
        panic!("tools/fanout.sql does not run against the current schema: {error}")
    });

    let _ = fs::remove_dir_all(&dir);
}

/// An id these two tables hand out must never be handed out again.
///
/// `generation_file_digests` folds a per-file digest of the edge rows and the
/// ledger rows, and since v22 the ledger tuple that digest is taken over
/// carries `source_file_id`, `reason_id` and `classification_id` — ids into
/// `paths` and `unresolved_texts` — rather than the path and the two texts.
/// That is only sound while an id means one thing forever. SQLite's default
/// rowid is `max(rowid) + 1`, so deleting the highest row hands its id to the
/// next insert; `AUTOINCREMENT` is what stops that, and both tables declare it.
///
/// If either lost it, the failure would be silent and would not look like a
/// schema fault. `prune_generations_except_latest` retires unreferenced pool
/// texts and unreferenced paths, so ids *are* freed in normal operation. A
/// reused id lets a digest taken generations apart compare equal to a digest
/// taken over different text, which puts the file on the
/// `unchanged_unresolved_files` fast path and leaves its stale ledger rows
/// live. The store would answer confidently and wrongly.
///
/// Nothing else in the workspace asserts this. `REQUIRED_SCHEMA` checks column
/// names, not column constraints; `a_migrated_store_carries_the_same_schema_as_a_fresh_one`
/// compares a migrated store's DDL against a fresh store's, which still agree
/// if both lost the keyword. So this asserts the *behaviour* rather than the
/// spelling: a keyword can be renamed or a table rebuilt by some future rung,
/// and the property that has to survive is that the second insert does not get
/// the first one's id.
#[test]
fn an_id_these_tables_hand_out_is_never_handed_out_again() {
    let dir = tmp_dir("id-reuse");
    let db_path = dir.join("index.sqlite");
    drop(Store::open(&db_path).expect("a fresh store must open"));
    let conn = rusqlite::Connection::open(&db_path).unwrap();

    for (table, column) in [("paths", "path"), ("unresolved_texts", "text")] {
        // A fresh store has both tables empty, so the row inserted here is the
        // highest -- which is the only case where a default rowid would be
        // reused, and therefore the case that tells the two schemes apart.
        let empty: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            empty, 0,
            "{table} must start empty or this test cannot distinguish a reused id"
        );

        conn.execute(
            &format!("INSERT INTO {table} ({column}) VALUES ('id-reuse-probe-first')"),
            [],
        )
        .unwrap();
        let first = conn.last_insert_rowid();
        conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [first])
            .unwrap();
        conn.execute(
            &format!("INSERT INTO {table} ({column}) VALUES ('id-reuse-probe-second')"),
            [],
        )
        .unwrap();
        let second = conn.last_insert_rowid();

        assert!(
            second > first,
            "{table} reused id {first} for a second, different row: without \
             AUTOINCREMENT an id stops being an identity, and a per-file digest \
             taken over these ids can then compare equal across two generations \
             that hold different text"
        );

        conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [second])
            .unwrap();
    }

    let _ = fs::remove_dir_all(&dir);
}

/// Whether the store object `name` is a view rather than a base table.
fn relation_is_view(conn: &rusqlite::Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT type FROM sqlite_master WHERE name = ?1",
        [name],
        |row| row.get::<_, String>(0),
    )
    .map(|kind| kind == "view")
    .unwrap_or(false)
}
