//! The substance measurement, at unit level.
//!
//! `false_accept.rs` measures this gate end to end against whole diffs. This
//! file pins the classification rules themselves, because the corpus can only
//! see the sum: a diff whose lines were misclassified into the wrong buckets in
//! compensating directions produces the same ratio and the same verdict.

use dc_verify::substance::{
    LOW_SUBSTANCE_DENOMINATOR, LOW_SUBSTANCE_NUMERATOR, MIN_LINES_TO_JUDGE, is_generated_path,
    measure,
};
use dc_verify::{ChangeStatus, FileDiff};

fn file_with(path: &str, added: &[&str], removed: &[&str]) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        old_path: None,
        status: ChangeStatus::Modified,
        added_lines: added
            .iter()
            .enumerate()
            .map(|(i, s)| (i as u32 + 1, s.to_string()))
            .collect(),
        removed_lines: removed.iter().map(|s| s.to_string()).collect(),
    }
}

// --- the classification rules ---

#[test]
fn punctuation_and_blank_lines_are_trivial() {
    let report = measure(&[file_with(
        "src/a.rs",
        &["}", "    }", "});", "", "   ", "]", ")", "{", "},", "|"],
        &[],
    )]);
    assert_eq!(report.trivial, 10, "{report:#?}");
    assert_eq!(report.substantive_lines, 0);
}

#[test]
fn a_three_character_run_is_content() {
    // The rule's boundary, both sides. `ok` is two characters and reads as
    // punctuation's neighbour; `let` is three and is real.
    let report = measure(&[file_with("src/a.rs", &["ok()", "let x = 1;"], &[])]);
    assert_eq!(report.trivial, 1, "{report:#?}");
    assert_eq!(report.substantive_lines, 1);
}

#[test]
fn a_run_must_be_contiguous_in_the_source_text() {
    // `a.b.c` has three alphanumerics but no run of three, so it is structure.
    // A rule that squeezed the punctuation out first would score it as content
    // and let a diff of chained single-letter accessors read as new work.
    let report = measure(&[file_with("src/a.rs", &["a.b.c", "a.bcd"], &[])]);
    assert_eq!(report.trivial, 1, "{report:#?}");
    assert_eq!(report.substantive_lines, 1);
}

#[test]
fn bare_block_keywords_are_trivial_however_they_are_punctuated() {
    let report = measure(&[file_with(
        "src/a.rs",
        &["} else {", "}else{", "else", "  end", "fi", "done"],
        &[],
    )]);
    assert_eq!(report.trivial, 6, "{report:#?}");
}

#[test]
fn a_keyword_with_content_beside_it_is_not_trivial() {
    // The structural list matches a *bare* keyword. `else if cached` is a
    // branch with a condition, and scoring it as punctuation would let real
    // logic disappear into the trivial bucket.
    let report = measure(&[file_with("src/a.rs", &["} else if cached {"], &[])]);
    assert_eq!(report.substantive_lines, 1, "{report:#?}");
}

#[test]
fn a_line_the_diff_also_removed_is_moved() {
    let report = measure(&[file_with(
        "src/a.rs",
        &["pub fn resolve(name: &str) -> Option<Symbol> {"],
        &["pub fn resolve(name: &str) -> Option<Symbol> {"],
    )]);
    assert_eq!(report.moved, 1, "{report:#?}");
    assert_eq!(report.substantive_lines, 0);
}

#[test]
fn a_move_is_recognised_across_files_and_through_reindentation() {
    // The two properties a per-file or whitespace-exact comparison would miss,
    // and the ones a real relocation almost always has: the text lands in a
    // different file, one indentation level in.
    let from = file_with("src/old.rs", &[], &["let resolved = index.lookup(name)?;"]);
    let to = file_with(
        "src/new.rs",
        &["        let resolved = index.lookup(name)?;"],
        &[],
    );
    let report = measure(&[from, to]);
    assert_eq!(report.moved, 1, "{report:#?}");
    assert_eq!(report.substantive_lines, 0);
}

#[test]
fn the_second_copy_of_a_line_is_repeated_and_the_first_is_not() {
    let a = file_with(
        "src/a.go",
        &["return fmt.Errorf(\"bad config: %w\", err)"],
        &[],
    );
    let b = file_with(
        "src/b.go",
        &["return fmt.Errorf(\"bad config: %w\", err)"],
        &[],
    );
    let report = measure(&[a, b]);
    assert_eq!(report.substantive_lines, 1, "{report:#?}");
    assert_eq!(report.repeated, 1);
}

#[test]
fn every_line_of_a_generated_file_is_generated_whatever_it_says() {
    // Including lines that would otherwise be substantive: the point is that
    // nobody wrote them, not that they are uninteresting text.
    let report = measure(&[file_with(
        "Cargo.lock",
        &["name = \"anyhow\"", "version = \"1.0.100\""],
        &[],
    )]);
    assert_eq!(report.generated, 2, "{report:#?}");
    assert_eq!(report.substantive_lines, 0);
}

// --- the generated-path heuristic ---

#[test]
fn generated_paths_are_recognised_by_segment_name_and_suffix() {
    for path in [
        "vendor/github.com/x/y.go",
        "web/node_modules/left-pad/index.js",
        "third_party/zlib/zlib.c",
        "Cargo.lock",
        "frontend/package-lock.json",
        "go.sum",
        "api/service.pb.go",
        "api/service_pb2.py",
        "src/model_generated.go",
        "web/bundle.min.js",
        "tests/__snapshots__/view.snap",
    ] {
        assert!(is_generated_path(path), "{path} should be generated");
    }
}

#[test]
fn a_windows_separator_does_not_hide_a_vendored_path() {
    // A caller may hand this crate paths straight from a Windows working tree.
    // Recognising `vendor/` and not `vendor\` would score a vendored refresh as
    // hand-written work on exactly one platform, which is the kind of split
    // this repository's CI matrix exists to catch.
    assert!(is_generated_path(r"vendor\github.com\x\y.go"));
    assert!(is_generated_path(r"web\node_modules\left-pad\index.js"));
}

#[test]
fn ordinary_source_is_not_generated() {
    for path in [
        "src/main.rs",
        "backend/go_orchestrator/verify/rigor.go",
        // `vendor` as part of a name rather than a whole segment. A substring
        // test would call this generated and silently drop a real file's lines
        // out of the measurement.
        "src/vendoring/policy.rs",
        "src/vendor_client.go",
        "docs/vendors.md",
    ] {
        assert!(!is_generated_path(path), "{path} should not be generated");
    }
}

// --- the verdict ---

#[test]
fn a_diff_below_the_floor_is_not_judged() {
    let added: Vec<String> = (0..MIN_LINES_TO_JUDGE - 1)
        .map(|n| format!("let value{n} = compute({n});"))
        .collect();
    let refs: Vec<&str> = added.iter().map(String::as_str).collect();
    let report = measure(&[file_with("src/a.rs", &refs, &[])]);
    assert!(!report.judged(), "{report:#?}");
    // And `is_low` must answer false for it — not because the diff is known to
    // be fine, but because there is nothing to say. `judged` is how a caller
    // tells those apart.
    assert!(!report.is_low());
}

#[test]
fn exactly_at_the_threshold_is_not_low() {
    // The boundary is `substantive * DEN < added * NUM`, so equality is not
    // low. Pinned because a `<=` here would quietly reclassify every diff
    // sitting exactly on the line, and no realistic corpus lands there often
    // enough to notice.
    let substantive = 10;
    let added = substantive * LOW_SUBSTANCE_DENOMINATOR / LOW_SUBSTANCE_NUMERATOR;
    let mut lines: Vec<String> = (0..substantive)
        .map(|n| format!("let value{n} = compute({n});"))
        .collect();
    lines.extend((0..added - substantive).map(|_| "}".to_string()));
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let report = measure(&[file_with("src/a.rs", &refs, &[])]);

    assert_eq!(report.substantive_lines, substantive, "{report:#?}");
    assert_eq!(report.added_lines, added);
    assert!(report.judged());
    assert!(
        !report.is_low(),
        "a ratio exactly at the threshold must not be low"
    );
}

#[test]
fn one_line_under_the_threshold_is_low() {
    // The other side of the same boundary, so the pair brackets it.
    let substantive = 10;
    let added = substantive * LOW_SUBSTANCE_DENOMINATOR / LOW_SUBSTANCE_NUMERATOR + 1;
    let mut lines: Vec<String> = (0..substantive)
        .map(|n| format!("let value{n} = compute({n});"))
        .collect();
    lines.extend((0..added - substantive).map(|_| "}".to_string()));
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let report = measure(&[file_with("src/a.rs", &refs, &[])]);
    assert!(report.is_low(), "{report:#?}");
}

#[test]
fn the_classes_partition_the_added_lines() {
    // The property the whole report rests on: every added line lands in exactly
    // one bucket. A line counted twice inflates the denominator and a line
    // counted in no bucket deflates it, and either makes the ratio a number
    // that cannot be checked against the diff it describes.
    let moved_and_repeated = "return fmt.Errorf(\"bad config: %w\", err)";
    let files = vec![
        file_with(
            "src/a.go",
            &[moved_and_repeated, "}", "newThing := build()"],
            &[moved_and_repeated],
        ),
        // Same line again, in a generated file, to make the buckets overlap as
        // much as the classifier allows.
        file_with("go.sum", &[moved_and_repeated, "x"], &[]),
        file_with("src/b.go", &[moved_and_repeated, ""], &[]),
    ];
    let report = measure(&files);
    assert_eq!(
        report.substantive_lines
            + report.trivial
            + report.moved
            + report.repeated
            + report.generated,
        report.added_lines,
        "{report:#?}"
    );
    assert_eq!(report.added_lines, 7);
    // And the per-file rows must add up to the same total, or the summary and
    // the detail are describing different diffs.
    assert_eq!(
        report.files.iter().map(|f| f.added_lines).sum::<usize>(),
        report.added_lines
    );
}

#[test]
fn a_file_that_only_removes_lines_contributes_no_added_lines() {
    let report = measure(&[file_with("src/gone.rs", &[], &["everything", "was here"])]);
    assert_eq!(report.added_lines, 0, "{report:#?}");
    assert!(report.files.is_empty());
    assert!(!report.judged());
}
