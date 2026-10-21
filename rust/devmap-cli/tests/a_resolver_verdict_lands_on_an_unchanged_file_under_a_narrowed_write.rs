//! A resolver verdict is a property of the whole corpus, so it must be written
//! for a file nobody edited even when the write is narrowed to the file
//! somebody did (`devmap build --affected <path>`).
//!
//! The resolver changes of this lane (overload self-edges and receiver binds,
//! Kotlin generic and reified types, a Python function-local module alias)
//! each decide edges of files that stay byte-identical. The narrowed build
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
    // Kotlin Z3 / Z5.
    assert!(
        has(rows, "Z.kt::useList", "Z.kt::List.total", "Calls"),
        "{when}: a local from a List<String> function reaches List.total"
    );
    assert!(
        has(rows, "Z.kt::useMock", "Z.kt::Repo.find", "Calls"),
        "{when}: mock<Repo>() types the local"
    );
    // Python function-local alias read.
    assert!(
        has(
            rows,
            "pkg/use.py::check",
            "pkg/m.py::CONST_READ",
            "References"
        ),
        "{when}: cr.CONST_READ through a function-local alias binds"
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
    write(
        &root,
        "Z.kt",
        "package gaps\n\nprivate fun List<String>.total(): Int = size\n\nfun names(): List<String> = listOf(\"a\")\n\nfun useList(flag: Boolean): Int {\n    val files = if (flag) names() else emptyList()\n    return files.total()\n}\n\ninterface Repo {\n    fun find(id: Int): String\n}\n\nfun <reified T> mock(): T = throw IllegalStateException()\n\nfun useMock(): String {\n    val repo = mock<Repo>()\n    return repo.find(1)\n}\n",
    );
    write(&root, "pkg/__init__.py", "");
    write(&root, "pkg/m.py", "CONST_DEAD = 1\nCONST_READ = 2\n");
    write(
        &root,
        "pkg/use.py",
        "def check():\n    from . import m as cr\n\n    return cr.CONST_READ\n",
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
