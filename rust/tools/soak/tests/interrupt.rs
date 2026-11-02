//! The real `soak` binary against a stand-in `devmap`, interrupted mid-build.
//!
//! A measured build leads a process group of its own (so a timeout can kill
//! the `/usr/bin/time` wrapper and the build under it together), which means a
//! terminal's Ctrl-C never reaches it. The soak must forward the interrupt, or
//! an interrupted soak leaves a build running against the store it just
//! restored the corpus under.

#![cfg(unix)]

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use common::{fixture, soak};
use dc_proc::Signal;

fn read_pid(path: &Path, within: Duration) -> Option<u32> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(pid) = fs::read_to_string(path)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
        {
            return Some(pid);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Whether `pid` is gone: no such process, or a zombie awaiting its reaper.
fn gone(pid: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if matches!(dc_proc::signal_process(pid, Signal::Probe),
            Err(e) if dc_proc::target_is_gone(&e))
        {
            return true;
        }
        let state = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default();
        if state.is_empty() || state.starts_with('Z') {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn interrupt_a_running_soak(signal: &str, name: &str, expected_exit: i32) {
    // The stand-in build records the pid of a long sleeper and waits on it.
    let (root, stub, state) = fixture(
        name,
        r#"case "$1" in build) sleep 31 & echo $! > "$STATE/child"; wait ;; esac; exit 1"#,
    );
    let mut child = soak(&root, &stub).spawn().expect("the soak starts");
    let sleeper = read_pid(&state.join("child"), Duration::from_secs(20));
    // The interrupt handler registers on its own thread; the build is already
    // running by now, so give the registration a moment.
    thread::sleep(Duration::from_millis(500));
    let _ = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        if let Some(pid) = sleeper {
            let _ = dc_proc::signal_process(pid, Signal::Kill);
        }
        panic!("the soak did not exit within 20 s of {signal}");
    };
    let sleeper = sleeper.expect("the stand-in build never started");
    let survived = !gone(sleeper, Duration::from_secs(3));
    // Whatever the verdict, never leave the sleeper behind.
    let _ = dc_proc::signal_process(sleeper, Signal::Kill);
    assert_eq!(status.code(), Some(expected_exit), "{status:?}");
    assert!(
        !survived,
        "the soak interrupted by {signal} left its build's process {sleeper} running"
    );
    let _ = fs::remove_dir_all(root.parent().expect("base"));
}

#[test]
fn an_interrupted_soak_does_not_leave_its_build_running() {
    interrupt_a_running_soak("-INT", "int", 130);
}

#[test]
fn a_terminated_soak_does_not_leave_its_build_running() {
    interrupt_a_running_soak("-TERM", "term", 143);
}
