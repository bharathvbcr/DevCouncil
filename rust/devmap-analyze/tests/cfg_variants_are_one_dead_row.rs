//! Two `#[cfg]` variants of one method are one dead row.
//!
//! `ShutdownSignals::recv` is written once for unix and once for windows. The
//! extractor emits both — each is a real declaration with its own span — and
//! both carry one qualified name, which is one graph node with one set of
//! callers. `devmap dead` listed that node twice.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

const SIGNALS: &str = "\
pub struct ShutdownSignals;

impl ShutdownSignals {
    #[cfg(unix)]
    fn recv(&mut self) -> &'static str {
        \"unix\"
    }

    #[cfg(windows)]
    fn recv(&mut self) -> &'static str {
        \"windows\"
    }

    fn drain(&self) {}
}
";

fn reports(files: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

#[test]
fn two_cfg_variants_of_an_uncalled_method_are_one_row() {
    let found = reports(&[("src/daemon.rs", SIGNALS)]);
    let symbols = found
        .iter()
        .filter(|report| report.file_path == "src/daemon.rs" && !report.is_exempt)
        .map(|report| report.symbol_name.as_str());
    let mut recv = 0;
    let mut drain = 0;
    for symbol in symbols {
        match symbol {
            "ShutdownSignals.recv" => recv += 1,
            "ShutdownSignals.drain" => drain += 1,
            _ => {}
        }
    }
    assert_eq!(recv, 1, "one identity, one row; got {found:?}");
    assert_eq!(drain, 1, "a method with one declaration is still reported once");
}

#[test]
fn the_dead_list_has_no_duplicate_rows() {
    let found = reports(&[("src/daemon.rs", SIGNALS)]);
    let mut keys: Vec<(&str, &str)> = found
        .iter()
        .map(|report| (report.file_path.as_str(), report.symbol_name.as_str()))
        .collect();
    let total = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), total, "every (file, symbol) appears once; got {found:?}");
}

#[test]
fn a_variant_overlapping_a_parse_error_exempts_the_identity() {
    let extraction_source = SIGNALS;
    let mut ext = extract_file("src/daemon.rs", extraction_source);
    // The windows variant's body, as if it had failed to parse.
    let at = extraction_source.find("\"windows\"").unwrap();
    ext.parse_outcome = devmap_extract::model::ParseOutcome::Partial {
        error_ranges: vec![devmap_extract::model::TextRange {
            start_byte: at,
            end_byte: at + 1,
        }],
    };
    let extractions = vec![ext];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    let found = analyze_liveness(&extractions, &resolution);

    let recv: Vec<_> = found
        .iter()
        .filter(|report| report.symbol_name == "ShutdownSignals.recv")
        .collect();
    assert_eq!(recv.len(), 1, "got {recv:?}");
    assert!(
        recv[0].is_exempt,
        "a parse error inside either variant may hide a caller; got {recv:?}"
    );
}
