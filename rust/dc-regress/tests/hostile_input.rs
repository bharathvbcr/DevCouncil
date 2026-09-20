//! Adversarial input. Every test here is an attempt to make the analysis
//! panic, hang, wrap, or — worst of all — answer confidently and wrongly.
//!
//! The last of those is the one worth the most attention. A crash is loud and
//! gets fixed; a plausible wrong attribution is indistinguishable from a right
//! one and gets acted on.

use dc_regress::blame::{BlameLine, FileBlame};
use dc_regress::join::{attribute, Attribution};
use dc_regress::rank::{rank, RankInputs, TouchedSymbol};
use dc_regress::{BlobIdentity, GraphSymbol};

fn basis() -> BlobIdentity {
    BlobIdentity::Blob("same".into())
}

fn line(no: u32, commit: &str) -> BlameLine {
    BlameLine {
        line_no: no,
        commit: commit.to_string(),
        author: "ada".into(),
        author_mail: "ada@example.com".into(),
        author_time: 1,
        uncommitted: commit.chars().all(|c| c == '0'),
    }
}

fn blame(lines: Vec<BlameLine>) -> FileBlame {
    FileBlame {
        path: "f.rs".into(),
        lines,
        basis: basis(),
    }
}

fn symbol(start: usize, end: usize) -> GraphSymbol {
    GraphSymbol {
        qualified_name: "f.rs::s".into(),
        file_path: "f.rs".into(),
        span_start: start,
        span_end: end,
        body_exact: None,
    }
}

fn attributed(outcome: Attribution) -> Vec<dc_regress::SymbolBlame> {
    match outcome {
        Attribution::Attributed(rows) => rows,
        Attribution::BasisMismatch { .. } => panic!("bases match in this fixture"),
    }
}

/// The property that keeps the attribution loop finite. A span is clamped to
/// the content, so the line range it produces can never exceed the file's own
/// line count however absurd the stored offsets are — which is what stops
/// `start_line..=end_line` from becoming a four-billion-iteration walk.
#[test]
fn an_absurd_span_cannot_produce_an_unbounded_line_walk() {
    let content = "a\nb\nc\n";
    let rows = attributed(attribute(
        &[symbol(usize::MAX / 2, usize::MAX)],
        &basis(),
        &blame(vec![line(1, "c1")]),
        content,
    ));
    assert!(
        rows[0].end_line <= content.matches('\n').count() as u32 + 1,
        "the line range is bounded by the content, not by the stored offsets"
    );
}

#[test]
fn an_empty_file_with_a_populated_blame_does_not_panic() {
    let rows = attributed(attribute(
        &[symbol(0, 0)],
        &basis(),
        &blame(vec![line(1, "c1"), line(2, "c2"), line(3, "c3")]),
        "",
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].start_line, 1);
}

#[test]
fn a_blame_with_no_lines_attributes_nothing_without_failing() {
    let rows = attributed(attribute(
        &[symbol(0, 5)],
        &basis(),
        &blame(Vec::new()),
        "a\nb\n",
    ));
    assert!(rows[0].commits.is_empty());
    assert_eq!(rows[0].uncommitted_lines, 0);
}

/// Blame lines past the end of the content are simply not found. They must not
/// shift attribution for the lines that do exist.
#[test]
fn blame_lines_beyond_the_file_are_ignored_not_folded_in() {
    let rows = attributed(attribute(
        &[symbol(0, 3)],
        &basis(),
        &blame(vec![line(1, "real"), line(9_999, "phantom")]),
        "a\nb\n",
    ));
    assert!(
        rows[0].commits.iter().all(|c| c.commit != "phantom"),
        "a blame line outside every symbol's range belongs to no symbol"
    );
}

/// Two blame rows claiming the same line is not something git produces. It
/// must not panic, and it must not double-count.
#[test]
fn duplicate_line_numbers_do_not_double_count() {
    let rows = attributed(attribute(
        &[symbol(0, 6)],
        &basis(),
        &blame(vec![line(1, "a"), line(1, "b"), line(2, "a")]),
        "x\ny\nz\n",
    ));
    let total: u32 = rows[0].commits.iter().map(|c| c.lines).sum();
    assert!(
        total <= 3,
        "a line can be attributed once; {total} exceeds the lines in range"
    );
}

/// A line number of zero cannot exist — blame is one-based — and must not
/// underflow the range walk.
#[test]
fn a_zero_line_number_does_not_underflow() {
    let rows = attributed(attribute(
        &[symbol(0, 3)],
        &basis(),
        &blame(vec![line(0, "impossible"), line(1, "real")]),
        "a\nb\n",
    ));
    assert!(rows[0].commits.iter().all(|c| c.commit != "impossible"));
}

#[test]
fn a_line_number_at_the_type_maximum_does_not_overflow_the_walk() {
    let rows = attributed(attribute(
        &[symbol(0, 3)],
        &basis(),
        &blame(vec![line(u32::MAX, "edge"), line(1, "real")]),
        "a\nb\n",
    ));
    assert_eq!(rows[0].commits.len(), 1);
    assert_eq!(rows[0].commits[0].commit, "real");
}

/// Many symbols over one blame. This is the shape that would be O(k·n) if the
/// blame were rescanned per symbol; it completes because it is indexed once.
#[test]
fn many_symbols_over_a_large_blame_completes() {
    let content: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
    let lines: Vec<BlameLine> = (1..=20_000).map(|n| line(n, "c1")).collect();
    let symbols: Vec<GraphSymbol> = (0..2_000)
        .map(|i| GraphSymbol {
            qualified_name: format!("f.rs::s{i}"),
            file_path: "f.rs".into(),
            span_start: i * 8,
            span_end: i * 8 + 40,
            body_exact: None,
        })
        .collect();
    let rows = attributed(attribute(&symbols, &basis(), &blame(lines), &content));
    assert_eq!(rows.len(), 2_000);
}

/// Content whose every character is multi-byte. Byte offsets and character
/// offsets diverge maximally here, which is where an implementation that
/// confused the two would produce its largest error.
#[test]
fn content_that_is_entirely_multibyte_attributes_correctly() {
    // Four bytes per character, ten per line.
    let content: String = (0..50).map(|_| format!("{}\n", "🎉".repeat(10))).collect();
    let lines: Vec<BlameLine> = (1..=50)
        .map(|n| line(n, if n <= 25 { "first" } else { "second" }))
        .collect();
    // A span covering exactly the first ten lines: 10 lines x (10 x 4 bytes + 1 newline).
    let bytes_per_line = 10 * 4 + 1;
    let rows = attributed(attribute(
        &[symbol(0, bytes_per_line * 10 - 1)],
        &basis(),
        &blame(lines),
        &content,
    ));
    assert_eq!(rows[0].start_line, 1);
    assert_eq!(
        rows[0].end_line, 10,
        "ten lines of four-byte characters are ten lines, not forty"
    );
    assert_eq!(rows[0].commits.len(), 1);
    assert_eq!(rows[0].commits[0].lines, 10);
}

/// A span that begins mid-character. Byte offsets from a stale or buggy index
/// can land anywhere, and the arithmetic must not panic on a boundary that is
/// not a character boundary — the exact failure that used to abort exports.
#[test]
fn a_span_starting_mid_character_does_not_panic() {
    let content = "🎉🎉🎉\n";
    for offset in 0..content.len() {
        let rows = attributed(attribute(
            &[symbol(offset, content.len())],
            &basis(),
            &blame(vec![line(1, "c1")]),
            content,
        ));
        assert!(rows[0].start_line >= 1);
    }
}

/// Every symbol being uncommitted is a real state — a new file in the working
/// tree — and must read as "no commit to suspect", not as "nothing touched
/// this".
#[test]
fn a_wholly_uncommitted_symbol_reports_no_suspects_but_counts_its_lines() {
    let zero = "0".repeat(40);
    let rows = attributed(attribute(
        &[symbol(0, 6)],
        &basis(),
        &blame(vec![line(1, &zero), line(2, &zero), line(3, &zero)]),
        "a\nb\nc\n",
    ));
    assert!(rows[0].commits.is_empty());
    assert_eq!(
        rows[0].uncommitted_lines, 3,
        "the lines exist and are named; they simply have no commit yet"
    );
}

/// A commit id that is not a hex string at all. The analysis does not validate
/// ids — git supplies them — but it must not treat a strange one as a special
/// value.
#[test]
fn a_non_hex_commit_id_is_carried_through_as_data() {
    let rows = attributed(attribute(
        &[symbol(0, 3)],
        &basis(),
        &blame(vec![line(1, "not-a-sha")]),
        "a\nb\n",
    ));
    assert_eq!(rows[0].commits[0].commit, "not-a-sha");
}

/// A commit id of all zeroes is the uncommitted marker at *any* length, so a
/// three-character one must not be mistaken for a real commit.
#[test]
fn a_short_all_zero_id_is_still_uncommitted() {
    let rows = attributed(attribute(
        &[symbol(0, 3)],
        &basis(),
        &blame(vec![line(1, "000")]),
        "a\nb\n",
    ));
    assert!(rows[0].commits.is_empty());
    assert_eq!(rows[0].uncommitted_lines, 1);
}

/// Ranking over a large hostile input: every commit touching every symbol at
/// the maximum line count. This is where integer overflow would show.
#[test]
fn ranking_a_saturating_corpus_stays_ordered_and_finite() {
    let inputs: Vec<RankInputs> = (0..500)
        .map(|i| RankInputs {
            commit: format!("c{i:04}"),
            author: "ada".into(),
            author_time: i as i64,
            touched: (0..20)
                .map(|j| TouchedSymbol {
                    qualified_name: format!("s{j}"),
                    file_path: "f.rs".into(),
                    distance: 0,
                    lines: u32::MAX,
                    body_changed: Some(true),
                })
                .collect(),
        })
        .collect();
    let ranked = rank(inputs);
    assert_eq!(ranked.len(), 500);
    // Saturated scores are all equal, so the tie-breakers decide — and they
    // must still produce a strict, reproducible order.
    let commits: Vec<&str> = ranked.iter().map(|s| s.commit.as_str()).collect();
    let mut sorted = commits.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        commits.len(),
        "every suspect appears exactly once"
    );
}

/// The basis check must not be bypassable by an empty string, which is the
/// value a careless caller would supply for "I do not know".
#[test]
fn an_empty_blob_id_does_not_compare_equal_to_a_missing_one() {
    let empty = BlobIdentity::Blob(String::new());
    assert!(!empty.comparable_with(&BlobIdentity::Unknown));
    assert!(
        empty.comparable_with(&BlobIdentity::Blob(String::new())),
        "two identical ids compare equal; the guard against a careless caller \
         is that `Unknown` exists and is never equal, not that empty strings \
         are special-cased"
    );
}

/// Mixed uncommitted and committed lines in one symbol must apportion both.
#[test]
fn a_partially_uncommitted_symbol_reports_both_halves() {
    let zero = "0".repeat(64);
    let rows = attributed(attribute(
        &[symbol(0, 8)],
        &basis(),
        &blame(vec![
            line(1, "c1"),
            line(2, &zero),
            line(3, "c1"),
            line(4, &zero),
        ]),
        "a\nb\nc\nd\n",
    ));
    assert_eq!(rows[0].uncommitted_lines, 2);
    assert_eq!(rows[0].commits.len(), 1);
    assert_eq!(rows[0].commits[0].lines, 2);
}
