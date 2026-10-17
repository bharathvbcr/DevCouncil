//! Adversarial pass on 8d4c6b97 (Swift receiver typing: typealias, tuple-member
//! receivers, `?.` chains, SwiftData `fetch`, own-initializer, casts).
//!
//! The principle under test is "abstain rather than emit a wrong edge". Every
//! test here states a shape in which Swift's own scoping says the name means
//! something other than what the extractor typed it as, and asserts that the
//! extractor does not claim the wrong type. Each one FAILS on 8d4c6b97.

use devmap_extract::extract_file;

fn receivers(source: &str, callee: &str) -> Vec<Option<String>> {
    extract_file("a.swift", source)
        .calls
        .into_iter()
        .filter(|call| call.callee_name == callee)
        .map(|call| call.receiver_expr)
        .collect()
}

/// Every type any local binding named `name` is recorded with.
fn types_of(source: &str, name: &str) -> Vec<String> {
    extract_file("a.swift", source)
        .local_bindings
        .into_iter()
        .filter(|binding| binding.name == name)
        .filter_map(|binding| binding.declared_type)
        .collect()
}

const PARCEL: &str = "struct Parcel { func weight() -> Int { 1 } }\nstruct Pod { func go() {} }\n";

// ---- tuple member receivers: a rebinding of the tuple's name hides the member

/// `let open = Other()` rebinds `open` in the same body; `open.transform` is
/// `Other`'s member, not the parameter tuple's `Case`.
#[test]
fn a_let_that_rebinds_a_tuple_parameter_hides_its_members() {
    let source = "struct Case {}\nfunc f(_ open: (transform: Case, Int)) {\n let open = Other()\n open.transform.go()\n}";
    assert_ne!(receivers(source, "go"), vec![Some("Case".to_string())]);
}

#[test]
fn an_if_let_that_rebinds_a_tuple_parameter_hides_its_members() {
    let source = "struct Case {}\nfunc f(_ open: (transform: Case, Int), _ m: Other?) {\n if let open = m { open.transform.go() }\n}";
    assert_ne!(receivers(source, "go"), vec![Some("Case".to_string())]);
}

#[test]
fn a_guard_let_that_rebinds_a_tuple_parameter_hides_its_members() {
    let source = "struct Case {}\nfunc f(_ open: (transform: Case, Int), _ m: Other?) {\n guard let open = m else { return }\n open.transform.go()\n}";
    assert_ne!(receivers(source, "go"), vec![Some("Case".to_string())]);
}

#[test]
fn a_for_in_that_rebinds_a_tuple_parameter_hides_its_members() {
    let source = "struct Case {}\nfunc f(_ open: (transform: Case, Int), _ m: [Other]) {\n for open in m { open.transform.go() }\n}";
    assert_ne!(receivers(source, "go"), vec![Some("Case".to_string())]);
}

#[test]
fn a_case_let_that_rebinds_a_tuple_parameter_hides_its_members() {
    let source = "struct Case {}\nfunc f(_ open: (transform: Case, Int), _ m: Other?) {\n switch m { case let open?: open.transform.go()\n default: break }\n}";
    assert_ne!(receivers(source, "go"), vec![Some("Case".to_string())]);
}

// ---- typealias: a name the alias index cannot see is shadowed

/// `func run<Entry>` makes `Entry` a generic parameter; the file-level
/// `typealias Entry = Parcel` is not what it names.
#[test]
fn a_generic_parameter_named_like_an_alias_is_not_the_alias() {
    let source =
        format!("{PARCEL}typealias Entry = Parcel\nfunc run<Entry>(_ e: Entry) {{ e.weight() }}");
    assert!(
        !types_of(&source, "e").contains(&"Parcel".to_string()),
        "{:?}",
        types_of(&source, "e")
    );
}

#[test]
fn a_generic_type_parameter_named_like_an_alias_is_not_the_alias() {
    let source = format!(
        "{PARCEL}typealias Entry = Parcel\nstruct Box<Entry> {{ func run(_ e: Entry) {{ e.weight() }} }}"
    );
    assert!(
        !types_of(&source, "e").contains(&"Parcel".to_string()),
        "{:?}",
        types_of(&source, "e")
    );
}

/// A `typealias` inside a function body is invisible to the alias index (it
/// skips function bodies), so the file-level alias of the same name wins.
#[test]
fn a_local_typealias_shadows_the_file_level_one() {
    let source = format!(
        "{PARCEL}typealias Entry = Parcel\nfunc f() {{\n typealias Entry = Pod\n let e: Entry = make()\n e.go()\n}}"
    );
    assert!(
        !types_of(&source, "e").contains(&"Parcel".to_string()),
        "{:?}",
        types_of(&source, "e")
    );
}

/// Inside a protocol (and its extensions) `Entry` is the associated type.
#[test]
fn an_associated_type_named_like_an_alias_is_not_the_alias() {
    let source = format!(
        "{PARCEL}typealias Entry = Parcel\nprotocol Sink {{ associatedtype Entry }}\nextension Sink {{ func run(_ e: Entry) {{ e.weight() }} }}"
    );
    assert!(
        !types_of(&source, "e").contains(&"Parcel".to_string()),
        "{:?}",
        types_of(&source, "e")
    );
}

/// `typealias Maybe = Parcel?` is `Optional<Parcel>`; `Maybe.some` is
/// `Optional.some`, not a static member of `Parcel`.
#[test]
fn an_alias_of_an_optional_is_not_a_receiver_of_its_wrapped_type() {
    let source =
        "struct Parcel { static func some() {} }\ntypealias Maybe = Parcel?\nfunc f() { _ = Maybe.some() }";
    assert_ne!(receivers(source, "some"), vec![Some("Parcel".to_string())]);
}

// ---- `?.` chain: Optional.map hands its closure the collection, not an element

/// `maybe` is `[Parcel]?`; in `h?.maybe.map { $0 }` the `.map` is
/// `Optional.map` on that optional member, so `$0` is `[Parcel]`.
#[test]
fn an_optional_collection_member_after_a_question_dot_keeps_its_optional_map() {
    let source = format!(
        "{PARCEL}struct Holder {{ var maybe: [Parcel]? }}\nfunc f(_ h: Holder?) {{ _ = h?.maybe.map {{ $0.weight() }} }}"
    );
    assert!(
        !types_of(&source, "$0").contains(&"Parcel".to_string()),
        "{:?}",
        types_of(&source, "$0")
    );
}

// ---- SwiftData fetch: the receiver and the descriptor's argument are not checked

/// A user type's own `fetch(_: FetchDescriptor<Habit>) -> [HabitRow]` in a file
/// that imports SwiftData is not `ModelContext.fetch`.
#[test]
fn a_user_defined_fetch_is_not_swiftdatas() {
    let source = "import SwiftData\nstruct Habit {}\nstruct HabitRow { func name() {} }\n\
                  struct Repo { func fetch(_ d: FetchDescriptor<Habit>) -> [HabitRow] { [] } }\n\
                  func f(_ repo: Repo) { _ = repo.fetch(FetchDescriptor<Habit>()).map { $0.name() } }";
    assert!(
        !types_of(source, "$0").contains(&"Habit".to_string()),
        "{:?}",
        types_of(source, "$0")
    );
}

/// `FetchDescriptor<T>` with `T` a generic parameter has no nominal element.
#[test]
fn a_fetch_of_a_generic_parameter_is_not_a_collection_of_a_type_named_like_it() {
    let source = "import SwiftData\nfunc f<T: PersistentModel>(_ c: ModelContext, _ t: T.Type) {\n\
                  _ = ((try? c.fetch(FetchDescriptor<T>())) ?? []).map { $0.go() }\n}";
    assert!(
        !types_of(source, "$0").contains(&"T".to_string()),
        "{:?}",
        types_of(source, "$0")
    );
}

// ---- cast receiver

/// `(x as? T)?.go()` where `T` is the function's generic parameter: `T` is no
/// type a name lookup can use (the extractor's own return-type rule says so).
#[test]
fn a_cast_to_a_generic_parameter_is_not_a_receiver_named_like_it() {
    let source = "func f<T>(_ x: Any) { (x as? T)?.go() }";
    assert_ne!(receivers(source, "go"), vec![Some("T".to_string())]);
}

// ---- own initializer: the fix covers `if`/`guard`/`while`/`let`, not `for`

/// In `for x in x.sorted()` the sequence's `x` is the outer `[Pod]`, not the
/// loop variable (a `Pod`); `sorted` is not a method of `Pod`.
#[test]
fn a_for_in_sequence_does_not_see_its_own_loop_variable() {
    let source = format!("{PARCEL}func f(_ x: [Pod]) {{ for x in x.sorted() {{ x.go() }} }}");
    let use_site = source.find("x.sorted").expect("the sequence");
    let wrong = extract_file("a.swift", &source)
        .local_bindings
        .into_iter()
        .any(|b| {
            b.name == "x" && b.start_byte == use_site && b.declared_type.as_deref() == Some("Pod")
        });
    assert!(!wrong, "the sequence's `x` was typed as the loop variable");
}
