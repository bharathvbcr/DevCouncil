//! Leading-dot member chains under tree-sitter-swift 0.7.4.
//!
//! 0.7.3 bound the leading dot to the first member alone: `.caption.weight(…)`
//! parsed as `call(navigation(prefix(., caption), weight), …)`. 0.7.4 binds
//! every prefix operator over the whole chain up to the first argument list:
//! `call(prefix(., navigation(caption, weight)), …)`. Read with the 0.7.3
//! assumptions, that shape lost the `weight` call (the callee was a prefix the
//! call identity refuses), recorded `caption` as a bare read of a value named
//! `caption`, and gave member references a dotless receiver.
//!
//! Each test pins what the extractor recorded under 0.7.3 for the same source
//! (measured on a 290-file Swift corpus before the bump).

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind};

/// `(callee, receiver)` of every call named `callee`.
fn calls(extraction: &Extraction, callee: &str) -> Vec<Option<String>> {
    extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == callee)
        .map(|call| call.receiver_expr.clone())
        .collect()
}

/// The receiver of every reference of `kind` named `name`.
fn refs(extraction: &Extraction, kind: ReferenceKind, name: &str) -> Vec<Option<String>> {
    extraction
        .references
        .iter()
        .filter(|reference| reference.kind == kind && reference.name == name)
        .map(|reference| reference.receiver_expr.clone())
        .collect()
}

#[test]
fn a_modifier_called_on_a_leading_dot_base_is_a_call_on_the_dotted_base() {
    let extraction = extract_file(
        "V.swift",
        "struct V {\n    func body(_ t: Text) -> Text {\n        t.font(.caption.weight(.medium))\n    }\n    func shade() -> Color {\n        let c = .black.opacity(0.4)\n        return c\n    }\n    func go() {\n        .shared.refresh()\n    }\n}\n",
    );
    assert_eq!(
        calls(&extraction, "weight"),
        vec![Some(".caption".to_string())]
    );
    assert_eq!(
        calls(&extraction, "opacity"),
        vec![Some(".black".to_string())]
    );
    assert_eq!(
        calls(&extraction, "refresh"),
        vec![Some(".shared".to_string())]
    );
    // The member a call names is the call, not also a read of a value.
    assert_eq!(refs(&extraction, ReferenceKind::Name, "weight"), vec![]);
    assert_eq!(refs(&extraction, ReferenceKind::Name, "opacity"), vec![]);
}

/// The base names a member of a type the site never spells: it is no read of a
/// local, parameter or property that merely shares its spelling.
#[test]
fn the_base_of_a_leading_dot_chain_is_not_a_bare_read() {
    let extraction = extract_file(
        "V.swift",
        "struct V {\n    let caption = 1\n    let accentColor = 2\n    func body(_ t: Text) -> Text {\n        t.font(.caption.weight(.medium)).tint(.accentColor.opacity(0.5))\n    }\n    func plain() -> Int { caption }\n}\n",
    );
    // `plain`'s `caption` is a real read; the leading-dot one is not.
    assert_eq!(refs(&extraction, ReferenceKind::Name, "caption").len(), 1);
    assert_eq!(
        refs(&extraction, ReferenceKind::Name, "accentColor"),
        vec![]
    );
}

/// A chain read without a call: each member's receiver keeps the leading dot,
/// so `.a.b` never reads as the member `b` of a value named `a`.
#[test]
fn a_member_of_a_leading_dot_chain_keeps_the_dot_on_its_receiver() {
    let extraction = extract_file(
        "V.swift",
        "struct V {\n    let a = 1\n    func f() -> Int {\n        let d = .a.b.c\n        return d\n    }\n}\n",
    );
    assert_eq!(
        refs(&extraction, ReferenceKind::Name, "b"),
        vec![Some(".a".to_string())]
    );
    assert_eq!(
        refs(&extraction, ReferenceKind::Name, "c"),
        vec![Some(".a.b".to_string())]
    );
    assert_eq!(refs(&extraction, ReferenceKind::Name, "a"), vec![]);
}

/// A chained leading dot names no owner for its base, even where the function
/// states a return type: the type is the chain's, not the base member's.
#[test]
fn a_chained_leading_dot_base_takes_no_owner_from_the_return_type() {
    let extraction = extract_file(
        "S.swift",
        "enum Shape { case circle }\nfunc g() -> Shape { .circle.next }\nfunc h() -> Shape { .circle.next() }\n",
    );
    assert_eq!(refs(&extraction, ReferenceKind::Name, "circle"), vec![]);
    assert_eq!(
        calls(&extraction, "next"),
        vec![Some(".circle".to_string())]
    );
}

/// `.Foo.init()` constructs the member `Foo` of a contextual type: no
/// construction of a type named `Foo`.
#[test]
fn a_leading_dot_init_constructs_no_type_of_the_base_name() {
    let extraction = extract_file(
        "S.swift",
        "struct Foo {}\nstruct Box { static let Foo = Box() }\nfunc f() -> Box { .Foo.init() }\nfunc g() -> Foo { Foo.init() }\n",
    );
    // Only `g`'s `Foo.init()` names the type.
    let callers: Vec<_> = extraction
        .calls
        .iter()
        .filter(|call| call.callee_name == "Foo")
        .map(|call| call.caller_symbol.clone().unwrap_or_default())
        .collect();
    assert_eq!(callers.len(), 1, "{callers:?}");
    assert!(callers[0].ends_with("::g"), "{callers:?}");
}

/// The prefix operators that are not a leading dot are read through, as they
/// were when the grammar bound them to the first operand.
#[test]
fn a_negated_member_call_still_names_its_receiver() {
    let extraction = extract_file(
        "S.swift",
        "func f(_ foo: Foo) -> Bool { !foo.bar() }\nfunc g(_ x: X) -> Bool { !x.y }\n",
    );
    assert_eq!(calls(&extraction, "bar"), vec![Some("foo".to_string())]);
    assert_eq!(
        refs(&extraction, ReferenceKind::Name, "y"),
        vec![Some("x".to_string())]
    );
}
