//! Ruleguard `dsl.Matcher` rules are framework entry points, not dead code.

use std::path::PathBuf;

use devmap_analyze::{analyze, DeadSymbolReport};
use devmap_extract::extract_file;
use devmap_extract::model::WiringKind;
use devmap_resolve::Resolver;

fn fixture_source() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata/fixtures/dead_hardening/ruleguard/rules.go");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

#[test]
fn ruleguard_matcher_rule_is_not_confident_dead() {
    let source = fixture_source();
    let extraction = extract_file("rules.go", &source);

    let wiring = extraction
        .wiring
        .iter()
        .find(|w| {
            w.target_symbol.ends_with("::revealErrorDropped")
                && w.kind == WiringKind::RuntimeEntryPoint
        })
        .unwrap_or_else(|| {
            panic!(
                "revealErrorDropped must carry RuntimeEntryPoint wiring; got {:?}",
                extraction.wiring
            )
        });
    assert!(
        wiring.details.contains("dsl.Matcher"),
        "wiring must name the Matcher contract: {wiring:?}"
    );

    let extractions = vec![extraction];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let summary = analyze(&extractions, &resolution);

    let report: Option<&DeadSymbolReport> = summary
        .dead_symbols
        .iter()
        .find(|r| r.symbol_name == "revealErrorDropped");
    if let Some(report) = report {
        assert!(
            report.is_exempt || report.confidence < 0.9,
            "revealErrorDropped must not be confident-dead; got {report:?}"
        );
        assert!(
            report.is_exempt,
            "revealErrorDropped should be exempt as a framework entry: {report:?}"
        );
    }
}
