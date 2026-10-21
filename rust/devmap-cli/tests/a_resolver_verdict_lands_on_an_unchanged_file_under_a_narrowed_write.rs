//! A resolver verdict is a property of the whole corpus, so it must be written
//! for a file nobody edited even when the write is narrowed to the file
//! somebody did (`devmap build --affected <path>`).
//!
//! The resolver change of this lane (same-file overload self-edges and the
//! receiver bind on an overloaded method) decides edges of a file that stays
//! byte-identical. The narrowed build
//! below edits only `unrelated.py`; the edges of the untouched files must be
//! the ones a cold build writes, and must be the corrected ones.

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

/// `source>target:kind:confidence` of every edge of the committed generation.
fn edges(root: &Path) -> Vec<String> {
    let store = devmap_store::Store::open(devmap_extract::paths::store_path(root)).unwrap();
    let mut rows = store.latest_edges_for_test().unwrap();
    rows.sort();
    rows
}

fn has(rows: &[String], source: &str, target: &str, kind: &str) -> bool {
    let prefix = format!("{source}>{target}:{kind}:");
    rows.iter().any(|row| row.starts_with(&prefix))
}

fn scratch(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("devmap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write(root: &Path, path: &str, source: &str) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, source).unwrap();
}

fn assert_corrected(rows: &[String], when: &str) {
    // Overloads: no self-edge, and the receiver call binds.
    assert!(
        !has(
            rows,
            "Overloads.kt::Calc.add",
            "Overloads.kt::Calc.add",
            "Calls"
        ),
        "{when}: an overload calling its sibling is no self-edge"
    );
    assert!(
        has(
            rows,
            "Overloads.kt::useCalc",
            "Overloads.kt::Calc.add",
            "Calls"
        ),
        "{when}: c.add(..) on a Calc binds"
    );
}

#[test]
fn resolver_verdicts_land_on_unchanged_files_in_a_narrowed_build() {
    let root = scratch("narrowed-resolver");
    write(
        &root,
        "Overloads.kt",
        "package gaps\n\nclass Calc {\n    fun add(a: Int): Int = a\n\n    fun add(a: String): Int = add(a.length)\n}\n\nfun useCalc(c: Calc): Int = c.add(1) + c.add(\"two\")\n",
    );
    write(&root, "unrelated.py", "def one():\n    return 1\n");
    run(&root, &["build", "."]);
    assert_corrected(&edges(&root), "cold build");

    write(&root, "unrelated.py", "def one():\n    return 11\n");
    run(&root, &["build", ".", "--affected", "unrelated.py"]);
    let narrowed = edges(&root);
    assert_corrected(&narrowed, "narrowed build");

    std::fs::remove_dir_all(devmap_extract::paths::state_dir(&root)).unwrap();
    run(&root, &["build", "."]);
    assert_eq!(
        narrowed,
        edges(&root),
        "the narrowed write must equal a cold build"
    );
    let _ = std::fs::remove_dir_all(&root);
}
