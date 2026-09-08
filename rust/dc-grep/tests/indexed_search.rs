use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dcgrep-index-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("fresh fixture");
        Self(root)
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn call(&self, command: &str, mut request: Value) -> (bool, Value) {
        request["root"] = json!(self.0);
        let mut child = Command::new(env!("CARGO_BIN_EXE_dcgrep"))
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let reply = serde_json::from_slice(&output.stdout).expect("one JSON reply");
        (output.status.success(), reply)
    }

    fn index(&self) -> Value {
        let (ok, reply) = self.call("index", json!({}));
        assert!(ok, "index command must succeed: {reply}");
        assert_eq!(reply["engine"], "tgrep-core");
        reply
    }

    fn search(&self, pattern: &str) -> Value {
        let (ok, reply) = self.call("search", json!({"pattern": pattern}));
        assert!(ok, "search must succeed: {reply}");
        reply
    }

    fn slot(&self) -> PathBuf {
        let cache = self.0.join(".devcouncil/dcgrep");
        let current: Value =
            serde_json::from_slice(&fs::read(cache.join("current.json")).unwrap()).unwrap();
        cache.join(current["slot"].as_str().unwrap())
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove fixture");
    }
}

#[test]
fn indexed_search_filters_unchanged_files_without_changing_matches() {
    let repo = Repo::new();
    repo.write("hit.rs", b"fn needle() {}\n");
    for n in 0..24 {
        repo.write(&format!("miss{n}.rs"), b"fn unrelated() {}\n");
    }
    let before = repo.search("needle");
    assert_eq!(before["files_searched"], 25);
    let built = repo.index();
    assert_eq!(built["files_indexed"], 25);
    let after = repo.search("needle");
    assert_eq!(after["matches"], before["matches"]);
    #[cfg(unix)]
    {
        assert_eq!(after["index"]["status"], "used");
        assert_eq!(after["index"]["files_filtered"], 24);
        assert_eq!(after["files_searched"], 1);
    }
}

#[test]
fn edits_new_files_deletions_and_ignore_changes_remain_visible() {
    let repo = Repo::new();
    repo.write("old.rs", "no match\n");
    repo.write("gone.rs", "needle\n");
    repo.write(".gitignore", "ignored.rs\n");
    repo.write("ignored.rs", "needle\n");
    repo.index();
    repo.write("old.rs", "needle\n");
    repo.write("new.rs", "needle\n");
    fs::remove_file(repo.0.join("gone.rs")).unwrap();
    repo.write(".gitignore", "new.rs\n");
    let result = repo.search("needle");
    assert_eq!(result["count"], 2, "{result}");
    let mut paths: Vec<_> = result["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["path"].as_str().unwrap())
        .collect();
    paths.sort();
    assert_eq!(paths, ["ignored.rs", "old.rs"]);
}

#[test]
fn regexes_and_non_utf8_files_keep_live_search_semantics() {
    let repo = Repo::new();
    repo.write("text.rs", "foo bar Straße Kelvin Äpple\n");
    repo.write("bytes.txt", b"foo\xffbar\n");
    repo.write("binary.dat", b"foo\0bar\n");
    let patterns = [
        "foo|bar",
        "foo.*bar",
        "[fb]",
        ".*",
        "(?i)kelvin",
        "(?-u:foo\\xFFbar)",
        "Straße",
        "foo?",
        "(?:foo|missing)",
        "\\bbar\\b",
    ];
    let before: Vec<_> = patterns.iter().map(|p| repo.search(p)).collect();
    repo.index();
    for (pattern, expected) in patterns.iter().zip(before) {
        let actual = repo.search(pattern);
        assert_eq!(
            actual["matches"], expected["matches"],
            "{pattern}: {actual}"
        );
        assert_eq!(
            actual["skipped"], expected["skipped"],
            "{pattern}: {actual}"
        );
    }
    let (ok, invalid) = repo.call("search", json!({"pattern":"["}));
    assert!(!ok);
    assert_eq!(invalid["ok"], false);
}

#[cfg(unix)]
#[test]
fn restoring_mtime_does_not_hide_a_same_size_edit() {
    let repo = Repo::new();
    repo.write("edit.rs", "absent\n");
    let path = repo.0.join("edit.rs");
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    repo.index();
    repo.write("edit.rs", "needle\n");
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let result = repo.search("needle");
    assert_eq!(result["count"], 1, "{result}");
    assert_eq!(result["index"]["files_stale"], 1);
}

#[test]
fn capped_index_covers_only_its_sample_and_searches_every_other_file() {
    let repo = Repo::new();
    for n in 0..10 {
        repo.write(&format!("file{n}.rs"), "needle\n");
    }
    let (ok, built) = repo.call("index", json!({"max_files":2}));
    assert!(ok, "{built}");
    assert_eq!(built["files_indexed"], 2);
    assert_eq!(built["traversal_complete"], false);
    assert_eq!(built["limit_reason"], "files");
    assert!(built["files_seen"].as_u64().unwrap() > 2);
    assert_eq!(repo.search("needle")["count"], 10);
    let (ok, error) = repo.call("index", json!({"max_files":50_001}));
    assert!(!ok);
    assert!(error["error"].as_str().unwrap().contains("ceiling"));
}

#[test]
fn corrupt_or_missing_index_files_fall_back_and_rebuild_recovers() {
    let repo = Repo::new();
    repo.write("hit.rs", "needle\n");
    repo.write("miss.rs", "other\n");
    for name in [
        "index.bin",
        "lookup.bin",
        "files.bin",
        "filestamps.json",
        "meta.json",
    ] {
        repo.index();
        fs::write(repo.slot().join(name), b"broken").unwrap();
        let result = repo.search("needle");
        assert_eq!(result["count"], 1, "{name}: {result}");
        assert_eq!(result["index"]["status"], "scan");
        assert!(result["index"]["reason"].as_str().unwrap().len() > 5);
    }
    repo.index();
    fs::remove_file(repo.slot().join("files.bin")).unwrap();
    assert_eq!(repo.search("needle")["count"], 1);
    repo.index();
    #[cfg(unix)]
    assert_eq!(repo.search("needle")["index"]["files_filtered"], 1);
}

#[test]
fn writer_contention_falls_back_without_blocking_or_publishing() {
    let repo = Repo::new();
    repo.write("hit.rs", "needle\n");
    repo.index();
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .open(repo.0.join(".devcouncil/dcgrep/cache.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let started = std::time::Instant::now();
    let result = repo.search("needle");
    assert_eq!(result["count"], 1);
    assert_eq!(result["index"]["status"], "scan");
    let (ok, failure) = repo.call("index", json!({}));
    assert!(!ok);
    assert!(failure["error"].as_str().unwrap().contains("busy"));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    drop(lock);
    repo.index();
}

#[test]
fn repeated_builds_recycle_two_slots_and_preserve_search() {
    let repo = Repo::new();
    for n in 0..5 {
        repo.write("hit.rs", format!("needle{n}\n"));
        repo.index();
        assert_eq!(repo.search(&format!("needle{n}"))["count"], 1);
    }
    let entries: Vec<_> = fs::read_dir(repo.0.join(".devcouncil/dcgrep"))
        .unwrap()
        .collect();
    assert_eq!(entries.len(), 4, "only two slots, pointer, and lock");
}

#[test]
fn empty_index_and_encoding_fallbacks_preserve_live_results() {
    let repo = Repo::new();
    assert_eq!(repo.index()["files_indexed"], 0);
    assert_eq!(repo.search("needle")["count"], 0);
    repo.write("utf8.txt", b"\xef\xbb\xbfneedle\n");
    repo.write("utf16.txt", b"\xff\xfen\0e\0e\0d\0l\0e\0\n\0");
    repo.write("bytes.txt", b"needle\xff\n");
    repo.write("binary.txt", b"needle\0\n");
    let before = repo.search("needle");
    assert_eq!(repo.index()["files_indexed"], 0);
    let after = repo.search("needle");
    assert_eq!(before["matches"], after["matches"]);
    assert_eq!(before["skipped"], after["skipped"]);
}

#[test]
fn subdirectory_case_insensitive_and_include_ignored_requests_keep_their_scope() {
    let repo = Repo::new();
    repo.write("src/main.rs", "NEEDLE\nneedle\n");
    repo.write("other.rs", "needle\n");
    repo.write(".gitignore", "ignored.rs\n");
    repo.write("ignored.rs", "needle\n");
    let requests = [
        json!({"pattern":"needle", "path":"src"}),
        json!({"pattern":"needle", "include_ignored":true}),
        json!({"pattern":"needle", "case_insensitive":true}),
        json!({"pattern":"needle", "max_results":1}),
    ];
    let before: Vec<_> = requests
        .iter()
        .map(|r| repo.call("search", r.clone()).1)
        .collect();
    repo.index();
    for (request, before) in requests.into_iter().zip(before) {
        let (ok, after) = repo.call("search", request.clone());
        assert!(ok, "{after}");
        assert_eq!(before["matches"], after["matches"], "{request}");
        assert_eq!(before["truncated"], after["truncated"], "{request}");
    }
}

#[cfg(unix)]
#[test]
fn cache_and_source_symlinks_cannot_redirect_indexing() {
    use std::os::unix::fs::symlink;
    let outside = Repo::new();
    outside.write("private.rs", "needle\n");
    let repo = Repo::new();
    repo.write("safe.rs", "other\n");
    symlink(outside.0.join("private.rs"), repo.0.join("link.rs")).unwrap();
    assert_eq!(repo.index()["files_indexed"], 1);
    fs::remove_file(repo.0.join("safe.rs")).unwrap();
    symlink(outside.0.join("private.rs"), repo.0.join("safe.rs")).unwrap();
    assert_eq!(repo.search("needle")["count"], 0);

    let redirected = Repo::new();
    symlink(&outside.0, redirected.0.join(".devcouncil")).unwrap();
    let (ok, reply) = redirected.call("index", json!({}));
    assert!(!ok, "{reply}");
    assert!(!outside.0.join("dcgrep").exists());
}

#[test]
fn rebuilding_repairs_a_corrupt_pointer_instead_of_requiring_manual_cleanup() {
    let repo = Repo::new();
    repo.write("hit.rs", "needle\n");
    repo.index();
    repo.write(".devcouncil/dcgrep/current.json", "broken");
    let fallback = repo.search("needle");
    assert_eq!(fallback["count"], 1);
    assert_eq!(fallback["index"]["status"], "scan");
    let rebuilt = repo.index();
    assert!(
        rebuilt["cache_warning"]
            .as_str()
            .unwrap()
            .contains("pointer")
    );
    assert_eq!(repo.search("needle")["count"], 1);
}
