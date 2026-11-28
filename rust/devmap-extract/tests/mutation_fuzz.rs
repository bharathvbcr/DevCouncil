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

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devmap_extract::languages::LANGUAGE_SPECS;
use devmap_extract::model::{Extraction, ParseOutcome};
use devmap_extract::treesitter::DEFAULT_PARSE_BUDGET;
use devmap_extract::{
    collect_sources_with_report, detect_language, extract_file, linked_grammar_keys,
};

#[allow(dead_code)]
mod stateful_fuzz {
    include!("../../testsupport/stateful_fuzz.rs");
}

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
    stateful_fuzz::env_usize(key, fallback)
}

fn fuzz_seed() -> u64 {
    stateful_fuzz::env_u64(
        "DEVMAP_FUZZ_SEED",
        stateful_fuzz::env_u64("DEVMAP_STATEFUL_SEED", DEFAULT_SEED),
    )
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
    if let Some(extra) = std::env::var_os("DEVMAP_FUZZ_CORPUS") {
        for entry in std::env::split_paths(&extra).filter(|path| !path.as_os_str().is_empty()) {
            roots.push(entry);
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

impl Mutation {
    fn label(self) -> &'static str {
        match self {
            Mutation::Truncate => "mutation::Truncate",
            Mutation::DuplicateLine => "mutation::DuplicateLine",
            Mutation::DropChar => "mutation::DropChar",
            Mutation::DropDelimiter => "mutation::DropDelimiter",
            Mutation::InsertNul => "mutation::InsertNul",
            Mutation::InsertBom => "mutation::InsertBom",
            Mutation::ToCrlf => "mutation::ToCrlf",
            Mutation::RtlOverride => "mutation::RtlOverride",
        }
    }
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

fn outcome_label(outcome: &ParseOutcome) -> &'static str {
    match outcome {
        ParseOutcome::Clean => "outcome::Clean",
        ParseOutcome::Partial { .. } => "outcome::Partial",
        ParseOutcome::Fallback { .. } => "outcome::Fallback",
        ParseOutcome::Failed { .. } => "outcome::Failed",
        ParseOutcome::Skipped { .. } => "outcome::Skipped",
    }
}

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

/// Everything a consumer is entitled to assume about an extraction, checked via
/// `stateful_fuzz::check` so failures carry a stable signature for minimization.
fn check_invariants(
    action: &'static str,
    label: &str,
    path: &str,
    source: &str,
    extraction: &Extraction,
) -> Result<(), stateful_fuzz::CheckFailure> {
    for symbol in &extraction.symbols {
        if let Some(failure) = stateful_fuzz::check(
            symbol.span.start_byte <= symbol.span.end_byte,
            "span_order",
            action,
            format!("{label}: inverted span on {:?}", symbol.qualified_name),
        )
        .into_failure()
        {
            return Err(failure);
        }
        if let Some(failure) = stateful_fuzz::check(
            symbol.span.end_byte <= source.len(),
            "span_bounds",
            action,
            format!(
                "{label}: span {}..{} escapes a {}-byte source on {:?}",
                symbol.span.start_byte,
                symbol.span.end_byte,
                source.len(),
                symbol.qualified_name
            ),
        )
        .into_failure()
        {
            return Err(failure);
        }
        // A span that splits a multi-byte character panics the first consumer
        // that slices the source with it — `devmap search`'s snippet, the
        // preview diff, and `byte_span_to_line_range` all do.
        if let Some(failure) = stateful_fuzz::check(
            source.is_char_boundary(symbol.span.start_byte)
                && source.is_char_boundary(symbol.span.end_byte),
            "span_char_boundary",
            action,
            format!(
                "{label}: span {}..{} splits a character on {:?}",
                symbol.span.start_byte, symbol.span.end_byte, symbol.qualified_name
            ),
        )
        .into_failure()
        {
            return Err(failure);
        }
        if let Some(failure) = stateful_fuzz::check(
            !symbol.name.is_empty(),
            "symbol_non_empty_name",
            action,
            format!("{label}: a symbol with no name cannot be addressed"),
        )
        .into_failure()
        {
            return Err(failure);
        }
    }

    for call in &extraction.calls {
        if let Some(failure) = stateful_fuzz::check(
            !call.callee_name.trim().is_empty(),
            "call_non_empty_callee",
            action,
            format!("{label}: call with empty callee_name joins on nothing"),
        )
        .into_failure()
        {
            return Err(failure);
        }
    }

    for reference in &extraction.references {
        if let Some(failure) = stateful_fuzz::check(
            !reference.name.trim().is_empty(),
            "ref_non_empty_name",
            action,
            format!("{label}: reference with empty name joins on nothing"),
        )
        .into_failure()
        {
            return Err(failure);
        }
    }

    match &extraction.parse_outcome {
        ParseOutcome::Clean => {}
        ParseOutcome::Partial { error_ranges, .. } => {
            if let Some(failure) = stateful_fuzz::check(
                !error_ranges.is_empty(),
                "partial_has_error_ranges",
                action,
                format!(
                    "{label}: `Partial` with no error ranges is `Clean` wearing a \
                     warning label — a consumer reading `overlaps_parse_error` gets \
                     an exemption nothing justifies"
                ),
            )
            .into_failure()
            {
                return Err(failure);
            }
            for range in error_ranges {
                if let Some(failure) = stateful_fuzz::check(
                    range.start_byte <= range.end_byte && range.end_byte <= source.len(),
                    "error_range_bounds",
                    action,
                    format!(
                        "{label}: error range {}..{} escapes a {}-byte source",
                        range.start_byte,
                        range.end_byte,
                        source.len()
                    ),
                )
                .into_failure()
                {
                    return Err(failure);
                }
                if let Some(failure) = stateful_fuzz::check(
                    source.is_char_boundary(range.start_byte)
                        && source.is_char_boundary(range.end_byte),
                    "error_range_char_boundary",
                    action,
                    format!(
                        "{label}: error range {}..{} splits a UTF-8 character",
                        range.start_byte, range.end_byte
                    ),
                )
                .into_failure()
                {
                    return Err(failure);
                }
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
            if let Some(failure) = stateful_fuzz::check(
                !reason.trim().is_empty(),
                "non_clean_has_reason",
                action,
                format!("{label}: a non-clean outcome with no reason is unactionable"),
            )
            .into_failure()
            {
                return Err(failure);
            }
            // The fabrication rule. `Failed` means nothing parsed the file and
            // `Fallback` means a line scanner matched names in it; neither tier
            // saw a call expression, so a call edge here was invented, and
            // nothing downstream can tell it from a parsed one.
            if let Some(failure) = stateful_fuzz::check(
                extraction.calls.is_empty(),
                "zero_fabricated_calls",
                action,
                format!(
                    "{label}: {} call edge(s) from a file that was never parsed: {:?}",
                    extraction.calls.len(),
                    extraction
                        .calls
                        .iter()
                        .map(|call| call.callee_name.as_str())
                        .take(5)
                        .collect::<Vec<_>>()
                ),
            )
            .into_failure()
            {
                return Err(failure);
            }
        }
    }

    if let Some(failure) = stateful_fuzz::check(
        extraction.file_path == path,
        "file_path_preserved",
        action,
        format!(
            "{label}: the path must survive extraction unmodified (got {:?}, expected {:?})",
            extraction.file_path, path
        ),
    )
    .into_failure()
    {
        return Err(failure);
    }

    Ok(())
}

/// Extract once, enforcing the time ceiling and full invariant set.
fn check_single_extraction(
    action: &'static str,
    label: &str,
    path: &str,
    source: &str,
) -> Result<Extraction, stateful_fuzz::CheckFailure> {
    let started = Instant::now();
    let extraction = extract_file(path, source);
    let elapsed = started.elapsed();
    if let Some(failure) = stateful_fuzz::check(
        elapsed <= time_ceiling(),
        "time_ceiling",
        action,
        format!(
            "{label}: extraction took {elapsed:?}, past the {:?} ceiling — some phase \
             of this path is not bounded by the parse budget",
            time_ceiling()
        ),
    )
    .into_failure()
    {
        return Err(failure);
    }
    check_invariants(action, label, path, source, &extraction)?;
    Ok(extraction)
}

/// Extract twice and assert both the full invariant set and byte-identical
/// determinism across identical inputs.
fn check_extraction_and_determinism(
    action: &'static str,
    label: &str,
    path: &str,
    source: &str,
) -> Result<Extraction, stateful_fuzz::CheckFailure> {
    let first = check_single_extraction(action, label, path, source)?;
    let second = extract_file(path, source);
    let first_json = serde_json::to_string(&first).unwrap();
    let second_json = serde_json::to_string(&second).unwrap();
    if let Some(failure) = stateful_fuzz::check(
        first_json == second_json,
        "determinism",
        action,
        format!("{label}: two extractions of identical bytes disagree"),
    )
    .into_failure()
    {
        return Err(failure);
    }
    Ok(first)
}

/// Minimize a failing source snippet via `stateful_fuzz::minimize_str` and panic with
/// a copy-pasteable minimal reproducer and `DEVMAP_FUZZ_SEED` replay hint.
fn fail_with_minimized_repro(
    seed: u64,
    test_name: &str,
    label: &str,
    path: &str,
    source: &str,
    check_determinism: bool,
    failure: stateful_fuzz::CheckFailure,
) -> ! {
    let sig = failure.signature();
    let max_attempts = if failure.oracle == "time_ceiling" {
        16
    } else {
        200
    };
    let minimized = stateful_fuzz::minimize_str(source, &sig, max_attempts, |cand| {
        if check_determinism {
            check_extraction_and_determinism(failure.action, "minimize", path, cand).err()
        } else {
            check_single_extraction(failure.action, "minimize", path, cand).err()
        }
    });
    let repro = stateful_fuzz::format_repro_test(
        test_name,
        seed,
        &failure,
        &[format!("extract_file({path:?}, {minimized:?})")],
    );
    panic!(
        "{label}\n\
         Failure: {failure}\n\
         Minimized source ({} -> {} bytes) for {path:?}:\n\
         {minimized:?}\n\
         Replay: DEVMAP_FUZZ_SEED={seed} cargo test --locked -p devmap-extract --test it -- mutation_fuzz:: --nocapture\n\n\
         {repro}",
        source.len(),
        minimized.len()
    );
}

/// Extract, timed and bounded, asserting the whole invariant set.
fn probe(label: &str, path: &str, source: &str) -> Extraction {
    match check_single_extraction("pathological_probe", label, path, source) {
        Ok(extraction) => extraction,
        Err(failure) => fail_with_minimized_repro(
            DEFAULT_SEED,
            "repro_pathological_shape",
            label,
            path,
            source,
            false,
            failure,
        ),
    }
}

/// The sweep: real files, damaged, across every language this build can find
/// source for.
#[test]
fn mutated_real_sources_never_panic_fabricate_or_drift() {
    let seed = fuzz_seed();
    let files_per_language = env_usize("DEVMAP_FUZZ_FILES", DEFAULT_FILES_PER_LANGUAGE);
    let rounds = env_usize("DEVMAP_FUZZ_ROUNDS", DEFAULT_ROUNDS);
    let mut rng = Rng(seed);
    let mut tracker = stateful_fuzz::HealthTracker::new();

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
    let linked: BTreeSet<&str> = LANGUAGE_SPECS.iter().map(|spec| spec.grammar).collect();
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
                let first = match check_extraction_and_determinism(
                    "mutated_real_source",
                    &label,
                    path,
                    &mutated,
                ) {
                    Ok(ext) => ext,
                    Err(failure) => {
                        tracker.record(
                            "mutated_real_source",
                            None,
                            &stateful_fuzz::StepOutcome::CheckFailed(failure.clone()),
                        );
                        fail_with_minimized_repro(
                            seed,
                            "repro_mutated_real_source",
                            &label,
                            path,
                            &mutated,
                            true,
                            failure,
                        );
                    }
                };
                tracker.record("mutated_real_source", None, &stateful_fuzz::StepOutcome::Ok);
                let mut prev = Some("mutated_real_source");
                for &mutation in &applied {
                    let m_label = mutation.label();
                    tracker.record(m_label, prev, &stateful_fuzz::StepOutcome::Ok);
                    prev = Some(m_label);
                }
                tracker.record(
                    outcome_label(&first.parse_outcome),
                    prev,
                    &stateful_fuzz::StepOutcome::Ok,
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
    eprintln!("{}", tracker.probe_report("mutation_fuzz::real_sources"));
    tracker.assert_actions_exercised(&[
        "mutated_real_source",
        "mutation::Truncate",
        "mutation::DuplicateLine",
        "mutation::DropChar",
        "mutation::DropDelimiter",
        "mutation::InsertNul",
        "mutation::InsertBom",
        "mutation::ToCrlf",
        "mutation::RtlOverride",
        "outcome::Clean",
        "outcome::Partial",
        "outcome::Failed",
    ]);
}

/// Minimal representative seed for every one of the 33 linked tree-sitter
/// grammars, so grammars absent from this repository still get full mutation,
/// `lower` damage, byte-offset, char-boundary, and determinism coverage.
const ALL_LINKED_GRAMMAR_SEEDS: &[(&str, &str, &str)] = &[
    (
        "astro",
        "seed.astro",
        "---\nconst greeting = \"こんにちは\";\nfunction render(x: number) { return x + 1; }\n---\n<div>{render(1)}</div>\n",
    ),
    (
        "c",
        "seed.c",
        "/* 日本語 */\nint helper(int x) { return x + 1; }\nint entry(int y) { return helper(y); }\n",
    ),
    (
        "cfml",
        "seed.cfm",
        "<cfset msg = \"hi\">\n<cffunction name=\"greet\"><cfreturn msg></cffunction>\n",
    ),
    (
        "cpp",
        "seed.cpp",
        "// テスト\nstruct Box { int get() const { return 1; } };\nint run(Box b) { return b.get(); }\n",
    ),
    (
        "csharp",
        "Seed.cs",
        "public class Worker { public int Step(int x) => Helper(x); private int Helper(int x) => x + 1; }\n",
    ),
    (
        "cuda",
        "seed.cu",
        "__device__ int add1(int x) { return x + 1; }\n__global__ void kernel(int *out) { out[0] = add1(1); }\n",
    ),
    (
        "dart",
        "seed.dart",
        "int helper(int x) => x + 1;\nint run(int y) { return helper(y); }\n",
    ),
    (
        "erlang",
        "seed.erl",
        "-module(seed).\n-export([run/1]).\nhelper(X) -> X + 1.\nrun(Y) -> helper(Y).\n",
    ),
    (
        "go",
        "seed.go",
        "package seed\n// 日本語\nfunc helper(x int) int { return x + 1 }\nfunc Run(y int) int { return helper(y) }\n",
    ),
    (
        "hcl",
        "main.tf",
        "variable \"env\" { default = \"dev\" }\nresource \"aws_s3_bucket\" \"logs\" { bucket = var.env }\n",
    ),
    (
        "java",
        "Seed.java",
        "public class Seed { int helper(int x) { return x + 1; } public int run(int y) { return helper(y); } }\n",
    ),
    (
        "javascript",
        "seed.js",
        "// 日本語\nfunction helper(x) { return x + 1; }\nexport function run(y) { return helper(y); }\n",
    ),
    (
        "kotlin",
        "Seed.kt",
        "class Seed { fun helper(x: Int): Int = x + 1\n    fun run(y: Int): Int = helper(y) }\n",
    ),
    (
        "liquid",
        "seed.liquid",
        "{% assign label = \"hello\" %}{% if label %}{{ label | upcase }}{% endif %}\n",
    ),
    (
        "lua",
        "seed.lua",
        "local function helper(x) return x + 1 end\nfunction run(y) return helper(y) end\n",
    ),
    (
        "luau",
        "seed.luau",
        "local function helper(x: number): number return x + 1 end\nlocal function run(y: number): number return helper(y) end\n",
    ),
    (
        "nix",
        "seed.nix",
        "let helper = x: x + 1; in { result = helper 41; }\n",
    ),
    (
        "objc",
        "seed.m",
        "@interface Greeter\n- (int)run:(int)x;\n@end\n@implementation Greeter\n- (int)run:(int)x { return x + 1; }\n@end\n",
    ),
    (
        "pascal",
        "seed.pas",
        "program Seed;\nfunction Helper(x: Integer): Integer;\nbegin\n  Helper := x + 1;\nend;\nbegin\n  Helper(1);\nend.\n",
    ),
    (
        "php",
        "seed.php",
        "<?php\nfunction helper($x) { return $x + 1; }\nfunction run($y) { return helper($y); }\n",
    ),
    (
        "python",
        "seed.py",
        "# 日本語\ndef helper(x: int) -> int:\n    return x + 1\n\ndef run(y: int) -> int:\n    return helper(y)\n",
    ),
    (
        "r",
        "seed.R",
        "helper <- function(x) { x + 1 }\nrun <- function(y) { helper(y) }\n",
    ),
    (
        "ruby",
        "seed.rb",
        "def helper(x)\n  x + 1\nend\ndef run(y)\n  helper(y)\nend\n",
    ),
    (
        "rust",
        "seed.rs",
        "// 日本語\nfn helper(x: i32) -> i32 { x + 1 }\npub fn run(y: i32) -> i32 { helper(y) }\n",
    ),
    (
        "scala",
        "Seed.scala",
        "object Seed {\n  def helper(x: Int): Int = x + 1\n  def run(y: Int): Int = helper(y)\n}\n",
    ),
    (
        "shell",
        "seed.sh",
        "#!/usr/bin/env bash\nhelper() { printf '%s' \"$1\"; }\nrun() { helper \"hi\"; }\n",
    ),
    (
        "solidity",
        "Seed.sol",
        "pragma solidity ^0.8.0;\ncontract Seed {\n    function helper(uint x) internal pure returns (uint) { return x + 1; }\n    function run(uint y) external pure returns (uint) { return helper(y); }\n}\n",
    ),
    (
        "sql",
        "seed.sql",
        "CREATE TABLE items (id INT PRIMARY KEY, name TEXT);\nSELECT id, name FROM items;\n",
    ),
    (
        "svelte",
        "Seed.svelte",
        "<script>\n  export function helper(x) { return x + 1; }\n</script>\n<p>{helper(1)}</p>\n",
    ),
    (
        "swift",
        "Seed.swift",
        "func helper(_ x: Int) -> Int { return x + 1 }\nfunc run(_ y: Int) -> Int { return helper(y) }\n",
    ),
    (
        "tsx",
        "Seed.tsx",
        "export function helper(x: number): number { return x + 1; }\nexport const View = () => <div>{helper(1)}</div>;\n",
    ),
    (
        "typescript",
        "seed.ts",
        "// 日本語\nexport function helper(x: number): number { return x + 1; }\nexport function run(y: number): number { return helper(y); }\n",
    ),
    (
        "vue",
        "Seed.vue",
        "<script setup>\nfunction helper(x) { return x + 1; }\n</script>\n<template><div>{{ helper(1) }}</div></template>\n",
    ),
];

/// Sources specifically exercising same-width `parse_repair`, `kotlinsemi`, and
/// `sveltetext` grammar-quirk repairs with multi-byte UTF-8 neighbors so any
/// byte-length drift or char-boundary skew fails loudly.
const PARSE_REPAIR_SEEDS: &[(&str, &str, &str)] = &[
    (
        "typescript_quirks",
        "quirks/ts_repair.ts",
        "// 日本語コメント 🎌\n\
         type Mod = typeof import('./store');\n\
         type Store = import('./store').InterfaceStore;\n\
         type Pair = [verb: string, object: string, number: number];\n\
         export function check(a: number, unique: { count: number }): boolean {\n\
           return a < unique.count;\n\
         }\n",
    ),
    (
        "tsx_quirks",
        "quirks/tsx_repair.tsx",
        "// マルチバイト 🎌\n\
         type Props = import('./card').CardProps;\n\
         type Tuple = [string: number, object: boolean];\n\
         export const Card = (p: Props, unique: { items: number }) => {\n\
           const ok = 0 < unique.items;\n\
           return <section>{ok ? String(p) : \"none\"}</section>;\n\
         };\n",
    ),
    (
        "rust_quirks",
        "quirks/rust_repair.rs",
        "// 日本語コメント 🎌\n\
         unsafe extern \"C\" {\n\
             fn native_hook(x: i32) -> i32;\n\
         }\n\
         struct Entry { path: String, #[cfg(unix)] ino: u64 }\n\
         pub fn inspect(raw: &str, e: Entry) -> usize {\n\
             let r = &raw;\n\
             let Entry { path, #[cfg(unix)] ino } = e;\n\
             #[cfg(unix)] let _ = ino;\n\
             r.len() + path.len()\n\
         }\n",
    ),
    (
        "shell_quirks",
        "quirks/shell_repair.sh",
        "#!/usr/bin/env bash\n\
         # 日本語コメント 🎌\n\
         set -euo pipefail\n\
         pin=\"abcdef1234567890\"\n\
         echo \"commit${pin:+ (${pin:0:12})}\"\n\
         out=\"$(printf 'hi')\"\n\
         echo \"$out\"\n",
    ),
    (
        "kotlin_quirks",
        "quirks/KotlinRepair.kt",
        "// 日本語コメント 🎌\n\
         class OneLine { val x = 1; fun get() = x }\n\
         fun top() {}\n",
    ),
    (
        "svelte_quirks",
        "quirks/SvelteRepair.svelte",
        "<!-- 日本語 🎌 -->\n\
         <script>\n\
           export function ping() { return 1; }\n\
         </script>\n\
         <div>a & b > c {ping()}</div>\n",
    ),
];

fn micro_lang_for_path(path: &str) -> stateful_fuzz::MicroLang {
    if path.ends_with(".rs") {
        stateful_fuzz::MicroLang::Rust
    } else if path.ends_with(".ts") {
        stateful_fuzz::MicroLang::TypeScript
    } else if path.ends_with(".go") {
        stateful_fuzz::MicroLang::Go
    } else {
        stateful_fuzz::MicroLang::Python
    }
}

/// Structure-aware `MicroRepoGen` + `param_elements` + `lower` sweep plus 100%
/// coverage of all 33 linked tree-sitter grammars and same-width `parse_repair`
/// paths, with `HealthTracker` telemetry and `minimize_str` reproducer emission.
#[test]
fn stateful_micro_repo_and_all_33_grammars_parse_repair_sweep() {
    let seed = fuzz_seed();
    let rounds = env_usize("DEVMAP_FUZZ_ROUNDS", DEFAULT_ROUNDS).max(1);
    let micro_param_rounds = rounds.max(2);
    let grammar_lower_rounds = rounds.max(2);
    let grammar_mutate_rounds = (rounds / 2).max(1);
    let repair_lower_rounds = rounds.max(3);
    let repair_mutate_rounds = rounds.max(2);
    let mut src = stateful_fuzz::Source::from_seed(seed);
    let mut rng = Rng(src.fork_seed());
    let mut tracker = stateful_fuzz::HealthTracker::new();

    // 1. Structure-aware MicroRepoGen + `param_elements` + `lower` damage.
    let langs = [
        stateful_fuzz::MicroLang::Rust,
        stateful_fuzz::MicroLang::TypeScript,
        stateful_fuzz::MicroLang::Go,
        stateful_fuzz::MicroLang::Python,
    ];
    let repo = stateful_fuzz::generate_micro_repo(&mut src, &langs, 8);
    for (path, content) in &repo {
        let lang = micro_lang_for_path(path);

        // Clean generated micro-repo file must parse cleanly and deterministically.
        let clean_label = format!("seed {seed} / micro::clean / {path}");
        let clean_ext =
            match check_extraction_and_determinism("micro::clean", &clean_label, path, content) {
                Ok(ext) => ext,
                Err(failure) => fail_with_minimized_repro(
                    seed,
                    "repro_micro_clean",
                    &clean_label,
                    path,
                    content,
                    true,
                    failure,
                ),
            };
        if let Some(failure) = stateful_fuzz::check(
            matches!(clean_ext.parse_outcome, ParseOutcome::Clean),
            "micro_repo_clean_parse",
            "micro::clean",
            format!(
                "{clean_label}: expected Clean, got {:?}",
                clean_ext.parse_outcome
            ),
        )
        .into_failure()
        {
            fail_with_minimized_repro(
                seed,
                "repro_micro_clean_parse",
                &clean_label,
                path,
                content,
                true,
                failure,
            );
        }
        tracker.record("micro::clean", None, &stateful_fuzz::StepOutcome::Ok);
        tracker.record(
            outcome_label(&clean_ext.parse_outcome),
            Some("micro::clean"),
            &stateful_fuzz::StepOutcome::Ok,
        );

        // Structural `param_elements` mutations (must stay syntactically valid).
        let mut evolved = content.clone();
        for round in 0..micro_param_rounds {
            evolved = stateful_fuzz::mutate_file_elements(&mut src, lang, &evolved);
            let label = format!("seed {seed} / micro::param_elements / {path} / round {round}");
            let ext = match check_extraction_and_determinism(
                "micro::param_elements",
                &label,
                path,
                &evolved,
            ) {
                Ok(ext) => ext,
                Err(failure) => fail_with_minimized_repro(
                    seed,
                    "repro_micro_param_elements",
                    &label,
                    path,
                    &evolved,
                    true,
                    failure,
                ),
            };
            if let Some(failure) = stateful_fuzz::check(
                matches!(ext.parse_outcome, ParseOutcome::Clean),
                "micro_param_elements_clean_parse",
                "micro::param_elements",
                format!(
                    "{label}: mutate_file_elements broke syntax; expected Clean, got {:?}",
                    ext.parse_outcome
                ),
            )
            .into_failure()
            {
                fail_with_minimized_repro(
                    seed,
                    "repro_micro_param_elements_clean",
                    &label,
                    path,
                    &evolved,
                    true,
                    failure,
                );
            }
            tracker.record(
                "micro::param_elements",
                Some("micro::clean"),
                &stateful_fuzz::StepOutcome::Ok,
            );
            tracker.record(
                outcome_label(&ext.parse_outcome),
                Some("micro::param_elements"),
                &stateful_fuzz::StepOutcome::Ok,
            );
        }

        // `lower` damage directly on the clean file.
        let lowered = stateful_fuzz::lower_damage(&mut src, content);
        let lower_label = format!("seed {seed} / micro::lower_damage / {path}");
        let lower_ext = match check_extraction_and_determinism(
            "micro::lower_damage",
            &lower_label,
            path,
            &lowered,
        ) {
            Ok(ext) => ext,
            Err(failure) => fail_with_minimized_repro(
                seed,
                "repro_micro_lower",
                &lower_label,
                path,
                &lowered,
                true,
                failure,
            ),
        };
        tracker.record(
            "micro::lower_damage",
            Some("micro::clean"),
            &stateful_fuzz::StepOutcome::Ok,
        );
        tracker.record(
            outcome_label(&lower_ext.parse_outcome),
            Some("micro::lower_damage"),
            &stateful_fuzz::StepOutcome::Ok,
        );

        // Chained `param_elements` -> `lower` damage.
        let chained = stateful_fuzz::lower_damage(&mut src, &evolved);
        let chain_label = format!("seed {seed} / micro::param_then_lower / {path}");
        let chain_ext = match check_extraction_and_determinism(
            "micro::param_then_lower",
            &chain_label,
            path,
            &chained,
        ) {
            Ok(ext) => ext,
            Err(failure) => fail_with_minimized_repro(
                seed,
                "repro_micro_param_then_lower",
                &chain_label,
                path,
                &chained,
                true,
                failure,
            ),
        };
        tracker.record(
            "micro::param_then_lower",
            Some("micro::param_elements"),
            &stateful_fuzz::StepOutcome::Ok,
        );
        tracker.record(
            outcome_label(&chain_ext.parse_outcome),
            Some("micro::param_then_lower"),
            &stateful_fuzz::StepOutcome::Ok,
        );
    }

    // 2. Assert 100% coverage of all 33 linked tree-sitter grammars.
    let expected_keys: BTreeSet<&str> = linked_grammar_keys().into_iter().collect();
    let seed_keys: BTreeSet<&str> = ALL_LINKED_GRAMMAR_SEEDS
        .iter()
        .map(|(grammar, _, _)| *grammar)
        .collect();
    assert_eq!(
        seed_keys, expected_keys,
        "ALL_LINKED_GRAMMAR_SEEDS must cover 100% of linked_grammar_keys()"
    );

    for &(grammar, path, snippet) in ALL_LINKED_GRAMMAR_SEEDS {
        let clean_label = format!("seed {seed} / grammar::clean / {grammar} / {path}");
        let clean_ext =
            match check_extraction_and_determinism("grammar::clean", &clean_label, path, snippet) {
                Ok(ext) => ext,
                Err(failure) => fail_with_minimized_repro(
                    seed,
                    "repro_grammar_clean",
                    &clean_label,
                    path,
                    snippet,
                    true,
                    failure,
                ),
            };
        let engine_dbg = format!("{:?}", clean_ext.engine);
        if let Some(failure) = stateful_fuzz::check(
            !engine_dbg.contains("Unavailable"),
            "linked_grammar_active",
            "grammar::clean",
            format!("{clean_label}: grammar {grammar} reported Unavailable ({engine_dbg})"),
        )
        .into_failure()
        {
            fail_with_minimized_repro(
                seed,
                "repro_grammar_unavailable",
                &clean_label,
                path,
                snippet,
                true,
                failure,
            );
        }
        tracker.record("grammar::clean", None, &stateful_fuzz::StepOutcome::Ok);
        tracker.record(
            outcome_label(&clean_ext.parse_outcome),
            Some("grammar::clean"),
            &stateful_fuzz::StepOutcome::Ok,
        );

        // Rounds of `lower_damage` across every linked grammar.
        for round in 0..grammar_lower_rounds {
            let damaged = stateful_fuzz::lower_damage(&mut src, snippet);
            let label = format!("seed {seed} / grammar::lower_damage / {grammar} / round {round}");
            let ext = match check_extraction_and_determinism(
                "grammar::lower_damage",
                &label,
                path,
                &damaged,
            ) {
                Ok(ext) => ext,
                Err(failure) => fail_with_minimized_repro(
                    seed,
                    "repro_grammar_lower",
                    &label,
                    path,
                    &damaged,
                    true,
                    failure,
                ),
            };
            tracker.record(
                "grammar::lower_damage",
                Some("grammar::clean"),
                &stateful_fuzz::StepOutcome::Ok,
            );
            tracker.record(
                outcome_label(&ext.parse_outcome),
                Some("grammar::lower_damage"),
                &stateful_fuzz::StepOutcome::Ok,
            );
        }

        // Rounds of byte/text Mutation across every linked grammar.
        for round in 0..grammar_mutate_rounds {
            let mutation = MUTATIONS[rng.below(MUTATIONS.len())];
            let mutated = apply(mutation, snippet, &mut rng);
            let label = format!(
                "seed {seed} / grammar::byte_mutate / {grammar} / round {round} / {mutation:?}"
            );
            let ext = match check_extraction_and_determinism(
                "grammar::byte_mutate",
                &label,
                path,
                &mutated,
            ) {
                Ok(ext) => ext,
                Err(failure) => fail_with_minimized_repro(
                    seed,
                    "repro_grammar_mutate",
                    &label,
                    path,
                    &mutated,
                    true,
                    failure,
                ),
            };
            tracker.record(
                "grammar::byte_mutate",
                Some("grammar::clean"),
                &stateful_fuzz::StepOutcome::Ok,
            );
            tracker.record(
                outcome_label(&ext.parse_outcome),
                Some("grammar::byte_mutate"),
                &stateful_fuzz::StepOutcome::Ok,
            );
        }
    }

    // 3. Same-width `parse_repair` / `kotlinsemi` / `sveltetext` quirk seeds +
    //    damaged variants (verifying offset/char-boundary preservation & determinism).
    let repair_mutations = [
        Mutation::DropDelimiter,
        Mutation::DropChar,
        Mutation::InsertBom,
        Mutation::ToCrlf,
        Mutation::RtlOverride,
        Mutation::InsertNul,
    ];
    for &(name, path, snippet) in PARSE_REPAIR_SEEDS {
        let clean_label = format!("seed {seed} / parse_repair::clean / {name}");
        let clean_ext = match check_extraction_and_determinism(
            "parse_repair::clean",
            &clean_label,
            path,
            snippet,
        ) {
            Ok(ext) => ext,
            Err(failure) => fail_with_minimized_repro(
                seed,
                "repro_parse_repair_clean",
                &clean_label,
                path,
                snippet,
                true,
                failure,
            ),
        };
        if let Some(failure) = stateful_fuzz::check(
            matches!(clean_ext.parse_outcome, ParseOutcome::Clean),
            "parse_repair_reaches_clean",
            "parse_repair::clean",
            format!(
                "{clean_label}: expected same-width repair to reach Clean, got {:?}",
                clean_ext.parse_outcome
            ),
        )
        .into_failure()
        {
            fail_with_minimized_repro(
                seed,
                "repro_parse_repair_not_clean",
                &clean_label,
                path,
                snippet,
                true,
                failure,
            );
        }
        tracker.record("parse_repair::clean", None, &stateful_fuzz::StepOutcome::Ok);
        tracker.record(
            outcome_label(&clean_ext.parse_outcome),
            Some("parse_repair::clean"),
            &stateful_fuzz::StepOutcome::Ok,
        );

        for round in 0..repair_lower_rounds {
            let damaged = stateful_fuzz::lower_damage(&mut src, snippet);
            let label =
                format!("seed {seed} / parse_repair::lower_damage / {name} / round {round}");
            let ext = match check_extraction_and_determinism(
                "parse_repair::lower_damage",
                &label,
                path,
                &damaged,
            ) {
                Ok(ext) => ext,
                Err(failure) => fail_with_minimized_repro(
                    seed,
                    "repro_parse_repair_lower",
                    &label,
                    path,
                    &damaged,
                    true,
                    failure,
                ),
            };
            tracker.record(
                "parse_repair::lower_damage",
                Some("parse_repair::clean"),
                &stateful_fuzz::StepOutcome::Ok,
            );
            tracker.record(
                outcome_label(&ext.parse_outcome),
                Some("parse_repair::lower_damage"),
                &stateful_fuzz::StepOutcome::Ok,
            );
        }

        for round in 0..repair_mutate_rounds {
            let m = repair_mutations[src.usize_below(repair_mutations.len())];
            let damaged = apply(m, snippet, &mut rng);
            let label = format!(
                "seed {seed} / parse_repair::damaged_quirk / {name} / round {round} / {m:?}"
            );
            let ext = match check_extraction_and_determinism(
                "parse_repair::damaged_quirk",
                &label,
                path,
                &damaged,
            ) {
                Ok(ext) => ext,
                Err(failure) => fail_with_minimized_repro(
                    seed,
                    "repro_parse_repair_damaged",
                    &label,
                    path,
                    &damaged,
                    true,
                    failure,
                ),
            };
            tracker.record(
                "parse_repair::damaged_quirk",
                Some("parse_repair::clean"),
                &stateful_fuzz::StepOutcome::Ok,
            );
            tracker.record(
                outcome_label(&ext.parse_outcome),
                Some("parse_repair::damaged_quirk"),
                &stateful_fuzz::StepOutcome::Ok,
            );
        }
    }

    eprintln!(
        "micro & 33-grammar sweep: seed {seed}, rounds={rounds} \
         (micro_param={micro_param_rounds}, grammar_lower={grammar_lower_rounds}, \
          grammar_mutate={grammar_mutate_rounds}, repair_lower={repair_lower_rounds}, \
          repair_mutate={repair_mutate_rounds})"
    );
    eprintln!(
        "{}",
        tracker.probe_report("mutation_fuzz::micro_and_33_grammars")
    );
    tracker.assert_actions_exercised(&[
        "micro::clean",
        "micro::param_elements",
        "micro::lower_damage",
        "micro::param_then_lower",
        "grammar::clean",
        "grammar::lower_damage",
        "grammar::byte_mutate",
        "parse_repair::clean",
        "parse_repair::lower_damage",
        "parse_repair::damaged_quirk",
        "outcome::Clean",
        "outcome::Partial",
        "outcome::Failed",
    ]);
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
