//! Ruby and PHP call extraction (SC34).
//!
//! Both languages fell through `extract_node` to the generic arm, which emits
//! declarations only, so every consumer that reads the call graph — `impact`,
//! `trace`, dead code, the PDG — answered for them from nothing, with no signal
//! separating "no callers" from "callers were never extracted".
//!
//! Every test here fails against the pre-change tree, where `calls` is empty
//! for both languages. They go through `devmap_extract::extract_file`, which is
//! the path a build takes, so they also cover the dispatcher wiring rather than
//! only the modules behind it.

use devmap_extract::model::{ExtractedCall, ExtractedSymbol, Extraction, ReferenceKind};

fn calls_of(extraction: &Extraction) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = extraction
        .calls
        .iter()
        .map(|call| {
            (
                call.caller_symbol.clone().unwrap_or_default(),
                call.receiver_expr.clone().unwrap_or_default(),
                call.callee_name.clone(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn callees(extraction: &Extraction) -> Vec<String> {
    let mut names: Vec<String> = extraction
        .calls
        .iter()
        .map(|call| call.callee_name.clone())
        .collect();
    names.sort();
    names
}

fn find_call<'a>(extraction: &'a Extraction, callee: &str) -> &'a ExtractedCall {
    extraction
        .calls
        .iter()
        .find(|call| call.callee_name == callee)
        .unwrap_or_else(|| panic!("no call to `{callee}` in {:?}", callees(extraction)))
}

/// The load-bearing invariant (SC9/SC10): a call edge whose `caller_symbol`
/// names no emitted symbol is unjoinable, so any traversal out of that symbol
/// silently misses the call. Verified here rather than assumed.
fn assert_no_orphaned_callers(extraction: &Extraction) {
    let emitted: Vec<&str> = extraction
        .symbols
        .iter()
        .map(|symbol: &ExtractedSymbol| symbol.qualified_name.as_str())
        .collect();
    let orphans: Vec<&str> = extraction
        .calls
        .iter()
        .filter_map(|call| call.caller_symbol.as_deref())
        .filter(|caller| !emitted.contains(caller))
        .collect();
    assert!(
        orphans.is_empty(),
        "call edges name callers no symbol carries: {orphans:?}; emitted symbols are {emitted:?}"
    );
}

/// Every callee name must be an identifier, never an expression (SC26/SC32).
fn assert_callees_are_identifiers(extraction: &Extraction) {
    let bad: Vec<&str> = extraction
        .calls
        .iter()
        .map(|call| call.callee_name.as_str())
        .filter(|name| {
            name.is_empty()
                || !name
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '#'))
        })
        .collect();
    assert!(
        bad.is_empty(),
        "callee names carry expression text: {bad:?}"
    );
}

// ---------------------------------------------------------------- Ruby

/// The exact regression: the Python implementation this port replaces recovers
/// `c.rb::main -> helper`, and this port recovered nothing.
#[test]
fn a_ruby_call_is_attributed_to_the_method_that_makes_it() {
    let extraction =
        devmap_extract::extract_file("c.rb", "def helper(a); a; end\ndef main; helper(1); end\n");
    assert_eq!(
        calls_of(&extraction),
        vec![(
            "c.rb::main".to_string(),
            String::new(),
            "helper".to_string()
        )]
    );
    assert_no_orphaned_callers(&extraction);
}

#[test]
fn ruby_receiver_shapes_keep_the_method_as_the_callee() {
    let source = r#"
class Service
  def run
    helper(1)
    self.inner
    @client.fetch(2)
    other&.maybe
    JSON.generate({ok: true})
    Foo::Bar.baz(3)
    ::Kernel.puts "z"
    plain_send arg
  end
end
"#;
    let extraction = devmap_extract::extract_file("s.rb", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "s.rb::Service.run".to_string(),
                String::new(),
                "helper".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                String::new(),
                "plain_send".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                "@client".to_string(),
                "fetch".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                "Bar".to_string(),
                "baz".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                "JSON".to_string(),
                "generate".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                "Kernel".to_string(),
                "puts".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                "other".to_string(),
                "maybe".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                "self".to_string(),
                "inner".to_string()
            ),
        ]
    );
    assert_callees_are_identifiers(&extraction);
    assert_no_orphaned_callers(&extraction);
}

/// `::Kernel.puts` reduces to the constant it names. The bare name is the
/// dispatch key, so `::Kernel` as a receiver would name a type no lookup finds.
#[test]
fn a_qualified_ruby_receiver_reduces_to_its_innermost_constant() {
    let extraction = devmap_extract::extract_file("q.rb", "def f\n  Foo::Bar::Baz.run(1)\nend\n");
    assert_eq!(
        find_call(&extraction, "run").receiver_expr.as_deref(),
        Some("Baz")
    );
}

#[test]
fn ruby_calls_inside_blocks_belong_to_the_enclosing_method() {
    let source = r#"
class Service
  def run(items)
    items.each { |i| tick(i) }
    items.map do |i|
      step(i)
    end
  end
end
"#;
    let extraction = devmap_extract::extract_file("b.rb", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "b.rb::Service.run".to_string(),
                String::new(),
                "step".to_string()
            ),
            (
                "b.rb::Service.run".to_string(),
                String::new(),
                "tick".to_string()
            ),
            (
                "b.rb::Service.run".to_string(),
                "items".to_string(),
                "each".to_string()
            ),
            (
                "b.rb::Service.run".to_string(),
                "items".to_string(),
                "map".to_string()
            ),
        ]
    );
    assert_no_orphaned_callers(&extraction);
}

/// `attr_accessor` and `require` are ordinary sends, not declarations, and a
/// class body is not a method. Their caller is the nearest symbol that actually
/// exists — the class for a body send, the file for a top-level one — never a
/// fabricated method scope.
#[test]
fn ruby_class_body_sends_belong_to_the_class_and_top_level_sends_to_the_file() {
    let source =
        "require \"json\"\nclass Service\n  attr_accessor :name\n  attr_reader :other\nend\n";
    let extraction = devmap_extract::extract_file("m.rb", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (String::new(), String::new(), "require".to_string()),
            (
                "m.rb::Service".to_string(),
                String::new(),
                "attr_accessor".to_string()
            ),
            (
                "m.rb::Service".to_string(),
                String::new(),
                "attr_reader".to_string()
            ),
        ]
    );
    assert_no_orphaned_callers(&extraction);
}

/// `class << self` is a `singleton_class` with a `value` field and no `name`,
/// so the emitter gives its methods no owner and names them `file::forge`. The
/// caller has to agree with that, coarse as it is, or the edge is an orphan.
#[test]
fn ruby_module_and_singleton_definitions_own_the_calls_they_contain() {
    let source = r#"
module Outer
  def helper
    inner_one(1)
  end
  class Inner
    def self.factory
      build(2)
    end
    class << self
      def forge
        cast(4)
      end
    end
  end
end
def top
  free(3)
end
"#;
    let extraction = devmap_extract::extract_file("d.rb", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "d.rb::Inner.factory".to_string(),
                String::new(),
                "build".to_string()
            ),
            (
                "d.rb::Outer.helper".to_string(),
                String::new(),
                "inner_one".to_string()
            ),
            ("d.rb::forge".to_string(), String::new(), "cast".to_string()),
            ("d.rb::top".to_string(), String::new(), "free".to_string()),
        ]
    );
    assert_no_orphaned_callers(&extraction);
}

/// Ruby has no `new` operator, so `Widget.new` is the only evidence a value is
/// a `Widget`. Recorded as a `Constructor` reference naming the class, which is
/// the shape the resolver reads to type a receiver.
#[test]
fn a_ruby_constructor_send_binds_its_local_to_the_class() {
    let extraction = devmap_extract::extract_file("c2.rb", "def build\n  w = Widget.new(1)\nend\n");
    let constructor = extraction
        .references
        .iter()
        .find(|reference| reference.kind == ReferenceKind::Constructor)
        .expect("Widget.new must record a constructor reference");
    assert_eq!(constructor.name, "Widget");
    assert_eq!(constructor.assigned_to.as_deref(), Some("w"));
    assert_eq!(
        constructor.enclosing_symbol.as_deref(),
        Some("c2.rb::build")
    );
    // A lowercase receiver names a local variable and proves nothing.
    let plain = devmap_extract::extract_file("c3.rb", "def build\n  w = factory.new\nend\n");
    assert!(plain
        .references
        .iter()
        .all(|reference| reference.kind != ReferenceKind::Constructor));
}

/// Three shapes that all reach the same `method` field and must not be treated
/// alike. An operator send has no name. An attribute write sends `name=` and is
/// handed the reader's node, so recording it would point a write at `def name`.
/// A predicate send *is* a name — the declaration is emitted as `ok?`, so the
/// callee has to carry the `?` or the edge cannot join.
#[test]
fn ruby_operator_and_setter_sends_are_refused_but_predicates_are_kept() {
    let source = "class A\n  def ok?; true; end\n  def name; 1; end\n  def f(a, b)\n    a.+(b)\n    a.name = b\n    a.ok?\n  end\nend\n";
    let extraction = devmap_extract::extract_file("o.rb", source);
    assert_eq!(callees(&extraction), vec!["ok?".to_string()]);
    // The declaration carries the same trailing character, which is why
    // admitting it in the callee joins an edge instead of inventing one.
    assert!(extraction
        .symbols
        .iter()
        .any(|symbol| symbol.qualified_name == "o.rb::A.ok?"));
    assert_no_orphaned_callers(&extraction);
}

// ---------------------------------------------------------------- PHP

/// The exact regression, PHP side.
#[test]
fn a_php_call_is_attributed_to_the_function_that_makes_it() {
    let extraction = devmap_extract::extract_file(
        "e.php",
        "<?php\nfunction helper($a){ return $a; }\nfunction main(){ return helper(1); }\n",
    );
    assert_eq!(
        calls_of(&extraction),
        vec![(
            "e.php::main".to_string(),
            String::new(),
            "helper".to_string()
        )]
    );
    assert_no_orphaned_callers(&extraction);
}

#[test]
fn php_call_shapes_keep_the_method_as_the_callee() {
    let source = r#"<?php
namespace App;
final class Service {
  public function run(): void {
    helper();
    $this->inner();
    self::stat();
    static::other();
    parent::base();
    Widget::make(1);
    \App\Deep\fn_call();
    \App\Widget::forge();
    $obj->method(2);
    $obj?->maybe();
  }
}
"#;
    let extraction = devmap_extract::extract_file("p.php", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "p.php::Service.run".to_string(),
                String::new(),
                "helper".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "$obj".to_string(),
                "maybe".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "$obj".to_string(),
                "method".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "$this".to_string(),
                "inner".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "App\\Deep".to_string(),
                "fn_call".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "Widget".to_string(),
                "forge".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "Widget".to_string(),
                "make".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "parent".to_string(),
                "base".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "self".to_string(),
                "stat".to_string()
            ),
            (
                "p.php::Service.run".to_string(),
                "static".to_string(),
                "other".to_string()
            ),
        ]
    );
    assert_callees_are_identifiers(&extraction);
    assert_no_orphaned_callers(&extraction);
}

#[test]
fn php_new_records_a_constructor_reference_and_binds_its_variable() {
    let extraction = devmap_extract::extract_file(
        "n.php",
        "<?php\nfunction build(){ $w = new Widget(3); $d = new \\App\\Deep(); }\n",
    );
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "n.php::build".to_string(),
                String::new(),
                "Widget".to_string()
            ),
            (
                "n.php::build".to_string(),
                "App".to_string(),
                "Deep".to_string()
            ),
        ]
    );
    let widget = extraction
        .references
        .iter()
        .find(|reference| reference.name == "Widget")
        .expect("new Widget must record a reference");
    assert_eq!(widget.kind, ReferenceKind::Constructor);
    assert_eq!(widget.assigned_to.as_deref(), Some("$w"));
    assert_no_orphaned_callers(&extraction);
}

/// A dynamic target names its callee only at run time. `is_callee_identity`
/// admits a leading `$`, so refusing these has to be structural or `$cb` gets
/// recorded as a callee name that no symbol can ever carry.
#[test]
fn php_dynamic_call_targets_record_nothing() {
    let source = r#"<?php
function run($cb, $arr, $obj, $cls, $m) {
  $cb();
  $arr['k']();
  $obj->$m();
  $cls::$m();
  $x = new $cls();
}
"#;
    let extraction = devmap_extract::extract_file("dyn.php", source);
    assert_eq!(callees(&extraction), Vec::<String>::new());
}

/// A trait, an interface and an enum all own their methods in the symbol
/// emitter. `enclosing_type_name` knows only `class_declaration`, so reusing it
/// here would attribute a trait method's calls to `file::method` while the
/// symbol is `file::T.method` — an orphaned edge.
#[test]
fn php_trait_and_enum_methods_own_their_calls() {
    let source = r#"<?php
trait T { public function tm() { helper_t(); } }
enum E { case A; public function em() { helper_e(); } }
final class C { public function cm() { helper_c(); } }
"#;
    let extraction = devmap_extract::extract_file("t.php", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "t.php::C.cm".to_string(),
                String::new(),
                "helper_c".to_string()
            ),
            (
                "t.php::E.em".to_string(),
                String::new(),
                "helper_e".to_string()
            ),
            (
                "t.php::T.tm".to_string(),
                String::new(),
                "helper_t".to_string()
            ),
        ]
    );
    assert_no_orphaned_callers(&extraction);
}

/// A closure emits no symbol, so naming one as a caller would name a node that
/// does not exist. The walk passes through to the enclosing named function,
/// exactly as it already does for an unnamed arrow function.
#[test]
fn php_closure_bodies_attribute_to_the_enclosing_named_function() {
    let source = r#"<?php
function outer() {
  $f = function() { inner_c(); };
  $g = fn($x) => scaled($x);
}
"#;
    let extraction = devmap_extract::extract_file("cl.php", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "cl.php::outer".to_string(),
                String::new(),
                "inner_c".to_string()
            ),
            (
                "cl.php::outer".to_string(),
                String::new(),
                "scaled".to_string()
            ),
        ]
    );
    assert_no_orphaned_callers(&extraction);
}

/// A function nested inside a function is owned by no type, so its calls must
/// not inherit the outer class the way an ancestor walk would give them.
#[test]
fn a_php_function_nested_in_a_method_is_owned_by_no_type() {
    let source = r#"<?php
class C {
  public function m() {
    function nested() { deep(); }
    shallow();
  }
}
"#;
    let extraction = devmap_extract::extract_file("nest.php", source);
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "nest.php::C.m".to_string(),
                String::new(),
                "shallow".to_string()
            ),
            (
                "nest.php::nested".to_string(),
                String::new(),
                "deep".to_string()
            ),
        ]
    );
    assert_no_orphaned_callers(&extraction);
}

/// The coverage list is what tells a consumer "this language has no call
/// graph" rather than leaving it as an indistinguishable zero.
#[test]
fn ruby_and_php_are_reported_as_covered() {
    assert!(devmap_extract::languages::capabilities_for_language("ruby")
        .contains(devmap_extract::languages::Capability::Calls));
    assert!(devmap_extract::languages::capabilities_for_language("php")
        .contains(devmap_extract::languages::Capability::Calls));
}
