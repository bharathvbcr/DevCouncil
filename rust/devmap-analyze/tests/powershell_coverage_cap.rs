//! Characterization: a PowerShell pattern-recovered file must not corpus-cap
//! Rust findings. A `.ps1` cannot call Rust symbols; charging it into
//! `is_complete()` demotes every confident dead row in the repository.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_resolve::Resolver;

#[test]
fn powershell_pattern_recovery_does_not_cap_rust_findings() {
    let extractions = vec![
        extract_file(
            "src/util.rs",
            "fn orphan() {}\n\nfn live() {}\n\nfn caller() {\n    live();\n}\n",
        ),
        extract_file(
            "install.ps1",
            "# Pattern-recovered only\nWrite-Host \"install\"\nfunction Invoke-Install { }\n",
        ),
    ];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let summary = analyze(&extractions, &resolution);
    let coverage = extraction_coverage(&extractions);

    let orphan = summary
        .dead_symbols
        .iter()
        .find(|r| r.file_path == "src/util.rs" && r.symbol_name == "orphan")
        .expect("orphan must be reported");
    assert!(
        (orphan.confidence - 0.9).abs() < 0.001,
        "a .ps1 beside Rust must not demote Rust findings below the confident tier; \
         got confidence={} reason={:?}",
        orphan.confidence,
        orphan.exemption_reason
    );
    assert_eq!(
        coverage.pattern_recovered_files, 1,
        "pattern recovery is still charged in coverage_gaps; coverage={coverage:?}"
    );
    assert!(
        coverage.is_complete(),
        "a PowerShell hole must not make a Rust corpus incomplete for capping: {coverage:?}"
    );
    assert!(
        coverage.is_complete_for("rust"),
        "Rust specifically must see a complete scan: {coverage:?}"
    );
    assert!(
        !language_can_reference("powershell", "rust"),
        "fixture assumption: powershell cannot reference rust"
    );
}
