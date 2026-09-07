//! X45 — a Go package spans its files, and the resolver now knows it.
//!
//! Go's spec puts every package-level identifier in the *package* block: a name
//! declared in `search/provider.go` is in scope, unqualified, in every other
//! file of `search/`. The resolver had no rung for that. What answered instead
//! was the global tier, which matches a bare name across the whole language
//! family — so the deterministic fact "this name is declared in this package"
//! arrived as `UniqueGlobal` (HIGH) when the family happened to hold one
//! declaration, and as `AmbiguousGlobal` (SPECULATIVE, fanned out to every
//! candidate) when it held more.
//!
//! Measured on the scholarlm corpus (4,289 files) before this rung existed:
//!
//! - **1,770** rows of the `unresolved` defect tier were a Go type reference to
//!   a name declared in another file of the *same package* — led by `Paper`
//!   (455 in `internal/search`), `Hypothesis` (449 in `internal/wisdev`) and
//!   `AgentSession` (338 in `internal/wisdev`). Every one of them is a name Go
//!   itself resolves without ambiguity.
//! - **31,587** same-directory cross-file Go edges were `UniqueGlobal` and
//!   **24,635** were `AmbiguousGlobal` — a package-scope fact answered from a
//!   name list, at two confidences neither of which is the one the evidence
//!   supports.
//!
//! The rung is `ResolutionKind::SamePackage`, at `DETERMINISTIC`, and it is a
//! new rung rather than a reuse because none of the six fits: the declaration
//! is not in this file (`SameFile`), no import names it (`ImportScoped`), no
//! receiver was typed (`ReceiverType`), and the global tiers are defined by
//! counting matches across the family rather than by a scope rule.
//!
//! Two packages can share a directory — `package foo` and its external test
//! package `foo_test` — so the rung keys on **(directory, package clause)** and
//! not on the directory alone. And a package-level name is what a bare
//! identifier can reach, so a method declared on a type is not a candidate.

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

/// Every edge out of `source_symbol` whose target is named `target_name`.
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

/// `search/provider.go` declares the package's `Paper`; `search/rank.go` uses
/// it unqualified, which is what Go's package block means.
const SEARCH_PROVIDER: &str = "\
package search

type Paper struct {
\tTitle string
}

func Trim(raw string) string { return raw }
";

const SEARCH_RANK: &str = "\
package search

func score(paper Paper) int {
\treturn len(paper.Title)
}

func normalise(raw string) string {
\treturn Trim(raw)
}
";

/// A second package declaring the very same type name, so the global tier
/// cannot answer: `Paper` matches twice across the Go family.
const PAPERGRAPH: &str = "\
package papergraph

type Paper struct {
\tID string
}

func Trim(raw string) string { return raw }
";

#[test]
fn a_type_declared_by_a_sibling_file_of_the_package_resolves_deterministically() {
    let result = resolve(&[
        ("internal/search/provider.go", SEARCH_PROVIDER),
        ("internal/search/rank.go", SEARCH_RANK),
        ("internal/papergraph/types.go", PAPERGRAPH),
    ]);

    let edges = edges_from(&result, "internal/search/rank.go::score", "Paper");
    assert_eq!(
        edges.len(),
        1,
        "`paper Paper` in rank.go names the package's own `Paper`, and exactly \
         that one — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_file, &edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert_eq!(edges[0].target_file, "internal/search/provider.go");
    assert_eq!(
        kind_of(edges[0]),
        ResolutionKind::SamePackage,
        "the evidence is Go's package block, not a count of family-wide matches"
    );
    assert_eq!(edges[0].confidence, Confidence::DETERMINISTIC);
}

#[test]
fn a_package_level_function_of_a_sibling_file_is_callable_unqualified() {
    let result = resolve(&[
        ("internal/search/provider.go", SEARCH_PROVIDER),
        ("internal/search/rank.go", SEARCH_RANK),
        ("internal/papergraph/types.go", PAPERGRAPH),
    ]);

    let edges = edges_from(&result, "internal/search/rank.go::normalise", "Trim");
    assert_eq!(
        edges.len(),
        1,
        "`Trim(raw)` is the package's own `Trim`; the second package's `Trim` is \
         not in scope here — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_file, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert_eq!(edges[0].target_file, "internal/search/provider.go");
    assert_eq!(kind_of(edges[0]), ResolutionKind::SamePackage);
}

/// The other direction, and the one that makes the rung a scope rule rather
/// than a preference: the same bare name written inside `papergraph` must reach
/// `papergraph`'s declaration and nothing in `search`.
#[test]
fn the_rung_is_a_scope_rule_so_each_package_reaches_only_its_own() {
    const PAPERGRAPH_USE: &str = "\
package papergraph

func identity(paper Paper) string {
\treturn paper.ID
}
";
    let result = resolve(&[
        ("internal/search/provider.go", SEARCH_PROVIDER),
        ("internal/papergraph/types.go", PAPERGRAPH),
        ("internal/papergraph/identity.go", PAPERGRAPH_USE),
    ]);

    let edges = edges_from(
        &result,
        "internal/papergraph/identity.go::identity",
        "Paper",
    );
    assert_eq!(edges.len(), 1, "one package, one declaration, one edge");
    assert_eq!(
        edges[0].target_file, "internal/papergraph/types.go",
        "a bare `Paper` inside papergraph cannot mean search's `Paper`"
    );
    assert_eq!(kind_of(edges[0]), ResolutionKind::SamePackage);
}

/// `search.Paper` from another package. The global tier abstains — two packages
/// declare `Paper` — and the qualifier is the only evidence that says which. It
/// is written down in a `TypeQualifier` sibling reference the extractor already
/// emits, and the file's own import statement binds it.
#[test]
fn a_qualified_type_from_another_package_resolves_through_the_files_import() {
    const API: &str = "\
package api

import (
\t\"example.com/app/internal/search\"
)

func identity(paper search.Paper) string {
\treturn paper.Title
}
";
    const GOMOD: &str = "module example.com/app\n\ngo 1.22\n";
    let extractions: Vec<Extraction> = [
        ("internal/search/provider.go", SEARCH_PROVIDER),
        ("internal/papergraph/types.go", PAPERGRAPH),
        ("internal/api/gateway.go", API),
    ]
    .iter()
    .map(|(path, source)| extract_file(path, source))
    .collect();
    let mut resolver = Resolver::new();
    let module = devmap_extract::parse_go_mod("go.mod", GOMOD).expect("go.mod parses");
    resolver.index_go_modules(std::slice::from_ref(&module));
    resolver.index_extractions(&extractions);
    let result = resolver.resolve_all(&extractions);

    let edges = edges_from(&result, "internal/api/gateway.go::identity", "Paper");
    assert_eq!(
        edges.len(),
        1,
        "`search.Paper` names one type: the `Paper` of the package the import \
         binds — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_file, kind_of(edge)))
            .collect::<Vec<_>>()
    );
    assert_eq!(edges[0].target_file, "internal/search/provider.go");
    assert_eq!(
        kind_of(edges[0]),
        ResolutionKind::ImportScoped,
        "the import statement is the evidence, so this is the import rung and \
         not a new one"
    );
}

/// An external test package (`package foo_test`) shares the directory with the
/// package it tests and is **not** in its package block: it reaches the tested
/// package only through an import, and never its unexported names. Keying the
/// rung on the directory alone would hand it the whole package for free.
#[test]
fn an_external_test_package_sharing_the_directory_is_not_the_same_package() {
    const EXTERNAL_TEST: &str = "\
package search_test

func check(paper Paper) int {
\treturn len(paper.Title)
}
";
    let result = resolve(&[
        ("internal/search/provider.go", SEARCH_PROVIDER),
        ("internal/search/external_test.go", EXTERNAL_TEST),
        ("internal/papergraph/types.go", PAPERGRAPH),
    ]);

    let edges = edges_from(&result, "internal/search/external_test.go::check", "Paper");
    assert!(
        edges
            .iter()
            .all(|edge| kind_of(edge) != ResolutionKind::SamePackage),
        "`package search_test` is a different package block from `package \
         search`; it names `Paper` only through an import — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_file, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}

/// A vendored copy lives in another directory, so it is another package however
/// identical its source. The rung must not widen to it.
#[test]
fn a_vendored_duplicate_of_the_package_is_a_different_package() {
    let result = resolve(&[
        ("internal/search/provider.go", SEARCH_PROVIDER),
        ("internal/search/rank.go", SEARCH_RANK),
        ("vendor/example.com/dep/search/provider.go", SEARCH_PROVIDER),
    ]);

    let edges = edges_from(&result, "internal/search/rank.go::score", "Paper");
    assert_eq!(
        edges.len(),
        1,
        "the vendored `search` is a different directory and so a different \
         package — got {:?}",
        edges
            .iter()
            .map(|edge| &edge.target_file)
            .collect::<Vec<_>>()
    );
    assert_eq!(edges[0].target_file, "internal/search/provider.go");
}

/// A **method** is declared on a type, not in the package block, so a bare
/// `Render()` in a sibling file cannot reach `func (p Paper) Render()`. This is
/// the cross-file form of the fabricated-caller defect `bare_name_is_in_scope`
/// was written to stop, and the rung must not reintroduce it.
#[test]
fn a_method_on_a_type_is_not_reachable_by_a_bare_call_from_the_package() {
    const WITH_METHOD: &str = "\
package search

type Paper struct {
\tTitle string
}

func (p Paper) Render() string { return p.Title }
";
    const BARE_CALLER: &str = "\
package search

func show() string {
\treturn Render()
}
";
    let result = resolve(&[
        ("internal/search/provider.go", WITH_METHOD),
        ("internal/search/show.go", BARE_CALLER),
    ]);

    let edges = edges_from(&result, "internal/search/show.go::show", "Render");
    assert!(
        edges
            .iter()
            .all(|edge| kind_of(edge) != ResolutionKind::SamePackage),
        "`Render` is a method of `Paper`, not a package-level name; a bare call \
         cannot reach it — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_file, &edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}

/// The abstention. Two files of one package declaring the same package-level
/// name does not compile, but the resolver indexes whatever it is pointed at —
/// a generated file beside its source, a merge left half-applied. Where the
/// package block holds the name twice, this rung has no answer and must not
/// pick one.
#[test]
fn a_name_the_package_declares_twice_is_an_abstention_not_a_choice() {
    const ONE: &str = "\
package search

func Trim(raw string) string { return raw }
";
    const TWO: &str = "\
package search

func Trim(raw string) string { return raw + \"!\" }
";
    const CALLER: &str = "\
package search

func use(raw string) string { return Trim(raw) }
";
    let result = resolve(&[
        ("internal/search/one.go", ONE),
        ("internal/search/two.go", TWO),
        ("internal/search/caller.go", CALLER),
    ]);

    let edges = edges_from(&result, "internal/search/caller.go::use", "Trim");
    assert!(
        edges
            .iter()
            .all(|edge| kind_of(edge) != ResolutionKind::SamePackage),
        "the package block holds `Trim` twice; a deterministic rung that picked \
         one would be picking by input order — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_file, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}

/// The precision half of the qualifier rung, and the reason it runs *first*.
///
/// `t *testing.T` reduces to the bare `T` for dispatch. In a file that itself
/// declares a `T`, the same-file rung matched that bare name and bound a
/// foreign type to a local one at `DETERMINISTIC` — a confident edge to a
/// declaration the signature demonstrably does not name. The qualifier is
/// written in the source and says so.
#[test]
fn a_qualified_type_does_not_bind_to_a_same_named_declaration_of_this_file() {
    const SHADOWING: &str = "\
package api

import (
\t\"testing\"
)

type T struct {
\tN int
}

func check(t *testing.T) {
\tt.Helper()
}
";
    let result = resolve(&[("internal/api/check_test.go", SHADOWING)]);

    let edges = edges_from(&result, "internal/api/check_test.go::check", "T");
    assert!(
        edges.is_empty(),
        "`testing.T` is qualified by an import that names no indexed file; the \
         local `type T struct` is not what the signature says — got {:?}",
        edges
            .iter()
            .map(|edge| (&edge.target_file, &edge.target_symbol, kind_of(edge)))
            .collect::<Vec<_>>()
    );
}
