//! A vendored minified bundle is declined, not raced against the clock.
//!
//! R4 says identical bytes give an identical graph. A 177 KB single-line
//! minified bundle broke that, and the break was not subtle. Six extractions of
//! `src/devcouncil/assets/vendor/force-graph.min.js` — the same bytes, the same
//! binary, one machine, a debug build — produced **three different answers**:
//!
//! ```text
//!   4.917s  Clean                                    449 symbols
//!   4.965s  Partial { error_ranges: [...] }          (mutated copy)
//!   5.009s  Failed { "…exceeded the 5s budget…" }      1 symbol
//! ```
//!
//! and the two `Failed` runs disagreed with *each other*, because which phase
//! the deadline landed in is written into the reason string. The file sits at
//! 4.7–5.0 s against a 5 s budget, so machine load decides the verdict. In a
//! release build on an idle machine it costs 0.9–1.7 s and looks stable; that
//! is the same coin, landing the same way twice.
//!
//! What rode on the coin flip was not a diagnostic. `Clean` publishes 449
//! symbols and lets dead-code analysis treat the file's silence as fact;
//! `Failed` publishes one `File` node, exempts the file, and is not
//! cache-admitted, so it is retried on every build. The map's answer to "is this
//! symbol dead" changed with the load average.
//!
//! `is_vendored_path` already recognised the file — but only to hang a
//! `WiringKind::Vendored` annotation on it, which exempts a file from
//! *liveness*, never from *extraction*. The knowledge existed and was not wired
//! to the parse decision. It is now: `is_minified_bundle` is the one owner of
//! the shape, and `extract_treesitter_with_budget` refuses before the grammar is
//! ever asked.
//!
//! The distinction the outcome carries is the point. A file nobody tried to
//! parse and a file whose parse ran out of clock are different facts, and only
//! the second says something is wrong. `cache.rs:150` and `model.rs:996` both
//! record what conflating them cost the last two times.

use devmap_extract::model::{ExtractionEngine, ParseOutcome, WiringKind};
use devmap_extract::wiring::is_minified_bundle;
use devmap_extract::{detect_language, extract_file, is_indexable_source};

/// A minified bundle of roughly the size and shape of the real one: ~180 KB of
/// dense single-character-identifier JavaScript across a handful of enormous
/// lines. Deterministic, so a failure is reproducible from the seed in the
/// source rather than from a checked-in 177 KB blob.
///
/// Built to be *expensive*, not merely long. The nesting and the operator
/// density are what cost tree-sitter its budget on the real file; 180 KB of
/// `var a=1;` repeated would parse in milliseconds and would prove nothing.
///
/// Measured against the skip, by handing these same bytes to
/// `extract_treesitter_with_budget` under a path the rule does not match:
/// **175,178 bytes, 11 lines, longest 34,902 — parsed in 4.35–4.56 s against
/// the 5 s budget**, over three debug-build runs. The real file measures
/// 4.7–5.0 s. So the fixture sits in the same straddle, which is the whole
/// property under test: a few hundred milliseconds of load either way decides
/// the outcome. `a_minified_bundle_is_skipped_deterministically` pins the shape
/// numerically so a later edit cannot quietly turn this into a cheap fixture
/// that would pass whatever the extractor did.
fn minified_bundle() -> String {
    let mut out = String::with_capacity(190_000);
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let names = [
        "t", "e", "n", "r", "i", "o", "a", "s", "u", "c", "l", "f", "d", "h", "p", "g",
    ];
    out.push_str("!function(t,e){\"object\"==typeof exports?e(exports):e(t.fg={})}(this,function(t){\"use strict\";");
    while out.len() < 175_000 {
        let a = names[(next() % 16) as usize];
        let b = names[(next() % 16) as usize];
        let c = names[(next() % 16) as usize];
        let d = next() % 97;
        out.push_str(&format!(
            "function {a}{d}({b},{c}){{return {b}?({c}={b}.{a}||{{}},\
             Object.keys({c}).reduce(function({a},{b}){{return {a}[{b}]=\
             ({c}[{b}]||0)+{d},{a}}},{{}})):({b}={c}||[],{b}.map(function({a}){{\
             return{{{b}:{a},{c}:{a}*{d}}}}}).filter(function({a}){{return {a}.{b}}}))}}"
        ));
        // A newline every ~35 KB: the real bundle is 177,599 bytes over five
        // lines, longest 97,856. Not one line, and nothing like source.
        if out.len() % 35_000 < 400 {
            out.push('\n');
        }
    }
    out.push_str("});\n");
    out
}

/// The path the detector already claimed, in the shape a repository actually
/// commits: a bundle vendored into an asset tree, not under `node_modules/`
/// or `dist/` — those are refused at discovery, which is why this is the case
/// that reaches the parser at all.
const BUNDLE: &str = "src/devcouncil/assets/vendor/force-graph.min.js";

/// Two extractions of identical bytes agree, and the outcome names the skip
/// rather than a timeout.
///
/// Both halves are load-bearing and they fail for different reasons. Before the
/// fix the second assertion fails on any machine — the outcome was `Clean`,
/// `Partial` or `Failed`, never a skip — while the first fails only where the
/// file straddles the budget, which is the failure the mutation fuzzer caught
/// and the one a fast machine hides.
#[test]
fn a_minified_bundle_is_skipped_deterministically() {
    let source = minified_bundle();
    assert!(
        source.len() > 150_000,
        "the fixture must be big enough to be the real problem, got {} bytes",
        source.len()
    );
    assert!(
        source.lines().count() < 20,
        "a minified bundle has no line structure, got {} lines",
        source.lines().count()
    );
    // The density is the cost. A fixture of the same size split into ordinary
    // source lines parses in milliseconds and would make the determinism
    // assertion below vacuous.
    let longest = source.lines().map(str::len).max().unwrap_or(0);
    assert!(
        longest > 20_000,
        "the fixture must have minified line lengths, longest is {longest}"
    );

    let first = extract_file(BUNDLE, &source);
    let second = extract_file(BUNDLE, &source);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "two extractions of identical bytes disagree"
    );

    let ParseOutcome::Skipped { reason } = &first.parse_outcome else {
        panic!(
            "a minified bundle must report a skip, not {:?}",
            first.parse_outcome
        );
    };
    assert!(
        reason.contains("minified"),
        "the outcome must name why the file was not parsed, got {reason:?}"
    );
    assert!(
        !reason.contains("budget") && !reason.contains("exceeded"),
        "a file nobody parsed must not report a timeout, got {reason:?}"
    );
    assert!(
        matches!(first.engine, ExtractionEngine::NotApplicable { .. }),
        "no grammar ran, so the engine must not claim one: {:?}",
        first.engine
    );
}

/// The skip is fast, because it happens before the grammar is asked.
///
/// Not a benchmark — a bound. The whole defect was a file spending 5 s to
/// produce a coin flip; a "fix" that still spent the 5 s and then discarded the
/// result would leave `dev map` just as slow on the tree that motivated this.
#[test]
fn the_skip_costs_nothing() {
    let source = minified_bundle();
    let started = std::time::Instant::now();
    let extraction = extract_file(BUNDLE, &source);
    let elapsed = started.elapsed();
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Skipped { .. }),
        "expected a skip, got {:?}",
        extraction.parse_outcome
    );
    // Two orders of magnitude below the 5 s budget the parse would have spent,
    // and far above anything a hash and a path check can cost, so this is a
    // regression detector rather than a flake.
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "a declined parse must not cost parse time, took {elapsed:?}"
    );
}

/// The file is still a node, still vendored, still addressable.
///
/// Declining to parse is not declining to record. `refused_extraction` learned
/// this the hard way — the `File` node used to be pushed only on the
/// tree-sitter path, so every refused file vanished from the graph and every
/// edge that pointed at one dangled.
#[test]
fn a_skipped_bundle_is_still_a_file_node_and_still_vendored() {
    let source = minified_bundle();
    let extraction = extract_file(BUNDLE, &source);

    assert_eq!(
        extraction.symbols.len(),
        1,
        "exactly the File node, no claimed declarations"
    );
    assert_eq!(extraction.symbols[0].qualified_name, BUNDLE);
    assert_eq!(extraction.symbols[0].span.end_byte, source.len());
    assert!(
        extraction.calls.is_empty() && extraction.imports.is_empty(),
        "nothing was parsed, so nothing may be claimed"
    );
    assert!(
        extraction
            .wiring
            .iter()
            .any(|w| w.kind == WiringKind::Vendored),
        "the vendored annotation is what exempts it from liveness: {:?}",
        extraction.wiring
    );
    assert!(
        !extraction.is_parse_failure(),
        "a file nobody tried to parse is not a parse failure"
    );
}

/// The predicate matches what a minifier names and nothing a human writes.
///
/// The suffix list is the whole rule on purpose. Widening the skip to every
/// path `is_vendored_path` matches would stop parsing ordinary readable source
/// under `vendor/` and `third_party/`, which yields real symbols today — a far
/// bigger change than this defect asks for, and one nobody measured.
#[test]
fn only_minifier_output_is_declined() {
    for path in [
        "src/devcouncil/assets/vendor/force-graph.min.js",
        "assets/app.min.css",
        "static/js/vendor/d3.min.js",
        "web/bundle.min.mjs",
        "WEB/BUNDLE.MIN.JS",
    ] {
        assert!(is_minified_bundle(path), "should be declined: {path}");
    }
    for path in [
        "vendor/github.com/pkg/errors/errors.go",
        "third_party/absl/strings/str_cat.cc",
        "src/mining.js",
        "src/admin.css",
        "src/minify.js",
        "docs/minified.md",
    ] {
        assert!(!is_minified_bundle(path), "should be parsed: {path}");
    }
}

/// Discovery still admits the file, and the language table still names it.
///
/// Pinned because the tempting shortcut is to make `is_indexable_source` reject
/// minified bundles instead. That would drop the `File` node too, and a
/// vendored asset that is *absent* from the map is a worse answer than one
/// present and marked unparsed — every edge into it would dangle, and nothing
/// would record that the repository contains it.
#[test]
fn the_bundle_is_still_discovered_and_still_javascript() {
    assert!(is_indexable_source(BUNDLE));
    assert_eq!(detect_language(std::path::Path::new(BUNDLE)), "javascript");
}
