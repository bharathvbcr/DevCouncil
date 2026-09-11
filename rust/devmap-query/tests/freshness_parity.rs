//! The kernel's freshness digests must equal Python's, string for string.
//!
//! `RepoMapper.map_is_stale` compares `generated_head` / `indexed_hash` /
//! `content_fingerprint` against values it recomputes in Python, at twelve call
//! sites. The moment the kernel stamps those fields itself, "equivalent" is not
//! good enough: a digest that differs by one byte makes every map the kernel
//! writes read stale forever, `--if-stale` never short-circuits, and the watcher
//! rebuilds in a loop. So the two implementations are run over a real tree and
//! their answers compared directly.
//!
//! Skipped, loudly, when no interpreter with `devcouncil` importable can be
//! found — a check that could not run must never report what a check that ran
//! and passed reports, so the skip prints its reason and the test name says what
//! it would have proved.

use std::path::{Path, PathBuf};
use std::process::Command;

use devmap_query::freshness::{
    content_fingerprint, files_fingerprint, git_head, inventory, InventoryLimits, InventorySource,
};

/// The checkout this crate is built from: `devmap-query` → `rust` → repo root.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crate is nested under <root>/rust/<crate>")
        .to_path_buf()
}

/// An interpreter that can import `devcouncil`.
///
/// `DEVMAP_PARITY_PYTHON` wins. Otherwise `<root>/.venv`, and then the venv of
/// the main checkout — in a git worktree the environment lives beside the
/// primary checkout, which `--git-common-dir` names, rather than in the tree
/// being tested.
fn python_interpreter(root: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(explicit) = std::env::var("DEVMAP_PARITY_PYTHON") {
        if !explicit.trim().is_empty() {
            candidates.push(PathBuf::from(explicit));
        }
    }
    candidates.push(root.join(".venv/bin/python"));
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .current_dir(root)
        .output()
    {
        if output.status.success() {
            let common = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(main_checkout) = Path::new(&common).parent() {
                candidates.push(main_checkout.join(".venv/bin/python"));
            }
        }
    }

    candidates.into_iter().find(|candidate| {
        candidate.is_file()
            && Command::new(candidate)
                .arg("-c")
                .arg("import devcouncil")
                .env("PYTHONPATH", root.join("src"))
                .output()
                .map(|out| out.status.success())
                .unwrap_or(false)
    })
}

const PYTHON_PROBE: &str = r#"
import json, sys
from pathlib import Path
from devcouncil.indexing.repo_mapper import RepoMapper

root = Path(sys.argv[1])
# persist_content_cache=False: a parity check must not modify the tree it
# measures, and the memo is advisory in both directions anyway.
mapper = RepoMapper(root, persist_content_cache=False)
files = mapper.get_git_files()
json.dump(
    {
        "head": mapper._git_head(),
        "indexed_hash": mapper._files_fingerprint(files),
        "content_fingerprint": mapper._content_fingerprint(files),
        "files": files,
    },
    sys.stdout,
)
"#;

struct PythonAnswer {
    head: String,
    indexed_hash: String,
    content_fingerprint: String,
    files: Vec<String>,
}

fn ask_python(python: &Path, root: &Path, tree: &Path) -> PythonAnswer {
    let output = Command::new(python)
        .arg("-c")
        .arg(PYTHON_PROBE)
        .arg(tree)
        .env("PYTHONPATH", root.join("src"))
        // The seam refuses to start a daemon during a probe; the same rule
        // applies to a test that shells into it.
        .env("DEVMAP_AUTOSPAWN", "0")
        .output()
        .expect("the interpreter runs");
    assert!(
        output.status.success(),
        "python probe failed on {}: {}",
        tree.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the probe emits one JSON document");
    PythonAnswer {
        head: value["head"].as_str().unwrap_or_default().to_string(),
        indexed_hash: value["indexed_hash"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        content_fingerprint: value["content_fingerprint"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        files: value["files"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn compare_tree(python: &Path, root: &Path, tree: &Path) {
    let expected = ask_python(python, root, tree);
    let mine = inventory(tree, InventoryLimits::default());
    assert_eq!(
        mine.source,
        InventorySource::Git,
        "the kernel could not enumerate {} while Python could",
        tree.display()
    );

    // The file *set* first: a fingerprint mismatch on differing inventories
    // says nothing about the digest, and the difference itself is the finding.
    if mine.files != expected.files {
        let missing: Vec<&String> = expected
            .files
            .iter()
            .filter(|path| !mine.files.contains(path))
            .take(10)
            .collect();
        let extra: Vec<&String> = mine
            .files
            .iter()
            .filter(|path| !expected.files.contains(path))
            .take(10)
            .collect();
        panic!(
            "inventory differs on {} ({} kernel vs {} python)\n  python has, kernel lacks: {missing:?}\n  kernel has, python lacks: {extra:?}",
            tree.display(),
            mine.files.len(),
            expected.files.len(),
        );
    }

    assert_eq!(
        git_head(tree),
        expected.head,
        "generated_head differs on {}",
        tree.display()
    );
    assert_eq!(
        files_fingerprint(&mine.files),
        expected.indexed_hash,
        "indexed_hash differs on {}",
        tree.display()
    );
    assert_eq!(
        content_fingerprint(tree, &mine.files, false),
        expected.content_fingerprint,
        "content_fingerprint differs on {}",
        tree.display()
    );
}

/// The trees to check: this repository, plus anything named in
/// `DEVMAP_PARITY_TREES` (`:`-separated absolute paths — the benchmark corpora
/// when a lane has them checked out).
fn parity_trees(root: &Path) -> Vec<PathBuf> {
    let mut trees = vec![root.to_path_buf()];
    if let Ok(extra) = std::env::var("DEVMAP_PARITY_TREES") {
        for entry in extra.split(':').filter(|entry| !entry.trim().is_empty()) {
            let path = PathBuf::from(entry);
            if path.is_dir() {
                trees.push(path);
            } else {
                eprintln!("DEVMAP_PARITY_TREES names {entry}, which is not a directory — skipped");
            }
        }
    }
    trees
}

#[test]
fn the_kernel_and_python_agree_on_all_three_freshness_digests() {
    let root = repo_root();
    let Some(python) = python_interpreter(&root) else {
        eprintln!(
            "SKIPPED the_kernel_and_python_agree_on_all_three_freshness_digests: no interpreter \
             with `devcouncil` importable (tried DEVMAP_PARITY_PYTHON, {}/.venv, and the main \
             checkout's .venv). The kernel's freshness stamping is UNVERIFIED in this run.",
            root.display()
        );
        return;
    };
    for tree in parity_trees(&root) {
        compare_tree(&python, &root, &tree);
    }
}

/// Parity has to survive the cases a whole-tree comparison never reaches: an
/// empty repository, a path git quotes without `-z`, a file whose name is a
/// generated-directory name, and an untracked file that appears between runs.
#[test]
fn the_two_agree_on_an_adversarial_tree() {
    let root = repo_root();
    let Some(python) = python_interpreter(&root) else {
        eprintln!(
            "SKIPPED the_two_agree_on_an_adversarial_tree: no interpreter with `devcouncil` \
             importable. Adversarial freshness parity is UNVERIFIED in this run."
        );
        return;
    };

    let tree = std::env::temp_dir().join(format!(
        "devmap-parity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tree).unwrap();
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(&tree)
            .output()
            .expect("git runs");
        assert!(
            status.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "parity@example.invalid"]);
    git(&["config", "user.name", "parity"]);

    // A repository with an inventory but no commit: `generated_head` is
    // genuinely unknown and both sides must say so the same way.
    std::fs::write(tree.join("a.py"), b"def a():\n    return 1\n").unwrap();
    compare_tree(&python, &root, &tree);

    std::fs::create_dir_all(tree.join("src/nested")).unwrap();
    // Non-ASCII, a space, and a quote-worthy name: without `-z` git C-quotes
    // these and the two sides would see different strings.
    std::fs::write(tree.join("src/café.py"), b"CAFE = 1\n").unwrap();
    std::fs::write(tree.join("src/two words.py"), b"X = 2\n").unwrap();
    std::fs::write(tree.join("src/nested/\"quoted\".py"), b"Q = 3\n").unwrap();
    // Excluded by name, by suffix, by top-level prefix, and by segment.
    std::fs::write(tree.join("src/nested/vendor"), b"not source\n").unwrap();
    std::fs::write(tree.join("src/logo.PNG"), b"\x89PNG\r\n").unwrap();
    std::fs::create_dir_all(tree.join("dist")).unwrap();
    std::fs::write(tree.join("dist/bundle.js"), b"//\n").unwrap();
    std::fs::create_dir_all(tree.join("node_modules/left-pad")).unwrap();
    std::fs::write(tree.join("node_modules/left-pad/index.js"), b"//\n").unwrap();
    // An empty file, and one big enough to cross the streaming chunk boundary.
    std::fs::write(tree.join("empty.py"), b"").unwrap();
    std::fs::write(tree.join("big.py"), vec![b'#'; (1 << 20) + 7]).unwrap();
    compare_tree(&python, &root, &tree);

    git(&["add", "-A"]);
    git(&["commit", "-qm", "parity"]);
    compare_tree(&python, &root, &tree);

    // An untracked file joins the inventory; a tracked file deleted from the
    // working tree leaves it.
    std::fs::write(tree.join("untracked.py"), b"U = 1\n").unwrap();
    std::fs::remove_file(tree.join("a.py")).unwrap();
    compare_tree(&python, &root, &tree);

    // …and with untracked files excluded, both sides drop it again.
    let limits = InventoryLimits {
        include_untracked: false,
        max_indexed_files: 50_000,
    };
    let tracked_only = inventory(&tree, limits);
    assert!(
        !tracked_only.files.iter().any(|path| path == "untracked.py"),
        "include_untracked=false must drop untracked paths: {:?}",
        tracked_only.files
    );

    let _ = std::fs::remove_dir_all(&tree);
}
