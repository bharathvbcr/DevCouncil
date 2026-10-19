//! Adversarial probes against `lane/search-partial` (7c7f22da): a missed name
//! lookup that names the charged partial-parse range holding the name.
//!
//! Each test asserts what a correct answer must say. A failing test is a break;
//! the break is described in the doc comment above it. Nothing here fixes
//! anything.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, TextRange};
use devmap_query::{NameQueryFilter, Request, ResolutionAvailability, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::PathBuf;

struct F {
    path: String,
    /// Source as it was when the index was built.
    indexed: String,
    /// Bytes on disk afterwards, when they differ from `indexed`.
    disk: Option<Vec<u8>>,
    ranges: Vec<(usize, usize)>,
    drop: Vec<String>,
}

impl F {
    fn new(path: &str, source: &str, ranges: &[(usize, usize)], drop: &[&str]) -> Self {
        F {
            path: path.to_string(),
            indexed: source.to_string(),
            disk: None,
            ranges: ranges.to_vec(),
            drop: drop.iter().map(|s| (*s).to_string()).collect(),
        }
    }
    /// A file whose `lost` snippet (found in `source`) is a charged hole that
    /// drops the declaration of `name`.
    fn hole(path: &str, source: &str, lost: &str, name: &str) -> Self {
        let start = source.find(lost).expect("lost snippet is in the source");
        F::new(path, source, &[(start, start + lost.len())], &[name])
    }
    fn on_disk(mut self, bytes: &[u8]) -> Self {
        self.disk = Some(bytes.to_vec());
        self
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("devmap-sp-attack-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn corpus(name: &str, files: Vec<F>) -> (Store, PathBuf) {
    let root = scratch(name);
    let mut extractions: Vec<Extraction> = Vec::new();
    for file in &files {
        let full = root.join(&file.path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, file.disk.as_deref().unwrap_or(file.indexed.as_bytes())).unwrap();
        let mut ext = extract_file(&file.path, &file.indexed);
        if !file.ranges.is_empty() {
            ext.symbols.retain(|s| !file.drop.contains(&s.name));
            ext.parse_outcome = ParseOutcome::Partial {
                error_ranges: file
                    .ranges
                    .iter()
                    .map(|&(start_byte, end_byte)| TextRange {
                        start_byte,
                        end_byte,
                    })
                    .collect(),
                nameless_ranges: Vec::new(),
            };
        }
        extractions.push(ext);
    }
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(0),
    );
    let store = Store::open_in_memory().unwrap();
    store
        .save_generation_with_opts(
            &extractions,
            &resolution,
            &analysis,
            GenerationWriteOpts {
                repo_root: Some(root.to_string_lossy().into_owned()),
                discovery_refusals: Some(Vec::new()),
                over_limit: Some(Vec::new()),
                ..Default::default()
            },
        )
        .unwrap();
    (store, root)
}

fn ask(
    store: &Store,
    query: &str,
    paths: &[&str],
    languages: &[&str],
) -> devmap_query::Response<devmap_query::SymbolHit> {
    let paths: Vec<String> = paths.iter().map(|p| (*p).to_string()).collect();
    let languages: Vec<String> = languages.iter().map(|p| (*p).to_string()).collect();
    let filter = NameQueryFilter::new(&paths, &languages, &[]).unwrap();
    StoreQueryEngine::new(store)
        .search_filtered(
            Request {
                query: query.to_string(),
                token_budget: 8_000,
                min_confidence: 0.0,
                max_depth: 1,
            },
            filter.as_ref(),
        )
        .unwrap_or_else(|error| panic!("{error}"))
}

fn describe(a: &devmap_query::Response<devmap_query::SymbolHit>) -> String {
    format!(
        "total={} resolution={:?} walk_incomplete={:?} freshness={:?}",
        a.total, a.resolution, a.walk_incomplete, a.source_freshness
    )
}

fn unavailable_reason(a: &devmap_query::Response<devmap_query::SymbolHit>) -> String {
    match &a.resolution {
        ResolutionAvailability::Unavailable { reason } => reason.clone(),
        _ => panic!("expected Unavailable: {}", describe(a)),
    }
}

fn is_unavailable(a: &devmap_query::Response<devmap_query::SymbolHit>) -> bool {
    matches!(a.resolution, ResolutionAvailability::Unavailable { .. })
}

// ---------------------------------------------------------------- probe 1

/// Rust is case-sensitive: `FOO` is not `foo`. A range that holds only `FOO`
/// cannot be hiding a declaration of `foo`, yet the lookup lowercases both.
#[test]
fn p1_rust_range_with_upper_case_name_is_not_blamed_for_a_lower_case_query() {
    let src = "pub const KEPT: u32 = 1;\npub const FOO: u32 = 2;\n";
    let (store, root) = corpus(
        "p1rust",
        vec![F::hole("consts.rs", src, "pub const FOO: u32 = 2;", "FOO")],
    );
    let a = ask(&store, "foo", &["consts.rs"], &[]);
    // search is case-insensitive over the index, so `foo` finds nothing only
    // because FOO was dropped; a correct reason may say the range may declare
    // *FOO* but must not claim it may declare `foo` in a case-sensitive language.
    let verdict = if is_unavailable(&a) {
        unavailable_reason(&a)
    } else {
        String::new()
    };
    assert!(
        !verdict.contains("may declare it"),
        "Rust `FOO` attributed to query `foo`: {verdict}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p1_python_range_with_capitalised_class_is_not_blamed_for_a_lower_case_query() {
    let src = "def kept():\n    pass\n\ndef Foo():\n    pass\n";
    let (store, root) = corpus(
        "p1py",
        vec![F::hole("m.py", src, "def Foo():\n    pass\n", "Foo")],
    );
    let a = ask(&store, "foo", &["m.py"], &[]);
    let verdict = if is_unavailable(&a) {
        unavailable_reason(&a)
    } else {
        String::new()
    };
    assert!(
        !verdict.contains("may declare it"),
        "Python `Foo` attributed to query `foo`: {verdict}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p1_go_range_with_exported_name_is_not_blamed_for_the_unexported_query() {
    let src = "package p\n\nfunc kept() {}\n\nfunc Foo() {}\n";
    let (store, root) = corpus(
        "p1go",
        vec![F::hole("p.go", src, "func Foo() {}", "Foo")],
    );
    let a = ask(&store, "foo", &["p.go"], &[]);
    let verdict = if is_unavailable(&a) {
        unavailable_reason(&a)
    } else {
        String::new()
    };
    assert!(
        !verdict.contains("may declare it"),
        "Go exported `Foo` attributed to unexported `foo`: {verdict}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `single_name` takes the last `.` segment, so a query that is a version, a
/// file name or a float becomes `0` / `rs` / `toml` and matches any range that
/// holds that token. The reason then says the range "may declare" the query.
#[test]
fn p1b_a_dotted_non_identifier_query_does_not_collapse_to_its_last_segment() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 0;\n";
    let (store, root) = corpus(
        "p1b",
        vec![F::hole("consts.rs", src, "pub const LOST: u32 = 0;", "LOST")],
    );
    let a = ask(&store, "release.0", &["consts.rs"], &[]);
    let verdict = if is_unavailable(&a) {
        unavailable_reason(&a)
    } else {
        String::new()
    };
    assert!(
        !verdict.contains("may declare it"),
        "query `release.0` collapsed to the literal `0`: {verdict}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p1c_a_file_name_query_does_not_collapse_to_its_extension() {
    let src = "pub const KEPT: u32 = 1;\nuse std::rs::thing; pub const LOST: u32 = 5;\n";
    let (store, root) = corpus(
        "p1c",
        vec![F::hole(
            "consts.rs",
            src,
            "use std::rs::thing; pub const LOST: u32 = 5;",
            "LOST",
        )],
    );
    let a = ask(&store, "nosuchfile.rs", &["consts.rs"], &[]);
    let verdict = if is_unavailable(&a) {
        unavailable_reason(&a)
    } else {
        String::new()
    };
    assert!(
        !verdict.contains("may declare it"),
        "query `nosuchfile.rs` collapsed to `rs`: {verdict}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 2

/// The disk file changed after the build. The ranges now cover other text; a
/// new name that was never in the charged region is blamed anyway, with line
/// numbers from the NEW file.
#[test]
fn p2_a_range_over_edited_bytes_does_not_attribute_a_name_that_was_never_lost() {
    let indexed = "pub const KEPT: u32 = 1;\npub const X: u32 = 2;\n";
    let edited = "pub const KEPT: u32 = 1;\npub const Y: u32 = 2;\n";
    let (store, root) = corpus(
        "p2a",
        vec![F::hole("consts.rs", indexed, "pub const X: u32 = 2;", "X").on_disk(edited.as_bytes())],
    );
    let a = ask(&store, "Y", &["consts.rs"], &[]);
    let verdict = if is_unavailable(&a) {
        unavailable_reason(&a)
    } else {
        String::new()
    };
    assert!(
        !verdict.contains("may declare it"),
        "stale bytes attributed `Y` to a range charged for `X`; {}: {verdict}",
        describe(&a)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// The inverse: the file shifted, so the range no longer covers the name. The
/// hole is real (the index lacks the symbol) but the answer says nothing.
#[test]
fn p2_a_shifted_file_does_not_turn_a_scoped_miss_into_a_clean_available_zero() {
    let indexed = "pub const KEPT: u32 = 1;\npub const LOST_IN_HOLE: u32 = 2;\n";
    let shifted = format!("// a new header line\n{indexed}");
    let (store, root) = corpus(
        "p2b",
        vec![F::hole(
            "consts.rs",
            indexed,
            "pub const LOST_IN_HOLE: u32 = 2;",
            "LOST_IN_HOLE",
        )
        .on_disk(shifted.as_bytes())],
    );
    let a = ask(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    assert!(
        is_unavailable(&a),
        "the charged hole is still in the stored generation; a stale read hid it: {}",
        describe(&a)
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 3

#[test]
fn p3_nul_bytes_in_the_range_do_not_panic_and_still_match() {
    let indexed = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let disk = "pub const KEPT: u32 = 1;\npub const \0LOST\0: u32 = 2;\0\n";
    // bytes shift by 2 relative to the indexed text; use a same-length edit
    let _ = disk;
    let same_len = "pub const KEPT: u32 = 1;\npub \0\0\0\0\0 LOST\0 u32 = 2;\n";
    let (store, root) = corpus(
        "p3nul",
        vec![F::hole("consts.rs", indexed, "pub const LOST: u32 = 2;", "LOST")
            .on_disk(same_len.as_bytes())],
    );
    let a = ask(&store, "LOST", &["consts.rs"], &[]);
    let reason = if is_unavailable(&a) {
        unavailable_reason(&a)
    } else {
        String::new()
    };
    assert!(!reason.contains('\0'), "raw NUL in reason: {reason:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Invalid UTF-8 on disk: the file cannot be read as text. A scoped lookup must
/// not be a clean Available 0.
#[test]
fn p3_invalid_utf8_file_scoped_is_unavailable() {
    let indexed = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let mut bad = indexed.as_bytes().to_vec();
    bad[30] = 0xFF;
    let (store, root) = corpus(
        "p3utf8s",
        vec![F::hole("consts.rs", indexed, "pub const LOST: u32 = 2;", "LOST").on_disk(&bad)],
    );
    let a = ask(&store, "LOST", &["consts.rs"], &[]);
    assert!(is_unavailable(&a), "{}", describe(&a));
    let _ = std::fs::remove_dir_all(&root);
}

/// The same file, unscoped. The unreadable hole is dropped from the answer
/// silently: `Available, total 0` and no mention anywhere.
#[test]
fn p3_invalid_utf8_file_unscoped_is_not_silent() {
    let indexed = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let mut bad = indexed.as_bytes().to_vec();
    bad[30] = 0xFF;
    let (store, root) = corpus(
        "p3utf8u",
        vec![F::hole("consts.rs", indexed, "pub const LOST: u32 = 2;", "LOST").on_disk(&bad)],
    );
    let a = ask(&store, "LOST", &[], &[]);
    let wire = serde_json::to_string(&a).unwrap();
    assert!(
        is_unavailable(&a) || wire.contains("consts.rs"),
        "a charged hole that could not be read was not reported: {}",
        describe(&a)
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p3_a_ten_megabyte_file_scoped_is_unavailable_and_fast() {
    let indexed = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let mut big = indexed.as_bytes().to_vec();
    big.extend(std::iter::repeat(b' ').take(10 * 1024 * 1024));
    let (store, root) = corpus(
        "p3big",
        vec![F::hole("consts.rs", indexed, "pub const LOST: u32 = 2;", "LOST").on_disk(&big)],
    );
    let t = std::time::Instant::now();
    let a = ask(&store, "LOST", &["consts.rs"], &[]);
    let took = t.elapsed();
    assert!(is_unavailable(&a), "{}", describe(&a));
    assert!(took.as_millis() < 2000, "10 MB file cost {took:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A range whose end is past EOF (file truncated after the build).
#[test]
fn p3_range_end_beyond_eof_is_carried_not_dropped() {
    let indexed = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let truncated = "pub const KEPT: u32 = 1;\n";
    let (store, root) = corpus(
        "p3eof",
        vec![F::hole("consts.rs", indexed, "pub const LOST: u32 = 2;", "LOST")
            .on_disk(truncated.as_bytes())],
    );
    let a = ask(&store, "LOST", &["consts.rs"], &[]);
    assert!(is_unavailable(&a), "{}", describe(&a));
    let _ = std::fs::remove_dir_all(&root);
}

/// Overlapping and unsorted error ranges from the producer: one name in one
/// text region is one match, not one per overlapping range, and nothing panics.
#[test]
fn p3_overlapping_unsorted_ranges_name_the_region_once() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let s = src.find("pub const LOST").unwrap();
    let e = s + "pub const LOST: u32 = 2;".len();
    let (store, root) = corpus(
        "p3ovl",
        vec![F::new(
            "consts.rs",
            src,
            &[(s + 4, e), (s, s + 10), (s + 2, e - 1), (s, e)],
            &["LOST"],
        )],
    );
    let a = ask(&store, "LOST", &["consts.rs"], &[]);
    let reason = unavailable_reason(&a);
    assert!(
        reason.contains("1 charged partial-parse range(s)"),
        "{reason}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A path with a newline and an ESC in it must not reach the sentence raw.
#[test]
fn p3_control_characters_in_a_path_are_escaped() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let (store, root) = corpus(
        "p3ctl",
        vec![F::hole("we\nird\u{1b}[31m.rs", src, "pub const LOST: u32 = 2;", "LOST")],
    );
    let a = ask(&store, "LOST", &[], &[]);
    let reason = unavailable_reason(&a);
    assert!(
        !reason.chars().any(|c| c.is_control()),
        "raw control character in the reason: {reason:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Five matched paths near PATH_MAX: the reason must stay bounded
/// (`devmap_query::bounded_text` / MAX_REASON_COMPONENT_BYTES = 4096).
#[test]
fn p3_the_reason_is_bounded() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let comp = "d".repeat(225);
    let files: Vec<F> = (0..6)
        .map(|n| {
            let path = format!("{comp}{n}/{comp}/{comp}/{comp}/f.rs");
            F::hole(&path, src, "pub const LOST: u32 = 2;", "LOST")
        })
        .collect();
    let (store, root) = corpus("p3len", files);
    let a = ask(&store, "LOST", &[], &[]);
    let reason = unavailable_reason(&a);
    assert!(
        reason.len() <= devmap_query::MAX_REASON_COMPONENT_BYTES + 300,
        "unbounded reason: {} bytes",
        reason.len()
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 4

/// 0, 1 and N ranges, and the 8 / 9 elision boundary of the producer's list.
#[test]
fn p4_zero_charged_ranges_is_a_plain_available_zero() {
    let src = "pub const KEPT: u32 = 1;\n";
    let (store, root) = corpus("p4zero", vec![F::new("consts.rs", src, &[], &[])]);
    let a = ask(&store, "NOPE", &["consts.rs"], &[]);
    assert!(!is_unavailable(&a), "{}", describe(&a));
    let _ = std::fs::remove_dir_all(&root);
}

fn many(n: usize, name_in: usize) -> (Store, PathBuf) {
    // n charged consts; the name sits only in range `name_in`.
    let mut src = String::from("pub const KEPT: u32 = 1;\n");
    let mut ranges = Vec::new();
    let mut drops = Vec::new();
    for i in 0..n {
        let line = if i == name_in {
            "pub const TARGET_NAME: u32 = 7;\n".to_string()
        } else {
            format!("pub const FILL_{i:03}: u32 = 7;\n")
        };
        let start = src.len();
        src.push_str(&line);
        ranges.push((start, start + line.trim_end().len()));
        drops.push(if i == name_in {
            "TARGET_NAME".to_string()
        } else {
            format!("FILL_{i:03}")
        });
    }
    let drop_refs: Vec<&str> = drops.iter().map(String::as_str).collect();
    corpus(
        &format!("p4many{n}-{name_in}"),
        vec![F::new("consts.rs", &src, &ranges, &drop_refs)],
    )
}

#[test]
fn p4_name_in_the_eighth_of_eight_ranges_is_found() {
    let (store, root) = many(8, 7);
    let reason = unavailable_reason(&ask(&store, "TARGET_NAME", &["consts.rs"], &[]));
    assert!(reason.contains("consts.rs lines"), "{reason}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p4_name_in_the_ninth_of_nine_ranges_is_carried_as_unchecked() {
    let (store, root) = many(9, 8);
    let a = ask(&store, "TARGET_NAME", &["consts.rs"], &[]);
    let reason = unavailable_reason(&a);
    assert!(reason.contains("not listed in the stored reason"), "{reason}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p4_one_charged_range_is_found() {
    let (store, root) = many(1, 0);
    let reason = unavailable_reason(&ask(&store, "TARGET_NAME", &["consts.rs"], &[]));
    assert!(reason.contains("1 charged partial-parse range(s)"), "{reason}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The same ninth-range name, UNSCOPED: the elided range is dropped silently.
#[test]
fn p4_elided_range_unscoped_is_not_silent() {
    let (store, root) = many(9, 8);
    let a = ask(&store, "TARGET_NAME", &[], &[]);
    let wire = serde_json::to_string(&a).unwrap();
    assert!(
        is_unavailable(&a) || wire.contains("consts.rs"),
        "an elided charged range was dropped without a word: {}",
        describe(&a)
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 5

/// 70 partial files; the name is in the 70th by path. Past the read cap (16 files) an
/// UNSCOPED lookup says nothing about the files it did not read.
#[test]
fn p5_the_read_cap_is_not_silent_when_unscoped() {
    let mut files = Vec::new();
    for i in 0..70 {
        let src = format!("pub const KEPT_{i:02}: u32 = 1;\npub const FILL_{i:02}: u32 = 2;\n");
        let hole = format!("pub const FILL_{i:02}: u32 = 2;");
        let name = format!("FILL_{i:02}");
        files.push(F::hole(&format!("m{i:02}.rs"), &src, &hole, &name));
    }
    let (store, root) = corpus("p5cap", files);
    let a = ask(&store, "FILL_69", &[], &[]);
    let wire = serde_json::to_string(&a).unwrap();
    assert!(
        is_unavailable(&a) || wire.contains("m69.rs") || wire.contains("read cap"),
        "FILL_69's file was past the read cap and the answer is a clean miss: {}",
        describe(&a)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Timing probe (run with --ignored --nocapture, release): 2000 partial files,
/// 64+ in scope, a miss. Prints min-of-5 of ten keystroke-like prefixes.
#[test]
#[ignore]
fn p5_timing_miss_on_2000_partial_files() {
    let mut files = Vec::new();
    for i in 0..2000 {
        let body = "x".repeat(400);
        let src = format!("pub const KEPT_{i:04}: u32 = 1;\n// {body}\npub const FILL_{i:04}: u32 = 2;\n");
        let hole = format!("pub const FILL_{i:04}: u32 = 2;");
        let name = format!("FILL_{i:04}");
        files.push(F::hole(&format!("d/m{i:04}.rs"), &src, &hole, &name));
    }
    let (store, root) = corpus("p5time", files);
    let prefixes = ["z", "zq", "zqx", "zqxv", "zqxvw", "zqxvwk", "zqxvwkj", "zqxvwkjy"];
    let mut best = std::time::Duration::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        for q in prefixes {
            let _ = ask(&store, q, &[], &[]);
        }
        best = best.min(t.elapsed());
    }
    let mut best_one = std::time::Duration::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let _ = ask(&store, "NOSUCHNAMEATALL", &[], &[]);
        best_one = best_one.min(t.elapsed());
    }
    println!("TIMING 8-prefix-sequence min-of-5 = {best:?}; single miss min-of-5 = {best_one:?}");
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 6

/// Language-only scope is unscoped for the hole check: a Rust hole is blamed
/// on a python-scoped lookup.
#[test]
fn p6_a_language_scope_excludes_holes_in_other_languages() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let (store, root) = corpus(
        "p6lang",
        vec![
            F::hole("consts.rs", src, "pub const LOST: u32 = 2;", "LOST"),
            F::new("ok.py", "def fine():\n    pass\n", &[], &[]),
        ],
    );
    let a = ask(&store, "LOST", &[], &["python"]);
    let wire = serde_json::to_string(&a).unwrap();
    assert!(
        !wire.contains("consts.rs"),
        "a python-scoped lookup is blamed on a Rust file: {}",
        describe(&a)
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p6_directory_scope_containing_the_holed_file_names_it() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let (store, root) = corpus(
        "p6dir",
        vec![F::hole("sub/deep/consts.rs", src, "pub const LOST: u32 = 2;", "LOST")],
    );
    for scope in ["sub", "sub/", "sub/deep", "./sub", "sub/deep/consts.rs", "sub/deep/consts.rs/"] {
        let a = ask(&store, "LOST", &[scope], &[]);
        assert!(
            is_unavailable(&a) && unavailable_reason(&a).contains("sub/deep/consts.rs"),
            "scope {scope:?}: {}",
            describe(&a)
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// An absolute path scope is kept absolute in `filter.paths()` and only made
/// relative later; the hole check compares it to relative stored paths.
#[test]
fn p6_absolute_path_scope_names_the_hole() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let (store, root) = corpus(
        "p6abs",
        vec![F::hole("consts.rs", src, "pub const LOST: u32 = 2;", "LOST")],
    );
    let abs = root.join("consts.rs");
    let a = ask(&store, "LOST", &[abs.to_str().unwrap()], &[]);
    assert!(is_unavailable(&a), "absolute scope: {}", describe(&a));
    let _ = std::fs::remove_dir_all(&root);
}

/// A glob scope is not a prefix. Whatever the scope layer does with it, a
/// lookup it cannot resolve must not read as a clean Available 0 over a hole.
#[test]
fn p6_glob_scope_characterisation() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let (store, root) = corpus(
        "p6glob",
        vec![F::hole("consts.rs", src, "pub const LOST: u32 = 2;", "LOST")],
    );
    let paths = vec!["*.rs".to_string()];
    let filter = NameQueryFilter::new(&paths, &[], &[]);
    match filter {
        Err(e) => println!("GLOB scope refused at construction: {e}"),
        Ok(filter) => {
            let r = StoreQueryEngine::new(&store).search_filtered(
                Request {
                    query: "LOST".into(),
                    token_budget: 8000,
                    min_confidence: 0.0,
                    max_depth: 1,
                },
                filter.as_ref(),
            );
            match r {
                Err(e) => println!("GLOB scope refused at query: {e}"),
                Ok(a) => println!("GLOB scope answered: {}", describe(&a)),
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 7

#[test]
fn p7_explore_of_a_missing_name_never_returns_available_over_a_matching_range() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let (store, root) = corpus(
        "p7",
        vec![F::hole("consts.rs", src, "pub const LOST: u32 = 2;", "LOST")],
    );
    for (query, paths) in [("LOST", vec![]), ("LOST", vec!["consts.rs".to_string()])] {
        let filter = NameQueryFilter::new(&paths, &[], &[]).unwrap();
        let report = StoreQueryEngine::new(&store)
            .explore_filtered(query, 5, 8_000, 0.0, 1, filter.as_ref())
            .unwrap();
        let d = &report.definitions;
        assert!(
            matches!(d.resolution, ResolutionAvailability::Unavailable { .. })
                || !d.items.is_empty(),
            "explore {query:?} scope {paths:?}: Available with no definitions: {:?} total={}",
            d.resolution,
            d.total
        );
        if let ResolutionAvailability::Unavailable { reason } = &d.resolution {
            assert!(reason.contains("consts.rs lines 2-2"), "{reason}");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A multi-word explore query is a description, not a name; but a qualified or
/// dotted query still reaches the check (see p1b), and `explore` of a plain
/// miss must still be Available.
#[test]
fn p7_explore_of_a_plain_miss_stays_available() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let (store, root) = corpus(
        "p7b",
        vec![F::hole("consts.rs", src, "pub const LOST: u32 = 2;", "LOST")],
    );
    let report = StoreQueryEngine::new(&store)
        .explore_filtered("NOTHING_LIKE_IT", 5, 8_000, 0.0, 1, None)
        .unwrap();
    assert!(
        matches!(
            report.definitions.resolution,
            ResolutionAvailability::Available
        ),
        "{:?}",
        report.definitions.resolution
    );
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 8

/// What MCP would put on the wire is the serde form of the Response. The reason
/// string must be the same text and bounded; with long paths it is not.
#[test]
fn p8_wire_reason_equals_library_reason_and_is_bounded() {
    let src = "pub const KEPT: u32 = 1;\npub const LOST: u32 = 2;\n";
    let comp = "e".repeat(225);
    let files: Vec<F> = (0..6)
        .map(|n| {
            let path = format!("{comp}{n}/{comp}/{comp}/{comp}/f.rs");
            F::hole(&path, src, "pub const LOST: u32 = 2;", "LOST")
        })
        .collect();
    let (store, root) = corpus("p8", files);
    let a = ask(&store, "LOST", &[], &[]);
    let reason = unavailable_reason(&a);
    let wire: serde_json::Value = serde_json::to_value(&a).unwrap();
    let on_wire = wire["resolution"]["Unavailable"]["reason"]
        .as_str()
        .unwrap_or("<missing>")
        .to_string();
    assert_eq!(reason, on_wire);
    assert!(
        on_wire.len() <= devmap_query::MAX_REASON_COMPONENT_BYTES + 300,
        "wire reason is {} bytes",
        on_wire.len()
    );
    let _ = std::fs::remove_dir_all(&root);
}
