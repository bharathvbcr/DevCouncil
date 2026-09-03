//! `devmap serve --print-socket-path` must name the endpoint a daemon binds.
//!
//! The kernel derived its socket from the root string as written while the
//! Python client derived one of its own, so a bare `devmap serve <root>` and a
//! client probe could end up on two different paths — two daemons per
//! repository, each indexing the same tree, with nothing on either side that
//! would show the split. The formula is now stated (FNV-1a 64 over the
//! canonical root, `<temp>/devmap-<16 hex>/ipc.sock`) and printable without
//! starting anything, so a second implementation can be checked against it.

use devmap_serve::{default_ipc_path_for, Daemon};
use devmap_store::Store;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("devmap-sockpath-{name}-{stamp}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The printed path is the path a daemon binds, and printing it creates
/// nothing.
#[tokio::test]
#[cfg(unix)]
async fn the_printed_socket_path_is_the_one_the_daemon_binds() {
    use std::os::unix::fs::PermissionsExt;

    let root = scratch("printed");
    let db = root.join("devmap.sqlite");

    // The endpoint directory must not exist yet, so "printing created nothing"
    // is a claim this test can actually check.
    let expected = default_ipc_path_for(&root);
    let endpoint_dir = expected.parent().expect("endpoint directory").to_path_buf();
    let _ = std::fs::remove_dir_all(&endpoint_dir);

    let printed = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--db")
        .arg(&db)
        .arg("serve")
        .arg("--print-socket-path")
        .arg(&root)
        .output()
        .expect("devmap serve --print-socket-path must run");
    assert!(
        printed.status.success(),
        "--print-socket-path exited {:?}: {}",
        printed.status.code(),
        String::from_utf8_lossy(&printed.stderr)
    );
    let announced = PathBuf::from(String::from_utf8_lossy(&printed.stdout).trim().to_string());
    assert!(
        !announced.as_os_str().is_empty(),
        "nothing was printed: stderr={}",
        String::from_utf8_lossy(&printed.stderr)
    );

    assert!(
        !endpoint_dir.exists(),
        "printing the socket path created {}",
        endpoint_dir.display()
    );
    assert!(
        !db.exists(),
        "printing the socket path created the store at {}",
        db.display()
    );

    // Now bind it for real and check the daemon lands on the same path.
    let daemon = Daemon::new(Store::open(&db).unwrap(), root.canonicalize().unwrap())
        .with_store_path(db.clone())
        .with_idle_poll(Duration::from_millis(20));
    let running = daemon.clone();
    let task = tokio::spawn(async move { running.run_loop().await });

    for _ in 0..300 {
        if announced.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        announced.exists(),
        "the daemon did not bind the announced endpoint {}",
        announced.display()
    );

    // The directory the socket sits in is owner-only, so the socket is not
    // merely 0600 itself but unreachable to anyone else on the machine.
    let mode = std::fs::metadata(&endpoint_dir)
        .expect("endpoint directory")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o700,
        "the endpoint directory is mode {mode:o}, not 0700"
    );

    daemon.request_shutdown();
    let outcome = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the daemon must shut down")
        .expect("the run loop must not panic");
    assert!(outcome.is_ok(), "shutdown failed: {outcome:?}");
    assert!(
        !announced.exists(),
        "the daemon left its socket behind after shutdown"
    );

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&endpoint_dir);
}

/// The same repository, named three ways, is one endpoint.
#[test]
#[cfg(unix)]
fn every_spelling_of_one_root_prints_one_endpoint() {
    let base = scratch("spellings");
    let real = base.join("repo");
    std::fs::create_dir_all(&real).unwrap();
    let link = base.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let print = |root: &std::path::Path| -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
            .arg("serve")
            .arg("--print-socket-path")
            .arg(root)
            .current_dir(&real)
            .output()
            .expect("devmap serve --print-socket-path must run");
        assert!(output.status.success(), "exited {:?}", output.status.code());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    let direct = print(&real);
    assert!(!direct.is_empty());
    assert_eq!(
        direct,
        print(&link),
        "a symlinked root printed a different endpoint, so one repository would \
         be served by two daemons"
    );
    assert_eq!(
        direct,
        print(std::path::Path::new(".")),
        "`.` printed a different endpoint than the directory it names"
    );

    let _ = std::fs::remove_dir_all(&base);
}
