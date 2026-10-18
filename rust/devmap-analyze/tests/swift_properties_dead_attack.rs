//! Adversarial dead-code probes for Swift member properties (e3b352fa).
//!
//! A private stored/computed property of a plain type with no supertype is the
//! one shape that is neither exempt nor protocol-visible, so every read shape
//! below must keep it from being reported. A failing test is a BREAK (a
//! confident "delete this" on a live property).

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::*;
use devmap_resolve::*;

fn reports(sources: &[(&str, &str)]) -> Vec<DeadSymbolReport> {
    let extractions: Vec<Extraction> = sources
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    analyze_liveness(&extractions, &resolution)
}

fn reported(reports: &[DeadSymbolReport], name_suffix: &str) -> bool {
    reports
        .iter()
        .any(|r| r.symbol_name.ends_with(name_suffix) && !r.is_exempt)
}

/// `body` is placed inside `struct S { private var secret = 1 ... }`.
fn secret_is_live(extra_in_type: &str, outside: &str) -> bool {
    let src = format!("struct S {{\n    private var secret = 1\n{extra_in_type}\n}}\n{outside}\n");
    !reported(&reports(&[("Sources/M/S.swift", src.as_str())]), "secret")
}

#[test]
fn control_an_unread_private_property_is_reported() {
    assert!(!secret_is_live("", ""), "control: nothing reads `secret`");
}

#[test]
fn a_keypath_read_keeps_a_private_stored_property_alive() {
    assert!(secret_is_live(
        "    func kp() -> KeyPath<S, Int> { \\S.secret }",
        ""
    ));
    assert!(secret_is_live(
        "    func kp() -> KeyPath<S, Int> { \\.secret }",
        ""
    ));
    assert!(secret_is_live(
        "    func v() -> Int { self[keyPath: \\.secret] }",
        ""
    ));
}

#[test]
fn an_interpolation_inout_and_compound_assignment_are_uses() {
    for body in [
        "    func a() -> String { \"\\(secret)\" }",
        "    mutating func b() { secret += 1 }",
        "    mutating func c() { bump(&secret) }\n    func bump(_ x: inout Int) {}",
        "    func d(_ o: S) -> Bool { secret == o.secret }",
        "    func e() { defer { _ = secret } }",
        "    func f() -> Int { switch secret { case 1: return 1; default: return 0 } }",
        "    func g() -> Int { [1, 2].map { $0 + secret }.count }",
        "    func h() -> Int { if secret > 0 { return 1 } else { return 0 } }",
        "    func i() -> Int { return secret }",
        "    func j() { print(secret) }",
        "    func k() -> Int { (secret) }",
        "    func l() -> Int? { Optional(secret) }",
        "    var m: Int { get { secret } }",
        "    subscript(i: Int) -> Int { secret + i }",
        "    init() { _ = secret }",
        "    var n: Int = 0 { didSet { _ = secret } }",
    ] {
        assert!(secret_is_live(body, ""), "read shape was reported dead: {body}");
    }
}

#[test]
fn a_same_file_extension_read_of_a_private_property_is_a_use() {
    assert!(secret_is_live(
        "",
        "extension S { func e() -> Int { secret } }"
    ));
    assert!(secret_is_live(
        "",
        "extension S { var e2: Int { self.secret } }"
    ));
    assert!(secret_is_live(
        "",
        "func free(_ s: S) -> Int { s.secret }"
    ));
}

#[test]
fn an_explicit_self_read_is_a_use_when_another_type_has_a_property_of_that_name() {
    // Two types spell `secret`; each is read only through `self.secret`.
    let src = "struct S {\n    private var secret = 1\n    func a() -> Int { self.secret }\n}\nstruct T {\n    private var secret = 2\n    func b() -> Int { self.secret }\n}\nfunc use(_ s: S, _ t: T) -> Int { s.a() + t.b() }\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(
        !reported(&r, "S.secret") && !reported(&r, "T.secret"),
        "a `self.x` read was lost to a same-named property: {:?}",
        r.iter().map(|x| (&x.symbol_name, x.confidence)).collect::<Vec<_>>()
    );
}

#[test]
fn a_member_read_as_the_right_operand_of_an_infix_operator_is_a_use() {
    // Pre-existing receiver-text defect (`1 + self`), newly visible as a
    // confident-tier dead finding on a live property.
    for body in [
        "    func a() -> Int { 1 + self.secret }",
        "    func b(_ o: S) -> Int { 1 + o.secret }",
        "    func c(_ o: S) -> Int { o.secret * 2 + o.secret }",
    ] {
        assert!(secret_is_live(body, ""), "reported dead: {body}");
    }
}

#[test]
fn a_bare_read_in_an_extension_computed_property_is_a_use() {
    assert!(secret_is_live(
        "",
        "extension S { var viaExt: Int { secret * 2 } }\nfunc use(_ s: S) -> Int { s.viaExt }"
    ));
}

#[test]
fn a_self_read_in_an_extension_computed_property_is_a_use() {
    assert!(secret_is_live(
        "",
        "extension S { var viaExt: Int { self.secret * 2 } }\nfunc use(_ s: S) -> Int { s.viaExt }"
    ));
}

#[test]
fn a_property_read_only_in_an_if_debug_block_is_a_use() {
    assert!(secret_is_live(
        "    func d() {\n#if DEBUG\n        print(secret)\n#endif\n    }",
        ""
    ));
    assert!(secret_is_live(
        "",
        "#if DEBUG\nextension S { func dbg() -> Int { secret } }\n#endif"
    ));
}

#[test]
fn a_property_read_only_from_a_preview_block_is_a_use() {
    assert!(secret_is_live(
        "",
        "#Preview {\n    Text(String(S().secret))\n}"
    ));
}

#[test]
fn a_didset_only_private_property_is_not_dead_code() {
    // An observer is a side effect: `watched = 1` is a use even with no read.
    let src = "struct S {\n    private var watched = 0 { didSet { log() } }\n    mutating func poke() { watched = 1 }\n    func log() {}\n}\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "watched"), "{:?}", r.iter().map(|x| &x.symbol_name).collect::<Vec<_>>());
}

#[test]
fn a_private_stored_property_of_a_codable_via_extension_is_not_reported() {
    // `struct S {}` + `extension S: Codable {}`: synthesized coding reads every
    // stored property, but the type's own declaration has no supertype.
    let src = "struct S {\n    private var secret = 1\n}\nextension S: Codable {}\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "secret"), "Codable-by-extension stored property reported dead");
}

#[test]
fn a_private_stored_property_of_a_codable_struct_is_not_reported() {
    let src = "struct S: Codable {\n    private var secret = 1\n}\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "secret"));
}

#[test]
fn a_private_stored_property_of_a_hashable_or_equatable_via_extension_is_not_reported() {
    let src = "struct S {\n    private var secret = 1\n}\nextension S: Hashable {}\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "secret"), "synthesized Hashable reads every stored property");
}

#[test]
fn an_objc_dynamic_and_wrapped_properties_are_exempt() {
    let src = "import SwiftUI\nclass C {\n    @objc dynamic private var kvo = 1\n    @objc private dynamic var kvo2 = 1\n}\nstruct V: View {\n    @State private var on = false\n    @Binding private var b: Bool\n    var body: some View { Toggle(\"\", isOn: $on) }\n}\n";
    let r = reports(&[("Sources/M/V.swift", src)]);
    for n in ["kvo", "kvo2", "on", "b"] {
        assert!(!reported(&r, n), "{n} reported dead");
    }
}

#[test]
fn a_property_wrapper_projected_value_read_is_a_use_of_the_wrapper_property() {
    // `_wrapped` / `$binding` read inside a plain (no-supertype) type.
    let src = "@propertyWrapper struct W { var wrappedValue: Int\n var projectedValue: Int { wrappedValue } }\nstruct S {\n    @W private var w = 1\n    func a() -> Int { $w + _w.wrappedValue }\n}\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "S.w"), "attributed wrapper property reported");
}

#[test]
fn a_private_computed_property_used_only_by_a_property_wrapper_argument() {
    let src = "struct S {\n    private static var limit: Int { 3 }\n    var capped: Int { min(1, Self.limit) }\n}\nfunc use(_ s: S) -> Int { s.capped }\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "limit"), "Self.limit read in a computed body is a use");
}

#[test]
fn a_property_read_through_a_stored_closure_default_is_a_use() {
    let src = "struct S {\n    private var base = 1\n    var f: (Int) -> Int { { $0 + self.base } }\n    lazy var g: Int = base + 1\n}\nfunc use(_ s: S) -> Int { s.f(1) + s.g }\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "base"));
}

#[test]
fn a_property_read_only_through_a_member_of_an_enum_case_payload_binding() {
    let src = "enum E {\n    case a(S)\n}\nstruct S {\n    var open: Int = 1\n}\nfunc use(_ e: E) -> Int { switch e { case .a(let s): return s.open } }\n";
    let r = reports(&[("Sources/M/S.swift", src)]);
    assert!(!reported(&r, "open"));
}
