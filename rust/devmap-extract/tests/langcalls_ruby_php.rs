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
    // `run` takes no parameters and binds nothing, so the argument `arg` and
    // the receiver `other` are bare sends too: Ruby calls a method for each.
    assert_eq!(
        calls_of(&extraction),
        vec![
            (
                "s.rb::Service.run".to_string(),
                String::new(),
                "arg".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                String::new(),
                "helper".to_string()
            ),
            (
                "s.rb::Service.run".to_string(),
                String::new(),
                "other".to_string()
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

/// Callee names a file's calls make from inside `caller`, sorted.
fn callees_from(extraction: &Extraction, caller: &str) -> Vec<String> {
    let mut names: Vec<String> = extraction
        .calls
        .iter()
        .filter(|call| call.caller_symbol.as_deref() == Some(caller))
        .map(|call| call.callee_name.clone())
        .collect();
    names.sort();
    names
}

/// The measured control (GAP-1 residue). `def entry; bare_helper; end` is a
/// send: Ruby's parser commits an identifier to a method call unless a binding
/// of that name appears in the enclosing scope, and nothing here binds
/// `bare_helper`. tree-sitter-ruby gives it a plain `identifier`, not a `call`,
/// so before this was claimed `impact` on `bare_helper` answered a confident
/// zero. The second method is the same name read as a local, which Ruby reads
/// as the local — recording a call there would be the SC9 wrong edge.
#[test]
fn a_bare_ruby_send_is_a_call_and_a_local_read_of_the_same_name_is_not() {
    let source = "class CtlRb\n  def entry\n    bare_helper\n  end\n  def entry_local\n    bare_helper = 1\n    bare_helper\n  end\n  def bare_helper\n    1\n  end\nend\n";
    let extraction = devmap_extract::extract_file("ctl.rb", source);
    assert_eq!(
        calls_of(&extraction),
        vec![(
            "ctl.rb::CtlRb.entry".to_string(),
            String::new(),
            "bare_helper".to_string()
        )]
    );
    // The mirrored reference is what the resolver reads; one call, one reference.
    let references: Vec<_> = extraction
        .references
        .iter()
        .filter(|reference| reference.kind == ReferenceKind::Call)
        .map(|reference| {
            (
                reference.name.as_str(),
                reference.enclosing_symbol.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        references,
        vec![("bare_helper", Some("ctl.rb::CtlRb.entry"))]
    );
    assert_no_orphaned_callers(&extraction);
}

/// Every value position a bare send can occupy, each with no binding of the
/// name in scope. `current_user.name` is the Rails shape: `current_user` is the
/// receiver *and* a send.
#[test]
fn bare_ruby_sends_are_claimed_in_every_value_position() {
    let source = r##"
class Page
  def endless = endless_target
  def statement; in_statement; end
  def argument; render in_argument; end
  def receiver; current_user.name; end
  def assigned; x = in_assignment; x; end
  def interpolated; "#{in_interpolation}"; end
  def condition; return 1 if in_condition; end
  def operand; in_left + in_right; end
  def indexed; in_object[in_index]; end
  def in_block; [1].each { |i| in_block_body(i); in_block_bare }; end
  def defaulted(a = in_default); a; end
  def rescued; risky rescue in_rescue_modifier; end
end
"##;
    let extraction = devmap_extract::extract_file("p.rb", source);
    let expect = [
        ("endless", vec!["endless_target"]),
        ("statement", vec!["in_statement"]),
        ("argument", vec!["in_argument", "render"]),
        ("receiver", vec!["current_user", "name"]),
        ("assigned", vec!["in_assignment"]),
        ("interpolated", vec!["in_interpolation"]),
        ("condition", vec!["in_condition"]),
        ("operand", vec!["in_left", "in_right"]),
        ("indexed", vec!["in_index", "in_object"]),
        ("in_block", vec!["each", "in_block_bare", "in_block_body"]),
        ("defaulted", vec!["in_default"]),
        ("rescued", vec!["in_rescue_modifier", "risky"]),
    ];
    for (method, callees) in expect {
        assert_eq!(
            callees_from(&extraction, &format!("p.rb::Page.{method}")),
            callees,
            "calls made by `{method}`"
        );
    }
    // `x = in_assignment` binds `x` to the send's value, as `x = f()` would.
    let assigned = find_call(&extraction, "in_assignment");
    assert_eq!(assigned.receiver_expr, None);
    let reference = extraction
        .references
        .iter()
        .find(|reference| reference.name == "in_assignment")
        .expect("the bare send records its reference");
    assert_eq!(reference.assigned_to.as_deref(), Some("x"));
    assert_callees_are_identifiers(&extraction);
    assert_no_orphaned_callers(&extraction);
}

/// Every binding form Ruby has, each followed by a read of the bound name. Not
/// one of these reads is a send, so not one may become a call. A form missed
/// here is a confidently wrong edge on ordinary code, which is why the list is
/// exhaustive against the grammar's binding positions rather than a sample.
#[test]
fn no_ruby_local_binding_form_is_mistaken_for_a_send() {
    let source = r#"
def positional(p); p; end
def optional(q = 1); q; end
def splat(*r); r; end
def keyword(k:, kd: 1); k; kd; end
def double_splat(**o); o; end
def block_param(&blk); blk; end
def destructured_param((da, db)); da; db; end
def assigned; a = 1; a; end
def operator_assigned; oa ||= 1; oa; end
def multiple; ma, (mb, *mc) = 1, 2; ma; mb; mc; end
def block_params; [1].each { |bp; blocal| bp; blocal }; end
def do_block_params; [1].each do |dp| dp end; end
def lambda_params; ->(lp) { lp }; end
def rescue_var; begin; rescue StandardError => err; err; end; end
def for_var; for fv in [1]; fv; end; end
def array_pattern(v); case v; in [pa, *prest] then pa; prest; end; end
def hash_pattern(v); case v; in {hk: hv} then hv; end; end
def hash_shorthand(v); case v; in {hs:} then hs; end; end
def as_pattern(v); case v; in Integer => asv then asv; end; end
def find_pattern(v); case v; in [*, fp, *] then fp; end; end
def alternative(v); case v; in [alt] | {alt:} then alt; end; end
def guarded(v); case v; in [gv] if gv then gv; end; end
def rightward(v); v => {rk:}; rk; end
def test_pattern(v); v in tp; tp; end
def named_capture(s); /(?<cap>\d+)/ =~ s; cap; end
def quoted_capture(s); /(?'qcap'\d+)/ =~ s; qcap; end
def bound_later; late; late = 1; end
def bound_in_block; [1].each { inner_bound = 1 }; inner_bound; end
"#;
    let extraction = devmap_extract::extract_file("l.rb", source);
    // The only sends in the file are the ones written as sends.
    assert_eq!(callees(&extraction), vec!["each", "each", "each"]);
    assert_no_orphaned_callers(&extraction);
}

/// Identifiers in a value position that still are not a send Ruby makes, or
/// whose scope this module cannot judge, are refused rather than guessed at.
#[test]
fn ruby_identifiers_that_are_not_judged_sends_are_refused() {
    let source = r#"
class Child < parent_expr
  alias new_name old_name
  undef gone
  def probe; defined?(maybe_defined); end
  def implicit_it; [1].each { it }; end
  def numbered; [1].each { _1 + _2 }; end
  def pinned(v); case v; in ^pinned_local then 1; end; end
  def obj.singleton_owner; 1; end
end
class << singleton_expr
end
"#;
    let extraction = devmap_extract::extract_file("r.rb", source);
    assert_eq!(callees(&extraction), vec!["each", "each"]);
    assert_no_orphaned_callers(&extraction);
}

/// Ruby's scope gates are `def`, `class`, `module` and the file: a local of
/// the class body is not visible inside a method, and a method's locals are
/// not visible to its sibling. Blocks are not gates, so a block sees the
/// method's locals. Each half of that rule is load-bearing in one direction.
#[test]
fn ruby_locals_are_judged_per_def_class_and_file_scope() {
    let source = r#"
top_local = 1
class Gate
  body_local = 1
  def reads_body_local; body_local; end
  def reads_top_local; top_local; end
  def owns_one; mine = 1; mine; end
  def reads_sibling; mine; end
  def through_block; seen = 1; [1].each { seen }; end
end
top_local
"#;
    let extraction = devmap_extract::extract_file("g.rb", source);
    assert_eq!(
        callees_from(&extraction, "g.rb::Gate.reads_body_local"),
        vec!["body_local"]
    );
    assert_eq!(
        callees_from(&extraction, "g.rb::Gate.reads_top_local"),
        vec!["top_local"]
    );
    assert_eq!(
        callees_from(&extraction, "g.rb::Gate.reads_sibling"),
        vec!["mine"]
    );
    assert!(callees_from(&extraction, "g.rb::Gate.owns_one").is_empty());
    assert_eq!(
        callees_from(&extraction, "g.rb::Gate.through_block"),
        vec!["each"]
    );
    // The class body reads none of its own locals, and the file reads its own.
    assert!(callees_from(&extraction, "g.rb::Gate").is_empty());
    assert!(extraction
        .calls
        .iter()
        .all(|call| call.caller_symbol.is_some() || call.callee_name != "top_local"));
    assert_no_orphaned_callers(&extraction);
}

/// A method Ruby defines without `def` has no declaration in the graph, so a
/// bare send naming it can only bind to a *different* method of that name —
/// measured on Homebrew, `TapCaskUnavailableError#to_s` calling its own
/// `attr_reader :tap` was bound to `Cask.tap` at 0.9. No call to
/// such a name can be right, so none is recorded; the plain `def` beside them
/// keeps its edge, which is the control that the refusal is not wholesale.
#[test]
fn a_bare_send_to_a_method_defined_without_def_is_refused() {
    let source = r#"
class Err
  attr_reader :reason
  attr_accessor(:acc)
  attr :plain_attr
  private attr_reader :priv
  define_method(:made) { 1 }
  alias_method :aliased, :to_s
  alias kw_alias to_s
  def_delegators :@target, :forwarded
  delegate :railsy, to: :target
  def to_s; reason; acc; plain_attr; priv; made; aliased; kw_alias; forwarded; railsy; real; end
  def real; 1; end
  class << self
    attr_reader :meta
    def build; meta; end
  end
end
class Point < Struct.new(:px, :py)
  def norm; px; end
end
"#;
    let extraction = devmap_extract::extract_file("a.rb", source);
    assert_eq!(callees_from(&extraction, "a.rb::Err.to_s"), vec!["real"]);
    assert!(callees_from(&extraction, "a.rb::build").is_empty());
    assert!(callees_from(&extraction, "a.rb::Point.norm").is_empty());
    assert_no_orphaned_callers(&extraction);
}

/// A scope tree-sitter could not parse cleanly has no trustworthy binding
/// list: error recovery can drop the very assignment that makes a name local.
/// Its bare identifiers are refused; a clean sibling method keeps its sends.
#[test]
fn a_ruby_scope_with_a_parse_error_claims_no_bare_sends() {
    let source = "def clean; clean_send; end\ndef broken\n  x = = 1\n  x\n  broken_send\nend\n";
    let extraction = devmap_extract::extract_file("e.rb", source);
    assert_eq!(callees_from(&extraction, "e.rb::clean"), vec!["clean_send"]);
    assert!(extraction
        .calls
        .iter()
        .all(|call| call.callee_name != "x" && call.callee_name != "broken_send"));
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
