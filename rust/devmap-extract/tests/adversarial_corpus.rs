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
            for reference in &extraction.references {
                assert!(
                    !reference.name.trim().is_empty(),
                    "{} / {label}: a reference with an empty name joins on nothing",
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

/// Sources that hold a known trigger for an empty name, extracted alongside the
/// fixtures. Without one the sweep over `testdata` passes on code that still
/// has the defect (none of the checked-in fixtures spells it), so this table is
/// what makes the sweep able to fail. Rows: (language key, path, source).
const TRIGGERS: &[(&str, &str, &str)] = &[
    (
        "swift",
        "triggers/nil_coalesced_cast.swift",
        "let a = b as? String ?? \"x\"\n",
    ),
    // A use path missing its keyword: the grammar reads a struct expression
    // with no spellable type and the Rust walk publishes a Constructor named "".
    ("rust", "triggers/path_brace_list.rs", " a::b::{c};"),
];

/// Fixture files that still publish an empty name: (language key, path relative
/// to `testdata`, the card that owns the fix). Consumed exactly: a row that no
/// longer reproduces fails the test, so the list cannot outlive its defects.
/// Rows that are not fixtures name their trigger's path.
///
/// Fixture language directories no grammar parses, so the sweep says nothing
/// about their emitters: (directory, why). Sorted by directory and consumed
/// exactly, like [`KNOWN_EMPTY`]: linking a grammar makes its row fail.
const NO_GRAMMAR: &[(&str, &str)] = &[
    (
        "cobol",
        "refused before any parser sees it (the vendored grammar does not terminate on malformed input)",
    ),
    (
        "vbnet",
        "no tree-sitter-vb grammar is linked (treesitter/tests.rs and cache.rs pin it as unavailable)",
    ),
];
const KNOWN_EMPTY: &[(&str, &str, &str)] = &[];

fn testdata_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            testdata_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// No language publishes a call or a reference with an empty name.
///
/// An empty name is a join on nothing: the resolver keys on it and the graph
/// grows a node every unresolved call attaches to. The hostile sweep above
/// cannot see the Swift case (it needs a grammar-recovery shape), so this
/// extracts every file under `testdata` plus the [`TRIGGERS`] and reports the
/// language and path of each offender. Coverage is asserted with both numbers:
/// every `testdata/fixtures/languages/<dir>` must yield at least one extraction.
#[test]
fn no_language_publishes_an_empty_name_in_fixture_output() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata");
    let mut files = Vec::new();
    testdata_files(&root, &mut files);
    assert!(!files.is_empty(), "no files under {}", root.display());

    let language_dirs: std::collections::BTreeSet<String> =
        std::fs::read_dir(root.join("fixtures/languages"))
            .expect("fixtures/languages")
            .map(|entry| entry.expect("entry").path())
            .filter(|p| p.is_dir())
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
    // Per fixture language directory: (sources extracted, sources a grammar parsed).
    let mut covered_dirs: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();

    let mut offenders: Vec<(String, String, usize, usize)> = Vec::new();
    let mut scanned = 0usize;
    let mut languages = std::collections::BTreeSet::new();
    // Returns whether a grammar parsed the source (Clean or Partial), which is
    // what "an extraction came back" means: a refusal, a failure or a pattern
    // fallback is not evidence about the language's emitters.
    let mut check = |language_hint: Option<&str>, rel: &str, source: &str| -> bool {
        let extraction = extract_file(rel, source);
        let parsed = matches!(
            extraction.parse_outcome,
            ParseOutcome::Clean | ParseOutcome::Partial { .. }
        );
        scanned += 1;
        languages.insert(extraction.language.clone());
        let calls = extraction
            .calls
            .iter()
            .filter(|c| c.callee_name.trim().is_empty())
            .count();
        let refs = extraction
            .references
            .iter()
            .filter(|r| r.name.trim().is_empty())
            .count();
        if calls != 0 || refs != 0 {
            let language = language_hint.unwrap_or(&extraction.language).to_string();
            offenders.push((language, rel.to_string(), calls, refs));
        }
        parsed
    };
    for path in &files {
        // Binary or non-UTF-8 fixtures are not source the extractor reads.
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(dir) = rel
            .strip_prefix("fixtures/languages/")
            .and_then(|rest| rest.split('/').next())
        {
            let parsed = check(None, &rel, &source);
            let counts = covered_dirs
                .entry(dir.to_string())
                .or_insert((0usize, 0usize));
            counts.0 += 1;
            counts.1 += usize::from(parsed);
        } else {
            check(None, &rel, &source);
        }
    }
    for (language, rel, source) in TRIGGERS {
        check(Some(language), rel, source);
    }
    eprintln!(
        "empty-name sweep: extracted {scanned} sources ({} files under testdata, {} triggers) \
         across {} language keys; {} of {} fixture language directories covered",
        files.len(),
        TRIGGERS.len(),
        languages.len(),
        covered_dirs.len(),
        language_dirs.len()
    );
    let seen_dirs: std::collections::BTreeSet<String> = covered_dirs.keys().cloned().collect();
    assert_eq!(
        seen_dirs, language_dirs,
        "every fixtures/languages directory must contribute at least one readable file"
    );
    let unparsed: Vec<&str> = covered_dirs
        .iter()
        .filter(|(_, (_, parsed))| *parsed == 0)
        .map(|(dir, _)| dir.as_str())
        .collect();
    let excluded: Vec<&str> = NO_GRAMMAR.iter().map(|(dir, _)| *dir).collect();
    assert_eq!(
        unparsed, excluded,
        "every fixtures/languages directory must yield at least one extraction a \
         grammar parsed (Clean or Partial), except the NO_GRAMMAR rows, which must \
         be exactly the directories that yielded none"
    );

    let mut unexpected = Vec::new();
    let mut consumed = vec![false; KNOWN_EMPTY.len()];
    for (language, rel, calls, refs) in &offenders {
        match KNOWN_EMPTY
            .iter()
            .position(|(l, p, _)| l == language && p == rel)
        {
            Some(at) => consumed[at] = true,
            None => unexpected.push(format!(
                "{language}: {rel}: {calls} empty-name call(s), {refs} empty-name reference(s)"
            )),
        }
    }
    let stale: Vec<_> = KNOWN_EMPTY
        .iter()
        .zip(&consumed)
        .filter(|(_, used)| !**used)
        .map(|(row, _)| row)
        .collect();
    assert!(
        unexpected.is_empty() && stale.is_empty(),
        "empty names published: {unexpected:#?}; KNOWN_EMPTY rows that no longer \
         reproduce: {stale:?}"
    );
}
