//! A liveness verdict is a property of the whole corpus, so it must land on a
//! file nobody edited even when the write is narrowed to the file somebody did.
//!
//! `devmap build --affected <path>` narrows what the build writes. Dead-symbol
//! rows are the generation-keyed half of a generation, written for every file;
//! if that ever became differential by *edited file*, a change in one file (or
//! in the liveness rules) would leave stale verdicts on every file it did not
//! touch. Two cases pin the edge:
//!
//! * the caller edit: delete the only call to `x` in `b.py`, narrow the write
//!   to `b.py`, and `x` in the untouched `lib.py` must become dead;
//! * the script rule: a shebang script's uncalled constant is dead (its
//!   file-level `ScriptEntry` no longer spares it) in a narrowed build whose
//!   only edit is elsewhere, and the narrowed answer equals a cold build's.

use std::path::Path;
use std::process::Command;

fn devmap() -> String {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join("devmap").to_string_lossy().into_owned()
}

fn run(root: &Path, args: &[&str]) {
    let out = Command::new(devmap())
        .args(args)
        .current_dir(root)
        .output()
        .expect("devmap");
    assert!(
        out.status.success(),
        "devmap {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `file::symbol` of every dead row nothing spares, sorted.
fn dead(root: &Path) -> Vec<String> {
    let store = devmap_store::Store::open(devmap_extract::paths::store_path(root)).unwrap();
    let mut rows: Vec<String> = store
        .latest_dead_symbols()
        .unwrap()
        .into_iter()
        .filter(|report| !report.is_exempt)
        .map(|report| format!("{}::{}", report.file_path, report.symbol_name))
        .collect();
    rows.sort();
    rows
}

fn scratch(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn deleting_the_only_caller_makes_a_symbol_in_an_unchanged_file_dead() {
    let root = scratch("narrowed-caller");
    std::fs::write(root.join("lib.py"), "def x():\n    return 1\n").unwrap();
    std::fs::write(
        root.join("b.py"),
        "from lib import x\n\n\ndef use():\n    return x()\n",
    )
    .unwrap();
    run(&root, &["build", "."]);
    assert!(
        !dead(&root).contains(&"lib.py::x".to_string()),
        "precondition: x is called: {:?}",
        dead(&root)
    );

    std::fs::write(root.join("b.py"), "def use():\n    return 2\n").unwrap();
    run(&root, &["build", ".", "--affected", "b.py"]);
    let narrowed = dead(&root);
    assert!(
        narrowed.contains(&"lib.py::x".to_string()),
        "lib.py was not edited, but its dead verdict must follow the edit to b.py: {narrowed:?}"
    );

    std::fs::remove_dir_all(devmap_extract::paths::state_dir(&root)).unwrap();
    run(&root, &["build", "."]);
    assert_eq!(
        narrowed,
        dead(&root),
        "the narrowed write must equal a cold build"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_scripts_uncalled_constant_is_dead_in_a_narrowed_build_that_never_touched_it() {
    let root = scratch("narrowed-script");
    std::fs::write(
        root.join("v5.py"),
        "#!/usr/bin/env python3\nF_POSITIONS = 3\n\n\ndef main():\n    return 0\n\n\nif __name__ == \"__main__\":\n    main()\n",
    )
    .unwrap();
    std::fs::write(root.join("b.py"), "def one():\n    return 1\n").unwrap();
    run(&root, &["build", "."]);

    std::fs::write(root.join("b.py"), "def one():\n    return 11\n").unwrap();
    run(&root, &["build", ".", "--affected", "b.py"]);
    let narrowed = dead(&root);
    assert!(
        narrowed.contains(&"v5.py::F_POSITIONS".to_string()),
        "the script's constant is dead whichever write produced the generation: {narrowed:?}"
    );
    assert!(
        !narrowed.contains(&"v5.py::main".to_string()),
        "the guarded entry is called: {narrowed:?}"
    );

    std::fs::remove_dir_all(devmap_extract::paths::state_dir(&root)).unwrap();
    run(&root, &["build", "."]);
    assert_eq!(
        narrowed,
        dead(&root),
        "the narrowed write must equal a cold build"
    );
    let _ = std::fs::remove_dir_all(&root);
}
