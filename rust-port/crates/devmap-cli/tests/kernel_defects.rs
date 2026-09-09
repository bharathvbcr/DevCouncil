//! Regression gates for the kernel defects K1–K13.
//!
//! Every test here failed against the tree as it stood before the
//! corresponding fix; the assertion messages name what the old behaviour was so
//! a future reader can tell a genuine regression from a re-specification.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

/// Unique per-test scratch root. Same shape as `store_hardening::tmp_dir`: a
/// microsecond stamp alone collides between same-tick callers, and the
/// `remove_dir_all` below would then delete a live sibling's fixture.
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
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture(label: &str) -> PathBuf {
    let root = tmp_dir(label);
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.py"), "def a():\n    return helper()\n").unwrap();
    std::fs::write(src.join("b.py"), "def helper():\n    return 1\n").unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .expect("devmap invocation")
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// K6: the default `--db` must be the Rust kernel's own store.
///
/// It defaulted to `.devcouncil/codeintel/index.sqlite`, which is the *Python*
/// engine's store (`user_version = 2`). Every `devmap` invocation that did not
/// pass `--db` therefore aimed at a database this binary cannot read, while the
/// Python seam (`devmap_engine.DEFAULT_DB_RELPATH`) had already moved to
/// `devmap.sqlite`. The two halves of one system disagreed about where the
/// store lives.
#[test]
fn k6_default_db_is_the_rust_store_the_python_seam_uses() {
    let root = fixture("k6-default-db");
    let output = run(&root, &["build", "."]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resolved = devmap_extract::paths::store_path(&root);
    assert!(
        resolved.is_file(),
        "the default --db must be the store devmap_extract::paths resolves for \
         the tree being built, got nothing at {}",
        resolved.display()
    );
    // The claim this test exists to make, unchanged: whatever directory the
    // default resolves to, it is never the Python engine's store. Checked under
    // both layouts, because a fixture that already had `.devcouncil/` would
    // resolve there and the assertion must still bite.
    for dir in [".devmap", ".devcouncil"] {
        assert!(
            !root.join(dir).join("codeintel/index.sqlite").exists(),
            "the default must no longer touch the Python engine's store ({dir})"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// K3: `devmap --version` must state the schema version too.
///
/// Every build of this workspace reports `devmap 0.1.0`, so the package version
/// alone cannot tell a caller whether the binary in hand can open the store in
/// hand. The schema number is the only part of the identity that answers that,
/// and reading it otherwise means opening a store — which is exactly what a
/// caller checking compatibility cannot safely do yet.
#[test]
fn k3_version_reports_the_schema_version_alongside_the_package_version() {
    let root = tmp_dir("k3-version");
    let output = run(&root, &["--version"]);
    let text = format!(
        "{}{}",
        stdout_of(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains(&format!("schema {}", devmap_store::CURRENT_SCHEMA_VERSION)),
        "`devmap --version` must name the schema version: {text:?}"
    );
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "`devmap --version` must still name the package version: {text:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// K3: `devmap status` is a read. It must not migrate the store.
///
/// `Store::open` runs the migration chain under an exclusive transaction from
/// every open, so asking a stale store how it was doing silently rewrote its
/// schema — an exclusive-lock write nobody requested, on the one command a
/// health check runs against a store it does not own. A read reports what it
/// found; migration belongs to `build`.
#[test]
fn k3_status_reports_an_outdated_schema_instead_of_migrating_it() {
    let root = fixture("k3-status-nomigrate");
    assert!(run(&root, &["build", "."]).status.success());
    let db = devmap_extract::paths::store_path(&root);
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute("PRAGMA user_version = 11", []).unwrap();
    }

    let output = run(&root, &["status"]);
    assert!(
        output.status.success(),
        "status must answer rather than fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_str(stdout_of(&output).trim()).unwrap();
    assert_eq!(
        payload["schema_outdated"],
        serde_json::json!(true),
        "status must report the store's schema as outdated: {payload}"
    );
    assert_eq!(payload["schema_version"], serde_json::json!(11));
    assert_eq!(
        payload["expected_schema_version"],
        serde_json::json!(devmap_store::CURRENT_SCHEMA_VERSION)
    );
    assert_eq!(
        payload["is_fresh"],
        serde_json::json!(false),
        "a store this binary cannot read is not fresh"
    );

    let conn = rusqlite::Connection::open(&db).unwrap();
    let observed: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        observed, 11,
        "status must not have run a schema migration under an exclusive lock"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// K4: `devmap build --full` forces a cold rebuild.
///
/// The Python CLI exposes `dev map --full`, but the kernel had no way to honour
/// it: the unchanged early-return and the extraction cache both applied
/// unconditionally, so an operator who suspected the store was wrong had
/// exactly one recovery — delete the database. `--full` bypasses both and
/// writes a full generation (empty affected set) from freshly parsed sources.
#[test]
fn k4_full_forces_a_new_generation_over_an_unchanged_tree() {
    let root = fixture("k4-full");
    assert!(run(&root, &["build", "."]).status.success());

    let warm = stdout_of(&run(&root, &["build", "."]));
    assert!(
        warm.contains("No source changes"),
        "precondition: a second build over an unchanged tree is skipped: {warm}"
    );

    let db = devmap_extract::paths::store_path(&root);
    let before = devmap_store::Store::open(&db)
        .unwrap()
        .latest_generation_id()
        .unwrap();

    let forced = run(&root, &["build", ".", "--full"]);
    assert!(
        forced.status.success(),
        "build --full failed: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
    let text = stdout_of(&forced);
    assert!(
        !text.contains("No source changes"),
        "--full must not take the unchanged early return: {text}"
    );

    let after = devmap_store::Store::open(&db)
        .unwrap()
        .latest_generation_id()
        .unwrap();
    assert!(
        after > before,
        "--full must persist a new generation ({before:?} -> {after:?})"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// K2: a warm build still reclaims, and the reclaim reaches the main file.
///
/// Two defects compounded here. The unchanged early-return jumped past
/// prune/vacuum entirely, so a store whose freelist was already large stayed
/// large through every no-change build. And `vacuum_if_needed` checkpointed
/// only *before* the incremental vacuum — in WAL mode the truncation it
/// performs lands in the WAL and never reaches the main database file without a
/// checkpoint *after*. Measured on this repository's own store: 42% freelist,
/// file size unchanged across eight consecutive builds, 109 MB of WAL.
///
/// The freelist is manufactured rather than accumulated: what is under test is
/// the reclaim, not the churn that produces waste.
#[test]
fn k2_a_no_change_build_reclaims_a_large_freelist_from_the_main_file() {
    let root = fixture("k2-warm-reclaim");
    assert!(run(&root, &["build", "."]).status.success());
    let db = devmap_extract::paths::store_path(&root);

    // Manufacture waste: write ~8 MB into a scratch table and drop it. The
    // pages go to the freelist; only a vacuum returns them to the filesystem.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE k2_ballast (id INTEGER PRIMARY KEY, blob BLOB)")
            .unwrap();
        let payload = vec![b'x'; 4000];
        let tx = conn.unchecked_transaction().unwrap();
        for id in 0..2000 {
            tx.execute(
                "INSERT INTO k2_ballast (id, blob) VALUES (?1, ?2)",
                rusqlite::params![id, payload],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        conn.execute_batch("DROP TABLE k2_ballast").unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
    }

    let (freelist_before, page_count_before) = page_accounting(&db);
    let bytes_before = std::fs::metadata(&db).unwrap().len();
    assert!(
        freelist_before * 20 > page_count_before,
        "fixture precondition: freelist {freelist_before} of {page_count_before} pages \
         must exceed the 5% vacuum threshold"
    );

    let output = run(&root, &["build", "."]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout_of(&output).contains("No source changes"),
        "precondition: this build must take the unchanged path: {}",
        stdout_of(&output)
    );

    let (freelist_after, page_count_after) = page_accounting(&db);
    let bytes_after = std::fs::metadata(&db).unwrap().len();

    // The reclaim must actually drain the freelist, not nibble at it.
    //
    // `PRAGMA incremental_vacuum(N)` frees one page *per stepped row*, and
    // rusqlite 0.31's `execute_batch` steps exactly once — so the pragma
    // returned after freeing a single page while reporting that it had been
    // asked for 65,536. On the live 701 MB store that read
    // `reclaim: incremental(65536 pages) at 67.7% free (116045 of 171335
    // pages)` in 1 ms, and across four consecutive builds the freelist moved
    // 116116 -> 116045 while the file never left 701 MB. An assertion that only
    // required the file to get smaller passed on that one page.
    //
    // `SLACK` covers pages the checkpoint and the reclaim's own bookkeeping
    // re-allocate; it is two orders of magnitude below the freelist under test,
    // so a one-page reclaim cannot hide inside it.
    const SLACK: i64 = 128;
    let expected_reclaim = freelist_before.min(vacuum_max_pages(&db));
    assert!(
        freelist_after <= freelist_before - expected_reclaim + SLACK,
        "the reclaim freed {} of an expected {expected_reclaim} pages \
         (freelist {freelist_before} -> {freelist_after})",
        freelist_before - freelist_after
    );
    assert!(
        page_count_before - page_count_after >= expected_reclaim - SLACK,
        "the file must shrink by about what was reclaimed: page_count \
         {page_count_before} -> {page_count_after}, expected to drop by \
         ~{expected_reclaim}"
    );
    assert!(
        bytes_after < bytes_before,
        "a no-change build must return free pages to the filesystem \
         ({bytes_before} -> {bytes_after} bytes)"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The reclaim cap, asked of the code that owns it rather than copied.
///
/// This was a hand-mirrored `const` carrying the cap's old value in pages. The
/// cap is a byte budget now, converted against the page size of the database in
/// front of it, and a mirror would have kept asserting against 65,536 pages —
/// still passing, because the fixture's freelist is far below either number,
/// and silently meaning something else. `Store::incremental_vacuum_max_pages`
/// is public for this: one owner for the policy, no second copy to drift.
fn vacuum_max_pages(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).unwrap();
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .unwrap();
    devmap_store::Store::incremental_vacuum_max_pages(page_size)
}

fn page_accounting(db: &Path) -> (i64, i64) {
    let conn = rusqlite::Connection::open(db).unwrap();
    let freelist: i64 = conn
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .unwrap();
    let pages: i64 = conn
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    (freelist, pages)
}

/// K1(e)+(g): a build cleans the queue, and a clean build reports fresh.
///
/// The measured failure: this repository's store held 64 pending rows at
/// `attempts = 5` that nothing could clear. `MAX_PENDING_ATTEMPTS` was only a
/// *read* filter, the sole non-test deleter acknowledged successful daemon work,
/// and `devmap build` never touched `pending_paths` at all — so
/// `devmap status` reported `is_fresh=false` and a degraded store forever, on a
/// tree that was perfectly indexed.
///
/// Every row in this fixture is one of the shapes actually found stuck.
#[test]
fn k1_build_reconciles_the_pending_queue_and_restores_freshness() {
    let root = fixture("k1-build-reconcile");
    assert!(run(&root, &["build", "."]).status.success());
    let db = devmap_extract::paths::store_path(&root);

    std::fs::write(
        root.join("huge.c"),
        vec![b'x'; devmap_extract::MAX_SOURCE_BYTES as usize + 1],
    )
    .unwrap();
    let stale_absolute = root
        .parent()
        .unwrap()
        .join("some-old-checkout/src/a.py")
        .display()
        .to_string();

    {
        let store = devmap_store::Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&[
                stale_absolute.clone(),
                "src".to_string(),
                "huge.c".to_string(),
                "README.md".to_string(),
                "src/a.py".to_string(),
            ])
            .unwrap();
        // Quarantine them all, the state the live store was found in.
        for _ in 0..devmap_store::MAX_PENDING_ATTEMPTS {
            store
                .bump_pending_attempts(
                    &store
                        .claim_pending_batch(usize::MAX)
                        .unwrap()
                        .into_iter()
                        .filter(|claim| {
                            [
                                stale_absolute.clone(),
                                "src".to_string(),
                                "huge.c".to_string(),
                                "README.md".to_string(),
                                "src/a.py".to_string(),
                            ]
                            .contains(&claim.path)
                        })
                        .collect::<Vec<_>>(),
                )
                .unwrap();
        }
        let before = store.status("x").unwrap();
        assert_eq!(before.quarantined_count, 5, "fixture precondition");
        assert!(
            before
                .degraded_reason
                .as_deref()
                .is_some_and(|reason| reason.contains(&stale_absolute)),
            "status must name what is stuck: {:?}",
            before.degraded_reason
        );
    }

    // A change, so this build actually writes a generation.
    std::fs::write(root.join("src/c.py"), "def c():\n    return 3\n").unwrap();
    let output = run(&root, &["build", "."]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = devmap_store::Store::open(&db).unwrap();
    let remaining = store.get_pending_paths().unwrap();
    assert!(
        remaining.is_empty(),
        "a committed full build leaves nothing the queue still owes: {remaining:?}"
    );
    let status = store.status("x").unwrap();
    assert_eq!(status.quarantined_count, 0);
    assert_eq!(status.degraded_reason, None);

    let payload: serde_json::Value =
        serde_json::from_str(stdout_of(&run(&root, &["status"])).trim()).unwrap();
    assert_eq!(
        payload["is_fresh"],
        serde_json::json!(true),
        "a clean build over a previously quarantined store must report fresh: {payload}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// K1(e): the unchanged early return still reconciles the queue.
///
/// A repository whose sources have not moved is exactly where a stale queue
/// hides — nothing else ever looks at it — so the warm path must not skip the
/// structural pass just because it skips the rebuild.
#[test]
fn k1_a_warm_build_still_drops_unprocessable_pending_rows() {
    let root = fixture("k1-warm-reconcile");
    assert!(run(&root, &["build", "."]).status.success());
    let db = devmap_extract::paths::store_path(&root);
    {
        let store = devmap_store::Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&["/definitely/not/in/this/repo.py".to_string()])
            .unwrap();
    }

    let output = run(&root, &["build", "."]);
    assert!(
        stdout_of(&output).contains("No source changes"),
        "precondition: this build must take the unchanged path"
    );
    assert!(
        devmap_store::Store::open(&db)
            .unwrap()
            .get_pending_paths()
            .unwrap()
            .is_empty(),
        "the warm path must still drop rows outside the repository"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// K1(f): `devmap repair --pending` drops what is stuck and says what it dropped.
#[test]
fn k1_repair_pending_drops_stuck_rows_and_reports_them() {
    let root = fixture("k1-repair-pending");
    assert!(run(&root, &["build", "."]).status.success());
    let db = devmap_extract::paths::store_path(&root);
    // A real, indexable file: the structural pass has no reason to touch it, so
    // only the quarantine pass can drop it. That separation is the point —
    // otherwise this test could pass with the quarantine drop missing entirely.
    std::fs::write(root.join("stuck.py"), "def stuck():\n    return 1\n").unwrap();
    {
        let store = devmap_store::Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&[
                "/elsewhere/gone.py".to_string(),
                "stuck.py".to_string(),
                "src/a.py".to_string(),
            ])
            .unwrap();
        for _ in 0..devmap_store::MAX_PENDING_ATTEMPTS {
            store
                .bump_pending_attempts(
                    &store
                        .claim_pending_batch(usize::MAX)
                        .unwrap()
                        .into_iter()
                        .filter(|claim| ["stuck.py".to_string()].contains(&claim.path))
                        .collect::<Vec<_>>(),
                )
                .unwrap();
        }
    }

    let output = run(&root, &["--json", "repair", "--pending"]);
    assert!(
        output.status.success(),
        "repair --pending failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_str(stdout_of(&output).trim()).unwrap();
    let unprocessable = payload["dropped_unprocessable"].as_array().unwrap();
    assert!(
        unprocessable
            .iter()
            .any(|entry| entry["path"] == "/elsewhere/gone.py"),
        "the path outside the repository must be dropped and named: {payload}"
    );
    assert!(
        payload["dropped_quarantined"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "stuck.py"),
        "the quarantined row must be dropped and named: {payload}"
    );

    let remaining = devmap_store::Store::open(&db)
        .unwrap()
        .get_pending_paths()
        .unwrap();
    assert_eq!(
        remaining,
        vec!["src/a.py".to_string()],
        "real pending work must survive the repair"
    );

    // Naming no target is still an error, and now names both.
    let bare = run(&root, &["repair"]);
    assert!(!bare.status.success());
    let stderr = String::from_utf8_lossy(&bare.stderr);
    assert!(stderr.contains("--pending"), "{stderr}");
    let _ = std::fs::remove_dir_all(&root);
}

/// K13: a build waits for another writer instead of racing it.
///
/// The test process itself takes the writer lock, so contention is a fact
/// rather than a hoped-for interleaving. Before the lock existed the second
/// build simply proceeded, and the two writers met at the persist with nothing
/// but SQLite's busy timeout between them.
#[test]
fn k13_a_build_waits_for_a_held_writer_lock_and_never_reports_database_is_locked() {
    let root = fixture("k13-build-waits");
    assert!(run(&root, &["build", "."]).status.success());
    let db = devmap_extract::paths::store_path(&root);
    std::fs::write(root.join("src/c.py"), "def c():\n    return 3\n").unwrap();

    let held = devmap_store::Store::lock_writer_at(&db, std::time::Duration::from_millis(100))
        .expect("test process takes the writer lock");

    let mut child = Command::new(devmap())
        .args(["build", "."])
        .current_dir(&root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn a competing build");

    // It must still be waiting: a build that got past the lock would have
    // finished this fixture in well under a second.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the second build ran while another writer held the store"
    );

    drop(held);
    let output = child.wait_with_output().expect("competing build finishes");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the waiting build must succeed once the lock is free: {stderr}"
    );
    assert!(
        !stderr.contains("database is locked"),
        "a serialized writer must never see SQLite's lock error: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// K1(e2): a whole-tree build supersedes every row queued before it started —
/// including rows that name directories.
///
/// `clear_pending_after_build` only retired rows whose path was itself in the
/// extraction set, and a directory is never an extraction. So after a full
/// `dev map` the live store still held 918 pending rows, every one a directory,
/// and `status` stayed NOT FRESH forever. `repair --pending` could not help
/// either: the directories exist, so the structural pass correctly keeps them
/// (the daemon's drain expands a directory into its files).
///
/// The class fix is to stop asking which *paths* a build touched and ask what
/// it *proved*: a build with no `--affected` narrowing read the entire tree, so
/// every request queued before it started is answered, whatever it named.
#[test]
fn k1_a_whole_tree_build_supersedes_directory_rows_queued_before_it() {
    let root = fixture("k1-directory-rows");
    assert!(run(&root, &["build", "."]).status.success());
    let db = devmap_extract::paths::store_path(&root);
    {
        let store = devmap_store::Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&[
                "src".to_string(),
                ".".to_string(),
                "src/a.py".to_string(),
                "README.md".to_string(),
            ])
            .unwrap();
        assert_eq!(store.get_pending_paths().unwrap().len(), 4);
    }

    // A real change, so this build persists a generation.
    std::fs::write(root.join("src/c.py"), "def c():\n    return 3\n").unwrap();
    let output = run(&root, &["build", "."]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let remaining = devmap_store::Store::open(&db)
        .unwrap()
        .get_pending_paths()
        .unwrap();
    assert!(
        remaining.is_empty(),
        "a whole-tree build answers every request queued before it, directories \
         included: {remaining:?}"
    );

    // And a request that arrives *after* a build starts is not answered by it.
    {
        let store = devmap_store::Store::open(&db).unwrap();
        store
            .enqueue_pending_paths(&["src/late.py".to_string()])
            .unwrap();
    }
    std::fs::write(root.join("src/late.py"), "def late():\n    return 4\n").unwrap();
    let warm = run(&root, &["build", "."]);
    assert!(warm.status.success());
    let _ = std::fs::remove_dir_all(&root);
}

/// Write a Cache Directory Tagging Standard marker into `dir`.
///
/// The exact 43-byte signature line cargo, pip, uv, ccache and friends write.
/// Verified against `rust/target/CACHEDIR.TAG` and `.ruff_cache/CACHEDIR.TAG`
/// on this machine.
fn tag_as_cache_dir(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n\
         # This file is a cache directory tag created by a build tool.\n",
    )
    .unwrap();
}

/// K7: a tagged cache directory is not source, however it is named.
///
/// `is_ignored_path` matches a fixed list of directory *names* — `target`,
/// `node_modules`, `dist`, `build` — so a cargo output directory called
/// anything else walks straight in. Measured in generation 779 of the live
/// store: 1,041 of 2,363 indexed files were `.fingerprint/*.json` and
/// `.rustc_info.json` under `rust-port/target-serve` and `target-store`,
/// neither gitignored nor named `target`, and the daemon had queued 47,000 rows
/// from them.
///
/// Both directories carried a `CACHEDIR.TAG`. That is the Cache Directory
/// Tagging Standard, and it is the one signal that does not depend on guessing
/// a name: the tool that created the cache says so itself.
#[test]
fn k7_a_cachedir_tagged_directory_is_never_walked_or_indexed() {
    let root = fixture("k7-cachedir-walk");
    let cache = root.join("target-serve/debug/.fingerprint");
    tag_as_cache_dir(&root.join("target-serve"));
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("build_script.py"), "def gen():\n    return 1\n").unwrap();
    std::fs::write(
        root.join("target-serve/rustc_info.py"),
        "def info(): pass\n",
    )
    .unwrap();

    let output = run(&root, &["--json", "build", "."]);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let db = devmap_extract::paths::store_path(&root);
    let indexed: Vec<String> = devmap_store::Store::open(&db)
        .unwrap()
        .latest_file_hashes()
        .unwrap()
        .into_keys()
        .collect();
    assert!(
        indexed.iter().any(|path| path == "src/a.py"),
        "precondition: real sources are still indexed: {indexed:?}"
    );
    assert!(
        !indexed.iter().any(|path| path.starts_with("target-serve/")),
        "a CACHEDIR.TAG directory must not be walked: {indexed:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// K7: `--affected` naming a path that is not repo-relative is refused.
///
/// The MCP `sync` tool forwards `arguments["paths"]` to `--affected` verbatim
/// (`integrations/mcp/handlers/codeintel.py`), so these strings are agent
/// input, not just a developer's shell history. `..` and a leading `/` were
/// walked as if they were repo-relative: the cache check joined them onto the
/// root and probed a sibling checkout, then reported "no tagged ancestor" —
/// the same answer it gives a file it checked and cleared. Refusing names the
/// rule the path broke instead.
#[test]
fn k7_affected_paths_that_escape_the_root_are_refused() {
    let root = fixture("k7-affected-escape");
    assert!(run(&root, &["build", "."]).status.success());

    for (candidate, expected) in [
        ("../outside.py", "`..`"),
        ("src/../../outside.py", "`..`"),
        ("/etc/passwd", "absolute"),
    ] {
        let output = run(&root, &["build", ".", "--affected", candidate]);
        assert!(
            !output.status.success(),
            "--affected {candidate} must be refused, not walked"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(candidate) && stderr.contains(expected),
            "the refusal must name the path and the rule it broke: {stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// K7: `--affected` naming a path inside a tagged cache directory is refused.
///
/// Fail loud rather than quietly narrow to nothing: a caller passing a cache
/// path has computed the wrong change set, and silently indexing it — or
/// silently dropping it — hides that.
#[test]
fn k7_affected_paths_inside_a_cache_directory_are_refused() {
    let root = fixture("k7-cachedir-affected");
    assert!(run(&root, &["build", "."]).status.success());
    tag_as_cache_dir(&root.join("target-serve"));
    std::fs::write(root.join("target-serve/x.py"), "def x(): pass\n").unwrap();

    let output = run(&root, &["build", ".", "--affected", "target-serve/x.py"]);
    assert!(
        !output.status.success(),
        "a cache path in --affected must be refused, not indexed"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("target-serve") && stderr.contains("CACHEDIR.TAG"),
        "the refusal must name the path and why: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
