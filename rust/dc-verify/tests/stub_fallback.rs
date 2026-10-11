//! The substring fallback, the degraded report for a file the tree could not
//! read, and the parse budget.
//!
//! Every file without a usable post-image takes the fallback, and for a long
//! time it matched only the exact no-argument spellings: `todo!("wire it")`,
//! `unimplemented!("later")`, Go's `panic("not implemented")` and a JavaScript
//! `throw new Error("Not implemented")` all passed it, and a `.rs` file with
//! `todo!("x")` and one syntax error passed clean because the tree refused it
//! and nothing said so.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use dc_verify::rigor::{Severity, detect_stubs, detect_stubs_report};
use dc_verify::stub_ast::{Lang, Unparsed, analyze_within};
use dc_verify::{ChangeStatus, FileDiff};

fn modified(path: &str, line: &str) -> FileDiff {
    FileDiff {
        path: path.to_string(),
        old_path: None,
        status: ChangeStatus::Modified,
        added_lines: vec![(3, line.to_string())],
        removed_lines: Vec::new(),
    }
}

#[test]
fn the_fallback_catches_placeholders_that_carry_arguments() {
    let blocking = [
        ("src/a.rs", r#"    todo!("wire it")"#),
        ("src/a.rs", r#"    unimplemented!("later")"#),
        ("src/a.rs", r#"    panic!("not implemented")"#),
        ("pkg/a.go", r#"	panic("not implemented")"#),
        ("pkg/a.go", r#"	panic("todo")"#),
        ("web/a.js", r#"  throw new Error("Not implemented");"#),
        (
            "web/a.ts",
            r#"  throw new Error('TODO: wire it up');"#.replace('\'', "\"").leak(),
        ),
        ("web/a.jsx", r#"  throw new Error("not yet implemented")"#),
        ("app/a.kt", r#"    TODO("later")"#),
        (
            "app/a.php",
            r#"    throw new Exception("not implemented");"#,
        ),
    ];
    for (path, line) in blocking {
        let findings = detect_stubs(&[modified(path, line)]);
        assert!(
            findings
                .iter()
                .any(|f| f.gate == "stub_detection" && f.severity == Severity::Blocking),
            "{path}: `{line}` passed the fallback: {findings:?}"
        );
    }
}

#[test]
fn the_fallback_does_not_flag_quotation_or_unrelated_calls() {
    let clean = [
        ("src/a.rs", r#"    log("todo!(x) is a macro")"#),
        ("src/a.rs", r#"    // see todo!("x") in the old code"#),
        ("pkg/a.go", r#"	panic("index out of range")"#),
        ("pkg/a.go", r#"	panic("todolist is corrupt")"#),
        ("web/a.js", r#"  throw new Error("invalid argument");"#),
        (
            "web/a.js",
            r#"  console.log("throw new Error(\"not implemented\")")"#,
        ),
        ("app/a.kt", r#"    val items = todoList("x")"#),
    ];
    for (path, line) in clean {
        let blocking: Vec<_> = detect_stubs(&[modified(path, line)])
            .into_iter()
            .filter(|f| f.severity == Severity::Blocking)
            .collect();
        assert!(
            blocking.is_empty(),
            "{path}: `{line}` blocked: {blocking:?}"
        );
    }
}

#[test]
fn a_file_the_tree_refused_is_reported_and_still_scanned() {
    // `todo!("x")` plus one syntax error: the tree refuses it, the fallback
    // must still see the placeholder, and the refusal must be on the report.
    let source = "fn a() {\n    let x = ;\n    todo!(\"x\")\n}\n";
    let file = FileDiff {
        path: "src/a.rs".to_string(),
        old_path: None,
        status: ChangeStatus::Modified,
        added_lines: vec![
            (2, "    let x = ;".to_string()),
            (3, "    todo!(\"x\")".to_string()),
        ],
        removed_lines: Vec::new(),
    };
    let report = detect_stubs_report(&[file], &|_| Some(source.to_string()));
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Blocking),
        "the fallback missed the placeholder: {report:?}"
    );
    assert_eq!(report.unparsed.len(), 1, "{report:?}");
    assert_eq!(report.unparsed[0].path, "src/a.rs");
    assert_eq!(report.unparsed[0].reason, "syntax error");

    // A clean parse is not reported.
    let ok = FileDiff {
        path: "src/b.rs".to_string(),
        old_path: None,
        status: ChangeStatus::Modified,
        added_lines: vec![(1, "fn b() {}".to_string())],
        removed_lines: Vec::new(),
    };
    let report = detect_stubs_report(&[ok], &|_| Some("fn b() {}\n".to_string()));
    assert!(report.unparsed.is_empty(), "{report:?}");
}

#[test]
fn a_pathological_parse_is_cancelled_within_its_budget() {
    // One 96 KB line of repeated syntax errors: error recovery is what makes a
    // tree-sitter parse superlinear (about a second here uncancelled, and far
    // worse on the audit's 100 KB file). The budget must end it and the file
    // must be reported as over budget — not parsed to the end and discarded.
    let source = "x = = 1 ".repeat(12_000);
    let added: HashSet<u32> = HashSet::from([1]);
    let started = Instant::now();
    let outcome = analyze_within(Lang::Python, &source, &added, Duration::from_millis(20));
    let took = started.elapsed();
    assert_eq!(outcome, Err(Unparsed::OverBudget), "after {took:?}");
    assert!(
        took < Duration::from_millis(500),
        "the parse ran {took:?} against a 20ms budget"
    );
}
