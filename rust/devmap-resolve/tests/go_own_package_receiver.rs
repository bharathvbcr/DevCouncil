//! A bare Go type name means the caller's own package's type.
//!
//! `type_methods` is keyed by `(family, bare type, method)`, so when two
//! packages each declare `type Client struct{}` with a `Zzembed` method, the
//! key `(Go, "Client", "Zzembed")` holds two hits. The receiver rung refused
//! anything but exactly one, so `c := &Client{}; c.Zzembed()` in package `llm`
//! resolved to nothing — although Go's scoping leaves the bare `Client` one
//! meaning: the package block of the file it is written in.
//!
//! The preference is only sound for a spelling *written unqualified at the
//! call site*. The receiver rung reduces `rpc.Client{..}` to the same bare
//! `Client`, and a type reached through a field or a return value is spelled
//! relative to the package that declared *that*, not the caller's. Both stay
//! refused, and the tests below pin them.

use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
use devmap_resolve::model::{Resolution, ResolutionKind, ResolutionResult, ResolvedEdge};
use devmap_resolve::Resolver;

const GOMOD: &str = "module example.com/app\n\ngo 1.22\n";

const RPC_CLIENT: &str = "\
package rpc

type Client struct{}

func (c *Client) Zzembed() {}
";

const LLM_CLIENT: &str = "\
package llm

type Client struct{}

func (c *Client) Zzembed() {}
";

fn resolve(files: &[(&str, &str)]) -> ResolutionResult {
    let extractions: Vec<Extraction> = files
        .iter()
        .map(|(path, source)| extract_file(path, source))
        .collect();
    let mut resolver = Resolver::new();
    let module = devmap_extract::parse_go_mod("go.mod", GOMOD).expect("go.mod parses");
    resolver.index_go_modules(std::slice::from_ref(&module));
    resolver.index_extractions(&extractions);
    resolver.resolve_all(&extractions).unwrap()
}

/// Every edge out of `source_symbol` to a symbol whose tail is `Zzembed`.
fn zzembed_edges<'a>(result: &'a ResolutionResult, source_symbol: &str) -> Vec<&'a ResolvedEdge> {
    result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_symbol == source_symbol
                && edge
                    .target_symbol
                    .rsplit(['.', ':'])
                    .next()
                    .is_some_and(|tail| tail == "Zzembed")
        })
        .collect()
}

fn kind_of(edge: &ResolvedEdge) -> ResolutionKind {
    edge.resolution
        .as_deref()
        .map(Resolution::kind)
        .expect("an edge the resolver built carries its own evidence")
}

fn describe(edges: &[&ResolvedEdge]) -> Vec<(String, String, ResolutionKind)> {
    edges
        .iter()
        .map(|edge| {
            (
                edge.target_file.clone(),
                edge.target_symbol.clone(),
                kind_of(edge),
            )
        })
        .collect()
}

/// Asserts exactly one `ReceiverType` edge, to `llm/client.go`'s method.
fn assert_binds_to_llm(result: &ResolutionResult, source_symbol: &str) {
    let edges = zzembed_edges(result, source_symbol);
    assert_eq!(
        describe(&edges),
        vec![(
            "llm/client.go".to_string(),
            "llm/client.go::Client.Zzembed".to_string(),
            ResolutionKind::ReceiverType,
        )],
        "a bare `Client` written in package llm is llm's Client"
    );
}

#[test]
fn a_composite_literal_in_a_test_file_binds_to_its_own_packages_type() {
    const TEST: &str = "\
package llm

import \"testing\"

func TestX(t *testing.T) {
\tc := &Client{}
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/client_test.go", TEST),
    ]);
    assert_binds_to_llm(&result, "llm/client_test.go::TestX");
}

#[test]
fn a_composite_literal_in_a_sibling_file_binds_to_its_own_packages_type() {
    const USE: &str = "\
package llm

func Use() {
\tc := Client{}
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    assert_binds_to_llm(&result, "llm/use.go::Use");
}

#[test]
fn a_declared_parameter_type_binds_to_its_own_packages_type() {
    const USE: &str = "\
package llm

func Use(c *Client) {
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    assert_binds_to_llm(&result, "llm/use.go::Use");
}

/// The method value `c.Zzembed` is a reference, not a call, and goes through
/// the reference rung — which carried the same `hits.len() == 1` refusal.
#[test]
fn a_method_value_binds_to_its_own_packages_type() {
    const USE: &str = "\
package llm

func Use() func() {
\tc := &Client{}
\treturn c.Zzembed
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    assert_binds_to_llm(&result, "llm/use.go::Use");
}

/// A package that declares no `Client` has nothing for a bare `Client` to
/// prefer. (It does not compile, but the resolver indexes what it is given.)
#[test]
fn a_third_package_without_the_type_still_resolves_nothing() {
    const APP: &str = "\
package app

func Run() {
\tc := &Client{}
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("app/main.go", APP),
    ]);
    let edges = zzembed_edges(&result, "app/main.go::Run");
    assert!(
        edges.is_empty(),
        "package app declares no Client, so neither declaration is evidence: {:?}",
        describe(&edges)
    );
}

/// `rpc.Client{}` reduces to the same bare `Client` the rung keys on. The
/// qualifier says *rpc's* type, so llm's own `Client` must not win here.
#[test]
fn a_qualified_composite_literal_never_binds_to_the_callers_own_type() {
    const USE: &str = "\
package llm

import \"example.com/app/rpc\"

func Use() {
\tc := &rpc.Client{}
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges.iter().any(|edge| edge.target_file == "llm/client.go"),
        "`rpc.Client` is not llm's Client: {:?}",
        describe(&edges)
    );
}

/// The same for a declared type: `c *rpc.Client`.
#[test]
fn a_qualified_parameter_type_never_binds_to_the_callers_own_type() {
    const USE: &str = "\
package llm

import \"example.com/app/rpc\"

func Use(c *rpc.Client) {
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges.iter().any(|edge| edge.target_file == "llm/client.go"),
        "`*rpc.Client` is not llm's Client: {:?}",
        describe(&edges)
    );
}

/// An alias hides the package name from the local name, so `r.Client{}` must
/// still refuse: the import's specifier ends in the competing package's
/// directory.
#[test]
fn an_aliased_qualified_literal_never_binds_to_the_callers_own_type() {
    const USE: &str = "\
package llm

import r \"example.com/app/rpc\"

func Use() {
\tc := &r.Client{}
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges.iter().any(|edge| edge.target_file == "llm/client.go"),
        "`r.Client` is rpc's Client: {:?}",
        describe(&edges)
    );
}

/// A dot-import puts rpc's `Client` in this file's scope too, so even a bare
/// `Client` has two meanings here.
#[test]
fn a_dot_import_of_a_competing_package_refuses() {
    const USE: &str = "\
package llm

import . \"example.com/app/rpc\"

func Use() {
\tc := &Client{}
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges.iter().any(|edge| edge.target_file == "llm/client.go"),
        "a dot-imported rpc makes the bare `Client` ambiguous: {:?}",
        describe(&edges)
    );
}

/// The import guard is about the *competing* packages. Importing something
/// that declares no `Client` leaves the bare spelling one meaning.
#[test]
fn an_unrelated_import_does_not_block_the_own_package_type() {
    const UTIL: &str = "\
package util

func Trim(s string) string { return s }
";
    const USE: &str = "\
package llm

import (
\t\"strings\"

\t\"example.com/app/util\"
)

func Use(raw string) string {
\tc := &Client{}
\tc.Zzembed()
\treturn util.Trim(strings.TrimSpace(raw))
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("util/util.go", UTIL),
        ("llm/use.go", USE),
    ]);
    assert_binds_to_llm(&result, "llm/use.go::Use");
}

/// Found by the A/B on Manvi: `NewRecord(inner llm.Provider)` calling
/// `inner.Name()`. `llm.Provider` is an interface, so its package holds no
/// `type_methods` hit at all — a guard that looked only for competing *hits*
/// let the reduced `Provider` bind to the caller's own `Provider.Name`.
#[test]
fn a_qualified_interface_parameter_never_binds_to_the_callers_own_type() {
    const LLM: &str = "\
package llm

type Provider interface {
\tName() string
}
";
    const BUDGET: &str = "\
package budget

type Provider struct{}

func (p *Provider) Name() string { return \"\" }
";
    const REPLAY: &str = "\
package replay

import \"example.com/app/llm\"

type Provider struct{}

func (p *Provider) Name() string { return \"\" }

func NewRecord(inner llm.Provider) string {
\treturn inner.Name()
}
";
    let result = resolve(&[
        ("llm/provider.go", LLM),
        ("llm/budget/budget.go", BUDGET),
        ("llm/replay/replay.go", REPLAY),
    ]);
    let wrong: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| {
            edge.source_symbol == "llm/replay/replay.go::NewRecord"
                && edge.target_symbol == "llm/replay/replay.go::Provider.Name"
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "`inner` is an llm.Provider, not replay's: {:?}",
        describe(&wrong)
    );
}

/// The shape the A/B gained most from: a method calling a sibling method on
/// its own receiver, when another package's type of the same name declares a
/// method of the same name too.
#[test]
fn a_method_receiver_binds_to_its_own_packages_type() {
    const LLM: &str = "\
package llm

type Client struct{}

func (c *Client) Zzembed() {}

func (c *Client) Act() {
\tc.Zzembed()
}
";
    let result = resolve(&[("rpc/client.go", RPC_CLIENT), ("llm/client.go", LLM)]);
    assert_binds_to_llm(&result, "llm/client.go::Client.Act");
}

/// Importing the competing package is not itself a reason to refuse: the
/// written spelling is bare, and that is what Go reads.
#[test]
fn importing_the_competing_package_does_not_block_a_bare_spelling() {
    const RPC: &str = "\
package rpc

type Client struct{}

func (c *Client) Zzembed() {}

func Version() string { return \"\" }
";
    const USE: &str = "\
package llm

import \"example.com/app/rpc\"

func Use() string {
\tc := &Client{}
\tc.Zzembed()
\treturn rpc.Version()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC),
        ("llm/client.go", LLM_CLIENT),
        ("llm/use.go", USE),
    ]);
    assert_binds_to_llm(&result, "llm/use.go::Use");
}

/// A type reached through a field is spelled relative to the package that
/// declared the field. `holder.Inner` is `*rpc.Client` in package svc, so a
/// caller in llm reading `h.Inner.Zzembed()` must not get llm's method.
#[test]
fn a_field_typed_in_another_package_never_binds_to_the_callers_own_type() {
    const SVC: &str = "\
package svc

import \"example.com/app/rpc\"

type Holder struct {
\tInner *rpc.Client
}
";
    const USE: &str = "\
package llm

import \"example.com/app/svc\"

func Use(h *svc.Holder) {
\th.Inner.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("svc/holder.go", SVC),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges.iter().any(|edge| edge.target_file == "llm/client.go"),
        "a field declared `*rpc.Client` in svc is not llm's Client: {:?}",
        describe(&edges)
    );
}

/// The sharper field case: the struct is llm's own, but its field is spelled
/// `*rpc.Client` in a sibling file that imports rpc. The caller imports
/// nothing, so only the receiver-is-a-path refusal stands between
/// `h.Inner.Zzembed()` and llm's method.
#[test]
fn a_same_package_field_typed_by_another_package_never_binds_to_the_callers_own_type() {
    const HOLDER: &str = "\
package llm

import \"example.com/app/rpc\"

type Holder struct {
\tInner *rpc.Client
}
";
    const USE: &str = "\
package llm

func Use(h *Holder) {
\th.Inner.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("llm/holder.go", HOLDER),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges.iter().any(|edge| edge.target_file == "llm/client.go"),
        "`Holder.Inner` is declared `*rpc.Client`: {:?}",
        describe(&edges)
    );
}

/// A type learned from a call's return is spelled in the callee's package.
/// `svc.Make()` returns `*rpc.Client`; the caller imports svc, not rpc, so the
/// import guard cannot see the competitor — nothing in this file wrote
/// `Client` for `c`, and that is what refuses.
#[test]
fn a_type_returned_from_another_package_never_binds_to_the_callers_own_type() {
    const SVC: &str = "\
package svc

import \"example.com/app/rpc\"

func Make() *rpc.Client { return &rpc.Client{} }
";
    const USE: &str = "\
package llm

import \"example.com/app/svc\"

func Use() {
\tc := svc.Make()
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_CLIENT),
        ("svc/make.go", SVC),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges.iter().any(|edge| edge.target_file == "llm/client.go"),
        "`svc.Make()` returns rpc's Client: {:?}",
        describe(&edges)
    );
}

/// A non-test file cannot see a method a `_test.go` file declares, so a
/// test-only declaration is not the caller's own type's method.
#[test]
fn a_non_test_caller_does_not_bind_to_a_test_only_method() {
    const LLM_TYPE: &str = "\
package llm

type Client struct{}
";
    const LLM_TEST_METHOD: &str = "\
package llm

func (c *Client) Zzembed() {}
";
    const USE: &str = "\
package llm

func Use() {
\tc := &Client{}
\tc.Zzembed()
}
";
    let result = resolve(&[
        ("rpc/client.go", RPC_CLIENT),
        ("llm/client.go", LLM_TYPE),
        ("llm/helpers_test.go", LLM_TEST_METHOD),
        ("llm/use.go", USE),
    ]);
    let edges = zzembed_edges(&result, "llm/use.go::Use");
    assert!(
        !edges
            .iter()
            .any(|edge| edge.target_file == "llm/helpers_test.go"),
        "the ordinary build of llm/use.go does not contain helpers_test.go: {:?}",
        describe(&edges)
    );
}
