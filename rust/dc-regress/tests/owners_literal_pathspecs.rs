//! Owner lookup treats every path as a literal file name.
//!
//! A path that is pathspec magic (`:(glob)x`, `:!x`, `*.rs`) must return that
//! file's own history, never the history of whatever the magic would match.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use dc_regress::blast::{owners_for_paths_with_program, OWNER_COMMIT_CAP};

fn git_as(repo: &Path, author: &str, args: &[&str]) {
    let email = format!("{}@example.com", author.to_lowercase());
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        // Fixture setup must name the odd files literally too.
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_AUTHOR_NAME", author)
        .env("GIT_AUTHOR_EMAIL", &email)
        .env("GIT_COMMITTER_NAME", author)
        .env("GIT_COMMITTER_EMAIL", &email)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "dc-regress-literal-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

/// Each file is committed by its own author, so the owners returned for a path
/// name exactly which files' histories were read.
fn owners_of(root: &Path, path: &str) -> Vec<String> {
    let (owners, gaps) = owners_for_paths_with_program(
        std::ffi::OsStr::new("git"),
        root,
        &[path.to_string()],
        OWNER_COMMIT_CAP,
        Duration::from_secs(10),
    );
    assert!(gaps.is_empty(), "{path}: unexpected gaps {gaps:?}");
    let mut names: Vec<String> = owners.into_iter().map(|o| o.name).collect();
    names.sort();
    names
}

#[test]
fn pathspec_magic_names_are_read_as_literal_files() {
    let root = temp_root("magic");
    git_as(&root, "Ada", &["init", "-q", "--initial-branch=main"]);
    let files = [
        (":(glob)x", "Colon"),
        ("x", "Plain"),
        ("*.rs", "Star"),
        ("other.rs", "Other"),
        (":!x", "Bang"),
        ("-dash", "Dash"),
        ("sp ace/ünï cødé.txt", "Uni"),
    ];
    for (name, author) in files {
        let full = root.join(name);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, format!("{name}\n")).unwrap();
        git_as(&root, author, &["add", "--", name]);
        git_as(&root, author, &["commit", "-q", "-m", name]);
    }

    // `:(glob)x` as magic would read x's history (Plain); literally it is Colon.
    assert_eq!(owners_of(&root, ":(glob)x"), ["Colon"]);
    // `*.rs` as a glob would also match other.rs (Other).
    assert_eq!(owners_of(&root, "*.rs"), ["Star"]);
    // `:!x` as magic is an exclude-only spec, which matches everything else.
    assert_eq!(owners_of(&root, ":!x"), ["Bang"]);
    assert_eq!(owners_of(&root, "-dash"), ["Dash"]);
    assert_eq!(owners_of(&root, "sp ace/ünï cødé.txt"), ["Uni"]);
}
