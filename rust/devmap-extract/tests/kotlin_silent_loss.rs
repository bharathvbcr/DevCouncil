//! extract-v98 row 15: a Kotlin declaration the grammar reads as an expression
//! is detected, and recovered where the re-parse can prove what it is.
//!
//! tree-sitter-kotlin-ng parses `@A @B private fun f() {}` as a nested
//! `annotated_expression` ending in an `infix_expression`, with no error node.
//! Before the loss detector the file was `Clean`, `f` was not a symbol, and no
//! consumer could tell. Every positive test here fails against `9ceef2dc`.
//!
//! The trigger, measured on the locked grammar (tree-sitter-kotlin-ng 1.1.0):
//! two or more annotations, then exactly one modifier-like word, then a
//! declaration that also reads as an expression, then another declaration (a
//! swallowed statement at the very end of a file parses correctly). It is not
//! specific to `fun`. The detector's own behaviour is pinned by the unit tests
//! in `src/kotlinloss.rs`; the tests here pin what a file reports.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, SymbolKind, TextRange};

/// Two annotations: the prefix that makes the grammar read what follows as an
/// expression.
const STACK: &str = "@Preview\n@Composable\n";

fn error_ranges(extraction: &Extraction) -> Vec<TextRange> {
    match &extraction.parse_outcome {
        ParseOutcome::Partial { error_ranges, .. } => error_ranges.clone(),
        other => panic!("expected a Partial outcome, got {other:?}"),
    }
}

fn range_of(source: &str, text: &str) -> TextRange {
    let start_byte = source.find(text).expect("text is in the source");
    TextRange {
        start_byte,
        end_byte: start_byte + text.len(),
    }
}

/// What each swallowed declaration kind becomes after recovery: the symbols it
/// emits, by qualified name and kind, and nothing else. A property and a type
/// alias are recovered declarations that emit no symbol.
#[test]
fn every_swallowed_declaration_kind_is_recovered_as_the_declaration_it_is() {
    for (declaration, want) in [
        ("private fun f() {}", vec![("f", SymbolKind::Function)]),
        ("private fun f() = 1", vec![("f", SymbolKind::Function)]),
        ("private class Q {}", vec![("Q", SymbolKind::Class)]),
        ("private object O {}", vec![("O", SymbolKind::Class)]),
        ("private interface I {}", vec![("I", SymbolKind::Interface)]),
        ("internal class Q", vec![("Q", SymbolKind::Class)]),
        ("private val y = 1", vec![]),
        ("private var y = 1", vec![]),
        ("private typealias T = Int", vec![]),
        ("open class O", vec![("O", SymbolKind::Class)]),
        ("sealed interface S", vec![("S", SymbolKind::Interface)]),
        ("abstract class AC {}", vec![("AC", SymbolKind::Class)]),
        ("class Plain {}", vec![("Plain", SymbolKind::Class)]),
    ] {
        let swallowed = format!("{STACK}{declaration}");
        let source = format!("{swallowed}\nfun after() {{}}\n");
        let extraction = extract_file("A.kt", &source);
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{declaration}: {:?}\n{:?}",
            extraction.parse_outcome,
            extraction.diagnostics
        );
        assert_eq!(
            extraction.recovered_decl_ranges,
            vec![range_of(&source, &swallowed)],
            "{declaration}: the repaired range is recorded"
        );
        assert!(
            extraction.decl_gaps.is_empty(),
            "{declaration}: a repaired range is not a gap: {:?}",
            extraction.decl_gaps
        );
        let mut got: Vec<(String, SymbolKind)> = extraction
            .symbols
            .iter()
            .filter(|symbol| symbol.kind != SymbolKind::File)
            .map(|symbol| (symbol.qualified_name.clone(), symbol.kind))
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        let mut expected: Vec<(String, SymbolKind)> = want
            .iter()
            .map(|(name, kind)| (format!("A.kt::{name}"), *kind))
            .collect();
        expected.push(("A.kt::after".to_string(), SymbolKind::Function));
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(got, expected, "{declaration}");
    }
}

/// The recovered symbol is the declaration, in the file's own coordinates:
/// its span starts at the first annotation and ends at the closing brace, and
/// it is private.
#[test]
fn a_recovered_symbol_spans_the_statement_and_keeps_its_visibility() {
    let swallowed = format!("{STACK}private fun f() {{}}");
    let source = format!("fun before() {{}}\n{swallowed}\nfun after() {{}}\n");
    let extraction = extract_file("A.kt", &source);
    let f = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "A.kt::f")
        .expect("f is recovered");
    let at = range_of(&source, &swallowed);
    assert_eq!(
        (f.span.start_byte, f.span.end_byte),
        (at.start_byte, at.end_byte)
    );
    assert!(
        !f.is_exported,
        "`private` is read from the recovered declaration"
    );
    assert_eq!(f.parent_symbol.as_deref(), Some("A.kt"));
}

/// A recovered member is owned by what the source says owns it, never by the
/// wrapper the re-parse put it in.
#[test]
fn a_recovered_class_keeps_its_members_and_names_no_wrapper() {
    let source =
        "@A\n@B\nprivate class Q {\n    fun m() {\n        helper()\n    }\n}\nfun helper() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(matches!(extraction.parse_outcome, ParseOutcome::Clean));
    let mut names: Vec<(&str, SymbolKind)> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| (symbol.qualified_name.as_str(), symbol.kind))
        .collect();
    names.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(
        names,
        vec![
            ("A.kt::Q", SymbolKind::Class),
            ("A.kt::Q.m", SymbolKind::Method),
            ("A.kt::helper", SymbolKind::Function),
        ]
    );
    let json = serde_json::to_string(&extraction).unwrap();
    assert!(
        !json.contains("Devmap"),
        "no trace of the wrapper in the payload"
    );
}

/// The original tree attributed the body's calls to the file and recorded
/// `f() { .. }` as a call to `f`. After recovery each call has the caller it
/// has in source, and the phantom call is gone.
#[test]
fn calls_inside_a_recovered_body_belong_to_the_recovered_function() {
    let source = "fun helper() {}\n@Preview\n@Composable\nprivate fun f() {\n    helper()\n}\nfun after() {\n    f()\n}\n";
    let extraction = extract_file("A.kt", source);
    assert!(matches!(extraction.parse_outcome, ParseOutcome::Clean));
    let calls: Vec<(Option<&str>, &str)> = extraction
        .calls
        .iter()
        .map(|call| (call.caller_symbol.as_deref(), call.callee_name.as_str()))
        .collect();
    assert_eq!(
        calls
            .iter()
            .filter(|(_, callee)| *callee == "helper")
            .collect::<Vec<_>>(),
        vec![&(Some("A.kt::f"), "helper")],
        "one call to helper, made by f: {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|(_, callee)| *callee == "f")
            .collect::<Vec<_>>(),
        vec![&(Some("A.kt::after"), "f")],
        "the only call to f is the one in after; `f() {{ .. }}` was the declaration: {calls:?}"
    );
    let body_call = extraction
        .calls
        .iter()
        .find(|call| call.callee_name == "helper")
        .unwrap();
    let f = extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "A.kt::f")
        .unwrap();
    assert!(
        f.span.start_byte <= body_call.span.start_byte
            && body_call.span.end_byte <= f.span.end_byte,
        "the call lies inside f in the file's coordinates"
    );
}

/// `(scope, local)` pairs and `(caller, callee)` pairs of an extraction.
type Scoped = (Vec<(String, String)>, Vec<(Option<String>, String)>);

/// What the extractor derives from a body (its locals and the lambdas inside
/// it) is filed under the function's own scope, the same string the function's
/// qualified name and its calls' `caller_symbol` use. A swallowed function and
/// an ordinary one with the same body must agree on every scope string.
#[test]
fn a_recovered_body_is_scoped_exactly_as_an_ordinary_one_is() {
    let body = "{\n    val tmp = 1\n    MaterialTheme {\n        helper(tmp)\n    }\n}";
    let swallowed =
        format!("fun helper(x: Int) {{}}\n@A\n@B\nprivate fun f() {body}\nfun after() {{}}\n");
    let ordinary =
        format!("fun helper(x: Int) {{}}\n@A\nprivate fun g() {body}\nfun after() {{}}\n");
    let recovered = extract_file("A.kt", &swallowed);
    let plain = extract_file("A.kt", &ordinary);
    assert!(
        matches!(recovered.parse_outcome, ParseOutcome::Clean),
        "{:?}",
        recovered.diagnostics
    );
    assert!(matches!(plain.parse_outcome, ParseOutcome::Clean));
    let scopes = |extraction: &Extraction, name: &str| -> Scoped {
        let mut locals: Vec<(String, String)> = extraction
            .scope_locals
            .iter()
            .map(|(scope, local)| (scope.replace(name, "FN"), local.clone()))
            .collect();
        locals.sort();
        let mut calls: Vec<(Option<String>, String)> = extraction
            .calls
            .iter()
            .map(|call| {
                (
                    call.caller_symbol
                        .as_ref()
                        .map(|caller| caller.replace(name, "FN")),
                    call.callee_name.clone(),
                )
            })
            .collect();
        calls.sort();
        (locals, calls)
    };
    assert_eq!(scopes(&recovered, "::f"), scopes(&plain, "::g"));
    assert!(
        recovered
            .scope_locals
            .iter()
            .any(|(scope, local)| scope == "A.kt::f" && local == "tmp"),
        "{:?}",
        recovered.scope_locals
    );
    let json = serde_json::to_string(&recovered).unwrap();
    assert!(
        !json.contains("Devmap"),
        "no trace of the wrapper in the payload"
    );
}

/// A recovered property or alias emits no symbol, and the calls in its
/// initializer keep the file as their caller, as they would had the grammar
/// read it correctly.
#[test]
fn a_recovered_property_initializer_keeps_its_calls_at_file_scope() {
    let source = "fun helper() = 1\n@A\n@B\nprivate val y = helper()\nfun after() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(matches!(extraction.parse_outcome, ParseOutcome::Clean));
    assert_eq!(
        extraction.recovered_decl_ranges,
        vec![range_of(source, "@A\n@B\nprivate val y = helper()")],
        "the statement was swallowed and recovered, not merely read"
    );
    let helper_calls: Vec<Option<&str>> = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "helper")
        .map(|call| call.caller_symbol.as_deref())
        .collect();
    assert_eq!(helper_calls.len(), 1, "{helper_calls:?}");
}

/// A swallowed statement the detector charged but the re-parse cannot prove
/// stays exactly as it was: `Partial`, with nothing emitted for it. Here the
/// body of the swallowed function holds a swallowed statement of its own, so the
/// wrapper still contains one.
#[test]
fn a_statement_the_reparse_cannot_prove_stays_partial_and_emits_nothing() {
    let inner = "@C\n@D\nprivate fun g() {}";
    let outer = format!("@A\n@B\nprivate fun f() {{\n{inner}\nlast()\n}}");
    let source = format!("{outer}\nfun after() {{}}\n");
    let extraction = extract_file("A.kt", &source);
    let ParseOutcome::Partial { error_ranges, .. } = &extraction.parse_outcome else {
        panic!("expected Partial: {:?}", extraction.parse_outcome);
    };
    assert!(
        error_ranges.contains(&range_of(&source, &outer)),
        "the unrecovered statement stays charged: {error_ranges:?}"
    );
    assert!(
        extraction.decl_gaps.contains(&range_of(&source, &outer)),
        "the unrecovered statement is a gap: {:?}",
        extraction.decl_gaps
    );
    assert!(extraction.recovered_decl_ranges.is_empty());
    let names: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert!(
        !names
            .iter()
            .any(|name| name.ends_with("::f") || name.ends_with("::g")),
        "nothing was emitted for the unproven statement: {names:?}"
    );
    assert!(names.contains(&"A.kt::after"), "{names:?}");
}

/// A swallowed statement inside a function body is not the shape recovery was
/// measured on. It is charged, not recovered.
#[test]
fn a_swallowed_statement_inside_a_function_stays_charged() {
    let swallowed = format!("{STACK}private fun h() {{}}");
    let source = format!("fun g() {{\n{swallowed}\nlast()\n}}\nfun after() {{}}\n");
    let extraction = extract_file("A.kt", &source);
    assert_eq!(
        error_ranges(&extraction),
        vec![range_of(&source, &swallowed)]
    );
    assert_eq!(extraction.decl_gaps, vec![range_of(&source, &swallowed)]);
    assert!(extraction.recovered_decl_ranges.is_empty());
}

/// Each swallowed statement is its own region, recovered on its own.
#[test]
fn each_swallowed_statement_is_recovered_on_its_own() {
    let first = "@A\n@B\nprivate fun f() {}";
    let second = "@A\n@B\nprivate class Q {}";
    let source = format!("{first}\nfun middle() {{}}\n{second}\nfun last() {{}}\n");
    let extraction = extract_file("M.kt", &source);
    assert!(matches!(extraction.parse_outcome, ParseOutcome::Clean));
    assert_eq!(
        extraction.recovered_decl_ranges,
        vec![range_of(&source, first), range_of(&source, second)]
    );
    assert!(extraction.decl_gaps.is_empty());
    let mut names: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["M.kt::Q", "M.kt::f", "M.kt::last", "M.kt::middle"]);
}

/// The first statement of a file and the statement after a function lose the
/// same way and are recovered the same way.
#[test]
fn the_statement_is_recovered_wherever_it_sits() {
    for (name, source) in [
        (
            "first statement",
            "@Preview\n@Composable\nprivate fun f() {}\nfun after() {}\n",
        ),
        (
            "after a function",
            "fun before() {}\n@Preview(showBackground = true)\n@Composable\nprivate fun f() {}\nfun after() {}\n",
        ),
        (
            "after a multi-line initializer and a blank line",
            "private val X = listOf(1, 2).map {\n    it + 1\n}\n\n@Preview\n@Composable\nprivate fun f() {}\nfun after() {}\n",
        ),
    ] {
        let extraction = extract_file("S.kt", source);
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{name}: {:?}",
            extraction.parse_outcome
        );
        assert!(
            extraction
                .symbols
                .iter()
                .any(|symbol| symbol.qualified_name == "S.kt::f"),
            "{name}"
        );
    }
}

/// Regions past the cap are not re-parsed. They stay charged, and the cap is
/// said in the diagnostics rather than applied silently.
#[test]
fn recovery_stops_at_its_region_cap_and_says_so() {
    let mut source = String::new();
    for index in 0..70 {
        source.push_str(&format!(
            "@A\n@B\nprivate fun f{index}() {{}}\nfun keep{index}() {{}}\n"
        ));
    }
    let extraction = extract_file("Many.kt", &source);
    let ranges = error_ranges(&extraction);
    assert_eq!(ranges.len(), 6, "70 regions, 64 recovered: {ranges:?}");
    assert_eq!(extraction.recovered_decl_ranges.len(), 64);
    assert_eq!(
        extraction.decl_gaps, ranges,
        "every unrecovered range is both charged and a gap"
    );
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|note| note.contains("stopped at 64 regions")),
        "{:?}",
        extraction.diagnostics
    );
    for index in 0..64 {
        assert!(extraction
            .symbols
            .iter()
            .any(|symbol| symbol.qualified_name == format!("Many.kt::f{index}")));
    }
    assert!(!extraction
        .symbols
        .iter()
        .any(|symbol| symbol.qualified_name == "Many.kt::f69"));
}

/// A source that spells the wrapper's own name is not recovered at all.
#[test]
fn a_source_that_spells_the_wrappers_name_is_left_charged() {
    let source =
        "val DevmapKotlinRecoveryWrapper = 1\n@A\n@B\nprivate fun f() {}\nfun after() {}\n";
    let extraction = extract_file("A.kt", source);
    assert_eq!(error_ranges(&extraction).len(), 1);
    assert_eq!(extraction.decl_gaps, error_ranges(&extraction));
    assert!(extraction.recovered_decl_ranges.is_empty());
    assert!(!extraction
        .symbols
        .iter()
        .any(|symbol| symbol.qualified_name == "A.kt::f"));
}

/// A file whose own first bytes spell the wrapper's header is an ordinary file
/// with an ordinary class: the declaration owner stops treating that class as
/// the wrapper unless a recovery is running.
#[test]
fn a_real_class_that_spells_the_wrappers_header_is_an_ordinary_class() {
    let source = "class DevmapKotlinRecoveryWrapper {\n    fun m() {}\n}\nfun after() {}\n";
    let extraction = extract_file("A.kt", source);
    assert!(matches!(extraction.parse_outcome, ParseOutcome::Clean));
    let mut names: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "A.kt::DevmapKotlinRecoveryWrapper",
            "A.kt::DevmapKotlinRecoveryWrapper.m",
            "A.kt::after"
        ]
    );
}

/// A one-line class body with a member is a whole-file grammar ERROR, so a
/// swallowed statement that holds one cannot be re-parsed: the wrapper has a
/// syntax error. It stays charged and nothing is emitted for it.
#[test]
fn a_swallowed_class_with_a_one_line_body_stays_charged() {
    let swallowed = format!("{STACK}private class Q {{ fun m() {{}}}}");
    let source = format!("{swallowed}\nfun after() {{}}\n");
    let extraction = extract_file("A.kt", &source);
    assert_eq!(
        error_ranges(&extraction),
        vec![range_of(&source, &swallowed)]
    );
    assert_eq!(extraction.decl_gaps, vec![range_of(&source, &swallowed)]);
    assert!(extraction.recovered_decl_ranges.is_empty());
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|note| note.contains("not recovered")),
        "{:?}",
        extraction.diagnostics
    );
}

/// Recovery is a pure function of the source.
#[test]
fn recovery_is_deterministic() {
    let source = "fun helper() {}\n@A\n@B\nprivate fun f() { helper() }\n@A\n@B\nprivate class Q {\n    fun m() = helper()\n}\nfun after() {}\n";
    assert_eq!(
        extract_file("A.kt", source).recovered_decl_ranges.len(),
        2,
        "both recovered"
    );
    let first = serde_json::to_string(&extract_file("A.kt", source)).unwrap();
    let second = serde_json::to_string(&extract_file("A.kt", source)).unwrap();
    assert_eq!(first, second);
}

/// What the grammar reads as a declaration, and what only looks like the
/// trigger, stays `Clean`.
#[test]
fn what_the_grammar_reads_correctly_stays_clean() {
    for (name, source) in [
        (
            "a single annotation",
            "private val X = 1\n@Preview\nprivate fun f() {}\nfun after() {}\n",
        ),
        (
            "two annotations and no modifier before fun",
            "private val X = 1\n@Preview\n@Composable\nfun f() {}\nfun after() {}\n",
        ),
        (
            "an expression body with a return type",
            "@Preview\n@Composable\nprivate fun f(): Int = 1\n",
        ),
        (
            "two modifiers",
            "@Preview\n@Composable\nprivate suspend fun s() {}\n",
        ),
        ("a data class", "@A\n@B\ndata class D(val a: Int)\n"),
        (
            "a class body member",
            "class K {\n    @A\n    @B\n    private fun m() {}\n}\n",
        ),
        ("backticked keywords", "fun `fun`() {}\nval `class` = 1\n"),
        (
            "keywords in a string, a line comment, a block comment and KDoc",
            "val s = \"private fun f() {}\"\n// @A @B private fun f() {}\n/* class X */\n/** @A @B private fun g() {} */\nfun h() {}\n",
        ),
        (
            "class references",
            "val k = String::class.java\nfun <T : Any> name(c: kotlin.reflect.KClass<T>) = T::class\n",
        ),
        (
            "an interface and an anonymous object",
            "interface Cb {\n    fun call()\n}\nval o = object {\n    val x = 1\n}\n",
        ),
        (
            "ordinary declarations",
            "package p\nimport a.B\nobject O {\n    fun g() = 2\n}\nenum class E {\n    X, Y\n}\ntypealias T = Int\nclass C(val a: Int) {\n    val b = 1\n    fun m() {}\n}\nfun top() {}\n",
        ),
    ] {
        let extraction = extract_file("C.kt", source);
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{name}: {:?}",
            extraction.parse_outcome
        );
    }
}

/// A genuine syntax error keeps its own ranges: recovering the swallowed
/// statement beside it removes only the swallowed statement's range.
#[test]
fn a_genuine_error_is_not_cleared_by_recovering_a_statement_beside_it() {
    let swallowed = "@A\n@B\nprivate fun f() {}";
    let source = format!("{swallowed}\nfun middle() {{}}\nfun broken( {{\n");
    let extraction = extract_file("E.kt", &source);
    let ranges = error_ranges(&extraction);
    assert!(
        !ranges.contains(&range_of(&source, swallowed)),
        "the swallowed statement was recovered and is no longer charged: {ranges:?}"
    );
    assert!(
        ranges
            .iter()
            .any(|range| range.start_byte >= source.find("fun broken").unwrap()),
        "the genuine error keeps its range: {ranges:?}"
    );
    assert_eq!(
        extraction.recovered_decl_ranges,
        vec![range_of(&source, swallowed)]
    );
    assert!(
        extraction.decl_gaps.is_empty(),
        "the genuine error is charged but is not a swallowed declaration"
    );
    assert!(extraction
        .symbols
        .iter()
        .any(|symbol| symbol.qualified_name == "E.kt::f"));
}

// ------------------------------------------- the opus review of 974edaa3

fn qualified(extraction: &Extraction) -> Vec<&str> {
    let mut names: Vec<&str> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    names.sort();
    names
}

/// A region the recovery cannot prove stays charged, and the expression
/// reading of it is not published either: the member `m` of a swallowed class
/// is not a top-level `A.kt::m`, and the swallowed declaration is not a call to
/// `class` or to `Q`. Fails against `974edaa3`, which kept all of it.
#[test]
fn an_unrecovered_region_publishes_nothing_the_expression_reading_made_of_it() {
    let swallowed = format!("{STACK}private class Q {{ fun m() {{}}}}");
    let source = format!("{swallowed}\nfun tail() {{}}\n");
    let extraction = extract_file("A.kt", &source);
    assert_eq!(
        error_ranges(&extraction),
        vec![range_of(&source, &swallowed)]
    );
    assert_eq!(extraction.decl_gaps, error_ranges(&extraction));
    assert_eq!(qualified(&extraction), ["A.kt::tail"]);
    let region = range_of(&source, &swallowed);
    let inside = |start: usize, end: usize| region.start_byte <= start && end <= region.end_byte;
    let calls: Vec<&str> = extraction
        .calls
        .iter()
        .filter(|call| inside(call.span.start_byte, call.span.end_byte))
        .map(|call| call.callee_name.as_str())
        .collect();
    assert!(calls.is_empty(), "calls from the charged region: {calls:?}");
    let references: Vec<&str> = extraction
        .references
        .iter()
        .filter(|reference| inside(reference.span.start_byte, reference.span.end_byte))
        .map(|reference| reference.name.as_str())
        .collect();
    assert!(
        references.is_empty(),
        "references from the charged region: {references:?}"
    );
    assert!(extraction
        .exports
        .iter()
        .all(|export| !inside(export.span.start_byte, export.span.end_byte)));
}

/// No call, anywhere in a Kotlin extraction, is named for a hard keyword: the
/// grammar's reading of a swallowed declaration as `private fun f() {}` is a call
/// to `fun`, and whether the region is recovered or not it must not be published.
#[test]
fn no_call_is_named_for_a_declaration_keyword() {
    for declaration in [
        "private fun f() {}",
        "private class Q {}",
        "private object O {}",
        "private interface I {}",
        "private val y = 1",
        "private typealias T = Int",
        "class Plain {}",
        "private class Q { fun m() {} }",
        "private object O { val x = 1 }",
    ] {
        let source = format!("{STACK}{declaration}\nfun tail() {{}}\n");
        let extraction = extract_file("A.kt", &source);
        let named: Vec<&str> = extraction
            .calls
            .iter()
            .map(|call| call.callee_name.as_str())
            .filter(|name| {
                [
                    "fun",
                    "class",
                    "interface",
                    "object",
                    "val",
                    "var",
                    "typealias",
                ]
                .contains(name)
            })
            .collect();
        assert!(named.is_empty(), "{declaration}: {named:?}");
    }
}

/// The same holds when the charged region sits inside a function body, where
/// recovery is never attempted: what the region made is dropped, what the
/// function does around it is kept.
#[test]
fn an_unrecovered_nested_region_is_dropped_and_its_surroundings_kept() {
    let region = format!("{STACK}private fun h() {{\n        inner()\n    }}");
    let source =
        format!("fun g() {{\n    before()\n    {region}\n    after()\n}}\nfun tail() {{}}\n");
    let extraction = extract_file("A.kt", &source);
    assert_eq!(error_ranges(&extraction), vec![range_of(&source, &region)]);
    assert_eq!(extraction.decl_gaps, error_ranges(&extraction));
    let callees: Vec<(Option<&str>, &str)> = extraction
        .calls
        .iter()
        .map(|call| (call.caller_symbol.as_deref(), call.callee_name.as_str()))
        .collect();
    assert!(
        callees.contains(&(Some("A.kt::g"), "before")),
        "{callees:?}"
    );
    assert!(callees.contains(&(Some("A.kt::g"), "after")), "{callees:?}");
    for phantom in ["fun", "h", "inner"] {
        assert!(
            !callees.iter().any(|(_, callee)| *callee == phantom),
            "`{phantom}` came from the charged region: {callees:?}"
        );
    }
}

/// A scope the phantom shared with a real symbol belongs to the real one: the
/// expression reading files the member `m` of a swallowed class as a top-level
/// `A.kt::m`, and a real top-level `m` keeps its scope locals.
#[test]
fn dropping_a_phantom_keeps_the_scope_of_a_real_symbol_of_the_same_name() {
    let source = format!(
        "{STACK}private class Q {{ fun m(a: Int) {{}}}}\nfun m(b: Int) {{\n    val z = 1\n}}\nfun tail() {{}}\n"
    );
    let extraction = extract_file("A.kt", &source);
    assert!(!error_ranges(&extraction).is_empty());
    assert!(
        extraction
            .scope_locals
            .iter()
            .any(|(scope, local)| scope == "A.kt::m" && local == "z"),
        "{:?}",
        extraction.scope_locals
    );
}

/// A class recovered whole holds a nested statement the outer tree also reads
/// as an expression (`companion object { .. }`, a stacked-annotation member).
/// That range is inside a recovered one, so it is not a separate loss: the file
/// is `Clean`. Fail against `974edaa3`, which matched ranges exactly.
#[test]
fn a_recovered_class_with_a_nested_charged_range_is_clean() {
    for (name, source) in [
        (
            "companion object",
            "@A\n@B\nprivate class Q {\n    companion object {\n        fun create() = Q()\n    }\n}\nfun after() {}\n",
        ),
        (
            "stacked member",
            "@A\n@B\nprivate class Q {\n    @X\n    @Y\n    private fun m() {}\n}\nfun after() {}\n",
        ),
    ] {
        let extraction = extract_file("A.kt", source);
        assert_eq!(extraction.recovered_decl_ranges.len(), 1, "{name}");
        assert!(extraction.decl_gaps.is_empty(), "{name}");
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{name}: {:?}",
            extraction.parse_outcome
        );
    }
    let extraction = extract_file(
        "A.kt",
        "@A\n@B\nprivate class Q {\n    @X\n    @Y\n    private fun m() {}\n}\nfun after() {}\n",
    );
    assert!(qualified(&extraction).contains(&"A.kt::Q.m"));
}

/// The outer tree's rows for a swallowed class's member are not kept beside the
/// recovered ones: `scope_locals` names `A.kt::P.m`, never the `A.kt::m` the
/// expression reading made up.
#[test]
fn a_recovered_member_leaves_no_ghost_scope_local() {
    let source = "@A\n@B\nprivate class P {\n    fun m(x: Foo): Base = x\n}\nfun after() {}\nclass Foo\nopen class Base\n";
    let extraction = extract_file("A.kt", source);
    assert_eq!(extraction.recovered_decl_ranges.len(), 1);
    assert!(
        extraction
            .scope_locals
            .iter()
            .all(|(scope, _)| scope != "A.kt::m"),
        "{:?}",
        extraction.scope_locals
    );
}

/// A comment between the colons and `class` does not make a class literal a
/// lost declaration.
#[test]
fn a_comment_between_the_colons_and_class_is_not_a_lost_declaration() {
    for source in [
        "val a = String:: /* c */ class\nval b = 1\n",
        "val a = String::\n// c\nclass\nval b = 1\n",
    ] {
        let extraction = extract_file("A.kt", source);
        assert!(
            matches!(extraction.parse_outcome, ParseOutcome::Clean),
            "{source:?}: {:?}",
            extraction.parse_outcome
        );
    }
}
