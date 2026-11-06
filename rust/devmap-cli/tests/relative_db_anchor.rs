//! A relative `--db` is anchored once, at parse, the way `--root` is.
//!
//! The flag is the only path this anchors. A store derived from the root keeps
//! that root's spelling. The language-server pass does not take `--db`: its
//! working directory is the build root, which `root_parser` already anchors,
//! and its scratch files are under the absolute temporary directory.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

fn devmap() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_devmap"))
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "devmap-reldb-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::new(devmap())
        .args(args)
        .current_dir(cwd)
        .env_remove("DEVMAP_HOME")
        .env("DEVMAP_AUTOSPAWN", "0")
        .output()
        .expect("devmap runs")
}

fn text(out: &Output) -> String {
    format!(
        "status={:?}\nSTDOUT:\n{}\nSTDERR:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn json(out: &Output, what: &str) -> serde_json::Value {
    assert!(out.status.success(), "{what} failed: {}", text(out));
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|error| panic!("{what}: not json ({error}): {}", text(out)))
}

fn db_path(cwd: &Path, db: &str) -> String {
    let out = run(
        cwd,
        &["--json", "--progress", "never", "--db", db, "status"],
    );
    json(&out, "status")["db_path"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Same fold `root_parser` applies after `std::path::absolute`: drop `.`,
/// fold `..`, and do not touch the filesystem.
fn lexical_clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn assert_one_absolute(actual: &str, suffix: &Path) {
    let path = Path::new(actual);
    assert!(
        path.is_absolute(),
        "a relative --db must be absolute after parse: {actual}"
    );
    assert!(
        path.ends_with(suffix),
        "{actual} does not end with {}",
        suffix.display()
    );
}

#[test]
fn a_relative_db_from_two_directories_names_one_absolute_store() {
    let base = scratch("same");
    let here = base.join("here");
    let nested = here.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let from_here = db_path(&here, "store.sqlite");
    let from_nested = db_path(&nested, "../store.sqlite");
    assert_eq!(from_here, from_nested);
    assert_one_absolute(&from_here, &Path::new("here").join("store.sqlite"));
    assert_ne!(from_here, "store.sqlite");
}

#[test]
fn dot_and_dotdot_fold_to_one_store() {
    let base = scratch("fold");
    std::fs::create_dir_all(base.join("real")).unwrap();
    let dotted = db_path(&base, "./store.sqlite");
    let plain = db_path(&base, "store.sqlite");
    let climbed = db_path(&base, "real/../store.sqlite");
    assert_eq!(dotted, plain);
    assert_eq!(climbed, plain);
    assert_one_absolute(&plain, Path::new("store.sqlite"));
    assert!(
        !Path::new(&plain)
            .components()
            .any(|component| component.as_os_str() == "real"),
        "dotdot must fold out of the path: {plain}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_component_is_not_resolved() {
    let base = scratch("link");
    let real = base.join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, base.join("link")).unwrap();
    let via_link = db_path(&base, "link/store.sqlite");
    assert_one_absolute(&via_link, &Path::new("link").join("store.sqlite"));
    assert!(
        !Path::new(&via_link)
            .components()
            .any(|component| component.as_os_str() == "real"),
        "the link target must not replace the spelling: {via_link}"
    );
}

#[test]
fn unicode_spaces_and_the_flag_after_the_subcommand_stay_absolute() {
    let base = scratch("odd");
    let dir = base.join("my code").join("名字");
    std::fs::create_dir_all(&dir).unwrap();
    let spelled = "my code/名字/store.sqlite";
    let before = db_path(&base, spelled);
    let after_command = run(
        &base,
        &["--json", "--progress", "never", "status", "--db", spelled],
    );
    let after = json(&after_command, "status after subcommand")["db_path"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(after, before);
    assert_one_absolute(
        &before,
        &Path::new("my code").join("名字").join("store.sqlite"),
    );
    let long = "n".repeat(200);
    let deep = base.join(&long);
    std::fs::create_dir_all(&deep).unwrap();
    let long_path = db_path(&base, &format!("{long}/store.sqlite"));
    assert_one_absolute(&long_path, &Path::new(&long).join("store.sqlite"));
}

#[test]
fn an_absolute_db_is_only_cleaned_and_an_omitted_db_keeps_the_derived_spelling() {
    let base = scratch("abs");
    let absolute = base.join("kept.sqlite");
    let cleaned = db_path(&base, &format!("{}/./x/../kept.sqlite", base.display()));
    assert_eq!(cleaned, lexical_clean(&absolute).display().to_string());
    let derived = run(&base, &["--json", "--progress", "never", "status"]);
    let derived_path = json(&derived, "derived status")["db_path"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !Path::new(&derived_path).is_absolute(),
        "an omitted --db stays the root-derived spelling: {derived_path}"
    );
}
