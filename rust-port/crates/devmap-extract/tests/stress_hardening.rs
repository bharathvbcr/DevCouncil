//! Adversarial inputs for the extraction paths added by SC17 and SC25.
//!
//! Every function these exercise walks a type expression recursively. A missing
//! bound is not a slow path but a stack overflow, which aborts the process — a
//! build that dies on one hostile file takes the whole repository with it. The
//! depth caps are asserted here rather than assumed, and each case also pins the
//! *fail-closed* direction: past the cap the extractor yields nothing, never a
//! guess.

use devmap_extract::extract_file;
use devmap_extract::model::{ParseOutcome, ReferenceKind};

/// Deeply nested Go pointers around a package-qualified type.
///
/// `**…*testing.T` must terminate. Past the depth cap the qualifier is simply
/// not recovered, which costs a classification and breaks nothing.
#[test]
fn deeply_nested_go_types_terminate_without_overflowing() {
    for depth in [1usize, 12, 200] {
        let ty = format!("{}testing.T", "*".repeat(depth));
        let source = format!(
            "package svc\nimport \"testing\"\nfunc run(t {ty}) {{\n\tt.Fatalf(\"x\")\n}}\n"
        );
        let extraction = extract_file("svc/deep.go", &source);
        // The point is that we get here at all.
        assert_ne!(
            extraction.parse_outcome,
            ParseOutcome::Failed {
                reason: String::new()
            },
            "depth {depth} must not fail outright"
        );
        let qualifiers: Vec<&str> = extraction
            .references
            .iter()
            .filter(|r| r.kind == ReferenceKind::TypeQualifier)
            .map(|r| r.name.as_str())
            .collect();
        if depth <= 16 {
            assert_eq!(
                qualifiers,
                vec!["testing"],
                "within the cap the qualifier must still be recovered at depth {depth}"
            );
        } else {
            assert!(
                qualifiers.is_empty(),
                "past the cap the extractor must yield nothing rather than a \
                 guess; depth {depth} produced {qualifiers:?}"
            );
        }
    }
}

/// A collection-typed parameter contributes no receiver binding at all — not a
/// type, and therefore not a qualifier either.
///
/// This is deliberate, and it is the SC9 lesson: binding `[]Foo` to `Foo` would
/// let `xs.Method()` dispatch onto `Foo`'s method set, which is a confidently
/// wrong edge. `go_type_name` refuses to type a slice, and the qualifier rides
/// on that same binding so the two can never disagree about what a value is.
#[test]
fn collection_typed_parameters_contribute_no_receiver_binding() {
    let extraction = extract_file(
        "svc/coll.go",
        "package svc\nimport \"testing\"\nfunc run(ts []*testing.T) {\n\t_ = ts\n}\n",
    );
    let bound: Vec<(&str, &str)> = extraction
        .references
        .iter()
        .filter(|r| matches!(r.kind, ReferenceKind::Type | ReferenceKind::TypeQualifier))
        .filter_map(|r| r.assigned_to.as_deref().map(|to| (to, r.name.as_str())))
        .collect();
    assert!(
        bound.is_empty(),
        "a slice-typed parameter must bind neither a type nor a qualifier, or \
         `ts.Method()` could dispatch onto the element type; got {bound:?}"
    );
}

/// The Rust equivalent: `&&&…reqwest::Client` behind many reference layers.
#[test]
fn deeply_nested_rust_types_terminate_without_overflowing() {
    for depth in [2usize, 20, 200] {
        let ty = format!("{}reqwest::Client", "&".repeat(depth));
        let source = format!("fn run(c: {ty}) {{\n    c.execute();\n}}\n");
        let extraction = extract_file("lib/deep.rs", &source);
        let qualifiers: Vec<&str> = extraction
            .references
            .iter()
            .filter(|r| r.kind == ReferenceKind::TypeQualifier)
            .map(|r| r.name.as_str())
            .collect();
        if depth <= 16 {
            assert_eq!(qualifiers, vec!["reqwest"], "depth {depth}");
        } else {
            assert!(
                qualifiers.is_empty(),
                "depth {depth} produced {qualifiers:?}"
            );
        }
    }
}

/// SC17's composite-literal unwrap has the same shape and the same risk.
#[test]
fn deeply_nested_go_composite_literals_terminate() {
    for depth in [3usize, 20, 200] {
        let ty = format!("{}Widget", "[]".repeat(depth));
        let source = format!("package svc\nfunc build() {{\n\t_ = {ty}{{}}\n}}\n");
        let extraction = extract_file("svc/lit.go", &source);
        let callees: Vec<&str> = extraction
            .calls
            .iter()
            .map(|c| c.callee_name.as_str())
            .collect();
        if depth <= 16 {
            assert!(
                callees.contains(&"Widget"),
                "depth {depth} must still reach the named type; got {callees:?}"
            );
        }
        // Past the cap nothing is emitted; the invariant that always holds is
        // that the raw type expression is never used as a callee.
        assert!(
            !callees.iter().any(|c| c.contains('[')),
            "a type expression must never become a callee; got {callees:?}"
        );
    }
}

/// Syntactically broken source must not fabricate qualifiers or calls.
///
/// X6/X7 already forbid promoting a recovered parse to authoritative output;
/// these paths are new and must obey the same rule.
#[test]
fn malformed_sources_yield_no_fabricated_type_evidence() {
    let cases: &[(&str, &str)] = &[
        ("svc/broken.go", "package svc\nfunc run(t *testing.\n"),
        ("lib/broken.rs", "fn run(c: &reqwest::) {\n    c.go(\n"),
        ("svc/half.go", "package svc\nfunc run(t *) { t.Do() }\n"),
        ("lib/empty.rs", ""),
        ("svc/empty.go", ""),
    ];
    for (path, source) in cases {
        let extraction = extract_file(path, source);
        for reference in &extraction.references {
            if reference.kind == ReferenceKind::TypeQualifier {
                assert!(
                    !reference.name.is_empty(),
                    "{path}: an empty qualifier is not evidence and must not be \
                     emitted"
                );
            }
        }
        for call in &extraction.calls {
            assert!(
                !call.callee_name.is_empty(),
                "{path}: an empty callee name must never be recorded"
            );
        }
    }
}

/// A pathologically wide file must complete in reasonable time and keep its
/// per-parameter bindings distinct — the qualifier map is keyed by name, and a
/// collision there is the SC9 defect.
#[test]
fn many_distinct_parameters_keep_distinct_qualifiers() {
    let mut source = String::from("package svc\n");
    for index in 0..200 {
        source.push_str(&format!("import pkg{index} \"example.com/pkg{index}\"\n"));
    }
    source.push_str("func run(\n");
    for index in 0..200 {
        source.push_str(&format!("\tv{index} *pkg{index}.Thing,\n"));
    }
    source.push_str(") {\n");
    for index in 0..200 {
        source.push_str(&format!("\tv{index}.Do()\n"));
    }
    source.push_str("}\n");

    let extraction = extract_file("svc/wide.go", &source);
    let mut seen: Vec<(String, String)> = extraction
        .references
        .iter()
        .filter(|r| r.kind == ReferenceKind::TypeQualifier)
        .filter_map(|r| r.assigned_to.clone().map(|to| (to, r.name.clone())))
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        200,
        "each parameter must carry its own qualifier; got {} distinct pairs",
        seen.len()
    );
    for (param, qualifier) in &seen {
        let index = param.trim_start_matches('v');
        assert_eq!(
            qualifier,
            &format!("pkg{index}"),
            "parameter {param} must map to its own package"
        );
    }
}

/// A truncated symbol list must still say so after it is persisted.
///
/// `fallback::scan_declarations` computes `truncated` correctly and
/// `extract_treesitter` pushes it into `diagnostics` with the comment
/// "Reported, not silently dropped". `Extraction::for_durable_store` — the
/// payload written to `generation_files.extraction_json` and the extraction
/// cache — then calls `diagnostics.clear()`, and `diagnostics` has no
/// production reader anywhere in the workspace. So the stored record of a
/// 2,500-declaration file was 2,000 symbols under a reason string that reads as
/// a complete recovery, and the missing 500 were indistinguishable from
/// declarations that do not exist.
#[test]
fn a_truncated_fallback_scan_survives_the_durable_store() {
    let total = devmap_extract::fallback::MAX_FALLBACK_SYMBOLS + 500;
    let mut source = String::from("syntax = \"proto3\";\n");
    for i in 0..total {
        source.push_str(&format!("message Msg{i} {{\n}}\n"));
    }

    let extraction = devmap_extract::extract_file("big.proto", &source);
    let durable = extraction.for_durable_store();

    let reason = match &durable.parse_outcome {
        devmap_extract::model::ParseOutcome::Fallback { reason } => reason.clone(),
        other => {
            panic!("a grammarless language with recovered declarations is Fallback: {other:?}")
        }
    };
    assert!(
        reason.contains("500"),
        "the persisted outcome must name the dropped declarations, got {reason:?}"
    );
    assert!(
        reason.contains(&total.to_string()),
        "the persisted outcome must name the true total, got {reason:?}"
    );
    // The prefix itself is unchanged; only the honesty about it is new.
    assert_eq!(
        durable.symbols.len(),
        devmap_extract::fallback::MAX_FALLBACK_SYMBOLS + 1,
        "the cap still applies (plus the File node)"
    );
}

/// A pathological source must not stall the build, and must not be described as
/// anything other than refused.
///
/// Measured before this bound existed (release build, tree-sitter-cpp 0.23): a
/// 4,000-byte C++ file of 2,000 nested braces took **130,994 ms** — over two
/// minutes for one file. Ruby took 8,807 ms on the same input; Python 58 ms. A
/// repository containing one generated or minified file of this shape made
/// `dev map` look like a hang, and made a daemon rebuild hold its lock for the
/// duration. Nothing reported it.
///
/// The bound is only half the fix. A cancelled tree-sitter parse can still hand
/// back a partial tree describing a prefix of the file; publishing that would
/// be a truncated symbol set presented as a complete one. The refusal is
/// explicit instead, and it must never claim the grammar is missing — that
/// sentence is reserved for a language with no grammar at all.
#[test]
fn a_pathological_source_is_refused_within_its_budget() {
    use std::time::{Duration, Instant};

    let hostile = format!("{}{}", "{".repeat(2_000), "}".repeat(2_000));
    let budget = Duration::from_millis(200);

    let started = Instant::now();
    let extraction = devmap_extract::treesitter::extract_treesitter_with_budget(
        "h.cpp", "cpp", &hostile, budget,
    );
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "a {:?} budget must bound the parse; took {elapsed:?}",
        budget
    );

    let reason = match &extraction.parse_outcome {
        devmap_extract::model::ParseOutcome::Failed { reason } => reason.clone(),
        other => panic!("an abandoned parse must be Failed, not {other:?}"),
    };
    assert!(
        reason.contains("budget"),
        "the refusal must name the budget, got {reason:?}"
    );
    assert!(
        !reason.contains("no linked tree-sitter grammar"),
        "a grammar that exists must never be reported missing: {reason:?}"
    );

    // Exactly the File node, and no symbols recovered from an unparsed file.
    let declarations: Vec<_> = extraction
        .symbols
        .iter()
        .filter(|s| s.kind != devmap_extract::model::SymbolKind::File)
        .collect();
    assert!(
        declarations.is_empty(),
        "an abandoned parse must claim no declarations, got {declarations:?}"
    );
    assert_eq!(
        extraction.symbols.len(),
        1,
        "the File node is still emitted"
    );

    // A legitimate file of the same language is unaffected by the bound.
    let ok = devmap_extract::treesitter::extract_treesitter_with_budget(
        "fine.cpp",
        "cpp",
        "int add(int a, int b) { return a + b; }\n",
        budget,
    );
    assert!(
        matches!(ok.parse_outcome, devmap_extract::model::ParseOutcome::Clean),
        "ordinary source still parses cleanly: {:?}",
        ok.parse_outcome
    );
}

/// A source containing a NUL byte is refused before any grammar sees it.
///
/// Not a style rule — a liveness one. Measured 2026-09-04: `"a\0b\0c\n"`, six
/// bytes, run through the vendored `tree-sitter-cobol` grammar did not
/// terminate within three minutes, while the same input costs every other
/// grammar microseconds. The parse budget cannot save this: tree-sitter's
/// progress callback is never reached from inside a scanner that is spinning,
/// and a spinning thread cannot be killed in-process. A single `.cbl` file with
/// a stray NUL would hang `dev map` outright and leave a daemon holding its
/// lock forever.
///
/// The boundary check is also correct on its own terms — a file containing a
/// NUL is binary, and every extractor here assumes text — and it holds for
/// every grammar, including vendored ones this repository does not control.
#[test]
fn a_source_containing_a_nul_byte_is_refused_before_parsing() {
    use std::time::{Duration, Instant};

    // `.cbl` is deliberately absent: COBOL is refused earlier still, by
    // `UNSAFE_GRAMMARS`, for a stronger reason than the NUL byte — see
    // `cobol_is_refused_promptly_rather_than_parsed_by_a_nonterminating_grammar`.
    for (path, _lang) in [("b.py", "python"), ("b.rs", "rust"), ("b.ts", "typescript")] {
        let started = Instant::now();
        let extraction = devmap_extract::extract_file(path, "a\0b\0c\n");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{path}: the refusal must be immediate, took {:?}",
            started.elapsed()
        );

        let reason = match &extraction.parse_outcome {
            devmap_extract::model::ParseOutcome::Failed { reason } => reason.clone(),
            other => panic!("{path}: a NUL-bearing source must be Failed, not {other:?}"),
        };
        assert!(
            reason.contains("NUL"),
            "{path}: the refusal must name the cause, got {reason:?}"
        );
        assert!(
            !reason.contains("no linked tree-sitter grammar"),
            "{path}: a grammar that exists must not be reported missing: {reason:?}"
        );
        // The File node survives so the file stays addressable as an edge target.
        assert_eq!(extraction.symbols.len(), 1, "{path}: only the File node");
    }

    // Text without a NUL is unaffected.
    let ok = devmap_extract::extract_file("fine.py", "def f():\n    return 1\n");
    assert!(
        matches!(ok.parse_outcome, devmap_extract::model::ParseOutcome::Clean),
        "ordinary source is untouched: {:?}",
        ok.parse_outcome
    );
}

/// COBOL is refused rather than parsed, and the refusal is prompt.
///
/// The vendored `tree-sitter-cobol` grammar **does not terminate** on malformed
/// input. Measured 2026-09-04: `"a\0b\0c\n"` (six bytes) and `"\u{feff}????\n"`
/// each ran past three minutes, while the same 14 hostile inputs across the
/// other 34 grammars complete in 5.03 s total. No in-process bound stops it —
/// tree-sitter's progress callback is never reached from inside a scanner that
/// is spinning, and a spinning thread cannot be killed.
///
/// What settled it was measuring what the grammar was *worth*. On a realistic
/// COBOL program (IDENTIFICATION/DATA/PROCEDURE DIVISION, two paragraphs, a
/// PERFORM) it parsed `Clean` and yielded **only the File node** — zero
/// declarations, zero calls — and the bounded fallback scanner recovers nothing
/// either. So the grammar contributed nothing measurable and carried an
/// unbounded hang, and unlinking it costs no coverage.
///
/// COBOL files remain indexed as File nodes, so they stay addressable as edge
/// targets and are honestly labelled.
#[test]
fn cobol_is_refused_promptly_rather_than_parsed_by_a_nonterminating_grammar() {
    use std::time::{Duration, Instant};

    // The two inputs that hung, plus an ordinary program.
    for source in [
        "a\0b\0c\n",
        "\u{feff}????\n",
        "       IDENTIFICATION DIVISION.\n       PROGRAM-ID. PAYROLL.\n",
    ] {
        let started = Instant::now();
        let extraction = devmap_extract::extract_file("pay.cbl", source);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "COBOL must be refused immediately, took {:?} on {source:?}",
            started.elapsed()
        );
        assert!(
            matches!(
                extraction.parse_outcome,
                devmap_extract::model::ParseOutcome::Failed { .. }
            ),
            "COBOL must be refused, got {:?}",
            extraction.parse_outcome
        );
        // Still addressable: the File node survives so edges can target it.
        assert_eq!(
            extraction.symbols.len(),
            1,
            "the File node is still emitted for {source:?}"
        );
    }
}
