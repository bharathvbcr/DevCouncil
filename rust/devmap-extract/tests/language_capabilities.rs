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
    language_capability_is_declared, Capabilities, Capability, LANGUAGE_SPECS,
    NON_REGISTRY_CAPABILITIES,
};
use devmap_extract::model::ReferenceKind;
use devmap_extract::{extract_file, Extraction};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

fn corpus_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../testdata/capabilities"
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

/// What each grammar *declares*, asked the way production asks it.
///
/// `Extraction::capabilities()` and not `capabilities_for_language(&language)`,
/// because those two are the same function for every grammar but one and the
/// exception is the whole point: an `.ipynb` stores `language: "notebook"` while
/// the parse ran under its kernel's grammar, so the string answers `NONE` for a
/// file that produced calls, imports, references and heritage. A matrix test
/// that asks a different function than the four production charge sites ask is
/// checking a claim nothing depends on.
fn declared_by_grammar() -> BTreeMap<String, Capabilities> {
    let mut by_grammar: BTreeMap<String, Capabilities> = BTreeMap::new();
    for (_, extraction) in probes() {
        by_grammar.insert(extraction.language.clone(), extraction.capabilities());
    }
    by_grammar
}

/// A bit deliberately left clear for a grammar that can be observed producing
/// it, with the reason.
///
/// **The gap this closes.** The matrix below proves `declared ==
/// observed-on-this-corpus`, which is not `declared == what the extractor
/// does`: a bit stays clear either because the extractor cannot produce it, or
/// because no probe happened to ask. Those two are indistinguishable, and the
/// second is the failure mode the whole capability registry exists to end —
/// `CALL_EXTRACTION_LANGUAGES` rotted by being a list nothing compared against
/// behaviour.
///
/// So an observation for a clear bit fails, *unless* the pair is named here.
/// Naming it converts "nobody checked" into a recorded decision, and the entry
/// is itself checked: a row whose observation stops happening fails too, so a
/// stale exemption cannot outlive the behaviour it excuses.
///
/// Under-claiming is the safe direction — a consumer treats the language as
/// blind and is more conservative, where a bit claimed but not delivered is the
/// exact failure W0.1 exists to prevent — but safe is not the same as
/// unexamined.
const UNDER_CLAIMED: &[(&str, Capability, &str)] = &[(
    "liquid",
    Capability::Heritage,
    "the vendored tree-sitter-liquid grammar fragments the template run for many \
     real script bodies (see `liquid_script_scanner_limits.rs`), so heritage is \
     recoverable from some `.liquid` files and not from others. Declaring the bit \
     would claim a coverage the grammar does not reliably deliver; leaving it \
     clear costs only conservatism.",
)];

fn is_under_claimed(grammar: &str, capability: Capability) -> bool {
    UNDER_CLAIMED
        .iter()
        .any(|(name, declared, _)| *name == grammar && *declared == capability)
}

#[test]
fn declared_capabilities_match_observed_extraction_in_both_directions() {
    let declared_by = declared_by_grammar();
    let mut wrong = Vec::new();
    for (grammar, seen) in observed_by_grammar() {
        let declared = declared_by
            .get(&grammar)
            .copied()
            .unwrap_or(Capabilities::NONE);
        for capability in Capability::ALL {
            let is_declared = declared.contains(*capability);
            let is_observed = seen.contains(capability);
            if is_declared == is_observed {
                continue;
            }
            if is_observed && is_under_claimed(&grammar, *capability) {
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

/// Every recorded under-claim must still be observable.
///
/// The vacuity guard for `UNDER_CLAIMED`. Without it, an entry whose extractor
/// stopped producing the capability — or which was never right — sits in the
/// table forever excusing a bit nothing checks, which is the state the table
/// exists to end.
#[test]
fn every_recorded_under_claim_is_still_observed() {
    let seen = observed_by_grammar();
    let stale: Vec<&str> = UNDER_CLAIMED
        .iter()
        .filter(|(grammar, capability, _)| {
            !seen
                .get(*grammar)
                .is_some_and(|caps| caps.contains(capability))
        })
        .map(|(grammar, _, _)| *grammar)
        .collect();
    assert!(
        stale.is_empty(),
        "{stale:?} are recorded as deliberately under-claiming a capability the \
         corpus no longer observes — either the probe stopped exercising it, or \
         the exemption has outlived its reason and should be deleted"
    );
    for (_, _, reason) in UNDER_CLAIMED {
        assert!(
            reason.len() > 40,
            "an under-claim without a real reason is the unexamined state it \
             exists to replace: {reason:?}"
        );
    }
}

/// The `LANGUAGE_SPECS`-only coverage guard, extended to the rest of the
/// registry.
///
/// `every_registry_language_has_a_probe` iterates `LANGUAGE_SPECS` and stops
/// there, so the thirteen `NON_REGISTRY_CAPABILITIES` rows were unverified — and
/// one of them was wrong in the direction that matters. `("notebook",
/// Capabilities::NONE)` sat beside a `notebook.rs` that fills
/// `extraction.imports` and `extraction.calls`, and nothing could notice
/// because there was no `.ipynb` in the corpus for the matrix to look at.
#[test]
fn every_declared_language_has_a_probe() {
    let covered: BTreeSet<String> = probes()
        .into_iter()
        .map(|(_, extraction)| extraction.language)
        .collect();
    let missing: Vec<&str> = NON_REGISTRY_CAPABILITIES
        .iter()
        .map(|(name, _, _)| *name)
        .filter(|name| !covered.contains(*name))
        .collect();
    assert!(
        missing.is_empty(),
        "no probe file in testdata/capabilities/ parses as: {missing:?} — these \
         rows declare capabilities nothing compares against behaviour, which is \
         the state `CALL_EXTRACTION_LANGUAGES` rotted in"
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

/// The three languages `unwired_candidates` still cannot answer for, by name.
///
/// This assertion used to read `import_blind.len() == 24` and to be titled
/// "the majority", because `unwired_candidates` asks whether a file has an
/// inbound `Imports` edge and for 24 of 35 languages the answer was
/// structurally *no*: the whole extractor had five `imports.push` sites and no
/// `#include` handler anywhere. W0.3 move 2 turned that number into four;
/// Swift then left because `import Foundation` names a module the same way
/// `import "app/store"` names a Go package.
///
/// It is now pinned by **name with a reason**, not by count. A count says the
/// list changed; it does not say whether a language left the list because an
/// extractor was written or because a bit was set by hand — which is the exact
/// rot `CALL_EXTRACTION_LANGUAGES` died of. Each entry below is a decision that
/// has to survive review to change:
///
/// * **`csharp`** — `using System;` names a *namespace*. A C# namespace spans
///   any number of files and one file may declare several, so no rule of the
///   language or its tooling maps a `using` to a file. Extracting one would
///   mean inventing a convention C# does not have.
/// * **`vb`** — `Imports System.Collections` is C#'s case again, and VB.NET has
///   no linked grammar besides: it reaches `scan_declarations`, whose
///   `Fallback` engine `grammar_read_this_file` already answers `false` for, so
///   its files are charged as coverage loss rather than as import blindness.
/// * **`cobol`** — `COPY MYCOPY.` really does name a copybook file, but COBOL
///   is refused by `UNSAFE_GRAMMARS` and reports `ParseOutcome::Failed`. Its
///   files are charged as a parse failure, which is a *stronger* signal than
///   import blindness and reaches `unwired_candidates` through a different,
///   earlier branch. Claiming imports for a file no grammar read would be the
///   "a check that could not run reports what a check that passed reports"
///   failure this repository exists to refuse.
///
/// SQL is not in `LANGUAGE_SPECS`; it is the remaining `NON_REGISTRY` grammar
/// that extracts calls but not imports (`.read` / `\i` are client commands,
/// not language-level file imports). Named beside these so it cannot hide.
///
/// The two that were added rather than declined are recorded here as well, so
/// the boundary is legible from one place: `hcl` because
/// `module { source = "./modules/vpc" }` names a path, and `cfml` because
/// `template="header.cfm"` does — even though CFML's `<cfscript>` bodies stay
/// opaque to its grammar and its `include` statements are a stated residual.
#[test]
fn the_languages_without_import_extraction_are_named_with_reasons() {
    let import_blind: Vec<&str> = LANGUAGE_SPECS
        .iter()
        .filter(|spec| !spec.capabilities.contains(Capability::Imports))
        .map(|spec| spec.grammar)
        .collect();
    assert_eq!(
        import_blind,
        vec!["csharp", "vb", "cobol"],
        "the set of import-blind languages changed; each entry is a documented \
         decision, so adding or removing one means updating the reason above"
    );

    let fallback_calls_without_imports: Vec<&str> = NON_REGISTRY_CAPABILITIES
        .iter()
        .filter(|(_, caps, _)| {
            caps.contains(Capability::Calls) && !caps.contains(Capability::Imports)
        })
        .map(|(name, _, _)| *name)
        .collect();
    assert_eq!(
        fallback_calls_without_imports,
        vec!["sql"],
        "a NON_REGISTRY grammar that extracts calls but not imports must be \
         named: shell left this list when `source` / `.` gained an extractor"
    );
}

/// Every language the dispatcher routes must declare `IMPORTS`, and vice versa.
///
/// The registry and `langimports` are two lists of the same fact, and the way
/// that goes wrong is not subtle: a module added without its flag extracts
/// imports that every consumer discards as "this language is blind", and a flag
/// set without its module claims coverage that does not exist. The
/// bidirectional probe above catches both *for the grammars the corpus
/// reaches*; this catches them at the source, for all of them, without needing
/// a fixture per language.
#[test]
fn the_import_dispatcher_and_the_registry_agree() {
    use devmap_extract::langimports::IMPORT_EXTRACTION_LANGUAGES;

    // Languages whose *specialised* arm in `extract_node` pushes imports rather
    // than the dispatcher — they are absent from `langimports` on purpose, and
    // their flag is older than this module.
    const SPECIALISED: &[&str] = &[
        "typescript",
        "tsx",
        "javascript",
        "python",
        "go",
        "rust",
        "svelte",
        "vue",
        "astro",
        "liquid",
    ];

    let declared: BTreeSet<&str> = LANGUAGE_SPECS
        .iter()
        .filter(|spec| spec.capabilities.contains(Capability::Imports))
        .map(|spec| spec.grammar)
        .chain(
            NON_REGISTRY_CAPABILITIES
                .iter()
                .filter(|(_, caps, _)| caps.contains(Capability::Imports))
                .map(|(name, _, _)| *name),
        )
        .collect();
    let dispatched: BTreeSet<&str> = IMPORT_EXTRACTION_LANGUAGES.iter().copied().collect();
    let specialised: BTreeSet<&str> = SPECIALISED.iter().copied().collect();

    // **R11.** The list and the dispatcher, checked against each other rather
    // than the list against a second list.
    //
    // `IMPORT_EXTRACTION_LANGUAGES` claimed to be "derived from the dispatcher
    // above by the test that reads it" and was a hand-written const compared
    // only against `LANGUAGE_SPECS` — two hand lists, with nothing looking at
    // the `match` arms. An arm added for a grammar with no registry row, or
    // removed while the const stayed, was caught only incidentally by the probe
    // corpus. `extracts_imports` asks the arms, so this closes both directions.
    let every_key: BTreeSet<&str> = LANGUAGE_SPECS
        .iter()
        .map(|spec| spec.grammar)
        .chain(dispatched.iter().copied())
        .chain(specialised.iter().copied())
        .collect();
    let arms: BTreeSet<&str> = every_key
        .iter()
        .copied()
        .filter(|grammar| devmap_extract::langimports::extracts_imports(grammar))
        .collect();
    assert_eq!(
        arms,
        dispatched,
        "IMPORT_EXTRACTION_LANGUAGES disagrees with the dispatcher it names: \
         arms without an entry {:?}, entries without an arm {:?}",
        arms.difference(&dispatched).collect::<Vec<_>>(),
        dispatched.difference(&arms).collect::<Vec<_>>()
    );

    let dispatched_without_flag: Vec<&&str> = dispatched.difference(&declared).collect();
    assert!(
        dispatched_without_flag.is_empty(),
        "{dispatched_without_flag:?} have an import extractor but no IMPORTS bit, \
         so every consumer still treats them as import-blind and their edges are \
         extracted and then ignored"
    );

    let flagged_without_extractor: Vec<&str> = declared
        .difference(&dispatched)
        .copied()
        .filter(|grammar| !specialised.contains(grammar))
        .collect();
    assert!(
        flagged_without_extractor.is_empty(),
        "{flagged_without_extractor:?} declare IMPORTS but no arm extracts them — \
         the bit claims coverage nothing delivers"
    );

    // And the specialised list is not a place to hide a language: every entry
    // must really be flagged, or it is a stale name rather than an exemption.
    let stale: Vec<&&str> = specialised.difference(&declared).collect();
    assert!(
        stale.is_empty(),
        "{stale:?} are listed as having a specialised import arm but do not \
         declare IMPORTS; the exemption list has rotted"
    );
}
