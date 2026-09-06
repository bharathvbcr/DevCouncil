//! Store-level regression gates for the kernel defects K1–K13.
//!
//! Each test failed against the tree as it stood before its fix. Where the old
//! behaviour is subtle the assertion message states it, so a later reader can
//! tell a regression from a deliberate re-specification.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devmap_store::{Store, CURRENT_SCHEMA_VERSION};

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

/// K3: a store this binary cannot open must say what to do about it.
///
/// The refusal was `unsupported future schema version 99` — no store path, no
/// statement of what this binary supports, and no remedy. An operator holding
/// that string cannot tell which of several stores was refused, nor that the
/// fix is to rebuild the kernel rather than to delete the database.
#[test]
fn k3_future_schema_refusal_names_the_store_the_versions_and_the_remedy() {
    let dir = tmp_dir("k3-future");
    let db_path = dir.join("future.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 99", []).unwrap();
    }

    let error = Store::open(&db_path)
        .err()
        .expect("a newer schema must be refused")
        .to_string();

    assert!(
        error.contains(&db_path.display().to_string()),
        "the refusal must name the store it refused: {error}"
    );
    assert!(
        error.contains("99"),
        "the refusal must state the store's schema version: {error}"
    );
    assert!(
        error.contains(&CURRENT_SCHEMA_VERSION.to_string()),
        "the refusal must state this binary's schema version: {error}"
    );
    assert!(
        error.contains("cargo build --release -p devmap-cli") && error.contains("DEVMAP_BINARY"),
        "the refusal must name the remedy (rebuild the binary): {error}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// K3: the same for a store older than any migration this binary carries.
///
/// `user_version = 2` is the Python engine's store — exactly what the old
/// default `--db` pointed at (K6) — and the refusal was
/// `unsupported schema version 2`.
#[test]
fn k3_outdated_schema_refusal_names_the_store_the_versions_and_the_remedy() {
    let dir = tmp_dir("k3-outdated");
    let db_path = dir.join("legacy.sqlite");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 2", []).unwrap();
    }

    let error = Store::open(&db_path)
        .err()
        .expect("a pre-v3 schema must be refused")
        .to_string();

    assert!(
        error.contains(&db_path.display().to_string()),
        "the refusal must name the store it refused: {error}"
    );
    assert!(
        error.contains(&CURRENT_SCHEMA_VERSION.to_string()),
        "the refusal must state this binary's schema version: {error}"
    );
    assert!(
        error.contains("devmap build"),
        "the refusal must name the remedy for an older store: {error}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// K3: reading a store's stamped schema must not migrate it.
///
/// `Store::open` runs the migration chain under an exclusive transaction from
/// every open, including read-only commands. `stored_schema_version` answers
/// the version question without writing, which is what lets `devmap status`
/// report `schema_outdated` instead of silently upgrading a store behind a
/// read.
#[test]
fn k3_reading_the_schema_version_does_not_migrate_the_store() {
    let dir = tmp_dir("k3-probe");
    let db_path = dir.join("v11.sqlite");
    {
        let _store = Store::open(&db_path).unwrap();
    }
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("PRAGMA user_version = 11", []).unwrap();
    }

    assert_eq!(Store::stored_schema_version(&db_path).unwrap(), Some(11));

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let observed: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        observed, 11,
        "probing the schema version must leave the store untouched"
    );
    assert_eq!(
        Store::stored_schema_version(dir.join("absent.sqlite")).unwrap(),
        None,
        "a missing store has no schema version and must not be created"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// K5: prose and data formats are not parse failures.
///
/// `history.parse_failed` counted every `ParseOutcome::Failed`, and a file with
/// no linked grammar reports `Failed` — so on this repository 294 of 1,310
/// files were reported as parse failures and every one of them was Markdown,
/// JSON, YAML, config or HTML. Nothing was broken; there is simply no grammar
/// for prose, and there never will be. The count that mattered — the 16 files
/// tree-sitter parsed and flagged errors in — was invisible underneath it.
///
/// The distinguishing case is the notebook. `"notebook"` is a non-declarative
/// language, so a classifier keyed on the language alone would call a malformed
/// `.ipynb` "not applicable" and lose a genuine failure. The classifier asks
/// the *engine*, which records whether a grammar was expected at all.
#[test]
fn k5_prose_formats_are_not_parse_failures_but_broken_files_still_are() {
    use devmap_extract::extract_file;

    let prose = [
        ("notes.md", "# Title\n\nSome prose.\n"),
        ("data.json", "{\"a\": 1}\n"),
        ("conf.yaml", "a: 1\n"),
        ("page.html", "<html><body>hi</body></html>\n"),
    ];
    for (path, source) in prose {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.is_parse_failure(),
            "{path} has no grammar by design and must not count as a parse failure \
             (outcome {:?}, engine {:?})",
            extraction.parse_outcome,
            extraction.engine
        );
        assert!(
            extraction
                .symbols
                .iter()
                .any(|symbol| symbol.kind == devmap_extract::SymbolKind::File),
            "{path} must still be addressable as a File node"
        );
    }

    // A notebook that is not valid JSON *is* a failure, and shares its language
    // with the prose formats above.
    let broken_notebook = extract_file("broken.ipynb", "{not json");
    assert!(
        broken_notebook.is_parse_failure(),
        "a malformed notebook is a real failure: {:?} / {:?}",
        broken_notebook.parse_outcome,
        broken_notebook.engine
    );

    // A syntactically broken Python file is `Partial`, never `Failed` —
    // tree-sitter is error-tolerant, so it reports error ranges rather than
    // refusing the file. It was never part of `parse_failed` and must not
    // become part of it: `Partial` is the tier this fix exists to make visible.
    let broken_python = extract_file("broken.py", "def a(:\n  return\n");
    assert!(
        matches!(
            broken_python.parse_outcome,
            devmap_extract::ParseOutcome::Partial { .. }
        ),
        "a broken .py parses partially, it does not fail: {:?}",
        broken_python.parse_outcome
    );
    assert!(!broken_python.is_parse_failure());
}

/// K5: the history row a build writes must carry the corrected count.
///
/// Store-level rather than extractor-level: `parse_failed` is computed where
/// the generation is persisted, so the classifier being right is only half of
/// it — the writer has to ask.
#[test]
fn k5_build_history_parse_failed_excludes_grammarless_prose() {
    use devmap_analyze::analyze;
    use devmap_extract::extract_file;
    use devmap_resolve::Resolver;

    let extractions = vec![
        extract_file("a.py", "def a():\n    return 1\n"),
        extract_file("README.md", "# hi\n"),
        extract_file("data.json", "{\"a\": 1}\n"),
        extract_file("broken.ipynb", "{not json"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);

    let store = Store::open_in_memory().unwrap();
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    let row = store.build_history(1).unwrap().pop().unwrap();
    assert_eq!(
        row.parse_failed, 1,
        "only the malformed notebook is a parse failure; the Markdown and JSON \
         files have no grammar by design"
    );
}

/// K13: the writer lock is exclusive and names its holder on timeout.
///
/// There was no cross-process writer lock at all. Two `devmap build`
/// processes — or a build and the daemon's drain — raced on SQLite's five
/// second `busy_timeout` alone, and the loser surfaced `database is locked`
/// *after* paying for a full extraction and resolution: all of the cost, none
/// of the result, and a message naming neither the other writer nor a remedy.
#[test]
fn k13_writer_lock_is_exclusive_and_names_the_holder_on_timeout() {
    let dir = tmp_dir("k13-writer-lock");
    let db_path = dir.join("devmap.sqlite");
    let store = Store::open(&db_path).unwrap();

    let held = store
        .lock_writer(std::time::Duration::from_millis(100))
        .expect("first writer takes the lock");
    assert!(held.is_held(), "a file-backed store must take a real lock");
    assert_eq!(
        held.path(),
        Some(Store::writer_lock_path(&db_path).as_path())
    );

    let contended = Store::lock_writer_at(&db_path, std::time::Duration::from_millis(50))
        .expect_err("a second writer must not take a held lock")
        .to_string();
    assert!(
        contended.contains(&format!("pid {}", std::process::id())),
        "the refusal must name the holder: {contended}"
    );
    assert!(
        !contended.contains("database is locked"),
        "the refusal must be the lock's own message, not SQLite's: {contended}"
    );

    drop(held);
    let reacquired = Store::lock_writer_at(&db_path, std::time::Duration::from_millis(50));
    assert!(
        reacquired.is_ok(),
        "the lock must be available once released: {reacquired:?}"
    );

    // An in-memory store has no second writer to exclude, and says so rather
    // than pretending to hold something.
    let memory = Store::open_in_memory().unwrap();
    let unheld = memory
        .lock_writer(std::time::Duration::from_millis(10))
        .unwrap();
    assert!(!unheld.is_held());
    assert_eq!(unheld.path(), None);

    let _ = fs::remove_dir_all(&dir);
}

/// K13: a generation write takes its write lock at `BEGIN`.
///
/// The generation write used a DEFERRED transaction while every prune beside it
/// used `Immediate`. SQLite exposes no way to read a transaction's behaviour
/// back, so the policy is asserted directly rather than inferred from a race
/// that reproduces only sometimes — the same reason `should_vacuum` is a pure
/// function with its own test.
#[test]
fn k13_generation_writes_use_an_immediate_transaction() {
    assert!(
        matches!(
            Store::GENERATION_TX_BEHAVIOR,
            rusqlite::TransactionBehavior::Immediate
        ),
        "a generation write must take the write lock at BEGIN, like the prunes \
         beside it — DEFERRED discovers the conflict partway through instead"
    );
}

/// K1(a): both queue producers now write one canonical spelling.
///
/// The watcher enqueued absolute paths and the connect-time reconcile enqueued
/// repo-relative ones, and `enqueue_pending_paths` inserted whichever it was
/// handed with no normalisation and no containment check. One file could
/// therefore occupy two rows, and a path from outside the tree could occupy one
/// forever — the drain rejects it, and nothing deletes it.
#[test]
fn k1_enqueue_under_root_normalizes_dedups_and_refuses_escapes() {
    let dir = tmp_dir("k1-enqueue");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a(): pass\n").unwrap();
    let store = Store::open(dir.join("devmap.sqlite")).unwrap();

    let report = store
        .enqueue_pending_paths_under_root(
            &root,
            &[
                root.join("src/a.py").display().to_string(),
                "src/a.py".to_string(),
                "./src/a.py".to_string(),
                "/etc/passwd".to_string(),
                "../outside.py".to_string(),
            ],
        )
        .unwrap();

    assert_eq!(
        report.enqueued,
        vec!["src/a.py".to_string()],
        "one file is one row, spelled repo-relative"
    );
    assert_eq!(
        report.refused.len(),
        2,
        "paths outside the root are refused, not queued: {:?}",
        report.refused
    );
    assert_eq!(store.get_pending_paths().unwrap(), vec!["src/a.py"]);
    let _ = fs::remove_dir_all(&dir);
}

/// K1(b): structurally unprocessable rows are deleted, not retried forever.
///
/// Every row in this fixture is one of the shapes actually found stuck at
/// `attempts = 5` on this repository's store: a path under a *previous*
/// location of the checkout, a directory, a file over `MAX_SOURCE_BYTES` (a
/// 30 MB vendored `parser.c`), and a file that is not an indexable source.
#[test]
fn k1_reconcile_drops_rows_no_retry_could_ever_process() {
    let dir = tmp_dir("k1-reconcile");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a(): pass\n").unwrap();
    fs::write(
        root.join("huge.c"),
        vec![b'x'; devmap_extract::MAX_SOURCE_BYTES as usize + 1],
    )
    .unwrap();
    fs::write(root.join("logo.png"), [0x89, 0x50, 0x4e, 0x47]).unwrap();
    let store = Store::open(dir.join("devmap.sqlite")).unwrap();

    let stale_absolute = dir.join("old-checkout/src/a.py").display().to_string();
    store
        .enqueue_pending_paths(&[
            "src/a.py".to_string(),
            "src".to_string(),
            "huge.c".to_string(),
            "logo.png".to_string(),
            "gone.py".to_string(),
            stale_absolute.clone(),
        ])
        .unwrap();

    let outcome = store.reconcile_pending_paths(&root).unwrap();
    let dropped: Vec<&str> = outcome
        .dropped
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    assert!(dropped.contains(&"huge.c"), "oversized: {dropped:?}");
    assert!(dropped.contains(&"logo.png"), "non-source: {dropped:?}");
    assert!(
        dropped.contains(&"gone.py"),
        "absent and not indexed: {dropped:?}"
    );
    assert!(
        dropped.contains(&stale_absolute.as_str()),
        "outside the root: {dropped:?}"
    );

    let remaining = store.get_pending_paths().unwrap();
    assert_eq!(
        remaining,
        vec!["src".to_string(), "src/a.py".to_string()],
        "a real source and a real directory are still work"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// K1(b): an absent path the graph still claims is a *deletion*, not garbage.
///
/// Dropping it would leave the stored generation asserting a file that is gone,
/// which is the failure the reconcile must not cause while fixing the one it
/// exists for.
#[test]
fn k1_reconcile_keeps_deletions_the_drain_still_has_to_process() {
    use devmap_analyze::analyze;
    use devmap_extract::extract_file;
    use devmap_resolve::Resolver;

    let dir = tmp_dir("k1-deletion");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("pkg")).unwrap();
    let store = Store::open(dir.join("devmap.sqlite")).unwrap();

    let extractions = vec![
        extract_file("pkg/removed.py", "def removed(): pass\n"),
        extract_file("kept.py", "def kept(): pass\n"),
    ];
    fs::write(root.join("kept.py"), "def kept(): pass\n").unwrap();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let analysis = analyze(&extractions, &resolution);
    store
        .save_generation(&extractions, &resolution, &analysis)
        .unwrap();

    store
        .enqueue_pending_paths(&[
            "pkg/removed.py".to_string(),
            "pkg".to_string(),
            "never_indexed.py".to_string(),
        ])
        .unwrap();
    // The directory is removed too, so both the file and its parent are absent.
    fs::remove_dir_all(root.join("pkg")).unwrap();

    let outcome = store.reconcile_pending_paths(&root).unwrap();
    let remaining = store.get_pending_paths().unwrap();
    assert!(
        remaining.contains(&"pkg/removed.py".to_string()),
        "a deleted file the graph still claims must stay queued: {remaining:?} \
         (dropped {:?})",
        outcome.dropped
    );
    assert!(
        remaining.contains(&"pkg".to_string()),
        "a deleted directory with indexed descendants must stay queued: {remaining:?}"
    );
    assert!(
        !remaining.contains(&"never_indexed.py".to_string()),
        "an absent path with nothing indexed under it is garbage: {remaining:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// K1(e): a committed build retires the work it has just done.
///
/// Two rules, because a build proves different things depending on whether it
/// was narrowed. A whole-tree build read everything, so it answers every
/// request that existed when it started — including one naming a *directory*,
/// which is never an extraction and which the old per-path rule therefore kept
/// forever (918 such rows survived a full `dev map` on the live store). A
/// `--affected` build read only what it was handed and may claim no more.
#[test]
fn k1_clear_after_build_retires_superseded_and_quarantined_rows() {
    use devmap_store::PendingSupersede;

    let store = Store::open_in_memory().unwrap();
    store
        .enqueue_pending_paths(&[
            "built.py".to_string(),
            "quarantined.py".to_string(),
            "still_pending.py".to_string(),
            "\u{0}devmap:git-head-changed".to_string(),
        ])
        .unwrap();
    for _ in 0..devmap_store::MAX_PENDING_ATTEMPTS {
        store
            .bump_pending_attempts(&["quarantined.py".to_string()])
            .unwrap();
    }

    // A narrowed build: only the paths it was handed are answered.
    let cleared = store
        .clear_pending_superseded(PendingSupersede::IndexedPaths(&[
            "built.py".to_string(),
            "other.py".to_string(),
        ]))
        .unwrap();
    assert!(cleared.contains(&"built.py".to_string()), "{cleared:?}");
    assert!(
        cleared.contains(&"quarantined.py".to_string()),
        "a build supersedes retries nobody could finish: {cleared:?}"
    );
    assert!(
        cleared.contains(&"\u{0}devmap:git-head-changed".to_string()),
        "the generation was written at the current HEAD: {cleared:?}"
    );
    assert_eq!(
        store.get_pending_paths().unwrap(),
        vec!["still_pending.py".to_string()],
        "work a narrowed build did not cover stays queued"
    );
}

/// K1(e2): the whole-tree rule is a *time* rule, not a path rule.
///
/// This is the half the old code could not express. `clear_pending_after_build`
/// asked "was this path in the extraction set", and a directory never is — so
/// after a full build the live store still held 918 rows, every one a cargo
/// fingerprint directory, and `status` stayed NOT FRESH with no command able to
/// clear them: `repair --pending` correctly keeps a directory that exists,
/// because the daemon's drain expands one into its files.
#[test]
fn k1_a_whole_tree_build_supersedes_every_row_queued_before_it_started() {
    use devmap_store::PendingSupersede;

    let store = Store::open_in_memory().unwrap();
    store
        .enqueue_pending_paths(&[
            "rust-port/target-serve/debug/.fingerprint/serde-abc123".to_string(),
            "src".to_string(),
            ".".to_string(),
            "src/a.py".to_string(),
        ])
        .unwrap();

    // The build starts here. Anything queued after this instant describes an
    // edit the walk may not have seen.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let build_start = Store::queue_clock_now();
    std::thread::sleep(std::time::Duration::from_millis(5));
    store
        .enqueue_pending_paths(&["src/edited_mid_build.py".to_string()])
        .unwrap();

    let cleared = store
        .clear_pending_superseded(PendingSupersede::WholeTreeBuiltAt(build_start))
        .unwrap();
    assert_eq!(
        cleared.len(),
        4,
        "every row queued before the build started is answered: {cleared:?}"
    );
    assert!(
        cleared.contains(&"src".to_string()) && cleared.contains(&".".to_string()),
        "directories included — they are exactly what the path rule could not \
         retire: {cleared:?}"
    );
    assert_eq!(
        store.get_pending_paths().unwrap(),
        vec!["src/edited_mid_build.py".to_string()],
        "an edit that arrived mid-build must survive: the walk may not have seen it"
    );
}

/// K1(g): a degraded status names what is stuck.
#[test]
fn k1_status_degraded_reason_names_the_quarantined_paths() {
    let store = Store::open_in_memory().unwrap();
    let stuck: Vec<String> = (0..7).map(|n| format!("stuck_{n}.py")).collect();
    store.enqueue_pending_paths(&stuck).unwrap();
    for _ in 0..devmap_store::MAX_PENDING_ATTEMPTS {
        store.bump_pending_attempts(&stuck).unwrap();
    }

    let status = store.status(":memory:").unwrap();
    assert_eq!(status.quarantined_count, 7);
    assert_eq!(
        status.quarantined_paths.len(),
        Store::DEGRADED_SAMPLE,
        "the reason carries a bounded sample, not the whole queue"
    );
    let reason = status.degraded_reason.expect("a stuck queue is degraded");
    assert!(
        reason.contains("stuck_0.py"),
        "the reason must name paths, not just a count: {reason}"
    );
    assert!(
        reason.contains("and 2 more"),
        "the sample must say how much it elided: {reason}"
    );
    assert!(
        reason.contains("devmap repair --pending"),
        "the reason must name the remedy: {reason}"
    );
}

/// K1(f): `devmap repair --pending`'s store half drops what it reports.
#[test]
fn k1_drop_quarantined_returns_exactly_what_it_deleted() {
    let store = Store::open_in_memory().unwrap();
    store
        .enqueue_pending_paths(&["stuck.py".to_string(), "fresh.py".to_string()])
        .unwrap();
    for _ in 0..devmap_store::MAX_PENDING_ATTEMPTS {
        store
            .bump_pending_attempts(&["stuck.py".to_string()])
            .unwrap();
    }

    assert_eq!(
        store.drop_quarantined_pending_paths().unwrap(),
        vec!["stuck.py".to_string()]
    );
    assert_eq!(store.get_pending_paths().unwrap(), vec!["fresh.py"]);
    assert!(store.drop_quarantined_pending_paths().unwrap().is_empty());
}

/// K1(d): acknowledging a claim leaves a newer watcher event queued.
///
/// The previous guard was `attempts > 0`, which only worked because the drain
/// bumped every path in the batch before doing any work — the accounting that
/// let one store-level failure quarantine all 64 paths at once.
#[test]
fn k1_claim_acknowledgement_survives_a_concurrent_requeue() {
    let store = Store::open_in_memory().unwrap();
    store.enqueue_pending_paths(&["a.py".to_string()]).unwrap();
    let claims = store.claim_pending_batch(16).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(
        store.pending_attempts("a.py").unwrap(),
        Some(0),
        "claiming must not charge an attempt"
    );

    // A watcher event arrives mid-drain: newer work, same path.
    std::thread::sleep(std::time::Duration::from_millis(5));
    store.enqueue_pending_paths(&["a.py".to_string()]).unwrap();

    assert_eq!(
        store.clear_claimed_pending_paths(&claims).unwrap(),
        0,
        "the re-enqueued row is newer work and must not be acknowledged"
    );
    assert_eq!(store.get_pending_paths().unwrap(), vec!["a.py"]);

    // With no intervening event the claim clears.
    let claims = store.claim_pending_batch(16).unwrap();
    assert_eq!(store.clear_claimed_pending_paths(&claims).unwrap(), 1);
    assert!(store.get_pending_paths().unwrap().is_empty());
}

/// K1(d): a failing path must not charge an attempt to its batch-mates.
///
/// `bump_pending_attempts(&batch)` ran over the whole claimed batch *before any
/// work started*, and the acknowledgement at the end only deleted rows whose
/// attempt count was positive — so the bump was load-bearing, and every path in
/// a batch shared one fate. Any error in a later batch-wide step returned
/// before that acknowledgement, leaving every claimed path one attempt worse
/// off for a failure none of them caused. Five such rounds quarantine the whole
/// batch, which is exactly the state this repository's store was found in: 64
/// rows at `attempts = 5`, the daemon's batch limit exactly.
///
/// The batch-wide failure here is a real store-level fault, not an injected
/// one: the cross-process writer lock (K13) cannot be opened for writing, which
/// is what a permissions or filesystem fault looks like from inside the drain.
/// It is taken *after* the per-path loop and *before* the acknowledgement,
/// which is precisely the window this test exists to cover.
///
/// It used to be produced by enqueuing two spellings of one file — the absolute
/// path the watcher used and the relative one the reconcile sweep used — and
/// relying on the generation write to refuse the duplicate. That route is gone:
/// `drain_pending_batch` now keys its fresh extractions by path in a
/// `BTreeMap`, deliberately, so an overlapping batch resolves to one entry per
/// file instead of failing on every retry. The de-duplication is the newer,
/// better behaviour; only this fixture's way of provoking a failure was
/// retired by it, so the fixture moved rather than the property.
#[test]
fn k1_a_failing_path_does_not_charge_an_attempt_to_its_batch_mates() {
    use devmap_serve::Daemon;

    let dir = tmp_dir("k1-bump-isolation");
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("good.py"), "def good():\n    return 1\n").unwrap();
    let canonical_root = root.canonicalize().unwrap();
    let db = dir.join("devmap.sqlite");

    // A path that fails in isolation: it climbs out of the daemon root, which
    // `collect_pending_path` refuses. Enqueued through the raw primitive
    // precisely because the canonicalising producer refuses it up front — this
    // is a row of the kind already sitting in stores today.
    let poison = "../escapes.py".to_string();
    let sibling = canonical_root.join("good.py").display().to_string();

    {
        let store = Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&[poison.clone(), "good.py".to_string(), sibling.clone()])
            .unwrap();
        drop(store);

        // Make the writer lock unopenable for writing. Created and chmodded
        // here rather than mid-drain because the drain offers no seam: the
        // fault has to already exist when `lock_writer` reaches it. The claim
        // and every read above it still work — this touches one lock file, not
        // the database.
        let lock_path = Store::writer_lock_path(&db);
        fs::write(&lock_path, b"").unwrap();
        let mut permissions = fs::metadata(&lock_path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&lock_path, permissions).unwrap();

        let store = Store::open(&db).unwrap();
        let daemon = Daemon::new(store, root.clone());
        let error = daemon
            .drain_pending_batch()
            .expect_err("an unwritable writer lock must fail the batch-wide step");
        // Fails closed rather than quietly: running as a user that can write
        // through a read-only mode would make the fixture inert, and this
        // assertion is what says so instead of the test passing vacuously.
        assert!(
            error.to_string().to_lowercase().contains("permission")
                || error.to_string().contains("denied"),
            "fixture precondition — the writer lock must be what failed: {error}"
        );

        let mut permissions = fs::metadata(&lock_path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&lock_path, permissions).unwrap();
    }

    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.pending_attempts(&poison).unwrap(),
        Some(1),
        "the path that actually failed carries the attempt"
    );
    assert_eq!(
        store.pending_attempts("good.py").unwrap(),
        Some(0),
        "a path that resolved cleanly must not be charged for a batch-wide \
         failure it did not cause"
    );
    assert_eq!(store.pending_attempts(&sibling).unwrap(), Some(0));

    // And with no batch-wide failure, a clean path is acknowledged outright.
    store.clear_pending_paths(&[sibling]).unwrap();
    {
        let store = Store::open(&db).unwrap();
        let daemon = Daemon::new(store, root.clone());
        assert_eq!(daemon.drain_pending_batch().unwrap(), 1);
    }
    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.pending_attempts("good.py").unwrap(),
        None,
        "the path that succeeded is gone, not merely un-bumped"
    );
    assert_eq!(store.pending_attempts(&poison).unwrap(), Some(2));
    let _ = fs::remove_dir_all(&dir);
}

/// K1(h): the drain claims a whole generation's worth of work, not 64 paths.
///
/// Every drain resolves and analyses the entire repository and commits one
/// generation, so a batch's cost barely depends on how many paths it carries.
/// At 64, a 50,000-path event — a branch switch, a large `git checkout` — took
/// roughly 780 consecutive full rebuilds at one every two seconds. The cap is a
/// bound on transaction size, not a work budget.
#[test]
fn k1_default_drain_batch_is_a_bound_not_a_budget() {
    // Read through a binding so the assertion is a runtime check of the
    // exported constant rather than one clippy folds away as trivially true.
    let limit = std::hint::black_box(devmap_serve::DEFAULT_DRAIN_BATCH_LIMIT);
    assert!(
        limit >= 8192,
        "a 64-path cap multiplies full rebuilds instead of bounding work; found {limit}"
    );
}

/// K1(c): a discovery refusal is a coverage fact, not pending work.
///
/// The connect-time sweep enqueued every `Oversized` and `Unreadable` refusal
/// "to preserve failure as durable work" — but the drain applies the *same*
/// limits, so those rows were guaranteed to fail every attempt, quarantine, and
/// then sit in the queue forever holding `is_fresh` at false. The 30 MB
/// vendored `parser.c` found stuck on this repository is the case exactly: it
/// is over `MAX_SOURCE_BYTES`, and no number of retries will shrink it.
#[test]
fn k1_connect_time_sweep_does_not_queue_refusals_it_can_never_process() {
    use devmap_serve::Daemon;

    let dir = tmp_dir("k1-refusals");
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("good.py"), "def good():\n    return 1\n").unwrap();
    fs::write(
        root.join("vendored.c"),
        vec![b'x'; devmap_extract::MAX_SOURCE_BYTES as usize + 1],
    )
    .unwrap();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let daemon = Daemon::new(store, root.clone());
    daemon.reconcile_connect_time().expect("connect-time sweep");

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let pending = store.get_pending_paths().unwrap();
    assert_eq!(
        pending,
        vec!["good.py".to_string()],
        "only work a drain can actually do belongs in the queue"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// K2b: the reclaim reports what it moved, and moves what it was asked for.
///
/// `PRAGMA incremental_vacuum(N)` frees one page **per row stepped**. Neither
/// `execute` (which refuses a row-returning statement outright) nor
/// `execute_batch` (rusqlite 0.31 steps once, then advances to the next
/// statement) steps it to exhaustion, so the reclaim freed a single page per
/// build while `VacuumAction::Incremental` printed the number it had requested.
/// The request and the result now travel separately, and the result is counted.
#[test]
fn k2_incremental_vacuum_frees_every_page_it_was_asked_for() {
    let dir = tmp_dir("k2-pages-freed");
    let db_path = dir.join("devmap.sqlite");
    let freelist_before;
    {
        let store = Store::open(&db_path).unwrap();
        drop(store);
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE ballast (id INTEGER PRIMARY KEY, blob BLOB)")
            .unwrap();
        // Ballast scaled to the page size, so the fixture means the same thing
        // whatever it is. The assertions below are in *pages*, and a fixed byte
        // volume is a different number of pages on a different page size: 8 MB
        // is ~2,000 pages at 4 KiB and ~500 at 16 KiB, which silently turned
        // this precondition into a failure when the store's page size was
        // raised. Scaling the rows keeps the page count — and therefore every
        // threshold below — comparable.
        let page_size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .unwrap();
        let rows = 2000 * page_size / 4096;
        let payload = vec![b'x'; 4000];
        let tx = conn.unchecked_transaction().unwrap();
        for id in 0..rows {
            tx.execute(
                "INSERT INTO ballast (id, blob) VALUES (?1, ?2)",
                rusqlite::params![id, payload],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        conn.execute_batch("DROP TABLE ballast").unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        freelist_before = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))
            .unwrap();
    }
    assert!(
        freelist_before > 1_500,
        "fixture precondition: a freelist worth reclaiming, found {freelist_before}"
    );

    let store = Store::open(&db_path).unwrap();
    let outcome = store.vacuum_if_needed().unwrap();
    assert!(
        matches!(
            outcome.action,
            devmap_store::VacuumAction::Incremental { .. }
        ),
        "expected an incremental reclaim, got {:?}",
        outcome.action
    );
    assert!(
        outcome.pages_freed >= freelist_before - 128,
        "the reclaim must free what it was asked for, not one page: freed {} of \
         {freelist_before}",
        outcome.pages_freed
    );

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let freelist_after: i64 = conn
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .unwrap();
    assert!(
        freelist_after <= 128,
        "the freelist must actually be drained: {freelist_before} -> {freelist_after}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Write a Cache Directory Tagging Standard marker into `dir`.
fn tag_as_cache_dir(dir: &std::path::Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n\
         # This file is a cache directory tag created by a build tool.\n",
    )
    .unwrap();
}

/// K7: the watcher's enqueue path refuses build caches at the door.
///
/// The watcher fires on every write cargo makes into its output directory, and
/// each event became durable pending work: 47,000 rows from `target-serve` and
/// `target-store` on this repository. The refusal lives on the enqueue the
/// watcher callback calls, which is the single choke point both queue producers
/// already share — so it cannot be bypassed by adding a third producer.
#[test]
fn k7_watcher_events_inside_a_cache_directory_are_never_enqueued() {
    let dir = tmp_dir("k7-watch-cache");
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a(): pass\n").unwrap();
    tag_as_cache_dir(&root.join("target-serve"));
    fs::create_dir_all(root.join("target-serve/debug/.fingerprint")).unwrap();
    fs::write(
        root.join("target-serve/debug/.fingerprint/lib.py"),
        "def x(): pass\n",
    )
    .unwrap();

    let store = Store::open(dir.join("devmap.sqlite")).unwrap();
    let report = store
        .enqueue_pending_paths_under_root(
            &root,
            &[
                root.join("src/a.py").display().to_string(),
                root.join("target-serve/debug/.fingerprint/lib.py")
                    .display()
                    .to_string(),
                "target-serve".to_string(),
            ],
        )
        .unwrap();

    assert_eq!(
        report.enqueued,
        vec!["src/a.py".to_string()],
        "only real source is queued"
    );
    assert_eq!(report.refused.len(), 2, "{:?}", report.refused);
    assert!(
        report
            .refused
            .iter()
            .all(|(_, reason)| reason.contains("CACHEDIR.TAG")),
        "the refusal must say why: {:?}",
        report.refused
    );

    // And rows already in the queue from before the refusal existed are
    // dropped by the structural reconcile rather than retried forever.
    store
        .enqueue_pending_paths(&["target-serve/debug/.fingerprint/lib.py".to_string()])
        .unwrap();
    let outcome = store.reconcile_pending_paths(&root).unwrap();
    assert!(
        outcome
            .dropped
            .iter()
            .any(|(path, reason)| path.starts_with("target-serve/")
                && reason.contains("CACHEDIR.TAG")),
        "the reconcile must drop pre-existing cache rows: {:?}",
        outcome.dropped
    );
    assert_eq!(store.get_pending_paths().unwrap(), vec!["src/a.py"]);
    let _ = fs::remove_dir_all(&dir);
}

/// K7: the signature is checked, not the filename.
///
/// A source file that happens to be named `CACHEDIR.TAG` must not delete its
/// whole directory from the index — that would be a silent, total loss of
/// coverage triggered by a filename.
#[test]
fn k7_a_cachedir_tag_without_the_signature_is_not_a_cache_directory() {
    let dir = tmp_dir("k7-signature");
    let tagged = dir.join("tagged");
    let impostor = dir.join("impostor");
    tag_as_cache_dir(&tagged);
    fs::create_dir_all(&impostor).unwrap();
    fs::write(
        impostor.join("CACHEDIR.TAG"),
        "not the standard signature\n",
    )
    .unwrap();
    let short = dir.join("short");
    fs::create_dir_all(&short).unwrap();
    fs::write(short.join("CACHEDIR.TAG"), "Signature:").unwrap();
    let bare = dir.join("bare");
    fs::create_dir_all(&bare).unwrap();

    assert!(devmap_extract::is_cache_directory(&tagged));
    assert!(!devmap_extract::is_cache_directory(&impostor));
    assert!(
        !devmap_extract::is_cache_directory(&short),
        "a file shorter than the signature cannot carry it"
    );
    assert!(!devmap_extract::is_cache_directory(&bare));
    let _ = fs::remove_dir_all(&dir);
}
