//! A clean notebook is a file the extractor read, and every counter must say so.
//!
//! **R10.** `NON_REGISTRY_CAPABILITIES` carried `("notebook", Capabilities::NONE)`
//! with the comment "its capabilities are that grammar's, resolved per file
//! rather than declared here". Nothing resolved it. `capabilities_for_language`
//! takes a `&str`, and `Extraction::language` for an `.ipynb` stays
//! `"notebook"` — the kernel name goes into `ExtractionEngine::Notebook {
//! kernel_language }` and never into `language` — so every charge site read
//! `NONE` verbatim for a file that `notebook.rs` had filled with imports and
//! calls and that `grammar_read_this_file()` reports true for.
//!
//! Four wrong answers followed from one string, and they compound:
//!
//! * charged `CallBlind` **and** `ImportBlind`, with the reason "`notebook` has
//!   no call extractor in this build" — which names a build limitation that does
//!   not exist;
//! * dropped from `files_with_call_extraction`, so the notebook left the
//!   *denominator* of the blind share while staying in its numerator, which
//!   moves the ceiling twice in the same direction;
//! * `file_is_call_blind` for every symbol it declares, so after the local-cap
//!   fix every notebook symbol would sit on `COVERAGE_LOSS_CONFIDENCE_CAP`;
//! * counted into `unwired_candidates`' `excluded_import_blind`.
//!
//! Invisible to CI because `every_registry_language_has_a_probe` iterates
//! `LANGUAGE_SPECS` and there was no `.ipynb` anywhere in the corpus. The probe
//! corpus now covers `NON_REGISTRY_CAPABILITIES` too, and this file pins the
//! consequences at the charge sites rather than at the extractor.

use devmap_analyze::{
    analyze_liveness_with_coverage, extraction_coverage, extraction_gaps, DiscoveryCoverage,
    ExtractionGap,
};
use devmap_extract::extract_file;
use devmap_extract::languages::Capability;
use devmap_extract::model::{Extraction, ExtractionEngine, ParseOutcome, SymbolKind};
use devmap_resolve::Resolver;

/// A notebook whose kernel is Python: it imports, declares, calls, and names a
/// supertype.
const NOTEBOOK: &str = r#"{
 "cells": [
  {"cell_type": "code", "execution_count": 1, "metadata": {}, "outputs": [],
   "source": ["import os\n", "\n", "\n", "class Widget(BaseWidget):\n", "    def draw(self):\n", "        return helper()\n"]},
  {"cell_type": "code", "execution_count": 2, "metadata": {}, "outputs": [],
   "source": ["def helper():\n", "    return os.getcwd()\n", "\n", "\n", "def unreferenced():\n", "    return 1\n"]}
 ],
 "metadata": {"language_info": {"name": "python"}},
 "nbformat": 4,
 "nbformat_minor": 5
}
"#;

fn notebook() -> Extraction {
    let ext = extract_file("analysis.ipynb", NOTEBOOK);
    assert!(
        matches!(ext.engine, ExtractionEngine::Notebook { .. }),
        "fixture assumption: the notebook engine ran — {:?}",
        ext.engine
    );
    assert!(
        matches!(ext.parse_outcome, ParseOutcome::Clean),
        "fixture assumption: the kernel grammar read it cleanly — {:?}",
        ext.parse_outcome
    );
    assert_eq!(
        ext.language, "notebook",
        "fixture assumption: `language` stays the container format, which is the \
         whole reason the string cannot answer the capability question"
    );
    ext
}

/// The owner: an extraction's capabilities are its *engine's*, not its
/// container format's.
#[test]
fn a_notebook_reports_the_capabilities_of_its_kernel() {
    let ext = notebook();
    let capabilities = ext.capabilities();
    for capability in [
        Capability::Calls,
        Capability::Imports,
        Capability::References,
        Capability::Heritage,
    ] {
        assert!(
            capabilities.contains(capability),
            "a Python-kernel notebook must declare Python's {} — got {capabilities:?}",
            capability.label()
        );
    }
    // And it delivers them, so the declaration is not a second unverified claim.
    assert!(!ext.calls.is_empty(), "the notebook extracted no calls");
    assert!(!ext.imports.is_empty(), "the notebook extracted no imports");
    assert!(
        !ext.references.is_empty(),
        "the notebook extracted no references — they were dropped entirely \
         until the relocation carried them forward alongside the calls"
    );
    assert!(
        ext.references.iter().any(|reference| matches!(
            reference.kind,
            devmap_extract::model::ReferenceKind::Heritage
                | devmap_extract::model::ReferenceKind::HeritageInterface
        )),
        "`class Widget(BaseWidget)` must produce a Heritage reference, or W1.2's \
         Extends edges can never fire for a notebook: {:?}",
        ext.references
    );
}

/// The charge sites: no gap is recorded for a file the extractor read.
#[test]
fn a_clean_notebook_is_charged_no_extraction_gap() {
    let extractions = vec![notebook()];
    let gaps = extraction_gaps(&extractions);
    assert!(
        gaps.is_empty(),
        "a notebook the kernel grammar read cleanly is not a coverage gap of \
         any kind: {gaps:?}"
    );

    let coverage = extraction_coverage(&extractions);
    assert_eq!(coverage.call_blind_files, 0);
    assert_eq!(coverage.import_blind_files, 0);
    assert_eq!(
        coverage.files_with_call_extraction, 1,
        "and it belongs in the denominator of the blind share, not outside both \
         sides of it: {coverage:?}"
    );
    assert!(
        coverage.is_complete(),
        "a one-notebook corpus is a complete scan: {coverage:?}"
    );
}

/// The consequence that would have bitten hardest: with the local call-blind
/// ceiling in place, a notebook symbol would have been pinned to the floor.
#[test]
fn a_notebook_symbol_is_not_capped_as_if_its_file_were_blind() {
    let extractions = vec![notebook()];
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    let outcome =
        analyze_liveness_with_coverage(&extractions, &resolution, DiscoveryCoverage::none());

    let unreferenced = outcome
        .reports
        .iter()
        .find(|report| report.symbol_name == "unreferenced")
        .unwrap_or_else(|| panic!("`unreferenced` must be reported: {:?}", outcome.reports));
    assert!(
        (unreferenced.confidence - 0.9).abs() < 1e-6,
        "a complete corpus of one clean notebook must reach the confident tier: \
         {unreferenced:?}"
    );
    assert_eq!(
        unreferenced.exemption_reason, None,
        "and carry no blindness caveat at all: {unreferenced:?}"
    );
}

/// The OFF direction. A notebook whose kernel this build genuinely cannot parse
/// must still be charged, or the fix has replaced one unconditional answer with
/// another.
#[test]
fn a_notebook_with_an_unreadable_kernel_is_still_a_gap() {
    let haskell = NOTEBOOK.replace("\"name\": \"python\"", "\"name\": \"haskell\"");
    let ext = extract_file("analysis.ipynb", &haskell);
    assert!(
        !matches!(ext.parse_outcome, ParseOutcome::Clean),
        "a kernel with no grammar in this build cannot report a clean parse: {:?}",
        ext.parse_outcome
    );
    let coverage = extraction_coverage(&[ext]);
    assert!(
        !coverage.is_complete(),
        "a notebook nothing could read is a hole in the corpus: {coverage:?}"
    );
}

/// A notebook that declares nothing is still not blind — the file was read, it
/// simply had nothing to say. The two must stay distinguishable.
#[test]
fn an_empty_notebook_is_read_not_blind() {
    let empty = r#"{
 "cells": [],
 "metadata": {"language_info": {"name": "python"}},
 "nbformat": 4,
 "nbformat_minor": 5
}
"#;
    let ext = extract_file("empty.ipynb", empty);
    let non_file_symbols = ext
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .count();
    assert_eq!(non_file_symbols, 0, "the fixture declares nothing");

    let gaps = extraction_gaps(std::slice::from_ref(&ext));
    assert!(
        gaps.iter().all(|gap| !matches!(
            gap.gap,
            ExtractionGap::CallBlind | ExtractionGap::ImportBlind
        )),
        "an empty notebook has nothing to extract, which is not the same as an \
         extractor that cannot read it: {gaps:?}"
    );
}
