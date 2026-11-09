//! `devmap seed --sync-delta` writes a lane's `delta.sqlite`, so it takes the
//! delta's writer lock for the whole sync.
//!
//! Before this it took none: a second writer (a daemon drain, another hook's
//! detached sync, a build) held the lock and the sync wrote under it anyway.
//! The test holds the lock from outside, starts the sync, and requires that the
//! delta stays byte-identical and the sync still running while the lock is
//! held, then that the sync completes and records its edit once the lock is
//! released.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use devmap_extract::paths::stack_delta_path;
use devmap_store::Store;

struct Fixture {
    base: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "devmap-delta-lock-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let main = base.join("main");
        fs::create_dir_all(main.join("src")).unwrap();
        fs::write(
            main.join("src/a.py"),
            "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n",
        )
        .unwrap();
        fs::write(
            main.join("src/b.py"),
            "from a import alpha\n\n\ndef gamma():\n    return alpha()\n",
        )
        .unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["add", "."]);
        git(&main, &["commit", "-qm", "fixture"]);
        git(&main, &["worktree", "add", "-q", "--detach", "../lane"]);
        Self { base }
    }

    fn main(&self) -> PathBuf {
        self.base.join("main")
    }

    fn lane(&self) -> PathBuf {
        self.base.join("lane").canonicalize().unwrap()
    }

    fn command(&self, root: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_devmap"));
        command
            .arg("--root")
            .arg(root)
            .args(args)
            .current_dir(&self.base)
            .env_remove("DEVMAP_HOME")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("DEVMAP_AUTOSPAWN", "0")
            .env("TOKIO_WORKER_THREADS", "2")
            .env("RAYON_NUM_THREADS", "2");
        command
    }

    fn devmap(&self, root: &Path, args: &[&str]) -> Output {
        self.command(root, args).output().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "core.hooksPath=",
            "-c",
            "init.templateDir=",
        ])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The delta's database bytes and its write-ahead log, if there is one. The
/// `.writer.lock` file is left out: it records the holder's pid by design.
fn delta_bytes(delta: &Path) -> (Vec<u8>, Option<Vec<u8>>) {
    let main = fs::read(delta).unwrap();
    let mut wal = delta.as_os_str().to_owned();
    wal.push("-wal");
    let wal = match fs::read(PathBuf::from(wal)) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => panic!("cannot read the delta's WAL: {err}"),
    };
    (main, wal)
}

#[test]
fn sync_delta_waits_for_a_held_delta_writer_lock_and_writes_nothing_meanwhile() {
    let fx = Fixture::new();
    let lane = fx.lane();
    let built = fx.devmap(&fx.main(), &["build", "--manifest"]);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let seeded = fx.devmap(&lane, &["--json", "seed", "--stacked"]);
    assert!(
        seeded.status.success(),
        "{}",
        String::from_utf8_lossy(&seeded.stderr)
    );

    // A first sync creates the delta; the second edit is what the held sync
    // would have to write.
    fs::write(lane.join("src/a.py"), "def alpha():\n    return 2\n").unwrap();
    let first = fx.devmap(&lane, &["--json", "seed", "--sync-delta"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let delta = stack_delta_path(&lane);
    assert!(
        delta.is_file(),
        "the first sync must have created the delta"
    );
    fs::write(lane.join("src/b.py"), "def gamma():\n    return 3\n").unwrap();
    let before = delta_bytes(&delta);

    let held = Store::lock_writer_at(&delta, Duration::from_secs(5)).unwrap();
    let mut child = fx
        .command(&lane, &["--json", "seed", "--sync-delta"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let exited_while_held = child.try_wait().unwrap();
    let during = delta_bytes(&delta);
    if exited_while_held.is_none() {
        // Release the lock; the sync may now proceed.
        drop(held);
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "the sync must complete once the lock is released: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["synced_files"], 2, "{report}");
        assert_ne!(
            delta_bytes(&delta),
            before,
            "the released sync must record its edit"
        );
    } else {
        child.wait().unwrap();
        drop(held);
    }
    assert!(
        exited_while_held.is_none(),
        "`seed --sync-delta` finished ({exited_while_held:?}) while the delta writer lock was held elsewhere"
    );
    assert_eq!(
        during, before,
        "the delta changed while another process held its writer lock"
    );
}
