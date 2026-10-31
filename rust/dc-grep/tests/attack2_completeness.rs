//! Second adversarial round on `rank`'s completeness claim, through the real
//! binary. Branch under attack: `fix/dcgrep-rank-completeness` at bfedd4a9.
//!
//! Three kinds of test live here, and the attribute says which:
//!
//! * plain `#[test]`: a property the ruling states. Red is a finding.
//! * `#[ignore = "FIX: ..."]`: a property the ruling does NOT promise, where the
//!   current behaviour is a design consequence the owner may accept. Run with
//!   `--ignored` to see the disagreement.
//! * `trigram_admission_matches_dbd0193a` prints an `EQUIV` line so the same test
//!   can be run against a stock dbd0193a build and the two outputs diffed.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

mod support;

const BIG: usize = 2 * 1024 * 1024;
const WORD: &[u8] = b"needle alpha beta\n";

struct Repo(PathBuf);

impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "dcgrep-attack2-{}-{}",
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
            Duration::from_secs(300),
        );
        let reply: Value = serde_json::from_slice(&stdout)
            .unwrap_or_else(|err| panic!("{command}: no JSON reply ({err}): {stdout:?}"));
        (ok, reply)
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

    fn slot(&self) -> PathBuf {
        let cache = self.0.join(".devcouncil/dcgrep");
        let pointer: Value =
            serde_json::from_slice(&fs::read(cache.join("current.json")).unwrap()).unwrap();
        cache.join(pointer["slot"].as_str().unwrap())
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repeated(len: usize) -> Vec<u8> {
    WORD.iter().copied().cycle().take(len).collect()
}

/// Property 1. The trigram index admits exactly the files dbd0193a admitted.
///
/// The expected set was measured on a stock dbd0193a build of this same corpus
/// (7 files indexed, postings 1,296, input bytes 2,395,998). The corpus is the
/// one the brief names: NUL at several offsets, invalid UTF-8 at the end of an
/// 8 KiB prefix, a BOM, an empty file, 2 MiB - 1, 2 MiB, 2 MiB + 1, a valid
/// character cut at 8 KiB, CRLF, and symlinks.
#[test]
fn trigram_admission_matches_dbd0193a() {
    let repo = Repo::new();
    repo.write("empty.txt", b"");
    repo.write("plain.txt", WORD.repeat(3));
    repo.write("crlf.txt", b"needle\r\nalpha\r\nbeta\r\n");
    repo.write("bom.txt", b"\xef\xbb\xbfneedle bom\n");
    for offset in [0usize, 1, 100, 8190, 8191, 8192, 8193, 20000] {
        let mut bytes = vec![b'x'; (offset + 50).max(100)];
        bytes[offset] = 0;
        bytes.extend_from_slice(b" needle\n");
        repo.write(&format!("nul_at_{offset}.txt"), bytes);
    }
    repo.write("nul_end.txt", b"needle text\0");
    let mut bad = vec![b'a'; 8191];
    bad.extend_from_slice(b"\xe9 needle tail\n");
    repo.write("bad_at_8191_small.txt", bad);
    repo.write("bad_at_end.txt", b"needle \xe9");
    let mut cut = vec![b'a'; 8191];
    cut.extend_from_slice("é needle\n".as_bytes());
    repo.write("cut_small.txt", cut);
    repo.write("exact_2mib.txt", repeated(BIG));
    repo.write("exact_2mib_minus1.txt", repeated(BIG - 1));
    repo.write("exact_2mib_plus1.txt", repeated(BIG + 1));
    let mut big_cut = vec![b'a'; 8191];
    big_cut.extend_from_slice("é".as_bytes());
    big_cut.extend(repeated(BIG));
    repo.write("big_cut8k.txt", big_cut);
    let mut late = repeated(8192);
    late.push(0);
    late.extend(repeated(BIG));
    repo.write("big_nul_late.txt", late);
    let mut early = b"head\0".to_vec();
    early.extend(repeated(BIG));
    repo.write("big_nul_early.txt", early);
    let mut bad_late = repeated(9000);
    bad_late.extend_from_slice(b"\xff\xfe");
    bad_late.extend(repeated(BIG));
    repo.write("big_bad_late.txt", bad_late);
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend(repeated(BIG));
    repo.write("big_bom.txt", bom);
    let mut png = b"\x89PNG\r\n\x1a\n\0\0".to_vec();
    png.extend((0..BIG).map(|n| (n * 31 % 251) as u8));
    repo.write("big_png.bin", png);
    let mut bad_8191 = repeated(8191);
    bad_8191.push(0xe9);
    bad_8191.extend(repeated(BIG));
    repo.write("big_bad_at_8191.txt", bad_8191);
    repo.write(
        "manyterms.txt",
        (0..30000)
            .map(|n| format!("w{n}x"))
            .collect::<Vec<_>>()
            .join(" "),
    );
    repo.write("sub/inner.txt", b"needle inner\n");
    std::os::unix::fs::symlink("plain.txt", repo.0.join("link_to_file.txt")).unwrap();
    std::os::unix::fs::symlink("sub", repo.0.join("link_to_dir")).unwrap();
    std::os::unix::fs::symlink("nonexistent", repo.0.join("dangling")).unwrap();

    let built = repo.index();
    let stamps: Value =
        serde_json::from_slice(&fs::read(repo.slot().join("filestamps.json")).unwrap()).unwrap();
    let stamped: BTreeSet<String> = stamps
        .get("stamps")
        .unwrap_or(&stamps)
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let expected: BTreeSet<String> = [
        "crlf.txt",
        "cut_small.txt",
        "empty.txt",
        "exact_2mib_minus1.txt",
        "manyterms.txt",
        "plain.txt",
        "sub/inner.txt",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    let mut found = Vec::new();
    for pattern in ["needle", "alpha beta", "caf", "bom", "inner", "tail", "x"] {
        let (ok, reply) = repo.call("search", json!({ "pattern": pattern, "max_results": 1000 }));
        assert!(ok, "{reply}");
        let paths: BTreeSet<String> = reply["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap().to_string())
            .collect();
        found.push(json!({ "pattern": pattern, "paths": paths }));
    }
    let trigram_fields = json!({
        "files_seen": built["files_seen"], "files_indexed": built["files_indexed"],
        "files_unindexed": built["files_unindexed"], "walk_errors": built["walk_errors"],
        "traversal_complete": built["traversal_complete"], "limit_reason": built["limit_reason"],
        "input_bytes": built["input_bytes"], "postings": built["postings"],
    });
    eprintln!(
        "EQUIV {}",
        json!({ "trigram": trigram_fields, "stamped": stamped, "search": found })
    );
    assert_eq!(stamped, expected, "trigram-admitted set changed: {built}");
    assert_eq!(built["files_indexed"], 7, "{built}");
    assert_eq!(built["postings"], 1296, "{built}");
    assert_eq!(built["input_bytes"], 2_395_998, "{built}");
}

/// Property 3. The byte bound is exact, end to end, through the real tokeniser,
/// the real walk and the real reader.
///
/// 186 files of 18,000 distinct terms plus one last file sized so the written
/// file is exactly 64 MiB. Term ids are FNV-1a hashes, so ids are generated
/// distinct and every term is new: `8 + path + 20 * terms` per document.
///
/// cap - 1 and cap must be accepted and be exactly as long as the sum predicts;
/// cap + 1 must be refused with `lexical_bytes`, leave the file under the
/// bound, and still open. At exactly 64 MiB the reader's `>` check accepts.
#[test]
fn the_byte_bound_is_exact_from_cap_minus_one_to_cap_plus_one() {
    const CAP: u64 = 64 * 1024 * 1024;
    const HEADER: u64 = 52;
    const FULL: u64 = 186;
    const TERMS: u64 = 18_000;
    fn fnv(word: &str) -> u32 {
        word.bytes().fold(0x811c_9dc5u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        })
    }
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut seen = std::collections::HashSet::new();
    let mut words = |n: u64| -> Vec<String> {
        let mut out = Vec::new();
        while (out.len() as u64) < n {
            let word: String = (0..9)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    (b'a' + (state % 26) as u8) as char
                })
                .collect();
            if seen.insert(fnv(&word)) {
                out.push(word);
            }
        }
        out
    };
    let repo = Repo::new();
    for n in 0..FULL {
        repo.write(&format!("d{n:03}.txt"), words(TERMS).join(" ") + "\n");
    }
    let remaining = CAP - (HEADER + FULL * (8 + 8 + 20 * TERMS));
    let terms = (remaining - 8 - 5) / 20;
    let path_len = remaining - 8 - 20 * terms;
    let last_words = words(terms);
    let last_text = last_words.join(" ") + "\n";
    let last_name = |len: u64| format!("{}.txt", "L".repeat(len as usize - 4));

    let mut report = Vec::new();
    for (label, len, fits) in [
        ("cap-1", path_len - 1, true),
        ("cap", path_len, true),
        ("cap+1", path_len + 1, false),
    ] {
        let _ = fs::remove_dir_all(repo.0.join(".devcouncil"));
        for entry in fs::read_dir(&repo.0).unwrap().flatten() {
            if entry.file_name().to_string_lossy().starts_with('L') {
                fs::remove_file(entry.path()).unwrap();
            }
        }
        repo.write(&last_name(len), &last_text);
        let built = repo.index();
        let size = fs::metadata(repo.slot().join("lexical.bin")).unwrap().len();
        let ranked = repo.rank(&last_words[0]);
        let predicted = HEADER + FULL * (8 + 8 + 20 * TERMS) + 8 + len + 20 * terms;
        report.push(format!("{label}: predicted {predicted} written {size}"));
        assert!(
            size <= CAP,
            "{label}: {size} exceeds the bound the reader enforces"
        );
        assert_eq!(
            ranked["ok"], true,
            "{label}: the reader must open what was written: {ranked}"
        );
        if fits {
            assert_eq!(size, predicted, "{label}: predicted != written");
            assert_eq!(built["lexical_files"], FULL + 1, "{label}: {built}");
            assert_eq!(
                built["lexical_limit_reason"],
                Value::Null,
                "{label}: {built}"
            );
            assert_eq!(ranked["complete"], true, "{label}: {ranked}");
            assert_eq!(ranked["count"], 1, "{label}: {ranked}");
        } else {
            assert_eq!(
                built["lexical_limit_reason"], "lexical_bytes",
                "{label}: {built}"
            );
            assert_eq!(
                built["lexical_files"], FULL,
                "{label}: the crossing add is refused whole: {built}"
            );
            assert_eq!(ranked["complete"], false, "{label}: {ranked}");
            assert_eq!(
                ranked["lexical_limit_reason"], "lexical_bytes",
                "{label}: {ranked}"
            );
            assert!(predicted > CAP, "{label}: test arithmetic");
        }
    }
    eprintln!("{}", report.join("\n"));
}

/// Property 4. `lexical_unindexed_by_class`: counts sum to `lexical_unindexed`,
/// each sample is at most 50 paths, paths with control characters survive as
/// data, and every sample path is root-relative.
#[test]
fn the_per_class_report_sums_caps_and_keeps_hostile_names_as_data() {
    let repo = Repo::new();
    repo.write("ok.rs", "fn ordinary_alpaca() {}\n");
    let many = (0..30_000)
        .map(|n| format!("w{n}x"))
        .collect::<Vec<_>>()
        .join(" ");
    for n in 0..55 {
        repo.write(&format!("cap{n:02}.txt"), &many);
    }
    repo.write("ctl\tname\nx.txt", &many);
    repo.write("esc\u{1b}[31mred.txt", &many);
    repo.write("quote\"name\\.txt", &many);
    let built = repo.index();
    let by = built["lexical_unindexed_by_class"]
        .as_object()
        .expect("by class");
    let sum: u64 = by
        .values()
        .map(|class| class["count"].as_u64().unwrap())
        .sum();
    assert_eq!(sum, built["lexical_unindexed"].as_u64().unwrap(), "{built}");
    for (class, entry) in by {
        let sample = entry["sample"].as_array().unwrap();
        assert!(sample.len() <= 50, "{class}: {} paths", sample.len());
        assert!(
            sample.len() as u64 <= entry["count"].as_u64().unwrap(),
            "{class}"
        );
        for path in sample {
            let path = path.as_str().unwrap();
            assert!(
                !path.starts_with('/'),
                "{class}: absolute path in a report: {path}"
            );
        }
    }
    assert_eq!(by["term_cap"]["count"], 58, "{built}");
    assert_eq!(by["term_cap"]["sample"].as_array().unwrap().len(), 50);
    let ranked = repo.rank("ordinary_alpaca");
    assert_eq!(
        ranked["lexical_unindexed"], built["lexical_unindexed"],
        "rank reads what the build wrote"
    );
    assert_eq!(ranked["complete"], false);
}

/// Property 2, sparse direction. Paths in an encoding that the walk never
/// reached (ghost, escaping, binary, unreadable-by-kind) are not a shortfall
/// and are reported as `lexical_unmatched`; a file the walk reached that the
/// encoding omits is one.
#[test]
fn encoding_paths_outside_the_walk_are_not_a_shortfall_and_missing_ones_are() {
    let header = json!({
        "schema": 1, "model": "fixture-encoder", "vocabulary": "wordpiece-30522",
        "vocab": ["[UNK]", "parse", "json", "server", "route", "handler", "##json", "gamma"],
        "query_weights": [0.0, 2.0, 3.0, 1.0, 1.0, 1.0, 2.5, 1.0],
        "parity": [
            {"text": "parse", "ids": [1]}, {"text": "json", "ids": [2]},
            {"text": "parseJson", "ids": [1, 6]}, {"text": "nothingatall", "ids": [0]},
        ],
    });
    let doc =
        |path: &str| json!({ "path": path, "total_terms": 10, "terms": [[1, 0.9]] }).to_string();
    for (label, with_extra_file, complete, unindexed, unmatched) in [
        ("extras only", false, true, 0, 3),
        ("one file omitted", true, false, 1, 3),
    ] {
        let repo = Repo::new();
        repo.write("src/a.rs", "fn a() {}\n");
        repo.write("src/b.rs", "fn b() {}\n");
        repo.write("blob.bin", [0u8, 1, 2]);
        if with_extra_file {
            repo.write("src/omitted.rs", "fn omitted() {}\n");
        }
        let text = format!(
            "{header}\n{}\n{}\n{}\n{}\n{}\n",
            doc("src/a.rs"),
            doc("src/b.rs"),
            doc("ghost/one.rs"),
            doc("../escape.rs"),
            doc("blob.bin"),
        );
        let encoding = repo.0.with_extension("attack2.jsonl");
        fs::write(&encoding, text).unwrap();
        let (ok, built) = repo.call("index", json!({ "sparse": encoding }));
        let _ = fs::remove_file(&encoding);
        assert!(ok, "{label}: {built}");
        assert_eq!(built["lexical_unindexed"], unindexed, "{label}: {built}");
        assert_eq!(built["lexical_unmatched"], unmatched, "{label}: {built}");
        let ranked = repo.rank("parse");
        assert_eq!(ranked["complete"], complete, "{label}: {ranked}");
    }
}

/// An index built twice over the same tree must say the same thing: the cache
/// directory the first build wrote must not be a file the second one trips on.
#[test]
fn rebuilding_over_its_own_cache_does_not_change_the_verdict() {
    let repo = Repo::new();
    repo.write("ok.rs", "fn ordinary_alpaca() {}\n");
    repo.write("sub/b.rs", "fn other() {}\n");
    let first = repo.index();
    for round in 1..4 {
        let again = repo.index();
        for field in ["files_seen", "lexical_files", "lexical_unindexed"] {
            assert_eq!(first[field], again[field], "round {round}: {field}");
        }
        assert_eq!(
            repo.rank("ordinary_alpaca")["complete"],
            true,
            "round {round}"
        );
    }
}

/// FIX (not the completeness family): the ruling classifies an oversized file by
/// its first 8 KiB, so a file with a NUL or an invalid byte AFTER that prefix is
/// counted as text and makes `complete` false, while the byte-identical content
/// under the 2 MiB bound is not admissible and leaves `complete` true. The
/// error is on the safe side; the verdicts still disagree for one content.
#[test]
#[ignore = "FIX: an oversized file with a late NUL or bad byte is called text; its small twin is not"]
fn an_oversized_file_gets_the_same_verdict_as_its_small_twin() {
    let verdict = |name: &str, bytes: Vec<u8>| {
        let repo = Repo::new();
        repo.write("ok.rs", "fn ordinary_alpaca() {}\n");
        repo.write(name, bytes);
        repo.index();
        repo.rank("ordinary_alpaca")["complete"].as_bool().unwrap()
    };
    let twin = |total: usize| {
        let mut bytes = repeated(8192);
        bytes.push(0);
        bytes.extend(repeated(total));
        bytes
    };
    let small = verdict("small.txt", twin(500_000));
    let big = verdict("big.txt", twin(BIG));
    assert_eq!(
        big, small,
        "late-NUL file: oversized complete={big}, small complete={small}"
    );

    let bad_at_8191 = |total: usize| {
        let mut bytes = repeated(8191);
        bytes.push(0xe9);
        bytes.extend(repeated(total));
        bytes
    };
    let small = verdict("small.txt", bad_at_8191(500_000));
    let big = verdict("big.txt", bad_at_8191(BIG));
    assert_eq!(
        big, small,
        "bad byte at 8191: oversized complete={big}, small complete={small}"
    );
}

/// NIT: `complete` is a statement about the build, not the tree now. A file
/// created after the index is invisible to `rank`, and `stale_files` counts only
/// ranked files, so nothing in the answer says the tree has outgrown the index.
#[test]
#[ignore = "NIT: complete is a build-time claim; a file added after the build is not mentioned"]
fn a_file_added_after_the_build_is_reflected_somewhere_in_the_answer() {
    let repo = Repo::new();
    repo.write("ok.rs", "fn ordinary_alpaca() {}\n");
    repo.index();
    repo.write("late.rs", "fn late_arrival_zebra() {}\n");
    let ranked = repo.rank("late_arrival_zebra");
    assert!(
        ranked["complete"] == false || ranked["stale_files"].as_u64().unwrap() > 0,
        "tree has an unindexed file but the answer is clean: {ranked}"
    );
}
