//! Adversarial probes for the Swift residue lane (`swiftscope.rs`, `langfields.rs`)
//! at 9661bb6a. Each test states the CORRECT behaviour; a failing test is a BREAK.
//!
//! House contract: a wrong type or edge is worse than an abstention; the lane
//! must not lose a binding main has; output is deterministic; walks are bounded.

use devmap_extract::extract_file;
use devmap_extract::model::{LocalBinding, ReferenceKind};

fn bindings(source: &str) -> Vec<LocalBinding> {
    let mut all = extract_file("a.swift", source).local_bindings;
    all.sort_by_key(|binding| binding.start_byte);
    all
}

/// The declared type the extractor states for the first use of `name`.
fn declared(source: &str, name: &str) -> Option<String> {
    bindings(source)
        .into_iter()
        .find(|binding| binding.name == name)
        .and_then(|binding| binding.declared_type)
}

/// The initializer shape the extractor states for the first use of `name`.
fn shape(source: &str, name: &str) -> Option<String> {
    bindings(source)
        .into_iter()
        .find(|binding| binding.name == name)
        .and_then(|binding| binding.initializer)
}

fn field(source: &str, name: &str) -> Option<String> {
    extract_file("a.swift", source)
        .references
        .into_iter()
        .filter(|reference| reference.kind == ReferenceKind::Type)
        .find(|reference| reference.assigned_to.as_deref() == Some(name))
        .map(|reference| reference.name)
}

const PAIR: &str = "struct Store { func go() {} }\nstruct Svc { func go() {} }\n\
     func makePair() -> (Store, Svc) { (Store(), Svc()) }\n";

const SNAP: &str = "struct Snap { func go() {} }\n\
     struct Store {\n    func snapshot(_ p: Int) -> Snap { Snap() }\n\
     \x20   func map(_ p: Int) -> Snap { Snap() }\n}\n";

// ---------------------------------------------------------------- tuples

/// `let (a, b): (P, P) = makePair()` states its own type. The function's element
/// types are not the locals' types: the locals are `P`.
#[test]
fn a_tuple_pattern_with_its_own_annotation_is_not_typed_by_the_function() {
    let source = "protocol P { func go() }\n\
         struct Store: P { func go() {} }\nstruct Svc: P { func go() {} }\n\
         func makePair() -> (Store, Svc) { (Store(), Svc()) }\n\
         func t() { let (a, b): (P, P) = makePair(); a.go(); b.go() }";
    assert_ne!(
        declared(source, "a").as_deref(),
        Some("Store"),
        "a is a P by its written annotation: {:?}",
        bindings(source)
    );
    assert_ne!(declared(source, "b").as_deref(), Some("Svc"));
}

/// `load(path:)` is not `load(id:)`: the file's function cannot accept this call,
/// so the call is another overload (another file, another module). Main states
/// that a function with parameters never states what a bare `name()` returns; the
/// tuple rule drops that and does not even compare labels.
#[test]
fn a_tuple_call_the_file_level_function_cannot_accept_is_not_typed_by_it() {
    let source = "struct Store { func go() {} }\nstruct Svc { func go() {} }\n\
         func load(id: Int) -> (Store, Svc) { (Store(), Svc()) }\n\
         func t() { let (a, b) = load(path: \"x\"); a.go(); b.go() }";
    assert_eq!(
        declared(source, "a"),
        None,
        "load(path:) is not load(id:): {:?}",
        bindings(source)
    );
    assert_eq!(declared(source, "b"), None);
}

/// Inside `H`, an unqualified `makePair()` finds a member of `H` (or of its
/// superclass) before a free function; Swift stops at the member. `Base` is not
/// in this file, so the lane cannot know it has none.
#[test]
fn a_destructure_inside_a_subclass_does_not_claim_the_file_level_function() {
    let source =
        format!("{PAIR}class H: Base {{ func t() {{ let (a, b) = makePair(); a.go(); b.go() }} }}");
    assert_eq!(
        declared(&source, "a"),
        None,
        "Base may declare makePair(): {:?}",
        bindings(&source)
    );
}

/// In `extension P`, `makePair()` is the requirement `P` declares, not the free
/// function: the member is found first. A protocol requirement is not a
/// `function_declaration`, so the one-function count does not see it.
#[test]
fn a_destructure_in_a_protocol_extension_is_not_typed_by_a_same_named_requirement() {
    let source = format!(
        "{PAIR}protocol P {{ func makePair() -> (Svc, Store) }}\n\
         extension P {{ func t() {{ let (a, b) = makePair(); a.go(); b.go() }} }}"
    );
    assert_ne!(
        declared(&source, "a").as_deref(),
        Some("Store"),
        "P.makePair() returns (Svc, Store): {:?}",
        bindings(&source)
    );
    assert_ne!(declared(&source, "b").as_deref(), Some("Svc"));
}

/// Locals are keyed per function, not per block. A destructure in an `if` block
/// binds `a` for the whole function, so the member `a` used after the block is
/// read as the destructured local: `Store`, where the member is an `Other`. Main
/// bound no tuple name at all, so the member read was its own.
#[test]
fn a_block_scoped_destructure_does_not_retype_a_member_used_after_the_block() {
    let source = format!(
        "{PAIR}struct Other {{ func run() {{}} }}\n\
         final class H {{\n    var a: Other = Other()\n\
         \x20   func go(_ flag: Bool) {{\n        if flag {{ let (a, b) = makePair(); print(b) }}\n\
         \x20       a.run()\n    }}\n}}"
    );
    let use_after = source.rfind("a.run()").expect("the member use");
    let binding = bindings(&source)
        .into_iter()
        .find(|binding| binding.name == "a" && binding.start_byte == use_after);
    assert_ne!(
        binding.and_then(|binding| binding.declared_type).as_deref(),
        Some("Store"),
        "the `a` after the block is H.a, an Other"
    );
}

/// A single nested `Store` is `Store` only inside the type that holds it. The
/// function returns the top-level one (another file's); at the call, inside `R`,
/// the spelling is `R.Store`. The wrapper path checks this and the tuple path
/// does not.
#[test]
fn a_tuple_element_that_is_only_a_nested_type_here_is_not_the_nested_type() {
    let source = "struct Svc { func go() {} }\n\
         func makePair() -> (Store, Svc) { fatalError() }\n\
         struct R {\n    struct Store { func go() {} }\n\
         \x20   func t() { let (a, b) = makePair(); a.go(); b.go() }\n}";
    assert_eq!(
        declared(source, "a"),
        None,
        "inside R, `Store` is R.Store; makePair returns another Store: {:?}",
        bindings(source)
    );
}

// ------------------------------------------------- parameter-receiver shape

/// `case .failure(let store)` hides the parameter `store`. The pattern binding is
/// recorded with `entry().or_default()`, which keeps the parameter's type, so the
/// shape reads `Store.snapshot` for a value that is not a `Store`.
#[test]
fn a_case_let_that_hides_a_parameter_does_not_qualify_the_receiver() {
    let source = format!(
        "{SNAP}func t(_ store: Store, _ r: Res) {{\n    switch r {{\n\
         \x20   case .failure(let store):\n        let s = store.snapshot(1)\n        s.go()\n\
         \x20   default: break\n    }}\n}}"
    );
    assert_ne!(
        shape(&source, "s").as_deref(),
        Some("Store.snapshot"),
        "store here is the case's payload, not the parameter: {:?}",
        bindings(&source)
    );
}

/// The same for `catch let store`, and for the commonest case of it: a parameter
/// `error` hidden by `case .failure(let error)`.
#[test]
fn a_catch_let_that_hides_a_parameter_does_not_qualify_the_receiver() {
    let source = format!(
        "{SNAP}func t(_ store: Store) {{\n    do {{ try f() }} catch let store {{\n\
         \x20       let s = store.snapshot(1)\n        s.go()\n    }}\n}}"
    );
    assert_ne!(
        shape(&source, "s").as_deref(),
        Some("Store.snapshot"),
        "{:?}",
        bindings(&source)
    );
}

/// `store` is an `Optional<Store>`: `store.map(..)` is `Optional.map`, not
/// `Store.map`. The binding is unwrapped to `Store`, and only a written `?` is
/// treated as optional.
#[test]
fn an_optional_parameter_does_not_qualify_optional_map() {
    for method in ["map", "flatMap"] {
        let source = format!(
            "struct Snap {{ func go() {{}} }}\n\
             struct Store {{ func {method}(_ p: Int) -> Snap {{ Snap() }} }}\n\
             func t(_ store: Store?) {{ let s = store.{method}(1); s.go() }}"
        );
        assert_ne!(
            shape(&source, "s"),
            Some(format!("Store.{method}")),
            "Optional.{method} is not Store.{method}: {:?}",
            bindings(&source)
        );
    }
}

/// `Mod.Store` is another module's `Store`. The type reader keeps the last
/// segment, so the qualified spelling reads `Store.snapshot` and meets this
/// corpus's own `Store`.
#[test]
fn a_module_qualified_parameter_does_not_qualify_a_corpus_type() {
    let source =
        format!("{SNAP}func t(_ store: Mod.Store) {{ let s = store.snapshot(1); s.go() }}");
    assert_ne!(
        shape(&source, "s").as_deref(),
        Some("Store.snapshot"),
        "Mod.Store is not this file's Store: {:?}",
        bindings(&source)
    );
}

/// `Store.Type` is a metatype, not a type named `Type`.
#[test]
fn a_metatype_parameter_is_not_a_type_named_type() {
    let source =
        format!("{SNAP}func t(_ store: Store.Type) {{ let s = store.snapshot(1); s.go() }}");
    assert_ne!(
        shape(&source, "s").as_deref(),
        Some("Type.snapshot"),
        "{:?}",
        bindings(&source)
    );
    assert_ne!(declared(&source, "store").as_deref(), Some("Type"));
}

// ---------------------------------------------------------------- wrappers

/// `Env` is `A.Env`, nested in `A`. `B` is not inside `A`: its `@Env` is another
/// file's wrapper. The wrapped type got a scope check; the wrapper did not.
#[test]
fn a_property_wrapper_nested_in_another_type_is_not_the_users_wrapper() {
    let source = "struct A {\n    @propertyWrapper struct Env { var wrappedValue: Client }\n}\n\
         struct B {\n    @Env var d\n}\n";
    assert_eq!(
        field(source, "d"),
        None,
        "B.d wears a wrapper this file does not show it"
    );
}

/// `struct R<Client>`: at `R`, `Client` is R's own generic parameter.
#[test]
fn a_users_generic_parameter_is_not_the_wrapped_type() {
    let source = "@propertyWrapper struct Env { var wrappedValue: Client }\n\
         struct R<Client> {\n    @Env var d\n}\n";
    assert_eq!(
        field(source, "d"),
        None,
        "inside R<Client>, `Client` is the parameter"
    );
}

// ------------------------------------------- determinism and bounded walks

fn serialized(source: &str) -> String {
    serde_json::to_string(&extract_file("a.swift", source)).expect("serializes")
}

/// 8k wrapper-attributed properties (past the 20_000-node cap of the wrapper
/// index): the same bytes twice, and a clean parse. A small file with the same
/// declarations does type them, so the abstention is the cap and not the input.
#[test]
fn a_file_of_many_wrapper_properties_extracts_the_same_twice_and_abstains_past_the_cap() {
    let wrapper = "@propertyWrapper struct Env { var wrappedValue: Client }\nstruct R {\n";
    let build = |count: usize| {
        let mut source = String::from(wrapper);
        for index in 0..count {
            source.push_str(&format!("    @Env(Client.self) var p{index}\n"));
        }
        source.push_str("}\n");
        source
    };
    let small = build(50);
    assert_eq!(field(&small, "p49").as_deref(), Some("Client"));
    let big = build(8_000);
    let first = serialized(&big);
    assert_eq!(first, serialized(&big), "extraction is not deterministic");
    assert!(
        !first.contains("budget exceeded"),
        "8k properties exhausted the parse budget"
    );
    assert_eq!(
        field(&big, "p7999"),
        None,
        "past the cap the wrapper index is unreadable"
    );
}

/// One function of 1500 `let (a, b) = makePair()` statements (50 KB). Each
/// destructure walks the whole enclosing scope for a nested `func makePair`
/// (`scope_declares_function`), so the cost is quadratic in the statements: the
/// file spends the whole 5 s parse budget and every symbol and reference in it is
/// dropped. The same file with `= (1, 2)` extracts in a fifth of a second, and
/// before the lane a tuple pattern bound no name at all.
#[test]
fn a_function_of_fifteen_hundred_destructures_does_not_exhaust_the_parse_budget() {
    let build = |value: &str| {
        let mut source = String::from(
            "struct Store {}\nstruct Svc {}\n\
             func makePair() -> (Store, Svc) { (Store(), Svc()) }\nfunc t() {\n",
        );
        for index in 0..1_500 {
            source.push_str(&format!("    let (a{index}, b{index}) = {value}\n"));
        }
        source.push_str("}\nfunc tail() { Marker().zzmarker() }\n");
        source
    };
    let has_tail = |source: &str| {
        let extraction = extract_file("a.swift", source);
        let marker = extraction
            .references
            .iter()
            .any(|reference| reference.name == "zzmarker");
        (marker, format!("{:?}", extraction.parse_outcome))
    };
    let control = has_tail(&build("(1, 2)"));
    assert!(control.0, "control: {}", control.1);
    let (kept, outcome) = has_tail(&build("makePair()"));
    assert!(
        kept,
        "1500 destructures dropped the whole file (tail reference lost): {outcome}"
    );
}
