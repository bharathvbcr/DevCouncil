//! W0.3 — no file is called unwired without evidence that nothing needs it.
//!
//! `unwired_candidates` names files an agent may delete. Two things have made
//! it wrong, and this file pins both.
//!
//! **Move 1 — the gate.** The scan originally asked "does this file have an
//! inbound `Imports` edge", and there were five `imports.push` sites in the
//! whole extractor, with no `#include` handler anywhere. For 24 of 35 languages
//! the answer was structurally always "no", so in a Java or C++ repository
//! *every* non-entry-root file was a candidate — they parse `Clean`, so the
//! coverage-loss escape never fired, and the list caps at 200. Up to 200
//! confidently-wrong filenames, each an invitation to delete a file the kernel
//! had never looked for a reference to. The gate excluded them and counted the
//! exclusion.
//!
//! **Move 2 — the extractors, and what they exposed.** Import extraction landed
//! for nineteen grammar keys, and the "before" assertion kept here fired
//! exactly as its own comment predicted it would. But it did not simply invert:
//! the moment Java stopped being excluded, `Helper.java` was reported —
//! because **Java files in one package import each other not at all**, so its
//! caller sitting one file away produces a resolved `References` edge and no
//! import. The same wrong answer, with a new reason.
//!
//! So the question moved with the evidence: *does anything depend on this
//! file*. Every confident cross-file dependency edge answers it, and an import
//! is one kind. The gate stays for the four languages whose import syntax names
//! no file at all — C#, VB.NET, Swift, COBOL — where it is a documented
//! decision rather than a gap.
//!
//! Both directions are asserted, and the "before" assertions are kept rather
//! than deleted: they are what proves each gate is load-bearing.

use devmap_extract::extract_file;
use devmap_extract::languages::{capabilities_for_language, Capability};
use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_query::code_graph::generate_code_graph_json;
use devmap_query::model::FreshnessInfo;
use devmap_resolve::Resolver;
use serde_json::Value;

/// A two-file Java project: `Main` is an entry root, `Helper` is used by it.
///
/// `Helper.java` has no inbound `Imports` edge and never could have one — Java
/// in the same package needs no import statement at all, so even the complete
/// `import` extractor this repository now has produces none here. It is wired
/// all the same, and the scan has to see that.
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

/// A C# project of the same shape, in a language whose imports name no file.
///
/// `using System;` names a namespace that spans files, so there is nothing for
/// an extractor to resolve and W0.3 move 2 declined C# on purpose. These two
/// files are what the gate is still for.
fn csharp_project() -> Vec<Extraction> {
    vec![
        extract_file(
            "Program.cs",
            "using System;\n\npublic class Program {\n    public static void Main() {}\n}\n",
        ),
        extract_file(
            "Orphan.cs",
            "using System;\n\npublic class Orphan {\n    public void Idle() {}\n}\n",
        ),
    ]
}

/// A Swift project of the same shape, for the second declined language.
fn swift_project() -> Vec<Extraction> {
    vec![
        extract_file(
            "Sources/App/main.swift",
            "import Foundation\n\nfunc run() {}\n",
        ),
        extract_file(
            "Sources/App/Orphan.swift",
            "import Foundation\n\nfunc idle() -> Int { return 1 }\n",
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
/// Without this the tests below could pass against a kernel that excluded C#
/// for some entirely different reason — a parse failure, say — and the gate
/// would be untested while looking tested.
#[test]
fn the_import_blind_fixtures_parse_clean() {
    for extraction in csharp_project().iter().chain(swift_project().iter()) {
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{} must parse cleanly: {:?}",
            extraction.file_path,
            extraction.parse_outcome
        );
        assert!(!extraction.is_parse_failure());
        assert!(
            extraction.imports.is_empty(),
            "{} is expected to yield no imports: `using` and `import` name a \
             namespace and a module, neither of which is a file",
            extraction.file_path
        );
    }
}

/// The bug, pinned: without the gate, every one of these files is a candidate.
///
/// Asserted through the capability registry rather than by re-running the old
/// code, because the old code is gone. What it establishes is the premise the
/// gate rests on — that the inbound-import question is unanswerable for these
/// languages — so an import extractor for one of them makes this test fail and
/// forces the fixtures below to be revisited rather than silently becoming
/// vacuous.
///
/// It did fail, on purpose, when W0.3 move 2 landed: the list read `java`,
/// `cpp`, `c`, `csharp`, `ruby`, `swift`, `kotlin`, `php`, and six of those
/// eight now extract imports. The two that remain are the two that will not,
/// and their reasons are in `language_capabilities.rs`.
#[test]
fn the_bug_this_gate_closes() {
    for language in ["csharp", "vb", "swift", "cobol"] {
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
    let excluded = graph(&csharp_project())["meta"]["devmap_rust"]["unwired_excluded_import_blind"]
        .as_u64()
        .expect("published");
    assert_eq!(
        excluded, 2,
        "both C# files must reach the import-blind gate — `Program.cs` is not \
         recognised as an entry root, so no earlier exemption removes it. If \
         fewer arrive, something else is masking the bug and this test proves \
         nothing"
    );
}

/// After the gate: zero candidates, and a non-zero exclusion count saying why.
#[test]
fn an_import_blind_project_reports_no_unwired_candidates() {
    for (name, extractions) in [("csharp", csharp_project()), ("swift", swift_project())] {
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
/// the file: one C# file must not suppress the Python findings beside it.
#[test]
fn a_mixed_corpus_gates_per_file_not_per_corpus() {
    let mut extractions = python_project();
    extractions.extend(csharp_project());
    let graph = graph(&extractions);
    let candidates = unwired(&graph);

    assert!(
        candidates.contains(&"orphan.py".to_string()),
        "the Python finding survives a C# file in the same tree: {candidates:?}"
    );
    assert!(
        !candidates.iter().any(|p| p.ends_with(".cs")),
        "no C# file may appear: {candidates:?}"
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
    let mut extractions = csharp_project();
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
        meta["unwired_excluded_import_blind"].as_u64().unwrap() >= 1,
        "the C# file is import-blind, not a coverage loss"
    );
}

// ---------------------------------------------------------------------------
// Move 2: the languages that left the gate, answered for rather than excluded.
// ---------------------------------------------------------------------------

/// The premise for the tests below: these fixtures really do extract imports
/// now, so their answers are answers rather than another exclusion.
#[test]
fn the_formerly_blind_fixtures_extract_imports() {
    for language in ["java", "cpp", "c", "ruby", "kotlin", "php"] {
        assert!(
            capabilities_for_language(language).contains(Capability::Imports),
            "{language} must extract imports, or the tests below pass by \
             exclusion and prove nothing about resolution"
        );
    }
    let cpp = cpp_project();
    let main = cpp
        .iter()
        .find(|ext| ext.file_path == "main.cpp")
        .expect("fixture");
    assert_eq!(
        main.imports
            .iter()
            .map(|import| import.module_specifier.as_str())
            .collect::<Vec<_>>(),
        vec!["helper.h"],
        "`#include \"helper.h\"` had no handler anywhere before move 2"
    );
}

/// A C++ header its translation units include is wired, and by an import edge.
#[test]
fn a_cpp_header_is_wired_by_the_include_that_names_it() {
    let candidates = unwired(&graph(&cpp_project()));
    assert!(
        !candidates.contains(&"helper.h".to_string()),
        "helper.h is included by two translation units: {candidates:?}"
    );
}

/// The regression move 2 would have introduced, pinned.
///
/// `Helper.java` has no inbound `Imports` edge and never will — same-package
/// Java files import each other not at all. The moment Java stopped being
/// excluded for blindness it was reported for *absence*, which is the same
/// wrong answer with a new reason. It is wired by the `new Helper().run()` its
/// caller performs, and the scan reads that.
#[test]
fn a_same_package_java_file_is_wired_by_its_caller_not_by_an_import() {
    let extractions = java_project();
    let helper = extractions
        .iter()
        .find(|ext| ext.file_path == "Helper.java")
        .expect("fixture");
    assert!(
        helper.imports.is_empty(),
        "the premise: nothing imports Helper.java and nothing can"
    );

    let graph = graph(&extractions);
    let candidates = unwired(&graph);
    assert!(
        !candidates.contains(&"Helper.java".to_string()),
        "Helper.java is used by Main.java; reporting it would be a \
         delete-this suggestion for a file with a caller in the same graph: \
         {candidates:?}"
    );
    assert_eq!(
        graph["meta"]["devmap_rust"]["unwired_excluded_import_blind"]
            .as_u64()
            .unwrap(),
        0,
        "nothing is excluded for import-blindness in a Java corpus any more; \
         the files are answered for, not skipped"
    );
}

/// And the OFF direction for the widened evidence: a file nothing depends on is
/// still reported, in a language that now extracts imports.
///
/// Without this the widening could have turned every file wired — which passes
/// every "must not be reported" assertion above perfectly and would mean the
/// feature had been deleted rather than corrected.
#[test]
fn a_java_file_nothing_depends_on_is_still_a_candidate() {
    let mut extractions = java_project();
    extractions.push(extract_file(
        "Stranded.java",
        "package app;\n\npublic class Stranded {\n    public void idle() {}\n}\n",
    ));
    let candidates = unwired(&graph(&extractions));
    assert!(
        candidates.contains(&"Stranded.java".to_string()),
        "nothing calls, references or imports Stranded.java: {candidates:?}"
    );
}

/// An ambiguous resolution is not evidence.
///
/// One ambiguous call fans out to as many as `AMBIGUOUS_FANOUT_CAP` candidate
/// files, of which at most one is right. Counting that fan-out as wiring would
/// mark every namesake's file wired on the strength of a name collision — the
/// widened evidence laundering uncertainty into "this file is used".
#[test]
fn an_ambiguous_call_does_not_wire_its_candidates() {
    let extractions = vec![
        extract_file("caller.py", "def go():\n    return handle()\n"),
        extract_file(
            "a/one.py",
            "class A:\n    def handle(self):\n        return 1\n",
        ),
        extract_file(
            "b/two.py",
            "class B:\n    def handle(self):\n        return 2\n",
        ),
    ];
    let candidates = unwired(&graph(&extractions));
    for path in ["a/one.py", "b/two.py"] {
        assert!(
            candidates.contains(&path.to_string()),
            "{path} is reached only by an ambiguous name match, which is a \
             guess and not evidence that anything depends on it: {candidates:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Go packages. An import names a package, never a file — so the collapse onto
// one synthetic `package:` node leaves every file behind it with no inbound
// file-level edge, and every one of them was reported as unwired. Measured on
// this repository: nine of eleven remaining Go candidates.
// ---------------------------------------------------------------------------

fn go_project() -> Vec<Extraction> {
    vec![
        extract_file("go.mod", "module app\n\ngo 1.22\n"),
        extract_file(
            "cmd/main.go",
            "package main\n\nimport \"app/store\"\n\nfunc main() {\n\tstore.Open()\n}\n",
        ),
        extract_file(
            "store/store.go",
            "package store\n\nfunc Open() error {\n\treturn nil\n}\n",
        ),
        // A second file in the same package. No statement a Go author could
        // write names it, and it is part of what the import pulled in.
        extract_file(
            "store/helpers.go",
            "package store\n\nfunc normalize(s string) string {\n\treturn s\n}\n",
        ),
    ]
}

/// Go resolution needs the module map, which a real build supplies from the
/// `go.mod` files it walked. Without it `import "app/store"` resolves to
/// nothing at all and the test would pass by the import never existing.
fn go_graph(extractions: &[Extraction]) -> Value {
    let modules = vec![devmap_extract::gomod::GoModule {
        prefix: "app".to_string(),
        dir: String::new(),
        replaces: Vec::new(),
    }];
    let mut resolver = Resolver::new();
    resolver.index_go_modules(&modules);
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

/// The premise: the import really does resolve, and really does collapse onto a
/// synthetic package node rather than onto the files.
#[test]
fn a_go_import_resolves_to_a_package_node_not_to_files() {
    let extractions = go_project();
    let modules = vec![devmap_extract::gomod::GoModule {
        prefix: "app".to_string(),
        dir: String::new(),
        replaces: Vec::new(),
    }];
    let mut resolver = Resolver::new();
    resolver.index_go_modules(&modules);
    resolver.index_extractions(&extractions);
    let resolved = resolver.resolve_all(&extractions);
    let targets: Vec<&str> = resolved
        .edges
        .iter()
        .filter(|edge| {
            edge.edge_kind == devmap_extract::model::EdgeKind::Imports
                && edge.source_file == "cmd/main.go"
        })
        .map(|edge| edge.target_file.as_str())
        .collect();
    assert_eq!(
        targets,
        vec!["package:store/store"],
        "the import must resolve, and must land on the package node — if it          resolved to the files the tests below would hold for a different          reason"
    );
}

#[test]
fn every_file_in_an_imported_go_package_is_wired() {
    let candidates = unwired(&go_graph(&go_project()));
    for path in ["store/store.go", "store/helpers.go"] {
        assert!(
            !candidates.contains(&path.to_string()),
            "{path} is part of the package cmd/main.go imports; nothing can \
             name it individually and nothing should have to: {candidates:?}"
        );
    }
}

/// The OFF direction: a Go package nothing imports is still reported.
///
/// Matched on the file's own directory rather than on a prefix, because a Go
/// package does not include its subdirectories — treating `store` as covering
/// `store/internal` would exempt a genuinely stranded file one level down.
#[test]
fn a_go_package_nothing_imports_is_still_a_candidate() {
    let mut extractions = go_project();
    extractions.push(extract_file(
        "store/internal/detail/detail.go",
        "package detail\n\nfunc Helper() int {\n\treturn 1\n}\n",
    ));
    extractions.push(extract_file(
        "orphanpkg/orphan.go",
        "package orphanpkg\n\nfunc Idle() int {\n\treturn 1\n}\n",
    ));
    let candidates = unwired(&go_graph(&extractions));
    for path in ["store/internal/detail/detail.go", "orphanpkg/orphan.go"] {
        assert!(
            candidates.contains(&path.to_string()),
            "{path} is in a package nothing imports: {candidates:?}"
        );
    }
}
