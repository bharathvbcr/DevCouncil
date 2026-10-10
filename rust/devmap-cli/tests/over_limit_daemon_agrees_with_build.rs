//! The daemon and the CLI handle a file that grows past `MAX_SOURCE_BYTES` the
//! same way: the rows go, the file is listed as over the limit, and nothing is
//! retried.
//!
//! The audit's reproduction (2026-10-09, `ts2`): with `devmap serve` running,
//! `src/a.ts` grew to 1,778,086 bytes. The daemon logged `changed source
//! "src/a.ts" is 1778086 bytes; limit is 1048576`, retried at 1, 2, 4, 8 and 16
//! seconds, quarantined the path, and `status` read `is_fresh: false` until a
//! manual `devmap repair --pending` — while `impact named1` went on serving the
//! pre-growth callers. The CLI build of the same edit dropped those rows and
//! called itself fresh. Two answers for one tree.
//!
//! Here because both halves are needed in one place: the `devmap` binary this
//! crate builds is the CLI path, and `devmap_serve::Daemon` is the daemon.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use devmap_serve::Daemon;
use devmap_store::Store;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn json(root: &Path, args: &[&str]) -> serde_json::Value {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("devmap {args:?}: {error}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "devmap {args:?} stdout is not JSON ({error}): {stdout}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-over-limit-daemon-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    // The daemon canonicalises its root; macOS `temp_dir()` is a symlink.
    root.canonicalize().unwrap()
}

const LIMIT: u64 = devmap_extract::MAX_SOURCE_BYTES;

fn caller_of_at_least(bytes: u64) -> String {
    let mut body = String::from("from lib import named1\n\n\ndef run():\n");
    while (body.len() as u64) < bytes {
        body.push_str("    named1()\n");
    }
    body
}

fn write_tree(root: &Path) {
    std::fs::write(root.join("lib.py"), "def named1():\n    return 1\n").unwrap();
    std::fs::write(root.join("a.py"), caller_of_at_least(4_000)).unwrap();
}

/// What `impact named1` says, reduced to the parts a reader acts on.
fn impact_answer(root: &Path) -> (u64, Vec<String>, bool) {
    let impact = json(root, &["--json", "impact", "named1"]);
    let mut sources: Vec<String> = impact["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["source_file"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    sources.sort();
    sources.dedup();
    let caveated = impact["walk_incomplete"]
        .as_str()
        .is_some_and(|reason| !reason.is_empty());
    (impact["total"].as_u64().unwrap_or(0), sources, caveated)
}

/// The same edit through both paths gives the same `impact` answer.
#[test]
fn the_cli_and_the_daemon_answer_impact_alike_after_a_file_crosses_the_limit() {
    let cli = scratch("cli");
    let daemon_root = scratch("daemon");
    for root in [&cli, &daemon_root] {
        write_tree(root);
        let build = json(root, &["--json", "build", "."]);
        assert!(build["generation_id"].as_u64().is_some(), "{build}");
        let (total, sources, _) = impact_answer(root);
        assert!(
            total > 0 && sources == vec!["a.py".to_string()],
            "precondition: a.py calls named1 while it is indexed ({total}, {sources:?})"
        );
    }

    // The edit, once per tree.
    for root in [&cli, &daemon_root] {
        std::fs::write(root.join("a.py"), caller_of_at_least(LIMIT + 700_000)).unwrap();
    }

    let build = json(&cli, &["--json", "build", "."]);
    assert_eq!(
        build["file_progress"]["delta"]["skipped_over_limit"], 1,
        "{build}"
    );

    let store_path = devmap_extract::paths::store_path(&daemon_root);
    let daemon = Daemon::new(Store::open(&store_path).unwrap(), daemon_root.clone());
    let reader = Store::open(&store_path).unwrap();
    reader
        .enqueue_pending_paths_under_root(&daemon_root, &["a.py".to_string()])
        .unwrap();
    daemon
        .drain_pending_batch()
        .expect("an over-limit path is a structural skip, not a failure");
    let status = reader.status("").unwrap();
    assert_eq!(status.pending_count, 0, "settled in one drain");
    assert_eq!(status.quarantined_count, 0, "and never charged a retry");

    let cli_answer = impact_answer(&cli);
    let daemon_answer = impact_answer(&daemon_root);
    assert_eq!(
        cli_answer, daemon_answer,
        "the CLI and the daemon must agree about an over-limit file's rows"
    );
    assert_eq!(
        cli_answer.1,
        Vec::<String>::new(),
        "both drop the rows a cold build of the grown tree would not write"
    );
    assert!(
        cli_answer.2,
        "and both caveat the empty answer, because the caller is unread"
    );

    for root in [&cli, &daemon_root] {
        let status = json(root, &["--json", "status"]);
        assert_eq!(status["is_fresh"], true, "{status}");
        assert_eq!(
            status["coverage_gaps"]["over_limit"]["total"], 1,
            "{status}"
        );
        assert_eq!(status["quarantined_count"], 0, "{status}");
    }

    let _ = std::fs::remove_dir_all(&cli);
    let _ = std::fs::remove_dir_all(&daemon_root);
}

/// A watched file grown past the limit settles: nothing pending, nothing
/// quarantined, inside a bound well short of the retry ladder (1+2+4+8+16 s).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watched_file_growing_past_the_limit_settles_without_retries() {
    let root = scratch("watched");
    write_tree(&root);
    let build = json(&root, &["--json", "build", "."]);
    assert!(build["generation_id"].as_u64().is_some(), "{build}");

    let store_path = devmap_extract::paths::store_path(&root);
    // Short and outside the tree: a Unix socket path is capped at 100 bytes
    // portably, and macOS `temp_dir()` alone spends most of that.
    let socket = PathBuf::from("/tmp").join(format!("devmap-ol-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let daemon = Daemon::new(Store::open(&store_path).unwrap(), root.clone())
        .with_store_path(store_path.clone())
        .with_ipc_path(socket)
        .with_idle_poll(Duration::from_millis(20))
        .with_max_idle(None);
    let handle = daemon.clone();
    let mut running = tokio::spawn(async move { daemon.run_loop().await });
    // Biased toward the endpoint, and racing the task: a `run_loop` that
    // returns an error never serves, and a bare `wait_until_serving` would
    // wait on it forever instead of saying why.
    tokio::time::timeout(Duration::from_secs(60), async {
        tokio::select! {
            biased;
            () = handle.wait_until_serving() => {}
            ended = &mut running => panic!("the daemon stopped before serving: {ended:?}"),
        }
    })
    .await
    .expect("run_loop neither served nor returned within 60 s");

    let reader = Store::open(&store_path).unwrap();
    let generation_before = reader.latest_generation_id().unwrap();
    std::fs::write(root.join("a.py"), caller_of_at_least(LIMIT + 700_000)).unwrap();

    // Settled means: a new generation lists the file as over the limit and the
    // queue is empty. The retry ladder alone is 31 s, so a 20 s bound fails a
    // path that is being retried rather than skipped.
    let bound = Duration::from_secs(20);
    let started = Instant::now();
    let settled = loop {
        let inventory = reader.latest_discovery_inventory().unwrap();
        let status = reader.status("").unwrap();
        assert_eq!(
            status.quarantined_count, 0,
            "an over-limit path must never be quarantined"
        );
        if reader.latest_generation_id().unwrap() != generation_before
            && inventory.over_limit.iter().any(|row| row.path == "a.py")
            && status.pending_count == 0
        {
            break true;
        }
        if started.elapsed() > bound {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let status = reader.status("").unwrap();
    handle.request_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(30), running).await;
    assert!(
        settled,
        "the drain did not settle the over-limit file within {bound:?}: pending {}, \
         quarantined {}",
        status.pending_count, status.quarantined_count
    );
    assert_eq!(status.pending_count, 0);
    assert_eq!(status.quarantined_count, 0);

    let _ = std::fs::remove_dir_all(&root);
}
