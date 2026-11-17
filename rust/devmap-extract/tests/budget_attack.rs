//! Adversarial tests for the parse-budget retry.

use devmap_extract::model::ParseOutcome;
use devmap_extract::{
    extract_all_with_budgets, extract_file, extract_file_with_budget, FileRef, ParseBudgets,
};
use std::time::Duration;

const FORCE_RETRY: ParseBudgets = ParseBudgets {
    first_pass: Duration::ZERO,
    retry: Duration::from_secs(60),
    retry_total: Duration::from_secs(600),
};

const NOTEBOOK: &str = r#"{"cells":[{"cell_type":"code","metadata":{},"source":["def a():\n","    return 1\n"],"outputs":[],"execution_count":null}],"metadata":{"kernelspec":{"language":"python","name":"python3"}},"nbformat":4,"nbformat_minor":5}"#;

/// Every budget refusal site must be recognised, or the retry silently skips it.
#[test]
fn a_notebook_refused_for_budget_is_recognised_as_a_budget_refusal() {
    let refused = extract_file_with_budget("nb.ipynb", NOTEBOOK, Duration::ZERO);
    assert!(refused.is_parse_failure());
    println!("NOTEBOOK REASON: {:?}", refused.parse_outcome);
    assert!(
        refused.is_budget_refusal(),
        "notebook budget refusal is not stamped, so the retry never re-runs it"
    );
}

#[test]
fn a_notebook_refused_in_the_parallel_pass_is_recovered() {
    let files = [FileRef {
        path: "nb.ipynb",
        source: NOTEBOOK,
    }];
    let (out, report) = extract_all_with_budgets(&files, None, FORCE_RETRY, None);
    assert!(
        !out[0].is_parse_failure(),
        "notebook stayed refused; report {report:?}"
    );
}

/// Recovered-by-retry must be indistinguishable from a first-pass parse at the
/// production budget (not merely from another long-budget parse).
#[test]
fn recovered_equals_first_pass_at_the_default_budget_across_languages() {
    let fixtures: &[(&str, &str)] = &[
        ("a.py", "import os\nclass C:\n    def m(self):\n        return os.getcwd()\n\ndef f():\n    return C().m()\n"),
        ("b.rs", "use std::fmt;\nstruct S;\nimpl S { fn m(&self) -> u8 { helper() } }\nfn helper() -> u8 { 1 }\n"),
        ("c.go", "package p\nimport \"fmt\"\ntype T struct{}\nfunc (t T) M() { fmt.Println(h()) }\nfunc h() int { return 1 }\n"),
        ("d.ts", "import { x } from './y';\nexport class A { m(): number { return x(); } }\nexport function g() { return new A().m(); }\n"),
        ("e.java", "package p;\nimport java.util.List;\nclass E { void m() { n(); } void n() {} }\n"),
        ("f.js", "const a = require('a');\nfunction f() { return a.g(); }\nmodule.exports = { f };\n"),
    ];
    let files: Vec<FileRef> = fixtures
        .iter()
        .map(|(path, source)| FileRef { path, source })
        .collect();
    let (out, _) = extract_all_with_budgets(&files, None, FORCE_RETRY, None);
    for (ext, (path, source)) in out.iter().zip(fixtures) {
        let first_pass = extract_file(path, source);
        assert!(!first_pass.is_parse_failure(), "{path} baseline failed");
        assert!(
            !ext.is_parse_failure(),
            "{path} not recovered: {:?}",
            ext.parse_outcome
        );
        assert_eq!(
            serde_json::to_value(ext).unwrap(),
            serde_json::to_value(&first_pass).unwrap(),
            "{path}"
        );
    }
}

#[test]
fn file_exceeding_symbol_ceiling_is_refused_as_budget_refusal() {
    let mut source = String::new();
    for i in 0..20_500 {
        source.push_str(&format!("fn f{i}() {{}}\n"));
    }
    assert!(
        source.len() < 1024 * 1024,
        "source must be within 1 MiB limit"
    );
    let ext = extract_file_with_budget("huge_symbols.rs", &source, Duration::from_secs(20));
    assert!(
        ext.is_parse_failure(),
        "file with >20k symbols must be refused"
    );
    assert!(
        ext.is_budget_refusal(),
        "refusal must carry budget refusal prefix"
    );
    assert_eq!(
        ext.symbols.len(),
        1,
        "only the File symbol is retained on budget refusal"
    );
    let ParseOutcome::Failed { reason } = &ext.parse_outcome else {
        panic!("must be failed");
    };
    assert!(
        reason.contains("single-file ceiling"),
        "reason must name single-file ceiling: {reason}"
    );
}

#[test]
fn file_exceeding_call_ceiling_truncates_calls_and_notes_diagnostic() {
    let mut source = String::from("fn run() {\n");
    for _ in 0..30_500 {
        source.push_str("    f();\n");
    }
    source.push_str("}\n");
    assert!(source.len() < 1024 * 1024);
    let ext = extract_file_with_budget("huge_calls.rs", &source, Duration::from_secs(20));
    assert!(
        !ext.is_parse_failure(),
        "calls ceiling does not refuse file: {:?}",
        ext.parse_outcome
    );
    assert_eq!(
        ext.calls.len(),
        devmap_extract::treesitter::MAX_FILE_CALLS,
        "calls must be capped at MAX_FILE_CALLS"
    );
    assert!(
        ext.diagnostics
            .iter()
            .any(|d| d.contains("call site(s) were dropped at the single-file ceiling")),
        "diagnostic must report dropped call sites: {:?}",
        ext.diagnostics
    );
}

#[test]
fn file_exceeding_reference_ceiling_truncates_references_and_notes_diagnostic() {
    let mut source = String::from("let a = 1;\nfunction run() {\n");
    for _ in 0..40_500 {
        source.push_str("    a;\n");
    }
    source.push_str("}\n");
    assert!(source.len() < 1024 * 1024);
    let ext = extract_file_with_budget("huge_refs.js", &source, Duration::from_secs(20));
    assert!(
        !ext.is_parse_failure(),
        "reference ceiling does not refuse file: {:?}",
        ext.parse_outcome
    );
    assert_eq!(
        ext.references.len(),
        devmap_extract::treesitter::MAX_FILE_REFERENCES,
        "references must be capped at MAX_FILE_REFERENCES"
    );
    assert!(
        ext.diagnostics
            .iter()
            .any(|d| d.contains("references dropped at the single-file ceiling")),
        "diagnostic must report dropped references: {:?}",
        ext.diagnostics
    );
}

/// A ceiling that cuts a file must leave a trace the store keeps.
/// `for_durable_store()` clears `diagnostics`, so a cut noted only there reads
/// `Clean` once stored, and a build that re-resolves from cached payloads cannot
/// tell the file from one read in full (2026-11-17 Opus review of 7767f7cf). The
/// notebook and embedded extractors carry theirs on `parse_outcome` for exactly
/// this reason. The unread region is the tail from the first dropped item, so
/// the items kept must be a byte prefix of the file.
fn assert_cut_is_stored_as_the_unread_tail(
    ext: &devmap_extract::model::Extraction,
    source_len: usize,
    kept_starts: Vec<usize>,
    what: &str,
) {
    let stored = ext.for_durable_store();
    let ParseOutcome::Partial { error_ranges, .. } = &stored.parse_outcome else {
        panic!(
            "a file cut at the {what} ceiling must not store as {:?}",
            stored.parse_outcome
        );
    };
    let cut = error_ranges
        .iter()
        .map(|range| range.start_byte)
        .min()
        .expect("a cut records a range");
    assert!(
        error_ranges
            .iter()
            .any(|range| range.end_byte == source_len),
        "{what}: the unread tail must run to the end of the file: {error_ranges:?}"
    );
    let last_kept = kept_starts
        .iter()
        .copied()
        .max()
        .expect("something was kept");
    assert!(
        last_kept <= cut,
        "{what}: the items kept must be a byte prefix of the file (the last kept starts at \
         {last_kept}, the cut is at {cut})"
    );
}

#[test]
fn a_file_cut_at_the_call_ceiling_stores_as_partial_over_the_unread_tail() {
    let mut source = String::from("fn run() {\n");
    for _ in 0..30_500 {
        source.push_str("    f();\n");
    }
    source.push_str("}\n");
    let ext = extract_file_with_budget("huge_calls.rs", &source, Duration::from_secs(20));
    assert_eq!(ext.calls.len(), devmap_extract::treesitter::MAX_FILE_CALLS);
    let starts = ext.calls.iter().map(|call| call.span.start_byte).collect();
    assert_cut_is_stored_as_the_unread_tail(&ext, source.len(), starts, "call");
}

#[test]
fn a_file_cut_at_the_reference_ceiling_stores_as_partial_over_the_unread_tail() {
    let mut source = String::from("let a = 1;\nfunction run() {\n");
    for _ in 0..40_500 {
        source.push_str("    a;\n");
    }
    source.push_str("}\n");
    let ext = extract_file_with_budget("huge_refs.js", &source, Duration::from_secs(20));
    assert_eq!(
        ext.references.len(),
        devmap_extract::treesitter::MAX_FILE_REFERENCES
    );
    let starts = ext
        .references
        .iter()
        .map(|reference| reference.span.start_byte)
        .collect();
    assert_cut_is_stored_as_the_unread_tail(&ext, source.len(), starts, "reference");
}

#[test]
fn sqlddl_overrun_refuses_file_with_budget_prefix() {
    let sql = "CREATE TABLE a (id int);\nCREATE TABLE b (id int REFERENCES a(id));\n";
    let ext = extract_file_with_budget("schema.sql", sql, Duration::ZERO);
    assert!(ext.is_parse_failure());
    assert!(
        ext.is_budget_refusal(),
        "zero-budget SQL relation recovery must refuse with budget prefix"
    );
}

#[test]
fn node_traversal_cap_stops_pathological_ast() {
    // 350,000 repetitions of "1+" produces > 1,000,000 AST nodes in ~700 KB (< 1 MiB)
    let mut source = String::from("const x = ");
    for _ in 0..350_000 {
        source.push_str("1+");
    }
    source.push_str("1;");
    assert!(source.len() < 1024 * 1024);
    let ext = extract_file_with_budget("pathological_nodes.js", &source, Duration::from_secs(20));
    assert!(
        ext.is_parse_failure(),
        "AST with >1M nodes must be refused: {:?}",
        ext.parse_outcome
    );
    assert!(
        ext.is_budget_refusal(),
        "refusal must carry budget refusal prefix: {:?}",
        ext.parse_outcome
    );
}
