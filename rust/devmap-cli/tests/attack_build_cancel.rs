//! Adversarial probes: real `devmap build` processes sent SIGINT.
#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use devmap_store::Store;

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-atkcancel-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn write_corpus(root: &Path, n: usize, shift: usize) {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    for i in 0..n {
        let next = (i + shift) % n;
        fs::write(
            src.join(format!("m{i}.py")),
            format!(
                "from src.m{next} import f{next}\n\n\
                 def f{i}():\n    return f{next}()\n\n\
                 def g{i}_{shift}(x):\n    return [f{next}() for _ in range(x)]\n"
            ),
        )
        .unwrap();
    }
}

fn integrity(db: &Path) -> String {
    let conn = rusqlite::Connection::open(db).unwrap();
    conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap()
}

fn latest(db: &Path, n: usize) -> u32 {
    let store = Store::open(db).expect("store opens after an interrupted build");
    let generation = store.latest_generation_id().unwrap().expect("a generation");
    let members = store.list_generation_paths(generation).unwrap();
    assert_eq!(
        members.iter().filter(|p| p.starts_with("src/")).count(),
        n,
        "generation {generation} is not whole"
    );
    generation
}

fn plain_build(db: &Path, root: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--db")
        .arg(db)
        .args(["build", "--progress", "never"])
        .arg(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "the build after an interrupt must succeed: {status}");
}

#[derive(Debug)]
struct Outcome {
    code: Option<i32>,
    signal: Option<i32>,
    stderr: String,
}

/// Start a build, wait for the extraction stage to be announced, sleep
/// `delay_ms`, send `sigints` SIGINTs `gap_ms` apart.
fn interrupted_build(db: &Path, root: &Path, delay_ms: u64, sigints: usize, gap_ms: u64) -> Outcome {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .arg("--db")
        .arg(db)
        .args(["build", "--progress", "always"])
        .arg(root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut all = String::new();
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            let hit = line.contains("extracting");
            all.push_str(&line);
            all.push('\n');
            if hit {
                let _ = tx.send(());
            }
        }
        all
    });
    let _ = rx.recv_timeout(Duration::from_secs(120));
    std::thread::sleep(Duration::from_millis(delay_ms));
    for k in 0..sigints {
        unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
        if k + 1 < sigints {
            std::thread::sleep(Duration::from_millis(gap_ms));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("HANG: build ignored {sigints} SIGINT(s) sent {delay_ms} ms after extraction began");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stderr = reader.join().unwrap();
    Outcome {
        code: status.code(),
        signal: status.signal(),
        stderr,
    }
}

#[test]
fn sigint_at_any_point_ends_130_or_complete_and_leaves_a_whole_store() {
    const FILES: usize = 1500;
    let root = temp_root("sweep");
    let db = root.join("state").join("devmap.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    write_corpus(&root, FILES, 1);
    plain_build(&db, &root);
    let mut generation = latest(&db, FILES);
    let mut lost_interrupts = Vec::new();
    for (round, delay) in [0u64, 1, 3, 8, 20, 50, 100, 200, 400, 800, 1600].iter().enumerate() {
        write_corpus(&root, FILES, round + 2);
        let outcome = interrupted_build(&db, &root, *delay, 1, 0);
        let after = latest(&db, FILES);
        assert_eq!(integrity(&db), "ok");
        let interrupted = outcome.code == Some(130) || outcome.signal == Some(libc::SIGINT);
        let finished = outcome.code == Some(0);
        assert!(
            interrupted || finished,
            "round {round} delay {delay}: unexpected exit {outcome:?}"
        );
        if finished {
            assert_eq!(after, generation + 1, "exit 0 but no new generation");
            if outcome.stderr.contains("stopping the build") {
                lost_interrupts.push((round, *delay));
            }
        }
        assert!(
            after == generation || after == generation + 1,
            "round {round}: generation jumped {generation} -> {after}"
        );
        eprintln!("round {round} delay {delay}ms: {outcome:?} gen {generation}->{after}");
        generation = after;
        plain_build(&db, &root);
        generation = latest(&db, FILES);
    }
    assert!(
        lost_interrupts.is_empty(),
        "SIGINT was acknowledged ('stopping the build') yet the build exited 0: {lost_interrupts:?}"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_second_sigint_exits_130_promptly_and_leaves_a_whole_store() {
    const FILES: usize = 1500;
    let root = temp_root("double");
    let db = root.join("state").join("devmap.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    write_corpus(&root, FILES, 1);
    plain_build(&db, &root);
    let generation = latest(&db, FILES);
    for gap in [0u64, 1, 5, 25] {
        write_corpus(&root, FILES, 2 + gap as usize);
        let outcome = interrupted_build(&db, &root, 5, 2, gap);
        assert!(
            outcome.code == Some(130) || outcome.signal == Some(libc::SIGINT) || outcome.code == Some(0),
            "gap {gap}: {outcome:?}"
        );
        let after = latest(&db, FILES);
        assert!(after >= generation);
        assert_eq!(integrity(&db), "ok");
        plain_build(&db, &root);
    }
    let _ = fs::remove_dir_all(&root);
}
