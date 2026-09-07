use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Temp roots must be unique per call, not merely per instant. `SystemTime`
/// resolution on macOS is 1 us, so two tests entering this function in the same
/// microsecond used to receive the *same* directory; whichever finished first
/// deleted the tree out from under its sibling. The pid and the monotonic
/// counter make the name unique within and across processes.
fn temp_root() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-progress-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    fs::write(root.join("src/main.py"), "def main():\n    return 0\n").expect("write fixture");
    root
}

#[test]
fn build_progress_is_bounded_complete_and_keeps_json_stdout_clean() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--progress", "always", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .output()
        .expect("run build with forced progress");

    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout remains one JSON value");
    assert_eq!(payload["files_indexed"], 1);

    let progress = String::from_utf8(output.stderr).expect("progress is UTF-8");
    for expected in ["[1/5]", "[2/5]", "[3/5]", "[4/5]", "[5/5]"] {
        assert!(
            progress.contains(expected),
            "missing {expected}: {progress}"
        );
    }
    assert!(
        progress.contains("complete"),
        "missing completion: {progress}"
    );

    fs::remove_dir_all(root).expect("remove fixture tree");
}

/// `persist:write` carries the split of what it wrote, by relation.
///
/// One number cannot be acted on. On this repository `persist:write` is 0.30 s
/// of a 1.10 s one-file incremental build, and the relations beneath it have
/// nothing in common as fixes: v18 put the edges and the unresolved ledger on
/// validity ranges and left the nodes, the full-text map, the file rows, the
/// dead symbols and the coverage gaps as full per-generation copies. Which of
/// those the 0.30 s is decides whether the next schema rung is worth its
/// migration, and no profiler outside the store can answer it — the node and
/// full-text inserts are one interleaved loop.
///
/// The split is asserted here, on the CLI's own `--json` output, because that
/// is where a reader meets it. Two properties, both about honesty:
/// every relation is named even when it wrote nothing, and the parts never
/// outlast the phase that contains them.
#[test]
fn the_persist_write_phase_reports_what_each_relation_cost() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--progress", "never", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .output()
        .expect("run build");
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout is one JSON value");

    /// The sub-phase named `wanted`, at any nesting depth.
    fn find<'a>(stages: &'a [serde_json::Value], wanted: &str) -> Option<&'a serde_json::Value> {
        for stage in stages {
            if stage["stage"] == wanted {
                return Some(stage);
            }
            if let Some(nested) = stage["sub"].as_array() {
                if let Some(hit) = find(nested, wanted) {
                    return Some(hit);
                }
            }
        }
        None
    }

    let stages = payload["timings"]["stages"]
        .as_array()
        .unwrap_or_else(|| panic!("a build carries timings: {payload}"));
    let write = find(stages, "persist:write")
        .unwrap_or_else(|| panic!("the write is a timed phase: {payload}"));
    let parts = write["sub"]
        .as_array()
        .unwrap_or_else(|| panic!("persist:write reports no per-relation split: {write}"));

    let named: Vec<&str> = parts
        .iter()
        .map(|part| part["stage"].as_str().unwrap_or("<unnamed>"))
        .collect();
    // Every relation, always — a relation that wrote nothing this build reports
    // zero rather than vanishing, because a missing name and a name worth
    // nothing are the same silence to a reader deciding what to fix.
    assert_eq!(
        named,
        vec![
            "file_rows",
            "nodes",
            "fts",
            "edges",
            "unresolved",
            "digests",
            "gaps",
            "dead",
            "history",
            "commit",
        ],
        "the split names every relation the write touches: {write}"
    );

    let whole = write["seconds"].as_f64().expect("the write has a duration");
    let charged: f64 = parts
        .iter()
        .map(|part| part["seconds"].as_f64().expect("a part has a duration"))
        .sum();
    assert!(
        charged <= whole + 1e-6,
        "the parts of a phase cannot outlast it: {charged}s charged of {whole}s: {write}"
    );

    fs::remove_dir_all(root).expect("remove fixture tree");
}

/// Fixture roots must never be shared between concurrently running tests.
/// Against a purely timestamp-keyed root this fails: `SystemTime` advances in
/// 1 us steps here, so threads entering together receive one identical path and
/// the first teardown destroys a live sibling's tree.
#[test]
fn fixture_roots_are_unique_under_concurrent_construction() {
    let roots: Vec<std::path::PathBuf> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| scope.spawn(|| (0..16).map(|_| temp_root()).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("fixture thread must not panic"))
            .collect()
    });

    let distinct: std::collections::BTreeSet<_> = roots.iter().collect();
    assert_eq!(
        distinct.len(),
        roots.len(),
        "temp_root() handed the same directory to two callers"
    );
    for root in roots {
        fs::remove_dir_all(root).expect("remove fixture tree");
    }
}

#[test]
fn progress_never_suppresses_all_progress_output() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--progress", "never", "--db"])
        .arg(&db)
        .arg("build")
        .arg(&root)
        .output()
        .expect("run build without progress");

    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    fs::remove_dir_all(root).expect("remove fixture tree");
}

#[test]
fn history_reports_measured_builds_and_deltas_as_json() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    for body in [
        "def main():\n    return 0\n",
        "def main():\n    return 1\n\ndef helper():\n    return 2\n",
    ] {
        fs::write(root.join("src/main.py"), body).expect("update fixture");
        let build = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["--json", "--progress", "never", "--db"])
            .arg(&db)
            .arg("build")
            .arg(&root)
            .output()
            .expect("run history fixture build");
        assert!(
            build.status.success(),
            "build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--db"])
        .arg(&db)
        .args(["history", "--last", "2"])
        .output()
        .expect("query build history");
    assert!(
        output.status.success(),
        "history failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("history stdout is JSON");
    assert_eq!(payload["shown"], 2);
    let history = payload["history"].as_array().expect("history is an array");
    assert_eq!(history.len(), 2);
    assert!(history[0]["build_ms"].is_number());
    assert!(history[0]["delta"]["symbols"].is_number());
    assert!(history[1]["delta"].is_null());

    fs::remove_dir_all(root).expect("remove fixture tree");
}

/// The most frequent build in the system had no phase profile at all.
///
/// A no-source-change build is what a watcher-driven repository does on almost
/// every tick, and `--json` answered it with
/// `{"unchanged":true,"files":…,"generation":…,"reclaim":…}` — no `timings` key
/// of any kind. So the one build shape a profiler most wants to look at was the
/// one it could not see, and "the warm path is fast" was an assertion nobody
/// could check from the tool's own output.
///
/// It is not a free-standing key either: the branch already spends measurable
/// time — it hashes every file in the tree to *prove* nothing changed, and it
/// runs `persist:vacuum` — so the absence was a reporting gap, not an empty
/// truth.
#[test]
fn an_unchanged_build_reports_its_own_timings() {
    let root = temp_root();
    let db = root.join("index.sqlite");
    let build = || {
        Command::new(env!("CARGO_BIN_EXE_devmap"))
            .args(["--json", "--db"])
            .arg(&db)
            .arg("build")
            .arg(&root)
            .output()
            .expect("run build")
    };

    let first = build();
    assert!(first.status.success(), "first build must succeed");

    let second = build();
    assert!(second.status.success(), "second build must succeed");
    let payload: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("--json must emit JSON");
    assert_eq!(
        payload["unchanged"],
        serde_json::Value::Bool(true),
        "the second build must take the unchanged path: {payload}"
    );

    let timings = payload
        .get("timings")
        .unwrap_or_else(|| panic!("an unchanged build must report timings: {payload}"));
    let total = timings["total_seconds"]
        .as_f64()
        .unwrap_or_else(|| panic!("total_seconds must be a number: {timings}"));
    assert!(
        total > 0.0,
        "a build that hashed every file cannot have taken zero time: {timings}"
    );
    let stages = timings["stages"]
        .as_array()
        .unwrap_or_else(|| panic!("stages must be an array: {timings}"));
    assert!(
        !stages.is_empty(),
        "the unchanged path runs discovery and a vacuum decision; both are stages: {timings}"
    );
    // The reclaim decision is the one stage a warm-path profiler is looking
    // for — K5 was a reclaim that reported success without doing anything.
    //
    // Searched through `sub` as well as the top level, because that is where it
    // legitimately lands: `persist:vacuum` is nested under the stage that ran
    // it, exactly as the full build nests its own `persist:*` entries.
    let names_vacuum = |stage: &serde_json::Value| {
        stage["stage"].as_str() == Some("persist:vacuum")
            || stage["sub"].as_array().is_some_and(|subs| {
                subs.iter()
                    .any(|entry| entry["stage"].as_str() == Some("persist:vacuum"))
            })
    };
    assert!(
        stages.iter().any(names_vacuum),
        "the vacuum decision must be a timed stage: {timings}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
