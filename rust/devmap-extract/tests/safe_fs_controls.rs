#![cfg(unix)]

use devmap_extract::safe_fs::{Access, Creation, PinnedDir, SafeFile};
use std::{
    ffi::OsStr,
    fs,
    io::Write,
    os::unix::fs::{symlink, MetadataExt, PermissionsExt},
};

fn root(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-safe-fs-{name}-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn ordinary_io_is_bounded_and_replacement_is_detected() {
    let root = root("ordinary");
    let path = root.join("nested/source");
    let mut file = SafeFile::open(&path, Access::ReadWrite, Creation::New).unwrap();
    file.rewrite(b"hello").unwrap();
    drop(file);
    assert_eq!(
        SafeFile::open(&path, Access::Read, Creation::Never)
            .unwrap()
            .read_text(5)
            .unwrap(),
        "hello"
    );
    assert!(SafeFile::open(&path, Access::Read, Creation::Never)
        .unwrap()
        .read_text(4)
        .is_err());
    let mut stale = SafeFile::open(&path, Access::ReadWrite, Creation::Never).unwrap();
    fs::rename(&path, root.join("original")).unwrap();
    fs::write(&path, "replacement").unwrap();
    assert!(stale.rewrite(b"must not write").is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    assert_eq!(fs::read_to_string(root.join("original")).unwrap(), "hello");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_creation_opens_one_shared_regular_file() {
    let root = root("concurrent-create");
    let parent = PinnedDir::open(&root, false).unwrap();
    let barrier = std::sync::Barrier::new(8);
    let failures = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let parent = &parent;
                let barrier = &barrier;
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for round in 0..64 {
                        barrier.wait();
                        let name = format!("shared-{round}");
                        if let Err(error) = parent.open_file(
                            OsStr::new(&name),
                            Access::ReadWrite,
                            Creation::IfMissing,
                        ) {
                            failures.push(format!("{name}: {error}"));
                        }
                        barrier.wait();
                    }
                    failures
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    drop(parent);
    fs::remove_dir_all(root).unwrap();
    assert!(failures.is_empty(), "concurrent opens failed: {failures:?}");
}

#[test]
fn pinned_parent_survives_a_directory_swap_without_redirecting_writes() {
    let root = root("parent-swap");
    fs::create_dir(root.join("parent")).unwrap();
    fs::create_dir(root.join("outside")).unwrap();
    let parent = PinnedDir::open(&root.join("parent"), false).unwrap();
    fs::rename(root.join("parent"), root.join("original")).unwrap();
    symlink(root.join("outside"), root.join("parent")).unwrap();
    parent
        .open_file(OsStr::new("file"), Access::ReadWrite, Creation::New)
        .unwrap()
        .write_all(b"safe")
        .unwrap();
    assert_eq!(fs::read(root.join("original/file")).unwrap(), b"safe");
    assert!(!root.join("outside/file").exists());
    assert!(SafeFile::open(&root.join("parent/new"), Access::ReadWrite, Creation::New).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn linked_writes_and_special_reads_are_refused() {
    let root = root("links");
    fs::write(root.join("victim"), b"sentinel").unwrap();
    symlink("victim", root.join("alias")).unwrap();
    fs::hard_link(root.join("victim"), root.join("hard")).unwrap();
    for leaf in ["alias", "hard"] {
        assert!(SafeFile::open(&root.join(leaf), Access::Append, Creation::IfMissing).is_err());
    }
    let fifo = root.join("fifo");
    let cpath = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    assert!(SafeFile::open(&fifo, Access::Read, Creation::Never).is_err());
    assert_eq!(fs::read(root.join("victim")).unwrap(), b"sentinel");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_permissions_are_changed_only_for_our_own_directory() {
    let root = root("owner");
    let directory = PinnedDir::open(&root, false).unwrap();
    directory.make_private().unwrap();
    assert_eq!(
        directory.metadata().unwrap().permissions().mode() & 0o777,
        0o700
    );
    let system = PinnedDir::open(std::path::Path::new("/"), false).unwrap();
    let before = system.metadata().unwrap();
    // Running as root cannot provide the foreign-owner control. Do not chmod
    // the filesystem root, including in such an environment.
    if before.uid() != directory.metadata().unwrap().uid() {
        assert!(system.make_private().is_err());
        assert_eq!(
            before.permissions().mode(),
            system.metadata().unwrap().permissions().mode()
        );
    }
    fs::remove_dir_all(root).unwrap();
}
