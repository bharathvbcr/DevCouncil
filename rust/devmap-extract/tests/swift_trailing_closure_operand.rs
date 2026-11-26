//! A trailing closure after the right operand of an operator, under
//! tree-sitter-swift 0.7.4.
//!
//! Swift gives a trailing closure to the postfix expression just before it, so
//! `x ?? y.foo { }` is `x ?? (y.foo { })`. tree-sitter-swift 0.7.4 binds a
//! trailing closure with no parenthesised arguments over the whole operator
//! expression instead: `call_expression(nil_coalescing_expression(x, y.foo),
//! { })`, and the same for `&&`, `||`, comparisons, equality, ranges, custom
//! operators, the ternary's last branch and `try`. Read as written, the call's
//! callee was the operator expression, so `foo` on `y` was never recorded, and
//! `x ?? foo { }.bar()` published `bar` on the receiver `x ?? foo { }`.
//!
//! Every bare spelling here must publish what its parenthesised twin does.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;

fn calls(extraction: &Extraction) -> Vec<(String, Option<String>)> {
    let mut calls: Vec<_> = extraction
        .calls
        .iter()
        .map(|call| (call.callee_name.clone(), call.receiver_expr.clone()))
        .collect();
    calls.sort();
    calls
}

fn in_function(expression: &str) -> String {
    format!("func f() {{\n    _ = {expression}\n}}\n")
}

/// `(bare, twin)`: the operator spelling and the same expression with its right
/// operand parenthesised, which every grammar version reads correctly.
const SHAPES: &[(&str, &str)] = &[
    ("x ?? y.foo { }", "x ?? (y.foo { })"),
    ("x || y.foo { }", "x || (y.foo { })"),
    ("x && y.foo { }", "x && (y.foo { })"),
    ("x < y.foo { }", "x < (y.foo { })"),
    ("x == y.foo { }", "x == (y.foo { })"),
    ("x ... y.foo { }", "x ... (y.foo { })"),
    ("x <+> y.foo { }", "x <+> (y.foo { })"),
    ("c ? x : y.foo { }", "c ? x : (y.foo { })"),
    ("x + y.foo { }", "x + (y.foo { })"),
    ("try x ?? y.foo { }", "try x ?? (y.foo { })"),
    ("b as? T ?? y.foo { }", "b as? T ?? (y.foo { })"),
    ("x ?? y ?? z.foo { }", "x ?? y ?? (z.foo { })"),
];

#[test]
fn a_trailing_closure_after_an_operator_is_a_call_of_the_right_operand() {
    let mut failures = Vec::new();
    for (bare, twin) in SHAPES {
        let bare_calls = calls(&extract_file("a.swift", &in_function(bare)));
        let twin_calls = calls(&extract_file("a.swift", &in_function(twin)));
        let receiver = if bare.contains("z.foo") { "z" } else { "y" };
        let foo = ("foo".to_string(), Some(receiver.to_string()));
        if !bare_calls.contains(&foo) {
            failures.push(format!("{bare}: no call {foo:?} in {bare_calls:?}"));
        }
        // The custom operator is a call of its own; its twin records it too.
        if bare_calls != twin_calls {
            failures.push(format!(
                "{bare}: {bare_calls:?} but its twin {twin}: {twin_calls:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_member_call_after_the_closure_takes_the_operand_as_its_receiver() {
    let mut failures = Vec::new();
    for (bare, twin, receiver) in [
        ("x ?? foo { }.bar()", "x ?? (foo { }.bar())", "foo { }"),
        (
            "x || y.foo { }.bar()",
            "x || (y.foo { }.bar())",
            "y.foo { }",
        ),
        (
            "b as? T ?? .foo { }.bar()",
            "b as? T ?? (.foo { }.bar())",
            ".foo { }",
        ),
        (
            "b as? T ?? .foo { } .bar { }",
            "b as? T ?? (.foo { } .bar { })",
            ".foo { }",
        ),
    ] {
        let bare_calls = calls(&extract_file("a.swift", &in_function(bare)));
        let twin_calls = calls(&extract_file("a.swift", &in_function(twin)));
        let bar = ("bar".to_string(), Some(receiver.to_string()));
        if !bare_calls.contains(&bar) {
            failures.push(format!("{bare}: no call {bar:?} in {bare_calls:?}"));
        }
        if bare_calls.iter().any(|(_, receiver)| {
            receiver
                .as_deref()
                .is_some_and(|r| r.contains("??") || r.contains("||"))
        }) {
            failures.push(format!("{bare}: an operator in a receiver: {bare_calls:?}"));
        }
        if bare_calls != twin_calls {
            failures.push(format!(
                "{bare}: {bare_calls:?} but its twin {twin}: {twin_calls:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A leading-dot member still names no callee, closure or not: the bare form
/// abstains exactly as its twin does.
#[test]
fn a_leading_dot_operand_with_a_closure_still_abstains() {
    for (bare, twin) in [
        ("x ?? .foo { }", "x ?? (.foo { })"),
        ("x == .foo { }", "x == (.foo { })"),
    ] {
        let bare_calls = calls(&extract_file("a.swift", &in_function(bare)));
        let twin_calls = calls(&extract_file("a.swift", &in_function(twin)));
        assert_eq!(bare_calls, twin_calls, "{bare} against {twin}");
        assert!(
            bare_calls.iter().all(|(name, _)| name != "foo"),
            "{bare}: {bare_calls:?}"
        );
    }
}

/// The body of `if`/`guard`/`while` after an operator condition is a block, not
/// a trailing closure, and the grammar reads it so: the call inside is recorded
/// with its own name and nothing else is invented.
#[test]
fn a_control_flow_body_after_an_operator_condition_is_not_a_closure() {
    let extraction = extract_file(
        "a.swift",
        "func f() {\n    if a ?? b { g() }\n    while x < y.count { h() }\n    guard p == q else { return }\n}\n",
    );
    let calls = calls(&extraction);
    for body_call in [("g".to_string(), None), ("h".to_string(), None)] {
        assert!(
            calls.contains(&body_call),
            "{body_call:?} missing: {calls:?}"
        );
    }
    // Only the operator calls the comparison arms record may join them.
    for (name, _) in &calls {
        assert!(
            matches!(name.as_str(), "g" | "h" | "<" | "=="),
            "invented a call {name}: {calls:?}"
        );
    }
}
