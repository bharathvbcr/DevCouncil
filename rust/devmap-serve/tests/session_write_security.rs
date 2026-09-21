#![cfg(unix)]
use devmap_serve::session_log::{append_query, live_log_path};
use std::fs;
use std::os::unix::fs::symlink;

#[test]
fn session_rotation_checks_both_names_and_preserves_normal_records() {
    use devmap_serve::session_log::{read_live, rotate_live};
    let root = std::env::temp_dir().join(format!("devmap-session-rotation-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let db = root.join("store.sqlite");
    fs::write(&db, "store").unwrap();
    append_query(&db, "devmap_status", None, None, None, 1);
    assert_eq!(read_live(&db).unwrap().records.len(), 1);
    assert!(rotate_live(&db, "../outside").is_err());
    fs::write(root.join("sentinel"), "outside sentinel").unwrap();
    symlink(root.join("sentinel"), root.join("sessions/linked.jsonl")).unwrap();
    assert!(rotate_live(&db, "linked").is_err());
    assert_eq!(
        fs::read_to_string(root.join("sentinel")).unwrap(),
        "outside sentinel"
    );
    assert!(rotate_live(&db, "normal").unwrap());
    assert!(read_live(&db).unwrap().records.is_empty());
    assert!(!rotate_live(&db, "again").unwrap());
    assert!(fs::read_to_string(root.join("sessions/normal.jsonl"))
        .unwrap()
        .contains("devmap_status"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn session_appends_refuse_linked_files_and_ancestors() {
    for case in ["leaf", "sessions", "state", "hardlink"] {
        let root = std::env::temp_dir().join(format!(
            "devmap-session-boundary-{case}-{}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("repo")).unwrap();
        fs::create_dir_all(root.join("outside")).unwrap();
        let victim = root.join("outside/sentinel");
        fs::write(&victim, b"outside sentinel\n").unwrap();
        let db = if case == "state" {
            symlink(root.join("outside"), root.join("repo/state")).unwrap();
            root.join("repo/state/devmap.sqlite")
        } else {
            root.join("repo/devmap.sqlite")
        };
        fs::write(&db, b"existing store").unwrap();
        let log = live_log_path(&db);
        match case {
            "leaf" | "hardlink" => {
                fs::create_dir_all(log.parent().unwrap()).unwrap();
                if case == "leaf" {
                    symlink(&victim, &log).unwrap();
                } else {
                    fs::hard_link(&victim, &log).unwrap();
                }
            }
            "sessions" => symlink(root.join("outside"), log.parent().unwrap()).unwrap(),
            "state" => {}
            _ => unreachable!(),
        }
        append_query(&db, "devmap_status", None, None, None, 1);
        assert_eq!(
            fs::read(&victim).unwrap(),
            b"outside sentinel\n",
            "{case} appended outside"
        );
        assert!(
            !root.join("outside/live.jsonl").exists(),
            "{case} created outside log"
        );
        assert!(
            !root.join("outside/sessions").exists(),
            "{case} created outside directory"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
