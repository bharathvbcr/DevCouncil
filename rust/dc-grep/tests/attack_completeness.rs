//! Adversarial review of `rank`'s completeness claim, through the real binary.
//!
//! Each test states the property a caller relies on: `complete: true` means every
//! rankable file was ranked, so a file's absence from the answer means nothing
//! in it matches. A test that FAILS here is a way the claim is false.
//!
//! The fixtures that need millions of postings are generated in-process with a
//! fixed-seed xorshift; no dependency is added.

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
            "dcgrep-attack-{}-{}",
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

    fn call_in(&self, command: &str, mut request: Value, within: Duration) -> (bool, Value) {
        request["root"] = json!(self.0);
        let (ok, stdout) = support::run(command, request.to_string().as_bytes(), within);
        let reply: Value = serde_json::from_slice(&stdout)
            .unwrap_or_else(|err| panic!("{command}: no JSON reply ({err}): {stdout:?}"));
        (ok, reply)
    }

    fn call(&self, command: &str, request: Value) -> (bool, Value) {
        self.call_in(command, request, Duration::from_secs(120))
    }

    fn index(&self) -> Value {
        let (ok, reply) = self.call("index", json!({}));
        assert!(ok, "index must succeed: {reply}");
        reply
    }

    fn rank(&self, query: &str) -> Value {
        let (ok, reply) = self.call("rank", json!({ "query": query }));
        assert!(ok, "rank must succeed: {reply}");
        reply
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        // Restore anything a test locked so the tree can be removed.
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

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn word(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| (b'a' + (self.next() % 26) as u8) as char)
            .collect()
    }
}

/// An unreadable directory hides every file under it from the walk. The walk
/// counts it in `walk_errors`; `rank` has no field for that, so it answers
/// `complete: true` over a tree it could not read.
#[test]
fn an_unreadable_directory_does_not_leave_the_ranking_complete() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write("locked/b.rs", "fn hidden_badger() {}\n");
    fs::set_permissions(repo.0.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let built = repo.index();
    let ranked = repo.rank("hidden_badger");
    eprintln!("index: {built}\nrank: {ranked}");
    assert!(
        built["walk_errors"].as_u64().unwrap_or(0) >= 1,
        "the walk must have errored for this to test anything: {built}"
    );
    assert_eq!(ranked["count"], 0);
    assert_eq!(
        ranked["complete"], false,
        "rank says complete over a tree whose subdirectory it could not read: {ranked}"
    );
}

/// A file the tokeniser stopped on (more than 20,000 terms) is left out of the
/// ranked index and counted in `lexical_unindexed`, and `rank` still says
/// `complete: true`. A query for a word in that file returns nothing.
#[test]
fn a_document_over_the_term_cap_does_not_leave_the_ranking_complete() {
    let repo = Repo::new();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut big = String::new();
    for _ in 0..21_000 {
        big.push_str(&rng.word(9));
        big.push(' ');
    }
    big.push_str("needlezebra\n");
    repo.write("big.txt", &big);
    repo.write("small.rs", "fn small_alpaca() {}\n");
    let built = repo.index();
    let ranked = repo.rank("needlezebra");
    eprintln!("index: {built}\nrank: {ranked}");
    assert_eq!(built["lexical_unindexed"], 1, "{built}");
    assert_eq!(ranked["count"], 0, "{ranked}");
    assert_eq!(
        ranked["complete"], false,
        "rank says complete while a rankable file was left out of it: {ranked}"
    );
}

/// An unreadable file (mode 000) is skipped silently, after being counted in
/// `files_seen`.
#[test]
fn an_unreadable_file_does_not_leave_the_ranking_complete() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write("b.rs", "fn hidden_badger() {}\n");
    fs::set_permissions(repo.0.join("b.rs"), fs::Permissions::from_mode(0o000)).unwrap();
    let built = repo.index();
    let ranked = repo.rank("hidden_badger");
    eprintln!("index: {built}\nrank: {ranked}");
    assert_eq!(ranked["count"], 0);
    assert_eq!(
        ranked["complete"], false,
        "rank says complete while an unreadable file is absent: {ranked}"
    );
}

/// A binary file and a non-UTF-8 file are never rankable. They are in
/// `files_seen`; they must not make the ranking incomplete.
#[test]
fn binary_and_non_utf8_files_do_not_make_a_complete_ranking_partial() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write("blob.bin", [0u8, 1, 2, 3, 0, 255, 254]);
    repo.write("latin1.txt", [b'c', b'a', b'f', 0xe9, b' ', b'x', b'y']);
    let built = repo.index();
    let ranked = repo.rank("visible_alpaca");
    eprintln!("index: {built}\nrank: {ranked}");
    assert_eq!(ranked["files_seen"], 3, "{ranked}");
    assert_eq!(ranked["files_indexed"], 1, "{ranked}");
    assert_eq!(ranked["complete"], true, "{ranked}");
}

/// A file at or over the 2 MiB read bound is skipped without a word. It is in
/// `files_seen`. Pin what the answer says.
#[test]
fn an_oversized_file_is_counted_and_pinned() {
    let repo = Repo::new();
    repo.write("a.rs", "fn visible_alpaca() {}\n");
    repo.write(
        "huge.txt",
        format!("{}needlezebra\n", "word ".repeat(500_000)),
    );
    let _ = repo.index();
    let ranked = repo.rank("needlezebra");
    eprintln!("rank: {ranked}");
    assert_eq!(ranked["count"], 0);
    assert_eq!(ranked["files_seen"], 2, "{ranked}");
    // Recorded for the report: what `complete` says about it.
    eprintln!("oversized complete = {}", ranked["complete"]);
}

/// The file ceiling is checked on the NEXT file, so exactly `max_files` files
/// complete, and a tree of N rankable files plus one unrankable trailing file
/// is called incomplete although every rankable file was ranked.
#[test]
fn the_file_ceiling_at_exact_fit_and_one_past() {
    // Exactly N rankable files: complete.
    let repo = Repo::new();
    for n in 0..4 {
        repo.write(&format!("f{n}.rs"), format!("fn word{n}_unique() {{}}\n"));
    }
    let (ok, built) = repo.call("index", json!({ "max_files": 4 }));
    assert!(ok, "{built}");
    let ranked = repo.rank("word0_unique");
    eprintln!("exact fit rank: {ranked}");
    assert_eq!(
        ranked["complete"], true,
        "exact fit must be complete: {ranked}"
    );

    // N+1 rankable files: incomplete, reason files.
    repo.write("f4.rs", "fn word4_unique() {}\n");
    let (ok, built) = repo.call("index", json!({ "max_files": 4 }));
    assert!(ok, "{built}");
    let ranked = repo.rank("word0_unique");
    eprintln!("one past rank: {ranked}");
    assert_eq!(ranked["complete"], false, "{ranked}");
    assert_eq!(ranked["lexical_limit_reason"], "files", "{ranked}");

    // N rankable + trailing binary: every rankable file ranked.
    let repo = Repo::new();
    for n in 0..4 {
        repo.write(&format!("f{n}.rs"), format!("fn word{n}_unique() {{}}\n"));
    }
    repo.write("zzz.bin", [0u8, 1, 2]);
    let (ok, built) = repo.call("index", json!({ "max_files": 4 }));
    assert!(ok, "{built}");
    let ranked = repo.rank("word0_unique");
    eprintln!("N + trailing binary rank: {ranked}");
    assert_eq!(
        ranked["complete"], true,
        "every rankable file was ranked; a trailing binary file must not make it partial: {ranked}"
    );
}

/// Learned-sparse: an encoding that omits a file the walk read. The file is
/// counted in `lexical_unindexed`, is searchable by nothing in the ranking,
/// and `rank` reports `complete: true`.
#[test]
fn an_encoding_missing_a_file_does_not_leave_the_ranking_complete() {
    let repo = Repo::new();
    repo.write("src/alpha.rs", "fn alpha() { let x = 1; }\n");
    repo.write("src/beta.rs", "fn beta() { let y = 2; }\n");
    repo.write("src/gamma.rs", "fn gamma() { let z = 3; }\n");
    let header = json!({
        "schema": 1,
        "model": "fixture-encoder",
        "vocabulary": "wordpiece-30522",
        "vocab": ["[UNK]", "parse", "json", "server", "route", "handler", "##json", "gamma"],
        "query_weights": [0.0, 2.0, 3.0, 1.0, 1.0, 1.0, 2.5, 1.0],
        "parity": [
            {"text": "parse", "ids": [1]},
            {"text": "json", "ids": [2]},
            {"text": "parseJson", "ids": [1, 6]},
            {"text": "nothingatall", "ids": [0]},
        ],
    });
    let doc = |path: &str, terms: Value| {
        json!({ "path": path, "total_terms": 10, "terms": terms }).to_string()
    };
    // gamma.rs is "newer than the model run": absent from the encoding.
    let text = format!(
        "{header}\n{}\n{}\n",
        doc("src/alpha.rs", json!([[1, 0.9]])),
        doc("src/beta.rs", json!([[2, 0.8]])),
    );
    let encoding = repo.0.with_extension("jsonl");
    fs::write(&encoding, text).unwrap();
    let (ok, built) = repo.call("index", json!({ "sparse": encoding }));
    let _ = fs::remove_file(&encoding);
    assert!(ok, "{built}");
    assert_eq!(built["lexical_unindexed"], 1, "{built}");
    let ranked = repo.rank("parse");
    eprintln!("index: {built}\nrank: {ranked}");
    assert_eq!(ranked["files_seen"], 3, "{ranked}");
    assert_eq!(ranked["files_indexed"], 2, "{ranked}");
    assert_eq!(
        ranked["complete"], false,
        "rank says complete while one of three files has no weights: {ranked}"
    );
}

/// Cost and ceiling interaction at the ranked index's OWN ceiling: 250 files of
/// 16,000 distinct random words is exactly 4,000,000 lexical postings (the
/// ceiling) with ~4M distinct terms. `term_table` is 12 bytes per term and
/// postings are 8, so `lexical.bin` is ~80 MB against a 64 MiB bound. The build
/// used to stop at the trigram ceiling long before this was reachable.
#[test]
fn the_ranked_index_at_its_own_ceiling_still_publishes() {
    let repo = Repo::new();
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for n in 0..250 {
        let mut text = String::with_capacity(16_000 * 9);
        for _ in 0..16_000 {
            text.push_str(&rng.word(8));
            text.push(' ');
        }
        repo.write(&format!("d{n:03}.txt"), text);
    }
    let started = std::time::Instant::now();
    let (ok, built) = repo.call("index", json!({}));
    eprintln!("index ok={ok} in {:?}: {built}", started.elapsed());
    assert!(
        ok,
        "an index whose ranked half is at its own documented ceiling must publish: {built}"
    );
    let ranked = repo.rank("needlezebra");
    eprintln!("rank: {ranked}");
}

/// Stale stamps for ranked-only files, at scale and through the binary: the
/// trigram index fills on this corpus (random words, ~19k distinct trigrams per
/// file) while the ranked one, drawing from a small pool, does not.
///
/// Every file is edited after the build. `search` may report staleness only
/// for the trigram snapshot's files; `rank` must call every edited file stale.
#[test]
fn staleness_through_search_and_rank_for_files_past_the_trigram_ceiling() {
    let repo = Repo::new();
    let mut rng = Rng(0x0bad_cafe_dead_beef);
    let pool: Vec<String> = (0..50_000).map(|_| rng.word(8)).collect();
    for n in 0..260 {
        let mut text = format!("marker{n:03}x ");
        for _ in 0..16_000 {
            text.push_str(&pool[(rng.next() % pool.len() as u64) as usize]);
            text.push(' ');
        }
        repo.write(&format!("d{n:03}.txt"), text);
    }
    let built = repo.index();
    eprintln!("index: {built}");
    let indexed = built["files_indexed"].as_u64().unwrap();
    let seen = built["files_seen"].as_u64().unwrap();
    assert_eq!(seen, 260, "{built}");
    assert!(
        indexed < seen,
        "the trigram index must have filled for this to test anything: {built}"
    );
    assert_eq!(built["limit_reason"], "postings", "{built}");
    assert_eq!(built["lexical_files"], 260, "{built}");
    assert_eq!(built["lexical_limit_reason"], Value::Null, "{built}");
    for n in 0..260 {
        let path = repo.0.join(format!("d{n:03}.txt"));
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("edited\n");
        fs::write(&path, text).unwrap();
    }
    let (ok, found) = repo.call(
        "search",
        json!({ "pattern": "marker", "max_results": 1000 }),
    );
    assert!(ok, "{found}");
    eprintln!("search index block: {}", found["index"]);
    assert_eq!(found["count"], 260, "search must still find every file");
    assert_eq!(
        found["index"]["files_stale"].as_u64().unwrap(),
        indexed,
        "search reports staleness for the trigram snapshot only: {}",
        found["index"]
    );
    let one = repo.rank("marker259x");
    eprintln!("rank one: {one}");
    assert_eq!(one["files"][0]["stale"], true, "{one}");
    assert_eq!(one["complete"], true, "{one}");
}
