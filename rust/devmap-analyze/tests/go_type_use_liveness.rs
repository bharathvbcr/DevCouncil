//! A Go type that its own file writes in type position (`var x t`) is not a
//! confident dead-code finding, and the sentence that explains a capped corpus
//! must agree with the cap it describes.
//!
//! Found on this repository: `toolsCallParams` in `devcouncil/mcp/server.go` is
//! declared as a struct and used as `var params toolsCallParams` in `callTool`,
//! yet sat in the non-exempt 0.9 tier. The extractor emits the `Type`
//! reference; the resolver does not bind it when another function in the same
//! file has a *qualified*-typed binding of the same name (`params
//! json.RawMessage` in `callBudgetFor`): the `@mod` slot is read back
//! file-wide, so `var params toolsCallParams` is routed down the qualified-type
//! path under the wrong qualifier. A plain `var x t` fixture binds fine, which
//! is why the shape needs the shadowing parameter.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;
use std::collections::BTreeMap;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

fn row<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|report| report.symbol_name == symbol)
}

fn confident_and_unexempt(report: &DeadSymbolReport) -> bool {
    report.exemption_reason.is_none() && report.confidence >= 0.9
}

const PATH: &str = "internal/mcp/server.go";

/// The `toolsCallParams` shape: a type used by `var params T`, in a file where
/// another function has a `params` parameter of a package-qualified type.
const SHADOWED: &str = concat!(
    "package mcp\n",
    "import \"encoding/json\"\n",
    "type t struct {\n\tName string\n}\n",
    "type u struct{}\n",
    "func budget(params json.RawMessage) int {\n\treturn len(params)\n}\n",
    "func call() {\n\tvar params t\n\t_ = params\n}\n",
);

#[test]
fn a_go_type_used_beside_a_same_named_qualified_binding_is_not_confident_dead() {
    let rows = reports(&[(PATH, SHADOWED)]);
    if let Some(t) = row(&rows, "t") {
        assert!(
            !confident_and_unexempt(t),
            "`var params t` uses t, yet it is a confident non-exempt dead row: {t:?}"
        );
    }
}

#[test]
fn the_plain_same_file_type_use_is_not_dead_either() {
    let source = "package mcp\ntype t struct{}\nfunc call() {\n\tvar x t\n\t_ = x\n}\n";
    let rows = reports(&[(PATH, source)]);
    if let Some(t) = row(&rows, "t") {
        assert!(!confident_and_unexempt(t), "{t:?}");
    }
}

#[test]
fn an_unmentioned_go_type_stays_confident() {
    let rows = reports(&[(PATH, SHADOWED)]);
    let u = row(&rows, "u").expect("u is named nowhere and must be reported");
    assert!(
        confident_and_unexempt(u),
        "an unmentioned type must stay at the confident tier, not be capped to 0.4: {u:?}"
    );
}

#[test]
fn a_truly_unreferenced_go_function_stays_confident() {
    let source = "package mcp\nfunc orphan() {}\nfunc main() {}\n";
    let rows = reports(&[(PATH, source)]);
    let orphan = row(&rows, "orphan").expect("orphan must be reported");
    assert!(confident_and_unexempt(orphan), "{orphan:?}");
}

/// Rust files read cleanly, one Python file failed to parse: the corpus is
/// incomplete, Python is capped, Rust is not.
fn rust_clean_python_blind() -> ExtractionCoverage {
    ExtractionCoverage {
        parse_failed_files: 1,
        files_with_call_extraction: 5,
        blind_by_language: BTreeMap::from([("python".to_string(), 1)]),
        covered_by_language: BTreeMap::from([("rust".to_string(), 3), ("python".to_string(), 2)]),
        ..ExtractionCoverage::default()
    }
}

#[test]
fn the_degraded_sentence_does_not_say_the_confident_tier_is_shut_while_a_language_is_eligible() {
    let coverage = rust_clean_python_blind();
    assert!(!coverage.is_complete());
    assert!(coverage.is_complete_for("rust"));
    assert!((coverage.cap_for("rust", 0.9) - 0.9).abs() < 1e-6);
    assert!(coverage.cap_for("python", 0.9) < 0.9);

    let reason = coverage.degraded_reason().expect("incomplete corpus");
    assert!(
        reason.contains("capped below the confident tier only for python")
            && reason.contains("rust remain eligible"),
        "the sentence must name which languages the cap reaches: {reason}"
    );
    assert!(
        !reason.contains("every language"),
        "an eligible language exists, so the cap is not universal: {reason}"
    );
}

#[test]
fn the_degraded_sentence_says_every_language_when_a_refusal_reaches_all() {
    let coverage = ExtractionCoverage {
        discovery_refused_files: 1,
        files_with_call_extraction: 5,
        covered_by_language: BTreeMap::from([("rust".to_string(), 5)]),
        ..ExtractionCoverage::default()
    };
    assert!(!coverage.is_complete_for("rust"));
    assert!(coverage.cap_for("rust", 0.9) < 0.9);
    let reason = coverage.degraded_reason().expect("incomplete corpus");
    assert!(
        reason.contains("findings in every language are capped below the confident tier"),
        "{reason}"
    );
}
