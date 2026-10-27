//! extract-v98 row 15: a Kotlin declaration the grammar reads as an expression
//! is no longer published `Clean`.
//!
//! tree-sitter-kotlin-ng parses `@A @B private fun f() {}` as a nested
//! `annotated_expression` ending in an `infix_expression`, with no error node.
//! Before the loss detector the file was `Clean`, `f` was not a symbol, and no
//! consumer could tell. Every test here fails against `9ceef2dc`, where the
//! outcome is `Clean`.
//!
//! The trigger, measured on the locked grammar (tree-sitter-kotlin-ng 1.1.0):
//! two or more annotations, then exactly one modifier-like word, then a
//! declaration that also reads as an expression. It is not specific to `fun`.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ParseOutcome, TextRange};

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

/// Every declaration kind behind a two-annotation stack, with the statement the
/// detector must charge. A trailing `fun after() {}` proves the rest of the file
/// is still read.
#[test]
fn every_declaration_the_grammar_swallows_makes_the_file_partial() {
    for declaration in [
        "private fun f() {}",
        "private fun f() = 1",
        "private class Q {}",
        "private object O {}",
        "private interface I {}",
        "internal class Q",
        "private val y = 1",
        "private var y = 1",
        "private typealias T = Int",
        "open class O",
        "sealed interface S",
        "enum class E { X }",
        "abstract class AC {}",
        "class Plain {}",
    ] {
        let swallowed = format!("{STACK}{declaration}");
        let source = format!("{swallowed}\nfun after() {{}}\n");
        let extraction = extract_file("A.kt", &source);
        assert_eq!(
            error_ranges(&extraction),
            vec![range_of(&source, &swallowed)],
            "{declaration}"
        );
        assert!(
            extraction
                .symbols
                .iter()
                .any(|symbol| symbol.qualified_name == "A.kt::after"),
            "{declaration}: the declaration after the swallowed one is still read"
        );
    }
}

/// The three shapes the audit and the ignored tests name: after a property, as
/// the first statement, and after a function.
#[test]
fn the_loss_is_charged_wherever_the_statement_sits() {
    for (name, source, swallowed) in [
        (
            "after a property",
            "private val X = 1\n@Preview\n@Composable\nprivate fun f() {}\nfun after() {}\n",
            "@Preview\n@Composable\nprivate fun f() {}",
        ),
        (
            "first statement",
            "@Preview\n@Composable\nprivate fun f() {}\nfun after() {}\n",
            "@Preview\n@Composable\nprivate fun f() {}",
        ),
        (
            "after a function",
            "fun before() {}\n@Preview(showBackground = true)\n@Composable\nprivate fun f() {}\nfun after() {}\n",
            "@Preview(showBackground = true)\n@Composable\nprivate fun f() {}",
        ),
        (
            "after a multi-line initializer and a blank line",
            "private val X = listOf(1, 2).map {\n    it + 1\n}\n\n@Preview\n@Composable\nprivate fun f() {}\nfun after() {}\n",
            "@Preview\n@Composable\nprivate fun f() {}",
        ),
    ] {
        let extraction = extract_file("S.kt", source);
        assert_eq!(
            error_ranges(&extraction),
            vec![range_of(source, swallowed)],
            "{name}"
        );
    }
}

/// Two swallowed statements in one file are two ranges, each charged once.
#[test]
fn each_swallowed_statement_is_its_own_range() {
    let first = "@A\n@B\nprivate fun f() {}";
    let second = "@A\n@B\nprivate class Q {}";
    let source = format!("{first}\nfun middle() {{}}\n{second}\nfun last() {{}}\n");
    let extraction = extract_file("M.kt", &source);
    assert_eq!(
        error_ranges(&extraction),
        vec![range_of(&source, first), range_of(&source, second)]
    );
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

/// A genuine syntax error keeps its own ranges, and a swallowed statement is
/// charged beside them rather than instead of them.
#[test]
fn a_swallowed_statement_is_charged_beside_a_genuine_error() {
    let swallowed = "@A\n@B\nprivate fun f() {}";
    let source = format!("{swallowed}\nfun broken( {{\n");
    let extraction = extract_file("E.kt", &source);
    let ranges = error_ranges(&extraction);
    assert!(
        ranges.contains(&range_of(&source, swallowed)),
        "the swallowed statement is charged: {ranges:?}"
    );
    assert!(
        ranges
            .iter()
            .any(|range| range.start_byte >= source.find("fun broken").unwrap()),
        "the genuine error keeps its range: {ranges:?}"
    );
}
