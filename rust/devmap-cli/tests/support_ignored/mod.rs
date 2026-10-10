//! A watcher-driven daemon over a scratch repository, for asserting that a
//! path the walk excludes is excluded by the watcher too.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use devmap_serve::Daemon;
use devmap_store::Store;

pub fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

pub fn json(root: &Path, args: &[&str]) -> serde_json::Value {
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

pub fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "devmap-ignored-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    root.canonicalize().unwrap()
}

/// Build `root`, run a watching daemon over it, create `excluded` (which the
/// rules exclude) and then `control` (which they do not), and return once the
/// control has been drained — proof the watcher was live — or panic.
///
/// Asserts along the way that `excluded` is never queued, never in a
/// generation, and that `status` reads fresh once the control has drained.
pub async fn excluded_path_stays_out(root: &Path, excluded: &str, control: &str) {
    let build = json(root, &["--json", "build", "."]);
    assert!(build["generation_id"].as_u64().is_some(), "{build}");
    let walked: Vec<String> = devmap_extract::collect_sources_with_report(root)
        .unwrap()
        .0
        .into_iter()
        .map(|(path, _)| path)
        .collect();

    let store_path = devmap_extract::paths::store_path(root);
    let socket = PathBuf::from("/tmp").join(format!(
        "devmap-ig-{}-{}.sock",
        std::process::id(),
        excluded.len()
    ));
    let _ = std::fs::remove_file(&socket);
    let daemon = Daemon::new(Store::open(&store_path).unwrap(), root.to_path_buf())
        .with_store_path(store_path.clone())
        .with_ipc_path(socket)
        .with_idle_poll(Duration::from_millis(20))
        .with_max_idle(None);
    let handle = daemon.clone();
    let mut running = tokio::spawn(async move { daemon.run_loop().await });
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
    // Let the connect-time sweep settle before the edits under test.
    let settle = Instant::now();
    while reader.status("").unwrap().pending_count != 0
        && settle.elapsed() < Duration::from_secs(20)
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    std::fs::write(root.join(excluded), "export const excluded = 1;\n").unwrap();
    assert!(
        !devmap_extract::collect_sources_with_report(root)
            .unwrap()
            .0
            .iter()
            .any(|(path, _)| path == excluded),
        "precondition: the cold walk excludes {excluded} (walked before: {walked:?})"
    );
    // Long enough for the watcher's debounce to have delivered it, sampled so
    // a queued row is seen before a drain can clear it.
    let watch = Instant::now();
    while watch.elapsed() < Duration::from_secs(2) {
        assert!(
            !reader
                .get_pending_paths()
                .unwrap()
                .iter()
                .any(|path| path.ends_with(excluded)),
            "{excluded} is excluded by the walk and must not be enqueued"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Positive control: a path the rules admit is drained, so the watcher was
    // live while the excluded one was ignored.
    std::fs::write(root.join(control), "export const control = 1;\n").unwrap();
    let bound = Duration::from_secs(20);
    let started = Instant::now();
    let drained = loop {
        let indexed = reader.latest_file_hashes().unwrap().contains_key(control);
        if indexed && reader.status("").unwrap().pending_count == 0 {
            break true;
        }
        if started.elapsed() > bound {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let status = reader.status("").unwrap();
    let hashes = reader.latest_file_hashes().unwrap();
    handle.request_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(30), running).await;
    assert!(
        drained,
        "the control edit to {control} was never drained within {bound:?}"
    );
    assert!(
        !hashes.contains_key(excluded),
        "{excluded} must not be in the graph: {:?}",
        hashes.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        status.source_freshness,
        Some(true),
        "the index matches the tree the walk sees, so status is fresh: {:?}",
        status.degraded_reason
    );
    let cli_status = json(root, &["--json", "status"]);
    assert_eq!(cli_status["is_fresh"], true, "{cli_status}");
}
