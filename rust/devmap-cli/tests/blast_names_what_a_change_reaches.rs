//! `devmap blast` end to end: build an index over a real repository, then ask
//! what a change to it affects.
//!
//! The library half is tested in `dc-regress`, against a graph built by hand.
//! What this proves is the wiring nothing else can — and the wiring is where
//! the two defects this command shipped with both lived:
//!
//! 1. The store holds a node **per file** whose span is the whole file. Handed
//!    to the analysis as a symbol, it contains every line of every change, so
//!    every line places, `unattributed` is always empty, and the whole
//!    honesty mechanism is disarmed by one row per file. A hand-built graph
//!    cannot catch that, because a hand-built graph has no file nodes.
//!
//! 2. The inbound walk has to be inbound. A hand-built graph returns whatever
//!    the test put in it whichever direction the code asks for; only a real
//!    index can show that asking for callers returned callers.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

static SEQUENCE: AtomicU32 = AtomicU32::new(0);

fn temp_root(label: &str) -> PathBuf {
    let seq = SEQUENCE.fetch_add(1, Ordering::SeqCst);
    let root =
        std::env::temp_dir().join(format!("devmap-blast-{label}-{}-{seq}", std::process::id()));
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
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn devmap(repo: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_devmap"))
        .args(args)
        .arg("--root")
        .arg(repo)
        .output()
        .expect("devmap runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn blast_json(repo: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = vec!["--json", "blast"];
    full.extend_from_slice(args);
    let (ok, out, err) = devmap(repo, &full);
    assert!(ok, "blast failed: {out}{err}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("blast must emit JSON: {e}\n{out}"))
}

const BASE: &str = "\
pub fn helper(flag: bool) -> bool {
    flag
}

pub fn caller() -> bool {
    helper(true)
}

pub fn bystander() -> u32 {
    7
}
";

/// `helper`'s body changes. Nothing else does.
const CHANGED: &str = "\
pub fn helper(flag: bool) -> bool {
    !flag
}

pub fn caller() -> bool {
    helper(true)
}

pub fn bystander() -> u32 {
    7
}
";

/// A new `use` line at the very top — module-level, inside no symbol.
const IMPORT_ADDED: &str = "\
use std::fmt::Debug;

pub fn helper(flag: bool) -> bool {
    flag
}

pub fn caller() -> bool {
    helper(true)
}

pub fn bystander() -> u32 {
    7
}
";

/// Two more files that call into `src/lib.rs` and into each other.
///
/// Not decoration. A single-file repository has exactly one file node and no
/// cross-file edges, so an inbound walk never *routes through* one — and a
/// test asserting "no file node is ever reported as a symbol" passes against
/// such a fixture no matter what the code does. That is what happened: the
/// seed path was filtered, the walk's output was not, and this test passed
/// anyway until a run against DevCouncil's own index showed 68 file nodes in
/// one `impacted` list. The fixture, not the assertion, was the weak part.
const MIDDLE: &str = "\
use crate::caller;

pub fn middle() -> bool {
    caller()
}
";

const OUTER: &str = "\
use crate::middle::middle;

pub fn outer() -> bool {
    middle()
}
";

const ROOT_MOD: &str = "\
pub mod middle;
pub mod outer;
";

fn setup(label: &str, after: &str) -> (PathBuf, String) {
    let repo = temp_root(label);
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "Ada"]);
    git(&repo, &["config", "user.email", "ada@example.com"]);

    std::fs::write(repo.join("src/lib.rs"), BASE).unwrap();
    // Several files with edges between them, so the inbound walk has somewhere
    // to route through. See `MIDDLE`.
    std::fs::write(repo.join("src/middle.rs"), MIDDLE).unwrap();
    std::fs::write(repo.join("src/outer.rs"), OUTER).unwrap();
    std::fs::write(repo.join("src/mods.rs"), ROOT_MOD).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "base", "--no-gpg-sign"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);

    std::fs::write(repo.join("src/lib.rs"), after).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "change", "--no-gpg-sign"]);

    // Built at the post-image, which is what pins the basis: the spans in the
    // index and the lines in the diff then describe the same blob.
    let (ok, out, err) = devmap(&repo, &["build", "--full"]);
    assert!(ok, "build failed: {out}{err}");
    (repo, base)
}

fn names(report: &serde_json::Value, key: &str, field: &str) -> Vec<String> {
    report[key]
        .as_array()
        .unwrap_or_else(|| panic!("`{key}` is an array in {report:#}"))
        .iter()
        .filter_map(|entry| entry[field].as_str())
        .map(str::to_string)
        .collect()
}

/// The whole point of the command: a changed function names the function that
/// calls it.
#[test]
fn a_changed_function_names_its_caller_as_affected() {
    let (repo, base) = setup("caller", CHANGED);
    let report = blast_json(&repo, &["--since", &base]);

    let seeds = names(&report, "seeds", "qualified_name");
    assert!(
        seeds.iter().any(|s| s.ends_with("::helper")),
        "the changed lines are inside `helper`: {seeds:?}"
    );
    assert!(
        !seeds.iter().any(|s| s.ends_with("::bystander")),
        "a function the change did not touch is not a seed: {seeds:?}"
    );

    let impacted = names(&report, "impacted", "qualified_name");
    assert!(
        impacted.iter().any(|s| s.ends_with("::caller")),
        "`caller` calls `helper`, so changing `helper` affects it: {impacted:?}\n{report:#}"
    );
}

/// The direction. `caller` calls `helper`; changing `helper` affects `caller`
/// and **not** the reverse. An outbound walk here would answer with what
/// `helper` calls — which is the mirror-image defect `suspects` shipped with,
/// and which no hand-built graph can detect.
#[test]
fn the_walk_runs_towards_callers_not_callees() {
    let (repo, base) = setup("direction", CHANGED);
    let report = blast_json(&repo, &["--since", &base]);
    let impacted = names(&report, "impacted", "qualified_name");

    assert!(
        impacted.iter().any(|s| s.ends_with("::caller")),
        "inbound: the caller of the changed symbol: {impacted:?}"
    );
    // `helper` calls nothing, so an outbound walk from it reaches nothing at
    // all — and an empty list would be indistinguishable from a broken one.
    // Asking about `caller` instead makes the two directions produce
    // different, checkable answers.
    let report = blast_json(&repo, &["--at", "src/lib.rs:6-7"]);
    let seeds = names(&report, "seeds", "qualified_name");
    assert!(
        seeds.iter().any(|s| s.ends_with("::caller")),
        "lines 6-7 are inside `caller`: {seeds:?}\n{report:#}"
    );
    let impacted = names(&report, "impacted", "qualified_name");
    assert!(
        !impacted.iter().any(|s| s.ends_with("::helper")),
        "`helper` is what `caller` *calls*; it is downstream of the change and \
         cannot be affected by it. Finding it here means the walk ran outbound: \
         {impacted:?}\n{report:#}"
    );
}

/// The defect a hand-built graph cannot catch. The store holds a node per file
/// spanning the whole file; if it reaches the analysis as a symbol, the added
/// `use` line lands "inside" it, `unattributed` comes back empty and the
/// report claims to have placed a line it did not place.
#[test]
fn a_changed_import_is_reported_as_unattributed_against_a_real_index() {
    let (repo, base) = setup("import", IMPORT_ADDED);
    let report = blast_json(&repo, &["--since", &base]);

    let unattributed = report["unattributed"]
        .as_array()
        .expect("an unattributed array");
    assert!(
        !unattributed.is_empty(),
        "a `use` line is inside no symbol and must be reported as such. An empty \
         list here means the file's own node reached the analysis as a symbol, \
         where its whole-file span swallows every line of every change:\n{report:#}"
    );
    assert_eq!(
        report["complete"].as_bool(),
        Some(false),
        "a change whose lines could not all be placed is not a complete answer"
    );
    assert!(
        unattributed
            .iter()
            .any(|entry| entry["reason"]["kind"] == "outside_every_symbol"),
        "and the reason is named: {report:#}"
    );
}

/// No node whose id is a bare path may appear as a seed or as an impacted
/// symbol. Asserted directly as well as through its consequence above, because
/// the consequence is one inference away and this is the fact.
#[test]
fn no_file_node_is_ever_reported_as_a_symbol() {
    let (repo, base) = setup("file-nodes", CHANGED);
    let report = blast_json(&repo, &["--since", &base]);

    for (key, field) in [("seeds", "qualified_name"), ("impacted", "qualified_name")] {
        for name in names(&report, key, field) {
            assert!(
                name.contains("::"),
                "`{name}` in `{key}` has no `::`, so it names a file rather than a \
                 symbol in one; a file node's span is the whole file and matches \
                 everything:\n{report:#}"
            );
        }
    }
}

/// Files and modules are the units the question is usually asked in.
#[test]
fn the_report_rolls_up_to_files_and_modules() {
    let (repo, base) = setup("rollup", CHANGED);
    let report = blast_json(&repo, &["--since", &base]);

    let files = names(&report, "files", "path");
    assert!(
        files.iter().any(|p| p == "src/lib.rs"),
        "the changed file is affected: {files:?}"
    );
    let modules = names(&report, "modules", "path");
    assert!(
        modules.iter().any(|p| p == "src"),
        "rolled up to its directory: {modules:?}"
    );
    assert!(
        report["modules"]
            .as_array()
            .expect("array")
            .iter()
            .any(|m| m["path"] == "src" && m["changed"] == true),
        "the module holding the change is marked as changed: {report:#}"
    );
}

/// A docs-only change has no symbols. It must still report the file it
/// changed — an empty file list would say a change that demonstrably touched a
/// file touched none.
#[test]
fn a_change_with_no_symbols_still_reports_the_file_it_touched() {
    let repo = temp_root("docs-only");
    git(&repo, &["init", "--initial-branch=main"]);
    git(&repo, &["config", "user.name", "Ada"]);
    git(&repo, &["config", "user.email", "ada@example.com"]);
    std::fs::write(repo.join("src/lib.rs"), BASE).unwrap();
    std::fs::write(repo.join("NOTES.txt"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "base", "--no-gpg-sign"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);

    std::fs::write(repo.join("NOTES.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "notes", "--no-gpg-sign"]);
    let (ok, out, err) = devmap(&repo, &["build", "--full"]);
    assert!(ok, "build failed: {out}{err}");

    let report = blast_json(&repo, &["--since", &base]);
    assert!(
        report["seeds"].as_array().expect("array").is_empty(),
        "a text file declares no symbols: {report:#}"
    );
    let files = names(&report, "files", "path");
    assert!(
        files.iter().any(|p| p == "NOTES.txt"),
        "the changed file is still reported: {files:?}\n{report:#}"
    );
}

/// The human rendering, which the JSON path never exercises. `suspects` had a
/// success branch that had only ever been run by hand; this is the same trap.
#[test]
fn the_human_rendering_names_the_change_and_its_refusals() {
    let (repo, base) = setup("human", IMPORT_ADDED);
    let (ok, text, err) = devmap(&repo, &["blast", "--since", &base]);
    assert!(ok, "the text rendering must not fail: {text}{err}");
    assert!(
        text.contains("could not examine everything"),
        "refusals print before the answer: {text}"
    );
    assert!(
        text.contains("landed in no symbol"),
        "the unplaced lines are named: {text}"
    );
    assert!(
        text.contains("file(s) changed"),
        "the header states what was examined: {text}"
    );
}

// ------------------------------------------------------------- what it refuses

#[test]
fn blast_without_a_starting_point_is_refused() {
    let (repo, _) = setup("no-args", CHANGED);
    let (ok, out, err) = devmap(&repo, &["blast"]);
    assert!(!ok, "a blast with no range and no location is refused");
    assert!(
        format!("{out}{err}").contains("--since"),
        "and the refusal says what to pass: {out}{err}"
    );
}

#[test]
fn an_empty_since_is_refused_rather_than_read_as_all_of_history() {
    let (repo, _) = setup("empty-since", CHANGED);
    let (ok, out, err) = devmap(&repo, &["blast", "--since", "   "]);
    assert!(!ok, "a blank revision must not mean `everything`");
    assert!(format!("{out}{err}").contains("--since"), "{out}{err}");
}

#[test]
fn a_backwards_at_range_is_refused() {
    let (repo, _) = setup("backwards-at", CHANGED);
    let (ok, out, err) = devmap(&repo, &["blast", "--at", "src/lib.rs:9-2"]);
    assert!(!ok, "a range that ends before it starts names no lines");
    assert!(
        format!("{out}{err}").contains("backwards"),
        "and says so: {out}{err}"
    );
}

#[test]
fn a_zero_line_in_an_at_range_is_refused() {
    let (repo, _) = setup("zero-line", CHANGED);
    let (ok, out, err) = devmap(&repo, &["blast", "--at", "src/lib.rs:0-4"]);
    assert!(!ok, "line numbers are one-based");
    assert!(format!("{out}{err}").contains("one-based"), "{out}{err}");
}

#[test]
fn since_and_at_are_alternatives_not_a_pair() {
    let (repo, base) = setup("both", CHANGED);
    let (ok, _, _) = devmap(
        &repo,
        &["blast", "--since", &base, "--at", "src/lib.rs:1-2"],
    );
    assert!(!ok, "passing both asks two different questions at once");
}

/// A refusal that reads as an answer is the failure mode this whole command is
/// built around. A rev that does not exist must not come back as "affects
/// nothing, complete".
#[test]
fn a_nonexistent_since_does_not_answer_that_nothing_is_affected() {
    let (repo, _) = setup("bad-since", CHANGED);
    let report = blast_json(
        &repo,
        &["--since", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"],
    );
    assert_eq!(
        report["complete"].as_bool(),
        Some(false),
        "an unreadable change is not a complete answer: {report:#}"
    );
    assert!(
        !report["unavailable"].as_array().expect("array").is_empty(),
        "and the ledger says why: {report:#}"
    );
}

/// The same distinction in the *rendered* output, which is what a human
/// actually reads. An empty change set after a refusal must not be summarised
/// as "changed no files" — a reader who takes that sentence and stops has been
/// told the opposite of the truth.
#[test]
fn an_unreadable_change_is_not_rendered_as_a_change_that_touched_nothing() {
    let (repo, _) = setup("unreadable-render", CHANGED);
    let (ok, text, err) = devmap(
        &repo,
        &[
            "blast",
            "--since",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        ],
    );
    assert!(ok, "the text rendering must not fail: {text}{err}");
    assert!(
        !text.contains("changed no files"),
        "an unreadable change must not be summarised as an empty one: {text}"
    );
    assert!(
        text.contains("could not be determined"),
        "it says the change could not be read: {text}"
    );
    assert!(
        text.contains("not a finding"),
        "and says so in as many words, because the sentence above it is the one \
         a hurried reader keeps: {text}"
    );
}
