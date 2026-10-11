//! The repository's .git/config does not choose what devmap's git reads run.
#![cfg(unix)]

// A repository whose .git/config points every program-bearing setting a read
// can reach at one marker script, the way a .git/config that arrived with the
// repository could. The script prints nothing and exits 0.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

struct Planted {
    root: PathBuf,
    marker: PathBuf,
}

impl Drop for Planted {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn planted(label: &str) -> Planted {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!(
        "planted-git-config-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let marker = base.join("ran");
    let script = base.join("planted.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"$*\" >> '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    git(&root, &["init", "--initial-branch=main"]);
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "base", "--no-gpg-sign"]);
    // A commit carrying a signature header, so log.showSignature has
    // something to verify.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    std::fs::write(root.join("src/lib.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
    git(&root, &["add", "-A"]);
    let tree = git(&root, &["write-tree"]);
    let parent = git(&root, &["rev-parse", "HEAD"]);
    let object = format!(
        "tree {tree}\nparent {parent}\nauthor Ada <ada@example.com> {now} +0000\n\
         committer Ada <ada@example.com> {now} +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n iQEz\n -----END PGP SIGNATURE-----\n\nsigned\n"
    );
    let object_file = base.join("commit-object");
    std::fs::write(&object_file, object).unwrap();
    let id = git(
        &root,
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            object_file.to_str().unwrap(),
        ],
    );
    git(&root, &["update-ref", "refs/heads/main", &id]);

    let script = script.to_str().unwrap();
    for (key, value) in [
        ("core.fsmonitor", script),
        ("diff.external", script),
        ("diff.planted.textconv", script),
        ("log.showSignature", "true"),
        ("gpg.program", script),
        ("filter.planted.clean", script),
        ("filter.planted.required", "true"),
    ] {
        git(&root, &["config", key, value]);
    }
    std::fs::write(
        root.join(".git/info/attributes"),
        "* diff=planted filter=planted\n",
    )
    .unwrap();
    Planted { root, marker }
}

fn assert_nothing_ran(p: &Planted) {
    if let Ok(ran) = std::fs::read_to_string(&p.marker) {
        panic!("a program named in .git/config ran during a read:\n{ran}");
    }
}

use devmap_query::freshness::{inventory_with_program, InventoryLimits};
use devmap_query::inventory::churn_with_program;
use std::ffi::OsStr;

#[test]
fn churn_does_not_verify_signatures_with_the_repository_s_gpg_program() {
    let p = planted("churn");
    let churn = churn_with_program(OsStr::new("git"), &p.root);
    assert_nothing_ran(&p);
    assert!(churn.computed, "churn: {}", churn.unavailable_reason);
    assert!(
        churn.commits_by_path.contains_key("src/lib.rs"),
        "{:?}",
        churn.commits_by_path
    );
}

#[test]
fn the_inventory_does_not_run_the_repository_s_fsmonitor() {
    let p = planted("inventory");
    let inventory = inventory_with_program(OsStr::new("git"), &p.root, InventoryLimits::default());
    assert_nothing_ran(&p);
    assert!(inventory.is_available(), "{inventory:?}");
    assert!(
        inventory.files.iter().any(|f| f == "src/lib.rs"),
        "{:?}",
        inventory.files
    );
}
