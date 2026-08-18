#![cfg(unix)]

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

fn wait_for_path(path: &std::path::Path) {
    for _ in 0..200 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {path:?}");
}

#[test]
fn kill9_restart_replays_durable_pending_work_and_replaces_stale_socket() {
    let root = temp_root();
    let db = root.join(".devcouncil/codeintel/index.sqlite");
    let socket = std::path::PathBuf::from(format!(
        "/tmp/devmap-pr-{}-{}.sock",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
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
    wait_for_path(&socket);
    std::thread::sleep(Duration::from_millis(250));
    first.kill().expect("kill first daemon");
    first.wait().expect("reap first daemon");
    assert!(
        socket.exists(),
        "SIGKILL should leave a stale socket fixture"
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
    wait_for_path(&socket);

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

    let _ = fs::remove_file(socket);
    fs::remove_dir_all(root).expect("remove fixture tree");
}
