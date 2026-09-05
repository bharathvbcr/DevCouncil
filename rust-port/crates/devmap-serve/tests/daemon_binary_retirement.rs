//! The one daemon exit path that cannot be tested beside the others.
//!
//! `run_loop` retires when the executable backing the process is replaced on
//! disk — `(size, mtime)` of `std::env::current_exe()`. Exercising that branch
//! end to end means moving the *running test binary's* mtime, and every other
//! daemon under test in the same process reads the same executable: doing it in
//! the unit-test module would retire them all mid-assertion. A test file with a
//! single test is its own process, so the side effect reaches nothing else.
//!
//! What is being pinned is the release, not the predicate. K-A5: two of
//! `run_loop`'s six exits — this one and the idle retirement — used to `return`
//! straight out, skipping `release_ipc_endpoint`, so the orderly path left the
//! same stale socket and held advisory lock a `kill -9` leaves and the next
//! client was told "devmap IPC endpoint is already active". The idle exit is
//! covered by `idle_retirement_releases_the_endpoint_before_run_loop_returns`;
//! this covers the other one.

#![cfg(unix)]

use std::path::Path;
use std::time::Duration;

use devmap_serve::Daemon;
use devmap_store::Store;

/// Move a file's modification time forward, the way a rebuild does.
///
/// Opened read-only: a running executable cannot be opened for write
/// (`ETXTBSY`), and `futimens` with explicit times needs ownership of the file
/// rather than write permission on it.
fn advance_modification_time(path: &Path) {
    let file = std::fs::File::open(path).expect("the running executable must be readable");
    let modified = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .expect("the running executable must have a modification time");
    let later = modified + Duration::from_secs(5);
    let times = std::fs::FileTimes::new()
        .set_accessed(later)
        .set_modified(later);
    file.set_times(times)
        .expect("the test process owns its own executable");
}

async fn wait_for(path: &Path, exists: bool) -> bool {
    for _ in 0..500 {
        if path.exists() == exists {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    path.exists() == exists
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_binary_retires_the_daemon_and_releases_its_endpoint() {
    let root =
        std::path::PathBuf::from("/tmp").join(format!("devmap-binret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.py"), "def main(): pass\n").unwrap();
    let socket = root.join("d.sock");

    // Idle retirement disabled, so the only exit available is the one under
    // test. Without that, a slow machine could retire the daemon for the wrong
    // reason and the test would pass having proved nothing.
    let daemon = Daemon::new(Store::open_in_memory().unwrap(), root.clone())
        .with_ipc_path(socket.clone())
        .with_idle_poll(Duration::from_millis(20))
        .with_max_idle(None);
    let serving = tokio::spawn(async move { daemon.run_loop().await });

    assert!(
        wait_for(&socket, true).await,
        "the daemon must bind its endpoint at {} before the binary changes",
        socket.display()
    );

    advance_modification_time(&std::env::current_exe().unwrap());

    let outcome = tokio::time::timeout(Duration::from_secs(30), serving)
        .await
        .expect("a daemon whose binary was replaced must retire")
        .expect("the daemon task must not panic");
    // NO `.await` between the line above and the socket assertion: the point of
    // the release is that the endpoint is gone by the time `run_loop` resolves,
    // and yielding here would let a merely-scheduled cleanup pass for one that
    // was awaited.
    assert!(
        outcome.is_ok(),
        "binary-change retirement reported: {outcome:?}"
    );
    assert!(
        !socket.exists(),
        "the daemon retired with its socket still at {} — the endpoint must be \
         released before `run_loop` resolves, or the next client is told the \
         endpoint is already active",
        socket.display()
    );

    // The advisory lock beside the socket is the half a stale-socket check
    // cannot see: `UnixIpcServer::bind` removes an unanswered socket by itself,
    // but a lock still held by this process refuses the bind outright. Binding
    // a fresh daemon on the same path is therefore the assertion that the whole
    // endpoint — socket and lock — was released.
    let successor = Daemon::new(Store::open_in_memory().unwrap(), root.clone())
        .with_ipc_path(socket.clone())
        .with_idle_poll(Duration::from_millis(20))
        .with_max_idle(Some(Duration::from_millis(50)));
    let second = tokio::time::timeout(Duration::from_secs(30), successor.run_loop())
        .await
        .expect("a successor daemon must be able to bind the released endpoint");
    assert!(
        second.is_ok(),
        "the released endpoint must be immediately rebindable: {second:?}"
    );
    assert!(
        wait_for(&socket, false).await,
        "and the successor's own orderly exit must release it again"
    );

    let _ = std::fs::remove_dir_all(&root);
}
