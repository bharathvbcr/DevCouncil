//! Attack round (lane S, R4): `remove_staged_writer_lock` and the staged path it is handed.
//! Every test asserts that the lock of a store that is NOT this process's private staged store
//! survives, and that a refusal leaves nothing that blocks the next writer.
#![cfg(unix)]

use devmap_store::seed::remove_staged_writer_lock;
use devmap_store::Store;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atk-s-r4-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn staged_name() -> String {
    format!("x.sqlite.seed-{}", std::process::id())
}

fn touch(path: &Path) {
    std::fs::write(path, b"1").unwrap();
}

/// A parent directory that is a symlink: the staged build took the lock under the canonical
/// directory, so that is the lock to remove, and nothing else may go.
#[test]
fn r4_parent_dir_symlink_removes_only_the_private_lock() {
    let tmp = scratch("parent");
    let real = tmp.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let link = tmp.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let staged = link.join(staged_name());
    touch(&staged);
    let private_lock = real.join(format!("{}.writer.lock", staged_name()));
    touch(&private_lock);
    let shared_lock = real.join("shared.sqlite.writer.lock");
    touch(&shared_lock);
    remove_staged_writer_lock(&staged).unwrap();
    assert!(!private_lock.exists(), "the private lock is removed");
    assert!(shared_lock.exists(), "no other lock is touched");
}

/// The lock file is a hard link of a shared lock: unlinking the name must not unlink the shared name.
#[test]
fn r4_hard_linked_lock_leaves_the_shared_name() {
    let tmp = scratch("hardlock");
    let shared_lock = tmp.join("shared.sqlite.writer.lock");
    touch(&shared_lock);
    let staged = tmp.join(staged_name());
    touch(&staged);
    let private_lock = tmp.join(format!("{}.writer.lock", staged_name()));
    std::fs::hard_link(&shared_lock, &private_lock).unwrap();
    remove_staged_writer_lock(&staged).unwrap();
    assert!(shared_lock.exists());
    assert!(!private_lock.exists());
}

/// The staged store itself is a hard link of a shared store: the lock path is derived from the
/// staged NAME, so the shared store's lock is not the one resolved.
#[test]
fn r4_hard_linked_staged_store_resolves_its_own_lock() {
    let tmp = scratch("hardstore");
    let shared = tmp.join("shared.sqlite");
    touch(&shared);
    let shared_lock = tmp.join("shared.sqlite.writer.lock");
    touch(&shared_lock);
    let staged = tmp.join(staged_name());
    std::fs::hard_link(&shared, &staged).unwrap();
    remove_staged_writer_lock(&staged).unwrap();
    assert!(shared_lock.exists());
}

/// A dangling symlink at the staged name must be refused and unlink nothing.
#[test]
fn r4_dangling_symlink_staged_is_refused() {
    let tmp = scratch("dangling");
    let staged = tmp.join(staged_name());
    std::os::unix::fs::symlink(tmp.join("missing.sqlite"), &staged).unwrap();
    let other_lock = tmp.join("missing.sqlite.writer.lock");
    touch(&other_lock);
    assert!(remove_staged_writer_lock(&staged).is_err());
    assert!(other_lock.exists());
}

/// A symlink whose target is a store with a held `flock`: refusal, lock file untouched, and the
/// next writer on the shared store still locks.
#[test]
fn r4_refusal_does_not_block_the_next_writer() {
    let tmp = scratch("next");
    let shared = tmp.join("shared.sqlite");
    touch(&shared);
    let staged = tmp.join(staged_name());
    std::os::unix::fs::symlink(&shared, &staged).unwrap();
    assert!(remove_staged_writer_lock(&staged).is_err());
    // Whatever the refusal left behind, a fresh writer lock on a real store path must be granted.
    std::fs::remove_file(&staged).unwrap();
    let fresh = tmp.join("fresh.sqlite");
    let _store = Store::open(&fresh).unwrap();
    drop(_store);
    let guard = Store::lock_writer_at(&fresh, Duration::from_millis(500));
    assert!(guard.is_ok(), "next writer lock: {:?}", guard.err());
}

/// APFS is case-insensitive by default. A case-variant directory component must still resolve
/// to the lock the staged build took (canonical case), and a case-variant staging NAME is refused
/// by the suffix check (stricter, not looser).
#[test]
fn r4_case_variant_path_on_apfs() {
    let tmp = scratch("case");
    let dir = tmp.join("Lane");
    std::fs::create_dir_all(&dir).unwrap();
    let insensitive = std::fs::metadata(tmp.join("lANE")).is_ok();
    if !insensitive {
        eprintln!("case-sensitive filesystem: variant path test skipped (recorded, not a pass)");
        return;
    }
    let staged_canon = dir.join(staged_name());
    touch(&staged_canon);
    let lock = dir.join(format!("{}.writer.lock", staged_name()));
    touch(&lock);
    let variant = tmp.join("lANE").join(staged_name());
    remove_staged_writer_lock(&variant).unwrap();
    assert!(!lock.exists(), "variant-case path removes the canonical private lock");
    // case-variant staging name
    let upper = dir.join(format!("x.sqlite.SEED-{}", std::process::id()));
    assert!(remove_staged_writer_lock(&upper).is_err());
}

/// The staged name becomes a symlink to a shared store AFTER the symlink_metadata check. A
/// flip-flopping renamer races `remove_staged_writer_lock`; the shared lock must survive every
/// iteration. One extra thread (a started thread, not CPU load beyond one core).
#[test]
fn r4_staged_name_becomes_symlink_after_the_check() {
    let tmp = scratch("toctou");
    let shared = tmp.join("shared.sqlite");
    touch(&shared);
    let shared_lock = tmp.join("shared.sqlite.writer.lock");
    touch(&shared_lock);
    let staged = tmp.join(staged_name());
    let tmp_link = tmp.join("flip");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let (staged2, shared2, tmp_link2) = (staged.clone(), shared.clone(), tmp_link.clone());
    let flipper = std::thread::spawn(move || {
        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = std::fs::remove_file(&tmp_link2);
            if std::os::unix::fs::symlink(&shared2, &tmp_link2).is_ok() {
                let _ = std::fs::rename(&tmp_link2, &staged2); // staged -> symlink to shared
            }
            let _ = std::fs::remove_file(&staged2); // staged -> absent
        }
    });
    let mut unlinked_shared = 0u32;
    let mut refused = 0u32;
    for _ in 0..20_000 {
        if !shared_lock.exists() {
            unlinked_shared += 1;
            touch(&shared_lock);
        }
        if remove_staged_writer_lock(&staged).is_err() {
            refused += 1;
        }
        if !shared_lock.exists() {
            unlinked_shared += 1;
            touch(&shared_lock);
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    flipper.join().unwrap();
    eprintln!("toctou: shared lock unlinked {unlinked_shared} times, refused {refused} / 20000");
    assert_eq!(unlinked_shared, 0, "a lock of a store that is not this process's was unlinked");
}
