//! X46 — `Self` in type position names the type the item is written inside.
//!
//! `fn with_god_nodes(self, …) -> Self` is a return type, and the type it
//! returns is written at the top of the `impl` block. The resolver had no rung
//! for that: `Self` is declared by no file, matched no import and matched no
//! symbol, so every one of them fell to `UnresolvedClass::Unresolved` — the
//! tier documented as "the only tier that indicates a defect". Measured on this
//! repository before this test existed, `Self` was the **largest single name in
//! that tier at 98 rows**, ahead of `Path` (45) and `TSSymbol` (27).
//!
//! The evidence is the same evidence X42 gave the *call* ladder for `self.m()`:
//! the enclosing type, read from `symbol_parents`, which is the extractor's own
//! answer for "what declares this". Once `Self` is read as that type's name,
//! the ordinary rungs answer — so the rung that fires is `SameFile` where the
//! type is declared beside its `impl`, and no new `ResolutionKind` is needed.
//!
//! The abstention is `trait`. In `trait Defaulted { fn blank() -> Self; }`,
//! `Self` is the *implementor* — a type this resolver cannot name, and one that
//! is certainly not `Defaulted`. Resolving it to the trait would emit a
//! DETERMINISTIC edge asserting a return type the code does not have. The
//! asymmetry with X42 is deliberate and is the difference between the two
//! questions: `Self::blank()` inside that same trait names a method the trait
//! really does declare, and X42 binds it there.

use devmap_extract::extract_file;
use devmap_extract::model::{Confidence, Extraction};
use devmap_resolve::model::{ResolutionKind, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions)
}

fn edges_from<'a>(
    result: &'a ResolutionResult,
    source_symbol: &str,
    target_name: &str,
) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_symbol == source_symbol
                && edge
                    .target_symbol
                    .rsplit(['.', ':'])
                    .next()
                    .is_some_and(|tail| tail == target_name)
        })
        .collect()
}

fn kind_of(edge: &ResolvedEdge) -> ResolutionKind {
    edge.resolution
        .as_deref()
        .map(devmap_resolve::model::Resolution::kind)
        .expect("an edge the resolver built carries its own evidence")
}

/// Whether `name` is still in the tier reserved for probable defects.
fn is_a_defect_row(result: &ResolutionResult, source_symbol: &str, name: &str) -> bool {
    result.unresolved.iter().any(|row| {
        row.source_symbol == source_symbol
            && row.callee_name == name
            && row.class.label() == "unresolved"
    })
}

const INHERENT: &str = r#"
pub struct Widget { pub n: usize }

impl Widget {
    pub fn new() -> Self { Widget { n: 0 } }
}
"#;

#[test]
fn self_in_an_inherent_impl_is_the_type_the_impl_is_for() {
    let result = resolve(&[("widget.rs", INHERENT)]);

    let edges = edges_from(&result, "widget.rs::Widget.new", "Widget");
    assert_eq!(
        edges.len(),
        1,
        "`-> Self` inside `impl Widget` returns a `Widget`, and the impl block \
         says so — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert_eq!(edges[0].target_file, "widget.rs");
    assert_eq!(
        kind_of(edges[0]),
        ResolutionKind::SameFile,
        "reading `Self` as the enclosing type hands the ordinary rungs a name \
         they can answer; it is not a rung of its own"
    );
    assert_eq!(edges[0].confidence, Confidence::DETERMINISTIC);
    assert!(
        !is_a_defect_row(&result, "widget.rs::Widget.new", "Self"),
        "a name the impl block states outright is not a probable defect"
    );
}

/// `impl Render for Widget` — `Self` is `Widget`, the implementor, never
/// `Render`. Getting this backwards would point every constructor at the trait.
#[test]
fn self_in_a_trait_impl_is_the_implementor_not_the_trait() {
    const TRAIT_IMPL: &str = r#"
pub struct Widget { pub n: usize }
pub trait Render { fn render(&self) -> Self; }

impl Render for Widget {
    fn render(&self) -> Self { Widget { n: self.n } }
}
"#;
    let result = resolve(&[("widget.rs", TRAIT_IMPL)]);

    let to_widget = edges_from(&result, "widget.rs::Widget.render", "Widget");
    assert_eq!(
        to_widget.len(),
        1,
        "`impl Render for Widget` makes `Self` the `Widget` — got {:?}",
        to_widget
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    let to_trait = edges_from(&result, "widget.rs::Widget.render", "Render");
    assert!(
        to_trait.is_empty(),
        "`Self` is the implementor; naming the trait would assert a return type \
         the code does not have — got {to_trait:?}"
    );
}

/// A generic impl. `Self` is `Generic<T>`, whose *name* is `Generic` — which is
/// the key every index here is built on.
#[test]
fn self_in_a_generic_impl_is_the_generic_type() {
    const GENERIC: &str = r#"
pub struct Generic<T> { pub v: T }

impl<T: Clone> Generic<T> {
    pub fn dup(&self) -> Self { Generic { v: self.v.clone() } }
}
"#;
    let result = resolve(&[("generic.rs", GENERIC)]);

    let edges = edges_from(&result, "generic.rs::Generic.dup", "Generic");
    assert_eq!(
        edges.len(),
        1,
        "`impl<T> Generic<T>` makes `Self` a `Generic` — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert_eq!(kind_of(edges[0]), ResolutionKind::SameFile);
}

/// The abstention. Inside a `trait`, `Self` is whatever type implements it —
/// not the trait, and not anything this resolver can name.
#[test]
fn self_inside_a_trait_declaration_is_the_implementor_so_the_rung_abstains() {
    const TRAIT_ONLY: &str = r#"
pub trait Defaulted {
    fn blank() -> Self;
    fn twice(&self) -> Self where Self: Sized { Self::blank() }
}
"#;
    let result = resolve(&[("defaulted.rs", TRAIT_ONLY)]);

    let edges = edges_from(&result, "defaulted.rs::Defaulted.twice", "Defaulted");
    assert!(
        edges
            .iter()
            .all(|edge| edge.details.as_deref() != Some("Type")),
        "`-> Self` in a trait item is the implementor; a DETERMINISTIC edge \
         naming the trait asserts a return type no implementation has — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, &edge.details, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}

/// An enum is as concrete as a struct, and `Self` inside its impl is it.
#[test]
fn self_in_an_enum_impl_is_the_enum() {
    const ENUM: &str = r#"
pub enum Mode { Fast, Slow }

impl Mode {
    pub fn flip(&self) -> Self { Mode::Fast }
}
"#;
    let result = resolve(&[("mode.rs", ENUM)]);

    let edges = edges_from(&result, "mode.rs::Mode.flip", "Mode");
    assert!(
        edges
            .iter()
            .any(|edge| edge.details.as_deref() == Some("Type")),
        "`-> Self` inside `impl Mode` returns a `Mode` — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_symbol, &edge.details, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}

/// `Self` written at file level — a `type Alias = Self;` outside any item, or a
/// trait method signature the extractor could not scope — has no enclosing type
/// and must resolve to nothing rather than to whatever the file declares first.
#[test]
fn self_with_no_enclosing_type_resolves_to_nothing() {
    const NO_SCOPE: &str = r#"
pub struct Widget { pub n: usize }
pub trait Render { fn render(&self) -> Self; }
"#;
    let result = resolve(&[("widget.rs", NO_SCOPE)]);

    let stray: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.source_symbol == "widget.rs" && edge.details.as_deref() == Some("Type"))
        .collect();
    assert!(
        stray.is_empty(),
        "a `Self` the extractor could not put inside an item names no type — \
         got {stray:?}"
    );
}

/// The crash. `impl Widget` in a file that does not declare `Widget` — a
/// perfectly ordinary Rust module split — asked `symbol_kind_in` about a name
/// the symbol index holds for *another* file. That function tested
/// `(file_hits.len() == 1).then_some(file_hits[0])`, and `then_some` evaluates
/// its argument, so the index ran before the length test could guard it:
/// "index out of bounds: the len is 0 but the index is 0", aborting the whole
/// build. Every caller before X46 had already proved the name was in the file.
#[test]
fn an_impl_whose_type_lives_in_another_file_does_not_abort_the_build() {
    const DECLARE: &str = r#"
pub struct Widget { pub n: usize }
"#;
    const IMPL_ELSEWHERE: &str = r#"
use crate::widget::Widget;

impl Widget {
    pub fn new() -> Self { Widget { n: 0 } }
}
"#;
    let result = resolve(&[("src/widget.rs", DECLARE), ("src/build.rs", IMPL_ELSEWHERE)]);

    assert!(
        !result.edges.is_empty(),
        "the fixture must reach the ladder at all"
    );
}
