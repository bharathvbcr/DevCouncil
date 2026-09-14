use std::fs;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use devmap_store::Store;

/// Unique per call, not merely per instant: `SystemTime` ticks every 1 us here,
/// so same-microsecond callers would otherwise share one fixture directory.
fn temp_root() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-process-recovery-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join(".devcouncil/codeintel")).expect("create fixture tree");
    root
}

/// Wait for a spawned daemon's endpoint to answer, or for the daemon to exit
/// without one.
///
/// `child` is not decoration. These daemons are separate processes, so the
/// in-process endpoint signal `devmap-serve`'s own tests use cannot reach them
/// and a poll is the only instrument available — but a poll that never asks
/// whether the process is still there reports "timed out waiting for the
/// socket" about a daemon that exited immediately and said why, which is the
/// clock taking the blame for a cause that was already known.
/// `daemon_storm_soak.rs::wait_until_bound` asks the same question; this is the
/// call site that did not.
fn wait_for_path(path: &std::path::Path, child: &mut std::process::Child) {
    for _ in 0..200 {
        if let Ok(Some(status)) = child.try_wait() {
            panic!("the daemon exited {status} instead of binding {path:?}");
        }
        #[cfg(unix)]
        let ready = std::os::unix::net::UnixStream::connect(path).is_ok();
        #[cfg(windows)]
        let ready = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .is_ok();
        if ready {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {path:?}");
}

#[test]
fn process_death_restart_replays_durable_pending_work_and_reclaims_the_endpoint() {
    let root = temp_root();
    let db = root.join(".devcouncil/codeintel/index.sqlite");
    let socket = devmap_serve::default_ipc_path_for(&root);
    let pending = "recover.py".to_string();
    fs::write(root.join(&pending), vec![b'x'; 1024 * 1024 + 1])
        .expect("create temporarily oversized source");
    Store::open(&db)
        .expect("open store")
        .enqueue_pending_paths(std::slice::from_ref(&pending))
        .expect("enqueue durable work");

    let binary = env!("CARGO_BIN_EXE_devmap");
    let mut first = Command::new(binary)
        .args(["--db"])
        .arg(&db)
        .arg("serve")
        .arg(&root)
        .arg("--socket")
        .arg(&socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start first daemon");
    wait_for_path(&socket, &mut first);
    std::thread::sleep(Duration::from_millis(250));
    first.kill().expect("kill first daemon");
    first.wait().expect("reap first daemon");
    #[cfg(unix)]
    assert!(
        socket.exists(),
        "SIGKILL should leave a stale socket fixture"
    );
    #[cfg(windows)]
    assert!(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&socket)
            .is_err(),
        "process death must release the named pipe"
    );
    assert_eq!(
        Store::open(&db)
            .unwrap()
            .status("test")
            .unwrap()
            .pending_count,
        1,
        "unacknowledged work must survive process death"
    );

    fs::write(
        root.join(&pending),
        "def recovered_symbol():\n    return 1\n",
    )
    .expect("make pending path recoverable");
    let mut second = Command::new(binary)
        .args(["--db"])
        .arg(&db)
        .arg("serve")
        .arg(&root)
        .arg("--socket")
        .arg(&socket)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("restart daemon");
    wait_for_path(&socket, &mut second);

    let observer = Store::open(&db).unwrap();
    let mut replayed = false;
    for _ in 0..200 {
        if observer.status("test").unwrap().pending_count == 0 {
            replayed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    second.kill().expect("stop restarted daemon");
    second.wait().expect("reap restarted daemon");
    assert!(
        replayed,
        "restarted daemon did not acknowledge replayed work"
    );
    let persisted = observer.latest_extractions().unwrap();
    assert!(persisted.iter().any(|extraction| {
        extraction.file_path == pending
            && extraction
                .symbols
                .iter()
                .any(|symbol| symbol.name == "recovered_symbol")
    }));

    drop(observer);
    let _ = fs::remove_file(socket);
    fs::remove_dir_all(root).expect("remove fixture tree");
}
