//! The read shapes a Swift property is reached by, as the extractor records
//! them: leading-dot members against a stated type, the right operand of an
//! infix operator, key paths, bodies written in an extension, and the
//! signatures that tell a property from an enum case.
//!
//! Each test records the answer at 9030d08c or at e3b352fa beside it.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind, SymbolKind};

/// `(receiver, enclosing symbol)` of every `Name` reference to `name`.
fn reads(extraction: &Extraction, name: &str) -> Vec<(Option<String>, Option<String>)> {
    extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Name && reference.name == name)
        .map(|reference| {
            (
                reference.receiver_expr.clone(),
                reference.enclosing_symbol.clone(),
            )
        })
        .collect()
}

fn receivers(extraction: &Extraction, name: &str) -> Vec<Option<String>> {
    reads(extraction, name)
        .into_iter()
        .map(|(receiver, _)| receiver)
        .collect()
}

/// e3b352fa dropped every leading-dot member, so `Shape.unit -> Shape.circle`
/// (present at 9030d08c through the bare-name rung) was lost.
#[test]
fn a_leading_dot_member_written_against_a_stated_type_keeps_that_type_as_receiver() {
    let extraction = extract_file(
        "S.swift",
        "enum Shape {\n    case circle(Int)\n    case square\n    static func unit() -> Shape { .circle(1) }\n    static func explicit() -> Shape { return .square }\n    static var made: Shape = .circle(2)\n    static var computed: Shape { .square }\n}\n",
    );
    assert_eq!(
        receivers(&extraction, "circle"),
        vec![Some("Shape".to_string()), Some("Shape".to_string())]
    );
    assert_eq!(
        receivers(&extraction, "square"),
        vec![Some("Shape".to_string()), Some("Shape".to_string())]
    );
}

/// An argument's parameter type is the callee's, an optional or array result
/// names another type: none of them states an owner for the member.
#[test]
fn a_leading_dot_member_with_no_stated_type_names_nothing() {
    let extraction = extract_file(
        "S.swift",
        "enum Shape { case circle(Int) }\nfunc a() { h(.circle(1)) }\nfunc b() -> Shape? { .circle(1) }\nfunc c() -> [Shape] { [.circle(1)] }\nfunc d(_ s: Shape) -> Bool { s == .circle(1) }\n",
    );
    assert_eq!(reads(&extraction, "circle"), vec![]);
}

/// 9030d08c: the right operand's receiver was the whole left side
/// (`p.a + q`), which types nothing.
#[test]
fn the_right_operand_of_an_infix_operator_keeps_its_own_receiver() {
    let extraction = extract_file(
        "S.swift",
        "func f(_ p: P, _ q: P) -> Int { p.a + q.b }\nfunc g(_ s: S) -> Int { 1 + s.x * s.y }\nstruct S {\n    var x: Int\n    func h() -> Int { 1 + self.x }\n    func i(_ b: B) -> Int { x + b.c.d }\n}\n",
    );
    assert_eq!(receivers(&extraction, "b"), vec![Some("q".to_string())]);
    assert_eq!(receivers(&extraction, "y"), vec![Some("s".to_string())]);
    assert!(receivers(&extraction, "x").contains(&Some("self".to_string())));
    assert_eq!(receivers(&extraction, "d"), vec![Some("b.c".to_string())]);
    // The left operands are untouched.
    assert_eq!(receivers(&extraction, "a"), vec![Some("p".to_string())]);
}

/// 9030d08c: `\S.secret` recorded the receiver text `\S`, which names no type.
#[test]
fn a_key_path_names_its_root_type_as_receiver() {
    let extraction = extract_file(
        "K.swift",
        "struct S {\n    var secret = 1\n    func a() -> KeyPath<S, Int> { \\S.secret }\n    func b() -> KeyPath<S, Int> { \\.secret }\n}\n",
    );
    let mut got = receivers(&extraction, "secret");
    got.sort();
    assert_eq!(got, vec![None, Some("S".to_string())]);
}

/// e3b352fa attributed a read in an extension's computed property to the file,
/// so no scope could see the type's members from it.
#[test]
fn a_read_in_an_extension_property_body_is_attributed_to_that_property() {
    let extraction = extract_file(
        "E.swift",
        "struct Box { var a: Int }\nextension Box {\n    var b: Int { a + 1 }\n    var c: Int { self.a }\n}\n",
    );
    let sites: Vec<_> = reads(&extraction, "a")
        .into_iter()
        .map(|(_, enclosing)| enclosing)
        .collect();
    assert_eq!(
        sites,
        vec![
            Some("E.swift::Box.b".to_string()),
            Some("E.swift::Box.c".to_string())
        ]
    );
}

#[test]
fn a_property_says_what_it_is_and_an_enum_case_says_nothing() {
    let extraction = extract_file(
        "P.swift",
        "protocol Gauge { var level: Int { get } }\nenum Mode {\n    case fast\n    var label: String { \"l\" }\n    static var count = 0\n}\n",
    );
    let signature = |qualified: &str| {
        extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == qualified)
            .unwrap_or_else(|| panic!("no {qualified}"))
            .signature
            .clone()
    };
    assert_eq!(
        signature("P.swift::Gauge.level").as_deref(),
        Some("property requirement")
    );
    assert_eq!(
        signature("P.swift::Mode.label").as_deref(),
        Some("computed property")
    );
    assert_eq!(
        signature("P.swift::Mode.count").as_deref(),
        Some("stored property")
    );
    assert_eq!(signature("P.swift::Mode.fast"), None);
    for symbol in &extraction.symbols {
        assert_eq!(
            symbol.is_member_property(),
            symbol.kind == SymbolKind::Field && symbol.name != "fast",
            "{}",
            symbol.qualified_name
        );
    }
}

/// The modifier shapes of the attack's list that are not tuple patterns or
/// backtick names (those are ft-915b0a03).
#[test]
fn every_modifier_shape_but_tuple_and_backtick_is_a_property() {
    let extraction = extract_file(
        "A.swift",
        "import SwiftUI\nclass C {\n    weak var a: C?\n    unowned let b: C\n    final var c = 1\n    open var d = 1\n    dynamic var e = 1\n    private(set) var f = 1\n    fileprivate var g = 1\n    @MainActor var h = 1\n    nonisolated let i = 1\n    @objc var j = 1\n    var _under = 1\n    var willy = 1 { willSet { _ = newValue } }\n    var m = 1, n = 2, o: Int = 3\n    init(b: C) { self.b = b }\n}\n",
    );
    for name in [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "_under", "willy", "m", "n", "o",
    ] {
        assert!(
            extraction
                .symbols
                .iter()
                .any(|symbol| symbol.is_member_property()
                    && symbol.qualified_name == format!("A.swift::C.{name}")),
            "C.{name} is not a property"
        );
    }
}
