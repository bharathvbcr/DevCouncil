//! The forward direction against a real repository: a change goes in, the
//! symbols/files/modules/tests it affects come out.
//!
//! The library's unit tests drive the parts. What this drives is the join —
//! that a line number out of `git diff` and a byte offset out of the graph
//! describe the same place in the same bytes, and that everything which cannot
//! be placed says so instead of vanishing.
//!
//! Every test here begins by asking what the *pre-change* behaviour is, and
//! most of them assert a property that a plausible-looking implementation gets
//! wrong: that a changed import is reported rather than dropped, that a pure
//! deletion seeds the function it was deleted from, that spans from another
//! revision are refused rather than clamped.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use dc_regress::blast::{blast_change_with_program, UnattributedReason};
use dc_regress::change::{ChangeSet, ChangeStatus, ChangedRange, FileChange};
use dc_regress::{AffectedTestFile, BlobIdentity, CodeGraph, ConeEntry, GraphSymbol};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// A unique root per test.
///
/// Both the pid and a process-wide counter, because `cargo test` runs these on
/// several threads of one process and a pid-only name collides between them.
fn temp_root(label: &str) -> PathBuf {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "dc-regress-blast-{label}-{}-{seq}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("temp root");
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
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn init_repo(label: &str) -> PathBuf {
    let root = temp_root(label);
    git(&root, &["init", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Ada"]);
    git(&root, &["config", "user.email", "ada@example.com"]);
    root
}

fn write(repo: &Path, path: &str, body: &str) {
    let full = repo.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(full, body).expect("write");
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", message, "--no-gpg-sign"]);
    git(repo, &["rev-parse", "HEAD"])
}

fn blob_of(repo: &Path, rev: &str, path: &str) -> BlobIdentity {
    BlobIdentity::Blob(git(repo, &["rev-parse", &format!("{rev}:{path}")]))
}

/// A graph built by hand, so the analysis can be driven over shapes without
/// standing up an index.
///
/// Deliberately does **not** hold a node for the file itself. The real adapter
/// filters those out and the trait requires it; a fake that included one would
/// make every test here pass for the wrong reason, because a whole-file span
/// contains every line of every change.
#[derive(Default)]
struct FakeGraph {
    symbols: Vec<GraphSymbol>,
    basis: Option<BlobIdentity>,
    inbound: Vec<ConeEntry>,
    inbound_incomplete: bool,
    tests: Vec<AffectedTestFile>,
    /// Recorded so a test can assert which seeds the walk was actually given.
    seen_seeds: std::cell::RefCell<Vec<String>>,
}

impl CodeGraph for FakeGraph {
    fn symbols_in(&self, file: &str) -> (Vec<GraphSymbol>, BlobIdentity) {
        (
            self.symbols
                .iter()
                .filter(|s| s.file_path == file)
                .cloned()
                .collect(),
            self.basis.clone().unwrap_or(BlobIdentity::Unknown),
        )
    }
    fn cone(&self, _symbol: &str, _depth: u32) -> (Vec<ConeEntry>, bool) {
        (Vec::new(), false)
    }
    fn impacted(&self, seeds: &[String], _depth: u32) -> (Vec<ConeEntry>, bool) {
        self.seen_seeds.borrow_mut().extend(seeds.iter().cloned());
        (self.inbound.clone(), self.inbound_incomplete)
    }
    fn affected_tests(&self, _seeds: &[String], _depth: u32) -> (Vec<AffectedTestFile>, bool) {
        (self.tests.clone(), false)
    }
    fn resolve_symptom(&self, _symptom: &str) -> Vec<String> {
        Vec::new()
    }
}

fn change_of(path: &str, ranges: &[(u32, u32)]) -> ChangeSet {
    ChangeSet {
        files: vec![FileChange {
            path: path.to_string(),
            renamed_from: None,
            status: ChangeStatus::Modified,
            ranges: ranges
                .iter()
                .map(|(start, end)| ChangedRange {
                    start_line: *start,
                    end_line: *end,
                    deletion_only: false,
                })
                .collect(),
        }],
        capped: false,
    }
}

/// Two functions with a `use` line above them, so there are lines that belong
/// to a symbol and lines that belong to no symbol.
const SOURCE: &str = "use std::fmt;\n\
                      \n\
                      pub fn helper() -> u32 {\n\
                      \x20   1\n\
                      }\n\
                      \n\
                      pub fn other() -> u32 {\n\
                      \x20   2\n\
                      }\n";

fn spans_for(source: &str) -> Vec<GraphSymbol> {
    let helper_start = source.find("pub fn helper").expect("helper");
    let helper_end = source[helper_start..].find("}\n").expect("close") + helper_start + 1;
    let other_start = source.find("pub fn other").expect("other");
    vec![
        GraphSymbol {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            span_start: helper_start,
            span_end: helper_end,
            body_exact: None,
        },
        GraphSymbol {
            qualified_name: "src/lib.rs::other".into(),
            file_path: "src/lib.rs".into(),
            span_start: other_start,
            span_end: source.len(),
            body_exact: None,
        },
    ]
}

fn fixture(label: &str) -> (PathBuf, String, FakeGraph) {
    let repo = init_repo(label);
    write(&repo, "src/lib.rs", SOURCE);
    let head = commit(&repo, "base");
    let graph = FakeGraph {
        symbols: spans_for(SOURCE),
        basis: Some(blob_of(&repo, &head, "src/lib.rs")),
        ..Default::default()
    };
    (repo, head, graph)
}

fn run(repo: &Path, graph: &FakeGraph, change: &ChangeSet, head: &str) -> dc_regress::BlastReport {
    blast_change_with_program(
        std::ffi::OsStr::new("git"),
        repo,
        graph,
        change,
        "test",
        head,
        3,
    )
}

#[test]
fn a_line_inside_a_function_seeds_that_function() {
    let (repo, head, graph) = fixture("inside");
    // Line 4 is `    1` — inside `helper`.
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(4, 4)]), &head);
    assert_eq!(report.seeds.len(), 1, "{report:#?}");
    assert_eq!(report.seeds[0].qualified_name, "src/lib.rs::helper");
    assert_eq!(report.seeds[0].changed_lines, 1);
    assert!(
        report.unattributed.is_empty(),
        "a line inside a symbol is placed, not reported as unplaceable"
    );
    assert!(report.complete);
}

/// The property the whole forward direction is built around. A changed import
/// belongs to no symbol, so a seed-based walk has nothing to start from — and
/// the one thing the report must not do is present that as "affects nothing".
#[test]
fn a_changed_import_is_reported_rather_than_dropped() {
    let (repo, head, graph) = fixture("import");
    // Line 1 is `use std::fmt;` — module-level.
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(1, 1)]), &head);

    assert!(report.seeds.is_empty(), "no symbol spans line 1");
    assert_eq!(report.unattributed.len(), 1, "{report:#?}");
    assert_eq!(report.unattributed[0].start_line, 1);
    assert!(matches!(
        report.unattributed[0].reason,
        UnattributedReason::OutsideEverySymbol { .. }
    ));
    assert!(
        !report.complete,
        "a change whose lines could not be placed must never read as a complete \
         answer — that is how a changed module constant comes to report an empty \
         blast radius"
    );
    assert!(
        report
            .unavailable
            .iter()
            .any(|u| matches!(u, dc_regress::Unavailable::ChangeUnattributed { .. })),
        "the ledger names the reason: {:#?}",
        report.unavailable
    );
}

/// Diagnosis, not attribution. The nearest symbol is named so a reader can see
/// at a glance that the range is a doc comment — but it must not become a
/// seed, because a top-level constant sits in exactly the same position.
#[test]
fn an_unplaced_range_names_its_nearest_symbol_without_seeding_it() {
    let (repo, head, graph) = fixture("nearest");
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(1, 1)]), &head);
    let UnattributedReason::OutsideEverySymbol {
        ref nearest_symbol, ..
    } = report.unattributed[0].reason
    else {
        panic!("expected an unplaced range: {report:#?}");
    };
    assert_eq!(nearest_symbol.as_deref(), Some("src/lib.rs::helper"));
    assert!(
        report.seeds.is_empty(),
        "naming the nearest symbol must not promote it to a seed"
    );
    assert!(
        graph.seen_seeds.borrow().is_empty(),
        "and the walk must never be handed it"
    );
}

/// A hunk that removed lines and added none has a post-image count of zero.
/// Read literally it touches nothing and the change disappears; it must seed
/// the function the code was removed from.
#[test]
fn a_pure_deletion_seeds_the_function_it_was_removed_from() {
    let (repo, head, graph) = fixture("deletion");
    // `@@ -4,2 +3,0 @@` — lines removed from after post-image line 3, which is
    // `pub fn helper() {`.
    let change = ChangeSet {
        files: vec![FileChange {
            path: "src/lib.rs".into(),
            renamed_from: None,
            status: ChangeStatus::Modified,
            ranges: vec![ChangedRange {
                start_line: 3,
                end_line: 4,
                deletion_only: true,
            }],
        }],
        capped: false,
    };
    let report = run(&repo, &graph, &change, &head);
    assert_eq!(report.seeds.len(), 1, "{report:#?}");
    assert_eq!(report.seeds[0].qualified_name, "src/lib.rs::helper");
    assert!(
        report.seeds[0].deletion_only,
        "the report must say the evidence is what is no longer there, or a reader \
         looking at the post-image will find nothing and conclude the tool is wrong"
    );
}

/// The defect the basis check exists for. Spans taken against one revision and
/// content from another produce plausible, ordered, wrong line numbers — and
/// every symbol placed through them is wrong with no signal.
#[test]
fn spans_from_another_revision_refuse_rather_than_place_lines_wrongly() {
    let (repo, head, _) = fixture("basis");
    let graph = FakeGraph {
        symbols: spans_for(SOURCE),
        basis: Some(BlobIdentity::Blob(
            "0000000000000000000000000000000000000000".into(),
        )),
        ..Default::default()
    };
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(4, 4)]), &head);
    assert!(
        report.seeds.is_empty(),
        "nothing may be placed: {report:#?}"
    );
    assert!(matches!(
        report.unattributed[0].reason,
        UnattributedReason::BasisMismatch { .. }
    ));
    assert!(!report.complete);
}

#[test]
fn an_unrecorded_basis_is_refused_like_a_mismatched_one() {
    let (repo, head, _) = fixture("unknown-basis");
    let graph = FakeGraph {
        symbols: spans_for(SOURCE),
        basis: Some(BlobIdentity::Unknown),
        ..Default::default()
    };
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(4, 4)]), &head);
    assert!(matches!(
        report.unattributed[0].reason,
        UnattributedReason::BasisMismatch { .. }
    ));
}

/// A file the graph knows nothing about — a language the index does not parse,
/// a docs file, a lockfile. It contributes no seeds, and saying so is the
/// difference between "nothing depends on this" and "I did not look".
#[test]
fn a_file_with_no_indexed_symbols_is_named_not_omitted() {
    let (repo, head, graph) = fixture("unindexed");
    let report = run(&repo, &graph, &change_of("README.md", &[(1, 10)]), &head);
    assert!(report.seeds.is_empty());
    assert_eq!(
        report.unattributed[0].reason,
        UnattributedReason::FileNotIndexed
    );
    assert!(!report.complete);
    assert!(
        report.files.iter().any(|f| f.path == "README.md"),
        "the changed file is still in the affected-file list: {:#?}",
        report.files
    );
}

/// Multi-byte content is where line arithmetic done on bytes-as-characters
/// breaks. The span must still land on the line the brace is actually on.
#[test]
fn multibyte_content_does_not_shift_which_symbol_a_line_lands_in() {
    let repo = init_repo("multibyte");
    let source = "use std::fmt;\n\
                  \n\
                  pub fn helper() -> u32 {\n\
                  \x20   // 🎉🎉🎉 three four-byte characters\n\
                  \x20   1\n\
                  }\n";
    write(&repo, "src/lib.rs", source);
    let head = commit(&repo, "base");
    let helper_start = source.find("pub fn helper").expect("helper");
    let graph = FakeGraph {
        symbols: vec![GraphSymbol {
            qualified_name: "src/lib.rs::helper".into(),
            file_path: "src/lib.rs".into(),
            span_start: helper_start,
            span_end: source.rfind('}').expect("close") + 1,
            body_exact: None,
        }],
        basis: Some(blob_of(&repo, &head, "src/lib.rs")),
        ..Default::default()
    };
    // Line 5 is `    1`, the last line inside the function.
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(5, 5)]), &head);
    assert_eq!(report.seeds.len(), 1, "{report:#?}");
    assert_eq!(report.seeds[0].qualified_name, "src/lib.rs::helper");
    assert_eq!(
        (report.seeds[0].start_line, report.seeds[0].end_line),
        (3, 6),
        "the emoji line must not push the closing brace onto a different line"
    );
}

#[test]
fn a_range_spanning_two_symbols_seeds_both() {
    let (repo, head, graph) = fixture("two");
    // Lines 4 through 8 cross out of `helper`, over the blank, into `other`.
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(4, 8)]), &head);
    let mut names: Vec<&str> = report
        .seeds
        .iter()
        .map(|s| s.qualified_name.as_str())
        .collect();
    names.sort();
    assert_eq!(names, vec!["src/lib.rs::helper", "src/lib.rs::other"]);
}

/// The lines between two symbols belong to neither. A range that covers a
/// symbol *and* a gap places what it can and reports the rest — it does not
/// round the gap into the nearest symbol.
#[test]
fn only_the_lines_actually_inside_a_symbol_are_counted_to_it() {
    let (repo, head, graph) = fixture("partial");
    // Lines 1 to 4: the import, a blank, and two lines of `helper`.
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(1, 4)]), &head);
    let helper = report
        .seeds
        .iter()
        .find(|s| s.qualified_name == "src/lib.rs::helper")
        .expect("helper is seeded");
    assert_eq!(
        helper.changed_lines, 2,
        "lines 3 and 4 are inside helper; lines 1 and 2 are not and must not be \
         counted to it: {report:#?}"
    );
}

/// A deleted file has no post-image, so nothing in it can be placed — but the
/// change still touched it, and a report that omits it is answering a narrower
/// question than the one asked.
#[test]
fn a_deleted_file_contributes_no_seeds_and_says_why() {
    let (repo, head, graph) = fixture("deleted");
    let change = ChangeSet {
        files: vec![FileChange {
            path: "src/gone.rs".into(),
            renamed_from: None,
            status: ChangeStatus::Deleted,
            ranges: Vec::new(),
        }],
        capped: false,
    };
    let report = run(&repo, &graph, &change, &head);
    assert!(report.seeds.is_empty());
    assert!(matches!(
        report.unattributed[0].reason,
        UnattributedReason::NoPostImageLines { .. }
    ));
    assert!(
        report.files.iter().any(|f| f.path == "src/gone.rs"),
        "the deleted file is still reported as changed"
    );
}

#[test]
fn a_binary_file_contributes_no_seeds_and_says_why() {
    let (repo, head, graph) = fixture("binary");
    let change = ChangeSet {
        files: vec![FileChange {
            path: "logo.png".into(),
            renamed_from: None,
            status: ChangeStatus::Binary,
            ranges: Vec::new(),
        }],
        capped: false,
    };
    let report = run(&repo, &graph, &change, &head);
    let UnattributedReason::NoPostImageLines { ref status } = report.unattributed[0].reason else {
        panic!("expected a no-lines reason: {report:#?}");
    };
    assert_eq!(status, "binary");
}

/// A mode change, or a rename with no edits. Nothing inside the content
/// changed, so there is nothing to place and — crucially — nothing missing.
/// The report stays complete.
#[test]
fn a_file_changed_without_any_lines_leaves_the_report_complete() {
    let (repo, head, graph) = fixture("mode-only");
    let change = ChangeSet {
        files: vec![FileChange {
            path: "src/lib.rs".into(),
            renamed_from: Some("src/old.rs".into()),
            status: ChangeStatus::Modified,
            ranges: Vec::new(),
        }],
        capped: false,
    };
    let report = run(&repo, &graph, &change, &head);
    assert!(report.seeds.is_empty());
    assert!(
        report.unattributed.is_empty(),
        "no lines changed, so no line is unaccounted for: {report:#?}"
    );
    assert!(
        report.complete,
        "a rename with no edits is a complete answer, not a degraded one"
    );
}

/// The diff and the blob disagreeing means one of them is not describing the
/// revision the analysis was told to run against. That is a different failure
/// from "these lines are module-level" and must not share its name.
#[test]
fn a_range_past_the_end_of_the_file_is_named_as_a_disagreement() {
    let (repo, head, graph) = fixture("past-end");
    let report = run(
        &repo,
        &graph,
        &change_of("src/lib.rs", &[(9_000, 9_001)]),
        &head,
    );
    assert!(matches!(
        report.unattributed[0].reason,
        UnattributedReason::BeyondEndOfFile { .. }
    ));
    assert!(!report.complete);
}

#[test]
fn the_inbound_walk_is_handed_the_seeds_and_its_result_is_reported() {
    let (repo, head, _) = fixture("inbound");
    let graph = FakeGraph {
        symbols: spans_for(SOURCE),
        basis: Some(blob_of(&repo, &head, "src/lib.rs")),
        inbound: vec![
            ConeEntry {
                qualified_name: "src/caller.rs::calls_helper".into(),
                file_path: "src/caller.rs".into(),
                distance: 1,
            },
            ConeEntry {
                qualified_name: "src/far.rs::indirect".into(),
                file_path: "src/far.rs".into(),
                distance: 2,
            },
        ],
        tests: vec![AffectedTestFile {
            path: "tests/it.rs".into(),
            distance: 1,
        }],
        ..Default::default()
    };
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(4, 4)]), &head);

    assert_eq!(
        *graph.seen_seeds.borrow(),
        vec!["src/lib.rs::helper".to_string()],
        "the walk starts from the symbol the change landed in"
    );
    assert_eq!(report.impacted.len(), 2);
    assert_eq!(report.impacted[0].distance, 1, "nearest first");
    assert_eq!(report.tests.len(), 1);

    // Files roll up both the changed file and the reached ones.
    let paths: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"src/lib.rs"), "{paths:?}");
    assert!(paths.contains(&"src/caller.rs"), "{paths:?}");
    // Modules roll up the directories.
    assert!(
        report.modules.iter().any(|m| m.path == "src" && m.changed),
        "{:#?}",
        report.modules
    );
}

/// A truncated walk is a lower bound. If that does not reach the report, a
/// short list of dependents reads as a complete one.
#[test]
fn a_truncated_inbound_walk_makes_the_report_incomplete() {
    let (repo, head, _) = fixture("truncated");
    let graph = FakeGraph {
        symbols: spans_for(SOURCE),
        basis: Some(blob_of(&repo, &head, "src/lib.rs")),
        inbound_incomplete: true,
        ..Default::default()
    };
    let report = run(&repo, &graph, &change_of("src/lib.rs", &[(4, 4)]), &head);
    assert!(!report.complete);
    assert!(report
        .unavailable
        .iter()
        .any(|u| matches!(u, dc_regress::Unavailable::ImpactIncomplete { .. })));
}

/// Two runs over identical input must produce an identical report, or the
/// report cannot be used as evidence and cannot be diffed by a test.
#[test]
fn the_report_is_deterministic_across_runs() {
    let (repo, head, _) = fixture("deterministic");
    let graph = FakeGraph {
        symbols: spans_for(SOURCE),
        basis: Some(blob_of(&repo, &head, "src/lib.rs")),
        inbound: (0..40)
            .map(|index| ConeEntry {
                qualified_name: format!("src/c{index}.rs::f"),
                file_path: format!("src/c{index}.rs"),
                // Equal distances, so only the tie-break decides the order.
                distance: 1,
            })
            .collect(),
        ..Default::default()
    };
    let change = change_of("src/lib.rs", &[(4, 4)]);
    let first = run(&repo, &graph, &change, &head);
    let second = run(&repo, &graph, &change, &head);
    assert_eq!(
        serde_json::to_string(&first).expect("serializes"),
        serde_json::to_string(&second).expect("serializes"),
        "a report that cannot reproduce itself is not evidence"
    );
}
