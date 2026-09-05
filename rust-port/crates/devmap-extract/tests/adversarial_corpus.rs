//! An adversarial sweep across every language spec.
//!
//! The existing hardening tests pin specific shapes found by specific bugs —
//! deeply nested Go types, malformed sources for one grammar. This file asks a
//! different question: for *every* one of the 35 language specs, does hostile
//! input make the extractor panic, fabricate, or lie about what it did?
//!
//! The invariants below are the ones a consumer actually depends on. They are
//! stated over the whole spec table rather than a sample, because a language
//! that nobody wrote a fixture for is exactly where an unchecked assumption
//! survives — the SC34 and K2 shape, where a language was silently absent from
//! every graph and parity stayed green.

use devmap_extract::languages::LANGUAGE_SPECS;
use devmap_extract::model::{ParseOutcome, SymbolKind};
use devmap_extract::treesitter::extract_treesitter_with_budget;
use devmap_extract::{extract_file, treesitter::DEFAULT_PARSE_BUDGET};
use std::time::Duration;

/// Short on purpose. The production budget is `DEFAULT_PARSE_BUDGET` (5 s); at
/// 35 specs x 14 inputs a suite that let each case run to that would take up to
/// 40 minutes. 300 ms is well above every legitimate parse measured here and
/// keeps the sweep to seconds — and the assertion below pins the relationship
/// so this cannot drift into testing a budget production does not use.
const SWEEP_BUDGET: Duration = Duration::from_millis(300);

/// Grammars this sweep cannot run, and why.
///
/// **Not a silent skip.** The sweep asserts its own coverage as
/// `covered + excluded == total`, names every exclusion in the failure
/// message, and fails if this list and the specs it names disagree — the same
/// rule the code under test is held to. A test that quietly stopped covering a
/// language would be the exact defect this file exists to find.
///
/// **Currently empty, and that is the point.** COBOL was here: its vendored
/// grammar does not terminate on malformed input (measured 2026-09-04 — six
/// bytes of NUL, and a BOM followed by garbage, each ran past three minutes),
/// and no in-process bound stops it. Rather than leave the sweep permanently
/// blind to one language, the grammar was unlinked — it yielded zero
/// declarations and zero calls even on well-formed source, so unlinking cost
/// nothing measurable — and COBOL is now refused before any parser sees it. It
/// is back under test here, exercising that refusal.
const UNRUNNABLE_GRAMMARS: &[(&str, &str)] = &[];

/// Hostile inputs, each with the failure mode it is probing.
fn hostile_sources() -> Vec<(&'static str, String)> {
    vec![
        ("empty", String::new()),
        ("whitespace only", "   \n\t\n   \n".to_string()),
        ("nul bytes", "a\0b\0c\n".to_string()),
        (
            "lone surrogates escaped",
            "\"\\ud800\\udc00\"\n".to_string(),
        ),
        ("bom then garbage", "\u{feff}????\n".to_string()),
        ("cr only line endings", "a\rb\rc\r".to_string()),
        // A single line far past the fallback scanner's MAX_LINE_BYTES.
        ("one huge line", format!("{}\n", "x".repeat(200_000))),
        // Unbalanced delimiters in every direction.
        ("unclosed braces", "{".repeat(500)),
        ("unclosed parens", "(".repeat(500)),
        ("closing only", "}".repeat(500)),
        // Deep nesting, the classic recursive-descent killer.
        (
            "deep nesting",
            format!("{}{}", "{".repeat(2_000), "}".repeat(2_000)),
        ),
        // Text that looks like a declaration in many languages at once.
        (
            "polyglot declaration soup",
            "class A { func b() { def c(): fn d() -> int { public void e() {\n".to_string(),
        ),
        // Multibyte characters straddling every offset the scanner might slice.
        ("multibyte density", "日本語のテキスト🎌🎌🎌\n".repeat(200)),
        // A very long identifier.
        (
            "giant identifier",
            format!("class {} {{}}\n", "N".repeat(50_000)),
        ),
    ]
}

/// Nothing hostile may panic, and no extraction may describe itself falsely.
///
/// Checked for every spec's first extension, against every hostile source:
///
/// * the call returns (a panic here fails the test by unwinding);
/// * every symbol span lies inside the source, and `start <= end` — a span into
///   nowhere reads to a consumer as a real location;
/// * no symbol has an empty name, which nothing downstream can address;
/// * a `Clean` outcome never accompanies a fabricated non-`File` symbol on
///   input that contains no identifier at all.
#[test]
fn no_language_panics_or_fabricates_on_hostile_input() {
    let sources = hostile_sources();
    let mut checked = 0usize;

    let mut excluded: Vec<&str> = Vec::new();
    for spec in LANGUAGE_SPECS {
        if let Some((_, why)) = UNRUNNABLE_GRAMMARS
            .iter()
            .find(|(grammar, _)| *grammar == spec.grammar)
        {
            excluded.push(spec.name);
            eprintln!("adversarial sweep EXCLUDES {} — {why}", spec.name);
            continue;
        }
        let ext = spec
            .extensions
            .first()
            .unwrap_or_else(|| panic!("{} declares no extension", spec.name));
        for (label, source) in &sources {
            let path = format!("hostile{ext}");
            let extraction =
                extract_treesitter_with_budget(&path, spec.grammar, source, SWEEP_BUDGET);
            checked += 1;

            for symbol in &extraction.symbols {
                assert!(
                    symbol.span.start_byte <= symbol.span.end_byte,
                    "{} / {label}: inverted span on {:?}",
                    spec.name,
                    symbol.qualified_name
                );
                assert!(
                    symbol.span.end_byte <= source.len(),
                    "{} / {label}: span {}..{} escapes a {}-byte source on {:?}",
                    spec.name,
                    symbol.span.start_byte,
                    symbol.span.end_byte,
                    source.len(),
                    symbol.qualified_name
                );
                assert!(
                    !symbol.name.is_empty(),
                    "{} / {label}: a symbol with no name cannot be addressed",
                    spec.name
                );
            }

            // Every edge endpoint must name something, or the resolver joins on
            // an empty key.
            for call in &extraction.calls {
                assert!(
                    !call.callee_name.trim().is_empty(),
                    "{} / {label}: a call with an empty callee joins on nothing",
                    spec.name
                );
            }

            // An input with no identifier characters cannot yield declarations.
            let has_identifier = source.chars().any(|c| c.is_alphanumeric());
            if !has_identifier {
                let invented: Vec<_> = extraction
                    .symbols
                    .iter()
                    .filter(|s| s.kind != SymbolKind::File)
                    .map(|s| s.qualified_name.as_str())
                    .collect();
                assert!(
                    invented.is_empty(),
                    "{} / {label}: declarations invented from input with no \
                     identifiers: {invented:?}",
                    spec.name
                );
            }

            // The outcome must not be a bare success carrying a failure reason.
            if let ParseOutcome::Failed { reason } | ParseOutcome::Fallback { reason } =
                &extraction.parse_outcome
            {
                assert!(
                    !reason.trim().is_empty(),
                    "{} / {label}: a non-clean outcome with no reason is unactionable",
                    spec.name
                );
            }
        }
    }

    assert!(
        SWEEP_BUDGET < DEFAULT_PARSE_BUDGET,
        "the sweep must run inside the production budget, not above it"
    );
    // Coverage is asserted with both numbers, never just the one that passed.
    let covered_specs = LANGUAGE_SPECS.len() - excluded.len();
    assert_eq!(
        checked,
        covered_specs * sources.len(),
        "every runnable spec must be exercised against every hostile source; \
         covered {covered_specs} of {} specs, excluded {excluded:?}",
        LANGUAGE_SPECS.len()
    );
    assert_eq!(
        excluded.len(),
        UNRUNNABLE_GRAMMARS.len(),
        "the exclusion list and the specs it names must agree: excluded \
         {excluded:?} against {UNRUNNABLE_GRAMMARS:?}"
    );
}

/// A hostile *file name* must not escape into the extraction's identity.
///
/// `V1` in this repo's audit: any generated HTML/JSON interpolation goes
/// through one escape helper. The extraction is upstream of that, so what it
/// must guarantee is weaker but load-bearing — the path it echoes back is the
/// path it was given, unmodified, so a sink can escape it once and correctly.
#[test]
fn a_hostile_file_name_is_echoed_verbatim_not_reinterpreted() {
    for name in [
        "x<img src=x onerror=alert(1)>.ts",
        "a\"b'c.py",
        "../../etc/passwd.go",
        "spaces and\ttabs.rs",
        "emoji🎌.py",
    ] {
        let extraction = extract_file(name, "def f(): pass\n");
        assert_eq!(
            extraction.file_path, name,
            "the path must survive extraction unmodified"
        );
        let file_nodes: Vec<_> = extraction
            .symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::File)
            .collect();
        assert_eq!(file_nodes.len(), 1, "exactly one File node for {name:?}");
        assert_eq!(file_nodes[0].qualified_name, name);
    }
}
