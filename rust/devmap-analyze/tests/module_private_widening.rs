//! Exported symbols enter the dead list only where the manifest or the
//! language proves nothing outside the corpus can call them — and then only at
//! the unconfirmed tier.
//!
//! Before 2026-10-09 every exported symbol was spared for being exported, and
//! nothing said so: scholarlm's 2,467 exported `internal/` functions were never
//! examined, two of them confirmed dead (`ValidateChunks`,
//! `StampSectionRescoredAt`). Go's own import rule proves `internal/` and
//! `package main` private, so those enter at 0.4 `exported_unreferenced`; an
//! export anywhere else stays spared and is counted in `excluded` under
//! `exported`, the visibility token — not filed under an interface it merely
//! happens to fit, which would shrink the count of what was never examined.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

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

fn report_for<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> &'a DeadSymbolReport {
    reports
        .iter()
        .find(|report| report.symbol_name == symbol)
        .unwrap_or_else(|| panic!("no row for {symbol}: {reports:?}"))
}

fn reason(report: &DeadSymbolReport) -> &str {
    report.exemption_reason.as_deref().unwrap_or("")
}

const STORE: &str = "package store\n\ntype Logger interface {\n\tLog(msg string)\n}\n";

const PUBLIC_UTIL: &str = "package util\n\n\
    func DeadUtil() int { return 0 }\n\n\
    type Sink struct{}\n\n\
    func (Sink) Log(msg string) {}\n";

const INTERNAL: &str = "package secret\n\n\
    func ValidateChunks(chunks []string) bool { return len(chunks) > 0 }\n\n\
    type FileSink struct{}\n\n\
    func (FileSink) Log(msg string) {}\n\n\
    type ID int\n\n\
    func (ID) String() string { return \"id\" }\n";

const MAIN: &str = "package main\n\n\
    func main() {}\n\n\
    func StampSectionRescoredAt() {}\n\n\
    func unusedHelper() {}\n";

fn corpus() -> Vec<DeadSymbolReport> {
    reports(&[
        ("pkg/store/store.go", STORE),
        ("pkg/util/util.go", PUBLIC_UTIL),
        ("internal/secret/secret.go", INTERNAL),
        ("cmd/tool/main.go", MAIN),
    ])
}

#[test]
fn an_uncalled_export_under_internal_or_in_package_main_is_an_unconfirmed_candidate() {
    let reports = corpus();
    for symbol in ["ValidateChunks", "StampSectionRescoredAt"] {
        let row = report_for(&reports, symbol);
        assert!(!row.is_exempt, "{symbol}: {row:?}");
        assert_eq!(
            row.confidence, UNPROVEN_TIER_CONFIDENCE,
            "{symbol}: {row:?}"
        );
        assert!(
            reason(row).starts_with(EXPORTED_UNREFERENCED),
            "{symbol}: {row:?}"
        );
        let evidence = row
            .evidence
            .as_ref()
            .expect("an unconfirmed row carries evidence");
        assert_eq!(
            evidence.sites, 0,
            "{symbol}: nothing names it: {evidence:?}"
        );
    }
    // `main.go`'s file-level entry annotation no longer spares its helpers:
    // `main` is the entry point by name, the rest are ordinary functions.
    let main = report_for(&reports, "main");
    assert!(main.is_exempt, "{main:?}");
    let helper = report_for(&reports, "unusedHelper");
    assert!(earned_confident_tier(helper), "{helper:?}");
}

#[test]
fn an_export_anywhere_else_is_spared_and_counted_as_not_examined() {
    let reports = corpus();
    for symbol in ["DeadUtil", "Sink.Log"] {
        let row = report_for(&reports, symbol);
        assert!(row.is_exempt, "{symbol}: {row:?}");
        assert_eq!(
            reason(row),
            EXPORTED_NOT_EXAMINED_REASON,
            "{symbol}: {row:?}"
        );
        assert_eq!(exemption_token(reason(row)), "exported");
    }
}

#[test]
fn an_internal_implementer_of_an_interface_stays_exempt_with_the_interface_named() {
    let reports = corpus();
    let sink = report_for(&reports, "FileSink.Log");
    assert!(sink.is_exempt, "{sink:?}");
    assert!(
        reason(sink).contains("implements interface `Logger`"),
        "{sink:?}"
    );
    assert_eq!(exemption_token(reason(sink)), "interface_implementation");

    let id = report_for(&reports, "ID.String");
    assert!(id.is_exempt, "{id:?}");
    assert_eq!(reason(id), GO_STD_INTERFACE_REASON);
}

/// The disclosure counts what the analysis spared, by token, per language —
/// and its two totals partition the exempt rows.
#[test]
fn the_excluded_population_partitions_the_exempt_rows() {
    let reports = corpus();
    let exempt: Vec<&DeadSymbolReport> = reports.iter().filter(|row| row.is_exempt).collect();
    let population = ExcludedPopulation::from_grouped_rows(
        exempt
            .iter()
            .map(|row| ("go".to_string(), row.exemption_reason.clone(), 1)),
    );
    assert_eq!(
        population.not_examined + population.exempt,
        exempt.len() as u64,
        "{population:?}"
    );
    let go = &population.languages[0];
    assert_eq!(go.by_reason.get("exported"), Some(&2), "{go:?}");
    assert_eq!(go.not_examined, 2, "{go:?}");
}
