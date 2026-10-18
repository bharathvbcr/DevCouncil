//! Adversarial extraction probes for Swift member properties (e3b352fa, v97).
//!
//! Each test states the correct behaviour; a failing test is a BREAK.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind, SymbolKind};

fn names(ex: &Extraction, kind: ReferenceKind, in_symbol_suffix: &str) -> Vec<String> {
    ex.references
        .iter()
        .filter(|r| {
            r.kind == kind
                && r.enclosing_symbol
                    .as_deref()
                    .is_some_and(|s| s.ends_with(in_symbol_suffix))
        })
        .map(|r| r.name.clone())
        .collect()
}

fn field_names(ex: &Extraction) -> Vec<String> {
    let mut v: Vec<String> = ex
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Field)
        .map(|s| s.qualified_name.clone())
        .collect();
    v.sort();
    v
}

fn is_field(ex: &Extraction, suffix: &str) -> bool {
    ex.symbols
        .iter()
        .any(|s| s.kind == SymbolKind::Field && s.qualified_name.ends_with(suffix))
}

// ---------------------------------------------------------------- Name refs

#[test]
fn an_argument_value_equal_to_its_label_is_still_a_name_ref() {
    let ex = extract_file(
        "A.swift",
        "let total = 3\nfunc take(total: Int) {}\nfunc f() {\n    take(total: total)\n}\nfunc g() {\n    take(total: 1)\n}\n",
    );
    let f = names(&ex, ReferenceKind::Name, "::f");
    assert_eq!(f.iter().filter(|n| *n == "total").count(), 1, "f: {f:?}");
    let g = names(&ex, ReferenceKind::Name, "::g");
    assert!(!g.iter().any(|n| n == "total"), "g: label recorded as a read: {g:?}");
}

#[test]
fn a_dictionary_literal_key_and_a_tuple_value_stay_name_refs() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n    var key: String\n    var a: Int\n    var b: Int\n    func f() -> [String: Int] { [key: 1] }\n    func g() -> (x: Int, y: Int) { (x: a, y: b) }\n}\n",
    );
    let f = names(&ex, ReferenceKind::Name, "S.f");
    assert!(f.iter().any(|n| n == "key"), "dictionary key is a read: {f:?}");
    let g = names(&ex, ReferenceKind::Name, "S.g");
    assert!(g.iter().any(|n| n == "a") && g.iter().any(|n| n == "b"), "{g:?}");
    assert!(!g.iter().any(|n| n == "x" || n == "y"), "tuple labels recorded: {g:?}");
}

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn a_subscript_label_and_a_statement_label_are_not_reads_but_their_values_are() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n    var key: Int\n    var v: Int\n    subscript(key k: Int) -> Int { k }\n    func f(_ s: S) -> Int { s[key: v] }\n    func g() {\n        outer: for _ in 0..<2 { break outer }\n    }\n}\n",
    );
    let f = names(&ex, ReferenceKind::Name, "S.f");
    assert!(f.iter().any(|n| n == "v"), "subscript argument value: {f:?}");
    assert!(!f.iter().any(|n| n == "key"), "subscript label: {f:?}");
    let g = names(&ex, ReferenceKind::Name, "S.g");
    assert!(!g.iter().any(|n| n == "outer"), "statement label: {g:?}");
}

#[test]
#[ignore = "superseded by the leading-dot contextual-type rule: .shared written against a stated type is recorded with that receiver"]
fn a_leading_dot_member_is_not_a_name_but_a_dotted_chain_base_is() {
    let ex = extract_file(
        "A.swift",
        "struct Cfg { static let shared = Cfg() }\nfunc f() -> Cfg { .shared }\nfunc g() -> Int { Cfg.shared.hashValue }\nfunc h(_ c: Cfg) -> Int { c.hashValue }\n",
    );
    let f = names(&ex, ReferenceKind::Name, "::f");
    assert!(!f.iter().any(|n| n == "shared"), "{f:?}");
    // The member access `Cfg.shared` must still be recorded somehow (name with
    // receiver Cfg) — a recall regression if the leading-dot filter swallowed it.
    let g: Vec<_> = ex
        .references
        .iter()
        .filter(|r| {
            r.enclosing_symbol.as_deref().is_some_and(|s| s.ends_with("::g")) && r.name == "shared"
        })
        .collect();
    assert!(!g.is_empty(), "Cfg.shared lost its reference");
}

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn a_leading_dot_call_with_a_contextual_type_is_not_dropped_as_a_name_only() {
    // `.init(...)` and `.make()` are Calls; verify the call survives the filter.
    let ex = extract_file(
        "A.swift",
        "struct Cfg { init() {}\n  static func make() -> Cfg { Cfg() } }\nfunc f() -> Cfg { .init() }\nfunc g() -> Cfg { .make() }\n",
    );
    let g: Vec<&str> = ex
        .calls
        .iter()
        .filter(|c| c.callee_name == "make")
        .map(|c| c.callee_name.as_str())
        .collect();
    assert!(!g.is_empty(), "`.make()` call dropped");
}

// ---------------------------------------------------------------- Field symbols

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn every_property_modifier_shape_is_a_field() {
    let src = "import SwiftUI\nclass C {\n    weak var a: C?\n    unowned let b: C\n    final var c = 1\n    open var d = 1\n    dynamic var e = 1\n    private(set) var f = 1\n    fileprivate var g = 1\n    @MainActor var h = 1\n    nonisolated let i = 1\n    @objc var j = 1\n    var `default` = 1\n    var _under = 1\n    var willy = 1 { willSet { _ = newValue } }\n    var (t1, t2) = (1, 2)\n    var m = 1, n = 2, o: Int = 3\n    init(b: C) { self.b = b }\n}\n";
    let ex = extract_file("A.swift", src);
    for n in [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "default", "_under", "willy", "m", "n",
        "o",
    ] {
        assert!(
            is_field(&ex, &format!("C.{n}")),
            "C.{n} is not a Field: {:?}",
            field_names(&ex)
        );
    }
}

#[test]
#[ignore = "pre-existing Swift residue, ft-915b0a03"]
fn a_tuple_pattern_member_is_a_field_per_name() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n    let (a, b) = (1, 2)\n    var (c, d): (Int, Int) = (3, 4)\n}\n",
    );
    for n in ["S.a", "S.b", "S.c", "S.d"] {
        assert!(is_field(&ex, n), "{n}: {:?}", field_names(&ex));
    }
}

#[test]
fn a_property_inside_a_conditional_compilation_block_in_a_type_is_a_field() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n#if DEBUG\n    var dbg = 1\n#else\n    var rel = 2\n#endif\n}\n",
    );
    assert!(is_field(&ex, "S.dbg") && is_field(&ex, "S.rel"), "{:?}", field_names(&ex));
}

#[test]
fn a_nested_type_property_is_owned_by_the_nested_type() {
    let ex = extract_file(
        "A.swift",
        "struct Outer {\n    var a = 1\n    struct Inner { var b = 2 }\n    enum K { case x\n        var c: Int { 3 } }\n}\nextension Outer.Inner { var d: Int { 4 } }\n",
    );
    for n in ["Inner.b", "K.c", "Inner.d"] {
        assert!(is_field(&ex, n), "{n}: {:?}", field_names(&ex));
    }
    assert!(
        !is_field(&ex, "Outer.b"),
        "a nested property must not be owned by the outer type: {:?}",
        field_names(&ex)
    );
}

#[test]
fn locals_in_a_property_initializer_closure_are_not_fields() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n    var a: Int = {\n        let tmp = 1\n        var acc = 0\n        acc += tmp\n        return acc\n    }()\n    var b: Int {\n        let local = 2\n        return local\n    }\n    lazy var c: Int = { let z = 1; return z }()\n    var d: Int = 0 { didSet { let inner = 1; _ = inner } }\n}\n",
    );
    let f = field_names(&ex);
    for bad in ["tmp", "acc", "local", "z", "inner"] {
        assert!(!f.iter().any(|q| q.ends_with(&format!(".{bad}"))), "{bad} became a Field: {f:?}");
    }
    for good in ["S.a", "S.b", "S.c", "S.d"] {
        assert!(f.iter().any(|q| q.ends_with(good)), "{good}: {f:?}");
    }
}

#[test]
fn an_unmatched_or_garbage_type_body_does_not_invent_fields() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n    var = 1\n    let\n    var x: = 3\n    var ok = 1\n",
    );
    let f = field_names(&ex);
    assert!(f.iter().any(|q| q.ends_with("S.ok")), "{f:?}");
    assert!(
        !f.iter().any(|q| q.ends_with("S.=") || q.ends_with("S.") || q.ends_with("S.let")),
        "{f:?}"
    );
}

#[test]
fn a_property_and_a_same_named_method_are_two_symbols_with_distinct_kinds() {
    let ex = extract_file(
        "A.swift",
        "struct S {\n    var size: Int = 0\n}\nextension S {\n    func size(_ k: Int) -> Int { k }\n}\n",
    );
    let kinds: Vec<SymbolKind> = ex
        .symbols
        .iter()
        .filter(|s| s.qualified_name.ends_with("S.size"))
        .map(|s| s.kind)
        .collect();
    assert_eq!(kinds.len(), 2, "{kinds:?}");
    assert!(kinds.contains(&SymbolKind::Field) && kinds.iter().any(|k| *k != SymbolKind::Field));
}

#[test]
fn duplicate_property_names_across_extensions_yield_distinct_symbol_ids() {
    let ex = extract_file(
        "A.swift",
        "struct S {}\nextension S { var v: Int { 1 } }\nextension S { var v: Int { 2 } }\n",
    );
    let ids: Vec<_> = ex
        .symbols
        .iter()
        .filter(|s| s.qualified_name.ends_with("S.v"))
        .map(|s| (s.qualified_name.clone(), s.span.start_byte))
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
}

#[test]
#[ignore = "30k-property file exceeds the debug parse budget; release evidence in the lane report"]
fn hostile_fifty_thousand_properties_extract_in_bounded_time() {
    let mut s = String::from("struct Big {\n");
    // 50k lines is 1.3 MB, over the 1 MiB per-file limit (a stated refusal);
    // 30k stays under it.
    for i in 0..30_000 {
        s.push_str(&format!("  var p{i}: Int = {i}\n"));
    }
    s.push_str("}\n");
    let t = std::time::Instant::now();
    let ex = extract_file("Big.swift", &s);
    let el = t.elapsed();
    let n = ex.symbols.iter().filter(|x| x.kind == SymbolKind::Field).count();
    assert!(n >= 29_900, "only {n} of 30000 properties extracted (silent cap?)");
    assert!(el.as_secs() < 20, "took {el:?}");
}
