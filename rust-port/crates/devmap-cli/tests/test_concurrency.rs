//! SC28 — concurrent processes must not corrupt the store or lose invariants.
//!
//! Every gate so far drove the binary one process at a time, but a real
//! repository has a watcher, an editor hook and a developer's own `dev map` all
//! reaching the same SQLite file. Nothing established that two builds racing
//! produce a valid store rather than a torn one, and "it uses SQLite" is an
//! assumption, not a measurement — WAL, the busy timeout, retention pruning and
//! the FTS index all have to hold together under contention.
//!
//! These drive the binary as separate OS processes on purpose. In-process
//! threads would share one connection pool and test something easier than what
//! actually happens.

use std::fs;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-conc-{tag}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    root
}

fn write_fixture(root: &std::path::Path, round: usize) {
    for index in 0..12 {
        fs::write(
            root.join("src").join(format!("mod{index}.py")),
            format!(
                "def helper{index}():\n    return {round}\n\n\
                 def caller{index}():\n    return helper{index}()\n"
            ),
        )
        .expect("write fixture module");
    }
}

/// Spawn `count` builds at once and require every one to exit successfully.
fn race_builds(db: &std::path::Path, root: &std::path::Path, count: usize) {
    let children: Vec<_> = (0..count)
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_devmap"))
                .args(["--progress", "never", "--db"])
                .arg(db)
                .arg("build")
                .arg(root)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn devmap build")
        })
        .collect();

    for (index, child) in children.into_iter().enumerate() {
        let output = child.wait_with_output().expect("await devmap build");
        assert!(
            output.status.success(),
            "concurrent build {index} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// Raw counts, for the same reason the retention tests use rusqlite: these are
/// per-table integrity questions with no equivalent on the public `Store` API,
/// and adding a raw-SQL escape hatch to `Store` for tests would be worse.
fn scalar(db: &std::path::Path, sql: &str) -> i64 {
    let conn = rusqlite::Connection::open(db).expect("open sqlite");
    conn.query_row(sql, [], |row| row.get(0))
        .expect("scalar query")
}

fn integrity_check(db: &std::path::Path) -> String {
    let conn = rusqlite::Connection::open(db).expect("open sqlite");
    conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .expect("integrity_check")
}

/// Racing builds leave a valid store, a bounded generation count, and a call
/// graph whose edges still join to real symbols.
///
/// The joinability check is the load-bearing one: SC9/SC10 showed that a graph
/// can be structurally intact yet have edges naming symbols that do not exist,
/// which no integrity check would catch.
#[test]
fn concurrent_builds_keep_the_store_valid_and_bounded() {
    let root = temp_root("writers");
    let db = root.join("index.sqlite");

    for round in 0..3 {
        write_fixture(&root, round);
        race_builds(&db, &root, 4);
    }

    assert_eq!(integrity_check(&db), "ok", "integrity_check must report ok");
    let generations = scalar(&db, "SELECT COUNT(*) FROM generations");
    assert!(
        generations <= i64::try_from(devmap_store::GENERATION_RETENTION).unwrap(),
        "retention must hold under contention: {generations} generations kept"
    );
    assert!(
        scalar(&db, "SELECT COUNT(*) FROM generation_nodes") > 0,
        "a raced build must still commit a populated generation"
    );
    let orphans = scalar(
        &db,
        "SELECT COUNT(*) FROM generation_edges e \
         WHERE e.edge_kind = 'Calls' AND NOT EXISTS ( \
           SELECT 1 FROM generation_nodes n \
           WHERE n.generation_id = e.generation_id \
             AND n.qualified_name = e.source_symbol)",
    );
    assert_eq!(
        orphans, 0,
        "every call edge must still name a symbol that exists"
    );
}

/// Readers must keep answering while a build commits.
///
/// A query that fails because a writer holds the database is a user-visible
/// outage, and the 5-second busy timeout plus WAL exist precisely to prevent it.
#[test]
fn queries_succeed_while_a_build_is_committing() {
    let root = temp_root("readers");
    let db = root.join("index.sqlite");
    write_fixture(&root, 0);
    race_builds(&db, &root, 1);

    write_fixture(&root, 1);
    let mut writer = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--progress", "never", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn writer");

    let mut failures = Vec::new();
    for attempt in 0..12 {
        let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["--db"])
            .arg(&db)
            .arg("status")
            .output()
            .expect("run status");
        if !output.status.success() {
            failures.push(format!(
                "attempt {attempt}: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    writer.wait().expect("await writer");

    assert!(
        failures.is_empty(),
        "reads must not fail while a build commits: {failures:?}"
    );
}
