//! Liveness for symbols whose identity carries a receiver.
//!
//! A Kotlin extension function is named `File::Person.extra`, so its
//! `ExtractedSymbol::name` is the bare `extra` while every edge naming it says
//! `File::Person.extra`. The two only meet if liveness strips the owner, and
//! until it did, splitting same-named extensions into distinct symbols — which
//! is the whole point of giving them their receiver — turned every one of them
//! into a confident dead-code finding.
//!
//! Measured on a 1,032-file Android corpus: one file declares seven
//! `private fun <T>.toJson()` on seven different types and calls each of them as
//! `it.toJson()` inside a `map { }`. The receiver `it` cannot be typed, so the
//! resolver emits an *ambiguous* edge naming one candidate, and the other six —
//! every one of them called on the next line — were reported dead at 0.9.

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
    let resolution = resolver.resolve_all(&extractions);
    analyze_liveness(&extractions, &resolution)
}

fn report_for<'a>(reports: &'a [DeadSymbolReport], symbol: &str) -> Option<&'a DeadSymbolReport> {
    reports.iter().find(|report| report.symbol_name == symbol)
}

const AMBIGUOUS: &str = concat!(
    "class Task\n",
    "class Note\n",
    "private fun Task.toJson(): String = \"t\"\n",
    "private fun Note.toJson(): String = \"n\"\n",
    "private fun encode(tasks: List<Task>, notes: List<Note>): String {\n",
    "    return tasks.map { it.toJson() }.toString() + notes.map { it.toJson() }.toString()\n",
    "}\n",
    "private fun neverCalled(): String = \"x\"\n",
);

/// An ambiguous call to a receiver-qualified name is evidence about **every**
/// candidate that carries it, not only the one the resolver named.
#[test]
fn an_ambiguous_receiver_qualified_call_downgrades_every_candidate_that_shares_its_name() {
    let reports = reports(&[("Codec.kt", AMBIGUOUS)]);
    for extension in ["Task.toJson", "Note.toJson"] {
        let found = report_for(&reports, extension)
            .unwrap_or_else(|| panic!("{extension} must appear in the report at all: {reports:?}"));
        assert!(
            found.confidence < 0.5,
            "{extension} is called on the very next line; a confident finding here \
             is a proposal to delete working code: {found:?}"
        );
        assert_eq!(
            found.exemption_reason.as_deref(),
            Some("only_ambiguous_callers"),
            "and the reason must name the ambiguity rather than a blanket exemption"
        );
    }
}

/// The control: the same file's genuinely uncalled private function is still a
/// confident finding.
///
/// Without this the test above passes for the wrong reason — a rule that
/// exempted everything would satisfy it.
#[test]
fn a_genuinely_uncalled_private_function_in_the_same_file_is_still_confident() {
    let reports = reports(&[("Codec.kt", AMBIGUOUS)]);
    let found = report_for(&reports, "neverCalled")
        .unwrap_or_else(|| panic!("`neverCalled` must be reported: {reports:?}"));
    assert!(
        !found.is_exempt && found.confidence > 0.8,
        "nothing names `neverCalled`, so the analysis must still say so: {found:?}"
    );
}

/// The relaxation is bounded to the ambiguous set and must not become an
/// exemption.
///
/// A resolved call to `A.run` says nothing about `B.run`. If the member-name
/// key were added to `called_symbols` instead of to `ambiguous_symbols`, every
/// same-named method in the file would be silently exempted — a much worse
/// trade than the one being made, and this is what would catch it.
#[test]
fn a_resolved_call_to_one_receiver_does_not_exempt_a_same_named_method_on_another() {
    let reports = reports(&[(
        "Two.kt",
        concat!(
            "class A {\n",
            "    private fun run(): Int = 1\n",
            "    fun go(): Int = run()\n",
            "}\n",
            "class B {\n",
            "    private fun run(): Int = 2\n",
            "}\n",
        ),
    )]);
    let found = report_for(&reports, "B.run")
        .unwrap_or_else(|| panic!("`B.run` must be reported: {reports:?}"));
    assert!(
        !found.is_exempt,
        "`B.run` is called by nothing; a call to `A.run` is not evidence about it: {found:?}"
    );
}
