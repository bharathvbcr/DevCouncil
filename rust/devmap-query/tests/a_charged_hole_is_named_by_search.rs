//! GitPulse ft-75ddb66b, the search half: a name that is declared inside a
//! charged partial-parse range of an INDEXED file is missing from the index, and
//! a miss has to say where it may be.
//!
//! Measured on the real store: `search DEFAULT_PARSE_BUDGET` answered
//! `Unavailable: outside index ... a COBOL parser, a JSON tree` — paths nobody
//! ever read — while the file that holds the declaration was indexed, held an
//! `ERROR` region over it, and was never named. Scoped to that file, the same
//! search answered `Available, total: 0`.
//!
//! The fixture is injected rather than provoked (an `ERROR` node needs a
//! grammar fault): `consts.rs` loses one declaration to a charged region,
//! another survives, and one path was skipped for size. The recovery of the
//! dropped symbol is the extraction half of the card and is not tested here.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, TextRange};
use devmap_query::{NameQueryFilter, Request, StoreQueryEngine};
use devmap_resolve::Resolver;
use devmap_store::{DiscoveryRefusal, GenerationWriteOpts, Store};
use std::path::PathBuf;

const OVER_LIMIT_PATH: &str = "vendor/cobol_parser.rs";

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("devmap-charged-hole-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("other")).unwrap();
    dir.canonicalize().unwrap()
}

/// `source` is written as `consts.rs`; each `lost` line loses its symbol and
/// becomes an error region of the file's `Partial` outcome.
fn corpus(name: &str, source: &str, lost: &[&str]) -> (Store, PathBuf) {
    let root = scratch(name);
    std::fs::write(root.join("consts.rs"), source).unwrap();
    std::fs::write(root.join("other/x.rs"), "pub const ELSEWHERE: u32 = 9;\n").unwrap();
    let mut consts: Extraction = extract_file("consts.rs", source);
    let mut regions = Vec::new();
    for line in lost {
        let start = source
            .find(line)
            .unwrap_or_else(|| panic!("{line} is in the source"));
        regions.push(TextRange {
            start_byte: start,
            end_byte: start + line.len(),
        });
    }
    consts.symbols.retain(|symbol| {
        !lost
            .iter()
            .any(|line| line.contains(&format!("const {}:", symbol.name)))
    });
    consts.parse_outcome = ParseOutcome::Partial {
        error_ranges: regions,
        nameless_ranges: Vec::new(),
    };
    let extractions = vec![
        consts,
        extract_file("other/x.rs", "pub const ELSEWHERE: u32 = 9;\n"),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let analysis = devmap_analyze::analyze_with_discovery(
        &extractions,
        &resolution,
        devmap_analyze::DiscoveryCoverage::refused(0).with_over_limit(vec![
            devmap_analyze::model::OverLimitFile {
                path: OVER_LIMIT_PATH.to_string(),
                bytes: 2_000_000,
                limit: 1_048_576,
            },
        ]),
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
                over_limit: Some(vec![DiscoveryRefusal {
                    path: OVER_LIMIT_PATH.to_string(),
                    reason: "2000000 bytes exceeds the 1048576 byte source ceiling".to_string(),
                }]),
                ..Default::default()
            },
        )
        .unwrap();
    (store, root)
}

fn ask(
    store: &Store,
    query: &str,
    scope: &[&str],
) -> devmap_query::Response<devmap_query::SymbolHit> {
    let paths: Vec<String> = scope.iter().map(|path| (*path).to_string()).collect();
    let filter = NameQueryFilter::new(&paths, &[], &[]).unwrap();
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

fn reason(answer: &devmap_query::Response<devmap_query::SymbolHit>) -> String {
    match &answer.resolution {
        devmap_query::ResolutionAvailability::Unavailable { reason } => reason.clone(),
        other => panic!(
            "expected Unavailable, got {other:?} with total {}",
            answer.total
        ),
    }
}

const SOURCE: &str = "pub const KEPT: u32 = 1;\npub const LOST_IN_HOLE: u32 = 2;\n";
const HOLE: &str = "pub const LOST_IN_HOLE: u32 = 2;";

/// Criterion 1 and 3: the declaring file and its lines, with the over-limit
/// path demoted to a labelled secondary.
#[test]
fn an_unscoped_miss_names_the_indexed_file_and_its_lines_before_any_unread_path() {
    let (store, root) = corpus("unscoped", SOURCE, &[HOLE]);
    let answer = ask(&store, "LOST_IN_HOLE", &[]);
    let reason = reason(&answer);
    let (primary, secondary) = reason
        .split_once("also unread")
        .unwrap_or_else(|| panic!("the over-limit path is listed as secondary: {reason}"));
    assert!(
        primary.contains("consts.rs lines 2-2") && primary.contains("may declare"),
        "{reason}"
    );
    assert!(
        !primary.contains(OVER_LIMIT_PATH),
        "the never-read path is not presented as the declaring file: {reason}"
    );
    assert!(secondary.contains(OVER_LIMIT_PATH), "{reason}");

    // The survivor is still an ordinary hit.
    let kept = ask(&store, "KEPT", &[]);
    assert_eq!(kept.total, 1);
    assert!(matches!(
        kept.resolution,
        devmap_query::ResolutionAvailability::Available
    ));
    let _ = std::fs::remove_dir_all(&root);
}

/// Criterion 2: scoped to the holed file the answer is never `Available, 0`.
#[test]
fn a_search_scoped_to_the_holed_file_is_unavailable_and_names_the_hole() {
    let (store, root) = corpus("scoped", SOURCE, &[HOLE]);
    let answer = ask(&store, "LOST_IN_HOLE", &["consts.rs"]);
    assert_eq!(answer.total, 0);
    let reason = reason(&answer);
    assert!(reason.contains("consts.rs lines 2-2"), "{reason}");
    assert!(
        !reason.contains(OVER_LIMIT_PATH),
        "the over-limit path is outside this scope and cannot be blamed: {reason}"
    );
    let kept = ask(&store, "KEPT", &["consts.rs"]);
    assert_eq!(
        kept.total, 1,
        "a surviving symbol in the same file is returned"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A hole the scope excludes is not this answer's business, and neither is a
/// range that does not contain the name.
#[test]
fn a_hole_outside_the_scope_or_without_the_name_is_not_blamed() {
    let (store, root) = corpus("excluded", SOURCE, &[HOLE]);
    let elsewhere = ask(&store, "LOST_IN_HOLE", &["other"]);
    assert_eq!(elsewhere.total, 0);
    assert!(
        matches!(
            elsewhere.resolution,
            devmap_query::ResolutionAvailability::Available
        ),
        "neither the hole in consts.rs nor the over-limit path is in scope: {:?}",
        elsewhere.resolution
    );
    let unrelated = ask(&store, "NO_SUCH_NAME", &["consts.rs"]);
    assert!(
        matches!(
            unrelated.resolution,
            devmap_query::ResolutionAvailability::Available
        ),
        "a range that does not contain the name cannot be hiding it: {:?}",
        unrelated.resolution
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A name that only a comment or a string spells in the range is "may be".
#[test]
fn a_name_only_in_a_comment_inside_the_range_is_labelled_as_possibly_not_a_declaration() {
    let source = "pub const KEPT: u32 = 1;\n// MENTIONED_ONLY is gone\n";
    let (store, root) = corpus("comment", source, &["// MENTIONED_ONLY is gone"]);
    let reason = reason(&ask(&store, "MENTIONED_ONLY", &["consts.rs"]));
    assert!(reason.contains("consts.rs lines 2-2"), "{reason}");
    assert!(reason.contains("may not be a declaration"), "{reason}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Identifier boundaries: a longer name is not the name.
#[test]
fn a_longer_identifier_that_contains_the_name_is_not_a_match() {
    let source = "pub const KEPT: u32 = 1;\npub const LOST_IN_HOLE_TOO: u32 = 2;\n";
    let (store, root) = corpus(
        "boundary",
        source,
        &["pub const LOST_IN_HOLE_TOO: u32 = 2;"],
    );
    let answer = ask(&store, "LOST_IN_HOLE", &["consts.rs"]);
    assert!(
        matches!(
            answer.resolution,
            devmap_query::ResolutionAvailability::Available
        ),
        "{:?}",
        answer.resolution
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Unicode names, matched the way search matches them (case-insensitively).
#[test]
fn a_unicode_name_in_a_charged_range_is_found() {
    let source = "pub const KEPT: u32 = 1;\npub const ÜBERMASS: u32 = 2;\n";
    let (store, root) = corpus("unicode", source, &["pub const ÜBERMASS: u32 = 2;"]);
    let reason = reason(&ask(&store, "übermass", &["consts.rs"]));
    assert!(reason.contains("consts.rs lines 2-2"), "{reason}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Many ranges: the list is capped and both numbers are carried; ranges the
/// stored sentence elided are reported as unchecked rather than as clean.
#[test]
fn many_charged_ranges_are_capped_with_the_total_and_the_unlisted_are_carried() {
    let lines: Vec<String> = (0..12)
        .map(|n| format!("pub const DUP_{n}: u32 = {n}; // DUPED\n"))
        .collect();
    let source = format!("pub const KEPT: u32 = 1;\n{}", lines.concat());
    let lost: Vec<String> = lines
        .iter()
        .map(|line| line.trim_end().to_string())
        .collect();
    let lost_refs: Vec<&str> = lost.iter().map(String::as_str).collect();
    let (store, root) = corpus("many", &source, &lost_refs);
    let reason = reason(&ask(&store, "DUPED", &["consts.rs"]));
    assert!(
        reason.contains("8 charged partial-parse range(s)"),
        "{reason}"
    );
    assert!(
        reason.contains("and 3 more"),
        "five shown of eight: {reason}"
    );
    assert!(
        reason.contains("4 charged range(s) not listed in the stored reason"),
        "the stored sentence lists eight of twelve: {reason}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
