//! Adversarial input for the W3.3 wiring port.
//!
//! The two rules ported from Python run over *every* source file in a corpus,
//! before anything decides whether the file is interesting. A pathological
//! input that makes them quadratic, or that walks out of the repository root,
//! costs every build — so the inputs below are chosen to break them rather than
//! to demonstrate them.
//!
//! Rust's `regex` has no backtracking, so catastrophic backtracking is
//! structurally impossible; what these check is everything else — unbounded
//! output, path escape, pathological nesting, and non-UTF-8-shaped text.

use devmap_extract::model::WiringKind;
use devmap_extract::wiring::{dynamic_reference_forms, extract_wiring_annotations, ALLOW_UNWIRED};
use std::time::Instant;

/// A file of nothing but dynamic imports must stay linear.
#[test]
fn a_file_of_nothing_but_dynamic_imports_stays_linear() {
    let small: String = (0..1_000)
        .map(|i| format!("import('./mod{i}');\n"))
        .collect();
    let large: String = (0..10_000)
        .map(|i| format!("import('./mod{i}');\n"))
        .collect();

    let start = Instant::now();
    let small_forms = dynamic_reference_forms("src/app.ts", &small);
    let small_elapsed = start.elapsed();

    let start = Instant::now();
    let large_forms = dynamic_reference_forms("src/app.ts", &large);
    let large_elapsed = start.elapsed();

    assert!(small_forms.len() >= 1_000, "{}", small_forms.len());
    assert!(large_forms.len() >= 10_000, "{}", large_forms.len());
    // Ten times the input for far less than a hundred times the time. A loose
    // bound on purpose: this is here to catch a quadratic scan, not to police
    // the machine it runs on.
    let ratio = large_elapsed.as_secs_f64() / small_elapsed.as_secs_f64().max(1e-9);
    assert!(
        ratio < 40.0,
        "10x the input took {ratio:.1}x the time ({small_elapsed:?} -> {large_elapsed:?}), \
         which is the shape of a quadratic scan"
    );
}

/// A specifier that climbs past the repository root must not produce a path
/// that escapes it.
///
/// `normalize_rel_path` pops on `..` and pops nothing off an empty stack, so
/// the result is clamped at the root rather than turning into `../../etc`.
#[test]
fn a_specifier_cannot_climb_out_of_the_repository() {
    for spec in [
        "../../../../../../../../etc/passwd",
        "./../../..",
        "../".repeat(500).as_str(),
    ] {
        let source = format!("import('{spec}');\n");
        for form in dynamic_reference_forms("src/deep/app.ts", &source) {
            assert!(
                !form.starts_with('/') && !form.contains(".."),
                "{spec:?} produced {form:?}, which names something outside the corpus"
            );
        }
    }
}

/// Deeply nested and absurdly long specifiers terminate.
#[test]
fn pathological_specifiers_terminate() {
    let deep = "a/".repeat(10_000);
    let cases = [
        format!("import('./{deep}mod');\n"),
        format!("importlib.import_module('{}')\n", "a.".repeat(10_000)),
        format!("import('{}');\n", "x".repeat(100_000)),
        // An unterminated literal: the pattern must simply not match.
        "import('unterminated\n".to_string(),
        // A quote storm.
        "'\"'\"'\"".repeat(10_000),
    ];
    for source in cases {
        let start = Instant::now();
        let forms = dynamic_reference_forms("src/app.ts", &source);
        assert!(
            start.elapsed().as_secs() < 5,
            "scanning took over 5s for a {}-byte input",
            source.len()
        );
        // No assertion on the contents: the property under test is that it
        // returns at all, and returns something bounded.
        assert!(forms.len() < 100_000, "unbounded output: {}", forms.len());
    }
}

/// The marker is found wherever it appears, and only where it appears.
#[test]
fn the_marker_survives_a_hostile_file() {
    let mut source = String::new();
    source.push_str(&"// filler\n".repeat(50_000));
    source.push_str(&format!("# {ALLOW_UNWIRED}\n"));
    source.push_str(&"// filler\n".repeat(50_000));
    let kinds: Vec<_> = extract_wiring_annotations("pkg/big.py", &source)
        .into_iter()
        .map(|a| a.kind)
        .collect();
    assert!(kinds.contains(&WiringKind::AllowUnwired));

    // And the same file without it.
    let clean = "// filler\n".repeat(100_000);
    let kinds: Vec<_> = extract_wiring_annotations("pkg/big.py", &clean)
        .into_iter()
        .map(|a| a.kind)
        .collect();
    assert!(!kinds.contains(&WiringKind::AllowUnwired));
}

/// Text the grammar would reject must not crash the scanner.
///
/// These rules run over every file before anything decides the file is
/// interesting, so they meet minified bundles, binary blobs mislabelled as
/// source, and half-written buffers.
#[test]
fn hostile_text_does_not_panic() {
    let cases: Vec<String> = vec![
        String::new(),
        "\0\0\0\0".repeat(1000),
        "\u{feff}import('./a')".to_string(),
        "𝕚𝕞𝕡𝕠𝕣𝕥('./a')".to_string(),
        // Multi-byte characters straddling what looks like a specifier.
        "import('./日本語/モジュール')".to_string(),
        "\r\n".repeat(50_000),
        "import(".repeat(20_000),
        "')".repeat(20_000),
    ];
    for source in cases {
        for path in ["src/a.ts", "pkg/a.py", "weird", "a.toml", ".hidden"] {
            let _ = dynamic_reference_forms(path, &source);
            let _ = extract_wiring_annotations(path, &source);
        }
    }
}

/// A path with no extension, or nothing but extension, must not panic.
#[test]
fn degenerate_paths_are_handled() {
    for path in ["", ".", "..", "/", "a/", ".ts", "a.", "a..b", "///"] {
        let _ = dynamic_reference_forms(path, "import('./x')\n");
        let _ = extract_wiring_annotations(path, "import('./x')\n");
    }
}

/// A UTF-8 specifier round-trips rather than being sliced mid-character.
///
/// `strip_suffix` and `rsplit_once` are char-boundary safe, but the extension
/// stripping walks a list of ASCII suffixes over a possibly multi-byte string,
/// which is exactly where a byte-index bug would show up.
#[test]
fn multibyte_specifiers_are_not_sliced_mid_character() {
    let forms = dynamic_reference_forms("src/app.ts", "import('./компонент.tsx')\n");
    assert!(
        forms.iter().any(|form| form.contains("компонент")),
        "{forms:?}"
    );
    for form in &forms {
        assert!(std::str::from_utf8(form.as_bytes()).is_ok());
    }
}
