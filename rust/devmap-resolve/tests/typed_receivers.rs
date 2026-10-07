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
