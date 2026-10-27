//! extract-v98 row 16: a Kotlin class, interface or object body whose last
//! member shares a line with the closing brace.
//!
//! tree-sitter-kotlin-ng reads `class A { val x = 1 }` as a whole-file ERROR: the
//! class and every declaration after it are lost, and a member that survives
//! keeps the file as its owner. The same text on several lines parses cleanly.
//! The extractor writes `;` over a space (same length, so every offset is the
//! file's own), re-parses, and keeps the result only if it is fully clean.
//!
//! Every positive test here fails against `833cc524`, where the file is
//! `Partial` and the declarations are lost.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind};

use crate::kotlin_recovery_twin::diff_against_twin;

fn names(extraction: &Extraction) -> Vec<&str> {
    let mut names: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    names.sort();
    names
}

fn is_clean(extraction: &Extraction) -> bool {
    matches!(extraction.parse_outcome, ParseOutcome::Clean)
}

/// `(label, the one-line file, the same file over several lines)`.
fn shapes() -> Vec<(&'static str, &'static str, &'static str, bool)> {
    vec![
        (
            "companion object",
            "class C { companion object { fun f() = 1 } }\nfun top() {}\nfun top2() = 2\n",
            "class C {\n    companion object {\n        fun f() = 1\n    }\n}\nfun top() {}\nfun top2() = 2\n",
            true,
        ),
        (
            "named companion object",
            "class C { companion object Factory { fun f() = 1 } }\nfun top() {}\n",
            "class C {\n    companion object Factory {\n        fun f() = 1\n    }\n}\nfun top() {}\n",
            true,
        ),
        (
            "class with a property",
            "class A { val x = 1 }\nfun top() {}\n",
            "class A {\n    val x = 1\n}\nfun top() {}\n",
            true,
        ),
        (
            "interface",
            "interface Cb { fun call() }\nfun top() {}\n",
            "interface Cb {\n    fun call()\n}\nfun top() {}\n",
            true,
        ),
        (
            "anonymous object",
            "val o = object { val x = 1 }\nfun top() {}\n",
            "val o = object {\n    val x = 1\n}\nfun top() {}\n",
            true,
        ),
        (
            "object after a package header",
            "package p\nobject O { fun g() = 2 }\nfun top() {}\n",
            "package p\nobject O {\n    fun g() = 2\n}\nfun top() {}\n",
            false,
        ),
        (
            "functional interface",
            "fun interface Cb { fun call() }\nfun top() {}\n",
            "fun interface Cb {\n    fun call()\n}\nfun top() {}\n",
            true,
        ),
        (
            "object expression with a supertype",
            "val o = object : Runnable { override fun run() {} }\nfun top() {}\n",
            "val o = object : Runnable {\n    override fun run() {}\n}\nfun top() {}\n",
            false,
        ),
        (
            "nested class",
            "class A { class B { fun f() {} } }\nfun top() {}\n",
            "class A {\n    class B {\n        fun f() {}\n    }\n}\nfun top() {}\n",
            true,
        ),
        (
            "members separated by semicolons",
            "class A { val x = 1; val y = 2 }\nfun top() {}\n",
            "class A {\n    val x = 1; val y = 2\n}\nfun top() {}\n",
            false,
        ),
        (
            "one-line lambdas around the class",
            "fun ui() { Button { click() } }\nclass A { fun m() { run { 1 } } }\nfun top() { if (a) { b() } else { c() } }\n",
            "fun ui() { Button { click() } }\nclass A {\n    fun m() { run { 1 } }\n}\nfun top() { if (a) { b() } else { c() } }\n",
            false,
        ),
    ]
}

/// Each shape comes back `Clean` and names what the multi-line twin names. The
/// grammar reads some of them as a visible ERROR (repaired by a `;`, recorded in
/// `recovered_decl_ranges`) and some it recovers by itself with a hidden
/// zero-width token (left as parsed, nothing recorded); which one it does
/// depends on the file, not on the shape alone, so the table says which.
#[test]
fn every_one_line_body_is_clean_and_equals_its_multi_line_twin() {
    for (label, one_line, twin, error_mode) in shapes() {
        let extraction = extract_file("A.kt", one_line);
        assert!(
            is_clean(&extraction),
            "{label}: {:?} {:?}",
            extraction.parse_outcome,
            extraction.diagnostics
        );
        assert_eq!(
            !extraction.recovered_decl_ranges.is_empty(),
            error_mode,
            "{label}: a repair is recorded exactly when the grammar reported an error"
        );
        let diffs = diff_against_twin(label, one_line, twin);
        assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    }
}

/// The companion case the ignored AC2 tests name: the member belongs to its
/// class, and the declarations after it are read. On `833cc524` the member is
/// `K.kt::f`, the class and `top` are lost.
#[test]
fn a_member_is_owned_by_its_class_and_later_declarations_are_read() {
    let extraction = extract_file(
        "K.kt",
        "class C { companion object { fun f() = 1 } }\nfun top() {}\nfun top2() = 2\n",
    );
    assert_eq!(
        names(&extraction),
        ["K.kt::C", "K.kt::C.f", "K.kt::top", "K.kt::top2"]
    );
    let f = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "K.kt::C.f")
        .unwrap();
    assert_eq!(f.kind, SymbolKind::Method);
}

/// The text is read through a rewritten tree, but every offset is the file's:
/// a symbol's span cuts the original text at its own start and end.
#[test]
fn spans_are_offsets_of_the_original_file() {
    let source = "class A { val x = 1 }\nclass Box { class In { val a = 1 } }\nfun after() {}\n";
    let extraction = extract_file("B.kt", source);
    assert!(is_clean(&extraction), "{:?}", extraction.diagnostics);
    let text = |name: &str| {
        let symbol = extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == name)
            .unwrap_or_else(|| panic!("{name} in {:?}", names(&extraction)));
        source[symbol.span.start_byte..symbol.span.end_byte].to_string()
    };
    assert_eq!(text("B.kt::A"), "class A { val x = 1 }");
    assert_eq!(text("B.kt::Box"), "class Box { class In { val a = 1 } }");
    assert_eq!(text("B.kt::Box.In"), "class In { val a = 1 }");
    assert_eq!(text("B.kt::after"), "fun after() {}");
    let ranges: Vec<&str> = extraction
        .recovered_decl_ranges
        .iter()
        .map(|range| &source[range.start_byte..range.end_byte])
        .collect();
    assert!(ranges.contains(&"class A { val x = 1 }"), "{ranges:?}");
    assert!(extraction.decl_gaps.is_empty());
}

/// A call inside a repaired member has the member as its caller, and a call in
/// a declaration after the class is read.
#[test]
fn calls_inside_a_repaired_member_belong_to_it() {
    let extraction = extract_file(
        "A.kt",
        "class C { companion object { fun f() = helper() } }\nfun helper() = 1\nfun after() { C.f() }\n",
    );
    assert!(is_clean(&extraction), "{:?}", extraction.diagnostics);
    assert!(!extraction.recovered_decl_ranges.is_empty());
    let calls: Vec<(Option<&str>, &str)> = extraction
        .calls
        .iter()
        .map(|call| (call.caller_symbol.as_deref(), call.callee_name.as_str()))
        .collect();
    assert!(calls.contains(&(Some("A.kt::C.f"), "helper")), "{calls:?}");
    assert!(calls.contains(&(Some("A.kt::after"), "f")), "{calls:?}");
}

/// No room for a separator: `}}` has no space to write a `;` over, and a
/// shifted insertion would move every later offset. The file is not repaired
/// and stays `Partial`, and it says why.
#[test]
fn a_body_with_no_space_before_its_closing_brace_stays_partial() {
    let source = "class A { fun f() {}}\nfun top() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
        "{:?}",
        extraction.parse_outcome
    );
    assert!(extraction.recovered_decl_ranges.is_empty());
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|note| note.contains("not repaired")),
        "{:?}",
        extraction.diagnostics
    );
}

/// A repair is kept only if the whole file then parses cleanly. A genuine syntax
/// error elsewhere keeps the file exactly as it was.
#[test]
fn a_file_with_another_error_is_not_repaired() {
    let source = "class A { val x = 1 }\nfun broken( {\nfun top() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(matches!(
        extraction.parse_outcome,
        ParseOutcome::Partial { .. }
    ));
    assert!(extraction.recovered_decl_ranges.is_empty());
    assert!(extraction
        .diagnostics
        .iter()
        .any(|note| note.contains("not repaired")));
}

/// A file that wants more separators than the cap is not rewritten, and says so.
#[test]
fn a_file_that_wants_too_many_separators_is_left_alone() {
    let mut source = String::new();
    for index in 0..600 {
        source.push_str(&format!("class C{index} {{ val x = 1 }}\n"));
    }
    source.push_str("fun top() {}\n");
    let extraction = extract_file("Big.kt", &source);
    assert!(
        matches!(extraction.parse_outcome, ParseOutcome::Partial { .. }),
        "{:?} {:?}",
        extraction.parse_outcome,
        extraction.diagnostics
    );
    assert!(extraction.recovered_decl_ranges.is_empty());
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|note| note.contains("separators would be written")),
        "{:?}",
        extraction.diagnostics
    );
}

/// The grammar recovers some of these shapes by itself with a hidden zero-width
/// token that no node shows: the class and every member are parsed and owned
/// correctly, and the file reads `Clean`. Such a file is not rewritten.
#[test]
fn a_file_the_grammar_recovered_by_itself_is_left_as_it_parsed() {
    let source = "class A {\n    fun a() { x() }\n    fun last() { x() } }\nfun top() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(is_clean(&extraction));
    assert!(extraction.recovered_decl_ranges.is_empty());
    assert_eq!(
        names(&extraction),
        ["A.kt::A", "A.kt::A.a", "A.kt::A.last", "A.kt::top"]
    );
    assert!(extraction
        .diagnostics
        .iter()
        .all(|note| !note.contains("one-line class body")));
}

/// A swallowed declaration (row 15) and a one-line body (row 16) in one file:
/// each is repaired, each by its own mechanism, and the file is `Clean`.
#[test]
fn a_stacked_annotation_statement_and_a_one_line_body_in_one_file() {
    let source = "class K { val x = 1 }\nfun helper() {}\n@A\n@B\nprivate fun f() { helper() }\nfun tail() {}\n";
    let extraction = extract_file("M.kt", source);
    assert!(
        is_clean(&extraction),
        "{:?} {:?}",
        extraction.parse_outcome,
        extraction.diagnostics
    );
    assert_eq!(
        names(&extraction),
        ["M.kt::K", "M.kt::f", "M.kt::helper", "M.kt::tail"]
    );
    let ranges: Vec<&str> = extraction
        .recovered_decl_ranges
        .iter()
        .map(|range| &source[range.start_byte..range.end_byte])
        .collect();
    assert!(ranges.contains(&"class K { val x = 1 }"), "{ranges:?}");
    assert!(
        ranges
            .iter()
            .any(|text| text.ends_with("private fun f() { helper() }")),
        "{ranges:?}"
    );
    assert!(extraction.decl_gaps.is_empty());
    let helper_callers: Vec<Option<&str>> = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "helper")
        .map(|call| call.caller_symbol.as_deref())
        .collect();
    assert_eq!(helper_callers, [Some("M.kt::f")]);
}

/// A file that parses cleanly is never rewritten: no repair is recorded and its
/// text reaches the extractor untouched.
#[test]
fn a_clean_file_is_not_rewritten() {
    let extraction = extract_file(
        "C.kt",
        "class A {\n    fun m() { run { 1 } }\n}\nfun top() {}\n",
    );
    assert!(is_clean(&extraction));
    assert!(extraction.recovered_decl_ranges.is_empty());
    assert!(extraction
        .diagnostics
        .iter()
        .all(|note| !note.contains("one-line class body")));
}

/// The repair is a pure function of the source.
#[test]
fn repair_is_deterministic() {
    let source = "class C { companion object { fun f() = 1 } }\nfun top() {}\n";
    let first = serde_json::to_string(&extract_file("K.kt", source)).unwrap();
    let second = serde_json::to_string(&extract_file("K.kt", source)).unwrap();
    assert_eq!(first, second);
}
