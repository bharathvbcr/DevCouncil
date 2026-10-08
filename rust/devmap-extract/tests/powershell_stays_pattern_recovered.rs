//! PowerShell is pattern-recovered, and says so.
//!
//! Decision 2026-10-08 (`docs/devmap/BENCHMARK_HARDENING_AUDIT.md`): no
//! PowerShell grammar is linked — one would be a new dependency, and none was
//! authorised — so `.ps1` declarations are recovered by pattern. That is only
//! acceptable while the extraction is labelled as such: a `Fallback` outcome is
//! what keeps dead-code analysis from treating the file's silence as a fact and
//! what `devmap status` lists under `pattern_recovered`. A grammar landing, or
//! the label being dropped, fails this test and reopens the decision.

use devmap_extract::extract_file;
use devmap_extract::model::{ParseOutcome, SymbolKind};

#[test]
fn a_powershell_script_is_labelled_pattern_recovered_and_keeps_its_functions() {
    let source = "function Get-Widget {\n    param($Name)\n    Write-Output $Name\n}\n\n\
                  function Install-Thing { Get-Widget -Name x }\n";
    let extraction = extract_file("scripts/install.ps1", source);
    assert_eq!(extraction.language, "powershell");
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Fallback { .. }),
        "a grammarless file must not read as parsed: {:?}",
        extraction.parse_outcome
    );
    let functions: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.name.as_str())
        .collect();
    for name in ["Get-Widget", "Install-Thing"] {
        assert!(
            functions.contains(&name),
            "{name} not recovered: {functions:?}"
        );
    }
}
