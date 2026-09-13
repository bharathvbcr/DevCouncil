//! `devmap --root A paths` must describe A, not a blend of A and the caller's
//! working directory.
//!
//! The regression this pins: the handler resolved `root`, `state_dir`,
//! `repo_map`, `code_graph`, `workspace` and `plugin_dir` from the *positional*
//! argument while `cli.db()` resolved `db_path` from `--root`. One answer then
//! named two repositories at once. Hooks are exactly the caller that breaks on
//! — they pass `--root <project>` because the agent's working directory is not
//! the repository the index belongs to — and the generated agent guide opens
//! with `devmap paths --json`, so the mixed answer sent agents to another
//! checkout's map.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

/// The binary this build produced, never one found on PATH: an installed
/// `devmap` is routinely a different version from the tree under test.
const DEVMAP: &str = env!("CARGO_BIN_EXE_devmap");

/// Two sibling repositories, so "the answer came from the other one" is
/// visible rather than merely possible.
///
/// The directory name carries a pid *and* a process-local counter, not just a
/// timestamp: these tests run in parallel and macOS hands out coarse enough
/// clock readings that two nanosecond-stamped names in the same process do
/// collide.
fn two_repos(label: &str) -> (PathBuf, PathBuf, PathBuf) {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let base = std::env::temp_dir().join(format!(
        "devmap-paths-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let subject = base.join("subject");
    let caller = base.join("caller");
    for repo in [&subject, &caller] {
        std::fs::create_dir_all(repo.join("src")).expect("mkdir");
        std::fs::write(repo.join("src/lib.rs"), "pub fn a() {}\n").expect("write");
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(repo)
                .status()
                .expect("git init")
                .success(),
            "git init failed"
        );
    }
    (base, subject, caller)
}

fn paths_json(cwd: &Path, args: &[&str]) -> Value {
    let out = Command::new(DEVMAP)
        .args(args)
        .arg("--json")
        .arg("paths")
        .current_dir(cwd)
        .output()
        .expect("run devmap paths");
    assert!(
        out.status.success(),
        "paths failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("paths emitted JSON")
}

/// Every path in the answer lies under one repository.
///
/// Both spellings of the expected root are accepted. `root` is canonicalized
/// by the command while `db_path` goes through `std::path::absolute`, which
/// deliberately does not resolve symlinks so it can name a store that does not
/// exist yet. On macOS the temp directory is reached through `/var` ->
/// `/private/var`, so the two agree on the repository and differ in spelling.
/// The invariant under test is which repository answered, not how it is
/// spelled.
fn assert_all_under(answer: &Value, expected: &Path, context: &str) {
    let canonical = expected.canonicalize().expect("canonical expected root");
    let spellings = [expected.to_path_buf(), canonical];
    for key in [
        "root",
        "state_dir",
        "db_path",
        "repo_map",
        "code_graph",
        "workspace",
        "plugin_dir",
    ] {
        let raw = answer[key]
            .as_str()
            .unwrap_or_else(|| panic!("{key} missing"));
        let value = Path::new(raw);
        assert!(
            spellings.iter().any(|root| value.starts_with(root)),
            "{context}: {key} = {raw}\n  is under neither {} nor {}",
            spellings[0].display(),
            spellings[1].display()
        );
    }
}

#[test]
fn an_explicit_root_decides_every_path_not_only_the_store() {
    let (base, subject, caller) = two_repos("explicit");
    let answer = paths_json(&caller, &["--root", subject.to_str().unwrap()]);
    assert_all_under(&answer, &subject, "--root from a sibling worktree");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_hook_style_invocation_from_an_unrelated_directory_still_answers_about_the_project() {
    // A hook runs in the agent's working directory, which after a `cd` is not
    // the repository. `--root ${CLAUDE_PROJECT_DIR}` is how every generated
    // hook states which repository it means.
    let (base, subject, _caller) = two_repos("hook");
    let elsewhere = std::env::temp_dir();
    let answer = paths_json(&elsewhere, &["--root", subject.to_str().unwrap()]);
    assert_all_under(&answer, &subject, "--root from outside any repository");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn without_an_explicit_root_the_working_directory_still_decides() {
    // The fix must not make `--root` the only way to select a repository:
    // a bare `devmap paths` inside a checkout answers about that checkout.
    let (base, subject, caller) = two_repos("implicit");
    let answer = paths_json(&caller, &[]);
    assert_all_under(&answer, &caller, "no --root, cwd is the repository");
    let other = paths_json(&subject, &[]);
    assert_all_under(&other, &subject, "no --root, sibling checkout");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_positional_root_is_still_honoured() {
    // `devmap paths <dir>` predates `--root` and stays supported.
    let (base, subject, caller) = two_repos("positional");
    let out = Command::new(DEVMAP)
        .args(["--json", "paths"])
        .arg(&subject)
        .current_dir(&caller)
        .output()
        .expect("run devmap paths <dir>");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_all_under(&answer, &subject, "positional root");
    let _ = std::fs::remove_dir_all(&base);
}
