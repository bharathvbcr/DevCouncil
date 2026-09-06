//! Swift and Scala call extraction (SC34).
//!
//! Every test here fails against the pre-change tree: `langcalls` had no module
//! for either language, so `extract_calls` returned without doing anything and
//! both languages produced declarations and zero calls.
//!
//! The extractors are called directly rather than through `extract_treesitter`,
//! so this file holds whether or not the dispatcher arms in `langcalls/mod.rs`
//! have been applied. The one test that needs the emitter — the orphaned-edge
//! invariant — reads symbols from `extract_treesitter`, which emits Swift and
//! Scala declarations today, and calls from the extractor directly, so it
//! compares the two sides that must agree without depending on the wiring
//! between them.

use devmap_extract::extract_treesitter;
use devmap_extract::langcalls::{scala, swift};
use devmap_extract::model::{ExtractedCall, ExtractedReference, ReferenceKind};
use tree_sitter::Parser;

/// Everything one file's call extractor produces, in traversal order.
struct Extracted {
    calls: Vec<ExtractedCall>,
    references: Vec<ExtractedReference>,
}

impl Extracted {
    /// Callee names in traversal order.
    fn callees(&self) -> Vec<&str> {
        self.calls
            .iter()
            .map(|call| call.callee_name.as_str())
            .collect()
    }

    /// The one call with this callee name, or a panic naming what was found.
    fn call(&self, callee: &str) -> &ExtractedCall {
        let hits: Vec<&ExtractedCall> = self
            .calls
            .iter()
            .filter(|call| call.callee_name == callee)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "expected exactly one call to `{callee}`, found {} among {:?}",
            hits.len(),
            self.callees()
        );
        hits[0]
    }

    fn receiver(&self, callee: &str) -> Option<&str> {
        self.call(callee).receiver_expr.as_deref()
    }

    fn caller(&self, callee: &str) -> Option<&str> {
        self.call(callee).caller_symbol.as_deref()
    }

    fn reference_kind(&self, callee: &str) -> ReferenceKind {
        let hits: Vec<&ExtractedReference> = self
            .references
            .iter()
            .filter(|reference| reference.name == callee)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "expected exactly one reference to `{callee}`"
        );
        hits[0].kind
    }

    fn binding(&self, callee: &str) -> Option<&str> {
        self.references
            .iter()
            .find(|reference| reference.name == callee)
            .and_then(|reference| reference.assigned_to.as_deref())
    }
}

fn extract(path: &str, lang: &str, source: &str) -> Extracted {
    let language: tree_sitter::Language = match lang {
        "swift" => tree_sitter_swift::LANGUAGE.into(),
        "scala" => tree_sitter_scala::LANGUAGE.into(),
        other => panic!("unsupported language {other}"),
    };
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("the grammar is already linked into this crate");
    let tree = parser.parse(source, None).expect("parse");
    let mut calls = Vec::new();
    let mut references = Vec::new();
    // The same pre-order walk `extract_treesitter` performs, so call order here
    // is the order the dispatcher produces.
    let mut worklist = vec![tree.root_node()];
    while let Some(node) = worklist.pop() {
        match lang {
            "swift" => swift::extract_swift_call(node, source, path, &mut calls, &mut references),
            _ => scala::extract_scala_call(node, source, path, &mut calls, &mut references),
        }
        for index in (0..node.child_count()).rev() {
            if let Some(child) = node.child(index) {
                worklist.push(child);
            }
        }
    }
    Extracted { calls, references }
}

/// Qualified names of every symbol the emitter produces for a file.
fn emitted_symbols(path: &str, lang: &str, source: &str) -> Vec<String> {
    extract_treesitter(path, lang, source)
        .symbols
        .into_iter()
        .map(|symbol| symbol.qualified_name)
        .collect()
}

const SWIFT_SERVICE: &str = r#"
protocol Greeter {
    func greet() -> String
}

extension Greeter {
    func greetTwice() -> String {
        return greet()
    }
}

struct Config {
    static func load() -> Config {
        return Config()
    }
}

class Service {
    let config: Config
    init(config: Config) {
        self.config = config
        register()
    }
    class func shared() -> Service {
        return Service(config: Config.load())
    }
    func run(items: [Int]) {
        items.forEach { item in
            handle(item)
        }
        self.helper()
        func nested() {
            deepest()
        }
        nested()
    }
    func helper() {}
    func register() {}
}
"#;

#[test]
fn swift_extracts_free_method_and_static_calls() {
    let extracted = extract("s.swift", "swift", SWIFT_SERVICE);
    for callee in ["greet", "load", "forEach", "handle", "helper", "nested"] {
        assert!(
            extracted.callees().contains(&callee),
            "`{callee}` is a call in this file; found {:?}",
            extracted.callees()
        );
    }
    // A static method call splits into receiver and name rather than recording
    // `Config.load` as a callee name (SC26/SC32).
    assert_eq!(extracted.receiver("load"), Some("Config"));
    assert_eq!(extracted.receiver("forEach"), Some("items"));
    assert_eq!(extracted.receiver("helper"), Some("self"));
    // An implicit-`self` call carries no receiver, which is what lets the
    // resolution ladder find it in the same file.
    assert_eq!(extracted.receiver("handle"), None);
}

#[test]
fn swift_attributes_calls_to_the_symbol_the_emitter_names() {
    let extracted = extract("s.swift", "swift", SWIFT_SERVICE);
    // Protocol-extension member: owned by the extended type.
    assert_eq!(
        extracted.caller("greet"),
        Some("s.swift::Greeter.greetTwice")
    );
    // Method of a class.
    assert_eq!(extracted.caller("forEach"), Some("s.swift::Service.run"));
    // Inside a trailing closure, the enclosing method still owns the call.
    assert_eq!(extracted.caller("handle"), Some("s.swift::Service.run"));
    // An `init` body is not a declaration the generic emitter names, so its
    // calls belong to the type — which is a symbol, unlike `s.swift::init`.
    assert_eq!(extracted.caller("register"), Some("s.swift::Service"));
    // A nested `func` is emitted unqualified by the generic emitter, so its
    // calls must name it unqualified too.
    assert_eq!(extracted.caller("deepest"), Some("s.swift::nested"));
}

#[test]
fn swift_optional_chaining_and_force_unwrap_keep_the_bare_receiver() {
    let source = r#"
func drive(loader: Loader?) {
    loader?.warm()
    loader!.cool()
}
"#;
    let extracted = extract("o.swift", "swift", source);
    assert_eq!(extracted.receiver("warm"), Some("loader"));
    assert_eq!(extracted.receiver("cool"), Some("loader"));
}

#[test]
fn swift_try_and_await_are_not_part_of_the_callee_name() {
    // SC26's shape: a wrapper standing between the call and its callee turned
    // `await invoke<Raw>('x')` into a callee named `await invoke`.
    let source = r#"
func drive() async {
    let a = try await fetch()
    let b = try? parse(a)
    let c = try! render(b)
    await refresh()
}
"#;
    let extracted = extract("t.swift", "swift", source);
    let mut callees = extracted.callees();
    callees.sort_unstable();
    assert_eq!(callees, vec!["fetch", "parse", "refresh", "render"]);
}

#[test]
fn swift_records_initializers_as_constructions_of_their_type() {
    let source = r#"
struct Person {
    init?(name: String) {}
    init() {
        self.init(name: "x")
    }
}

func build() {
    let a = Person(name: "n")
    let b = Person.init(name: "m")
    let c = Set<Int>()
    let d = Box<Int>(value: 1)
}
"#;
    let extracted = extract("i.swift", "swift", source);
    let mut callees = extracted.callees();
    callees.sort_unstable();
    // `Person.init(...)` is a construction of `Person`, not a call to a symbol
    // named `init` — the generic emitter emits nothing for an `init_declaration`,
    // so an `init` callee could never resolve. `self.init(...)` names the type
    // the call is already inside and is deliberately dropped.
    assert_eq!(callees, vec!["Box", "Person", "Person", "Set"]);
    assert_eq!(
        extracted.reference_kind("Set"),
        ReferenceKind::Constructor,
        "`Set<Int>()` is a constructor_expression"
    );
    assert_eq!(extracted.receiver("Box"), None);
}

#[test]
fn swift_extracts_trailing_and_multiple_trailing_closures() {
    let source = r#"
func drive() {
    run(with: 1) { value in
        handle(value)
    } onError: { err in
        report(err)
    }
    UIView.animate(withDuration: 0.3, animations: {
        warm()
    })
    Task { await refresh() }
}
"#;
    let extracted = extract("c.swift", "swift", source);
    let mut callees = extracted.callees();
    callees.sort_unstable();
    assert_eq!(
        callees,
        vec!["Task", "animate", "handle", "refresh", "report", "run", "warm"]
    );
    assert_eq!(extracted.receiver("animate"), Some("UIView"));
    assert_eq!(extracted.caller("handle"), Some("c.swift::drive"));
}

#[test]
fn swift_extracts_swiftui_modifier_chains_and_body_calls() {
    let source = r#"
struct ContentView: View {
    var body: some View {
        VStack {
            Text("hello")
                .padding()
                .foregroundColor(.red)
        }
        .onAppear {
            load()
        }
    }
    func load() {}
}
"#;
    let extracted = extract("v.swift", "swift", source);
    let mut callees = extracted.callees();
    callees.sort_unstable();
    assert_eq!(
        callees,
        vec![
            "Text",
            "VStack",
            "foregroundColor",
            "load",
            "onAppear",
            "padding"
        ]
    );
    // A computed property is not a declaration the generic emitter names, so a
    // `body` call belongs to the view type rather than to the file.
    assert_eq!(extracted.caller("VStack"), Some("v.swift::ContentView"));
    // The receiver of a chained modifier is an expression, not a name. It is
    // still carried: dropping it would make `.padding()` look like a bare call
    // and let it resolve to a same-file `padding` at deterministic confidence.
    let chained = extracted.receiver("foregroundColor").expect("receiver");
    assert!(
        chained.starts_with("Text(\"hello\")"),
        "receiver was {chained:?}"
    );
    assert!(
        chained.len() <= 100 && !chained.contains('\n'),
        "a chained receiver must stay bounded and single-line, was {} bytes",
        chained.len()
    );
}

#[test]
fn swift_never_records_an_expression_as_a_callee() {
    let source = r#"
func f() {
    let a = [Int]()
    let b = { () -> Int in 1 }()
    obj.chain()[0].run()
}
"#;
    let extracted = extract("e.swift", "swift", source);
    for callee in extracted.callees() {
        assert!(
            callee
                .chars()
                .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#')),
            "callee `{callee}` is an expression, not a name"
        );
    }
    // `[Int]()` constructs a builtin collection type that names no user symbol,
    // and an immediately-invoked closure has no callee identity at all.
    assert_eq!(extracted.callees(), vec!["run", "chain"]);
}

#[test]
fn swift_subscripts_are_not_calls_to_the_collection() {
    // The grammar makes `items[0]` a `call_expression` whose callee is the
    // collection. Recording it would let an array named after a same-file
    // function resolve to that function at deterministic confidence.
    let source = r#"
func f(items: [Int], render: ([Int]) -> Void) {
    let a = items[0]
    let b = dict["k"]
    render(items)
}
"#;
    let extracted = extract("sub.swift", "swift", source);
    assert_eq!(extracted.callees(), vec!["render"]);
}

#[test]
fn swift_binds_a_constructed_value_to_its_local_name() {
    let source = r#"
func f() {
    let service = Service()
    var other: Loader
    other = Loader()
    let nested = wrap(Inner())
}
"#;
    let extracted = extract("b.swift", "swift", source);
    assert_eq!(extracted.binding("Service"), Some("service"));
    assert_eq!(extracted.binding("Loader"), Some("other"));
    assert_eq!(extracted.binding("wrap"), Some("nested"));
    // An argument's own call must not claim the outer binding.
    assert_eq!(extracted.binding("Inner"), None);
}

const SCALA_REGISTRY: &str = r#"
package demo

case class Point(x: Int, y: Int)

trait Shape {
  def describe(): String = render()
  def render(): String
}

object Registry {
  def apply(name: String): Point = Point(1, 2)
  def register(p: Point): Unit = println(p)
  def main(): Unit = {
    val p = Point(1, 2)
    Registry.register(p)
    val q = Registry("named")
    val s = new Service()
    val o = new pkg.Other(1)
    s.start()
    def inner(): Unit = deepest()
    inner()
    words foreach println
    val total = 1 + 2
  }
}

class Service {
  def start(): Unit = ()
}
"#;

#[test]
fn scala_extracts_def_method_and_apply_calls() {
    let extracted = extract("r.scala", "scala", SCALA_REGISTRY);
    for callee in [
        "render", "Point", "println", "register", "Registry", "start", "inner", "deepest",
        "foreach", "Service", "Other",
    ] {
        assert!(
            extracted.callees().contains(&callee),
            "`{callee}` is a call in this file; found {:?}",
            extracted.callees()
        );
    }
    assert_eq!(extracted.receiver("register"), Some("Registry"));
    assert_eq!(extracted.receiver("start"), Some("s"));
    assert_eq!(extracted.binding("Service"), Some("s"));
}

#[test]
fn scala_attributes_object_methods_to_the_qualified_symbol_the_emitter_names() {
    let extracted = extract("r.scala", "scala", SCALA_REGISTRY);
    // `object` is a class to the emitter but invisible to
    // `enclosing_type_name`, so attributing through the shared helper would name
    // `r.scala::main`, which matches no node — the SC9/SC10 orphan shape.
    assert_eq!(extracted.caller("Registry"), Some("r.scala::Registry.main"));
    // A `trait` body is qualified by neither, and both agree it is unqualified.
    assert_eq!(extracted.caller("render"), Some("r.scala::describe"));
    // A nested `def` is emitted unqualified by the generic emitter.
    assert_eq!(extracted.caller("deepest"), Some("r.scala::inner"));
}

#[test]
fn scala_records_new_instances_with_their_qualifier() {
    let extracted = extract("r.scala", "scala", SCALA_REGISTRY);
    assert_eq!(
        extracted.reference_kind("Service"),
        ReferenceKind::Constructor
    );
    // `new pkg.Other(1)` names the type, not the package path.
    assert_eq!(extracted.receiver("Other"), Some("pkg"));
    assert_eq!(extracted.receiver("Service"), None);
}

#[test]
fn scala_records_alphabetic_infix_calls_and_not_symbolic_operators() {
    let extracted = extract("r.scala", "scala", SCALA_REGISTRY);
    // `words foreach println` is `words.foreach(println)`.
    assert_eq!(extracted.receiver("foreach"), Some("words"));
    // `1 + 2` is `1.+(2)`, and `+` is deliberately not recorded: it names no
    // identifier, and the corpus-wide volume of stdlib operators would swamp
    // the unresolved tiers for no resolvable target.
    for callee in extracted.callees() {
        assert!(
            !callee.contains('+'),
            "symbolic operator `{callee}` must not be recorded"
        );
    }
}

#[test]
fn scala_generic_and_chained_calls_name_the_method() {
    let source = r#"
object M {
  def run(): Unit = {
    val t = go[Int](3)
    list.map(transform).foreach(println)
    val s = new Box[Int]()
  }
}
"#;
    let extracted = extract("g.scala", "scala", source);
    let mut callees = extracted.callees();
    callees.sort_unstable();
    // `transform` and `println` are passed by name rather than invoked, so they
    // are references to a function value, not call sites. Recording them as
    // calls would claim an argument list that is not there; the frozen Python
    // baseline misses them for the same reason. Named as a gap in the report.
    assert_eq!(callees, vec!["Box", "foreach", "go", "map"]);
    // Type arguments are not part of a callee's identity.
    assert_eq!(extracted.receiver("go"), None);
    assert_eq!(extracted.receiver("map"), Some("list"));
}

/// The invariant SC9 and SC10 were both violations of: a call edge names a
/// caller, and that name is a join key. An edge whose source matches no node's
/// qualified name is an orphan, and orphans are gated at zero.
#[test]
fn every_caller_symbol_names_an_emitted_symbol() {
    let cases: [(&str, &str, &str); 4] = [
        ("s.swift", "swift", SWIFT_SERVICE),
        ("r.scala", "scala", SCALA_REGISTRY),
        (
            "v.swift",
            "swift",
            r#"
struct ContentView: View {
    var body: some View {
        VStack { Text("hi").padding() }
    }
    init() { prepare() }
    func prepare() {}
}

enum Mode {
    case fast
    static func pick() -> Mode { return choose() }
}
"#,
        ),
        (
            "n.scala",
            "scala",
            r#"
package demo

object Outer {
  class Inner {
    def deep(): Unit = helper()
    def helper(): Unit = ()
  }
  val eager = boot()
  def boot(): Unit = ()
}
"#,
        ),
    ];
    for (path, lang, source) in cases {
        let symbols = emitted_symbols(path, lang, source);
        let extracted = extract(path, lang, source);
        assert!(
            !extracted.calls.is_empty(),
            "{path} extracted no calls, so the invariant would be vacuous"
        );
        for call in &extracted.calls {
            let Some(caller) = call.caller_symbol.as_deref() else {
                // File scope: the file's own symbol always exists.
                continue;
            };
            assert!(
                symbols.iter().any(|symbol| symbol == caller),
                "orphaned call edge in {path}: `{caller}` -> `{}` names no emitted symbol; \
                 emitted are {symbols:?}",
                call.callee_name
            );
        }
        for reference in &extracted.references {
            let Some(scope) = reference.enclosing_symbol.as_deref() else {
                continue;
            };
            assert!(
                symbols.iter().any(|symbol| symbol == scope),
                "orphaned reference in {path}: `{scope}` names no emitted symbol"
            );
        }
    }
}

/// Extraction is a pure function of the source, so the same input must produce
/// the same rows in the same order — the property the determinism gate checks
/// at corpus scale.
#[test]
fn extraction_order_is_stable_across_runs() {
    for (path, lang, source) in [
        ("s.swift", "swift", SWIFT_SERVICE),
        ("r.scala", "scala", SCALA_REGISTRY),
    ] {
        let first = extract(path, lang, source);
        let second = extract(path, lang, source);
        assert_eq!(first.callees(), second.callees());
        let spans: Vec<_> = first
            .calls
            .iter()
            .map(|call| call.span.start_byte)
            .collect();
        let repeat: Vec<_> = second
            .calls
            .iter()
            .map(|call| call.span.start_byte)
            .collect();
        assert_eq!(spans, repeat);
    }
}

/// The wiring itself: with the dispatcher arms applied, a Swift and a Scala file
/// produce call edges through the public extraction entry point.
///
/// This is the test that fails *behaviourally* rather than at compile time
/// against the pre-change tree — `extract_calls` returned without doing
/// anything, so both counts were zero — and it keeps failing until the arms in
/// `langcalls/mod.rs` are applied, not merely the module declarations.
#[test]
fn the_dispatcher_routes_swift_and_scala_to_these_extractors() {
    assert!(
        devmap_extract::languages::capabilities_for_language("swift")
            .contains(devmap_extract::languages::Capability::Calls),
        "swift must be reported as covered"
    );
    assert!(
        devmap_extract::languages::capabilities_for_language("scala")
            .contains(devmap_extract::languages::Capability::Calls),
        "scala must be reported as covered"
    );
    for (path, lang, source) in [
        ("s.swift", "swift", SWIFT_SERVICE),
        ("r.scala", "scala", SCALA_REGISTRY),
    ] {
        let through_dispatcher = extract_treesitter(path, lang, source).calls;
        let direct = extract(path, lang, source);
        assert!(
            !through_dispatcher.is_empty(),
            "{lang} produced no calls through extract_treesitter; the dispatcher arm is missing"
        );
        assert_eq!(
            through_dispatcher.len(),
            direct.calls.len(),
            "the dispatcher must produce exactly what the extractor produces for {lang}"
        );
    }
}

/// A node kind the emitter names but this module walks past is how a caller name
/// silently stops matching its node. Pinned against the emitter's own output for
/// the shapes that differ between the two qualification rules.
#[test]
fn nested_and_object_scoped_declarations_agree_with_the_emitter() {
    let scala = r#"
object Outer {
  def top(): Unit = ()
  class Inner {
    def deep(): Unit = ()
  }
}
"#;
    let symbols = emitted_symbols("o.scala", "scala", scala);
    assert!(symbols.contains(&"o.scala::Outer.top".to_string()));
    assert!(symbols.contains(&"o.scala::Inner.deep".to_string()));

    let swift = r#"
struct Holder {
    func method() {}
}

func outer() {
    func inner() {}
}
"#;
    let symbols = emitted_symbols("h.swift", "swift", swift);
    assert!(symbols.contains(&"h.swift::Holder.method".to_string()));
    assert!(
        symbols.contains(&"h.swift::inner".to_string()),
        "the generic emitter names a nested func unqualified; got {symbols:?}"
    );
}
