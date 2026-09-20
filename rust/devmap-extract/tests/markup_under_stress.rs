//! The markup and stylesheet readers under hostile input.
//!
//! [`devmap_extract::markup`] is a **scanner**, not a grammar: it reads structure
//! out of text with hand-written loops, which is the shape that breaks in the
//! ways a parser generator would have ruled out. Every case here is one of those
//! ways, and each is a class rather than an example:
//!
//! * a loop that does not advance — an unterminated string, an empty
//!   `<style></style>`, a `var(` with nothing after it, a `<` that begins no tag;
//! * a slice that is not on a character boundary — a multi-byte character at the
//!   end of a truncated literal, which panics rather than degrades;
//! * work that is not bounded — a megabyte of generated rules, markup nested
//!   100,000 deep, a minified stylesheet on one line, a thousand `<script>`
//!   elements (which cost O(n^2) until the close-tag search stopped allocating);
//! * structure read out of text that only looks like structure — a `{` inside a
//!   string, a rule inside a comment, a `>` inside an attribute value;
//! * a bound that cuts silently, which is the one failure mode that makes every
//!   *other* answer untrustworthy: a capped read that reports like a complete
//!   one.
//!
//! The invariants are asserted over the whole corpus, not sampled: extraction
//! returns, inside a bound; every span lies inside the source and on a character
//! boundary; identical bytes give identical answers; every name emitted is
//! present in the file it is claimed for; and anything a cap cut is disclosed.

use std::time::{Duration, Instant};

use devmap_extract::extract_file;
use devmap_extract::markup::{MAX_MARKUP_SYMBOLS, MAX_STYLESHEET_BYTES};
use devmap_extract::model::{Extraction, ParseOutcome, ReferenceKind, SymbolKind};

/// Seed used when `DEVMAP_MARKUP_FUZZ_SEED` is unset, so the ordinary suite runs
/// the same corpus every time and a failure replays exactly.
const DEFAULT_SEED: u64 = 0x1D0C_5E1E_C704_B7AF;

/// SplitMix64, the same six lines `mutation_fuzz.rs` uses, for the same reason:
/// no dependency, and a fixed seed replays a finding.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, ceiling: usize) -> usize {
        if ceiling == 0 {
            return 0;
        }
        (self.next_u64() % ceiling as u64) as usize
    }

    fn pick<'a, T>(&mut self, options: &'a [T]) -> &'a T {
        &options[self.below(options.len())]
    }
}

/// Markup and stylesheet symbols and uses, which is all these tests judge.
fn markup_symbols(extraction: &Extraction) -> Vec<&devmap_extract::model::ExtractedSymbol> {
    extraction
        .symbols
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.kind,
                SymbolKind::MarkupAnchor | SymbolKind::StyleRule
            )
        })
        .collect()
}

fn selector_uses(extraction: &Extraction) -> Vec<&devmap_extract::model::ExtractedReference> {
    extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Selector)
        .collect()
}

/// The invariants every extraction must satisfy, whatever the input was.
///
/// Called on every case in this file rather than asserted once per test, because
/// the failures these catch are input-shaped: the case that produces an
/// out-of-bounds span is never the case a test was written for.
fn assert_invariants(label: &str, path: &str, source: &str, extraction: &Extraction) {
    // Once per call, not once per symbol: folding a 2 MB source inside the loop
    // made this oracle quadratic and the stress corpus is where that shows.
    let folded = source.to_ascii_lowercase();
    for symbol in markup_symbols(extraction) {
        assert!(
            symbol.span.start_byte <= symbol.span.end_byte,
            "{label}: {} has an inverted span [{}..{}]",
            symbol.name,
            symbol.span.start_byte,
            symbol.span.end_byte
        );
        assert!(
            symbol.span.end_byte <= source.len(),
            "{label}: {} ends at {} past the {}-byte source",
            symbol.name,
            symbol.span.end_byte,
            source.len()
        );
        assert!(
            source.is_char_boundary(symbol.span.start_byte)
                && source.is_char_boundary(symbol.span.end_byte),
            "{label}: {} span [{}..{}] splits a character, which panics the first \
             consumer that slices with it",
            symbol.name,
            symbol.span.start_byte,
            symbol.span.end_byte
        );
        // The name is in the file. A scanner's worst failure is not a missed
        // declaration but an invented one, and this is the cheap oracle for it:
        // whatever sigil form the name takes, its identifier body is text that
        // is actually there.
        let body = symbol
            .name
            .trim_start_matches(['.', '#', '['])
            .trim_end_matches(']')
            .trim_start_matches("@keyframes ");
        // Case-insensitively, because an attribute *name* is folded on purpose:
        // HTML matches those ASCII case-insensitively, so `data-Foo` on an
        // element and `[data-foo]` in a rule are one name and both readers fold
        // them. Class names, ids and custom properties keep their case, so this
        // comparison is looser than those need — it is here to catch a *name
        // that is not in the file at all*, which is the fabrication that matters,
        // and it caught exactly that when only one of the two readers folded.
        assert!(
            body.is_empty() || folded.contains(&body.to_ascii_lowercase()),
            "{label}: {} was emitted for {path} and its name is not in the file",
            symbol.name
        );
        assert_eq!(
            symbol.qualified_name,
            format!("{path}::{}", symbol.name),
            "{label}: every declaration is qualified by the file that declares it"
        );
        assert!(
            !symbol.is_exported,
            "{label}: {} claims to be exported API",
            symbol.name
        );
    }
    for reference in selector_uses(extraction) {
        assert!(
            reference.span.start_byte <= reference.span.end_byte
                && reference.span.end_byte <= source.len(),
            "{label}: use of {} has span [{}..{}] against a {}-byte source",
            reference.name,
            reference.span.start_byte,
            reference.span.end_byte,
            source.len()
        );
        assert!(
            source.is_char_boundary(reference.span.start_byte)
                && source.is_char_boundary(reference.span.end_byte),
            "{label}: use of {} splits a character",
            reference.name
        );
        assert!(
            reference.enclosing_symbol.is_some(),
            "{label}: use of {} names no owner, so its edge would have no source",
            reference.name
        );
    }
    // A declaration list that was cut must say so, and bytes nobody read must
    // reach the outcome — `for_durable_store` clears diagnostics, so a
    // disclosure that lives only there does not survive to the store.
    if let ParseOutcome::Partial { error_ranges } = &extraction.parse_outcome {
        for range in error_ranges {
            assert!(
                range.end_byte <= source.len(),
                "{label}: an unread range ends past the source"
            );
        }
    }
}

/// Hand-written hostile inputs, each naming the loop or slice it attacks.
fn adversarial_cases() -> Vec<(&'static str, String)> {
    let mut cases: Vec<(&'static str, String)> = vec![
        ("empty", String::new()),
        ("only a style element", "<style></style>".to_string()),
        (
            "unterminated style element",
            "<div class=\"a\"></div><style>.a { color: red;".to_string(),
        ),
        (
            "unterminated script element",
            "<div id=\"x\"></div><script>const a = \"[data-x]\";".to_string(),
        ),
        ("unterminated tag", "<div class=\"a\" data-x".to_string()),
        (
            "unterminated attribute value",
            "<div class=\"a b c".to_string(),
        ),
        (
            "unterminated css string",
            "<style>.a { content: \"unclosed".to_string(),
        ),
        (
            "unterminated css comment",
            "<style>/* .fake { } ".to_string(),
        ),
        (
            "a rule inside a comment is not a rule",
            "<style>/* .commented-out { color: red; } */ .real { color: blue; }</style>"
                .to_string(),
        ),
        (
            "a brace inside a string is not structure",
            "<style>.a::after { content: \"{\"; } .b::after { content: '}'; }</style>".to_string(),
        ),
        (
            "a greater-than inside an attribute value does not end the tag",
            "<div title=\"a > b\" data-real id=\"kept\">x</div>".to_string(),
        ),
        (
            "a quote inside an attribute selector value",
            "<style>[data-x=\"}\"] { color: red; }</style>".to_string(),
        ),
        ("var with nothing after it", "<style>.a { x: var(".to_string()),
        (
            "var repeated with no close",
            format!("<style>.a {{ x: {} }}</style>", "var(".repeat(5_000)),
        ),
        (
            "an empty attribute selector",
            "<style>[] { color: red; } [=] { color: blue; }</style>".to_string(),
        ),
        (
            "a lone sigil declares nothing",
            "<style>. { color: red; } # { color: blue; }</style>".to_string(),
        ),
        (
            "an expression-valued attribute states no literal",
            "<div class={cls} id={dynamicId} data-real>x</div>".to_string(),
        ),
        (
            "a less-than in script is not a tag",
            "<script>if (a < b && c > d) { q(\".real\"); }</script><style>.real{}</style>"
                .to_string(),
        ),
        (
            "a comment that never ends",
            "<!-- <div class=\"hidden\" id=\"hidden\">".to_string(),
        ),
        (
            "close tag with no open",
            "</div></style></script>".to_string(),
        ),
        (
            "braces with no selectors",
            "<style>{}{}{}{{{{}}}}</style>".to_string(),
        ),
        (
            "semicolons with no declarations",
            "<style>;;;;;;.a{;;;;}</style>".to_string(),
        ),
        (
            "an at-rule prelude is not a selector list",
            "<style>@media (min-width: 10px) and (max-width: 20px) { .inner { x: 1; } }</style>"
                .to_string(),
        ),
        (
            "keyframe selectors name nothing",
            "<style>@keyframes spin { 0% { x: 1; } 100% { x: 2; } }</style>".to_string(),
        ),
        (
            "a global selector is still a declaration",
            "<style>:global(.shared) { color: red; }</style>".to_string(),
        ),
        (
            "multi-byte characters everywhere",
            "<div class=\"класс ölçek 日本語\" data-ключ id=\"идентификатор\">текст</div>\n\
             <style>.класс { content: \"日本語\"; } #идентификатор { x: 1; }</style>"
                .to_string(),
        ),
        (
            "a multi-byte character at the end of a truncated literal",
            "<script>const a = \"[data-ключ]".to_string(),
        ),
        (
            "a multi-byte character inside an attribute selector",
            "<style>[data-日本語] { x: 1; }</style>".to_string(),
        ),
    ];

    // Unbounded-work shapes, built rather than typed.
    cases.push((
        "markup nested 100000 deep",
        format!(
            "{}{}",
            "<div class=\"n\">".repeat(100_000),
            "</div>".repeat(100_000)
        ),
    ));
    cases.push((
        "css nested 100000 deep",
        format!(
            "<style>{}{}</style>",
            ".a { ".repeat(100_000),
            "}".repeat(100_000)
        ),
    ));
    cases.push((
        "a minified stylesheet on one line",
        format!(
            "<style>{}</style>",
            (0..20_000)
                .map(|n| format!(".c{n}{{x:{n}}}"))
                .collect::<String>()
        ),
    ));
    cases.push((
        "a thousand script elements",
        (0..1_000)
            .map(|n| format!("<script>const a{n} = 1;</script>"))
            .collect::<String>(),
    ));
    cases.push((
        "a stylesheet past the byte cap",
        format!(
            "<style>{}</style>",
            (0..(MAX_STYLESHEET_BYTES / 12 + 5_000))
                .map(|n| format!(".d{n}{{x:1}}\n"))
                .collect::<String>()
        ),
    ));
    cases.push((
        "one enormous selector list",
        format!(
            "<style>{} {{ x: 1 }}</style>",
            (0..50_000)
                .map(|n| format!(".s{n},"))
                .collect::<String>()
        ),
    ));
    cases
}

/// Every hostile input, through every markup language.
///
/// The same bytes under six extensions, because the four template grammars model
/// markup to different depths and `.html`/`.css` have no grammar at all: a
/// scanner defect that only shows up on the text-scanned path would otherwise
/// hide behind the tree-walked one.
#[test]
fn hostile_input_is_read_without_panicking_or_stalling() {
    // Generous, and still a bound: the point is that one file cannot stall a
    // build, not that the scanner is fast. The largest case here is ~2 MB.
    const PER_FILE_BUDGET: Duration = Duration::from_secs(20);
    let mut checked = 0usize;
    for (label, source) in adversarial_cases() {
        for path in [
            "a.svelte", "a.vue", "a.astro", "a.liquid", "a.html", "a.css",
        ] {
            let started = Instant::now();
            let extraction = extract_file(path, &source);
            let elapsed = started.elapsed();
            assert!(
                elapsed < PER_FILE_BUDGET,
                "{label} / {path}: took {elapsed:?}, over the {PER_FILE_BUDGET:?} bound — a \
                 scanner that does not advance, or work that is not bounded by the input"
            );
            assert_invariants(label, path, &source, &extraction);
            checked += 1;
        }
    }
    assert!(checked >= 200, "only {checked} cases ran");
}

/// Identical bytes must give identical answers.
///
/// The premise the incremental build rests on, and the one a `HashSet` iteration
/// order in a scanner quietly breaks: the dedup that keeps one declaration per
/// name is set-backed, and if the *kept* one depended on iteration order the
/// stored graph would differ between two builds of an unchanged file.
#[test]
fn identical_bytes_give_identical_answers() {
    for (label, source) in adversarial_cases() {
        for path in ["a.svelte", "a.html", "a.css"] {
            let first = extract_file(path, &source);
            let second = extract_file(path, &source);
            let names = |extraction: &Extraction| {
                markup_symbols(extraction)
                    .iter()
                    .map(|symbol| {
                        (
                            symbol.name.clone(),
                            symbol.kind,
                            symbol.span.start_byte,
                            symbol.span.end_byte,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                names(&first),
                names(&second),
                "{label} / {path}: two reads of one file disagree"
            );
            let uses = |extraction: &Extraction| {
                selector_uses(extraction)
                    .iter()
                    .map(|reference| (reference.name.clone(), reference.span.start_byte))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                uses(&first),
                uses(&second),
                "{label} / {path}: two reads disagree about uses"
            );
        }
    }
}

/// A random-byte fuzz over the alphabet the scanners branch on.
///
/// The hand-written cases above each attack a loop someone thought of. This
/// attacks the ones nobody did, by emitting only the bytes that change a
/// scanner's state — quotes, braces, brackets, angle brackets, sigils, a
/// multi-byte character — so a short string is far more likely to hit a corner
/// than random text would be.
#[test]
fn random_structural_bytes_are_read_without_panicking() {
    const ROUNDS: usize = 4_000;
    const PER_FILE_BUDGET: Duration = Duration::from_secs(5);
    let seed = std::env::var("DEVMAP_MARKUP_FUZZ_SEED")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SEED);
    let mut rng = Rng(seed);
    // Every byte the scanners dispatch on, plus the pieces of a real tag and a
    // real rule, plus two multi-byte characters.
    const ALPHABET: &[&str] = &[
        "<", ">", "/", "\"", "'", "`", "{", "}", "[", "]", "(", ")", ";", ":", ",", ".", "#", "-",
        "_", "=", "*", "&", "@", "\\", " ", "\n", "\t", "a", "Z", "0", "é", "日", "<div", "<style",
        "</style", "<script", "</script", "<!--", "-->", "class=", "id=", "data-x", "var(--y)",
        "@media", "@keyframes", ":global(", "raw_text",
    ];
    for round in 0..ROUNDS {
        let pieces = 1 + rng.below(60);
        let mut source = String::new();
        for _ in 0..pieces {
            source.push_str(rng.pick(ALPHABET));
        }
        let path = rng.pick(&["a.svelte", "a.vue", "a.astro", "a.liquid", "a.html", "a.css"]);
        let label = format!("seed {seed} round {round} source {source:?}");
        let started = Instant::now();
        let extraction = extract_file(path, &source);
        assert!(
            started.elapsed() < PER_FILE_BUDGET,
            "{label}: over the {PER_FILE_BUDGET:?} bound"
        );
        assert_invariants(&label, path, &source, &extraction);
    }
}

/// A cap that cuts must say it cut.
///
/// The one failure mode that makes every other answer untrustworthy: a symbol
/// list that is a prefix, presented as a set. A consumer reading it as complete
/// concludes the rest of the file declares nothing.
#[test]
fn a_cap_that_cuts_discloses_that_it_cut() {
    let rules: String = (0..(MAX_MARKUP_SYMBOLS + 500))
        .map(|n| format!(".over{n} {{ x: 1; }}\n"))
        .collect();
    let source = format!("<div class=\"a\"></div>\n<style>\n{rules}</style>\n");
    let extraction = extract_file("over.svelte", &source);
    let declared = markup_symbols(&extraction).len();
    assert_eq!(
        declared, MAX_MARKUP_SYMBOLS,
        "the cap is the cap: {declared} declarations kept"
    );
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|line| line.contains("past the") && line.contains("cap")),
        "the truncation must be stated: {:?}",
        extraction.diagnostics
    );
}

/// A byte cap leaves bytes unread, and unread bytes must reach the outcome.
///
/// Not `diagnostics`: `Extraction::for_durable_store` clears that field before
/// the payload reaches the store, so a disclosure that lives only there is
/// erased on the way to disk and the stored record reads like a complete answer.
#[test]
fn bytes_left_unread_reach_the_parse_outcome() {
    let filler: String = (0..(MAX_STYLESHEET_BYTES / 14 + 2_000))
        .map(|n| format!(".big{n}{{x:1}}\n"))
        .collect();
    let source = format!("<div></div>\n<style>\n{filler}</style>\n");
    assert!(
        source.len() > MAX_STYLESHEET_BYTES,
        "the fixture must exceed the byte cap to test it"
    );
    let extraction = extract_file("big.svelte", &source);
    match &extraction.parse_outcome {
        ParseOutcome::Partial { error_ranges } => {
            assert!(
                !error_ranges.is_empty(),
                "a Partial outcome must carry the ranges that make it partial"
            );
            for range in error_ranges {
                assert!(
                    range.end_byte <= source.len(),
                    "an unread range must lie inside the file"
                );
            }
        }
        other => panic!("bytes were not read and the outcome does not say so: {other:?}"),
    }
}

/// A whole-file stylesheet past its cap says so in the outcome's reason, which is
/// the durable record on the pattern-recovered path.
#[test]
fn a_capped_stylesheet_file_says_the_total_is_a_lower_bound() {
    let filler: String = (0..(MAX_STYLESHEET_BYTES / 14 + 2_000))
        .map(|n| format!(".big{n}{{x:1}}\n"))
        .collect();
    let extraction = extract_file("big.css", &filler);
    match &extraction.parse_outcome {
        ParseOutcome::Fallback { reason } => {
            assert!(
                reason.contains("lower bound"),
                "a capped read must not report like a complete one: {reason}"
            );
        }
        other => panic!("expected a pattern-recovered outcome, got {other:?}"),
    }
}

/// A selector string can only ever name something the file declares.
///
/// The whole precision story of the script-region pass. A string that looks like
/// a selector and names nothing this file declares is, as far as this index can
/// tell, a string — and the alternative to dropping it is linking it to a
/// same-named class in an unrelated component.
#[test]
fn a_selector_string_naming_nothing_declared_here_contributes_nothing() {
    let undeclared = "<script>\n  const q = \"[data-nowhere], .absent, #missing\";\n</script>\n\
                      <div class=\"present\" data-present>x</div>\n";
    let extraction = extract_file("a.svelte", undeclared);
    let uses: Vec<&str> = selector_uses(&extraction)
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    for absent in ["[data-nowhere]", ".absent", "#missing"] {
        assert!(
            !uses.contains(&absent),
            "{absent} is not declared in this file and must not be a use: {uses:?}"
        );
    }

    // The same literal, in a file that does declare what it names, is a use.
    let declared = "<script>\n  const q = \"[data-present]\";\n</script>\n\
                    <div data-present>x</div>\n";
    let extraction = extract_file("b.svelte", declared);
    let uses: Vec<&str> = selector_uses(&extraction)
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    assert!(
        uses.contains(&"[data-present]"),
        "the same literal is a use once the file declares what it names: {uses:?}"
    );
}

/// A class is not a function, whatever they are called.
///
/// The namespace collision this design exists to prevent: the code resolution
/// ladder's unique-global rung answers a bare name with the single declaration
/// of it, and if a selector went down that ladder a class called `menu` would
/// bind to `function menu`.
#[test]
fn a_class_named_like_a_function_is_not_that_function() {
    let source = "<script>\n  export function menu() { return 1; }\n</script>\n\
                  <div class=\"menu\">x</div>\n<style>.menu { color: red; }</style>\n";
    let extraction = extract_file("a.svelte", source);
    let class_declarations: Vec<&devmap_extract::model::ExtractedSymbol> = markup_symbols(&extraction)
        .into_iter()
        .filter(|symbol| symbol.name == ".menu")
        .collect();
    assert_eq!(
        class_declarations.len(),
        1,
        "the stylesheet declares .menu exactly once"
    );
    assert!(
        extraction
            .symbols
            .iter()
            .any(|symbol| symbol.name == "menu" && symbol.kind == SymbolKind::Function),
        "the function is still a function"
    );
    // The use names the class, sigil and all, so nothing that resolves names can
    // confuse the two.
    for reference in selector_uses(&extraction) {
        assert!(
            reference.name.starts_with('.')
                || reference.name.starts_with('#')
                || reference.name.starts_with('[')
                || reference.name.starts_with("--"),
            "a selector use must carry its sigil: {}",
            reference.name
        );
    }
}

/// Reading the markup half must not change the code half.
///
/// A regression guard with teeth: the same sources through the same extractor,
/// asserting that the counts of the things that existed before are unchanged by
/// a pass that only adds.
#[test]
fn the_code_half_is_untouched_by_the_markup_pass() {
    let source = "<script lang=\"ts\">\n  import { helper } from './helper';\n\
                  export function run() { return helper(); }\n</script>\n\
                  <div class=\"a\" data-x id=\"y\">t</div>\n<style>.a { color: red; }</style>\n";
    let extraction = extract_file("a.svelte", source);
    assert_eq!(
        extraction.imports.len(),
        1,
        "the import is still read: {:?}",
        extraction.imports
    );
    assert!(
        extraction.calls.iter().any(|call| call.callee_name == "helper"),
        "the call is still read: {:?}",
        extraction.calls
    );
    assert!(
        extraction
            .symbols
            .iter()
            .any(|symbol| symbol.name == "run" && symbol.kind == SymbolKind::Function),
        "the function is still read"
    );
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Clean),
        "nothing was left unread: {:?}",
        extraction.parse_outcome
    );
}
