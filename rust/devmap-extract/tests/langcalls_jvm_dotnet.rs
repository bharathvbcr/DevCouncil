//! Call extraction for Java, C#, Kotlin and Dart.
//!
//! All four reached `extract_node`'s generic arm, which emits declarations and
//! nothing else, so **not one call was extracted for any of them** — measured
//! at 0 `Calls` on a fixture whose only statement is one function calling
//! another, and confirmed at 0 across 417 real `.java` files and 8,950 real
//! `.kt` files before this change. Unlike Ruby, Swift, PHP, Scala and Lua, the
//! Python implementation this port replaces extracted 0 calls for these four as
//! well, so this is new capability rather than a migration regression.
//!
//! Every test here fails against the pre-change tree, where `extract_calls` is
//! a no-op and each assertion below is made against an empty call list.
//!
//! **Kotlin fixtures are deliberately written multi-line.** `tree-sitter-kotlin-ng`
//! 1.1 fails to parse a class whose body is written on a single line when a
//! second such class follows it — the whole file collapses into one `ERROR`
//! node and the second class is dropped. Idiomatically formatted Kotlin is
//! unaffected (8,926 of 8,950 real files parse `Clean`), but a one-line fixture
//! would silently be testing error recovery instead of the grammar.

use devmap_extract::extract_file;
use devmap_extract::model::{Extraction, ReferenceKind};

fn callees(extraction: &Extraction) -> Vec<&str> {
    extraction
        .calls
        .iter()
        .map(|call| call.callee_name.as_str())
        .collect()
}

/// `(callee, receiver)` pairs, which is what a receiver-dispatch defect shows up in.
fn targets(extraction: &Extraction) -> Vec<(&str, Option<&str>)> {
    extraction
        .calls
        .iter()
        .map(|call| (call.callee_name.as_str(), call.receiver_expr.as_deref()))
        .collect()
}

fn caller_of<'a>(extraction: &'a Extraction, callee: &str) -> Option<&'a str> {
    extraction
        .calls
        .iter()
        .find(|call| call.callee_name == callee)
        .and_then(|call| call.caller_symbol.as_deref())
}

/// Call edges whose `caller_symbol` names no symbol this file emitted.
///
/// This is the SC9/SC10 invariant, and it is the load-bearing check in this
/// file: a qualified name is the graph's join key, so an edge naming a source
/// that does not exist is invisible to every traversal *out of* that symbol
/// while still looking like a perfectly well-formed row. A `None` caller is not
/// an orphan — the resolver substitutes the file path, and the File node always
/// exists — so it is excluded here and covered separately.
fn orphaned_callers(extraction: &Extraction) -> Vec<&str> {
    let emitted: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    extraction
        .calls
        .iter()
        .filter_map(|call| call.caller_symbol.as_deref())
        .filter(|caller| !emitted.contains(caller))
        .collect()
}

const JAVA: &str = r#"
package com.example;

class App {
    int helper(int a) { return a; }

    static int stat() { return 1; }

    void run() {
        helper(1);
        this.helper(2);
        App.stat();
        App other = new App();
        other.helper(3);
        java.util.Collections.<String>emptyList();
        source.first().second();
        new Thread(() -> helper(9)).start();
        int[] block = new int[3];
        java.util.List<String> names = new java.util.ArrayList<>();
        Outer.Inner nested = new Outer.Inner();
        super.toString();
    }
}

interface Greeter {
    default int greet() { return fallback(); }

    int fallback();
}

enum Mode {
    ON;

    int describe() { return width(); }

    int width() { return 1; }
}
"#;

const CSHARP: &str = r#"
namespace Example {
    class App {
        public App() { Helper(0); }

        int Helper(int a) => a;

        static int Stat() => 1;

        int Run() {
            Helper(1);
            this.Helper(2);
            App.Stat();
            var other = new App();
            other?.Helper(3);
            text.Trim();
            var label = nameof(Helper);
            source.First().Second();
            System.Console.WriteLine("x");
            Generic<int>();
            var list = new System.Collections.Generic.List<int>();
            base.ToString();
            return 0;
        }

        static int Generic<T>() => 1;
    }

    struct Point {
        int Length() { return Width(); }

        int Width() { return 1; }
    }

    interface IShape {
        int Area() { return Sides(); }

        int Sides() { return 1; }
    }
}
"#;

const KOTLIN: &str = r#"
package example

fun helper(a: Int): Int {
    return a
}

fun String.shout(): String {
    return this
}

class Widget {
    fun run(): Int {
        var total = helper(1)
        total += this.helper(2)
        val other = Widget()
        total += other.run()
        total += maybe?.run()
        items.map { it.width() }
        scope.launch { started() }
        async { spawned() }
        withContext(Dispatchers.IO) { switched() }
        value.let { scoped() }
        Person(name = "y")
        total += 1 to 2
        total += first shl second
        deep?.nested?.reach()
        maybe?.attempt() ?: fallback()
        "text".shout()
        return total
    }

    fun helper(a: Int): Int {
        return a
    }
}

object Registry {
    fun register(): Int {
        return helper(1)
    }
}

class Screen : Base() {
    override fun onCreate(state: Bundle?) {
        super.onCreate(state)
        findViewById(1)
    }
}

suspend fun load(): Int {
    return fetch()
}

fun Person.describe(): Int {
    return helper(3)
}

interface Shape {
    fun area(): Int {
        return helper(4)
    }
}
"#;

const DART: &str = r#"
int helper(int a) => a;

class Widget {
    Widget();

    Widget.named();

    int run() {
        helper(1);
        this.helper(2);
        var made = Widget();
        var namedMade = Widget.named();
        var allocated = new Widget();
        var frozen = const Widget();
        made.run();
        buffer..add(1)..add(2);
        maybe?.attempt();
        super.run();
        return 0;
    }
}
"#;

// ---------------------------------------------------------------- Java

#[test]
fn java_records_the_call_shapes_its_grammar_produces() {
    let extraction = extract_file("App.java", JAVA);
    let found = targets(&extraction);

    // A bare call, a `this.` call, a static call through the type, and a call
    // through a local — the four ways a Java method is reached.
    assert!(found.contains(&("helper", None)), "bare call: {found:?}");
    assert!(
        found.contains(&("helper", Some("this"))),
        "`this.` call keeps its receiver so an unresolved one is an uninferred receiver, not a bare-name failure: {found:?}"
    );
    assert!(found.contains(&("stat", Some("App"))), "static: {found:?}");
    assert!(
        found.contains(&("helper", Some("other"))),
        "instance call through a local: {found:?}"
    );

    // `new T()`, including through a package path and a nested type.
    assert!(found.contains(&("App", None)), "constructor: {found:?}");
    assert!(
        found.contains(&("ArrayList", Some("util"))),
        "`new java.util.ArrayList<>()` constructs `ArrayList`; the type arguments are not part of its identity: {found:?}"
    );
    assert!(
        found.contains(&("Inner", Some("Outer"))),
        "`new Outer.Inner()` constructs `Inner`: {found:?}"
    );

    // An explicit type argument must not leak into the callee, and a dotted
    // qualifier contributes only its last segment.
    assert!(
        found.contains(&("emptyList", Some("Collections"))),
        "`java.util.Collections.<String>emptyList()`: {found:?}"
    );

    // Both links of a chain are recorded, each from its own node.
    assert!(found.contains(&("first", Some("source"))), "{found:?}");
    assert!(
        found.contains(&("second", Some("source.first()"))),
        "{found:?}"
    );

    // A call inside a lambda belongs to the method that declares the lambda,
    // which is the only symbol that exists to own it.
    assert_eq!(
        caller_of(&extraction, "helper"),
        Some("App.java::App.run"),
        "calls attribute to the enclosing method"
    );
    assert!(
        extraction
            .calls
            .iter()
            .any(|call| call.callee_name == "helper"
                && call.caller_symbol.as_deref() == Some("App.java::App.run")),
        "the lambda body's `helper(9)` is attributed to `run`"
    );
}

#[test]
fn java_refuses_the_shapes_that_would_manufacture_a_phantom_or_a_self_edge() {
    let extraction = extract_file("App.java", JAVA);
    let found = callees(&extraction);
    // Anchored on the calls that *are* recorded, so this test cannot pass by
    // extracting nothing at all.
    assert!(
        found.contains(&"helper") && found.contains(&"Thread"),
        "{found:?}"
    );

    // `super.toString()` names the *superclass's* method, which the call site
    // never identifies. Recording callee `toString` would let same-file
    // resolution bind it to an override in this very class at DETERMINISTIC
    // confidence — a wrong self-edge that also makes every override look called.
    assert!(
        !found.contains(&"toString"),
        "`super.` must not become a call to this class's own method: {found:?}"
    );
    // `new int[3]` allocates; it invokes no constructor.
    assert!(
        !found.contains(&"int"),
        "an array allocation names no callee: {found:?}"
    );
    // `this(…)` / `super(…)` name a constructor overload the call site does not
    // identify; every constructor of a class shares one name.
    assert!(
        !found.contains(&"super") && !found.contains(&"this"),
        "no callee is a keyword: {found:?}"
    );
}

#[test]
fn a_java_method_in_an_interface_or_an_enum_is_named_the_way_the_symbol_is() {
    // The reason this module does not simply call `enclosing_callable_qualified`.
    // The declaration side owns a method by `interface_declaration` and
    // `enum_declaration` — both are non-`Function` kinds in
    // `generic_symbol_kind` — while `enclosing_type_name` matches only
    // `class_*`, `trait_item` and `impl_item`. Attributing through the latter
    // yields `App.java::fallback` where the symbol is `App.java::Greeter.greet`'s
    // sibling `App.java::Greeter.fallback`: an edge that joins to nothing.
    let extraction = extract_file("App.java", JAVA);
    assert_eq!(
        caller_of(&extraction, "fallback"),
        Some("App.java::Greeter.greet"),
        "a call inside an interface's default method belongs to that method"
    );
    assert_eq!(
        caller_of(&extraction, "width"),
        Some("App.java::Mode.describe"),
        "a call inside an enum's method belongs to that method"
    );
}

// ---------------------------------------------------------------- C#

#[test]
fn csharp_records_the_call_shapes_its_grammar_produces() {
    let extraction = extract_file("App.cs", CSHARP);
    let found = targets(&extraction);

    assert!(found.contains(&("Helper", None)), "bare call: {found:?}");
    assert!(found.contains(&("Helper", Some("this"))), "{found:?}");
    assert!(found.contains(&("Stat", Some("App"))), "static: {found:?}");
    assert!(found.contains(&("App", None)), "constructor: {found:?}");
    assert!(
        found.contains(&("Helper", Some("other"))),
        "`other?.Helper(3)`: `?.` and `.` are the same call and must split alike: {found:?}"
    );
    // An extension method is written and parsed exactly like an instance call;
    // nothing at the call site distinguishes them, so it arrives receiver-bearing.
    assert!(
        found.contains(&("Trim", Some("text"))),
        "extension: {found:?}"
    );
    assert!(found.contains(&("First", Some("source"))), "{found:?}");
    assert!(
        found.contains(&("Second", Some("source.First()"))),
        "{found:?}"
    );
    assert!(
        found.contains(&("WriteLine", Some("Console"))),
        "a dotted qualifier contributes its last segment: {found:?}"
    );
    assert!(
        found.contains(&("Generic", None)),
        "`Generic<int>()` keeps no type argument in its callee: {found:?}"
    );
    assert!(
        found.contains(&("List", Some("Generic"))),
        "`new System.Collections.Generic.List<int>()` constructs `List`: {found:?}"
    );
    // A call in a constructor body belongs to the constructor, which *is* an
    // emitted symbol in C#.
    assert_eq!(
        caller_of(&extraction, "Helper"),
        Some("App.cs::App.App"),
        "the first `Helper` call is the one in the constructor"
    );
}

#[test]
fn csharp_refuses_nameof_and_base() {
    let extraction = extract_file("App.cs", CSHARP);
    let found = callees(&extraction);
    assert!(
        found.contains(&"Helper") && found.contains(&"WriteLine"),
        "{found:?}"
    );
    // `nameof` is a contextual operator the grammar parses as an invocation. No
    // corpus declares a method by that name, so an edge would be a phantom
    // callee of exactly the kind SC17 removed.
    assert!(
        !found.contains(&"nameof"),
        "`nameof` is an operator, not a method: {found:?}"
    );
    assert!(
        !found.contains(&"ToString"),
        "`base.` must not become a call to this class's own method: {found:?}"
    );
}

#[test]
fn a_csharp_method_in_a_struct_or_an_interface_is_named_the_way_the_symbol_is() {
    let extraction = extract_file("App.cs", CSHARP);
    assert_eq!(
        caller_of(&extraction, "Width"),
        Some("App.cs::Point.Length"),
        "a call inside a struct's method belongs to that method"
    );
    assert_eq!(
        caller_of(&extraction, "Sides"),
        Some("App.cs::IShape.Area"),
        "a call inside an interface's method belongs to that method"
    );
}

#[test]
fn csharp_declarations_are_present_and_this_change_does_not_move_them() {
    // Recorded because the brief asked whether C# declarations are missing too.
    // They are not: a namespace, class, struct, interface, constructor and
    // method all reach the symbol table. Pinned so a later claim that "C# emits
    // nothing" has to contend with a test.
    let extraction = extract_file("App.cs", CSHARP);
    let emitted: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    for expected in [
        "App.cs::Example",
        "App.cs::Example.App",
        "App.cs::App.App",
        "App.cs::App.Helper",
        "App.cs::Example.Point",
        "App.cs::Point.Length",
        "App.cs::Example.IShape",
        "App.cs::IShape.Area",
    ] {
        assert!(
            emitted.contains(&expected),
            "missing {expected}: {emitted:?}"
        );
    }
}

// ---------------------------------------------------------------- Kotlin

#[test]
fn kotlin_records_the_call_shapes_its_grammar_produces() {
    let extraction = extract_file("Main.kt", KOTLIN);
    assert!(
        matches!(
            extraction.parse_outcome,
            devmap_extract::model::ParseOutcome::Clean
        ),
        "the fixture must parse cleanly or it is testing error recovery: {:?}",
        extraction.parse_outcome
    );
    let found = targets(&extraction);

    assert!(found.contains(&("helper", None)), "bare call: {found:?}");
    assert!(found.contains(&("helper", Some("this"))), "{found:?}");
    assert!(
        found.contains(&("run", Some("other"))),
        "instance call through a local: {found:?}"
    );
    assert!(
        found.contains(&("run", Some("maybe"))),
        "a safe call `maybe?.run()` splits like a plain one: {found:?}"
    );
    assert!(
        found.contains(&("reach", Some("deep?.nested"))),
        "a safe-call chain keeps the whole chain as the receiver: {found:?}"
    );
    assert!(
        found.contains(&("attempt", Some("maybe"))) && found.contains(&("fallback", None)),
        "both sides of an elvis expression are calls: {found:?}"
    );

    // Trailing lambdas: the call itself, and the calls inside the lambda body.
    assert!(found.contains(&("map", Some("items"))), "{found:?}");
    assert!(
        found.contains(&("width", Some("it"))),
        "inside the lambda: {found:?}"
    );
    assert!(
        found.contains(&("launch", Some("scope"))),
        "coroutine builder: {found:?}"
    );
    assert!(
        found.contains(&("started", None)),
        "inside `launch`: {found:?}"
    );
    assert!(found.contains(&("async", None)), "{found:?}");
    assert!(
        found.contains(&("spawned", None)),
        "inside `async`: {found:?}"
    );
    assert!(
        found.contains(&("let", Some("value"))) && found.contains(&("scoped", None)),
        "a scope function is an extension call plus its lambda body: {found:?}"
    );

    // `withContext(x) { … }` parses as an outer call whose *callee* is the
    // inner `withContext(x)` call. Reading the outer node's callee text would
    // record a whole call expression as a callee name — the SC26/SC32 defect.
    assert!(
        found.contains(&("withContext", None)) && found.contains(&("switched", None)),
        "a trailing lambda on a parenthesised call records the inner call once: {found:?}"
    );
    assert_eq!(
        found
            .iter()
            .filter(|(callee, _)| *callee == "withContext")
            .count(),
        1,
        "and records it exactly once: {found:?}"
    );

    // A constructor call has no `new` in Kotlin and is syntactically a call.
    assert!(found.contains(&("Widget", None)), "constructor: {found:?}");
    assert!(
        found.contains(&("Person", None)),
        "named arguments do not change the shape: {found:?}"
    );

    // Infix calls. An `infix fun` is a declaration like any other, and a
    // repository defining its own infix DSL would otherwise see every one of
    // those functions as uncalled.
    assert!(found.contains(&("to", Some("1"))), "infix `to`: {found:?}");
    assert!(
        found.contains(&("shl", Some("first"))),
        "infix `shl`: {found:?}"
    );

    // An extension *call* is receiver-bearing and resolves only if a symbol
    // matches; nothing is invented for it.
    assert!(
        found.contains(&("shout", Some("\"text\""))),
        "extension call: {found:?}"
    );

    assert!(
        found.contains(&("fetch", None)),
        "a `suspend` call: {found:?}"
    );
    assert!(
        found.contains(&("findViewById", None)),
        "an Android lifecycle override's body: {found:?}"
    );
}

#[test]
fn kotlin_refuses_super_so_an_override_does_not_call_itself() {
    let extraction = extract_file("Main.kt", KOTLIN);
    let found = callees(&extraction);
    // `findViewById` sits in the same override body, so this test fails both if
    // the refusal stops working and if the body stops being read at all.
    assert!(found.contains(&"findViewById"), "{found:?}");
    // `override fun onCreate` calling `super.onCreate` is the most common shape
    // in Android Kotlin. Recording callee `onCreate` would let same-file
    // resolution bind it to the override itself at DETERMINISTIC confidence —
    // wrong, and it would make every lifecycle override appear called and so
    // exempt from dead-code analysis.
    assert!(
        !found.contains(&"onCreate"),
        "`super.onCreate` must not become a self-edge: {found:?}"
    );
}

#[test]
fn kotlin_calls_are_attributed_to_the_declaration_the_emitter_actually_named() {
    let extraction = extract_file("Main.kt", KOTLIN);
    assert_eq!(
        caller_of(&extraction, "started"),
        Some("Main.kt::Widget.run"),
        "a call inside a lambda belongs to the enclosing named function"
    );
    assert_eq!(
        caller_of(&extraction, "register"),
        None,
        "there is no call named `register`; the object's own body calls `helper`"
    );
    assert_eq!(
        caller_of(&extraction, "fetch"),
        Some("Main.kt::load"),
        "a top-level `suspend fun` owns its calls"
    );
    assert_eq!(
        caller_of(&extraction, "findViewById"),
        Some("Main.kt::Screen.onCreate"),
        "an override owns its calls"
    );
    assert_eq!(
        caller_of(&extraction, "area"),
        None,
        "there is no call named `area`"
    );
    // An extension function declaration used to lose its receiver:
    // `fun Person.describe()` was emitted as `Main.kt::describe`. It now takes
    // the receiver as its owner, the same identity a Go receiver keeps under
    // SC9 and a Rust `impl` method under SC11, and this test moved with it —
    // the property it pins is that the two sides agree, whatever the answer is,
    // so it must fail if only one of them changes.
    let emitted: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert!(
        emitted.contains(&"Main.kt::Person.describe"),
        "the declaration side owns it by its receiver: {emitted:?}"
    );
    assert!(
        !emitted.contains(&"Main.kt::describe"),
        "and no bare copy survives beside it: {emitted:?}"
    );
    assert!(
        extraction
            .calls
            .iter()
            .any(|call| call.caller_symbol.as_deref() == Some("Main.kt::Person.describe")),
        "so the call side must name it the same way, or the edge is an orphan"
    );
}

// ---------------------------------------------------------------- Dart

#[test]
fn dart_records_the_call_shapes_its_grammar_produces() {
    let extraction = extract_file("main.dart", DART);
    let found = targets(&extraction);

    assert!(found.contains(&("helper", None)), "bare call: {found:?}");
    assert!(found.contains(&("helper", Some("this"))), "{found:?}");
    assert!(found.contains(&("run", Some("made"))), "{found:?}");
    assert!(
        found.contains(&("Widget", None)),
        "`Widget()`, `new Widget()` and `const Widget()` all construct `Widget`: {found:?}"
    );
    assert_eq!(
        found
            .iter()
            .filter(|(callee, _)| *callee == "Widget")
            .count(),
        3,
        "and all three are recorded: {found:?}"
    );
    assert!(
        found.contains(&("named", Some("Widget"))),
        "a named constructor is a member of its class: {found:?}"
    );
    assert!(
        found.contains(&("attempt", Some("maybe"))),
        "`maybe?.attempt()`: {found:?}"
    );
    // A cascade applies each section to the same target.
    assert_eq!(
        found
            .iter()
            .filter(|target| **target == ("add", Some("buffer")))
            .count(),
        2,
        "both cascade sections are calls on `buffer`: {found:?}"
    );
    assert!(
        !callees(&extraction).contains(&"run") || found.contains(&("run", Some("made"))),
        "`super.run()` must not become a call to this class's own method"
    );
    assert_eq!(
        found
            .iter()
            .filter(|(callee, receiver)| *callee == "run" && receiver.is_none())
            .count(),
        0,
        "in particular `super.run()` records nothing: {found:?}"
    );
}

#[test]
fn dart_calls_are_attributed_to_the_callable_the_emitter_named() {
    // This test previously pinned the opposite: `tree-sitter-dart` puts a
    // declaration's name one level down on a `signature` child, so
    // `generic_declaration_name` — which reads `name` on the declaration itself
    // — found nothing and **no Dart function or method became a symbol at
    // all**. Every Dart call was then attributed to its enclosing class, or to
    // the file for a top-level function: coarse, joinable, and inert, because no
    // Dart call could resolve to a Dart target that did not exist.
    //
    // `langdecl::dart` reads the signature, so the callables exist and the
    // attribution follows them. The assertions are inverted rather than
    // deleted, because the gap they described is exactly what closed.
    let extraction = extract_file("main.dart", DART);
    let emitted: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    assert!(
        emitted.contains(&"main.dart::Widget"),
        "the class is emitted: {emitted:?}"
    );
    assert!(
        emitted.contains(&"main.dart::helper"),
        "and so is a top-level function: {emitted:?}"
    );
    assert!(
        emitted.contains(&"main.dart::Widget.run"),
        "and so is a method, owned by its class: {emitted:?}"
    );
    assert_eq!(
        caller_of(&extraction, "helper"),
        Some("main.dart::Widget.run"),
        "a call in a method body is owned by the method, not by the class"
    );
    assert!(
        !extraction.calls.is_empty(),
        "the calls themselves are extracted and now resolve to Dart targets"
    );
}

// ------------------------------------------------- cross-language invariants

#[test]
fn no_call_names_a_source_symbol_that_does_not_exist() {
    // SC9/SC10. A qualified name is the graph's join key: an edge whose source
    // matches no node is invisible to every traversal out of that symbol while
    // still looking well-formed. Measured at 0 across 417 real `.java` files
    // (3,622 calls) and 8,950 real `.kt` files (135,829 calls).
    for (path, source) in [
        ("App.java", JAVA),
        ("App.cs", CSHARP),
        ("Main.kt", KOTLIN),
        ("main.dart", DART),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.calls.is_empty(),
            "{path} extracted no calls, so this invariant would hold vacuously"
        );
        assert_eq!(
            orphaned_callers(&extraction),
            Vec::<&str>::new(),
            "{path} produced call edges whose source symbol is not an emitted symbol"
        );
    }
}

#[test]
fn every_callee_is_an_identifier_and_never_an_expression() {
    // SC26 and SC32 are two rounds of fixing exactly this: a callee built from
    // a target's own source text records whole expressions as names, and those
    // rows can never join to a symbol. `split_call_target` is the only place a
    // callee name is built here, and it refuses anything not identifier-shaped.
    for (path, source) in [
        ("App.java", JAVA),
        ("App.cs", CSHARP),
        ("Main.kt", KOTLIN),
        ("main.dart", DART),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.calls.is_empty(),
            "{path} extracted no calls at all"
        );
        for call in &extraction.calls {
            assert!(
                !call.callee_name.is_empty()
                    && call
                        .callee_name
                        .chars()
                        .all(|character| character.is_alphanumeric()
                            || matches!(character, '_' | '$' | '#')),
                "{path}: callee {:?} is an expression, not a name",
                call.callee_name
            );
        }
    }
}

#[test]
fn every_call_carries_a_reference_that_agrees_with_it() {
    // The reference is what the resolver reads to type a receiver, so a call
    // without its reference silently loses `var w = Widget(); w.run()`
    // dispatch. Both are emitted from one place; this pins that they stay
    // paired and agree on the enclosing symbol.
    for (path, source) in [
        ("App.java", JAVA),
        ("App.cs", CSHARP),
        ("Main.kt", KOTLIN),
        ("main.dart", DART),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            !extraction.calls.is_empty(),
            "{path} extracted no calls, so this invariant would hold vacuously"
        );
        for call in &extraction.calls {
            let matched = extraction.references.iter().any(|reference| {
                matches!(
                    reference.kind,
                    ReferenceKind::Call | ReferenceKind::Constructor
                ) && reference.name == call.callee_name
                    && reference.enclosing_symbol == call.caller_symbol
            });
            assert!(
                matched,
                "{path}: call to {:?} has no matching Call/Constructor reference",
                call.callee_name
            );
        }
    }
}

#[test]
fn a_constructed_value_binds_its_local_name_so_the_receiver_can_be_typed() {
    // `var other = new App(); other.Helper(3)` only dispatches deterministically
    // if the constructor reference carries `assigned_to`. The resolver's own
    // filter — the referenced name must index to exactly one class or struct —
    // is the safety, so recording a binding for an ordinary call costs nothing.
    for (path, source, binding, constructed) in [
        ("App.java", JAVA, "other", "App"),
        ("App.cs", CSHARP, "other", "App"),
        ("Main.kt", KOTLIN, "other", "Widget"),
        ("main.dart", DART, "made", "Widget"),
    ] {
        let extraction = extract_file(path, source);
        assert!(
            extraction.references.iter().any(|reference| {
                reference.name == constructed && reference.assigned_to.as_deref() == Some(binding)
            }),
            "{path}: `{binding} = {constructed}(…)` did not bind {binding}"
        );
    }
}
