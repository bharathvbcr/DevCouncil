//! The daemon shutdown, exercised against real processes in a real owned
//! process group under the real `/usr/bin/time`.
//!
//! The stand-in daemon is a shell that records its pid and then handles TERM
//! one of three ways: exits 0 (graceful), exits 7 (failure), or ignores it.
//!
//! One case the shell harness's Python tests carried is retired rather than
//! ported: "cleanup before session creation terminates the launcher". The
//! shell started the wrapper through a `perl … setsid` launcher, so for a
//! moment a live launcher existed outside any owned group. The wrapper is now
//! spawned with `process_group(0)`, which takes effect before `exec`; there is
//! no such moment, and `the_group_exists_from_the_moment_of_spawn` pins that.

#![cfg(unix)]

use std::fs::{self, File};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use dc_proc::Signal;
use devmap_soak::corpus::Corpus;
use devmap_soak::daemon::{Daemon, Identity, Os, Signals};
use devmap_soak::Cleanup;

const GRACE: Duration = Duration::from_secs(1);

struct Fixture {
    dir: PathBuf,
    daemon: Option<Daemon>,
    leader: u32,
    sentinel: Child,
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soak-shutdown-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn require_tools() {
    assert!(
        Path::new("/usr/bin/time").exists(),
        "/usr/bin/time is required by the soak; install the 'time' package"
    );
    assert!(
        Command::new("pgrep")
            .arg("-x")
            .arg("no-such-process-name")
            .status()
            .is_ok(),
        "pgrep is required by the soak"
    );
}

/// Start the stand-in daemon under `/usr/bin/time -p`, leading its own group.
fn start(name: &str, on_term: &str, signals: Box<dyn Signals>) -> Fixture {
    require_tools();
    let dir = scratch(name);
    let pidfile = dir.join("pid");
    let script = format!(
        "trap '{on_term}' TERM; echo $$ > '{}'; while :; do sleep 0.05; done",
        pidfile.display()
    );
    let wrapper = Command::new("/usr/bin/time")
        .args(["-p", "/bin/sh", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(File::create(dir.join("time.log")).expect("log"))
        .process_group(0)
        .spawn()
        .expect("the wrapper starts");
    let leader = wrapper.id();
    let daemon = Daemon::adopt(wrapper, GRACE, signals);
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(&pidfile).map_or(true, |s| s.trim().is_empty()) {
        assert!(
            Instant::now() < deadline,
            "the stand-in daemon never started"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let sentinel = Command::new("sleep").arg("30").spawn().expect("sentinel");
    Fixture {
        dir,
        daemon: Some(daemon),
        leader,
        sentinel,
    }
}

impl Fixture {
    fn daemon(&mut self) -> &mut Daemon {
        self.daemon.as_mut().expect("the daemon is still held")
    }

    fn daemon_pid(&self) -> u32 {
        fs::read_to_string(self.dir.join("pid"))
            .expect("pid file")
            .trim()
            .parse()
            .expect("a pid")
    }

    fn identify(&mut self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.daemon().identify() {
                Identity::Found(pid) => return pid,
                Identity::NotYet => {}
                other => panic!("identity: {other:?}"),
            }
            assert!(Instant::now() < deadline, "the daemon was never identified");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop(&mut self) -> (bool, String, Duration) {
        let mut out = Vec::new();
        let start = Instant::now();
        let ok = self.daemon().stop(&mut out);
        (ok, String::from_utf8(out).expect("utf-8"), start.elapsed())
    }

    fn time_log(&self) -> String {
        fs::read_to_string(self.dir.join("time.log")).unwrap_or_default()
    }

    fn assert_sentinel_alive(&mut self) {
        assert!(
            self.sentinel.try_wait().expect("sentinel").is_none(),
            "shutdown killed an unrelated process"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = dc_proc::signal_group(self.leader, Signal::Kill);
        drop(self.daemon.take());
        let _ = self.sentinel.kill();
        let _ = self.sentinel.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Whether `pid` is gone: no such process, or a zombie awaiting its reaper.
fn gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match dc_proc::signal_process(pid, Signal::Probe) {
            Err(e) if dc_proc::target_is_gone(&e) => return true,
            _ => {}
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

/// An operating system that refuses SIGKILL and delivers everything else.
struct RefusesKill;

impl Signals for RefusesKill {
    fn process(&self, pid: u32, signal: Signal) -> io::Result<()> {
        if signal == Signal::Kill {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        dc_proc::signal_process(pid, signal)
    }
    fn group(&self, leader: u32, signal: Signal) -> io::Result<()> {
        if signal == Signal::Kill {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        dc_proc::signal_group(leader, signal)
    }
}

#[test]
fn a_graceful_shutdown_retains_the_time_receipt() {
    let mut f = start("graceful", "exit 0", Box::new(Os));
    f.identify();
    let (ok, out, _) = f.stop();
    assert!(ok, "{out}");
    assert!(f.time_log().contains("real "), "receipt: {}", f.time_log());
    assert!(gone(f.daemon_pid()));
    f.assert_sentinel_alive();
}

#[test]
fn a_sigterm_ignoring_daemon_is_bounded_and_fails_the_soak() {
    let mut f = start("ignore", "", Box::new(Os));
    f.identify();
    let (ok, out, took) = f.stop();
    assert!(!ok);
    assert!(
        out.contains("SOAK FAIL: daemon shutdown exceeded 1 seconds"),
        "{out}"
    );
    assert!(took < Duration::from_secs(6), "a 1 s grace took {took:?}");
    assert!(
        gone(f.daemon_pid()),
        "shutdown left the owned daemon running"
    );
    f.assert_sentinel_alive();
}

#[test]
fn exit_cleanup_escalates_and_is_idempotent() {
    let mut f = start("cleanup", "", Box::new(Os));
    f.identify();
    let target = f.dir.join("main.py");
    fs::write(&target, "x = 1\n").expect("target");
    let corpus = Corpus::open(&f.dir, 1).expect("corpus");
    corpus.append_cycle(1).expect("edit");
    let endpoint = f.dir.join("socket");
    fs::write(&endpoint, "").expect("endpoint");
    // The timer's report sits inside the corpus; an interrupted run used to
    // leave it there.
    let timer_log = f.dir.join(".soak_time.1");
    fs::write(&timer_log, "real 0.01\n").expect("timer log");
    let mut cleanup = Cleanup {
        corpus: Some(corpus.clone()),
        daemon: f.daemon.take(),
        endpoint: Some(endpoint.clone()),
        timer_log: Some(timer_log.clone()),
    };
    let mut out = Vec::new();
    let first = cleanup.run(&mut out);
    let second = cleanup.run(&mut out);
    assert!(!first, "an escalated shutdown is not a clean exit");
    assert_eq!(first, second, "a second cleanup changed the verdict");
    assert!(
        !endpoint.exists() && !corpus.orig.exists() && !timer_log.exists(),
        "cleanup left its files"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("target"),
        "x = 1\n",
        "the corpus was not restored"
    );
    assert!(gone(f.daemon_pid()));
    f.assert_sentinel_alive();
}

#[test]
fn a_startup_failure_cleans_up_before_the_daemon_is_identified() {
    let mut f = start("unidentified", "", Box::new(Os));
    let mut cleanup = Cleanup {
        daemon: f.daemon.take(),
        ..Cleanup::default()
    };
    let mut out = Vec::new();
    assert!(!cleanup.run(&mut out));
    let out = String::from_utf8(out).expect("utf-8");
    assert!(
        out.contains("before its child identity was verified"),
        "{out}"
    );
    assert!(
        gone(f.daemon_pid()),
        "the unidentified daemon survived its group's cleanup"
    );
    f.assert_sentinel_alive();
}

#[test]
fn an_unconfirmed_forced_exit_is_bounded_and_reported() {
    let mut f = start("refused", "", Box::new(RefusesKill));
    f.identify();
    let (ok, out, took) = f.stop();
    assert!(!ok);
    assert!(
        out.contains("could not confirm termination after SIGKILL"),
        "{out}"
    );
    assert!(
        took < Duration::from_secs(6),
        "an unconfirmable kill took {took:?}"
    );
    assert!(
        !gone(f.daemon_pid()),
        "the refusal was not modelled: the daemon died anyway"
    );
    f.assert_sentinel_alive();
}

#[test]
fn a_failed_daemon_exit_cannot_count_as_a_successful_soak() {
    let mut f = start("failure", "exit 7", Box::new(Os));
    f.identify();
    let (ok, out, _) = f.stop();
    assert!(!ok);
    assert!(
        out.contains("SOAK FAIL: daemon shutdown exited with status 7"),
        "{out}"
    );
    f.assert_sentinel_alive();
}

#[test]
fn an_exited_wrapper_cannot_hide_a_reparented_daemon() {
    let mut f = start("orphan", "", Box::new(Os));
    let daemon_pid = f.daemon_pid();
    dc_proc::signal_process(f.leader, Signal::Kill).expect("kill the wrapper");
    let mut cleanup = Cleanup {
        daemon: f.daemon.take(),
        ..Cleanup::default()
    };
    let mut out = Vec::new();
    assert!(!cleanup.run(&mut out));
    assert!(
        gone(daemon_pid),
        "the reparented daemon outlived its wrapper's cleanup"
    );
    f.assert_sentinel_alive();
}

#[test]
fn the_group_exists_from_the_moment_of_spawn() {
    require_tools();
    let dir = scratch("spawn");
    let daemon = Daemon::spawn(
        "-p",
        Path::new("/bin/sh"),
        &["-c", "trap '' TERM; while :; do sleep 0.05; done"],
        &[],
        &dir,
        &dir.join("time.log"),
        GRACE,
    )
    .expect("spawn");
    dc_proc::signal_group(daemon.leader(), Signal::Probe).expect("the owned group exists at once");
    let mut cleanup = Cleanup {
        daemon: Some(daemon),
        ..Cleanup::default()
    };
    assert!(
        !cleanup.run(&mut Vec::new()),
        "an unidentified daemon is never a clean stop"
    );
    let _ = fs::remove_dir_all(&dir);
}
