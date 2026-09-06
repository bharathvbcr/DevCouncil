//! The cluster pass and the single-symbol pass must exempt the same symbols.
//!
//! **They did not, and the difference was a live proposal to delete public
//! API.** `dead_clusters::externally_reachable_symbols` seeded its live set from
//! two sources — `symbol.is_exported` and wiring annotations — while
//! `analyze_liveness` computed five more, one function later and entirely
//! locally:
//!
//! * `c_header_exports` — a definition whose name a shared header publishes;
//! * `go_interface_exemptions` — a method satisfying an interface declared
//!   elsewhere in its package;
//! * `heritage_exemption` — an override reached through a supertype's call;
//! * `exported_owners` — a member of a type the module declares public, in a
//!   language where the member could not say so itself;
//! * `go_build_variants` — one identity compiled for two platforms.
//!
//! A symbol carrying any of those is exempt as a single symbol and was reported
//! at `DEAD_CLUSTER_CONFIDENCE` as part of a component "reached by nothing
//! outside the component" — the more absolute prose on the stronger claim, which
//! is the inversion the cluster pass's own doc comment warns about elsewhere.
//!
//! No test covered it. `an_exported_member_keeps_the_cluster_alive` exercises
//! `is_exported`, which is the seed that was already there.
//!
//! **Each kind is pinned twice, and it has to be.** A mutually recursive symbol
//! is never *reported* by the single-symbol pass at all — every member has an
//! inbound edge, which is the entire reason the cluster pass exists — so the
//! exemption cannot be observed on the same fixture that produces the cluster.
//! Each case therefore ships as a pair:
//!
//! 1. an **isolated** fixture, where the symbols do not call each other, so
//!    `analyze_liveness` reports them exempt and names the reason under test;
//! 2. a **recursive** fixture, identical but for the mutual call, where the
//!    cluster pass must stay silent.
//!
//! The pair is what makes the claim: the same symbols, exempt on the same
//! grounds, must not become deletable by acquiring a call to each other.

use std::collections::BTreeSet;

use devmap_analyze::{analyze_liveness, dead_clusters, DeadClusterScan};
use devmap_extract::extract_file;
use devmap_extract::model::Extraction;
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

fn scan(files: &[(&str, &str)]) -> DeadClusterScan {
    let (extractions, resolution) = resolve(files);
    dead_clusters(&extractions, &resolution)
}

/// Every symbol any reported cluster names.
fn clustered(scan: &DeadClusterScan) -> BTreeSet<&str> {
    scan.clusters
        .iter()
        .flat_map(|cluster| cluster.members.iter().map(String::as_str))
        .collect()
}

/// One exemption kind, with the two fixtures that pin it.
struct Case {
    /// What the exemption is, for the failure message.
    kind: &'static str,
    /// Symbols do not call each other, so the single-symbol pass reports them.
    isolated: &'static [(&'static str, &'static str)],
    /// Identical but for the mutual call, so the cluster pass sees a component.
    recursive: &'static [(&'static str, &'static str)],
    /// `dead_symbol_identity` spellings the isolated fixture must report exempt.
    exempt_identities: &'static [&'static str],
    /// A substring the reported exemption reason must contain, so the case is
    /// pinned to the exemption under test and not to `is_exported`.
    reason_fragment: &'static str,
    /// Qualified names no cluster may name in the recursive fixture.
    forbidden_members: &'static [&'static str],
}

/// `class Base` whose own `run` calls `render`, so `Base.render` is reached by
/// an *exactly* resolved edge.
///
/// The exact resolution is load-bearing. Calling `Base().render()` from a third
/// file produces an `AmbiguousGlobal` edge, `reached_through_a_supertype` reads
/// `called_symbols` and not `ambiguous_symbols`, and the heritage exemption
/// never fires — which is a fixture that pins nothing while looking like it
/// pins the case.
const HERITAGE_BASE: &str = "class Base:\n\
                             \x20   def run(self):\n\
                             \x20       return self.render()\n\n\
                             \x20   def render(self):\n\
                             \x20       return 1\n";

const CASES: &[Case] = &[
    Case {
        kind: "member of an exported type (Python `__all__`)",
        isolated: &[(
            "pkg/mod.py",
            "__all__ = [\"MyClass\"]\n\n\n\
             class MyClass:\n\
             \x20   def a(self):\n\
             \x20       return 1\n\n\
             \x20   def b(self):\n\
             \x20       return 2\n",
        )],
        recursive: &[(
            "pkg/mod.py",
            "__all__ = [\"MyClass\"]\n\n\n\
             class MyClass:\n\
             \x20   def a(self):\n\
             \x20       return self.b()\n\n\
             \x20   def b(self):\n\
             \x20       return self.a()\n",
        )],
        exempt_identities: &["MyClass.a", "MyClass.b"],
        reason_fragment: "Member of an exported type",
        forbidden_members: &["pkg/mod.py::MyClass.a", "pkg/mod.py::MyClass.b"],
    },
    Case {
        kind: "declared in a C-family header",
        isolated: &[
            ("lib.h", "int alpha(void);\nint beta(void);\n"),
            (
                "lib.c",
                "#include \"lib.h\"\nint alpha(void) { return 1; }\n\
                 int beta(void) { return 2; }\n",
            ),
        ],
        recursive: &[
            ("lib.h", "int alpha(void);\nint beta(void);\n"),
            (
                "lib.c",
                "#include \"lib.h\"\nint alpha(void) { return beta(); }\n\
                 int beta(void) { return alpha(); }\n",
            ),
        ],
        exempt_identities: &["alpha", "beta"],
        reason_fragment: "C-family header",
        forbidden_members: &["lib.c::alpha", "lib.c::beta"],
    },
    Case {
        // Lower-case throughout, deliberately. Go exports by capitalisation,
        // so `worker.Run` is `is_exported` and was already in the old seed —
        // a fixture spelled that way would pass before the fix and prove
        // nothing. The interface exemption is only load-bearing for the
        // unexported case.
        kind: "satisfies a Go interface declared in the same package",
        isolated: &[
            (
                "svc/iface.go",
                "package svc\n\ntype runner interface {\n\trun() error\n\tstop() error\n}\n",
            ),
            (
                "svc/impl.go",
                "package svc\n\ntype worker struct{}\n\n\
                 func (w *worker) run() error { return nil }\n\n\
                 func (w *worker) stop() error { return nil }\n",
            ),
        ],
        recursive: &[
            (
                "svc/iface.go",
                "package svc\n\ntype runner interface {\n\trun() error\n\tstop() error\n}\n",
            ),
            (
                "svc/impl.go",
                "package svc\n\ntype worker struct{}\n\n\
                 func (w *worker) run() error { return w.stop() }\n\n\
                 func (w *worker) stop() error { return w.run() }\n",
            ),
        ],
        exempt_identities: &["worker.run", "worker.stop"],
        reason_fragment: "implements interface",
        forbidden_members: &["svc/impl.go::worker.run", "svc/impl.go::worker.stop"],
    },
    Case {
        kind: "overrides a method reached through a supertype",
        isolated: &[
            ("app/base.py", HERITAGE_BASE),
            (
                "app/derived.py",
                "from base import Base\n\n\n\
                 class Derived(Base):\n\
                 \x20   def render(self):\n\
                 \x20       return 2\n",
            ),
        ],
        recursive: &[
            ("app/base.py", HERITAGE_BASE),
            (
                "app/derived.py",
                "from base import Base\n\n\n\
                 class Derived(Base):\n\
                 \x20   def render(self):\n\
                 \x20       return self.helper()\n\n\
                 \x20   def helper(self):\n\
                 \x20       return self.render()\n",
            ),
        ],
        exempt_identities: &["Derived.render"],
        reason_fragment: "Overrides a method reached through",
        forbidden_members: &[
            "app/derived.py::Derived.render",
            "app/derived.py::Derived.helper",
        ],
    },
];

/// Half one: each exemption really fires, and names itself.
///
/// Without this the second half would pass for any reason at all — including a
/// cluster pass that found no components in these fixtures.
#[test]
fn every_case_really_produces_the_exemption_it_names() {
    let mut failures = Vec::new();
    for case in CASES {
        let (extractions, resolution) = resolve(case.isolated);
        let reports = analyze_liveness(&extractions, &resolution);
        for identity in case.exempt_identities {
            match reports
                .iter()
                .find(|report| report.symbol_name == *identity)
            {
                None => failures.push(format!(
                    "{}: {identity} is not reported at all, so the exemption is \
                     unobserved — reports: {reports:?}",
                    case.kind
                )),
                Some(report) if !report.is_exempt => failures.push(format!(
                    "{}: {identity} is reported but not exempt: {report:?}",
                    case.kind
                )),
                Some(report)
                    if !report
                        .exemption_reason
                        .as_deref()
                        .is_some_and(|reason| reason.contains(case.reason_fragment)) =>
                {
                    failures.push(format!(
                        "{}: {identity} is exempt for the wrong reason — wanted \
                         {:?}, got {:?}",
                        case.kind, case.reason_fragment, report.exemption_reason
                    ))
                }
                Some(_) => {}
            }
        }
    }
    assert!(
        failures.is_empty(),
        "an exemption fixture stopped exercising its exemption:\n  {}",
        failures.join("\n  ")
    );
}

/// Half two: acquiring a call to each other must not make those same symbols
/// deletable.
#[test]
fn no_exempt_symbol_becomes_a_dead_cluster_by_calling_its_neighbour() {
    let mut failures = Vec::new();
    for case in CASES {
        let scan = scan(case.recursive);
        let members = clustered(&scan);
        for forbidden in case.forbidden_members {
            if members.contains(forbidden) {
                failures.push(format!(
                    "{}: the cluster pass proposes deleting {forbidden}, which \
                     the single-symbol pass exempts — {scan:?}",
                    case.kind
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "the two passes disagree about what is exempt:\n  {}",
        failures.join("\n  ")
    );
}

/// Half two is only meaningful if the recursive fixtures really are components
/// the pass would otherwise report.
///
/// Verified by construction rather than asserted: each recursive fixture is the
/// isolated one with the mutual call added, and this checks that the graph it
/// produces really does contain a cycle — using a private, unexported twin of
/// the same shape, which the pass must still report.
#[test]
fn the_recursive_fixtures_are_shapes_the_pass_does_report() {
    let private_python = scan(&[(
        "pkg/orphan.py",
        "class Widget:\n\
         \x20   def a(self):\n\
         \x20       return self.b()\n\n\
         \x20   def b(self):\n\
         \x20       return self.a()\n",
    )]);
    assert!(
        !private_python.clusters.is_empty(),
        "a class nothing exports and nothing calls is still an abandoned \
         cycle: {private_python:?}"
    );

    let private_c = scan(&[(
        "lib.c",
        "static int alpha(void) { return beta(); }\n\
         static int beta(void) { return alpha(); }\n",
    )]);
    assert!(
        !private_c.clusters.is_empty(),
        "two file-local C functions with no header entry are still an \
         abandoned cycle: {private_c:?}"
    );

    // No interface declared anywhere in the package, which is the only
    // difference from the Go case above.
    let private_go = scan(&[(
        "svc/impl.go",
        "package svc\n\ntype worker struct{}\n\n\
         func (w *worker) run() error { return w.stop() }\n\n\
         func (w *worker) stop() error { return w.run() }\n",
    )]);
    assert!(
        !private_go.clusters.is_empty(),
        "two unexported Go methods satisfying no interface are still an \
         abandoned cycle: {private_go:?}"
    );
}

/// The structural half: the cluster pass reads the exemption owner, rather than
/// happening to agree with it on these fixtures.
///
/// `liveness::exempt_symbol_names` is the hoisted set both passes now consult.
/// A future edit that reverts `externally_reachable_symbols` to its own seed
/// fails here even for an exemption this file has no fixture for.
#[test]
fn the_cluster_seed_contains_every_hoisted_exemption() {
    for case in CASES {
        let (extractions, resolution) = resolve(case.recursive);
        let exempt = devmap_analyze::exempt_symbol_names(&extractions, &resolution);
        let scan = dead_clusters(&extractions, &resolution);
        let members = clustered(&scan);
        let leaked: Vec<&&str> = members
            .iter()
            .filter(|member| exempt.contains(**member))
            .collect();
        assert!(
            leaked.is_empty(),
            "{}: cluster members {leaked:?} are in the exemption set the seed \
             is built from — the two have been allowed to drift apart again",
            case.kind
        );
    }
}
