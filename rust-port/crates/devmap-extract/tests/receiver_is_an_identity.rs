//! X44 — a receiver is an identity, not a copy of the source.
//!
//! `ExtractedCall::receiver_expr` and `ExtractedReference::receiver_expr` were
//! filled with `get_node_text` of the receiver node, whole. For
//! `runner.invoke(app, ["init"]).output.strip()` that is the entire left-hand
//! expression; for a receiver that is itself a multi-line chain it is the whole
//! chain, newlines included.
//!
//! Measured on this repository before this change, over one generation's
//! `generation_unresolved`:
//!
//! * **13,387** rows carry a receiver that is an expression rather than a name;
//! * **4,973** receivers contain a newline;
//! * the longest is **38,644 characters** — the body of `Resolver::resolve_all`,
//!   stored as the "receiver" of a method called on the end of it;
//! * 1.17 MB of receiver text in total, of which 535 KB sits in the 3,323 rows
//!   longer than 64 characters.
//!
//! Two costs, and the second is the one that matters. The store carries a
//! megabyte of duplicated source. And the field is documented as existing "so
//! the classification can be audited rather than trusted" — which means being
//! *grouped* and read. A receiver that is a unique 38 KB string groups with
//! nothing and answers no question; every one of those rows is its own
//! singleton.
//!
//! The identity is the thing the classifier already reads: a receiver's leftmost
//! segment. So a receiver that is a call becomes that call's **callee name** —
//! `invoke` for `runner.invoke(app, ["init"])` — and everything else is bounded,
//! with the bound made visible rather than silent.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;

fn extract(path: &str, source: &str) -> Extraction {
    extract_file(path, source)
}

fn receiver_of<'a>(extraction: &'a Extraction, callee: &str) -> Option<&'a str> {
    extraction
        .calls
        .iter()
        .find(|call| call.callee_name == callee)
        .and_then(|call| call.receiver_expr.as_deref())
}

#[test]
fn a_receiver_that_is_a_call_is_named_by_that_calls_callee() {
    let extraction = extract(
        "t.py",
        "def check(runner, app):\n    return runner.invoke(app, [\"init\"]).output\n",
    );

    assert_eq!(
        receiver_of(&extraction, "output"),
        None,
        "`.output` is an attribute access, not a call, so it has no call row"
    );

    let extraction = extract(
        "t.py",
        "def check(runner, app):\n    return runner.invoke(app, [\"init\"]).strip()\n",
    );
    assert_eq!(
        receiver_of(&extraction, "strip"),
        Some("runner.invoke"),
        "the receiver of `.strip()` is what `runner.invoke(...)` returned. The \
         argument list is the part that made the string unique; the chain's \
         root is the part the classifier reads, and both must survive"
    );
}

#[test]
fn a_plain_receiver_is_left_exactly_as_it_was() {
    let extraction = extract("t.py", "def check(cfg):\n    return cfg.enabled.strip()\n");
    assert_eq!(
        receiver_of(&extraction, "strip"),
        Some("cfg.enabled"),
        "a dotted path of plain names is already an identity — the resolver \
         roots its classification at `cfg`, and nothing here may change that"
    );
}

#[test]
fn a_rust_chained_call_receiver_is_the_inner_callee() {
    let extraction = extract(
        "t.rs",
        "pub fn go(rows: &[u32]) -> String {\n    \
         rows.iter().map(|value| value + 1).collect::<Vec<_>>().len().to_string()\n}\n",
    );
    assert_eq!(
        receiver_of(&extraction, "to_string"),
        Some("rows.iter.map.collect.len"),
        "every link of the chain, and none of its arguments — one key per \
         distinct chain, rooted where the classifier looks"
    );
}

/// The bound, and the reason it is visible. A receiver this walk cannot reduce
/// to an identity is still recorded — the field's job is to let the
/// classification be audited — but it is bounded, and the truncation is marked
/// so a reader can never mistake a cut string for the whole expression.
#[test]
fn an_irreducible_receiver_is_bounded_and_says_so() {
    let long = "value_".repeat(60);
    let source = format!("def check(a, b):\n    return (a if b else {long}).strip()\n");
    let extraction = extract("t.py", &source);

    let receiver = receiver_of(&extraction, "strip").expect("the call has a receiver");
    assert!(
        receiver.chars().count() <= 72,
        "a receiver is an identity, not a copy of the expression; got {} chars",
        receiver.chars().count()
    );
    assert!(
        receiver.ends_with('\u{2026}'),
        "a truncated receiver must say it was truncated, or a cut string reads \
         as the whole expression; got {receiver:?}"
    );
}

/// No receiver may span lines. A newline in this field is the signature of a
/// whole block having been copied into it.
#[test]
fn no_receiver_spans_more_than_one_line() {
    let extraction = extract(
        "t.py",
        "def check(rows):\n    return [\n        row\n        for row in rows\n    ].pop().strip()\n",
    );

    for call in &extraction.calls {
        let Some(receiver) = call.receiver_expr.as_deref() else {
            continue;
        };
        assert!(
            !receiver.contains('\n'),
            "receiver of `{}` spans lines: {receiver:?}",
            call.callee_name
        );
    }
}

/// The same rule on the reference side, which fills the field from its own
/// helper and shares the same defect.
#[test]
fn a_reference_receiver_is_bounded_too() {
    let extraction = extract(
        "t.py",
        "def check(runner, app):\n    return runner.invoke(app, [\"init\"]).output\n",
    );

    let receiver = extraction
        .references
        .iter()
        .find(|reference| reference.name == "output")
        .and_then(|reference| reference.receiver_expr.as_deref())
        .expect("`.output` is a member access with a receiver");
    assert_eq!(
        receiver, "runner.invoke",
        "an attribute read off a call result names that call, not its argument list"
    );
}
