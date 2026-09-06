//! `LanguageSpec::capabilities` is a derived fact, and this is the derivation.
//!
//! The constant these bits replace — `CALL_EXTRACTION_LANGUAGES` — rotted
//! because nothing ever compared it to behaviour. It advertised 17 languages
//! while the extractor handled 31, and it claimed a reader ("the coverage
//! report") that did not exist. A hand-maintained capability list is a comment
//! that lints clean.
//!
//! So every bit is pinned in **both directions** against a real extraction over
//! `testdata/capabilities/`:
//!
//! * a vector the extractor fills for a language whose bit is clear fails —
//!   that is a language that gained an extractor and forgot the flag;
//! * a bit set for a language whose vector stays empty fails — that is a flag
//!   claiming coverage nothing delivers.
//!
//! Plus two guards that keep the corpus honest: every registry language must
//! have a probe file (or the assertion above is vacuous for it), and every
//! grammar a probe reaches must have a declared capability row.

use devmap_extract::languages::{
    capabilities_for_language, language_capability_is_declared, Capability, LANGUAGE_SPECS,
};
use devmap_extract::model::ReferenceKind;
use devmap_extract::{extract_file, Extraction};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../testdata/capabilities"
    ))
}

/// Every probe file, extracted. Files only: the tree picks up a `.devcouncil`
/// directory on any machine where the agent loop has run in it.
fn probes() -> Vec<(String, Extraction)> {
    let mut paths: Vec<PathBuf> = fs::read_dir(corpus_dir())
        .expect("capability corpus must exist")
        .map(|e| e.expect("readable dir entry").path())
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "capability corpus is empty; the bidirectional assertions below would pass vacuously"
    );
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            let source = fs::read_to_string(&p).expect("probe file must be UTF-8");
            let extraction = extract_file(&name, &source);
            (name, extraction)
        })
        .collect()
}

/// What one extraction actually produced, as a capability set.
fn observed(extraction: &Extraction) -> BTreeSet<Capability> {
    let mut seen = BTreeSet::new();
    if !extraction.calls.is_empty() {
        seen.insert(Capability::Calls);
    }
    if !extraction.imports.is_empty() {
        seen.insert(Capability::Imports);
    }
    if !extraction.references.is_empty() {
        seen.insert(Capability::References);
    }
    if extraction.references.iter().any(|r| {
        matches!(
            r.kind,
            ReferenceKind::Heritage | ReferenceKind::HeritageInterface
        )
    }) {
        seen.insert(Capability::Heritage);
    }
    seen
}

/// Observations unioned per grammar.
///
/// Union, not per-file: two probes can legitimately reach one grammar — `.ts`
/// and `.ets` both parse as `typescript`, `.cpp` and `.metal` both as `cpp` —
/// and a capability the language has need only be demonstrated by one of them.
/// Intersecting would demand every probe exercise every feature, which makes
/// the corpus brittle without making the claim stronger.
fn observed_by_grammar() -> BTreeMap<String, BTreeSet<Capability>> {
    let mut by_grammar: BTreeMap<String, BTreeSet<Capability>> = BTreeMap::new();
    for (_, extraction) in probes() {
        by_grammar
            .entry(extraction.language.clone())
            .or_default()
            .extend(observed(&extraction));
    }
    by_grammar
}

#[test]
fn declared_capabilities_match_observed_extraction_in_both_directions() {
    let mut wrong = Vec::new();
    for (grammar, seen) in observed_by_grammar() {
        let declared = capabilities_for_language(&grammar);
        for capability in Capability::ALL {
            let is_declared = declared.contains(*capability);
            let is_observed = seen.contains(capability);
            if is_declared == is_observed {
                continue;
            }
            wrong.push(if is_observed {
                format!(
                    "{grammar}: extracts {} but the registry does not declare it — \
                     a language gained an extractor and the capability bit was not set, \
                     so every consumer still treats this language as blind",
                    capability.label()
                )
            } else {
                format!(
                    "{grammar}: declares {} but the probe extracted none — \
                     either the extractor regressed or the bit claims coverage \
                     that does not exist",
                    capability.label()
                )
            });
        }
    }
    assert!(
        wrong.is_empty(),
        "capability drift:\n  {}",
        wrong.join("\n  ")
    );
}

/// A language with no probe is a language whose bits nothing checks.
///
/// Without this, adding `LanguageSpec` #36 and declaring it `CALLS` would pass
/// the assertion above by never being looked at — reintroducing exactly the
/// unverified-claim failure the capability registry exists to end.
#[test]
fn every_registry_language_has_a_probe() {
    let covered: BTreeSet<String> = probes()
        .into_iter()
        .map(|(_, extraction)| extraction.language)
        .collect();
    let missing: Vec<&str> = LANGUAGE_SPECS
        .iter()
        .map(|spec| spec.grammar)
        .filter(|grammar| !covered.contains(*grammar))
        .collect();
    assert!(
        missing.is_empty(),
        "no probe file in testdata/capabilities/ parses as: {missing:?} — \
         add one with a call, an import, a reference and a supertype, or this \
         language's capability bits are unverified"
    );
}

/// Fail-closed has to be visible, not silent.
///
/// `capabilities_for_language` returns `NONE` for an unknown grammar, which is
/// the right default and an unreadable one: "declared to observe nothing" and
/// "never heard of it" render identically. This makes the second case loud for
/// every grammar the corpus can actually reach.
#[test]
fn every_reachable_grammar_declares_capabilities() {
    let undeclared: Vec<String> = observed_by_grammar()
        .into_keys()
        .filter(|grammar| !language_capability_is_declared(grammar))
        .collect();
    assert!(
        undeclared.is_empty(),
        "grammar reached by a probe with no capability row: {undeclared:?} — \
         it silently defaults to NONE, so its files would be charged as blind"
    );
}

/// Capability is a property of the grammar, so specs sharing one must agree.
///
/// `detect_language` returns `spec.grammar`, every dispatch site matches on it,
/// and `Extraction::language` stores it — so ArkTS cannot extract calls that
/// TypeScript does not. Declaring otherwise would make
/// `capabilities_for_language` depend on which of the two specs it found first.
#[test]
fn capabilities_agree_within_a_grammar() {
    let mut by_grammar: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for spec in LANGUAGE_SPECS {
        by_grammar.entry(spec.grammar).or_default().push(spec.name);
    }
    for (grammar, names) in by_grammar {
        if names.len() < 2 {
            continue;
        }
        let specs: Vec<_> = LANGUAGE_SPECS
            .iter()
            .filter(|s| s.grammar == grammar)
            .collect();
        let first = specs[0].capabilities;
        for spec in &specs[1..] {
            assert_eq!(
                spec.capabilities, first,
                "{} and {} both parse as `{grammar}` but declare different capabilities; \
                 the lookup is by grammar, so one of them would be silently ignored",
                specs[0].name, spec.name
            );
        }
    }
}

/// The two languages W0.2 exists for.
///
/// CFML and HCL parse `Clean`, emit a file symbol, and extract no calls — so
/// nothing in the coverage machinery charged them and every top-level symbol in
/// such a file was reported at the `extracted` tier. Pinned by name because the
/// bug is specifically that these two look identical to a language that was
/// examined and genuinely had no callers.
#[test]
fn the_call_blind_clean_parsing_languages_are_named() {
    let call_blind: Vec<&str> = LANGUAGE_SPECS
        .iter()
        .filter(|spec| !spec.capabilities.contains(Capability::Calls))
        .map(|spec| spec.grammar)
        .collect();
    assert_eq!(
        call_blind,
        // `vb` and `cobol` are already charged — VB.NET falls to
        // `scan_declarations` (`Fallback`) and COBOL is refused by
        // `UNSAFE_GRAMMARS` (`Failed`). `cfml` and `hcl` are the two that parse
        // `Clean` and escape, which is the whole of W0.2.
        vec!["vb", "cfml", "cobol", "hcl"],
        "the set of call-blind languages changed; if one gained a call \
         extractor this is the pin to update, and `extraction_coverage` stops \
         charging it"
    );
}

/// The scale of W0.3, pinned as a number.
///
/// `unwired_candidates` asks whether a file has an inbound `Imports` edge. For
/// a language in this list the answer is always no, because there are only five
/// `imports.push` sites in the whole extractor. The count is asserted so that
/// adding import extraction is visible here as the list shrinking, rather than
/// landing with no evidence that anything changed.
#[test]
fn import_blind_languages_are_the_majority() {
    let import_blind: Vec<&str> = LANGUAGE_SPECS
        .iter()
        .filter(|spec| !spec.capabilities.contains(Capability::Imports))
        .map(|spec| spec.grammar)
        .collect();
    assert_eq!(
        import_blind.len(),
        24,
        "import-blind languages: {import_blind:?}"
    );
    for expected in [
        "java", "csharp", "c", "cpp", "ruby", "php", "swift", "kotlin",
    ] {
        assert!(
            import_blind.contains(&expected),
            "{expected} is expected to be import-blind"
        );
    }
}
