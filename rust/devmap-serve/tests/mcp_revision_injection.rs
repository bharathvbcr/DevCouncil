//! A revision an MCP caller passes is never an option to git.
//!
//! `devmap_blast` and `devmap_suspects` take `since` as a free string, and it
//! reached `git diff` / `git log` as an argument of its own. Reproduced over
//! this server before the fix: `since: "--ext-diff"` switched the
//! repository's `diff.<driver>.command` back on and ran it, and
//! `since: "--output=<file>"` truncated the file — each answered
//! `isError: false` with an empty report. A prompt-injected agent could run
//! a program the repository's `.git/config` names through a read-only tool.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use devmap_serve::mcp::{handle_line, StoreSlot};
use devmap_store::Store;
use serde_json::{json, Value};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

struct Fixture {
    base: PathBuf,
    store: Arc<StoreSlot>,
    /// Appended to by the planted diff driver whenever it runs.
    marker: PathBuf,
    /// A file outside the repository that `--output=` would truncate.
    precious: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

const PRECIOUS: &str = "not to be truncated\n";

fn git(root: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Two commits of a small Python corpus, indexed at the second, with a diff
/// driver and `diff.external` in `.git/config` that both name a marker
/// script, and an uncommitted edit so a diff against the working tree has
/// something to hand the driver.
fn fixture(label: &str) -> Fixture {
    let base = std::env::temp_dir().join(format!(
        "devmap-rev-injection-{label}-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let marker = base.join("ran");
    let precious = base.join("precious.txt");
    std::fs::write(&precious, PRECIOUS).unwrap();
    let script = base.join("planted.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\necho \"$*\" >> '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

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
    let after = "def helper(rows):\n    return sum(rows) + 1\n";
    std::fs::write(root.join("core.py"), after).unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "change", "--no-gpg-sign"]);
    let head = git(&root, &["rev-parse", "HEAD"]);

    let script = script.to_str().unwrap();
    git(&root, &["config", "diff.evil.command", script]);
    git(&root, &["config", "diff.external", script]);
    std::fs::write(root.join(".git/info/attributes"), "*.py diff=evil\n").unwrap();
    std::fs::write(
        root.join("core.py"),
        "def helper(rows):\n    return sum(rows) + 2\n",
    )
    .unwrap();

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
    Fixture {
        base,
        store: Arc::new(StoreSlot::ready("rev-injection", Arc::new(store))),
        marker,
        precious,
    }
}

async fn call(store: &Arc<StoreSlot>, tool: &str, arguments: Value) -> Value {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}
    });
    let response = handle_line(store, &frame.to_string())
        .await
        .expect("a request must produce a response frame");
    response["result"].clone()
}

fn assert_refused_and_harmless(f: &Fixture, tool: &str, since: &str, result: &Value) {
    if let Ok(ran) = std::fs::read_to_string(&f.marker) {
        panic!("{tool} since={since:?} ran a program the repository names:\n{ran}");
    }
    assert_eq!(
        std::fs::read_to_string(&f.precious).unwrap(),
        PRECIOUS,
        "{tool} since={since:?} wrote over a file outside the repository"
    );
    assert_eq!(
        result["isError"],
        json!(true),
        "{tool} since={since:?} must be refused, not answered: {result}"
    );
    let text = result.to_string();
    assert!(
        text.contains("begins with '-'"),
        "{tool} since={since:?}: the refusal must say why: {result}"
    );
}

fn hostile_revisions(f: &Fixture) -> Vec<String> {
    vec![
        "--ext-diff".to_string(),
        format!("--output={}", f.precious.display()),
    ]
}

#[tokio::test]
async fn blast_refuses_a_since_that_is_an_option() {
    let f = fixture("blast");
    for since in hostile_revisions(&f) {
        let result = call(&f.store, "devmap_blast", json!({"since": since})).await;
        assert_refused_and_harmless(&f, "devmap_blast", &since, &result);
    }
}

#[tokio::test]
async fn suspects_refuses_a_since_that_is_an_option() {
    let f = fixture("suspects");
    for since in hostile_revisions(&f) {
        let result = call(
            &f.store,
            "devmap_suspects",
            json!({"symptom": "run", "since": since}),
        )
        .await;
        assert_refused_and_harmless(&f, "devmap_suspects", &since, &result);
    }
}

/// The refusal is of options, not of revision syntax: a relative revision
/// still answers, and still runs nothing the repository names.
#[tokio::test]
async fn a_relative_revision_still_answers() {
    let f = fixture("relative");
    let result = call(&f.store, "devmap_blast", json!({"since": "HEAD~1"})).await;
    assert_eq!(result["isError"], json!(false), "{result}");
    let text = result.to_string();
    assert!(
        text.contains("core.py"),
        "the change is to core.py: {result}"
    );
    assert!(
        !f.marker.exists(),
        "blast ran a program the repository names: {}",
        std::fs::read_to_string(&f.marker).unwrap_or_default()
    );
}
