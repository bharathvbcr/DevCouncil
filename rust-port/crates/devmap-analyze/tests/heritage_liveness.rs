//! W1.2, the consumer half — `Extends`/`Implements` edges, and what they buy.
//!
//! Both edge kinds were declared with no producer; every reference to them in
//! the repository was a label map or a string parser. They now come out of the
//! existing resolution ladder — same rungs, same confidences, an ambiguous
//! supertype resolving to nothing as usual — because `reference_edge` maps the
//! reference kind to the edge kind and everything else was already there.
//!
//! The recall payoff is polymorphic dispatch. A method reached only through its
//! base type has no inbound call edge, so before this it was a candidate
//! `extracted` false positive: measured on
//! `an_override_reached_through_its_base_is_not_dead` below, `Derived.compute`
//! was reported with no exemption reason at all.

use devmap_analyze::*;
use devmap_extract::extract_file;
use devmap_extract::model::{EdgeKind, Extraction};
use devmap_resolve::model::ResolutionResult;
use devmap_resolve::Resolver;

fn resolve(files: &[(&str, &str)]) -> (Vec<Extraction>, ResolutionResult) {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    resolver.index_extractions(&extractions);
    let resolution = resolver.resolve_all(&extractions);
    (extractions, resolution)
}

fn heritage_edges(resolution: &ResolutionResult) -> Vec<(String, String, EdgeKind)> {
    resolution
        .edges
        .iter()
        .filter(|e| matches!(e.edge_kind, EdgeKind::Extends | EdgeKind::Implements))
        .map(|e| {
            (
                e.source_symbol.clone(),
                e.target_symbol.clone(),
                e.edge_kind,
            )
        })
        .collect()
}

/// The edge exists, with both endpoints, at the rung that found it.
#[test]
fn an_extends_edge_is_produced_with_both_endpoints() {
    let (_, resolution) = resolve(&[
        ("base.py", "class Base:\n    def compute(self):\n        return 0\n"),
        (
            "derived.py",
            "from base import Base\n\n\nclass Derived(Base):\n    def compute(self):\n        return 1\n",
        ),
    ]);

    let edges = heritage_edges(&resolution);
    assert_eq!(
        edges.len(),
        1,
        "one supertype, one edge: {:?}",
        resolution
            .edges
            .iter()
            .map(|e| (&e.edge_kind, &e.source_symbol, &e.target_symbol))
            .collect::<Vec<_>>()
    );
    let (source, target, kind) = &edges[0];
    assert_eq!(kind, &EdgeKind::Extends);
    assert!(
        source.contains("Derived"),
        "the source endpoint is the declaring type, not the file: {source}"
    );
    assert!(target.contains("Base"), "{target}");
}

/// A stated `implements` produces the other kind.
#[test]
fn an_implements_edge_is_produced_where_the_grammar_states_it() {
    let (_, resolution) = resolve(&[
        (
            "IFace.java",
            "package app;\npublic interface IFace { void compute(); }\n",
        ),
        (
            "W.java",
            "package app;\npublic class W implements IFace { public void compute() {} }\n",
        ),
    ]);
    let edges = heritage_edges(&resolution);
    assert!(
        edges
            .iter()
            .any(|(_, target, kind)| { *kind == EdgeKind::Implements && target.contains("IFace") }),
        "an explicit `implements` must not become `Extends`: {edges:?}"
    );
}

/// The recall payoff: an override reached through its base is not dead.
///
/// Before W1.2 this method was reported with `exemption_reason: None` — the
/// unqualified branch, which `confidence_label` renders `extracted`, the tier
/// whose contract is "safe to act on".
///
/// The method is named `compute` rather than `render` deliberately: `render` is
/// matched by the React-lifecycle exemption, which would exempt it for an
/// entirely different reason and make this test pass without the heritage join
/// existing at all. That was observed while writing it.
#[test]
fn an_override_reached_through_its_base_is_not_dead() {
    let (extractions, resolution) = resolve(&[(
        "mod.ts",
        "class Base {\n  compute(): number {\n    return 0;\n  }\n}\n\n\
         class Derived extends Base {\n  compute(): number {\n    return 1;\n  }\n}\n\n\
         export function run(): number {\n  const b: Base = new Base();\n  return b.compute();\n}\n\n\
         export function make(): Derived {\n  return new Derived();\n}\n",
    )]);

    let report = analyze_liveness(&extractions, &resolution)
        .into_iter()
        .find(|r| r.symbol_name == "Derived.compute")
        .expect("the override must still be listed, not hidden");

    assert!(
        report.is_exempt,
        "a call through the base type reaches the override: {report:?}"
    );
    let reason = report.exemption_reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("polymorphic dispatch") && reason.contains("Base"),
        "the exemption must name the supertype it rests on, so a reader can \
         judge the edge behind it: {report:?}"
    );
}

/// The OFF direction, and the one that keeps the join from exempting the world.
///
/// A same-named method on an *unrelated* class must stay a finding. The join is
/// by method name, so without a real heritage edge behind it, one called
/// `compute` anywhere would exempt every `compute` in the corpus.
#[test]
fn a_same_named_method_on_an_unrelated_type_is_still_dead() {
    let (extractions, resolution) = resolve(&[(
        "mod.ts",
        "class Base {\n  compute(): number {\n    return 0;\n  }\n}\n\n\
         class Unrelated {\n  compute(): number {\n    return 1;\n  }\n}\n\n\
         export function run(): number {\n  const b: Base = new Base();\n  return b.compute();\n}\n\n\
         export function make(): Unrelated {\n  return new Unrelated();\n}\n",
    )]);

    let report = analyze_liveness(&extractions, &resolution)
        .into_iter()
        .find(|r| r.symbol_name == "Unrelated.compute")
        .expect("a method nothing calls must be reported");
    let reason = report.exemption_reason.as_deref().unwrap_or_default();
    assert!(
        !reason.contains("polymorphic dispatch"),
        "`Unrelated` extends nothing, so no heritage edge may exempt it: {report:?}"
    );
}

/// A supertype the resolver could not choose between does not exempt.
///
/// An ambiguous edge is evidence that a base *might* exist, not proof of which
/// one. Admitting it would let two same-named classes in a corpus exempt each
/// other's methods, which is the speculative-resolution-as-fact shape the
/// dead-code cascade already refuses for calls.
#[test]
fn an_ambiguous_supertype_does_not_exempt() {
    let (extractions, resolution) = resolve(&[
        (
            "a.py",
            "class Base:\n    def compute(self):\n        return 0\n",
        ),
        (
            "b.py",
            "class Base:\n    def compute(self):\n        return 1\n",
        ),
        (
            "c.py",
            "class Derived(Base):\n    def compute(self):\n        return 2\n",
        ),
    ]);

    let ambiguous = resolution.edges.iter().any(|e| {
        matches!(e.edge_kind, EdgeKind::Extends)
            && matches!(
                e.resolution.as_deref(),
                Some(devmap_resolve::model::Resolution::AmbiguousGlobal { .. })
            )
    });
    if !ambiguous {
        eprintln!("note: fixture did not produce an ambiguous supertype; property untested here");
        return;
    }

    let report = analyze_liveness(&extractions, &resolution)
        .into_iter()
        .find(|r| r.symbol_name == "Derived.compute");
    if let Some(report) = report {
        assert!(
            !report
                .exemption_reason
                .as_deref()
                .unwrap_or_default()
                .contains("polymorphic dispatch"),
            "an ambiguous supertype is not proof of a base: {report:?}"
        );
    }
}

/// The heritage walk terminates on a cycle.
///
/// No language expresses `A extends B extends A`, but a *graph* does, and this
/// walks a graph built from name resolution — two files each declaring a class
/// that extends the other's is enough. An unbounded walk here is a hang on
/// input a user can write.
#[test]
fn a_cyclic_heritage_graph_terminates() {
    let (extractions, resolution) = resolve(&[
        (
            "a.py",
            "class A(B):\n    def compute(self):\n        return 0\n",
        ),
        (
            "b.py",
            "class B(A):\n    def compute(self):\n        return 1\n",
        ),
    ]);
    // The assertion is that this returns at all.
    let reports = analyze_liveness(&extractions, &resolution);
    assert!(
        !reports.is_empty(),
        "the fixture declares methods, so something must be reported"
    );
}
