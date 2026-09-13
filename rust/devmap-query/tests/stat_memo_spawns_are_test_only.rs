//! The two `stat_memo` cases that need a child process.
//!
//! They live here rather than beside the code they test because
//! `a_subprocess_is_bounded_in_time_and_bytes` scans every production source
//! file for `Command::new(` and fails on any hit — the kernel routes all of its
//! own child processes through `run_bounded`, and a textual scan is what makes
//! that checkable. It exempts `tests/`, on the stated grounds that fixtures may
//! spawn `git` and the binary under test unbounded.
//!
//! "Unbounded" is not licence to hang, though: the fifo case below blocks
//! forever if it regresses, so it runs on its own thread behind a deadline and
//! fails rather than wedging the suite.

use std::path::Path;

use devmap_query::stat_memo;

fn scratch(label: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "devmap-statmemo-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// mtime as `SystemTime`, so "was it really restored?" is checkable
/// without reaching for a platform crate.
fn mtime_of(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .expect("stat")
        .modified()
        .expect("mtime")
}

#[test]
fn a_rewrite_that_restores_the_old_mtime_still_moves_the_key() {
    // The `cp -p` / `tar -x` / `rsync --times` shape: same size, mtime put
    // back. Only ctime separates the two states, so this is the case that
    // decides whether the key can be back-dated into a wrong hit.
    let dir = scratch("backdate");
    let path = dir.join("binary");
    let reference = dir.join("reference");
    std::fs::write(&path, b"aaaaaaaa").expect("write");
    std::fs::write(&reference, b"").expect("write reference");

    // Park the original mtime on a second file, rewrite, then put it back
    // the way `cp -p` does — through the same `utimensat` call.
    let original = mtime_of(&path);
    if !touch_from(&path, &reference) {
        eprintln!("skipped: `touch -r` is unavailable");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let before = stat_memo::stat_key(&std::fs::metadata(&path).expect("stat"));
    std::fs::write(&path, b"bbbbbbbb").expect("rewrite");
    if !touch_from(&reference, &path) {
        eprintln!("skipped: `touch -r` is unavailable");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    // Both halves of the premise, checked rather than assumed: if the size
    // moved, or `touch -r` did not restore mtime to the nanosecond, this
    // test would pass without saying anything about ctime.
    assert_eq!(
        std::fs::metadata(&path).expect("stat").len(),
        8,
        "the fixture must keep the size identical"
    );
    if mtime_of(&path) != original {
        eprintln!("skipped: `touch -r` did not restore mtime exactly");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }

    let after = stat_memo::stat_key(&std::fs::metadata(&path).expect("stat"));
    if cfg!(unix) {
        assert_ne!(
            before, after,
            "a restored mtime must not restore the memo key"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `touch -r source target`, which calls the same `utimensat` that `cp -p`
/// does. False when the tool is missing or refuses.
fn touch_from(source: &Path, target: &Path) -> bool {
    std::process::Command::new("touch")
        .arg("-r")
        .arg(source)
        .arg(target)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[test]
fn a_fifo_reads_as_absent_instead_of_blocking() {
    let dir = scratch("fifo");
    let path = dir.join("memo.json");
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !made {
        eprintln!("skipped: mkfifo is unavailable");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    // On its own thread with a deadline: if this regresses it blocks in
    // `open(2)` forever, and a hung suite says less than a failed test.
    let (tx, rx) = std::sync::mpsc::channel();
    let probe = path.clone();
    std::thread::spawn(move || {
        let _ = tx.send(stat_memo::read_bounded(&probe, 1 << 20).is_none());
    });
    match rx.recv_timeout(std::time::Duration::from_secs(15)) {
        Ok(absent) => assert!(absent, "a fifo is not a memo"),
        Err(_) => panic!("read_bounded blocked on a fifo instead of returning"),
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
}
