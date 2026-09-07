//! `package_managers` and `test_commands` must come from the repository, not
//! from a constant.
//!
//! Both shipped as `[]` on every repository the kernel has ever mapped, with
//! `package_managers_computed: false` / `test_commands_computed: false` saying
//! honestly that nothing had looked. The reason given was that the evidence
//! never reaches `extractions`: a `.lock` file matches no language spec, so
//! `is_indexable_source` excludes it, and a test command needs the *contents*
//! of `pyproject.toml` / `package.json` rather than their existence.
//!
//! That reason is about the extraction pass, not about the kernel. The kernel
//! is handed a repository root — `build_code_graph_value` already takes one —
//! and reading a bounded inventory off it answers both questions from evidence
//! this producer can actually support.
//!
//! End to end through the binary rather than against the library function, so
//! the assertion covers the call site as well as the computation: a manifest
//! writer that computes an inventory nobody hands a root to is the same empty
//! list with a `true` marker on it, which is strictly worse than what it
//! replaced.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn temp_root(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock must be after epoch")
        .as_nanos();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "devmap-inventory-{tag}-{}-{stamp}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src")).expect("create fixture tree");
    fs::create_dir_all(root.join("tests")).expect("create fixture tests");
    root
}

fn write(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(path, body).expect("write fixture file");
}

/// A polyglot repository whose manifests name four managers and five commands.
fn write_polyglot(root: &Path) {
    write(
        root,
        "pyproject.toml",
        "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n\
         [tool.pytest.ini_options]\ntestpaths = [\"tests\"]\n",
    );
    write(
        root,
        "uv.lock",
        "version = 1\nrequires-python = \">=3.12\"\n",
    );
    write(
        root,
        "package.json",
        "{\"name\":\"fixture\",\"scripts\":{\"test\":\"jest\",\"lint\":\"eslint .\"}}\n",
    );
    write(root, "package-lock.json", "{\"lockfileVersion\":3}\n");
    write(root, "Cargo.toml", "[package]\nname = \"fixture\"\n");
    write(root, "go.mod", "module example.com/fixture\n\ngo 1.22\n");
    write(root, "src/app.py", "def app():\n    return 1\n");
    write(
        root,
        "tests/test_app.py",
        "from app import app\n\n\ndef test_app():\n    assert app() == 1\n",
    );
}

fn build_map(root: &Path) -> Value {
    let db = root.join("index.sqlite");
    let output = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(["--json", "--db"])
        .arg(&db)
        .arg("build")
        .arg(root)
        .output()
        .expect("run build");
    assert!(
        output.status.success(),
        "build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .current_dir(root)
        .args(["--json", "--db"])
        .arg(&db)
        .arg("manifest")
        .arg(root)
        .output()
        .expect("run manifest");
    assert!(
        manifest.status.success(),
        "manifest failed: stdout={} stderr={}",
        String::from_utf8_lossy(&manifest.stdout),
        String::from_utf8_lossy(&manifest.stderr)
    );
    let map_path = devmap_extract::paths::repo_map_path(root);
    serde_json::from_str(&fs::read_to_string(&map_path).expect("read repo_map.json"))
        .expect("repo_map.json parses")
}

fn strings(map: &Value, key: &str) -> Vec<String> {
    map[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be an array: {}", map[key]))
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect()
}

fn marker(map: &Value, key: &str) -> Option<bool> {
    map["meta"]["devmap_rust"][key].as_bool()
}

/// Provenance lives under `meta.devmap_rust`, beside `unavailable` and the
/// other `*_computed` markers, rather than as a new top-level key: a reader
/// that wants to know how far to trust a field already looks there.
fn provenance_strings(map: &Value, key: &str) -> Vec<String> {
    map["meta"]["devmap_rust"][key]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "meta.devmap_rust.{key} must be an array: {}",
                map["meta"]["devmap_rust"]
            )
        })
        .iter()
        .map(|entry| entry.as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn the_package_managers_a_repository_declares_are_named() {
    let root = temp_root("managers");
    write_polyglot(&root);
    let map = build_map(&root);

    let managers = strings(&map, "package_managers");
    for expected in ["npm", "uv", "cargo", "go mod"] {
        assert!(
            managers.iter().any(|name| name == expected),
            "{expected} is declared by this fixture's manifests but is not in \
             package_managers: {managers:?}"
        );
    }
}

#[test]
fn the_test_commands_the_manifests_name_are_reported() {
    let root = temp_root("commands");
    write_polyglot(&root);
    let map = build_map(&root);

    let commands = strings(&map, "test_commands");
    for expected in ["npm test", "pytest", "cargo test", "go test ./..."] {
        assert!(
            commands.iter().any(|name| name == expected),
            "{expected} is named by this fixture's manifests but is not in \
             test_commands: {commands:?}"
        );
    }
}

/// The rule `tests/unit/test_cli_commands.py:91` pins: a bare `pyproject.toml`
/// is not evidence of uv. Only the lock file is.
#[test]
fn a_bare_pyproject_is_not_evidence_of_uv() {
    let root = temp_root("bare-pyproject");
    write(
        root.as_path(),
        "pyproject.toml",
        "[project]\nname = \"sample\"\n",
    );
    write(root.as_path(), "src/app.py", "def app():\n    return 1\n");
    let map = build_map(&root);

    let managers = strings(&map, "package_managers");
    assert!(
        !managers.iter().any(|name| name == "uv"),
        "a pyproject.toml with no uv.lock beside it is not evidence of uv: \
         {managers:?}"
    );
    assert_eq!(
        marker(&map, "package_managers_computed"),
        Some(true),
        "an empty answer from a pass that ran must still say it ran"
    );
}

/// The marker and the value are one claim. A `true` marker beside a list the
/// producer never derived is the defect the markers exist to prevent, and so is
/// a `false` marker beside a derived list.
#[test]
fn the_markers_and_the_values_agree() {
    let root = temp_root("markers");
    write_polyglot(&root);
    let map = build_map(&root);

    let managers = strings(&map, "package_managers");
    let commands = strings(&map, "test_commands");
    assert_eq!(
        marker(&map, "package_managers_computed"),
        Some(true),
        "package_managers holds {managers:?}, so the marker cannot say nothing looked"
    );
    assert_eq!(
        marker(&map, "test_commands_computed"),
        Some(true),
        "test_commands holds {commands:?}, so the marker cannot say nothing looked"
    );
    assert!(
        !managers.is_empty(),
        "the marker claims a pass ran; an empty list from it would make the \
         marker the only difference between this repository and one with no \
         manifests at all"
    );
    assert!(!commands.is_empty(), "same for test_commands: {commands:?}");
}

/// A manifest too large to read is named, not silently dropped. "Could not
/// read" and "read and found nothing" must never be the same answer.
#[test]
fn an_oversized_manifest_is_reported_rather_than_skipped() {
    let root = temp_root("oversize");
    write(root.as_path(), "src/app.py", "def app():\n    return 1\n");
    // Valid JSON, well past the read cap: one enormous script value.
    let filler = "x".repeat(devmap_query::inventory::MANIFEST_READ_CAP as usize + 4_096);
    write(
        root.as_path(),
        "package.json",
        &format!("{{\"name\":\"big\",\"scripts\":{{\"test\":\"{filler}\"}}}}\n"),
    );
    let map = build_map(&root);

    let refused = provenance_strings(&map, "inventory_refused_oversize");
    assert!(
        refused.iter().any(|path| path == "package.json"),
        "a manifest past the read cap must be named as refused, not dropped: \
         {refused:?}"
    );
    // Its *declaration* is still evidence of npm — only its contents were
    // unreadable, and the two claims are separate.
    assert!(
        strings(&map, "package_managers")
            .iter()
            .any(|name| name == "npm"),
        "an unreadable package.json still declares npm"
    );
}
