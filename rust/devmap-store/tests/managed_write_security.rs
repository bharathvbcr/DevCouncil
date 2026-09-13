#![cfg(unix)]
use devmap_store::Store;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::time::Duration;

#[test]
fn writer_lock_cannot_modify_a_link_target() {
    for case in ["symlink", "hardlink", "parent"] {
        let root = std::env::temp_dir().join(format!(
            "devmap-lock-boundary-{case}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("repo")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        let victim = root.join("outside/notes");
        fs::write(&victim, b"outside sentinel").unwrap();
        let db = if case == "parent" {
            symlink(root.join("outside"), root.join("repo/state")).unwrap();
            root.join("repo/state/devmap.sqlite")
        } else {
            root.join("repo/devmap.sqlite")
        };
        let lock = Store::writer_lock_path(&db);
        match case {
            "symlink" => symlink(&victim, lock).unwrap(),
            "hardlink" => fs::hard_link(&victim, lock).unwrap(),
            "parent" => {}
            _ => unreachable!(),
        }
        assert!(
            Store::lock_writer_at(&db, Duration::ZERO).is_err(),
            "{case} lock was accepted"
        );
        assert_eq!(fs::read(&victim).unwrap(), b"outside sentinel");
        assert!(!root.join("outside/devmap.sqlite.writer.lock").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn sidecar_repair_cannot_change_unrelated_permissions() {
    for case in [
        "symlink",
        "hardlink",
        "invalid_database",
        "unrelated_database",
    ] {
        let root = std::env::temp_dir().join(format!(
            "devmap-mode-boundary-{case}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let db = root.join("store.sqlite");
        if case == "invalid_database" {
            fs::write(&db, b"not a SQLite database").unwrap();
        } else if case == "unrelated_database" {
            let connection = rusqlite::Connection::open(&db).unwrap();
            connection
                .execute_batch("CREATE TABLE unrelated(value TEXT)")
                .unwrap();
        } else {
            drop(Store::open(&db).unwrap());
        }
        let victim = root.join("outside-sentinel");
        fs::write(&victim, b"outside sentinel").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o400)).unwrap();
        let sidecar = root.join("store.sqlite-wal");
        if case == "symlink" {
            symlink(&victim, &sidecar).unwrap();
        } else if case == "hardlink" {
            fs::hard_link(&victim, &sidecar).unwrap();
        } else {
            fs::write(&sidecar, b"independent sidecar sentinel").unwrap();
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o400)).unwrap();
        }
        assert!(Store::open(&db).is_err(), "{case} should refuse the store");
        let mode = fs::metadata(&victim).unwrap().permissions().mode() & 0o777;
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(mode, 0o400, "{case} broadened unrelated permissions");
        assert_eq!(fs::read(&victim).unwrap(), b"outside sentinel");
        if matches!(case, "invalid_database" | "unrelated_database") {
            assert_eq!(
                fs::metadata(&sidecar).unwrap().permissions().mode() & 0o777,
                0o400
            );
            assert_eq!(fs::read(&sidecar).unwrap(), b"independent sidecar sentinel");
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn final_database_aliases_still_share_writer_ownership() {
    let root = std::env::temp_dir().join(format!("devmap-db-alias-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let db = root.join("store.sqlite");
    drop(Store::open(&db).unwrap());
    let alias = root.join("alias.sqlite");
    symlink(&db, &alias).unwrap();
    for (first, second) in [(&alias, &db), (&db, &alias)] {
        let owner = Store::lock_writer_at(first, Duration::ZERO).unwrap();
        assert!(owner.is_held());
        assert!(Store::lock_writer_at(second, Duration::ZERO).is_err());
        drop(owner);
    }
    fs::remove_dir_all(root).unwrap();
}
