//! Closing adversarial probes against `lane/search-partial-parse` (bee31e38).
//!
//! Each test asserts what a correct answer must say; a failing test is a break,
//! described in the doc comment above it. Nothing here fixes anything.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, TextRange};
use devmap_query::{NameQueryFilter, Request, ResolutionAvailability, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{GenerationWriteOpts, Store};
use std::path::PathBuf;

struct F {
    path: String,
    indexed: String,
    disk: Option<Vec<u8>>,
    ranges: Vec<(usize, usize)>,
    drop: Vec<String>,
    mtime_back: bool,
}

impl F {
    fn new(path: &str, source: &str, ranges: &[(usize, usize)], drop: &[&str]) -> Self {
        F {
            path: path.to_string(),
            indexed: source.to_string(),
            disk: None,
            ranges: ranges.to_vec(),
            drop: drop.iter().map(|s| (*s).to_string()).collect(),
            mtime_back: false,
        }
    }
    fn hole(path: &str, source: &str, lost: &str, name: &str) -> Self {
        let start = source.find(lost).expect("lost snippet is in the source");
        F::new(path, source, &[(start, start + lost.len())], &[name])
    }
    fn on_disk(mut self, bytes: &[u8]) -> Self {
        self.disk = Some(bytes.to_vec());
        self
    }
    fn old_mtime(mut self) -> Self {
        self.mtime_back = true;
        self
    }
}

fn scratch(name: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "devmap-spc-attack-{name}-{}-{seq}",
        std::process::id()
    ));
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
        if file.mtime_back {
            let handle = std::fs::OpenOptions::new().write(true).open(&full).unwrap();
            handle
                .set_modified(
                    std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(86_400),
                )
                .unwrap();
        }
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

fn filter(paths: &[&str], languages: &[&str]) -> Option<NameQueryFilter> {
    let paths: Vec<String> = paths.iter().map(|p| (*p).to_string()).collect();
    let languages: Vec<String> = languages.iter().map(|p| (*p).to_string()).collect();
    NameQueryFilter::new(&paths, &languages, &[]).unwrap()
}

fn ask(
    store: &Store,
    query: &str,
    paths: &[&str],
    languages: &[&str],
) -> devmap_query::Response<devmap_query::SymbolHit> {
    StoreQueryEngine::new(store)
        .search_filtered(
            Request {
                query: query.to_string(),
                token_budget: 8_000,
                min_confidence: 0.0,
                max_depth: 1,
            },
            filter(paths, languages).as_ref(),
        )
        .unwrap_or_else(|error| panic!("{error}"))
}

fn explore(
    store: &Store,
    query: &str,
    paths: &[&str],
    languages: &[&str],
) -> devmap_query::ExploreReport {
    StoreQueryEngine::new(store)
        .explore_filtered(query, 10, 8_000, 0.0, 1, filter(paths, languages).as_ref())
        .unwrap_or_else(|error| panic!("{error}"))
}

fn describe(a: &devmap_query::Response<devmap_query::SymbolHit>) -> String {
    format!(
        "total={} resolution={:?} walk_incomplete={:?}",
        a.total, a.resolution, a.walk_incomplete
    )
}

fn is_unavailable(a: &devmap_query::Response<devmap_query::SymbolHit>) -> bool {
    matches!(a.resolution, ResolutionAvailability::Unavailable { .. })
}

fn reason(a: &devmap_query::Response<devmap_query::SymbolHit>) -> String {
    match &a.resolution {
        ResolutionAvailability::Unavailable { reason } => reason.clone(),
        _ => panic!("expected Unavailable: {}", describe(a)),
    }
}

const RS: &str = "pub const KEPT: u32 = 1;\npub const LOST_IN_HOLE: u32 = 2;\n";
const RS_HOLE: &str = "pub const LOST_IN_HOLE: u32 = 2;";

// ---------------------------------------------------------------- probe 1

/// Unchanged bytes, changed mtime: the check is by content hash, so the region
/// is still read and attributed.
#[test]
fn p1_unchanged_content_with_a_new_mtime_is_still_checked() {
    let (store, root) = corpus(
        "p1mtime",
        vec![F::hole("consts.rs", RS, RS_HOLE, "LOST_IN_HOLE").old_mtime()],
    );
    let a = ask(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    let why = reason(&a);
    assert!(why.contains("lines 2-2"), "{why}");
    assert!(!why.contains("changed since the build"), "false changed: {why}");
    let _ = std::fs::remove_dir_all(&root);
}

/// CRLF endings throughout: hash is over raw bytes both ways, offsets too.
#[test]
fn p1_crlf_file_is_checked_and_not_called_changed() {
    let src = "pub const KEPT: u32 = 1;\r\npub const LOST_IN_HOLE: u32 = 2;\r\n";
    let (store, root) = corpus(
        "p1crlf",
        vec![F::hole("consts.rs", src, RS_HOLE, "LOST_IN_HOLE")],
    );
    let a = ask(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    let why = reason(&a);
    assert!(why.contains("lines 2-2") && !why.contains("changed since"), "{why}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A leading BOM (3 bytes) shifts every offset; both sides keep it.
#[test]
fn p1_bom_file_is_checked_and_not_called_changed() {
    let src = format!("\u{feff}{RS}");
    let (store, root) = corpus(
        "p1bom",
        vec![F::hole("consts.rs", &src, RS_HOLE, "LOST_IN_HOLE")],
    );
    let a = ask(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    let why = reason(&a);
    assert!(why.contains("lines 2-2") && !why.contains("changed since"), "{why}");
    let _ = std::fs::remove_dir_all(&root);
}

/// The build's own reader and the check's reader must hash the same bytes for
/// a BOM + CRLF file: compare `extract_tree`'s recorded hash with
/// `read_repo_source` + `content_hash`.
#[test]
fn p1_the_build_read_path_and_the_check_read_path_hash_alike() {
    let root = scratch("p1tree");
    let bytes = "\u{feff}pub const A: u32 = 1;\r\npub fn broken( {{ \r\n";
    std::fs::write(root.join("a.rs"), bytes).unwrap();
    let built = devmap_extract::extract_tree(&root).unwrap();
    let ext = built
        .iter()
        .find(|e| e.file_path == "a.rs")
        .expect("a.rs built");
    let source = devmap_extract::safe_fs::read_repo_source(
        Some(&root),
        "a.rs",
        devmap_extract::MAX_SOURCE_BYTES,
    )
    .unwrap();
    assert_eq!(devmap_extract::content_hash(&source), ext.content_hash);
    let _ = std::fs::remove_dir_all(&root);
}

/// A file exactly at the source ceiling is still readable (limit is inclusive).
#[test]
fn p1_a_file_exactly_at_the_ceiling_is_checked() {
    let limit = devmap_extract::MAX_SOURCE_BYTES as usize;
    let head = format!("{RS}// ");
    let mut src = head.clone();
    src.push_str(&"x".repeat(limit - head.len() - 1));
    src.push('\n');
    assert_eq!(src.len(), limit);
    let (store, root) = corpus(
        "p1limit",
        vec![F::hole("consts.rs", &src, RS_HOLE, "LOST_IN_HOLE")],
    );
    let a = ask(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    let why = reason(&a);
    assert!(why.contains("lines 2-2"), "{why}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Edited bytes after the build: unchecked, never a clean Available-0 (scoped).
#[test]
fn p1_edited_file_scoped_is_unavailable_and_says_changed() {
    let edited = format!("// shifted\n{RS}");
    let (store, root) = corpus(
        "p1edit",
        vec![F::hole("consts.rs", RS, RS_HOLE, "LOST_IN_HOLE").on_disk(edited.as_bytes())],
    );
    let a = ask(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    assert!(is_unavailable(&a), "{}", describe(&a));
    assert!(reason(&a).contains("changed since the build"));
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 2

fn qualified_hole(path: &str, source: &str, lost: &str, name: &str, query: &str) {
    let (store, root) = corpus(
        &format!("p2-{name}"),
        vec![F::hole(path, source, lost, name)],
    );
    // (No bare-name control: a bare `ts`/`go`/`py` substring-matches the file's own
    // path and symbols, so it is a hit. `Config::ts` in the Rust test below is the
    // control: the same shape with `::` is attributed.)
    let a = ask(&store, query, &[path], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        is_unavailable(&a) && reason(&a).contains(path),
        "`{query}` is a real qualified name whose last segment `{name}` sits in a charged \
         hole of {path}; scoped answer: {}",
        describe(&a)
    );
}

/// Go method `go` on `foo`: `foo.go` is a qualified identifier, but `.go` is
/// Go's extension, so the lookup treats it as a file name and skips attribution.
#[test]
fn p2_foo_dot_go_method_in_a_go_hole_is_attributed() {
    let src = "package p\n\ntype foo struct{}\n\nfunc (f foo) kept() {}\n\nfunc (f foo) go() {}\n";
    qualified_hole("m.go", src, "func (f foo) go() {}", "go", "foo.go");
}

/// TypeScript `Config.ts`: a method named `ts` on class `Config`.
#[test]
fn p2_config_dot_ts_method_in_a_ts_hole_is_attributed() {
    let src = "export class Config {\n  kept() {}\n  ts() { return 1; }\n}\n";
    qualified_hole("c.ts", src, "ts() { return 1; }", "ts", "Config.ts");
}

/// Python `Foo.py`: a method `py`.
#[test]
fn p2_foo_dot_py_method_in_a_python_hole_is_attributed() {
    let src = "class Foo:\n    def kept(self):\n        pass\n\n    def py(self):\n        pass\n";
    qualified_hole("m.py", src, "def py(self):\n        pass", "py", "Foo.py");
}

/// Rust path form `Config::ts` is not dotted, so it must keep working.
#[test]
fn p2_double_colon_form_with_an_extension_name_is_attributed() {
    let src = "pub struct Config;\nimpl Config {\n    pub fn kept() {}\n    pub fn ts() {}\n}\n";
    qualified_hole("c.rs", src, "pub fn ts() {}", "ts", "Config::ts");
}

/// An extension-shaped query that really is a file name stays a file name.
#[test]
fn p2_a_file_name_query_is_not_blamed() {
    let (store, root) = corpus(
        "p2file",
        vec![F::hole("consts.rs", RS, RS_HOLE, "LOST_IN_HOLE")],
    );
    let a = ask(&store, "nosuchfile.rs", &["consts.rs"], &[]);
    assert!(!is_unavailable(&a), "{}", describe(&a));
    let _ = std::fs::remove_dir_all(&root);
}

/// Case folding: SQL, Pascal, VB, CFML, COBOL fold; PHP function and class
/// names are also case-insensitive but are not in the list.
#[test]
fn p2_php_function_names_ignore_case_so_a_lower_case_query_matches() {
    let src = "<?php\nfunction kept() {}\nfunction LostInHole() {}\n";
    let (store, root) = corpus(
        "p2php",
        vec![F::hole("a.php", src, "function LostInHole() {}", "LostInHole")],
    );
    let a = ask(&store, "lostinhole", &["a.php"], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        is_unavailable(&a),
        "PHP function names are case-insensitive, so `lostinhole` may be `LostInHole`: {}",
        describe(&a)
    );
}

#[test]
fn p2_sql_and_pascal_fold_case_on_their_stored_language_key() {
    let cases = [
        (
            "a.sql",
            "SELECT 1;\nCREATE TABLE Users (id int);\n",
            "CREATE TABLE Users (id int);",
            "users",
            "Users",
        ),
        (
            "a.pas",
            "unit u;\ninterface\nprocedure Kept;\nprocedure LostProc;\nimplementation\nend.\n",
            "procedure LostProc;",
            "lostproc",
            "LostProc",
        ),
    ];
    for (path, src, lost, query, dropped) in cases {
        let at = src.find(lost).unwrap();
        let (store, root) = corpus(
            "p2fold",
            vec![F::new(path, src, &[(at, at + lost.len())], &[dropped])],
        );
        let a = ask(&store, query, &[path], &[]);
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            is_unavailable(&a) && reason(&a).contains(path),
            "{path}: a case-insensitive language, hole holds the name in another case: {}",
            describe(&a)
        );
    }
}

// ---------------------------------------------------------------- probe 3

fn partial_changed_beside_a_hit() -> (Store, PathBuf) {
    corpus(
        "p3",
        vec![
            F::new("good.rs", "pub fn present() {}\n", &[], &[]),
            F::hole("consts.rs", RS, RS_HOLE, "LOST_IN_HOLE")
                .on_disk(format!("// shifted\n{RS}").as_bytes()),
        ],
    )
}

/// A found symbol is a complete answer for that name: unchecked regions
/// elsewhere are not its business.
#[test]
fn p3_an_unscoped_hit_never_carries_the_unchecked_note() {
    let (store, root) = partial_changed_beside_a_hit();
    let a = ask(&store, "present", &[], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(a.total >= 1, "{}", describe(&a));
    let wire = serde_json::to_string(&a).unwrap();
    assert!(!wire.contains("could not be checked"), "{wire}");
}

/// Positive control: the same unscoped lookup that MISSES does carry it.
#[test]
fn p3_control_an_unscoped_miss_carries_the_unchecked_note() {
    let (store, root) = partial_changed_beside_a_hit();
    let a = ask(&store, "nothere", &[], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        a.walk_incomplete
            .clone()
            .unwrap_or_default()
            .contains("could not be checked"),
        "{}",
        describe(&a)
    );
}

/// A hit through a PATH scope that excludes the holed file: still no note.
#[test]
fn p3_a_scoped_hit_elsewhere_has_no_note() {
    let (store, root) = partial_changed_beside_a_hit();
    let a = ask(&store, "present", &["good.rs"], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(a.total >= 1);
    assert!(!serde_json::to_string(&a)
        .unwrap()
        .contains("could not be checked"));
}

// ---------------------------------------------------------------- probe 4

fn many_files(dir: &str, n: usize, ext: &str, start: usize) -> Vec<F> {
    (start..start + n)
        .map(|i| {
            let (src, hole, name) = match ext {
                "py" => (
                    format!("KEPT_{i:03} = 1\nFILL_{i:03} = 2\n"),
                    format!("FILL_{i:03} = 2"),
                    format!("FILL_{i:03}"),
                ),
                _ => (
                    format!("pub const KEPT_{i:03}: u32 = 1;\npub const FILL_{i:03}: u32 = 2;\n"),
                    format!("pub const FILL_{i:03}: u32 = 2;"),
                    format!("FILL_{i:03}"),
                ),
            };
            F::hole(&format!("{dir}/m{i:03}.{ext}"), &src, &hole, &name)
        })
        .collect()
}

/// 70 partial files under a/ (6 past the 64 cap), 3 under b/. A lookup scoped to
/// b/ has 3 in scope: nothing is past the cap, so nothing may be called
/// unchecked for the cap.
#[test]
fn p4_out_of_scope_partial_files_do_not_inflate_the_count_for_a_path_scope() {
    let mut files = many_files("a", 70, "rs", 0);
    files.extend(many_files("b", 3, "rs", 100));
    let (store, root) = corpus("p4path", files);
    let b = ask(&store, "NOSUCH", &["b"], &[]);
    let wire = serde_json::to_string(&b).unwrap();
    assert!(!wire.contains("more partial-parse"), "{wire}");
    assert!(!is_unavailable(&b), "{}", describe(&b));
    // In scope for a/: exactly 6 are past the cap, not 9.
    let a = ask(&store, "NOSUCH", &["a"], &[]);
    let why = reason(&a);
    assert!(why.contains("6 more partial-parse file(s)"), "{why}");
    // Unscoped: 73 total, 9 past the cap.
    let u = ask(&store, "NOSUCH", &[], &[]);
    let note = u.walk_incomplete.clone().unwrap_or_default();
    assert!(note.contains("9 more partial-parse file(s)"), "{note}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Same, by language scope (the scope that has no path prefix at all).
#[test]
fn p4_out_of_scope_partial_files_do_not_inflate_the_count_for_a_language_scope() {
    let mut files = many_files("a", 70, "rs", 0);
    files.extend(many_files("b", 2, "py", 100));
    let (store, root) = corpus("p4lang", files);
    let a = ask(&store, "NOSUCH", &[], &["python"]);
    let wire = serde_json::to_string(&a).unwrap();
    assert!(!wire.contains("more partial-parse"), "{wire}");
    assert!(!is_unavailable(&a), "{}", describe(&a));
    let _ = std::fs::remove_dir_all(&root);
}

/// Zero partial files: a plain Available zero, no note, both scoped and not.
#[test]
fn p4_zero_partial_files_is_a_plain_available_zero() {
    let (store, root) = corpus("p4zero", vec![F::new("a.rs", RS, &[], &[])]);
    for paths in [&[][..], &["a.rs"][..]] {
        let a = ask(&store, "NOSUCH", paths, &[]);
        assert!(!is_unavailable(&a), "{}", describe(&a));
        assert!(!serde_json::to_string(&a)
            .unwrap()
            .contains("could not be checked"));
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The whole reason is capped even when many long unchecked paths are named.
#[test]
fn p4_the_reason_stays_bounded_with_many_long_paths() {
    let deep = "d".repeat(150);
    let mut files = Vec::new();
    for i in 0..10 {
        let f = F::hole(
            &format!("{deep}/{deep}{i}/m.rs"),
            RS,
            RS_HOLE,
            "LOST_IN_HOLE",
        )
        .on_disk(b"// changed\n");
        files.push(f);
    }
    let (store, root) = corpus("p4long", files);
    let a = ask(&store, "NOSUCH", &[&deep], &[]);
    let why = reason(&a);
    assert!(why.len() <= 4_096, "reason is {} bytes", why.len());
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- probe 5

/// explore and search agree: unscoped miss carries the note, scoped miss is
/// Unavailable, hit carries nothing.
#[test]
fn p5_explore_matches_search_on_an_unscoped_miss() {
    let (store, root) = partial_changed_beside_a_hit();
    let s = ask(&store, "nothere", &[], &[]);
    let e = explore(&store, "nothere", &[], &[]);
    let _ = std::fs::remove_dir_all(&root);
    let s_note = s
        .walk_incomplete
        .clone()
        .unwrap_or_default()
        .contains("could not be checked");
    let e_note = e
        .definitions
        .walk_incomplete
        .clone()
        .unwrap_or_default()
        .contains("could not be checked");
    assert!(s_note, "{}", describe(&s));
    assert!(
        e_note,
        "explore dropped the unchecked note: {:?}",
        e.definitions.walk_incomplete
    );
}

#[test]
fn p5_explore_matches_search_on_a_scoped_miss() {
    let (store, root) = partial_changed_beside_a_hit();
    let s = ask(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    let e = explore(&store, "LOST_IN_HOLE", &["consts.rs"], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(is_unavailable(&s));
    assert!(
        matches!(
            e.definitions.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "{:?}",
        e.definitions.resolution
    );
}

#[test]
fn p5_explore_hit_carries_no_unchecked_note() {
    let (store, root) = partial_changed_beside_a_hit();
    let e = explore(&store, "present", &[], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert!(e.definitions.total >= 1);
    let wire = serde_json::to_string(&e).unwrap();
    assert!(!wire.contains("could not be checked"), "{wire}");
}

/// explore with a dotted extension-named query behaves like search's.
#[test]
fn p5_explore_dotted_extension_query_matches_search() {
    let src = "export class Config {\n  kept() {}\n  ts() { return 1; }\n}\n";
    let (store, root) = corpus(
        "p5dot",
        vec![F::hole("c.ts", src, "ts() { return 1; }", "ts")],
    );
    let s = ask(&store, "Config.ts", &["c.ts"], &[]);
    let e = explore(&store, "Config.ts", &["c.ts"], &[]);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(
        is_unavailable(&s),
        matches!(
            e.definitions.resolution,
            ResolutionAvailability::Unavailable { .. }
        ),
        "search and explore disagree"
    );
}

// ---------------------------------------------------------------- probe 6

/// Timing (release, `--ignored --nocapture`): 2000 partial files; a hit, an
/// unscoped miss, a scoped miss. min-of-5 each.
#[test]
#[ignore]
fn p6_timing_hit_and_miss_on_2000_partial_files() {
    let mut files = Vec::new();
    for i in 0..2000 {
        let body = "x".repeat(400);
        let src = format!(
            "pub const KEPT_{i:04}: u32 = 1;\n// {body}\npub const FILL_{i:04}: u32 = 2;\n"
        );
        let hole = format!("pub const FILL_{i:04}: u32 = 2;");
        files.push(F::hole(
            &format!("d{}/m{i:04}.rs", i % 20),
            &src,
            &hole,
            &format!("FILL_{i:04}"),
        ));
    }
    let (store, root) = corpus("p6time", files);
    let time = |query: &str, paths: &[&str]| {
        let mut best = std::time::Duration::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            for _ in 0..5 {
                let _ = ask(&store, query, paths, &[]);
            }
            best = best.min(t.elapsed() / 5);
        }
        best
    };
    println!("TIMING hit            {:?}", time("KEPT_0042", &[]));
    println!("TIMING hit scoped     {:?}", time("KEPT_0042", &["d2"]));
    println!("TIMING miss unscoped  {:?}", time("NOSUCHNAMEATALL", &[]));
    println!("TIMING miss scoped    {:?}", time("NOSUCHNAMEATALL", &["d2"]));
    let _ = std::fs::remove_dir_all(&root);
}
