//! A seeded mutation fuzzer over real source files.
//!
//! `adversarial_corpus.rs` asks every grammar the same fourteen hostile
//! questions. This file asks a different one: what happens when *real* code —
//! the kind every one of these grammars parses cleanly all day — is damaged
//! slightly. That is the shape a half-saved editor buffer, a truncated `git
//! checkout`, a CRLF round trip through a Windows checkout, or a corrupted
//! network filesystem actually produces, and it is the input a grammar has the
//! most opportunity to be confidently wrong about: enough structure survives
//! for a parse to succeed, and the result is a symbol set nothing verified.
//!
//! The invariants asserted are the ones a consumer acts on, not the ones that
//! happen to hold today:
//!
//! * the extractor returns — a panic here takes a whole `devmap build` with it;
//! * it returns inside twice the production parse budget, so one damaged file
//!   cannot stall the build behind an unbounded parse;
//! * a `Partial` outcome carries the error ranges that make it partial, and a
//!   `Failed`/`Fallback` outcome carries a reason a reader can act on;
//! * every symbol span lies inside the source *and* on character boundaries —
//!   a span that splits a multi-byte character panics the first consumer that
//!   slices with it;
//! * a file whose parse failed or fell back to pattern matching contributes
//!   **zero** call edges, because a call recovered from a structure nobody
//!   parsed is a fabricated edge, and `analyze_liveness` cannot tell one from a
//!   real one;
//! * two extractions of identical bytes are byte-identical, which is the
//!   premise the whole incremental build rests on.
//!
//! Seeded, and the seed is printed on every run and on every failure, so a
//! finding replays exactly. The default corpus is this repository and the
//! default round count is small enough to belong in the ordinary suite;
//! `DEVMAP_FUZZ_*` widens both for a long run without editing the test.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devmap_extract::languages::LANGUAGE_SPECS;
use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_extract::treesitter::DEFAULT_PARSE_BUDGET;
use devmap_extract::{collect_sources_with_report, detect_language, extract_file};

/// Files per language in the default run.
const DEFAULT_FILES_PER_LANGUAGE: usize = 2;
/// Mutated variants generated per file in the default run.
const DEFAULT_ROUNDS: usize = 3;
/// Seed used when `DEVMAP_FUZZ_SEED` is unset, so the ordinary suite runs the
/// same cases every time and a regression is not a coin flip.
const DEFAULT_SEED: u64 = 0x5EED_1A2B_3C4D_5E6F;

/// The wall-clock ceiling one extraction may take.
///
/// Twice the production budget, not equal to it: the budget bounds the
/// *parser*, and the surrounding work — language detection, the declaration
/// walk, wiring annotation, the fallback scan — is outside it. A file that
/// takes longer than this has an unbounded phase somewhere, which is the defect
/// worth finding.
fn time_ceiling() -> Duration {
    DEFAULT_PARSE_BUDGET * 2
}

fn env_usize(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

/// SplitMix64. Six lines, no dependency, and a fixed seed replays a failure
/// exactly — which is the only property a fuzzer's generator needs here.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-enough in `[0, bound)`; `0` for an empty range.
    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }
}

/// The worktree this crate lives in.
fn repo_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while dir.parent().is_some() {
        if dir.join(".git").exists() {
            return dir;
        }
        dir.pop();
    }
    // No git metadata (a vendored copy, a tarball build): the workspace root is
    // still a real corpus of Rust, and saying so beats failing the suite.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Real source files grouped by language, capped per language.
///
/// Reuses `collect_sources_with_report` rather than walking by hand, so the
/// corpus obeys the same ignore rules, the same size ceiling and the same
/// ordering the build does. A fuzzer fed by a second discovery implementation
/// would be fuzzing inputs production never sees.
fn corpus(files_per_language: usize) -> BTreeMap<String, Vec<(String, String)>> {
    let mut roots = vec![repo_root()];
    if let Ok(extra) = std::env::var("DEVMAP_FUZZ_CORPUS") {
        for entry in extra.split(':').filter(|value| !value.is_empty()) {
            roots.push(PathBuf::from(entry));
        }
    }

    let mut by_language: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for root in roots {
        let Ok((sources, _report)) = collect_sources_with_report(&root) else {
            continue;
        };
        for (path, source) in sources {
            let language = detect_language(Path::new(&path)).to_string();
            let bucket = by_language.entry(language).or_default();
            if bucket.len() < files_per_language {
                bucket.push((path, source));
            }
        }
    }
    by_language
}

/// One damaging edit, named by the failure mode it is probing.
#[derive(Debug, Clone, Copy)]
enum Mutation {
    /// A half-written file: the tail is simply gone.
    Truncate,
    /// A botched merge or an editor's duplicate-line keystroke.
    DuplicateLine,
    /// A single character lost — most interestingly a delimiter.
    DropChar,
    /// The delimiter case on purpose, rather than by luck.
    DropDelimiter,
    /// A binary byte in a text file: the extractor refuses these outright, and
    /// the refusal is part of the contract.
    InsertNul,
    /// A UTF-8 BOM a Windows editor added, in the middle as well as the front.
    InsertBom,
    /// A checkout with `core.autocrlf` on.
    ToCrlf,
    /// A right-to-left override, which reorders how the source *displays*
    /// without changing what it says — the Trojan Source shape.
    RtlOverride,
}

const MUTATIONS: [Mutation; 8] = [
    Mutation::Truncate,
    Mutation::DuplicateLine,
    Mutation::DropChar,
    Mutation::DropDelimiter,
    Mutation::InsertNul,
    Mutation::InsertBom,
    Mutation::ToCrlf,
    Mutation::RtlOverride,
];

/// A byte offset that is a character boundary at or before `at`.
fn boundary(source: &str, at: usize) -> usize {
    let mut index = at.min(source.len());
    while index > 0 && !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn apply(mutation: Mutation, source: &str, rng: &mut Rng) -> String {
    match mutation {
        Mutation::Truncate => {
            let cut = boundary(source, rng.below(source.len().max(1)));
            source[..cut].to_string()
        }
        Mutation::DuplicateLine => {
            let lines: Vec<&str> = source.lines().collect();
            if lines.is_empty() {
                return source.to_string();
            }
            let index = rng.below(lines.len());
            let mut out = String::with_capacity(source.len() + lines[index].len() + 1);
            for (position, line) in lines.iter().enumerate() {
                out.push_str(line);
                out.push('\n');
                if position == index {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            out
        }
        Mutation::DropChar => {
            if source.is_empty() {
                return String::new();
            }
            let start = boundary(source, rng.below(source.len()));
            let end = source[start..]
                .chars()
                .next()
                .map(|character| start + character.len_utf8())
                .unwrap_or(start);
            let mut out = String::with_capacity(source.len());
            out.push_str(&source[..start]);
            out.push_str(&source[end..]);
            out
        }
        Mutation::DropDelimiter => {
            let positions: Vec<usize> = source
                .char_indices()
                .filter(|(_, character)| matches!(character, '{' | '}' | '(' | ')' | '[' | ']'))
                .map(|(index, _)| index)
                .collect();
            if positions.is_empty() {
                return source.to_string();
            }
            let at = positions[rng.below(positions.len())];
            let mut out = String::with_capacity(source.len());
            out.push_str(&source[..at]);
            out.push_str(&source[at + 1..]);
            out
        }
        Mutation::InsertNul => {
            let at = boundary(source, rng.below(source.len().max(1)));
            format!("{}\0{}", &source[..at], &source[at..])
        }
        Mutation::InsertBom => {
            let at = boundary(source, rng.below(source.len().max(1)));
            format!("{}\u{feff}{}", &source[..at], &source[at..])
        }
        Mutation::ToCrlf => source.replace('\n', "\r\n"),
        Mutation::RtlOverride => {
            let at = boundary(source, rng.below(source.len().max(1)));
            format!("{}\u{202e}{}", &source[..at], &source[at..])
        }
    }
}

/// Everything a consumer is entitled to assume about an extraction, checked.
fn assert_invariants(label: &str, path: &str, source: &str, extraction: &Extraction) {
    for symbol in &extraction.symbols {
        assert!(
            symbol.span.start_byte <= symbol.span.end_byte,
            "{label}: inverted span on {:?}",
            symbol.qualified_name
        );
        assert!(
            symbol.span.end_byte <= source.len(),
            "{label}: span {}..{} escapes a {}-byte source on {:?}",
            symbol.span.start_byte,
            symbol.span.end_byte,
            source.len(),
            symbol.qualified_name
        );
        // A span that splits a multi-byte character panics the first consumer
        // that slices the source with it — `devmap search`'s snippet, the
        // preview diff, and `byte_span_to_line_range` all do.
        assert!(
            source.is_char_boundary(symbol.span.start_byte)
                && source.is_char_boundary(symbol.span.end_byte),
            "{label}: span {}..{} splits a character on {:?}",
            symbol.span.start_byte,
            symbol.span.end_byte,
            symbol.qualified_name
        );
        assert!(
            !symbol.name.is_empty(),
            "{label}: a symbol with no name cannot be addressed"
        );
    }

    match &extraction.parse_outcome {
        ParseOutcome::Clean => {}
        ParseOutcome::Partial { error_ranges } => {
            assert!(
                !error_ranges.is_empty(),
                "{label}: `Partial` with no error ranges is `Clean` wearing a \
                 warning label — a consumer reading `overlaps_parse_error` gets \
                 an exemption nothing justifies"
            );
            for range in error_ranges {
                assert!(
                    range.start_byte <= range.end_byte && range.end_byte <= source.len(),
                    "{label}: error range {}..{} escapes a {}-byte source",
                    range.start_byte,
                    range.end_byte,
                    source.len()
                );
            }
        }
        // `Skipped` is here because this suite is where the defect that
        // introduced it was found: a minified bundle in the corpus straddled
        // the parse budget, so two extractions of identical bytes disagreed and
        // the determinism assertion above fired. The bundle is now declined
        // before the grammar runs, and a declined file owes the same debt as
        // any other non-clean outcome — it must say why.
        ParseOutcome::Failed { reason }
        | ParseOutcome::Fallback { reason }
        | ParseOutcome::Skipped { reason } => {
            assert!(
                !reason.trim().is_empty(),
                "{label}: a non-clean outcome with no reason is unactionable"
            );
            // The fabrication rule. `Failed` means nothing parsed the file and
            // `Fallback` means a line scanner matched names in it; neither tier
            // saw a call expression, so a call edge here was invented, and
            // nothing downstream can tell it from a parsed one.
            assert!(
                extraction.calls.is_empty(),
                "{label}: {} call edge(s) from a file that was never parsed: {:?}",
                extraction.calls.len(),
                extraction
                    .calls
                    .iter()
                    .map(|call| call.callee_name.as_str())
                    .take(5)
                    .collect::<Vec<_>>()
            );
        }
    }

    assert_eq!(
        extraction.file_path, path,
        "{label}: the path must survive extraction unmodified"
    );
}

/// Extract, timed and bounded, asserting the whole invariant set.
fn probe(label: &str, path: &str, source: &str) -> Extraction {
    let started = Instant::now();
    let extraction = extract_file(path, source);
    let elapsed = started.elapsed();
    assert!(
        elapsed <= time_ceiling(),
        "{label}: extraction took {elapsed:?}, past the {:?} ceiling — some phase \
         of this path is not bounded by the parse budget",
        time_ceiling()
    );
    assert_invariants(label, path, source, &extraction);
    extraction
}

/// The sweep: real files, damaged, across every language this build can find
/// source for.
#[test]
fn mutated_real_sources_never_panic_fabricate_or_drift() {
    let seed = std::env::var("DEVMAP_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    let files_per_language = env_usize("DEVMAP_FUZZ_FILES", DEFAULT_FILES_PER_LANGUAGE);
    let rounds = env_usize("DEVMAP_FUZZ_ROUNDS", DEFAULT_ROUNDS);
    let mut rng = Rng(seed);

    let corpus = corpus(files_per_language);
    // Both numbers, always. The corpus is whatever source this checkout
    // happens to contain, so "every linked grammar" is a claim this run is
    // usually not entitled to make — reporting only the covered count would be
    // presenting a capped sample as complete coverage.
    //
    // Keyed by `spec.grammar`, which is exactly what `detect_language` returns
    // and what the corpus map is keyed by. `spec.name` is the display label
    // ("Pascal/Delphi"), and comparing the two reported 41 of 35 languages
    // covered with every real one listed as missing.
    let linked: std::collections::BTreeSet<&str> =
        LANGUAGE_SPECS.iter().map(|spec| spec.grammar).collect();
    let covered = linked
        .iter()
        .filter(|grammar| corpus.contains_key(**grammar))
        .count();
    let uncovered: Vec<&str> = linked
        .iter()
        .filter(|grammar| !corpus.contains_key(**grammar))
        .copied()
        .collect();
    eprintln!(
        "mutation fuzz: seed {seed}, {files_per_language} file(s) x {rounds} round(s) per \
         language, {covered} of {} linked grammars have source in this corpus; \
         {} non-grammar language(s) also exercised (uncovered grammars: {uncovered:?})",
        linked.len(),
        corpus.len() - covered
    );
    assert!(
        covered >= 5,
        "the corpus found source for only {covered} language(s); this run proves \
         nothing. Point DEVMAP_FUZZ_CORPUS at a real tree."
    );

    let mut cases = 0usize;
    for (language, files) in &corpus {
        for (path, source) in files {
            for round in 0..rounds {
                // One to eight edits, so a single-character defect and a
                // thoroughly mangled file are both in range.
                let edits = 1 + rng.below(8);
                let mut mutated = source.clone();
                let mut applied = Vec::new();
                for _ in 0..edits {
                    let mutation = MUTATIONS[rng.below(MUTATIONS.len())];
                    applied.push(mutation);
                    mutated = apply(mutation, &mutated, &mut rng);
                }
                let label =
                    format!("seed {seed} / {language} / {path} / round {round} / {applied:?}");
                let first = probe(&label, path, &mutated);
                // Determinism, on the damaged input rather than the clean one:
                // an unstable iteration order or a hash-seeded map shows itself
                // where the parse is ragged, not where it is tidy, and the
                // incremental build's whole premise is that identical bytes
                // give an identical graph.
                let second = extract_file(path, &mutated);
                assert_eq!(
                    serde_json::to_string(&first).unwrap(),
                    serde_json::to_string(&second).unwrap(),
                    "{label}: two extractions of identical bytes disagree"
                );
                cases += 1;
            }
        }
    }

    assert_eq!(
        cases,
        corpus.values().map(Vec::len).sum::<usize>() * rounds,
        "every corpus file must be exercised for every round"
    );
    eprintln!("mutation fuzz: {cases} mutated extractions checked");
}

/// The pathological *shapes*, which are too expensive to generate per file.
///
/// Each is a real report from somewhere: a generated parser table nested past
/// any sane depth, a log file committed as source, and a file sitting exactly on
/// the discovery ceiling — the boundary the size check is written around, and
/// therefore the one an off-by-one lands on.
#[test]
fn pathological_shapes_stay_bounded_and_honest() {
    // 10k-deep nesting, in a language whose grammar is recursive descent.
    let deep = format!("{}{}", "{".repeat(10_000), "}".repeat(10_000));
    probe("10k nesting / rust", "deep.rs", &deep);
    probe("10k nesting / python", "deep.py", &deep);
    probe("10k nesting / typescript", "deep.ts", &deep);

    // A million lines. Small per line, so this is about the per-line work, not
    // about parser depth.
    let many_lines = "x = 1\n".repeat(1_000_000);
    probe("1M lines / python", "wide.py", &many_lines);

    // Exactly at the discovery ceiling: admitted by `collect_sources_with_report`
    // (the limit is exclusive), so it is a file the extractor really does see.
    let at_ceiling = "# padding padding padding padding padding padding padding\n"
        .repeat((devmap_extract::MAX_SOURCE_BYTES as usize / 58) + 1);
    let at_ceiling: String = at_ceiling
        .chars()
        .take(devmap_extract::MAX_SOURCE_BYTES as usize)
        .collect();
    assert_eq!(
        at_ceiling.len(),
        devmap_extract::MAX_SOURCE_BYTES as usize,
        "the fixture must sit exactly on the ceiling"
    );
    probe(
        "exactly MAX_SOURCE_BYTES / python",
        "ceiling.py",
        &at_ceiling,
    );
}
