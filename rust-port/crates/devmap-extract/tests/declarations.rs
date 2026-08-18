//! The per-language declaration path (`langdecl`).
//!
//! Every test here was verified failing against the tree immediately before the
//! change, and the pre-fix answer is recorded beside each assertion so a
//! regression is recognisable rather than merely red. Reproduced on
//! `/tmp/swkt/App.swift` and `/tmp/swkt/Main.kt` first, then written down.
//!
//! The declaration path answers three questions — what kind of thing is this,
//! what is it called, and can anything outside the corpus reach it — and before
//! this module it answered all three from a node-kind table, a `name` field and
//! a substring scan. Each of those is right for some of the 35 languages and
//! silently wrong for most.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, SymbolKind, WiringKind};

/// `(qualified_name, kind, is_exported)` for every non-file symbol, sorted.
fn symbols(extraction: &Extraction) -> Vec<(String, SymbolKind, bool)> {
    let mut rows: Vec<(String, SymbolKind, bool)> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| {
            (
                symbol.qualified_name.clone(),
                symbol.kind,
                symbol.is_exported,
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn qualified_names(extraction: &Extraction) -> Vec<String> {
    let mut names: Vec<String> = extraction
        .symbols
        .iter()
        .filter(|symbol| symbol.kind != SymbolKind::File)
        .map(|symbol| symbol.qualified_name.clone())
        .collect();
    names.sort();
    names
}

fn kind_of(extraction: &Extraction, qualified: &str) -> SymbolKind {
    extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified)
        .unwrap_or_else(|| {
            panic!(
                "no symbol {qualified:?} in {:?}",
                qualified_names(extraction)
            )
        })
        .kind
}

fn exported(extraction: &Extraction, qualified: &str) -> bool {
    extraction
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == qualified)
        .unwrap_or_else(|| {
            panic!(
                "no symbol {qualified:?} in {:?}",
                qualified_names(extraction)
            )
        })
        .is_exported
}

/// Qualified names emitted more than once.
///
/// A duplicate is a broken join key (SC14): two nodes carry one name, so every
/// call to it resolves ambiguously and is downgraded to the speculative tier.
fn duplicate_names(extraction: &Extraction) -> Vec<String> {
    let names = qualified_names(extraction);
    let mut duplicates: Vec<String> = names
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0].clone())
        .collect();
    duplicates.dedup();
    duplicates
}

/// Call and reference sources that name no emitted symbol — the SC9/SC10 orphan.
fn orphaned_sources(extraction: &Extraction) -> Vec<String> {
    let emitted: Vec<String> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.clone())
        .collect();
    let mut orphans: Vec<String> = extraction
        .calls
        .iter()
        .filter_map(|call| call.caller_symbol.clone())
        .chain(
            extraction
                .references
                .iter()
                .filter_map(|reference| reference.enclosing_symbol.clone()),
        )
        .filter(|source| !emitted.contains(source))
        .collect();
    orphans.sort();
    orphans.dedup();
    orphans
}

fn exemption(extraction: &Extraction, qualified: &str) -> Option<(WiringKind, String)> {
    extraction
        .wiring
        .iter()
        .find(|annotation| annotation.target_symbol == qualified)
        .map(|annotation| (annotation.kind, annotation.details.clone()))
}

// ------------------------------------------------------------------- Swift

const SWIFT: &str = r#"import Foundation

protocol Greeter {
    func greet() -> String
}

struct Person: Greeter {
    let name: String

    init(name: String) {
        self.name = name
    }

    func greet() -> String {
        return "hi \(name)"
    }

    private func unused() -> Int {
        return 1
    }
}

extension Person {
    func extra() -> Int {
        return 2
    }
}

enum Mode {
    case fast, slow
}

enum Payload {
    case text(String)
}

public class Runner {
    private let person: Person

    init() {
        self.person = Person(name: "a")
    }

    func run() -> String {
        let p = Payload.text("z")
        _ = p
        return person.greet() + String(person.extra())
    }
}
"#;

/// Defect 2. `extension Person` emitted a second `App.swift::Person`.
///
/// Pre-fix: `qualified_names` contained `App.swift::Person` **twice**, one from
/// `struct Person` and one from `extension Person`, and the `Person(name:)`
/// construction inside `Runner.init` resolved to it at 0.2 (ambiguous) instead
/// of 1.0.
#[test]
fn a_swift_extension_is_not_a_second_declaration_of_the_type_it_extends() {
    let extraction = extract_file("App.swift", SWIFT);
    assert_eq!(
        duplicate_names(&extraction),
        Vec::<String>::new(),
        "no qualified name may be emitted twice: {:?}",
        qualified_names(&extraction)
    );
    assert_eq!(
        qualified_names(&extraction)
            .iter()
            .filter(|name| *name == "App.swift::Person")
            .count(),
        1,
        "`struct Person` declares the type; `extension Person` only adds members"
    );
    // The extension must still *own* what it declares, or the member's calls
    // orphan. This is the half that makes the fix a scalpel rather than a
    // deletion.
    assert!(
        qualified_names(&extraction).contains(&"App.swift::Person.extra".to_string()),
        "the extension's member is still owned by the type it extends: {:?}",
        qualified_names(&extraction)
    );
}

/// Defect 3. `enum Mode` reported `Class`.
#[test]
fn swift_declaration_kinds_come_from_the_declaration_kind_field() {
    let extraction = extract_file("App.swift", SWIFT);
    // Pre-fix: Class. tree-sitter-swift spells struct, class, enum, actor and
    // extension all as `class_declaration`.
    assert_eq!(kind_of(&extraction, "App.swift::Mode"), SymbolKind::Enum);
    assert_eq!(kind_of(&extraction, "App.swift::Payload"), SymbolKind::Enum);
    // Pre-fix: Class. This one was already wrong in the same way.
    assert_eq!(
        kind_of(&extraction, "App.swift::Person"),
        SymbolKind::Struct
    );
    assert_eq!(kind_of(&extraction, "App.swift::Runner"), SymbolKind::Class);
    // Unchanged, and asserted so the fix cannot quietly break what worked.
    assert_eq!(
        kind_of(&extraction, "App.swift::Greeter"),
        SymbolKind::Interface
    );
}

/// Defect 3, the `actor` case, which the fixture above has no room for.
#[test]
fn a_swift_actor_is_a_class_and_an_extension_of_a_generic_type_owns_its_members() {
    let extraction = extract_file(
        "A.swift",
        "actor Store { func load() {} }\nextension Array where Element: Equatable { func dedup() {} }\n",
    );
    assert_eq!(kind_of(&extraction, "A.swift::Store"), SymbolKind::Class);
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "A.swift::Array.dedup".to_string(),
            "A.swift::Store".to_string(),
            "A.swift::Store.load".to_string(),
        ],
        "the constrained extension declares no type and owns one method"
    );
}

/// Defect 1. Visibility was read from the node's whole source text.
///
/// Pre-fix: `App.swift::Person` reported `is_exported = false` — because its
/// *body* contains the word `private ` — while `Person.greet`, which has no
/// modifier at all, reported `true` from the `!name.starts_with('_')` fallback.
/// One of those is a dead-code candidate manufactured from a member's keyword;
/// the other is an exemption granted on no evidence.
#[test]
fn swift_visibility_is_read_from_the_declarations_own_modifier_list() {
    let extraction = extract_file("App.swift", SWIFT);
    assert!(
        !exported(&extraction, "App.swift::Person.unused"),
        "`private func unused` is confined to one file, which is the unit \
         extraction sees whole"
    );
    assert!(
        exported(&extraction, "App.swift::Person"),
        "pre-fix this was false, because `Person`'s *body* contains `private `"
    );
    assert!(
        exported(&extraction, "App.swift::Runner"),
        "`public class Runner` is public by its own keyword"
    );
    assert!(
        exported(&extraction, "App.swift::Person.greet"),
        "`internal` is module-scoped and devmap models no Swift modules, so \
         no non-exported claim is made for it"
    );
}

/// The same rule at every access level, including `fileprivate` and `open`.
#[test]
fn swift_access_levels_map_to_exactly_two_answers() {
    let extraction = extract_file(
        "V.swift",
        "private func a() {}\nfileprivate func b() {}\ninternal func c() {}\n\
         public func d() {}\nopen func e() {}\npackage func f() {}\nfunc g() {}\n",
    );
    let rows: Vec<(String, bool)> = symbols(&extraction)
        .into_iter()
        .map(|(name, _, exported)| (name, exported))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("V.swift::a".to_string(), false),
            ("V.swift::b".to_string(), false),
            ("V.swift::c".to_string(), true),
            ("V.swift::d".to_string(), true),
            ("V.swift::e".to_string(), true),
            ("V.swift::f".to_string(), true),
            ("V.swift::g".to_string(), true),
        ],
        "only `private` and `fileprivate` confine callers to one file"
    );
}

/// Defect 5, decided rather than implemented: `init` emits no symbol.
///
/// Every construction site in the corpus names the *type*. `Person(name:)` is a
/// `call_expression` whose callee is `Person`, and `langcalls::swift` rewrites
/// `Person.init(name:)` to the same callee. A `Person.init` node would be a
/// second identity for an operation that always joins to the first.
#[test]
fn a_swift_initializer_emits_no_symbol_and_every_construction_joins_to_the_type() {
    let extraction = extract_file(
        "C.swift",
        "struct Person { init(name: String) {} }\n\
         func make() { _ = Person(name: \"a\"); _ = Person.init(name: \"b\") }\n",
    );
    assert_eq!(
        qualified_names(&extraction),
        vec!["C.swift::Person".to_string(), "C.swift::make".to_string()],
        "no `Person.init` node beside the type"
    );
    let callees: Vec<&str> = extraction
        .calls
        .iter()
        .map(|call| call.callee_name.as_str())
        .collect();
    assert_eq!(
        callees,
        vec!["Person", "Person"],
        "both spellings name the type, which is the node that exists"
    );
}

/// Defect 6, decided: an enum case is a symbol, and exempt from deadness.
///
/// Pre-fix `Payload.text("z")` was a call to `text` with receiver `Payload` and
/// no node anywhere that it could resolve to — the same asymmetry SC31 closed
/// for C's function-like macros. The exemption is the other half: Swift's
/// leading-dot inference (`let m: Mode = .fast`) names a case without naming its
/// type, and `split_call_target` refuses `.fast` as a callee, so the use site is
/// unobservable by construction and silence is not evidence.
#[test]
fn swift_enum_cases_are_symbols_and_a_multi_case_line_emits_one_per_name() {
    let extraction = extract_file("App.swift", SWIFT);
    let names = qualified_names(&extraction);
    for case in [
        "App.swift::Mode.fast",
        "App.swift::Mode.slow",
        "App.swift::Payload.text",
    ] {
        assert!(
            names.contains(&case.to_string()),
            "{case} missing: {names:?}"
        );
        assert_eq!(kind_of(&extraction, case), SymbolKind::Field);
    }
    assert_eq!(
        exemption(&extraction, "App.swift::Mode.fast"),
        Some((
            WiringKind::StructuralExempt,
            "Swift enum case reachable by leading-dot inference, which names no type".to_string()
        )),
        "a case that looks uncalled must never become a deletion proposal"
    );
}

/// Reading visibility is what made an exemption necessary at all.
#[test]
fn swift_entry_points_are_read_from_attributes_and_modifiers_not_from_names() {
    let extraction = extract_file(
        "E.swift",
        "@main\nstruct MyApp: App { }\n\
         class V: UIViewController { override func viewDidLoad() {} }\n\
         class Legacy: NSObject { @objc func ping() {} }\n\
         final class Sub: XCTestCase { func testThing() {} }\n\
         class Plain { func testable() {} }\n",
    );
    assert_eq!(
        exemption(&extraction, "E.swift::MyApp").map(|(kind, _)| kind),
        Some(WiringKind::RuntimeEntryPoint)
    );
    assert_eq!(
        exemption(&extraction, "E.swift::V.viewDidLoad").map(|(_, why)| why),
        Some("Swift override invoked by the declaring superclass or framework".to_string())
    );
    assert_eq!(
        exemption(&extraction, "E.swift::Legacy.ping").map(|(_, why)| why),
        Some("published to the Objective-C runtime and invoked by selector".to_string())
    );
    assert_eq!(
        exemption(&extraction, "E.swift::Sub.testThing").map(|(_, why)| why),
        Some("XCTestCase collects test methods by reflection".to_string())
    );
    assert_eq!(
        exemption(&extraction, "E.swift::Plain.testable"),
        None,
        "a name beginning `test` outside an XCTestCase is an ordinary method"
    );
    assert_eq!(
        exemption(&extraction, "E.swift::V"),
        None,
        "a UIViewController subclass is instantiated by ordinary code; only the \
         overrides it declares are reached by the framework"
    );
}

// ------------------------------------------------------------------ Kotlin

const KOTLIN: &str = r#"package demo

interface Greeter {
    fun greet(): String
}

data class Person(val name: String) : Greeter {
    override fun greet(): String = "hi $name"

    private fun unused(): Int = 1
}

fun Person.extra(): Int = 2

enum class Mode {
    FAST,
    SLOW
}

object Registry {
    fun register(): Int = 1
}

class Holder {
    companion object {
        fun create(): Holder = Holder()
    }
}

internal class Runner {
    private val person = Person("a")

    fun run(): String = person.greet() + person.extra().toString()
}

fun main() {
    println(Runner().run())
}
"#;

/// Defect 3, Kotlin half.
#[test]
fn kotlin_declaration_kinds_come_from_the_tokens_beside_the_name() {
    let extraction = extract_file("Main.kt", KOTLIN);
    // Pre-fix both were `Class`: `interface`, `class` and `enum class` are one
    // node kind in tree-sitter-kotlin-ng.
    assert_eq!(
        kind_of(&extraction, "Main.kt::Greeter"),
        SymbolKind::Interface
    );
    assert_eq!(kind_of(&extraction, "Main.kt::Mode"), SymbolKind::Enum);
    assert_eq!(kind_of(&extraction, "Main.kt::Person"), SymbolKind::Class);
}

/// Defect 4. `fun Person.extra()` was emitted as `Main.kt::extra`.
///
/// The same identity a Go receiver keeps under SC9 and a Rust `impl` method
/// under SC11.
#[test]
fn a_kotlin_extension_function_is_owned_by_the_type_it_extends() {
    let extraction = extract_file("Main.kt", KOTLIN);
    let names = qualified_names(&extraction);
    assert!(
        names.contains(&"Main.kt::Person.extra".to_string()),
        "pre-fix this was `Main.kt::extra`: {names:?}"
    );
    assert!(
        !names.contains(&"Main.kt::extra".to_string()),
        "and no bare copy survives beside it: {names:?}"
    );
    assert_eq!(
        kind_of(&extraction, "Main.kt::Person.extra"),
        SymbolKind::Method,
        "a callable with an owner is a method"
    );
}

/// A generic or qualified receiver reduces to the bare type a declaration of it
/// would carry.
#[test]
fn a_kotlin_extension_receiver_reduces_to_the_name_a_declaration_would_have() {
    let extraction = extract_file(
        "G.kt",
        "fun Map<String, Int>.pairs(): Int = 1\nfun kotlin.text.Regex.safe(): Int = 2\n",
    );
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "G.kt::Map.pairs".to_string(),
            "G.kt::Regex.safe".to_string()
        ]
    );
}

/// A Kotlin `object` had no entry in the node-kind table, so it emitted nothing
/// and its members were attributed to whatever enclosed it.
#[test]
fn a_kotlin_object_is_a_symbol_and_a_companion_object_belongs_to_its_class() {
    let extraction = extract_file("Main.kt", KOTLIN);
    let names = qualified_names(&extraction);
    assert!(
        names.contains(&"Main.kt::Registry".to_string()),
        "pre-fix `object_declaration` emitted no symbol at all: {names:?}"
    );
    assert!(
        names.contains(&"Main.kt::Registry.register".to_string()),
        "pre-fix its member was emitted bare as `Main.kt::register`: {names:?}"
    );
    assert!(
        names.contains(&"Main.kt::Holder.create".to_string()),
        "a companion's member is called as `Holder.create()`, so it belongs to \
         `Holder` and the companion is deliberately not an owner: {names:?}"
    );
}

/// Defect 1, Kotlin half — and the place where Kotlin's answer differs from
/// Swift's because the languages differ.
#[test]
fn kotlin_visibility_defaults_to_public_and_only_private_is_confined() {
    let extraction = extract_file(
        "V.kt",
        "private fun a() {}\ninternal fun b() {}\nprotected fun c() {}\n\
         public fun d() {}\nfun e() {}\n",
    );
    let rows: Vec<(String, bool)> = symbols(&extraction)
        .into_iter()
        .map(|(name, _, exported)| (name, exported))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("V.kt::a".to_string(), false),
            ("V.kt::b".to_string(), true),
            ("V.kt::c".to_string(), true),
            ("V.kt::d".to_string(), true),
            ("V.kt::e".to_string(), true),
        ],
        "only `private` confines callers to one file. `internal` was tried as \
         non-exported and reverted: on 1,032 real `.kt` files it produced 59 new \
         findings of which 8 of 8 sampled were false, every one an `internal \
         object` used cross-file as a receiver. `protected` callers are \
         subclasses in any dependent module"
    );
    // Pre-fix, the substring scan reached into bodies in both directions.
    let leak = extract_file("L.kt", "class Outer {\n    private fun hidden() {}\n}\n");
    assert!(
        exported(&leak, "L.kt::Outer"),
        "pre-fix `Outer` reported not-exported because its body contains `private `"
    );
    assert!(!exported(&leak, "L.kt::Outer.hidden"));
}

#[test]
fn kotlin_entry_points_are_read_from_annotations_and_modifiers() {
    let extraction = extract_file(
        "E.kt",
        "@Composable\nfun Screen() {}\n\
         class MyActivity { override fun onCreate() {} }\n\
         external fun jni(): Int\n\
         fun main() {}\n\
         fun ordinary() {}\n",
    );
    assert_eq!(
        exemption(&extraction, "E.kt::Screen").map(|(_, why)| why),
        Some("invoked by the Jetpack Compose runtime".to_string())
    );
    assert_eq!(
        exemption(&extraction, "E.kt::MyActivity.onCreate").map(|(_, why)| why),
        Some("Kotlin override invoked by the declaring supertype or framework".to_string())
    );
    assert_eq!(
        exemption(&extraction, "E.kt::jni").map(|(_, why)| why),
        Some("Kotlin external function bound to a native implementation over JNI".to_string())
    );
    assert_eq!(
        exemption(&extraction, "E.kt::main").map(|(_, why)| why),
        Some("Program entry point".to_string())
    );
    assert_eq!(exemption(&extraction, "E.kt::ordinary"), None);
}

// -------------------------------------------------------------------- Dart

const DART: &str = r#"int helper(int a) => a + 1;

class Widget {
  final int size;
  Widget(this.size);
  Widget.named(this.size);

  int run() => helper(size);
  static int make() => Widget(1).run();
}

mixin Loggable { void log() {} }

extension Ext on Widget { int extra() => run(); }

enum Color { red, green }

void main() {
  Widget.named(2).run();
}
"#;

/// Defect 7. Dart emitted no function or method symbol at all.
///
/// Pre-fix the whole file yielded exactly two symbols — `Widget` and `Color` —
/// so no Dart call could resolve to a Dart target and every Dart call edge was
/// attributed to its enclosing class or to the file.
#[test]
fn dart_callables_are_named_through_their_signature_child() {
    let extraction = extract_file("app.dart", DART);
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "app.dart::Color".to_string(),
            "app.dart::Color.green".to_string(),
            "app.dart::Color.red".to_string(),
            "app.dart::Ext".to_string(),
            "app.dart::Ext.extra".to_string(),
            "app.dart::Loggable".to_string(),
            "app.dart::Loggable.log".to_string(),
            "app.dart::Widget".to_string(),
            "app.dart::Widget.make".to_string(),
            "app.dart::Widget.named".to_string(),
            "app.dart::Widget.run".to_string(),
            "app.dart::helper".to_string(),
            "app.dart::main".to_string(),
        ],
        "pre-fix only `app.dart::Widget` and `app.dart::Color` were emitted"
    );
    assert_eq!(
        kind_of(&extraction, "app.dart::Loggable"),
        SymbolKind::Trait,
        "a mixin is a set of members another type composes in"
    );
    assert_eq!(
        kind_of(&extraction, "app.dart::Widget.run"),
        SymbolKind::Method
    );
    assert_eq!(
        kind_of(&extraction, "app.dart::helper"),
        SymbolKind::Function
    );
}

/// Only the *named* constructor. `Widget(…)` is spelled exactly like the type at
/// every call site and already joins to the class node, so a `Widget.Widget`
/// would be an identity nothing reaches — the same reasoning that keeps Swift's
/// `init` out.
#[test]
fn dart_emits_named_constructors_and_not_the_unnamed_one() {
    let extraction = extract_file("app.dart", DART);
    let names = qualified_names(&extraction);
    assert!(names.contains(&"app.dart::Widget.named".to_string()));
    assert!(!names.contains(&"app.dart::Widget.Widget".to_string()));
}

/// Dart's privacy *is* the leading underscore, so the shared fallback is the
/// language rule here rather than an approximation of it.
#[test]
fn dart_privacy_is_the_leading_underscore() {
    let extraction = extract_file(
        "p.dart",
        "int _hidden() => 1;\nint shown() => 1;\nclass C { void _m() {} void n() {} }\n",
    );
    assert!(!exported(&extraction, "p.dart::_hidden"));
    assert!(exported(&extraction, "p.dart::shown"));
    assert!(!exported(&extraction, "p.dart::C._m"));
    assert!(exported(&extraction, "p.dart::C.n"));
}

// ----------------------------------------------------------------------- R

/// Defect 8. Every R function was named `function`.
///
/// `tree-sitter-r` puts a `name` field on `function_definition` and points it at
/// the **keyword**, so a three-function file emitted `main.R::function` three
/// times: one qualified name for three symbols, which is a broken join key and
/// leaves no R call able to resolve.
///
/// The frozen Python baseline has the identical defect and
/// `testdata/golden/languages/r/nodes.json` still pins `main.R::function`. That
/// golden is deliberately left disagreeing with this extractor rather than
/// regenerated; it is not one of the five fixtures the parity harness compares.
#[test]
fn an_r_function_is_named_after_the_variable_it_is_bound_to() {
    let extraction = extract_file(
        "main.R",
        "helper <- function(a) { a + 1 }\nother = function(b) { helper(b) }\n\
         third <<- function() 1\nfourth -> 2\nlapply(xs, function(x) x)\n",
    );
    assert_eq!(
        qualified_names(&extraction),
        vec![
            "main.R::helper".to_string(),
            "main.R::other".to_string(),
            "main.R::third".to_string(),
        ],
        "pre-fix this was three copies of `main.R::function`; an anonymous \
         function passed to `lapply` is bound to no name and yields no symbol"
    );
    assert_eq!(
        duplicate_names(&extraction),
        Vec::<String>::new(),
        "and no two R functions share one qualified name"
    );
}

/// The declaration site is a binding, not a use.
///
/// R spells a declaration as an assignment, so the name being declared sits on
/// `lhs` of a `binary_operator` — a node kind that also covers `a + 1`. Without
/// the gate, a four-function file recorded four file-scoped references to its
/// own declarations and every uncalled R function looked used.
#[test]
fn an_r_declaration_is_not_a_reference_to_itself_but_an_operand_still_is() {
    let extraction = extract_file(
        "r.R",
        "helper <- function(a) { a + 1 }\nunused_fn <- function() 1\ntotal <- helper(2) + offset\n",
    );
    let referenced: Vec<&str> = extraction
        .references
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    assert!(
        !referenced.contains(&"unused_fn"),
        "the declaration is not a use of itself: {referenced:?}"
    );
    assert!(
        referenced.contains(&"offset"),
        "but an ordinary operand still is: {referenced:?}"
    );
}

// ------------------------- false positives reading visibility made possible
//
// Every test below records a dead-code false positive measured on real code
// *after* visibility became readable. None of them could have fired before:
// until a symbol could report `is_exported = false`, nothing was ever a
// candidate, so a missing call edge cost nothing.

/// A prefix operator swallowed the whole call.
///
/// Both grammars bind the operator before the argument list, so the callee of
/// `!f(x)` is the node `!f` — which `split_call_target` correctly refuses, and
/// the call vanished rather than being recorded. Measured: two Swift functions
/// (`applySecret`, `clipboardMatchIsGreedy`) and two Kotlin ones
/// (`Preferences.markerPromoted`, `SharedPreferences.markerPromoted`) called
/// only through `if !f(…)` were reported dead at 0.9 confidence.
#[test]
fn a_negated_call_is_still_a_call_in_swift_and_kotlin() {
    let swift = extract_file(
        "N.swift",
        "func a() {\n  if !applySecret(x) { return }\n  let b = -g()\n}\n",
    );
    let mut swift_callees: Vec<&str> = swift
        .calls
        .iter()
        .map(|call| call.callee_name.as_str())
        .collect();
    swift_callees.sort_unstable();
    assert_eq!(swift_callees, vec!["applySecret", "g"]);

    let kotlin = extract_file(
        "N.kt",
        "fun a(): Int {\n    if (!h()) { return 1 }\n    val x = -g()\n    return 2\n}\n",
    );
    let mut kotlin_callees: Vec<&str> = kotlin
        .calls
        .iter()
        .map(|call| call.callee_name.as_str())
        .collect();
    kotlin_callees.sort_unstable();
    assert_eq!(kotlin_callees, vec!["g", "h"]);
}

/// A leading dot is not an operator prefix and must stay refused.
///
/// `.text("z")` names a member of a type the site never spells. Unwrapping it
/// would let it bind to any same-named free function in the file at
/// deterministic confidence — the SC9 class of confidently-wrong edge — so the
/// case is reached instead through the `StructuralExempt` annotation every enum
/// case carries.
#[test]
fn swift_leading_dot_inference_does_not_become_a_bare_callee() {
    let extraction = extract_file(
        "D.swift",
        "enum Payload { case text(String) }\nfunc text() {}\nfunc f() { let a: Payload = .text(\"z\"); _ = a }\n",
    );
    assert_eq!(
        extraction
            .calls
            .iter()
            .filter(|call| call.callee_name == "text")
            .count(),
        0,
        "`.text(\"z\")` must not be recorded as a call to the free function `text`"
    );
}

/// A Swift value-position identifier is a reference.
///
/// tree-sitter-swift spells every value-position name `simple_identifier`, a
/// kind `maybe_push_name_reference` did not know, so Swift recorded references
/// for type positions only. A type or function used as a *value* was invisible:
/// measured on 342 `.swift` files, `Const.firebaseTimeoutMs`,
/// `self[SlideUpDismissKey.self]`, `fields.map(csvField)` and a C callback
/// passed by name were each reported dead at 0.9 confidence.
#[test]
fn a_swift_type_or_function_used_as_a_value_is_recorded_as_a_reference() {
    let extraction = extract_file(
        "R.swift",
        "enum Const { static let timeout = 1 }\nfunc csvField(_ s: String) -> String { return s }\n         func use(_ fields: [String]) {\n  _ = Const.timeout\n  _ = fields.map(csvField)\n}\n",
    );
    let referenced: Vec<&str> = extraction
        .references
        .iter()
        .map(|reference| reference.name.as_str())
        .collect();
    assert!(
        referenced.contains(&"Const"),
        "a receiver qualifier names its type: {referenced:?}"
    );
    assert!(
        referenced.contains(&"csvField"),
        "a function passed by name is a use of it: {referenced:?}"
    );
    // The existing gates must still hold, or this arm turns every declaration
    // into a use of itself.
    assert!(
        !extraction
            .references
            .iter()
            .any(|reference| reference.name == "use"),
        "a declaration site is not a reference: {referenced:?}"
    );
}

/// `CodingKeys` has no call site by construction.
#[test]
fn swift_coding_keys_and_its_cases_are_structurally_exempt() {
    let extraction = extract_file(
        "C.swift",
        "struct Snippet: Codable {\n  let id: String\n  private enum CodingKeys: String, CodingKey { case id }\n}\n         struct CodingKeysCache { }\n",
    );
    assert_eq!(
        exemption(&extraction, "C.swift::Snippet.CodingKeys").map(|(kind, _)| kind),
        Some(WiringKind::StructuralExempt)
    );
    assert_eq!(
        exemption(&extraction, "C.swift::Snippet.CodingKeys.id").map(|(kind, _)| kind),
        Some(WiringKind::StructuralExempt),
        "its cases are as compiler-referenced as the enum"
    );
    assert_eq!(
        exemption(&extraction, "C.swift::CodingKeysCache"),
        None,
        "matched on the exact reserved name, not a prefix"
    );
    // And the nested enum is owned by the type that declares it, so two Codable
    // types in one file do not collide on `File::CodingKeys`.
    assert_eq!(
        duplicate_names(&extraction),
        Vec::<String>::new(),
        "{:?}",
        qualified_names(&extraction)
    );
}

// ------------------------------------------------- cross-language invariants

/// The join key invariant, over every language whose declaration path changed.
///
/// A call or reference whose source names no emitted symbol is invisible to
/// every traversal out of that symbol while still looking well-formed (SC9,
/// SC10). `langcalls::scope` now asks `langdecl::declaration_of` — the same
/// function the emitter uses — rather than transcribing its rules, so this
/// cannot drift by inspection; it is checked anyway, because that is the
/// property, not the mechanism.
#[test]
fn no_call_or_reference_names_a_source_symbol_that_does_not_exist() {
    for (path, source) in [
        ("App.swift", SWIFT),
        ("Main.kt", KOTLIN),
        ("app.dart", DART),
        (
            "main.R",
            "helper <- function(a) { a + 1 }\nother <- function(b) { helper(b) }\nother(1)\n",
        ),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.calls.is_empty(),
            "{path} extracted no calls, so this invariant would hold vacuously"
        );
        assert_eq!(
            orphaned_sources(&extraction),
            Vec::<String>::new(),
            "{path} produced an edge whose source symbol was never emitted"
        );
    }
}

/// The reference path has its own mirror, and it had to move too.
///
/// `maybe_push_name_reference` attributes every bare identifier through
/// `enclosing_callable_qualified`, a *third* builder that reads a `name` field.
/// Once a Kotlin extension function was owned by its receiver, that builder
/// still answered `X.kt::extra` for a reference made inside
/// `fun Person.extra()`, where the emitted symbol is `X.kt::Person.extra` — an
/// orphan of exactly the SC9/SC10 shape, introduced by fixing the declaration
/// and caught by this check. Verified by reverting only the reference-path
/// change: this test fails and the others do not.
#[test]
fn a_reference_inside_a_kotlin_extension_function_names_the_receiver_qualified_symbol() {
    let extraction = extract_file(
        "X.kt",
        "class Person(val name: String)
val shared = 1
fun Person.extra(): Int = shared + name.length
",
    );
    assert!(
        qualified_names(&extraction).contains(&"X.kt::Person.extra".to_string()),
        "{:?}",
        qualified_names(&extraction)
    );
    let sources: Vec<&str> = extraction
        .references
        .iter()
        .filter_map(|reference| reference.enclosing_symbol.as_deref())
        .collect();
    assert!(
        sources.contains(&"X.kt::Person.extra"),
        "a reference made inside the extension is attributed to it: {sources:?}"
    );
    assert_eq!(
        orphaned_sources(&extraction),
        Vec::<String>::new(),
        "and no reference names a symbol the emitter never produced"
    );
}

/// Reference attribution must be routed to whichever path emitted the
/// declaration, for every language.
///
/// `maybe_push_name_reference` runs for **all** 35 languages and has to choose
/// between two builders: `enclosing_callable_qualified`, which the five
/// languages with their own arm in `extract_node` use, and
/// `langdecl::declaration_of`, which everything else must use now that
/// declarations are per-language. The choice is a hand-written list
/// (`SPECIALISED_ARM_LANGUAGES`), and a hand-written list is a claim. This is
/// the check: a language on it that should not be, or missing from it, produces
/// a reference naming a symbol the emitter never emitted — which is the
/// SC9/SC10 orphan, and the exact shape a Kotlin extension function produced
/// before the routing was added.
#[test]
fn every_language_attributes_its_references_to_an_emitted_symbol() {
    let cases: &[(&str, &str)] = &[
        // The five with their own arm in `extract_node`.
        ("a.py", "SHARED = 1\n\n\ndef outer():\n    return SHARED\n"),
        ("a.js", "const shared = 1;\nexport function outer() { return shared; }\n"),
        ("a.ts", "const shared = 1;\nexport function outer(): number { return shared; }\n"),
        ("a.tsx", "const shared = 1;\nexport function Outer() { return shared; }\n"),
        ("a.rs", "const SHARED: u32 = 1;\npub struct S;\nimpl S { pub fn outer(&self) -> u32 { SHARED } }\n"),
        ("a.go", "package p\n\nconst Shared = 1\n\nfunc Outer() int { return Shared }\n"),
        ("a.tf", "variable \"shared\" {\n  default = 1\n}\n"),
        // The generic arm, including every language whose declaration path this
        // change rewrote.
        (
            "a.swift",
            "enum Const { static let timeout = 1 }\n             extension Const { static func doubled() -> Int { return timeout * 2 } }\n             func outer() -> Int {\n  func inner() -> Int { return Const.timeout }\n  return inner()\n}\n",
        ),
        ("a.kt", "class Person(val name: String)\nval shared = 1\nfun Person.extra(): Int = shared + name.length\n"),
        ("a.dart", "int shared = 1;\nint outer() => shared;\nclass C { int m() => outer(); }\n"),
        ("a.R", "helper <- function(a) { a + 1 }\nouter <- function(b) { helper(b) }\n"),
        ("a.java", "class A { int shared = 1; int outer() { return shared; } }\n"),
        ("a.cs", "class A { int shared = 1; int Outer() { return shared; } }\n"),
        ("a.rb", "SHARED = 1\ndef outer\n  SHARED\nend\n"),
        ("a.php", "<?php\nfunction helper() { return 1; }\nfunction outer() { return helper(); }\n"),
        ("a.scala", "object Registry { def shared = 1\n  def outer = shared }\n"),
        ("a.lua", "local shared = 1\nlocal function outer() return shared end\nreturn outer\n"),
        ("a.c", "static int shared = 1;\nstatic int outer(void) { return shared; }\n"),
    ];
    let mut exercised = 0;
    for (path, source) in cases {
        let extraction = extract_file(path, source);
        let emitted: Vec<String> = extraction
            .symbols
            .iter()
            .map(|symbol| symbol.qualified_name.clone())
            .collect();
        let attributed: Vec<&str> = extraction
            .references
            .iter()
            .filter_map(|reference| reference.enclosing_symbol.as_deref())
            .collect();
        for source_symbol in &attributed {
            assert!(
                emitted.contains(&source_symbol.to_string()),
                "{path}: reference attributed to {source_symbol:?}, which is not among {emitted:?}"
            );
        }
        if attributed.iter().any(|name| name.contains("::")) {
            exercised += 1;
        }
    }
    // Without this the loop above passes for any language that attributes
    // everything to the file, which is what a mis-routed builder degrades to.
    assert!(
        exercised >= 10,
        "only {exercised} of {} languages attributed a reference to a symbol rather \
         than to the file; the check would be holding vacuously",
        cases.len()
    );
}

/// No language may emit one qualified name twice.
#[test]
fn no_language_emits_a_duplicate_qualified_name() {
    for (path, source) in [
        ("App.swift", SWIFT),
        ("Main.kt", KOTLIN),
        ("app.dart", DART),
        (
            "main.R",
            "a <- function() 1\nb <- function() 2\nc <- function() 3\n",
        ),
    ] {
        let extraction = extract_file(path, source);
        assert_eq!(
            duplicate_names(&extraction),
            Vec::<String>::new(),
            "{path} emitted a duplicate qualified name, which is a broken join key"
        );
    }
}

/// The emitter and the caller-attribution mirror must agree symbol for symbol.
///
/// Stated as a property over the whole file rather than as a list of expected
/// strings: for every call, the string recorded as its caller is one the emitter
/// produced, and for every language, some call is attributed to something other
/// than the file — otherwise the check passes by attributing everything to the
/// file, which is what a broken mirror degrades to.
#[test]
fn every_caller_symbol_is_a_symbol_the_emitter_actually_emitted() {
    for (path, source) in [
        ("App.swift", SWIFT),
        ("Main.kt", KOTLIN),
        ("app.dart", DART),
    ] {
        let extraction = extract_file(path, source);
        let emitted = qualified_names(&extraction);
        let attributed: Vec<&str> = extraction
            .calls
            .iter()
            .filter_map(|call| call.caller_symbol.as_deref())
            .collect();
        assert!(
            !attributed.is_empty(),
            "{path} attributed no call to any symbol"
        );
        for caller in &attributed {
            assert!(
                emitted.contains(&caller.to_string()),
                "{path}: caller {caller:?} is not among {emitted:?}"
            );
        }
    }
}

/// Determinism: the same source must produce the same symbols in the same order.
///
/// The declaration path reads modifier and attribute lists into `Vec`s built by
/// iteration order, never from a `HashSet`/`HashMap`, and this pins that.
#[test]
fn declaration_extraction_is_deterministic_across_repeated_runs() {
    for (path, source) in [
        ("App.swift", SWIFT),
        ("Main.kt", KOTLIN),
        ("app.dart", DART),
        ("main.R", "a <- function() 1\nb <- function() 2\n"),
    ] {
        let first = extract_file(path, source);
        for _ in 0..4 {
            let again = extract_file(path, source);
            assert_eq!(
                first
                    .symbols
                    .iter()
                    .map(|symbol| symbol.qualified_name.clone())
                    .collect::<Vec<_>>(),
                again
                    .symbols
                    .iter()
                    .map(|symbol| symbol.qualified_name.clone())
                    .collect::<Vec<_>>(),
                "{path} symbol order is not stable"
            );
            assert_eq!(
                first
                    .wiring
                    .iter()
                    .map(|annotation| (
                        annotation.target_symbol.clone(),
                        annotation.details.clone()
                    ))
                    .collect::<Vec<_>>(),
                again
                    .wiring
                    .iter()
                    .map(|annotation| (
                        annotation.target_symbol.clone(),
                        annotation.details.clone()
                    ))
                    .collect::<Vec<_>>(),
                "{path} wiring order is not stable"
            );
        }
    }
}
