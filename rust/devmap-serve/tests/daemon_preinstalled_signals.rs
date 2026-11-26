//! A stop signal that arrives while the caller is still opening the store is not
//! lost: the streams installed before the open are the daemon's own, so the
//! signal is pending when `run_loop` first looks. A fresh subscription made
//! after the signal would never see it (tokio does not replay one).
#![cfg(unix)]

use std::process::Command;
use std::time::Duration;

use devmap_serve::{Daemon, ShutdownSignals};
use devmap_store::Store;

#[tokio::test]
async fn a_stop_signal_sent_before_the_loop_starts_ends_the_loop() {
    let dir = std::env::temp_dir().join(format!("daemon-preinstalled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let db = dir.join("devmap.sqlite");

    let signals = ShutdownSignals::install().expect("signals install");
    let store = Store::open(&db).unwrap();
    // The signal lands after the streams exist and before the daemon runs.
    let sent = Command::new("kill")
        .args(["-TERM", &std::process::id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    // tokio only marks the signal pending in the handler; the driver delivers it
    // to listeners when it runs. Let it run now, so a subscription made after
    // this point would miss the signal.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Premise: a fresh subscriber made now does not see the signal.
    let mut fresh = ShutdownSignals::install().expect("fresh signals install");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), fresh.recv())
            .await
            .is_err(),
        "a new subscription saw the earlier signal, so the test proves nothing"
    );
    drop(fresh);

    let daemon = Daemon::new(store, dir.clone())
        .with_store_path(db.clone())
        .with_ipc_path(dir.join("d.sock"))
        .with_shutdown_signals(signals);
    let outcome = tokio::time::timeout(Duration::from_secs(30), daemon.run_loop()).await;
    let _ = std::fs::remove_dir_all(&dir);
    outcome
        .expect("the stop signal sent before run_loop was lost")
        .expect("an orderly stop is not an error");
}
