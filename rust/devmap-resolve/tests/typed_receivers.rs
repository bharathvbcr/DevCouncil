//! A method call on a receiver whose type the source states binds to the
//! method.
//!
//! Each case here was a 0.40 dead row on this repository: the method had a
//! real caller, the caller's receiver had a type written in the same file, and
//! the resolver filed the call as an uninferred receiver because nothing
//! connected the receiver to that type. The shapes differ — a Go `var`, a
//! value returned by an associated function, a closure parameter, a
//! `MutexGuard` — and so does where the type is written, which is why each
//! has its own case.

use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::{ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions).unwrap();
    (extractions, resolution)
}

fn calls_to<'a>(result: &'a ResolutionResult, target: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| edge.edge_kind == EdgeKind::Calls && edge.target_symbol == target)
        .collect()
}

const GO_REQUIREMENT: &str = "\
package dc

type Priority string

func (p Priority) valid() bool { return p != \"\" }

type Source string

func (s Source) valid() bool { return s != \"\" }

type Method string

func (m Method) valid() bool { return m != \"\" }

type requirementWire struct {
\tPriority Priority
\tSource   *Source
}

type acceptanceWire struct {
\tMethod Method
}

func decodeRequirement() bool {
\tvar w requirementWire
\tif !w.Priority.valid() {
\t\treturn false
\t}
\treturn (*w.Source).valid()
}

func decodeAcceptance() bool {
\tvar w acceptanceWire
\treturn w.Method.valid()
}
";

#[test]
fn a_go_var_types_the_fields_its_value_receiver_methods_are_called_on() {
    let (_, result) = resolve(&[("dc/requirement.go", GO_REQUIREMENT)]);

    for (target, caller) in [
        ("dc/requirement.go::Priority.valid", "dc/requirement.go::decodeRequirement"),
        ("dc/requirement.go::Source.valid", "dc/requirement.go::decodeRequirement"),
        ("dc/requirement.go::Method.valid", "dc/requirement.go::decodeAcceptance"),
    ] {
        let edges = calls_to(&result, target);
        assert_eq!(
            edges.len(),
            1,
            "`var w T` types `w`, so `w.Field.valid()` reaches {target}; got {edges:?}"
        );
        assert_eq!(edges[0].source_symbol, caller);
    }
}

const COLLECTED: &str = "\
pub struct Collected;

impl Collected {
    pub fn can_skip(&self) -> bool { true }
    pub fn merge(&mut self) {}
}
";

fn rust_calls(user: &str, method: &str) -> Vec<String> {
    // With a second `Collected` in the corpus, as a real workspace has: the
    // corpus-wide receiver map withdraws a type name two declarations share,
    // so only what the binding itself states can answer.
    let (_, result) = resolve(&[
        ("src/collected.rs", COLLECTED),
        ("src/user.rs", user),
        ("other/src/lib.rs", "pub struct Collected;\n"),
    ]);
    calls_to(&result, &format!("src/collected.rs::Collected.{method}"))
        .into_iter()
        .map(|edge| edge.source_symbol.clone())
        .collect()
}

#[test]
fn an_ok_arm_on_a_mutex_lock_is_a_guard_that_derefs_to_the_inner_type() {
    let user = "\
use crate::collected::Collected;
use std::sync::Mutex;

fn skippable(shared: &Mutex<Collected>) -> bool {
    match shared.lock() {
        Ok(guard) => guard.can_skip(),
        Err(poisoned) => poisoned.into_inner().can_skip(),
    }
}
";
    assert_eq!(rust_calls(user, "can_skip"), vec!["src/user.rs::skippable"]);
}

#[test]
fn an_unwrapped_lock_in_a_let_is_a_guard() {
    for (spelling, caller) in [
        ("let mut guard = shared.lock().unwrap();", "a"),
        ("let mut guard = shared.lock().expect(\"poisoned\");", "b"),
        ("let mut guard = shared.lock()?;", "c"),
    ] {
        let user = format!(
            "use crate::collected::Collected;\nuse std::sync::{{Arc, Mutex}};\n\n\
             fn {caller}(shared: Arc<Mutex<Collected>>) -> Result<(), E> {{\n    \
             {spelling}\n    guard.merge();\n    Ok(())\n}}\n"
        );
        assert_eq!(
            rust_calls(&user, "merge"),
            vec![format!("src/user.rs::{caller}")],
            "`{spelling}` over an `Arc<Mutex<Collected>>` is a guard over Collected"
        );
    }
}

#[test]
fn an_rwlock_read_in_if_let_is_a_guard() {
    let user = "\
use crate::collected::Collected;
use std::sync::RwLock;

fn peek(shared: &RwLock<Collected>) -> bool {
    if let Ok(view) = shared.read() {
        return view.can_skip();
    }
    false
}
";
    assert_eq!(rust_calls(user, "can_skip"), vec!["src/user.rs::peek"]);
}

#[test]
fn a_loop_variable_over_a_typed_collection_is_its_element() {
    for (param, iterable) in [
        ("items: Vec<Collected>", "items"),
        ("items: &[Collected]", "items"),
        ("items: &Vec<Collected>", "items.iter()"),
        ("items: Vec<Collected>", "&items"),
        ("items: [Collected; 4]", "items.into_iter()"),
    ] {
        let user = format!(
            "use crate::collected::Collected;\n\n\
             fn each({param}) {{\n    for item in {iterable} {{\n        item.can_skip();\n    }}\n}}\n"
        );
        assert_eq!(
            rust_calls(&user, "can_skip"),
            vec!["src/user.rs::each".to_string()],
            "`for item in {iterable}` with `{param}` binds each Collected"
        );
    }
}

#[test]
fn a_parameter_with_a_lifetime_keeps_its_type() {
    let user = "\
use crate::collected::Collected;

pub struct Graph<'a> {
    collected: &'a Collected,
}

impl<'a> Graph<'a> {
    pub fn new(collected: &'a Collected, repo: &Path) -> anyhow::Result<Self> {
        let skip = collected.can_skip();
        for item in collected.can_skip() {}
        Ok(Self { collected })
    }
}
";
    // A second `Collected` elsewhere, as a real workspace has: the corpus-wide
    // receiver map withdraws a type name two declarations share, so only the
    // binding's own declared type can answer here.
    let (_, result) = resolve(&[
        ("src/collected.rs", COLLECTED),
        ("src/user.rs", user),
        ("other/src/lib.rs", "pub struct Collected;\n"),
    ]);
    let callers: Vec<_> = calls_to(&result, "src/collected.rs::Collected.can_skip")
        .into_iter()
        .map(|edge| edge.source_symbol.clone())
        .collect();
    assert_eq!(
        callers,
        vec!["src/user.rs::Graph.new"],
        "`&'a Collected` is a Collected; the lifetime is not part of the name"
    );
}

const RANKS: &str = "\
pub struct PathRanks;

impl PathRanks {
    pub fn read(conn: &Connection) -> Result<Self> { todo!() }
    pub fn fresh() -> Self { PathRanks }
    pub fn other(conn: &Connection) -> Result<Other> { todo!() }
    pub fn rank_of(&self, id: i64) -> u32 { 0 }
}
";

fn rank_callers(user: &str) -> Vec<String> {
    let (_, result) = resolve(&[
        ("src/ranks.rs", RANKS),
        ("src/user.rs", user),
        ("other/src/lib.rs", "pub struct PathRanks;\n"),
    ]);
    calls_to(&result, "src/ranks.rs::PathRanks.rank_of")
        .into_iter()
        .map(|edge| edge.source_symbol.clone())
        .collect()
}

#[test]
fn a_binding_from_an_associated_function_has_its_declared_return_type() {
    for value in [
        "PathRanks::read(&snapshot)?",
        "PathRanks::read(&snapshot).unwrap()",
        "PathRanks::read(&snapshot).expect(\"ranks\")",
        "crate::ranks::PathRanks::read(&snapshot)?",
        "PathRanks::fresh()",
    ] {
        let user = format!(
            "use crate::ranks::PathRanks;\n\n\
             fn user(snapshot: &Connection) -> Result<()> {{\n    \
             let paths = {value};\n    paths.rank_of(1);\n    Ok(())\n}}\n"
        );
        assert_eq!(
            rank_callers(&user),
            vec!["src/user.rs::user"],
            "`let paths = {value}` is a PathRanks by `read`'s declared return type"
        );
    }
}

#[test]
fn a_result_that_was_not_unwrapped_is_not_its_ok_type() {
    for value in [
        "PathRanks::read(&snapshot)",
        "PathRanks::other(&snapshot)?",
        "PathRanks::missing(&snapshot)?",
    ] {
        let user = format!(
            "use crate::ranks::PathRanks;\n\n\
             fn user(snapshot: &Connection) {{\n    let paths = {value};\n    paths.rank_of(1);\n}}\n"
        );
        assert!(
            rank_callers(&user).is_empty(),
            "`let paths = {value}` is not a PathRanks"
        );
    }
}

const NOTE: &str = "\
use crate::collected::Collected;

pub fn note(shared: &Mutex<Collected>, tally: impl FnOnce(&mut Collected)) {}
pub fn note_generic<F>(shared: &Mutex<Collected>, tally: F) where F: FnOnce(&mut Collected) {}
pub fn not_a_closure(shared: &Mutex<Collected>, tally: Collected) {}
";

#[test]
fn a_closure_parameter_has_the_type_its_callee_declares() {
    for (import, call) in [
        ("use crate::note::note;", "note(&shared, |c| { c.merge(); });"),
        ("use crate::note::note_generic;", "note_generic(&shared, |c| c.merge());"),
    ] {
        let user = format!(
            "{import}\n\nfn walk(shared: Mutex<Collected>) {{\n    {call}\n}}\n"
        );
        let (_, result) = resolve(&[
            ("src/collected.rs", COLLECTED),
            ("src/note.rs", NOTE),
            ("src/user.rs", &user),
            ("other/src/lib.rs", "pub struct Collected;\n"),
        ]);
        let callers: Vec<_> = calls_to(&result, "src/collected.rs::Collected.merge")
            .into_iter()
            .map(|edge| edge.source_symbol.clone())
            .collect();
        assert_eq!(callers, vec!["src/user.rs::walk"], "{call}");
    }

    // The callee in the same file, as `dc-grep`'s `note` is.
    let same_file = format!(
        "{}\nfn walk(shared: Mutex<Collected>) {{\n    note(&shared, |c| c.merge());\n}}\n",
        NOTE
    );
    let (_, result) = resolve(&[
        ("src/collected.rs", COLLECTED),
        ("src/note.rs", &same_file),
        ("other/src/lib.rs", "pub struct Collected;\n"),
    ]);
    assert_eq!(
        calls_to(&result, "src/collected.rs::Collected.merge").len(),
        1,
        "a callee declared beside the call types its closure too"
    );
}

#[test]
fn a_closure_is_not_typed_by_a_callee_this_file_does_not_bind() {
    for (import, call) in [
        // Not a closure parameter type.
        ("use crate::note::not_a_closure;", "not_a_closure(&shared, |c| c.merge());"),
        // Never imported: a corpus-wide match on the name is not evidence.
        ("", "note(&shared, |c| c.merge());"),
        // A method call: its receiver would have to be typed first.
        ("use crate::note::note;", "shared.note(|c| c.merge());"),
    ] {
        let user = format!(
            "{import}\n\nfn walk(shared: Mutex<Collected>) {{\n    {call}\n}}\n"
        );
        let (_, result) = resolve(&[
            ("src/collected.rs", COLLECTED),
            ("src/note.rs", NOTE),
            ("src/user.rs", &user),
            ("other/src/lib.rs", "pub struct Collected;\n"),
        ]);
        assert!(
            calls_to(&result, "src/collected.rs::Collected.merge").is_empty(),
            "{call}"
        );
    }
}

#[test]
fn a_mutex_that_is_never_locked_has_no_inner_methods() {
    for body in [
        "shared.can_skip();",
        "let guard = shared.lock();\n    guard.can_skip();",
        "match shared.lock() {\n        Err(guard) => guard.can_skip(),\n        _ => false,\n    };",
        "let guard = shared.try_lock().unwrap();\n    guard.can_skip();",
    ] {
        let user = format!(
            "use crate::collected::Collected;\nuse std::sync::Mutex;\n\n\
             fn f(shared: &Mutex<Collected>) {{\n    {body}\n}}\n"
        );
        assert!(
            rust_calls(&user, "can_skip").is_empty(),
            "a `Mutex<T>` has no `T` methods until it is locked and unwrapped: {body}"
        );
    }
}

#[test]
fn a_loop_over_an_option_or_a_map_is_not_typed() {
    for param in [
        "items: Option<Collected>",
        "items: HashMap<String, Collected>",
        "items: Vec<Vec<Collected>>",
    ] {
        let user = format!(
            "use crate::collected::Collected;\n\n\
             fn each({param}) {{\n    for item in items {{\n        item.can_skip();\n    }}\n}}\n"
        );
        assert!(
            rust_calls(&user, "can_skip").is_empty(),
            "`for item in items` with `{param}` does not yield a Collected"
        );
    }
}

#[test]
fn a_closure_parameter_shadows_the_guard_it_names() {
    let user = "\
use crate::collected::Collected;
use std::sync::Mutex;

fn f(shared: &Mutex<Collected>, others: Vec<Other>) {
    let guard = shared.lock().unwrap();
    others.iter().for_each(|guard| guard.can_skip());
}
";
    assert!(
        rust_calls(user, "can_skip").is_empty(),
        "inside the closure `guard` is the closure's parameter, not the lock"
    );
}

#[test]
fn a_go_var_of_a_slice_does_not_type_its_name_as_the_element() {
    // Asked of the extraction, not the edge list: an unexported selector on an
    // untyped receiver has its own package-scope rung, which would answer
    // `xs.run()` whatever this declaration says.
    let (extractions, result) = resolve(&[(
        "dc/list.go",
        "package dc\n\ntype Item struct{}\n\nfunc (i Item) Run() {}\n\n\
         func use() {\n\tvar xs []Item\n\tvar p *Item\n\txs.Run()\n\tp.Run()\n}\n",
    )]);

    let bound: Vec<_> = extractions[0]
        .references
        .iter()
        .filter(|reference| {
            reference.name == "Item"
                && reference.assigned_to.is_some()
                && reference.enclosing_symbol.as_deref() == Some("dc/list.go::use")
        })
        .map(|reference| reference.assigned_to.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(
        bound,
        vec!["p"],
        "`var p *Item` types `p`; `var xs []Item` makes `xs` a slice, not an Item"
    );
    let edges = calls_to(&result, "dc/list.go::Item.Run");
    assert_eq!(edges.len(), 1, "only `p.Run()` is a call on an Item; got {edges:?}");
}

#[test]
fn a_go_var_declaring_two_names_binds_neither() {
    let (extractions, _) = resolve(&[(
        "dc/pair.go",
        "package dc\n\ntype Item struct{}\n\nfunc use() {\n\tvar a, b Item\n\t_ = a\n\t_ = b\n}\n",
    )]);

    let bound: Vec<_> = extractions[0]
        .references
        .iter()
        .filter(|reference| reference.name == "Item" && reference.assigned_to.is_some())
        .collect();
    assert!(
        bound.is_empty(),
        "one `assigned_to` cannot name both `a` and `b`; got {bound:?}"
    );
}
