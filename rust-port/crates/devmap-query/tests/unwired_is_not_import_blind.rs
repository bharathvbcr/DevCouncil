//! W0.3 — no file is called unwired in a language where no import was looked for.
//!
//! `unwired_candidates` asks one question: does this file have an inbound
//! `Imports` edge from a non-test file? There are five `imports.push` sites in
//! the whole extractor — Python, JS/TS/TSX, Rust `use`, Go `import_spec`, and
//! the embedded-script merge — so for 24 of 35 languages the answer is
//! structurally always "no". `langcalls/*` and `langdecl/*` do not even take an
//! `imports` parameter, and `#include` has no handler anywhere.
//!
//! The consequence, measured against the pre-gate kernel by
//! `the_bug_this_gate_closes` below: in a Java or C++ repository every
//! non-entry-root, non-exempt file was an unwired candidate. Those files parse
//! `Clean`, so the `excluded_coverage_loss` escape never fired for them, and
//! the list caps at 200 — so an agent was handed up to 200 confidently-wrong
//! filenames, each one an invitation to delete a file the kernel had simply
//! never looked for a reference to.
//!
//! Both directions are asserted, and the "before" assertion is kept rather than
//! deleted: it is what proves the gate is load-bearing, and it becomes the
//! regression guard once import extraction for these languages lands — at which
//! point it fails, and the failure is the news.

use devmap_extract::extract_file;
use devmap_extract::languages::{capabilities_for_language, Capability};
use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_query::code_graph::generate_code_graph_json;
use devmap_query::model::FreshnessInfo;
use devmap_resolve::Resolver;
use serde_json::Value;

/// A two-file Java project: `Main` is an entry root, `Helper` is used by it.
///
/// `Helper.java` has no inbound `Imports` edge and never could have one — which
/// is the whole point. Java in the same package needs no import statement at
/// all, so even a complete `import` extractor would not produce one here.
fn java_project() -> Vec<Extraction> {
    vec![
        extract_file(
            "Main.java",
            "package app;\n\npublic class Main {\n    public static void main(String[] args) {\n\
             \x20       new Helper().run();\n    }\n}\n",
        ),
        extract_file(
            "Helper.java",
            "package app;\n\npublic class Helper {\n    public void run() {}\n}\n",
        ),
    ]
}

/// A C++ project whose two translation units are joined only by `#include`.
fn cpp_project() -> Vec<Extraction> {
    vec![
        extract_file(
            "main.cpp",
            "#include \"helper.h\"\n\nint main() {\n    return help();\n}\n",
        ),
        extract_file("helper.h", "#pragma once\nint help();\n"),
        extract_file(
            "helper.cpp",
            "#include \"helper.h\"\n\nint help() { return 1; }\n",
        ),
    ]
}

/// A Python project of the same shape, as the control.
fn python_project() -> Vec<Extraction> {
    vec![
        extract_file("main.py", "from helper import run\n\n\nrun()\n"),
        extract_file("helper.py", "def run():\n    return 1\n"),
        extract_file("orphan.py", "def nobody_imports_this():\n    return 1\n"),
    ]
}

fn graph(extractions: &[Extraction]) -> Value {
    let mut resolver = Resolver::new();
    resolver.index_extractions(extractions);
    let resolution = resolver.resolve_all(extractions);
    let analysis = devmap_analyze::analyze(extractions, &resolution);
    let json = generate_code_graph_json(
        extractions,
        &analysis,
        &resolution.edges,
        &FreshnessInfo {
            head_sha: "abc123".to_string(),
            generation_id: 7,
            pending_count: 0,
            stamped: Default::default(),
        },
        None,
    )
    .expect("graph renders");
    serde_json::from_str(&json).expect("graph is JSON")
}

fn unwired(graph: &Value) -> Vec<String> {
    graph["unwired_candidates"]
        .as_array()
        .expect("unwired_candidates array")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// The premise: these files parse cleanly, so no existing escape covers them.
///
/// Without this the two tests below could both pass against a kernel that
/// excluded Java for some entirely different reason — a parse failure, say —
/// and the gate would be untested while looking tested.
#[test]
fn the_import_blind_fixtures_parse_clean() {
    for extraction in java_project().iter().chain(cpp_project().iter()) {
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{} must parse cleanly: {:?}",
            extraction.file_path,
            extraction.parse_outcome
        );
        assert!(!extraction.is_parse_failure());
        assert!(
            extraction.imports.is_empty(),
            "{} is expected to yield no imports",
            extraction.file_path
        );
    }
}

/// The bug, pinned: without the gate, every one of these files is a candidate.
///
/// Asserted through the capability registry rather than by re-running the old
/// code, because the old code is gone. What it establishes is the premise the
/// gate rests on — that the inbound-import question is unanswerable for these
/// languages — so a future import extractor makes this test fail and forces the
/// fixtures below to be revisited rather than silently becoming vacuous.
#[test]
fn the_bug_this_gate_closes() {
    for language in [
        "java", "cpp", "c", "csharp", "ruby", "swift", "kotlin", "php",
    ] {
        assert!(
            !capabilities_for_language(language).contains(Capability::Imports),
            "{language} now extracts imports — the gate no longer needs to \
             exclude it, and `unwired_candidates` must be re-verified against \
             real import edges for this language"
        );
    }

    // And the shape that made it costly: the fixture files are not exempt by
    // any other rule, so the gate is the only thing keeping them off the list.
    // Read through the published counter rather than the private entry-root
    // predicate — if some other exemption were doing the work, nothing would
    // have been charged here.
    let excluded = graph(&java_project())["meta"]["devmap_rust"]["unwired_excluded_import_blind"]
        .as_u64()
        .expect("published");
    assert_eq!(
        excluded, 2,
        "both Java files must reach the import-blind gate; if fewer do, some \
         other exemption is masking the bug and this test proves nothing"
    );
}

/// After the gate: zero candidates, and a non-zero exclusion count saying why.
#[test]
fn an_import_blind_project_reports_no_unwired_candidates() {
    for (name, extractions) in [("java", java_project()), ("cpp", cpp_project())] {
        let graph = graph(&extractions);
        assert!(
            unwired(&graph).is_empty(),
            "{name}: no file may be called unwired in a language where no \
             import was ever looked for: {:?}",
            unwired(&graph)
        );

        let excluded = graph["meta"]["devmap_rust"]["unwired_excluded_import_blind"]
            .as_u64()
            .expect("unwired_excluded_import_blind must be published");
        assert!(
            excluded > 0,
            "{name}: a filtered list under a bare total is how \"we did not \
             look\" comes to read as \"we looked and found nothing\""
        );
    }
}

/// The OFF direction. A language that *does* extract imports keeps reporting.
///
/// Without this the gate could be excluding everything — an empty
/// `unwired_candidates` for every input passes the test above perfectly, and
/// would mean the feature had been deleted rather than corrected.
#[test]
fn an_import_capable_project_still_reports_its_unwired_files() {
    let extractions = python_project();
    let graph = graph(&extractions);
    let candidates = unwired(&graph);

    assert!(
        candidates.contains(&"orphan.py".to_string()),
        "a Python file nothing imports is still a candidate: {candidates:?}"
    );
    assert!(
        !candidates.contains(&"helper.py".to_string()),
        "helper.py is imported by main.py: {candidates:?}"
    );
    assert_eq!(
        graph["meta"]["devmap_rust"]["unwired_excluded_import_blind"]
            .as_u64()
            .unwrap(),
        0,
        "nothing was excluded for import-blindness in a pure-Python corpus"
    );
}

/// A mixed corpus excludes only the blind half.
///
/// The failure this guards against is a gate keyed on the corpus rather than on
/// the file: one Java file must not suppress the Python findings beside it.
#[test]
fn a_mixed_corpus_gates_per_file_not_per_corpus() {
    let mut extractions = python_project();
    extractions.extend(java_project());
    let graph = graph(&extractions);
    let candidates = unwired(&graph);

    assert!(
        candidates.contains(&"orphan.py".to_string()),
        "the Python finding survives a Java file in the same tree: {candidates:?}"
    );
    assert!(
        !candidates.iter().any(|p| p.ends_with(".java")),
        "no Java file may appear: {candidates:?}"
    );
    assert!(
        graph["meta"]["devmap_rust"]["unwired_excluded_import_blind"]
            .as_u64()
            .unwrap()
            > 0
    );
}

/// The two exclusion reasons stay distinguishable.
///
/// One is a file this run could not read, which a re-index might fix; the other
/// is a language this build cannot read imports for, which no re-run will. Summed
/// into a single counter they would be one number that means neither thing.
#[test]
fn the_two_exclusion_reasons_are_counted_apart() {
    let mut extractions = java_project();
    // `.ps1` reaches the regex fallback: a real coverage loss, not a language
    // blind spot.
    extractions.push(extract_file("build.ps1", "function Render {\n  Help\n}\n"));

    let graph = graph(&extractions);
    let meta = &graph["meta"]["devmap_rust"];
    assert_eq!(
        meta["unwired_excluded_coverage_loss"].as_u64().unwrap(),
        1,
        "the PowerShell file is a coverage loss"
    );
    assert!(
        meta["unwired_excluded_import_blind"].as_u64().unwrap() >= 2,
        "the Java files are import-blind, not coverage losses"
    );
}
