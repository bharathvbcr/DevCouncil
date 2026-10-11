//! A Rust method call whose receiver is typed by a trait — `&dyn T`,
//! `Box<dyn T>`, `impl T`, a generic `X: T`, or `self` inside the trait's own
//! default body — dispatches through that trait. The one target the source
//! states with certainty is the trait's declaration of the method, never one of
//! its implementations: which impl runs is decided at run time (or at
//! monomorphisation), so binding an impl at confidence 1.0 would be a guess.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::Resolver;

struct Resolved {
    /// (source, target, resolution kind, confidence millis)
    edges: Vec<(String, String, String, i64)>,
    unresolved: Vec<(String, String)>,
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
        edges: result
            .edges
            .into_iter()
            .map(|edge| {
                let kind = edge
                    .resolution
                    .as_deref()
                    .map(|r| format!("{:?}", r.kind()))
                    .unwrap_or_default();
                (
                    edge.source_symbol,
                    edge.target_symbol,
                    kind,
                    edge.confidence.to_millis(),
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
    fn from(&self, source: &str) -> Vec<&(String, String, String, i64)> {
        self.edges.iter().filter(|e| e.0 == source).collect()
    }

    fn has(&self, source: &str, target: &str) -> bool {
        self.edges.iter().any(|e| e.0 == source && e.1 == target)
    }

    /// No HIGH edge from `source` reaches an implementation of the trait
    /// method. A speculative dispatch candidate below 0.9 may.
    fn binds_no_impl(&self, source: &str, impls: &[&str]) -> bool {
        !self
            .edges
            .iter()
            .any(|e| e.0 == source && impls.contains(&e.1.as_str()) && e.3 >= 900)
    }
}

const RENDER: &str = "pub trait Render {\n    fn render(&self) -> u8;\n}\n\
pub struct Sq;\npub struct Ci;\n\
impl Render for Sq {\n    fn render(&self) -> u8 { 1 }\n}\n\
impl Render for Ci {\n    fn render(&self) -> u8 { 2 }\n}\n";

const IMPLS: &[&str] = &["src/lib.rs::Sq.render", "src/lib.rs::Ci.render"];

#[test]
fn a_dyn_trait_parameter_binds_the_trait_declaration() {
    let src = format!("{RENDER}pub fn caller(s: &dyn Render) -> u8 {{\n    s.render()\n}}\n");
    let r = resolve(&[("src/lib.rs", &src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::Render.render"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::caller"),
        r.unresolved
    );
    assert!(r.binds_no_impl("src/lib.rs::caller", IMPLS), "{:?}", r.edges);
}

#[test]
fn a_boxed_dyn_local_binds_the_trait_declaration() {
    let src = format!(
        "{RENDER}pub fn caller() -> u8 {{\n    let o: Box<dyn Render> = Box::new(Sq);\n    o.render()\n}}\n"
    );
    let r = resolve(&[("src/lib.rs", &src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::Render.render"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::caller"),
        r.unresolved
    );
    assert!(r.binds_no_impl("src/lib.rs::caller", IMPLS), "{:?}", r.edges);
}

#[test]
fn self_in_a_trait_default_body_binds_the_trait_declaration() {
    let src = "pub trait Template {\n    fn tmpl(&self) -> u8 {\n        self.hook() + 1\n    }\n    fn hook(&self) -> u8;\n}\n\
pub struct Concrete;\nimpl Template for Concrete {\n    fn hook(&self) -> u8 { 27 }\n}\n";
    let r = resolve(&[("src/lib.rs", src)]);
    assert!(
        r.has("src/lib.rs::Template.tmpl", "src/lib.rs::Template.hook"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::Template.tmpl"),
        r.unresolved
    );
    assert!(
        r.binds_no_impl("src/lib.rs::Template.tmpl", &["src/lib.rs::Concrete.hook"]),
        "{:?}",
        r.edges
    );
}

#[test]
fn a_dyn_receiver_whose_trait_is_unknown_binds_nothing_certain() {
    // `Missing` is declared nowhere the resolver can see; a same-named method
    // on an unrelated struct must not answer for it.
    let src = "pub struct Other;\nimpl Other {\n    pub fn render(&self) -> u8 { 0 }\n}\n\
pub fn caller(s: &dyn Missing) -> u8 {\n    s.render()\n}\n";
    let r = resolve(&[("src/lib.rs", src)]);
    assert!(
        !r.from("src/lib.rs::caller")
            .iter()
            .any(|e| e.1 == "src/lib.rs::Other.render" && e.3 >= 900),
        "{:?}",
        r.edges
    );
}

#[test]
fn a_multi_bound_dyn_receiver_is_not_typed_by_one_of_its_traits() {
    // `dyn A + B` names two traits; the receiver is not "an A".
    let src = "pub trait A { fn go(&self); }\npub trait B { fn go(&self); }\n\
pub fn caller(s: &(dyn A + B)) {\n    s.go()\n}\n";
    let r = resolve(&[("src/lib.rs", src)]);
    assert!(
        !r.from("src/lib.rs::caller")
            .iter()
            .any(|e| (e.1 == "src/lib.rs::A.go" || e.1 == "src/lib.rs::B.go") && e.3 >= 900),
        "{:?}",
        r.edges
    );
}

#[test]
fn a_local_bound_to_an_impl_trait_return_binds_the_trait_declaration() {
    let src = format!(
        "{RENDER}pub fn make() -> impl Render {{\n    Sq\n}}\n\
pub fn caller() -> u8 {{\n    let r = make();\n    r.render()\n}}\n"
    );
    let r = resolve(&[("src/lib.rs", &src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::Render.render"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::caller"),
        r.unresolved
    );
    assert!(r.binds_no_impl("src/lib.rs::caller", IMPLS), "{:?}", r.edges);
}

#[test]
fn an_impl_trait_return_whose_name_is_also_a_struct_is_not_typed() {
    // `Render` is a trait here and a struct in another file: the bare name
    // `type_methods` keys on cannot tell them apart, so nothing binds HIGH.
    let src = format!(
        "{RENDER}pub fn make() -> impl Render {{\n    Sq\n}}\n\
pub fn caller() -> u8 {{\n    let r = make();\n    r.render()\n}}\n"
    );
    let other = "pub struct Render;\nimpl Render {\n    pub fn render(&self) -> u8 { 9 }\n}\n";
    let r = resolve(&[("src/lib.rs", &src), ("other/src/lib.rs", other)]);
    assert!(
        !r.from("src/lib.rs::caller")
            .iter()
            .any(|e| e.1 == "other/src/lib.rs::Render.render" && e.3 >= 900),
        "{:?}",
        r.edges
    );
}

#[test]
fn a_generic_bounded_by_one_trait_binds_the_trait_declaration() {
    let src = format!("{RENDER}pub fn caller<T: Render>(t: &T) -> u8 {{\n    t.render()\n}}\n");
    let r = resolve(&[("src/lib.rs", &src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::Render.render"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::caller"),
        r.unresolved
    );
    assert!(r.binds_no_impl("src/lib.rs::caller", IMPLS), "{:?}", r.edges);
}

#[test]
fn a_where_clause_bound_binds_the_trait_declaration() {
    let src = format!(
        "{RENDER}pub fn caller<T>(t: &T) -> u8\nwhere\n    T: Render + Send,\n{{\n    t.render()\n}}\n"
    );
    let r = resolve(&[("src/lib.rs", &src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::Render.render"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::caller"),
        r.unresolved
    );
}

#[test]
fn a_generic_shadows_a_namesake_struct_and_two_bounds_bind_nothing() {
    // A struct called `T` exists, but inside `caller` the name is the generic
    // parameter, bounded by two traits that both declare `render`.
    let src = "pub trait A { fn render(&self) -> u8; }\npub trait B { fn render(&self) -> u8; }\n\
pub struct T;\nimpl T {\n    pub fn render(&self) -> u8 { 0 }\n}\n\
pub fn caller<T: A + B>(t: &T) -> u8 {\n    t.render()\n}\n";
    let r = resolve(&[("src/lib.rs", src)]);
    assert!(
        !r.from("src/lib.rs::caller").iter().any(|e| e.3 >= 900
            && ["src/lib.rs::T.render", "src/lib.rs::A.render", "src/lib.rs::B.render"]
                .contains(&e.1.as_str())),
        "{:?}",
        r.edges
    );
}

#[test]
fn an_associated_fn_through_a_bounded_generic_binds_the_trait_declaration() {
    let src = "pub trait Factory {\n    fn create() -> Self;\n}\npub struct Dog;\n\
impl Factory for Dog {\n    fn create() -> Self { Dog }\n}\n\
pub fn caller<T: Factory>() -> T {\n    T::create()\n}\n";
    let r = resolve(&[("src/lib.rs", src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::Factory.create"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::caller"),
        r.unresolved
    );
    assert!(
        r.binds_no_impl("src/lib.rs::caller", &["src/lib.rs::Dog.create"]),
        "{:?}",
        r.edges
    );
}

const TEMPLATE: &str = "pub trait Template {\n    fn tmpl(&self) -> u8 {\n        1\n    }\n}\n\
pub struct Concrete;\nimpl Template for Concrete {}\n";

#[test]
fn a_trait_default_method_the_impl_does_not_write_is_the_trait_body() {
    let src = format!("{TEMPLATE}pub fn caller(c: &Concrete) -> u8 {{\n    c.tmpl()\n}}\n");
    let r = resolve(&[("src/lib.rs", &src)]);
    assert!(
        r.has("src/lib.rs::caller", "src/lib.rs::Template.tmpl"),
        "{:?} unresolved={:?}",
        r.from("src/lib.rs::caller"),
        r.unresolved
    );
}

#[test]
fn two_implemented_traits_with_the_same_default_bind_neither() {
    // `c.tmpl()` here does not compile without disambiguation; the resolver
    // has no rule to pick one and must not.
    let src = format!(
        "{TEMPLATE}pub trait Other {{\n    fn tmpl(&self) -> u8 {{\n        2\n    }}\n}}\n\
impl Other for Concrete {{}}\npub fn caller(c: &Concrete) -> u8 {{\n    c.tmpl()\n}}\n"
    );
    let r = resolve(&[("src/lib.rs", &src)]);
    assert!(
        !r.from("src/lib.rs::caller").iter().any(|e| e.3 >= 900
            && (e.1 == "src/lib.rs::Template.tmpl" || e.1 == "src/lib.rs::Other.tmpl")),
        "{:?}",
        r.edges
    );
}
