//! `build_history.build_ms` means one thing: the time a build held the store's
//! writer lock, from acquiring it to the end of the vacuum.
//!
//! Before, the CLI started the clock before `writer:wait` (so queueing behind
//! another writer was counted as building) while the daemon started it after
//! the lock; and both stopped it inside the save transaction, before the
//! commit, the generation and extraction prunes and the vacuum, so up to 8 s of
//! a starved build was missing from the number. Each test below fails on that
//! code: the first by the lock wait that was counted, the second by the work
//! after the save that was not.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use devmap_store::Store;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn fixture(name: &str, files: usize) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("devmap-build-ms-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    for i in 0..files {
        write_module(&root, i, 0);
    }
    let db = root.join("index.sqlite");
    (root, db)
}

fn write_module(root: &Path, index: usize, revision: usize) {
    let mut source = String::new();
    for f in 0..8 {
        source.push_str(&format!(
            "def f{index}_{f}_r{revision}(x):\n    return g{index}_{f}(x) + {f}\n\n\
             def g{index}_{f}(x):\n    return x * {index}\n\n"
        ));
    }
    std::fs::write(root.join(format!("m{index:04}.py")), source).unwrap();
}

fn build_args(db: &Path) -> Vec<std::ffi::OsString> {
    vec!["--db".into(), db.into(), "--json".into(), "build".into(), ".".into()]
}

fn parse(stdout: &[u8], stderr: &[u8]) -> serde_json::Value {
    let text = String::from_utf8_lossy(stdout);
    serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {text}\n{}",
            String::from_utf8_lossy(stderr)
        )
    })
}

fn run_build(root: &Path, db: &Path) -> serde_json::Value {
    let out = Command::new(devmap())
        .args(build_args(db))
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    parse(&out.stdout, &out.stderr)
}

/// Seconds of the first stage named `wanted`, at any nesting.
fn stage_seconds(stages: &[serde_json::Value], wanted: &str) -> Option<f64> {
    stages.iter().find_map(|stage| {
        if stage["stage"] == wanted {
            stage["seconds"].as_f64()
        } else {
            stage["sub"]
                .as_array()
                .and_then(|nested| stage_seconds(nested, wanted))
        }
    })
}

fn newest_build_ms(root: &Path, db: &Path) -> u64 {
    let out = Command::new(devmap())
        .arg("--db")
        .arg(db)
        .args(["--json", "history", "--last", "1"])
        .current_dir(root)
        .output()
        .unwrap();
    let history = parse(&out.stdout, &out.stderr);
    history["history"][0]["build_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("newest build has no build_ms: {history}"))
}

#[test]
fn time_spent_waiting_for_the_writer_lock_is_not_build_time() {
    let (root, db) = fixture("wait", 4);
    run_build(&root, &db);
    write_module(&root, 0, 1);

    // Hold the lock from a second handle for a while; the build queues behind
    // it, then does ordinary work.
    let guard = Store::lock_writer_at(&db, Duration::from_secs(5)).unwrap();
    let child = Command::new(devmap())
        .args(build_args(&db))
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(4000));
    drop(guard);
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let build = parse(&out.stdout, &out.stderr);

    let stages = build["timings"]["stages"].as_array().unwrap();
    let waited = stage_seconds(stages, "writer:wait").expect("writer:wait is a timed stage");
    let total = build["timings"]["total_seconds"].as_f64().unwrap();
    assert!(waited >= 2.5, "precondition: the build queued {waited:.2} s");

    let build_ms = newest_build_ms(&root, &db) as f64;
    println!(
        "waited {waited:.3} s, total {total:.3} s, build_ms {build_ms:.0}, \
         margin {:.0} ms",
        (total * 1000.0 - 1500.0) - build_ms
    );
    assert!(
        build_ms < total * 1000.0 - 1500.0,
        "build_ms {build_ms} includes the {waited:.2} s spent waiting for the lock \
         (total {total:.2} s)"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn build_ms_covers_the_commit_the_prunes_and_the_vacuum() {
    // Enough content, over enough generations, that the commit and the prunes
    // are measurable.
    let (root, db) = fixture("end", 1500);
    run_build(&root, &db);
    for revision in 1..=3 {
        for index in 0..150 {
            write_module(&root, index, revision);
        }
        run_build(&root, &db);
    }
    for index in 0..150 {
        write_module(&root, index, 9);
    }
    let build = run_build(&root, &db);

    let stages = build["timings"]["stages"].as_array().unwrap();
    let waited = stage_seconds(stages, "writer:wait").unwrap_or(0.0);
    let after_save: f64 = ["persist:prune_generations", "persist:prune_extractions", "persist:vacuum"]
        .iter()
        .map(|name| {
            stage_seconds(stages, name).unwrap_or_else(|| panic!("no {name} stage: {build}"))
        })
        .sum();
    // Everything up to the end of the vacuum: the stages from the first
    // through the one holding the vacuum, minus the lock wait, minus whatever
    // that last stage did that no timed sub-phase accounts for (work after the
    // vacuum, which `build_ms` rightly omits; work before it that no sub-phase
    // names is dropped too, which only loosens the bound).
    let vacuum_stage = stages
        .iter()
        .position(|stage| {
            stage["sub"]
                .as_array()
                .is_some_and(|subs| stage_seconds(subs, "persist:vacuum").is_some())
        })
        .unwrap_or_else(|| panic!("precondition: some stage holds persist:vacuum: {build}"));
    let holder = &stages[vacuum_stage];
    let holder_subs: f64 = holder["sub"]
        .as_array()
        .map(|subs| subs.iter().filter_map(|s| s["seconds"].as_f64()).sum())
        .unwrap_or(0.0);
    let tail = holder["seconds"].as_f64().unwrap() - holder_subs;
    let through_vacuum = stages[..=vacuum_stage]
        .iter()
        .filter_map(|stage| stage["seconds"].as_f64())
        .sum::<f64>()
        - waited
        - tail.max(0.0);

    let build_ms = newest_build_ms(&root, &db) as f64;
    println!(
        "through_vacuum {:.0} ms, build_ms {build_ms:.0}, after-save work {:.0} ms, \
         margin {:.0} ms",
        through_vacuum * 1000.0,
        after_save * 1000.0,
        build_ms - through_vacuum * 1000.0
    );
    assert!(
        build_ms >= through_vacuum * 1000.0 - 10.0,
        "build_ms {build_ms:.0} stops before the end of the vacuum ({:.0} ms), missing the \
         {:.0} ms the commit, prunes and vacuum took",
        through_vacuum * 1000.0,
        after_save * 1000.0
    );
    let _ = std::fs::remove_dir_all(&root);
}
