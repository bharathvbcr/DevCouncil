//! `workspace add` registers a repository's directory in *this* repository's
//! registry, and says what it did.
//!
//! Read back from the release binary with `--db` naming a store outside the
//! standard layout (`<scratch>/corpus-db/devmap.sqlite`, cwd inside the
//! corpus): the registry was written to `<scratch>/../.devmap/workspace.json`
//! — two directories above the store, in a directory that is nobody's
//! repository — because the store-to-root inverse answered the grandparent of
//! any path. The same command registered a regular file, an empty name and a
//! name with a newline in it, and re-registering a name replaced the entry
//! without saying so.

use std::path::{Path, PathBuf};
use std::process::Command;

fn devmap() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap")
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-wsreg-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn add(cwd: &Path, db: &Path, name: &str, path: &Path) -> (Option<i32>, serde_json::Value, String) {
    let out = Command::new(devmap())
        .args(["--json", "--db"])
        .arg(db)
        .args(["workspace", "add", "--name", name])
        .arg(path)
        .current_dir(cwd)
        .output()
        .expect("devmap runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value = serde_json::from_str(stdout.trim()).unwrap_or(serde_json::Value::Null);
    (
        out.status.code(),
        value,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn an_off_layout_store_does_not_place_the_registry_above_the_repository() {
    let dir = scratch("offlayout");
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let db = dir.join("corpus-db").join("devmap.sqlite");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();

    let (code, value, stderr) = add(&repo, &db, "repo", &repo);
    assert_eq!(code, Some(0), "{value}\n{stderr}");
    let registry = PathBuf::from(value["registry"].as_str().expect("registry path"));
    assert!(
        registry.starts_with(&repo),
        "the registry must live in the repository the command ran in, not at {}",
        registry.display()
    );
    for stray in [dir.join(".devmap"), dir.join(".devcouncil")] {
        assert!(
            !stray.exists(),
            "nothing may be written above the repository: {}",
            stray.display()
        );
    }
}

#[test]
fn a_registered_repository_is_a_directory_and_its_name_is_a_label() {
    let dir = scratch("labels");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let db = repo.join(".devmap").join("codeintel").join("devmap.sqlite");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "not a repository\n").unwrap();

    let (code, value, stderr) = add(&repo, &db, "filey", &file);
    assert_ne!(code, Some(0), "a file was registered: {value}");
    assert!(stderr.contains("not a directory"), "{stderr}");

    for name in ["", "a\nb"] {
        let (code, value, stderr) = add(&repo, &db, name, &repo);
        assert_ne!(code, Some(0), "{name:?} was accepted as a name: {value}");
        assert!(
            stderr.contains("name"),
            "{name:?}: the refusal must say what is wrong: {stderr}"
        );
    }
}

#[test]
fn re_registering_a_name_says_it_replaced_the_entry() {
    let dir = scratch("replace");
    let repo = dir.join("repo");
    let other = dir.join("other");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let db = repo.join(".devmap").join("codeintel").join("devmap.sqlite");

    let (code, first, stderr) = add(&repo, &db, "thing", &repo);
    assert_eq!(code, Some(0), "{first}\n{stderr}");
    assert_eq!(first["replaced"], false, "{first}");
    let (code, second, stderr) = add(&repo, &db, "thing", &other);
    assert_eq!(code, Some(0), "{second}\n{stderr}");
    assert_eq!(second["replaced"], true, "{second}");
}
