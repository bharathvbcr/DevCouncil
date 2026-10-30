//! `complete` on a ranking is `no ceiling AND walk_errors == 0 AND
//! lexical_unindexed == 0`, and each way a rankable file can be left out of the
//! index is a counted shortfall rather than a silent `continue`.
//!
//! Through the real binary, one fixture per class that can be constructed on
//! this platform. Not constructed here, and why: a file that changes between
//! the two stat calls around its read (a race), a read that fails after a
//! successful open, an untrusted version stamp, and a file name that is not
//! UTF-8 (the filesystem on macOS refuses to create one). Each of those paths
//! bumps the same counter as the classes below.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

mod support;

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dcgrep-shortfall-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fresh fixture");
        Self(root)
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        let full = self.0.join(path);
        fs::create_dir_all(full.parent().unwrap()).expect("fixture parent");
        fs::write(full, contents).expect("fixture file");
    }

    fn call(&self, command: &str, mut request: Value) -> (bool, Value) {
        request["root"] = json!(self.0);
        let (ok, stdout) = support::run(
            command,
            request.to_string().as_bytes(),
            Duration::from_secs(120),
        );
        let reply: Value = serde_json::from_slice(&stdout)
            .unwrap_or_else(|err| panic!("{command}: no JSON reply ({err}): {stdout:?}"));
        (ok, reply)
    }

    /// Builds the index and ranks one query, returning both replies.
    fn built_and_ranked(&self, query: &str) -> (Value, Value) {
        let (ok, built) = self.call("index", json!({}));
        assert!(ok, "index must succeed: {built}");
        let (ok, ranked) = self.call("rank", json!({ "query": query }));
        assert!(ok, "rank must succeed: {ranked}");
        (built, ranked)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        fn unlock(dir: &std::path::Path) {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
            if let Ok(entries) = fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o755));
                    if path.is_dir() {
                        unlock(&path);
                    }
                }
            }
        }
        unlock(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_clean_tree_is_complete_with_both_counts_reported_as_zero() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write("b.rs", "fn visible_badger() {}\n");
    let (built, ranked) = repo.built_and_ranked("visible_alpaca");
    assert_eq!(built["lexical_unindexed"], 0, "{built}");
    assert_eq!(ranked["complete"], true, "{ranked}");
    assert_eq!(ranked["walk_errors"], 0, "{ranked}");
    assert_eq!(ranked["lexical_unindexed"], 0, "{ranked}");
    assert_eq!(ranked["lexical_limit_reason"], Value::Null, "{ranked}");
}

#[test]
fn an_unreadable_file_is_counted_not_skipped() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write("b.rs", "fn hidden_badger() {}\n");
    fs::set_permissions(repo.0.join("b.rs"), fs::Permissions::from_mode(0o000)).unwrap();
    let (built, ranked) = repo.built_and_ranked("visible_alpaca");
    assert_eq!(built["lexical_unindexed"], 1, "{built}");
    assert_eq!(ranked["lexical_unindexed"], 1, "{ranked}");
    // An unreadable file is not a walk error: the walk saw it.
    assert_eq!(ranked["walk_errors"], 0, "{ranked}");
    assert_eq!(ranked["complete"], false, "{ranked}");
    assert_eq!(ranked["lexical_limit_reason"], Value::Null, "{ranked}");
}

#[test]
fn an_unreadable_directory_is_a_walk_error_and_nothing_else() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write("locked/b.rs", "fn hidden_badger() {}\n");
    fs::set_permissions(repo.0.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let (_, ranked) = repo.built_and_ranked("visible_alpaca");
    assert!(ranked["walk_errors"].as_u64().unwrap() >= 1, "{ranked}");
    assert_eq!(ranked["lexical_unindexed"], 0, "{ranked}");
    assert_eq!(ranked["complete"], false, "{ranked}");
}

#[test]
fn a_file_over_the_read_bound_is_counted() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write(
        "huge.txt",
        format!("{}needlezebra\n", "word ".repeat(500_000)),
    );
    let (built, ranked) = repo.built_and_ranked("needlezebra");
    assert_eq!(built["lexical_unindexed"], 1, "{built}");
    assert_eq!(ranked["count"], 0, "{ranked}");
    assert_eq!(ranked["lexical_unindexed"], 1, "{ranked}");
    assert_eq!(ranked["complete"], false, "{ranked}");
}

#[test]
fn a_file_over_the_term_cap_is_counted() {
    let repo = Repo::new();
    let mut big = String::new();
    // Letters only, so each is one term: digits split a token.
    for n in 0..21_000u32 {
        let word: String = (0..6)
            .map(|digit| (b'a' + ((n / 26u32.pow(digit)) % 26) as u8) as char)
            .collect();
        big.push_str(&word);
        big.push(' ');
    }
    repo.write("big.txt", &big);
    repo.write("small.rs", "fn small_alpaca() {}\n");
    let (_, ranked) = repo.built_and_ranked("small_alpaca");
    assert_eq!(ranked["lexical_unindexed"], 1, "{ranked}");
    assert_eq!(ranked["complete"], false, "{ranked}");
}

#[test]
fn binary_and_non_utf8_files_are_not_a_shortfall() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write("blob.bin", [0u8, 1, 2, 3, 0, 255, 254]);
    repo.write("latin1.txt", [b'c', b'a', b'f', 0xe9, b' ', b'x', b'y']);
    let (built, ranked) = repo.built_and_ranked("visible_alpaca");
    assert_eq!(built["lexical_unindexed"], 0, "{built}");
    assert_eq!(ranked["files_seen"], 3, "{ranked}");
    assert_eq!(ranked["complete"], true, "{ranked}");
}

#[test]
fn a_file_missing_from_the_encoding_is_counted() {
    let repo = Repo::new();
    repo.write("src/alpha.rs", "fn alpha() { let x = 1; }\n");
    repo.write("src/gamma.rs", "fn gamma() { let z = 3; }\n");
    let header = json!({
        "schema": 1,
        "model": "fixture-encoder",
        "vocabulary": "wordpiece-30522",
        "vocab": ["[UNK]", "parse", "json"],
        "query_weights": [0.0, 2.0, 3.0],
        "parity": [{"text": "parse", "ids": [1]}, {"text": "json", "ids": [2]}],
    });
    let text = format!(
        "{header}\n{}\n",
        json!({ "path": "src/alpha.rs", "total_terms": 10, "terms": [[1, 0.9]] })
    );
    let encoding = repo.0.with_extension("jsonl");
    fs::write(&encoding, text).unwrap();
    let (ok, built) = repo.call("index", json!({ "sparse": encoding }));
    let _ = fs::remove_file(&encoding);
    assert!(ok, "{built}");
    assert_eq!(built["lexical_unindexed"], 1, "{built}");
    let (ok, ranked) = repo.call("rank", json!({ "query": "parse" }));
    assert!(ok, "{ranked}");
    assert_eq!(ranked["lexical_unindexed"], 1, "{ranked}");
    assert_eq!(ranked["complete"], false, "{ranked}");
}

/// A ceiling and a shortfall together: both are reported, neither hides the
/// other.
#[test]
fn a_ceiling_and_a_shortfall_are_both_reported() {
    let repo = Repo::new();
    for n in 0..4 {
        repo.write(&format!("f{n}.rs"), format!("fn word{n}_unique() {{}}\n"));
    }
    repo.write("huge.txt", "word ".repeat(500_000));
    let (ok, built) = repo.call("index", json!({ "max_files": 3 }));
    assert!(ok, "{built}");
    let (ok, ranked) = repo.call("rank", json!({ "query": "word0_unique" }));
    assert!(ok, "{ranked}");
    assert_eq!(ranked["complete"], false, "{ranked}");
    assert_eq!(ranked["lexical_limit_reason"], "files", "{ranked}");
}
