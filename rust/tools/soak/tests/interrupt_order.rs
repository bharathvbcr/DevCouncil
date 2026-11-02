//! An interrupt must win against the main thread it wakes.
//!
//! The interrupt kills the measured build, and that kill is what wakes the
//! main thread: its build returns an error, it reports the failure and heads
//! for its own cleanup and `exit(1)`. If it gets there first, an interrupted
//! soak exits 1 with "SOAK FAIL: initial build" instead of 130/143. These
//! tests stand in for both threads and hold the interrupt side's cleanup lock
//! open for a second after the kill, so the main thread gets every chance to
//! overtake it.

#![cfg(unix)]

use std::io::Write;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use devmap_soak::rss::{self, Builds};
use devmap_soak::{stop_for_interrupt, Cleanup, QuietAfterInterrupt};

#[test]
fn the_woken_main_thread_neither_reports_a_failure_nor_reaches_cleanup_first() {
    let builds: &'static Builds = Box::leak(Box::new(Builds::new()));
    let cleanup = Arc::new(Mutex::new(Cleanup::default()));
    let dir = std::env::temp_dir().join(format!("soak-interrupt-order-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let log = dir.join("time.log");
    let flag = rss::time_flag().expect("this host has /usr/bin/time");

    // The main thread: a measured build only the interrupt can end, then the
    // failure line, then its cleanup.
    let (report, reported) = mpsc::channel();
    let main = {
        let cleanup = Arc::clone(&cleanup);
        let log = log.clone();
        thread::spawn(move || {
            let mut out = QuietAfterInterrupt {
                inner: Vec::new(),
                builds,
            };
            let build = rss::measure_in(
                builds,
                flag,
                &log,
                Path::new("/bin/sleep"),
                &["30"],
                Duration::from_secs(60),
            );
            let _ = writeln!(out, "SOAK FAIL: initial build");
            let reached_cleanup = {
                let _held = cleanup.lock().unwrap_or_else(|e| e.into_inner());
                Instant::now()
            };
            let _ = report.send((build.is_err(), out.inner, reached_cleanup));
        })
    };

    // Let the build start (the log is created just before the spawn).
    let deadline = Instant::now() + Duration::from_secs(10);
    while !log.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    thread::sleep(Duration::from_millis(200));

    // The interrupt thread: stop, then take a second before exiting.
    let started = Instant::now();
    let held = stop_for_interrupt(&cleanup, builds, &mut Vec::new());
    thread::sleep(Duration::from_secs(1));
    let released = Instant::now();
    drop(held);

    let (failed, said, reached_cleanup) = reported
        .recv_timeout(Duration::from_secs(30))
        .expect("the killed build woke the main thread");
    main.join().expect("main thread");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the interrupt did not end the 30 s build"
    );
    assert!(failed, "the interrupted build reported success");
    assert!(
        reached_cleanup >= released,
        "the main thread reached its cleanup (and its exit 1) while the interrupt still held it"
    );
    assert!(
        said.is_empty(),
        "an interrupted soak reported a failure: {}",
        String::from_utf8_lossy(&said)
    );
}

#[test]
fn output_passes_until_the_interrupt_and_is_dropped_after() {
    let builds = Builds::new();
    let mut out = QuietAfterInterrupt {
        inner: Vec::new(),
        builds: &builds,
    };
    writeln!(out, "soak churn file: a.ts").expect("write");
    builds.interrupt();
    writeln!(out, "SOAK FAIL: build (dirty) cycle 3").expect("write");
    assert_eq!(
        String::from_utf8_lossy(&out.inner),
        "soak churn file: a.ts\n"
    );
}
