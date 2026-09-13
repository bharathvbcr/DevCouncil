#![cfg(unix)]

use devmap_extract::subprocess::{run_bounded, Bounds};
use devmap_extract::{scan_tree, CacheDirectoryCache, CacheVerdict, CACHEDIR_TAG_SIGNATURE};
use std::fs;
use std::process::Command;
use std::time::Duration;

#[test]
fn special_cache_marker_cannot_block_discovery() {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child.args([
        "--ignored",
        "--exact",
        "fifo_cache_marker_child",
        "--nocapture",
    ]);
    let result = run_bounded(
        &mut child,
        Bounds {
            deadline: Duration::from_secs(3),
            stdout_cap: 16384,
            stderr_cap: 16384,
        },
    )
    .expect("cache marker reads must finish within the deadline");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
#[ignore = "bounded child of special_cache_marker_cannot_block_discovery"]
fn fifo_cache_marker_child() {
    let root = std::env::temp_dir().join(format!("devmap-cache-fifo-{}", std::process::id()));
    fs::create_dir_all(root.join("pkg")).unwrap();
    let fifo = root.join("pkg/CACHEDIR.TAG");
    let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    fs::write(root.join("pkg/main.py"), "def f(): pass\n").unwrap();
    let scanned = scan_tree(&root).unwrap();
    assert!(
        scanned.sources.is_empty(),
        "an unexamined marker must not admit the subtree"
    );
    assert!(
        scanned
            .report
            .skipped_paths
            .iter()
            .any(|(path, reason)| path == "pkg"
                && matches!(
                    reason,
                    devmap_extract::DiscoverySkipReason::Unreadable { .. }
                )),
        "the unreadable marker must be reported"
    );
    let mut cache = CacheDirectoryCache::default();
    assert!(matches!(
        cache.tagged_ancestor(&root, "pkg/main.py"),
        CacheVerdict::Unreadable { .. }
    ));
    assert!(devmap_extract::collect_go_modules(&root)
        .unwrap_err()
        .to_string()
        .contains("CACHEDIR.TAG"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cache_marker_checks_only_the_required_prefix() {
    let root = std::env::temp_dir().join(format!("devmap-cache-prefix-{}", std::process::id()));
    fs::create_dir_all(root.join("pkg")).unwrap();
    let mut tag = CACHEDIR_TAG_SIGNATURE.to_vec();
    tag.extend(vec![b'#'; 2 * 1024 * 1024]);
    fs::write(root.join("pkg/CACHEDIR.TAG"), tag).unwrap();
    fs::write(root.join("pkg/main.py"), "def f(): pass\n").unwrap();
    let mut cache = CacheDirectoryCache::default();
    assert_eq!(
        cache.tagged_ancestor(&root, "pkg/main.py"),
        CacheVerdict::Inside("pkg".into())
    );
    assert!(scan_tree(&root).unwrap().sources.is_empty());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn linked_and_directory_markers_are_reported_as_unexamined() {
    let root = std::env::temp_dir().join(format!("devmap-cache-links-{}", std::process::id()));
    fs::create_dir_all(root.join("linked")).unwrap();
    fs::create_dir_all(root.join("directory/CACHEDIR.TAG")).unwrap();
    fs::write(root.join("tag"), CACHEDIR_TAG_SIGNATURE).unwrap();
    std::os::unix::fs::symlink(root.join("tag"), root.join("linked/CACHEDIR.TAG")).unwrap();
    for dir in ["linked", "directory"] {
        fs::write(root.join(dir).join("main.py"), "def f(): pass\n").unwrap();
        assert!(devmap_extract::is_cache_directory(&root.join(dir)).is_err());
    }
    let report = scan_tree(&root).unwrap().report;
    assert_eq!(
        report
            .skipped_paths
            .iter()
            .filter(
                |(path, reason)| ["linked", "directory"].contains(&path.as_str())
                    && matches!(
                        reason,
                        devmap_extract::DiscoverySkipReason::Unreadable { .. }
                    )
            )
            .count(),
        2
    );
    fs::remove_dir_all(root).unwrap();
}
