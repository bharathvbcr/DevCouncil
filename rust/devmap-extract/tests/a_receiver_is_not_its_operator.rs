//! `*path` is not a receiver; `path` is.
//!
//! The classifier and the receiver-type rung both read a receiver's leftmost
//! *segment*, and a leading `&`, `*` or `!` is not one. Every such row reached
//! `UninferredReceiver` with the binding that would have typed it sitting one
//! character to the right: 370 on this repository (`*path`, `*name`, `&…`) and
//! 107 on a 306-file Swift corpus, where tree-sitter-swift makes `!Self` the
//! navigation target of `!Self.containsNul(v)` — a call to the enclosing type's
//! own static method that the `self` rung therefore could not see.
//!
//! The rule is "strip only when what is left is a plain path", and the
//! restriction carries the whole argument: a reference or a dereference denotes
//! the *same value* as its operand, so the operand's declared type is the
//! receiver's. `!name.is_empty()` denotes a `bool` computed from a call, and
//! rooting it at `name` would claim the method belongs to whatever `name` is.
//! Half this file exists to hold that line.

use devmap_extract::extract_file;

fn receiver_of(path: &str, source: &str, callee: &str) -> Option<String> {
    extract_file(path, source)
        .calls
        .into_iter()
        .find(|call| call.callee_name == callee)
        .and_then(|call| call.receiver_expr)
}

/// A dereference, a reference, and a reference-to-mutable all name one binding.
#[test]
fn a_reference_or_dereference_reduces_to_its_binding() {
    for (source, callee, expected) in [
        ("fn g(p: &&str) { let s = (*p).zzto(); }", "zzto", "p"),
        ("fn g(x: &Foo) { (&x).zzuse(); }", "zzuse", "x"),
        ("fn g(x: Foo) { (&mut x).zzuse(); }", "zzuse", "x"),
        ("fn g(p: &&&str) { (**p).zzto(); }", "zzto", "p"),
    ] {
        assert_eq!(
            receiver_of("a.rs", source, callee),
            Some(expected.to_string()),
            "{source}"
        );
    }
}

/// Swift's `!Self.f(v)` records `!Self`, and `Self` is what it means.
#[test]
fn a_swift_negated_static_call_reduces_to_self() {
    assert_eq!(
        receiver_of(
            "a.swift",
            "func g() { if !Self.zzcheck(x) { return } }",
            "zzcheck"
        ),
        Some("Self".to_string()),
        "the enclosing type's own static method must be reachable through the \
         `self` rung, which reads the receiver as a name"
    );
}

/// ...and an unnegated one is untouched, so the reduction changed nothing there.
#[test]
fn an_unnegated_swift_static_call_is_unchanged() {
    assert_eq!(
        receiver_of("a.swift", "func g() { _ = Self.zzplain(x) }", "zzplain"),
        Some("Self".to_string())
    );
}

/// `$` and `#` are name characters in the languages that spell bindings that
/// way, so `!$0` reduces like `!flag` does.
#[test]
fn a_sigil_binding_is_a_name() {
    assert_eq!(
        receiver_of(
            "a.swift",
            "func g(items: [A]) { _ = items.filter { !$0.zzok() } }",
            "zzok"
        ),
        Some("$0".to_string()),
        "a Swift closure parameter is a binding, not an operator"
    );
    assert_eq!(
        receiver_of("a.php", "<?php function g($r) { $r->zzuse(); }", "zzuse"),
        Some("$r".to_string()),
        "every PHP variable starts with a sigil, and an untouched one stays so"
    );
    // A parenthesised operand is **not** reduced, and that is the rule doing
    // its job rather than a gap in it: `(!$r)` is text this function is asked
    // to judge as a path, and a parenthesis is not a name character. Reducing
    // it would mean stripping brackets too, and a receiver that has brackets in
    // it is an expression — the case `a_non_path_operand_keeps_its_operator`
    // exists to keep out.
    assert_eq!(
        receiver_of("a.php", "<?php function g($r) { (!$r)->zzuse(); }", "zzuse"),
        Some("(!$r)".to_string())
    );
}

/// A negated **call** is a different value, and keeps its operator.
///
/// This is the line the rule must not cross. `(!name.is_empty()).then(…)` calls
/// a method on a `bool`; rooting the receiver at `name` would hand the
/// classifier a binding that has nothing to do with the method.
#[test]
fn a_negated_call_keeps_its_operator() {
    for (path, source, callee) in [
        (
            "a.rs",
            "fn g(name: &str) { let t = (!name.is_empty()).zzthen(); }",
            "zzthen",
        ),
        ("a.ts", "function g(a: A) { (!a.ok()).zzthen(); }", "zzthen"),
    ] {
        let receiver = receiver_of(path, source, callee).expect("a receiver");
        assert!(
            receiver.starts_with('!'),
            "{path}: a negated call must keep its operator, got {receiver:?}"
        );
    }
}

/// An indexed or parenthesised operand keeps its operator too.
#[test]
fn a_non_path_operand_keeps_its_operator() {
    for (source, callee) in [
        ("fn g(v: &[u8]) { (*v.first().unwrap()).zzto(); }", "zzto"),
        ("fn g(v: &[Foo]) { (&v[0]).zzuse(); }", "zzuse"),
    ] {
        let receiver = receiver_of("a.rs", source, callee).expect("a receiver");
        assert!(
            receiver.starts_with('*') || receiver.starts_with('&'),
            "{source}: an expression operand is not a binding, got {receiver:?}"
        );
    }
}

/// A receiver that never had an operator is byte-for-byte what it was.
#[test]
fn an_ordinary_receiver_is_untouched() {
    for (source, callee, expected) in [
        ("fn g(r: H) { r.zzm(); }", "zzm", "r"),
        ("fn g() { std::fs::zzwrite(a, b); }", "zzwrite", "std::fs"),
        ("fn g(r: H) { r.zza().zzb(); }", "zzb", "r.zza"),
    ] {
        assert_eq!(
            receiver_of("a.rs", source, callee),
            Some(expected.to_string()),
            "{source}"
        );
    }
}

/// The strip is bounded, and a pathological run of operators is left alone
/// rather than walked to exhaustion.
#[test]
fn a_pathological_operator_run_is_bounded() {
    let source = format!("fn g(x: Foo) {{ ({}x).zzuse(); }}", "&".repeat(64));
    let receiver = receiver_of("a.rs", &source, "zzuse").expect("a receiver");
    assert!(
        receiver.starts_with('&'),
        "past the bound the text is kept as it was, got {receiver:?}"
    );
}
