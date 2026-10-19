//! A cancelled request stops the analysis it abandoned.
//!
//! The request timeout answers the client and trips the request's `Cancel`;
//! the `spawn_blocking` task it left behind cannot be aborted. Before the
//! analysis consulted that token, a `blast` or `suspects` that outlived its
//! 30 s timeout kept spawning git for a client that had gone.

use std::path::{Path, PathBuf};
use std::process::Command;

use devmap_store::Store;
use serde_json::{json, Value};

use crate::protocol::{dispatch, IpcRequest, UnappliedEdits};

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    base: PathBuf,
    store: Store,
    since: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn fixture() -> Fixture {
    // The tests in this file run in parallel in one process: a directory named
    // by pid alone is one tree both of them remove and `git init` at once.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let base = std::env::temp_dir().join(format!("devmap-serve-cancel-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--initial-branch=main"]);
    let caller = "from core import helper\n\n\ndef run(rows):\n    return helper(rows)\n";
    std::fs::write(
        root.join("core.py"),
        "def helper(rows):\n    return sum(rows)\n",
    )
    .unwrap();
    std::fs::write(root.join("caller.py"), caller).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "base", "--no-gpg-sign"]);
    let since = git(&root, &["rev-parse", "HEAD"]);
    let after = "def helper(rows):\n    return sum(rows) + 1\n";
    std::fs::write(root.join("core.py"), after).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "change", "--no-gpg-sign"]);
    let head = git(&root, &["rev-parse", "HEAD"]);

    let sources = [("core.py", after), ("caller.py", caller)];
    let extractions: Vec<_> = sources
        .iter()
        .map(|(path, source)| devmap_extract::extract_file(path, source))
        .collect();
    let mut resolver = devmap_resolve::Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze(&extractions, &resolution);
    let store = Store::open_in_memory().expect("in-memory store");
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            devmap_store::GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .expect("generation");
    store.restamp_latest_head(&head).expect("head");
    Fixture { base, store, since }
}

fn ask(f: &Fixture, cancel: &devmap_query::Cancel, request: Value) -> Value {
    let request: IpcRequest = serde_json::from_value(request).expect("a request");
    dispatch(&f.store, request, cancel, &UnappliedEdits::default()).expect("an answer")
}

fn exhausted(report: &Value) -> bool {
    report["unavailable"]
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["kind"] == "budget_exhausted"))
}

#[test]
fn a_cancelled_blast_says_it_was_cut_short() {
    let f = fixture();
    let live = ask(
        &f,
        &devmap_query::Cancel::new(),
        json!({"version": 1, "cmd": "blast", "since": f.since}),
    );
    assert!(
        !exhausted(&live),
        "an uncancelled request is not cut short: {live:#}"
    );
    assert!(
        live["changed_files"]
            .as_array()
            .is_some_and(|r| !r.is_empty()),
        "{live:#}"
    );

    let cancel = devmap_query::Cancel::new();
    cancel.cancel();
    let report = ask(
        &f,
        &cancel,
        json!({"version": 1, "cmd": "blast", "since": f.since}),
    );
    assert!(exhausted(&report), "{report:#}");
    assert_eq!(report["complete"], json!(false), "{report:#}");
    assert!(
        report["seeds"].as_array().unwrap().is_empty(),
        "no file was read after the cancel: {report:#}"
    );
}

#[test]
fn a_cancelled_suspects_says_it_was_cut_short() {
    let f = fixture();
    let cancel = devmap_query::Cancel::new();
    cancel.cancel();
    let report = ask(
        &f,
        &cancel,
        json!({"version": 1, "cmd": "suspects", "symptom": "run", "since": f.since}),
    );
    assert!(exhausted(&report), "{report:#}");
    assert_eq!(report["complete"], json!(false), "{report:#}");
}

// ---- the store graph's basis reads through the program it was given ----

use dc_regress::blast::{blast_change_with_program, UnattributedReason};
use dc_regress::{BlobIdentity, CodeGraph};
use dc_regress_store::StoreGraph;
use std::os::unix::fs::PermissionsExt;

fn script(f: &Fixture, name: &str, body: &str) -> PathBuf {
    let path = f.base.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn real_git() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .expect("sh");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn the_graphs_basis_is_read_through_the_injected_program() {
    let f = fixture();
    let log = f.base.join("spawns.log");
    let counting = script(
        &f,
        "counting-git",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexec '{}' \"$@\"\n",
            log.display(),
            real_git()
        ),
    );
    let repo = f.base.join("repo");
    let graph = StoreGraph::new(&f.store, &repo)
        .expect("graph")
        .with_program(counting.as_os_str());
    let (_, basis) = graph.symbols_in("core.py");
    assert!(
        matches!(basis, BlobIdentity::Blob(_)),
        "the basis resolved: {basis:?}"
    );
    let spawned = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        spawned.contains("cat-file"),
        "the basis was asked of the injected program, not of PATH's git: {spawned:?}"
    );
}

#[test]
fn a_basis_that_could_not_be_read_is_unavailable_content_not_a_mismatch() {
    let f = fixture();
    let repo = f.base.join("repo");
    let change = dc_regress::change::changes_between(&repo, &f.since, "HEAD").expect("change");
    let broken = script(&f, "broken-git", "#!/bin/sh\nexit 1\n");
    let graph = StoreGraph::new(&f.store, &repo)
        .expect("graph")
        .with_program(broken.as_os_str());
    let (_, basis) = graph.symbols_in("core.py");
    assert!(
        matches!(basis, BlobIdentity::Unreadable { .. }),
        "a git that cannot answer is not an unrecorded basis: {basis:?}"
    );
    let report = blast_change_with_program(
        broken.as_os_str(),
        &repo,
        &graph,
        &change,
        "test",
        "HEAD",
        3,
    );
    assert!(
        report
            .unattributed
            .iter()
            .any(|u| matches!(u.reason, UnattributedReason::ContentUnavailable { .. })),
        "{report:#?}"
    );
    assert!(
        !report
            .unattributed
            .iter()
            .any(|u| matches!(u.reason, UnattributedReason::BasisMismatch { .. })),
        "a transient read failure is never reported as a basis mismatch: {report:#?}"
    );
}
