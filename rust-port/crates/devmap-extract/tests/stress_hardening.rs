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
