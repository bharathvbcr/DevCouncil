//! Adversarial input to the forward direction.
//!
//! The diff this reads is not trusted input. It is produced by a `git` whose
//! version is unknown, over content an attacker may have written, in a
//! repository whose paths, encodings and line endings nobody controls. And the
//! graph on the other side of the join is a persisted artifact that may
//! disagree with the file it describes.
//!
//! The bar is not "does not crash". It is: **no input produces a confident
//! wrong answer.** A refusal is a fine outcome. A silently short list is not,
//! because a short list of affected files reads exactly like a small change.

use dc_regress::blast::{blast_change_with_program, UnattributedReason, MAX_SEED_SYMBOLS};
use dc_regress::change::{parse_diff, ChangeRefusal, ChangeStatus, ChangedRange, FileChange};
use dc_regress::{AffectedTestFile, BlobIdentity, ChangeSet, CodeGraph, ConeEntry, GraphSymbol};

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

fn temp_root(label: &str) -> PathBuf {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "dc-regress-hostile-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
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

/// A repository holding `content` at `path`, and the blob id of those bytes.
fn repo_with(label: &str, path: &str, content: &str) -> (PathBuf, String, BlobIdentity) {
    let root = temp_root(label);
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Ada"]);
    git(&root, &["config", "user.email", "ada@example.com"]);
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(&full, content).expect("write");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-m", "base", "--no-gpg-sign"]);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let blob = BlobIdentity::Blob(git(&root, &["rev-parse", &format!("{head}:{path}")]));
    (root, head, blob)
}

struct Graph {
    symbols: Vec<GraphSymbol>,
    basis: BlobIdentity,
}

impl CodeGraph for Graph {
    fn symbols_in(&self, file: &str) -> (Vec<GraphSymbol>, BlobIdentity) {
        (
            self.symbols
                .iter()
                .filter(|s| s.file_path == file)
                .cloned()
                .collect(),
            self.basis.clone(),
        )
    }
    fn cone(&self, _symbol: &str, _depth: u32) -> (Vec<ConeEntry>, bool) {
        (Vec::new(), false)
    }
    fn impacted(&self, _seeds: &[String], _depth: u32) -> (Vec<ConeEntry>, bool) {
        (Vec::new(), false)
    }
    fn affected_tests(&self, _seeds: &[String], _depth: u32) -> (Vec<AffectedTestFile>, bool) {
        (Vec::new(), false)
    }
    fn resolve_symptom(&self, _symptom: &str) -> Vec<String> {
        Vec::new()
    }
}

fn range(start: u32, end: u32) -> ChangedRange {
    ChangedRange {
        start_line: start,
        end_line: end,
        deletion_only: false,
    }
}

fn change(path: &str, ranges: Vec<ChangedRange>) -> ChangeSet {
    ChangeSet {
        files: vec![FileChange {
            path: path.to_string(),
            renamed_from: None,
            status: ChangeStatus::Modified,
            ranges,
        }],
        capped: false,
    }
}

fn run(repo: &Path, graph: &Graph, set: &ChangeSet, head: &str) -> dc_regress::BlastReport {
    blast_change_with_program(
        std::ffi::OsStr::new("git"),
        repo,
        graph,
        set,
        "hostile",
        head,
        3,
    )
}

// ---------------------------------------------------------------- the parser

/// `@@ -1 +1,4294967295 @@`. The end line is computed from start plus count,
/// and an unchecked add wraps to a range that ends before it begins.
#[test]
fn a_hunk_count_at_the_type_maximum_does_not_wrap_the_range() {
    let set = parse_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1,4294967295 @@\n")
        .expect("parses");
    let range = &set.files[0].ranges[0];
    assert!(
        range.end_line >= range.start_line,
        "saturating, never wrapped: {range:?}"
    );
}

#[test]
fn a_hunk_start_at_the_type_maximum_does_not_wrap_the_range() {
    let set = parse_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +4294967295,9 @@\n")
        .expect("parses");
    let range = &set.files[0].ranges[0];
    assert_eq!(range.start_line, u32::MAX);
    assert!(range.end_line >= range.start_line);
}

/// A count that does not fit `u32` at all. Refused by name rather than
/// silently wrapped into a small plausible number.
#[test]
fn a_hunk_count_past_the_type_is_refused_not_wrapped() {
    assert!(matches!(
        parse_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1,99999999999999999999 @@\n"),
        Err(ChangeRefusal::UnparsableHunk { .. })
    ));
}

#[test]
fn a_negative_looking_count_is_refused() {
    assert!(matches!(
        parse_diff("diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +-5,3 @@\n"),
        Err(ChangeRefusal::UnparsableHunk { .. })
    ));
}

/// The widest range the type permits, summed. Computing `end - start + 1` in
/// `u32` panics in a debug build on exactly this input.
#[test]
fn counting_the_widest_possible_range_does_not_overflow() {
    let set = ChangeSet {
        files: vec![FileChange {
            path: "f".into(),
            renamed_from: None,
            status: ChangeStatus::Modified,
            ranges: vec![ChangedRange {
                start_line: 0,
                end_line: u32::MAX,
                deletion_only: false,
            }],
        }],
        capped: false,
    };
    assert_eq!(set.touched_lines(), u64::from(u32::MAX) + 1);
}

#[test]
fn an_empty_diff_is_an_empty_change_not_an_error() {
    let set = parse_diff("").expect("an empty diff parses");
    assert!(set.files.is_empty());
    assert_eq!(set.touched_lines(), 0);
}

/// A header with no hunks at all — a mode change. The file is reported as
/// changed with no ranges, which is different from not being reported.
#[test]
fn a_header_with_no_hunks_still_reports_the_file() {
    let set =
        parse_diff("diff --git a/f b/f\nold mode 100644\nnew mode 100755\n--- a/f\n+++ b/f\n")
            .expect("parses");
    assert_eq!(set.files.len(), 1);
    assert!(set.files[0].ranges.is_empty());
}

/// Hunks arriving before any file header have no file to belong to. Dropping
/// them is right; panicking or attributing them to the next file is not.
#[test]
fn hunks_with_no_file_header_are_dropped_without_panicking() {
    let set =
        parse_diff("@@ -1 +1 @@\n@@ -5 +5 @@\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -9 +9 @@\n")
            .expect("parses");
    assert_eq!(set.files.len(), 1);
    assert_eq!(
        set.files[0].ranges.len(),
        1,
        "only the hunk that followed a header belongs to a file"
    );
    assert_eq!(set.files[0].ranges[0].start_line, 9);
}

/// A path with non-ASCII bytes arrives raw under `core.quotePath=false` and
/// must survive as itself — a mangled path matches nothing in the graph and
/// the file silently drops out of the answer.
#[test]
fn a_non_ascii_path_survives_the_parse_intact() {
    let set = parse_diff("diff --git a/src/日本.rs b/src/日本.rs\n--- a/src/日本.rs\n+++ b/src/日本.rs\n@@ -1 +1 @@\n")
        .expect("parses");
    assert_eq!(set.files[0].path, "src/日本.rs");
}

/// A path that is literally `a/...` twice over. `strip_prefix("a/")` must
/// remove git's prefix and not a directory that happens to be named `a`.
#[test]
fn a_path_whose_own_first_directory_is_a_keeps_it() {
    let set = parse_diff("diff --git a/a/x.rs b/a/x.rs\n--- a/a/x.rs\n+++ b/a/x.rs\n@@ -1 +1 @@\n")
        .expect("parses");
    assert_eq!(
        set.files[0].path, "a/x.rs",
        "one prefix is stripped, not both"
    );
}

// ------------------------------------------------------------------ the join

const BODY: &str = "use std::fmt;\n\npub fn f() -> u32 {\n    1\n}\n";

fn f_symbol() -> GraphSymbol {
    GraphSymbol {
        qualified_name: "f.rs::f".into(),
        file_path: "f.rs".into(),
        span_start: BODY.find("pub fn f").expect("f"),
        span_end: BODY.rfind('}').expect("close") + 1,
        body_exact: None,
    }
}

/// A stored span whose end precedes its start. It describes one line, not a
/// backwards range, and must not underflow the overlap arithmetic.
#[test]
fn an_inverted_stored_span_does_not_break_the_overlap() {
    let (repo, head, basis) = repo_with("inverted", "f.rs", BODY);
    let graph = Graph {
        symbols: vec![GraphSymbol {
            span_start: 40,
            span_end: 2,
            ..f_symbol()
        }],
        basis,
    };
    let report = run(&repo, &graph, &change("f.rs", vec![range(1, 5)]), &head);
    for seed in &report.seeds {
        assert!(seed.end_line >= seed.start_line, "never inverted: {seed:?}");
    }
}

/// A stored span past the end of the content. The basis check has already
/// established these are the same bytes, so this is a degenerate stored span
/// rather than the wrong file — clamped, never wrapped.
#[test]
fn a_stored_span_past_the_end_does_not_wrap() {
    let (repo, head, basis) = repo_with("past", "f.rs", BODY);
    let graph = Graph {
        symbols: vec![GraphSymbol {
            span_start: usize::MAX - 10,
            span_end: usize::MAX,
            ..f_symbol()
        }],
        basis,
    };
    let report = run(&repo, &graph, &change("f.rs", vec![range(1, 5)]), &head);
    for seed in &report.seeds {
        assert!(seed.end_line >= seed.start_line);
    }
}

/// A caller-supplied range that runs backwards. It names no lines, so it hits
/// nothing — and the miss is reported rather than being turned into a
/// gigantic `changed_lines` by an underflowing subtraction.
#[test]
fn a_backwards_range_names_no_lines_and_is_reported() {
    let (repo, head, basis) = repo_with("backwards", "f.rs", BODY);
    let graph = Graph {
        symbols: vec![f_symbol()],
        basis,
    };
    let report = run(&repo, &graph, &change("f.rs", vec![range(9_000, 2)]), &head);
    assert!(report.seeds.is_empty(), "{report:#?}");
    assert_eq!(report.unattributed.len(), 1);
    assert!(!report.complete);
}

/// An empty file has no lines. Every range is past its end, and that is a
/// disagreement between the diff and the blob rather than module-level code.
#[test]
fn an_empty_file_places_nothing_and_says_the_diff_disagrees() {
    let (repo, head, basis) = repo_with("empty", "f.rs", "");
    let graph = Graph {
        symbols: vec![GraphSymbol {
            qualified_name: "f.rs::f".into(),
            file_path: "f.rs".into(),
            span_start: 0,
            span_end: 0,
            body_exact: None,
        }],
        basis,
    };
    let report = run(&repo, &graph, &change("f.rs", vec![range(5, 9)]), &head);
    assert!(matches!(
        report.unattributed[0].reason,
        UnattributedReason::BeyondEndOfFile { .. }
    ));
}

/// CRLF content. `git diff` counts lines by `\n`, and so does the line index;
/// if one of them counted `\r\n` pairs differently every placement would shift
/// by one per preceding line.
#[test]
fn crlf_content_does_not_shift_placement() {
    let body = "use std::fmt;\r\n\r\npub fn f() -> u32 {\r\n    1\r\n}\r\n";
    let (repo, head, basis) = repo_with("crlf", "f.rs", body);
    let graph = Graph {
        symbols: vec![GraphSymbol {
            qualified_name: "f.rs::f".into(),
            file_path: "f.rs".into(),
            span_start: body.find("pub fn f").expect("f"),
            span_end: body.rfind('}').expect("close") + 1,
            body_exact: None,
        }],
        basis,
    };
    // Line 4 is `    1`, inside the function on any counting of CRLF.
    let report = run(&repo, &graph, &change("f.rs", vec![range(4, 4)]), &head);
    assert_eq!(report.seeds.len(), 1, "{report:#?}");
    assert_eq!(
        (report.seeds[0].start_line, report.seeds[0].end_line),
        (3, 5)
    );
}

/// A file with no trailing newline. The final line still exists and a change
/// to it must land in the symbol that contains it.
#[test]
fn a_file_without_a_trailing_newline_still_places_its_last_line() {
    let body = "pub fn f() -> u32 {\n    1\n}";
    let (repo, head, basis) = repo_with("no-trailing", "f.rs", body);
    let graph = Graph {
        symbols: vec![GraphSymbol {
            qualified_name: "f.rs::f".into(),
            file_path: "f.rs".into(),
            span_start: 0,
            span_end: body.len(),
            body_exact: None,
        }],
        basis,
    };
    let report = run(&repo, &graph, &change("f.rs", vec![range(3, 3)]), &head);
    assert_eq!(report.seeds.len(), 1, "{report:#?}");
}

/// Nested symbols — a type and its methods, an impl block and its functions.
/// A line inside the inner one is inside the outer one too, and both really
/// did change.
#[test]
fn a_line_inside_nested_symbols_seeds_both() {
    let (repo, head, basis) = repo_with("nested", "f.rs", BODY);
    let graph = Graph {
        symbols: vec![
            GraphSymbol {
                qualified_name: "f.rs::Outer".into(),
                span_start: 0,
                span_end: BODY.len(),
                ..f_symbol()
            },
            f_symbol(),
        ],
        basis,
    };
    let report = run(&repo, &graph, &change("f.rs", vec![range(4, 4)]), &head);
    assert_eq!(report.seeds.len(), 2, "{report:#?}");
}

/// A file listed twice in one change. Both entries' ranges must be counted,
/// and the seed must not be double-created.
#[test]
fn the_same_file_appearing_twice_accumulates_rather_than_duplicating() {
    let (repo, head, basis) = repo_with("twice", "f.rs", BODY);
    let graph = Graph {
        symbols: vec![f_symbol()],
        basis,
    };
    let entry = |ranges: Vec<ChangedRange>| FileChange {
        path: "f.rs".into(),
        renamed_from: None,
        status: ChangeStatus::Modified,
        ranges,
    };
    let set = ChangeSet {
        files: vec![entry(vec![range(3, 3)]), entry(vec![range(4, 4)])],
        capped: false,
    };
    let report = run(&repo, &graph, &set, &head);
    assert_eq!(report.seeds.len(), 1, "one symbol, not two: {report:#?}");
    assert_eq!(
        report.seeds[0].changed_lines, 2,
        "both appearances are counted"
    );
}

/// Past the seed cap the walk runs from a prefix. Which prefix matters, and
/// that it *is* a prefix must be stated — a trimmed answer that reads as a
/// whole one is the failure this crate exists to prevent.
#[test]
fn past_the_seed_cap_the_most_changed_survive_and_the_cap_is_disclosed() {
    // One symbol per line, so a range over the whole file seeds all of them.
    let count = MAX_SEED_SYMBOLS + 50;
    let body: String = (0..count)
        .map(|index| format!("fn s{index}() {{}}\n"))
        .collect();
    let (repo, head, basis) = repo_with("seed-cap", "f.rs", &body);

    let mut symbols = Vec::new();
    let mut offset = 0usize;
    for index in 0..count {
        let line = format!("fn s{index}() {{}}\n");
        symbols.push(GraphSymbol {
            qualified_name: format!("f.rs::s{index}"),
            file_path: "f.rs".into(),
            span_start: offset,
            // Ends before the newline, so each symbol occupies exactly one
            // line and no two of them share one.
            span_end: offset + line.len() - 1,
            body_exact: None,
        });
        offset += line.len();
    }
    let graph = Graph { symbols, basis };

    let report = run(
        &repo,
        &graph,
        &change("f.rs", vec![range(1, count as u32)]),
        &head,
    );
    assert_eq!(report.seeds.len(), MAX_SEED_SYMBOLS);
    assert!(
        report.unavailable.iter().any(|u| matches!(
            u,
            dc_regress::Unavailable::SeedsCapped { touched, .. } if *touched == count
        )),
        "the cap names how many were touched, not just how many survived: {:#?}",
        report.unavailable
    );
    assert!(!report.complete);
}

/// A change whose file the repository does not contain at the post-image rev.
/// `git show` refuses, and that refusal must reach the report rather than
/// leaving the file silently absent from it.
#[test]
fn a_file_missing_at_the_post_image_rev_is_named() {
    let (repo, head, basis) = repo_with("missing", "f.rs", BODY);
    let graph = Graph {
        symbols: vec![GraphSymbol {
            qualified_name: "ghost.rs::g".into(),
            file_path: "ghost.rs".into(),
            span_start: 0,
            span_end: 10,
            body_exact: None,
        }],
        basis,
    };
    let report = run(&repo, &graph, &change("ghost.rs", vec![range(1, 2)]), &head);
    assert!(matches!(
        report.unattributed[0].reason,
        UnattributedReason::ContentUnavailable { .. }
    ));
    assert!(!report.complete);
}

/// A rev that names nothing. Every file refuses, and the report says so rather
/// than coming back empty and complete.
#[test]
fn a_nonexistent_rev_refuses_every_file_rather_than_answering_empty() {
    let (repo, _, basis) = repo_with("bad-rev", "f.rs", BODY);
    let graph = Graph {
        symbols: vec![f_symbol()],
        basis,
    };
    let report = run(
        &repo,
        &graph,
        &change("f.rs", vec![range(1, 5)]),
        "0000000000000000000000000000000000000000",
    );
    assert!(report.seeds.is_empty());
    assert!(
        !report.complete,
        "an empty answer from a rev that does not exist must never read as \
         'this change affects nothing': {report:#?}"
    );
}
