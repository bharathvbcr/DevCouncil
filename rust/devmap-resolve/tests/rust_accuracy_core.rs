//! Rust receiver and path accuracy at its core: the shapes the S1 accuracy
//! fixture rows (`rust/testdata/accuracy/rust`) pin one construct at a time,
//! stated here as the smallest source that shows each one, with the near miss
//! each rule must keep refusing.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

struct Resolved {
    edges: Vec<(String, String, String)>,
    /// `(source, target, confidence in thousandths)` for every edge.
    tiers: Vec<(String, String, i64)>,
    unresolved: Vec<(String, String)>,
    /// `(source, callee, class label)` for every ledger row.
    classes: Vec<(String, String, &'static str)>,
}

fn resolve(files: &[(&str, &str)]) -> Resolved {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, body)| extract_file(path, body))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions).expect("resolution");
    Resolved {
        tiers: result
            .edges
            .iter()
            .map(|edge| {
                (
                    edge.source_symbol.clone(),
                    edge.target_symbol.clone(),
                    edge.confidence.to_millis(),
                )
            })
            .collect(),
        classes: result
            .unresolved
            .iter()
            .map(|row| {
                (
                    row.source_symbol.clone(),
                    row.callee_name.clone(),
                    row.class.label(),
                )
            })
            .collect(),
        edges: result
            .edges
            .into_iter()
            .map(|edge| {
                (
                    edge.source_symbol,
                    edge.target_symbol,
                    format!("{:?}", edge.edge_kind),
                )
            })
            .collect(),
        unresolved: result
            .unresolved
            .into_iter()
            .map(|row| (row.source_symbol, row.callee_name))
            .collect(),
    }
}

impl Resolved {
    fn has(&self, source: &str, target: &str) -> bool {
        self.edges
            .iter()
            .any(|(from, to, _)| from == source && to == target)
    }

    /// The highest confidence of any edge from `source` to `target`.
    fn tier(&self, source: &str, target: &str) -> Option<i64> {
        self.tiers
            .iter()
            .filter(|(from, to, _)| from == source && to == target)
            .map(|(_, _, millis)| *millis)
            .max()
    }

    fn class_of(&self, source: &str, callee: &str) -> Vec<&'static str> {
        self.classes
            .iter()
            .filter(|(from, name, _)| from == source && name == callee)
            .map(|(_, _, class)| *class)
            .collect()
    }
}

#[test]
fn self_dot_f_on_a_foreign_type_is_not_the_free_function_but_self_path_f_is() {
    let src = "\
use std::process::Command;
pub fn spawn(c: &mut Command) { c.spawn(); }
pub trait Ext { fn run_ext(&mut self); }
impl Ext for Command {
    fn run_ext(&mut self) { self.spawn(); }
}
pub fn via_path() { self::spawn_here(); }
fn spawn_here() {}
";
    let r = resolve(&[("a.rs", src)]);
    assert!(
        !r.has("a.rs::Command.run_ext", "a.rs::spawn"),
        "self.spawn() on a foreign type bound the free fn: {:?}",
        r.edges
    );
    assert!(
        r.has("a.rs::via_path", "a.rs::spawn_here"),
        "`self::f()` is a module path and still binds: {:?}",
        r.edges
    );
}

#[test]
fn a_method_name_inside_macro_arguments_is_not_a_reference_to_a_free_fn() {
    let src = "\
pub struct Gauge;
impl Gauge { pub fn level(&self) -> u8 { 1 } }
pub fn level() -> u8 { 0 }
pub fn probe(g: &Gauge) { assert_eq!(g.level(), 1); }
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::probe", "a.rs::Gauge.level"), "{:?}", r.edges);
    assert!(!r.has("a.rs::probe", "a.rs::level"), "{:?}", r.edges);
}

#[test]
fn a_call_inside_a_closure_in_a_same_name_let_binds_the_outer_item_but_a_captured_local_does_not() {
    let src = "\
fn find(x: u8) -> Option<u8> { Some(x) }
pub fn outer(opt: Option<u8>) -> u8 {
    let (n, find) = (1, opt.and_then(|p| find(p)));
    n + find.unwrap_or(0)
}
pub fn shadowed(opt: Option<u8>) -> u8 {
    let (n, find) = (1, opt.map(|find: fn(u8) -> u8| find(1)));
    n + find.unwrap_or(0)
}
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::outer", "a.rs::find"), "{:?}", r.edges);
    assert!(!r.has("a.rs::shadowed", "a.rs::find"), "{:?}", r.edges);
}

#[test]
fn an_inline_module_path_binds_this_files_function_and_a_crate_path_does_not() {
    let src = "\
mod fast { pub fn gated() -> u8 { 3 } }
pub fn spawn() {}
pub fn caller() -> u8 {
    tokio::spawn();
    fast::gated()
}
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::caller", "a.rs::gated"), "{:?}", r.edges);
    assert!(
        !r.has("a.rs::caller", "a.rs::spawn"),
        "`tokio::spawn()` is another crate's: {:?}",
        r.edges
    );
}

#[test]
fn a_method_is_found_through_deref_and_a_type_declaring_it_stops_the_walk() {
    let src = "\
use std::ops::Deref;
pub struct Inner;
impl Inner { pub fn only_inner(&self) {} pub fn shared(&self) {} }
pub struct Guard(Inner);
impl Deref for Guard { type Target = Inner; fn deref(&self) -> &Inner { &self.0 } }
impl Guard { pub fn shared(&self) {} }
pub fn use_it(g: Guard) { g.only_inner(); g.shared(); }
";
    let r = resolve(&[("a.rs", src)]);
    assert!(
        r.has("a.rs::use_it", "a.rs::Inner.only_inner"),
        "{:?}",
        r.edges
    );
    assert!(r.has("a.rs::use_it", "a.rs::Guard.shared"), "{:?}", r.edges);
    assert!(
        !r.has("a.rs::use_it", "a.rs::Inner.shared"),
        "{:?}",
        r.edges
    );
}

#[test]
fn closure_and_pattern_receivers_take_the_element_type() {
    let src = "\
pub struct Item { v: i32 }
impl Item { pub fn keep(&self) -> bool { self.v > 0 } pub fn score(&self) -> i32 { self.v } }
pub fn chain(items: &[Item]) -> i32 {
    items.iter().filter(|i| i.keep()).map(|i| i.score()).sum()
}
pub fn some(opt: Option<Item>) -> i32 {
    if let Some(it) = opt { it.score() } else { 0 }
}
pub fn after_map(items: &[Item]) -> usize {
    items.iter().map(|i| i.score()).filter(|n| n.abs() > 1).count()
}
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::chain", "a.rs::Item.keep"), "{:?}", r.edges);
    assert!(r.has("a.rs::chain", "a.rs::Item.score"), "{:?}", r.edges);
    assert!(r.has("a.rs::some", "a.rs::Item.score"), "{:?}", r.edges);
}

#[test]
fn a_free_function_value_types_its_receiver_through_question_mark_and_module_paths() {
    let src = "\
pub struct Out;
impl Out { pub fn read(&self) -> u8 { 1 } }
pub fn make() -> Out { Out }
pub fn try_make() -> Result<Out, String> { Ok(Out) }
mod sub { pub fn nested() -> super::Out { super::Out } }
pub fn go() -> Result<u8, String> {
    let a = make().read();
    let b = try_make()?.read();
    let c = sub::nested().read();
    Ok(a + b + c)
}
pub fn not_unwrapped() { let _ = try_make().read(); }
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::go", "a.rs::Out.read"), "{:?}", r.edges);
    assert!(
        !r.has("a.rs::not_unwrapped", "a.rs::Out.read"),
        "a Result is not an Out until `?` unwraps it: {:?}",
        r.edges
    );
}

#[test]
fn guard_chains_and_wrapper_constructors_reach_the_wrapped_type() {
    let src = "\
use std::{cell::RefCell, rc::Rc, sync::{Arc, Mutex}};
pub struct Bag { n: u8 }
impl Bag { pub fn push(&mut self) {} }
pub fn go() {
    let rc = Rc::new(RefCell::new(Bag { n: 0 }));
    rc.borrow_mut().push();
    let m = Arc::new(Mutex::new(Bag { n: 0 }));
    m.lock().unwrap().push();
}
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::go", "a.rs::Bag.push"), "{:?}", r.edges);
}

#[test]
fn a_literal_receiver_binds_one_extension_method_and_abstains_between_two() {
    let one = "\
pub trait StrExt { fn shout(&self) -> String; }
impl StrExt for str { fn shout(&self) -> String { self.to_uppercase() } }
pub fn go() -> String { \"abc\".shout() }
";
    let r = resolve(&[("a.rs", one)]);
    assert!(r.has("a.rs::go", "a.rs::str.shout"), "{:?}", r.edges);
    let two = "\
pub trait StrExt { fn shout(&self) -> String; }
impl StrExt for str { fn shout(&self) -> String { self.to_uppercase() } }
impl StrExt for String { fn shout(&self) -> String { self.to_uppercase() } }
pub fn go() -> String { \"abc\".shout() }
";
    let r = resolve(&[("a.rs", two)]);
    assert!(
        !r.edges.iter().any(|(from, _, _)| from == "a.rs::go"
            && r.edges
                .iter()
                .any(|(f, t, _)| f == from && t.ends_with(".shout"))),
        "two impls answer a string literal: {:?}",
        r.edges
    );
}

#[test]
fn generate_handler_lists_are_references_by_path() {
    let main = "\
mod commands;
fn main() {
    tauri::Builder::default().invoke_handler(tauri::generate_handler![
        commands::cmd_a,
        #[cfg(debug_assertions)]
        commands::cmd_b,
    ]);
}
";
    let commands = "pub fn cmd_a() {}\nfn cmd_b() {}\n";
    let r = resolve(&[("main.rs", main), ("commands.rs", commands)]);
    assert!(
        r.has("main.rs::main", "commands.rs::cmd_a"),
        "{:?}",
        r.edges
    );
    assert!(
        r.has("main.rs::main", "commands.rs::cmd_b"),
        "{:?}",
        r.edges
    );
}

#[test]
fn a_local_macro_invocation_makes_the_calls_its_rules_make() {
    let src = "\
pub fn emit(m: &str) -> usize { m.len() }
macro_rules! log { ($m:expr) => { $crate::emit($m) }; }
pub fn caller() -> usize { log!(\"x\") }
";
    let r = resolve(&[("src/lib.rs", src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::emit"),
        "{:?}",
        r.edges
    );
}

#[test]
fn implicit_operator_calls_bind_when_the_type_is_stated_and_leave_no_row_otherwise() {
    let src = "\
use std::ops::Add;
#[derive(Clone, Copy)]
pub struct V(pub i32);
impl Add for V { type Output = V; fn add(self, o: V) -> V { V(self.0 + o.0) } }
pub struct Res;
impl Drop for Res { fn drop(&mut self) {} }
pub fn go(a: i32, b: i32) -> i32 {
    let s = V(1) + V(2);
    let _r = Res;
    a + b + s.0
}
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::go", "a.rs::V.add"), "{:?}", r.edges);
    assert!(r.has("a.rs::go", "a.rs::Res.drop"), "{:?}", r.edges);
    assert!(
        !r.unresolved
            .iter()
            .any(|(from, callee)| from == "a.rs::go" && matches!(callee.as_str(), "add" | "drop")),
        "an operator candidate that binds nothing must not be recorded: {:?}",
        r.unresolved
    );
}

#[test]
fn inline_module_twins_share_one_identity() {
    let twins = "\
mod alpha { pub fn twin() -> u8 { 1 } }
mod beta { pub fn twin() -> u8 { 2 } }
pub fn go() -> u8 { alpha::twin() }
";
    let r = resolve(&[("a.rs", twins)]);
    // One node stands for both functions (DIVERGENCES: inline-module twins), so
    // the path binds that node; it cannot say which module's function it is.
    assert!(r.has("a.rs::go", "a.rs::twin"), "{:?}", r.edges);
}

#[test]
fn a_question_mark_into_a_declared_error_type_is_a_candidate_from_call_below_the_high_tier() {
    let src = "\
pub struct MyErr;
impl From<std::num::ParseIntError> for MyErr { fn from(_: std::num::ParseIntError) -> Self { MyErr } }
pub fn go(s: &str) -> Result<i32, MyErr> { Ok(s.parse::<i32>()?) }
fn own() -> Result<i32, MyErr> { Ok(1) }
pub fn identity() -> Result<i32, MyErr> { Ok(own()?) }
pub fn in_closure(v: &[&str]) -> Result<Vec<i32>, MyErr> {
    Ok(v.iter().map(|s| -> Result<i32, std::num::ParseIntError> { Ok(s.parse::<i32>()?) }).filter_map(Result::ok).collect())
}
pub fn optional(s: &str) -> Option<i32> { let n = s.parse::<i32>().ok()?; Some(n) }
";
    let r = resolve(&[("a.rs", src)]);
    // The operand's error type is not known, so the conversion is a candidate:
    // an edge a walk follows, never a HIGH claim, since `?` on a value that is
    // already a `MyErr` runs the identity conversion instead.
    let tier = r.tier("a.rs::go", "a.rs::MyErr.from");
    assert!(
        tier.is_some_and(|millis| millis < 900),
        "`?` into Result<_, MyErr> is a below-HIGH candidate call to MyErr::from, got {tier:?}: {:?}",
        r.tiers
    );
    // A same-file function whose own error type is `MyErr` is the identity.
    assert!(
        !r.has("a.rs::identity", "a.rs::MyErr.from"),
        "{:?}",
        r.edges
    );
    // A closure returns its own type, not the function's.
    assert!(
        !r.has("a.rs::in_closure", "a.rs::MyErr.from"),
        "{:?}",
        r.edges
    );
    // `?` on an Option converts nothing.
    assert!(
        !r.has("a.rs::optional", "a.rs::MyErr.from"),
        "{:?}",
        r.edges
    );
    assert!(
        r.class_of("a.rs::identity", "from").is_empty(),
        "a candidate that binds nothing leaves no row: {:?}",
        r.classes
    );
}

#[test]
fn a_blanket_impl_answers_a_receiver_whose_type_declares_no_such_method() {
    let src = "\
use std::fmt::Display;
pub trait Describe { fn describe(&self) -> String; }
impl<T: Display> Describe for T { fn describe(&self) -> String { format!(\"<{}>\", self) } }
pub struct Own;
impl Own { pub fn describe(&self) -> String { String::new() } }
pub struct Plain;
impl std::fmt::Display for Plain { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { Ok(()) } }
pub fn literal() -> String { 5.describe() }
pub fn typed(p: Plain) -> String { p.describe() }
pub fn own(o: Own) -> String { o.describe() }
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::literal", "a.rs::T.describe"), "{:?}", r.edges);
    assert!(r.has("a.rs::typed", "a.rs::T.describe"), "{:?}", r.edges);
    // A type that declares the method itself answers before any blanket impl.
    assert!(r.has("a.rs::own", "a.rs::Own.describe"), "{:?}", r.edges);
    assert!(!r.has("a.rs::own", "a.rs::T.describe"), "{:?}", r.edges);

    let two = "\
pub trait A { fn shared(&self) -> u8; }
pub trait B { fn shared(&self) -> u8; }
impl<T: Copy> A for T { fn shared(&self) -> u8 { 1 } }
impl<U: Clone> B for U { fn shared(&self) -> u8 { 2 } }
pub fn go() -> u8 { 5.shared() }
";
    let r = resolve(&[("a.rs", two)]);
    assert!(
        !r.edges
            .iter()
            .any(|(from, to, _)| from == "a.rs::go" && to.ends_with(".shared")),
        "two blanket impls answer: {:?}",
        r.edges
    );
}

#[test]
fn a_receiver_whose_known_type_declares_no_such_member_is_an_unresolved_member() {
    let src = "\
pub struct Known;
impl Known { pub fn present(&self) {} }
pub struct Other;
impl Other { pub fn absent(&self) {} }
pub fn go(k: Known) { k.absent(); }
";
    let r = resolve(&[("a.rs", src)]);
    assert_eq!(
        r.class_of("a.rs::go", "absent"),
        vec!["unresolved_member"],
        "{:?}",
        r.classes
    );
}

#[test]
fn a_call_through_a_const_function_pointer_calls_the_function_it_names() {
    let src = "\
pub fn helper() -> u8 { 24 }
mod inner { pub fn nested() -> u8 { 1 } }
pub const FPTR: fn() -> u8 = helper;
pub static SPTR: fn() -> u8 = inner::nested;
pub const CLOSURE: fn() -> u8 = || 3;
pub fn go() -> u8 { FPTR() + SPTR() + CLOSURE() }
";
    let r = resolve(&[("a.rs", src)]);
    assert!(r.has("a.rs::go", "a.rs::FPTR"), "{:?}", r.edges);
    assert!(r.has("a.rs::go", "a.rs::helper"), "{:?}", r.edges);
    assert!(r.has("a.rs::go", "a.rs::nested"), "{:?}", r.edges);
}

#[test]
fn a_macro_rules_definition_is_a_macro_symbol_that_its_invocations_reference() {
    let src = "\
macro_rules! twice { ($e:expr) => { $e + $e }; }
pub fn log() -> u8 { 1 }
pub fn go() -> u8 { twice!(2) }
";
    let ext = extract_file("a.rs", src);
    let twice = ext
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "a.rs::twice")
        .unwrap_or_else(|| panic!("no macro symbol: {:?}", ext.symbols));
    assert_eq!(twice.kind, devmap_extract::model::SymbolKind::Macro);
    assert!(!twice.is_exported, "not `#[macro_export]`");

    let elsewhere = "pub fn other() -> u8 { twice!(3) }\n";
    let r = resolve(&[("a.rs", src), ("c.rs", elsewhere)]);
    assert!(r.has("a.rs::go", "a.rs::twice"), "{:?}", r.edges);
    // Pinned limit (DIVERGENCES): an invocation in a file that does not define
    // the macro is not linked, and leaves no ledger row either.
    assert!(!r.has("c.rs::other", "a.rs::twice"), "{:?}", r.edges);
    assert!(
        r.class_of("c.rs::other", "twice").is_empty(),
        "{:?}",
        r.classes
    );

    // The macro is no call target, and no namesake: `log()` is how a function
    // is called, so a macro `log` neither takes the call nor makes the one
    // function of that name ambiguous.
    let named_like_macro = "\
macro_rules! log { () => { 0 }; }
pub fn go() -> u8 { log() }
";
    let r = resolve(&[("m.rs", named_like_macro), ("a.rs", src)]);
    assert!(!r.has("m.rs::go", "m.rs::log"), "{:?}", r.edges);
    assert!(r.has("m.rs::go", "a.rs::log"), "{:?}", r.edges);
}
