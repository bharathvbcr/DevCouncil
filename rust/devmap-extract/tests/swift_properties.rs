//! Swift member properties are symbols (extraction schema v97).
//!
//! Before v97 a stored or computed `let`/`var` was no symbol anywhere in the
//! graph, so `p.isPaused` and `$0.isCompleted` named a member with nothing to
//! bind to and landed in the ledger as `no_namesake`. Each test below records
//! the pre-change answer beside its assertion.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind, SymbolKind, WiringKind};

const SOURCE: &str = r#"protocol Gauge {
    var level: Int { get }
    static var unit: String { get }
}

struct Meter: Gauge {
    var level: Int = 0
    static var shared = Meter()
    lazy var cache: [Int] = []
    var doubled: Int { level * 2 }
    var watched: Int = 0 { didSet { log() } }
    @Published var published = 1
    var x = 1, y = 2
    class var kind: String { "m" }
    private var secret = 3
    private var computedSecret: Int { 4 }
    func work() -> Int {
        let local = 1
        var inner: Int { 3 }
        return local + inner
    }
}

extension Meter {
    var ext: Int { 1 }
}

enum Mode {
    case fast
    var label: String { "l" }
}

let globalValue = 3
"#;

fn kind_of(extraction: &Extraction, qualified: &str) -> Option<SymbolKind> {
    extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified)
        .map(|symbol| symbol.kind)
}

fn exemption(extraction: &Extraction, qualified: &str) -> Option<(WiringKind, String)> {
    extraction
        .wiring
        .iter()
        .find(|annotation| annotation.target_symbol == qualified)
        .map(|annotation| (annotation.kind, annotation.details.clone()))
}

/// Pre-change: none of these was a symbol, so every `kind_of` was `None`.
#[test]
fn a_member_property_is_a_field_owned_by_its_types_bare_name() {
    let extraction = extract_file("App.swift", SOURCE);
    for qualified in [
        "App.swift::Gauge.level",
        "App.swift::Gauge.unit",
        "App.swift::Meter.level",
        "App.swift::Meter.shared",
        "App.swift::Meter.cache",
        "App.swift::Meter.doubled",
        "App.swift::Meter.watched",
        "App.swift::Meter.published",
        "App.swift::Meter.x",
        "App.swift::Meter.y",
        "App.swift::Meter.kind",
        "App.swift::Meter.secret",
        "App.swift::Meter.computedSecret",
        "App.swift::Meter.ext",
        "App.swift::Mode.label",
    ] {
        assert_eq!(
            kind_of(&extraction, qualified),
            Some(SymbolKind::Field),
            "{qualified} must be a Field symbol"
        );
    }
}

/// A `let`/`var` in a function or at file level is a local or a global, not a
/// member of anything.
#[test]
fn a_local_or_a_file_level_binding_is_not_a_property() {
    let extraction = extract_file("App.swift", SOURCE);
    for qualified in [
        "App.swift::Meter.local",
        "App.swift::Meter.inner",
        "App.swift::Meter.work.local",
        "App.swift::globalValue",
        "App.swift::local",
        "App.swift::inner",
    ] {
        assert_eq!(
            kind_of(&extraction, qualified),
            None,
            "{qualified} must not be a symbol"
        );
    }
}

/// An enum case is still a `Field` (and still exempt) beside the properties.
#[test]
fn an_enum_case_is_still_exempt_beside_its_enums_property() {
    let extraction = extract_file("App.swift", SOURCE);
    assert_eq!(
        kind_of(&extraction, "App.swift::Mode.fast"),
        Some(SymbolKind::Field)
    );
    assert_eq!(
        exemption(&extraction, "App.swift::Mode.fast").map(|(kind, _)| kind),
        Some(WiringKind::StructuralExempt)
    );
    assert_eq!(
        exemption(&extraction, "App.swift::Mode.label"),
        None,
        "a computed property of an enum is not a case, and nothing reads it by inference"
    );
}

#[test]
fn a_property_is_exempt_only_where_a_framework_or_the_compiler_reads_it() {
    let extraction = extract_file("App.swift", SOURCE);
    // A protocol requirement declares what a conformer provides.
    assert_eq!(
        exemption(&extraction, "App.swift::Gauge.level").map(|(kind, _)| kind),
        Some(WiringKind::StructuralExempt)
    );
    // A property wrapper is read through its projected value.
    assert_eq!(
        exemption(&extraction, "App.swift::Meter.published").map(|(kind, _)| kind),
        Some(WiringKind::StructuralExempt)
    );
    // A private stored property of a type with supertypes is read by a
    // synthesized conformance.
    assert_eq!(
        exemption(&extraction, "App.swift::Meter.secret").map(|(kind, _)| kind),
        Some(WiringKind::StructuralExempt)
    );
    // A private *computed* property is read by nothing but a use.
    assert_eq!(
        exemption(&extraction, "App.swift::Meter.computedSecret"),
        None
    );
    assert!(
        !extraction
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "App.swift::Meter.computedSecret")
            .expect("computedSecret")
            .is_exported
    );
    // Every property in a type with no supertype is a plain candidate.
    let plain = extract_file(
        "P.swift",
        "struct P {\n    private var dead = 1\n    var open = 2\n}\n",
    );
    assert_eq!(exemption(&plain, "P.swift::P.dead"), None);
    assert_eq!(exemption(&plain, "P.swift::P.open"), None);
    assert!(
        !plain
            .symbols
            .iter()
            .find(|symbol| symbol.qualified_name == "P.swift::P.dead")
            .expect("dead")
            .is_exported
    );
}

/// `@objc`/`@IBOutlet` and `@objcMembers` publish a property to the runtime.
#[test]
fn an_objc_property_is_a_runtime_entry_point() {
    let extraction = extract_file(
        "V.swift",
        "@objcMembers class V {\n    var a = 1\n}\nclass W {\n    @IBOutlet var b: Int!\n    @objc dynamic var c = 1\n}\n",
    );
    for qualified in ["V.swift::V.a", "V.swift::W.b", "V.swift::W.c"] {
        assert_eq!(
            exemption(&extraction, qualified).map(|(kind, _)| kind),
            Some(WiringKind::RuntimeEntryPoint),
            "{qualified}"
        );
    }
}

/// Pre-change: `count` in `f(count: 1)` and `.count` were both `Name`
/// references, which a property named `count` would now bind.
#[test]
fn an_argument_label_and_a_leading_dot_member_are_not_name_references() {
    let extraction = extract_file(
        "T.swift",
        "struct T {\n    var count: Int\n    func a() { g(count: 1) }\n    func b() { h(.count) }\n    func c() -> Int { count }\n}\n",
    );
    let sites: Vec<_> = extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Name && reference.name == "count")
        .map(|reference| reference.enclosing_symbol.clone())
        .collect();
    assert_eq!(
        sites,
        vec![Some("T.swift::T.c".to_string())],
        "only the bare read in `c` is a read of `count`"
    );
}

/// Pre-change: a member name was a file-scope local, so this read of `value`
/// in a getter (or a property initializer) was dropped as shadowed and the
/// stored property had no reader at all.
#[test]
fn a_bare_read_in_a_computed_property_body_is_a_reference() {
    let extraction = extract_file(
        "A.swift",
        "struct A {\n    private var value: Int\n    var viaGet: Int { get { value } }\n    var viaShort: Int { value }\n    var viaInit = 1 + 1\n}\n",
    );
    let reads: Vec<_> = extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Name && reference.name == "value")
        .map(|reference| reference.enclosing_symbol.clone())
        .collect();
    assert_eq!(
        reads,
        vec![Some("A.swift::A".to_string()); 2],
        "both getter reads of `value`, attributed to the type"
    );
}

/// A property read keeps its receiver, so the resolver can tell `p.count`
/// from a bare `count`.
#[test]
fn a_property_read_carries_its_receiver() {
    let extraction = extract_file(
        "R.swift",
        "func f(_ p: P) -> Int { p.count }\nfunc g(_ ps: [P]) -> Bool { ps.contains { $0.flag } }\n",
    );
    let receiver_of = |name: &str| {
        extraction
            .references
            .iter()
            .find(|reference| reference.kind == ReferenceKind::Name && reference.name == name)
            .and_then(|reference| reference.receiver_expr.clone())
    };
    assert_eq!(receiver_of("count").as_deref(), Some("p"));
    assert_eq!(receiver_of("flag").as_deref(), Some("$0"));
}

/// The property is a target, not a caller: a call inside its initializer or
/// getter keeps the enclosing type as its caller, as before v97.
#[test]
fn a_call_inside_a_property_body_is_still_the_types() {
    let extraction = extract_file(
        "C.swift",
        "struct C {\n    var full: String { format() }\n    var stored = make()\n    func format() -> String { \"\" }\n}\n",
    );
    let callers: Vec<_> = extraction
        .calls
        .iter()
        .map(|call| (call.callee_name.clone(), call.caller_symbol.clone()))
        .collect();
    assert!(
        callers.contains(&("format".to_string(), Some("C.swift::C".to_string()))),
        "{callers:?}"
    );
    assert!(
        callers.contains(&("make".to_string(), Some("C.swift::C".to_string()))),
        "{callers:?}"
    );
}

/// Two names of one declaration are two symbols, and no qualified name repeats.
#[test]
fn properties_do_not_collide_on_a_qualified_name() {
    let extraction = extract_file("App.swift", SOURCE);
    let mut names: Vec<_> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect();
    names.sort();
    let before = names.len();
    names.dedup();
    assert_eq!(
        before,
        names.len(),
        "a duplicate qualified name is a broken join key"
    );
}
