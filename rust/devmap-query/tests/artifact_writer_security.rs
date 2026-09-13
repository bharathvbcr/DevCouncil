#![cfg(unix)]
use std::{fs, os::unix::fs::symlink};

#[test]
fn preplanted_temporary_names_are_neither_written_nor_removed() {
    let root =
        std::env::temp_dir().join(format!("devmap-artifact-preplant-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let victim = root.join("victim");
    fs::write(&victim, b"sentinel").unwrap();
    // This integration process has made no previous atomic writes.
    for sequence in 0..16 {
        symlink(
            &victim,
            root.join(format!("graph.{}.{sequence}.tmp", std::process::id())),
        )
        .unwrap();
    }
    assert!(devmap_query::write_atomic(&root.join("graph"), b"new graph").is_err());
    assert!(!root.join("graph").exists());
    assert_eq!(fs::read(&victim).unwrap(), b"sentinel");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 17);
    assert!(devmap_query::write_atomic(&root.join("graph"), b"new graph").unwrap());
    assert!(!devmap_query::write_atomic(&root.join("graph"), b"new graph").unwrap());
    // An unchanged publication needs no write, including when the destination
    // directory was subsequently made read-only.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
    let unchanged = devmap_query::write_atomic(&root.join("graph"), b"new graph");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(!unchanged.expect("unchanged artifact does not need directory write access"));
}
