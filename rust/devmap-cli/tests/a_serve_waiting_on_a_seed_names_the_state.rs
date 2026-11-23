//! `devmap serve` creating a store while a seed holds its writer lock used to
//! block silently for up to `Store::WRITER_LOCK_WAIT` (60 s) with nothing on
//! any stream, so a host that starts the server with a shorter timeout saw a
//! hang it could not name. The wait now says what it is waiting for on stderr,
//! stays bounded, and ends at once when the process is told to stop.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use devmap_store::Store;

const STATE: &str = "state=waiting_for_seed_writer_lock";

struct Served {
    child: Child,
    lines: Receiver<String>,
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("serve-wait-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn start(dir: &Path, db: &Path) -> Served {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--db")
        .arg(db)
        .arg("serve")
        .arg(dir)
        .arg("--socket")
        .arg(dir.join("serve.sock"))
        .env("DEVMAP_AUTOSPAWN", "0")
        .env("DEVMAP_MAX_IDLE_SECS", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("devmap serve spawns");
    let stderr = child.stderr.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    Served { child, lines }
}

impl Served {
    fn line_containing(&self, needle: &str, limit: Duration) -> Option<String> {
        let deadline = Instant::now() + limit;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return Some(line),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }

    fn terminate_and_wait(&mut self, limit: Duration) -> Option<std::process::ExitStatus> {
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        None
    }
}

/// A host whose start timeout (here 4 s) is far shorter than the 60 s wait must
/// be told what the server is waiting for, and must be able to end the wait.
#[test]
fn a_serve_start_waiting_on_a_seed_names_the_state_and_ends_when_told_to_stop() {
    let dir = scratch("named");
    let db = dir.join("devmap.sqlite");
    let held = Store::lock_writer_at(&db, Duration::from_secs(5)).unwrap();

    let mut served = start(&dir, &db);
    let line = served
        .line_containing(STATE, Duration::from_secs(4))
        .expect("no named state on stderr within the host's start timeout");
    assert!(line.contains(&format!("pid={}", std::process::id())), "{line}");
    assert!(line.contains("bound_s=60"), "{line}");
    assert!(!db.exists(), "no store may appear while the lock is held");

    let started = Instant::now();
    let status = served
        .terminate_and_wait(Duration::from_secs(10))
        .expect("the wait did not end when the process was told to stop");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "stopping took {:?}",
        started.elapsed()
    );
    assert!(!status.success(), "a cancelled start is not a clean serve");
    assert!(!db.exists(), "a cancelled start created a store");
    drop(held);
}

/// The wait is a wait, not a refusal: once the seed lets go the server creates
/// its store and binds.
#[test]
fn a_serve_start_proceeds_once_the_seed_releases_the_lock() {
    let dir = scratch("released");
    let db = dir.join("devmap.sqlite");
    let held = Store::lock_writer_at(&db, Duration::from_secs(5)).unwrap();

    let mut served = start(&dir, &db);
    served
        .line_containing(STATE, Duration::from_secs(10))
        .expect("no named state on stderr");
    drop(held);
    served
        .line_containing("state=writer_lock_acquired", Duration::from_secs(20))
        .expect("the wait never reported that it ended");
    let socket = dir.join("serve.sock");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !socket.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(socket.exists(), "the server never bound after the release");
    assert!(db.is_file());
    served.terminate_and_wait(Duration::from_secs(20));
}
